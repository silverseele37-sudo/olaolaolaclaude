//! AgX: el mapeo de tono que convierte radiancia en píxeles.
//!
//! Se elige AgX y no Reinhard/ACES por una razón medible: en una escena CAD con
//! un foco especular sobre metal, Reinhard desatura y ACES vira el naranja hacia
//! el amarillo. AgX mantiene el tono al desaturar hacia blanco, que es lo que
//! hace que un render de producto no parezca «de videojuego».
//!
//! ## Por qué el *outset* no es la inversa del *inset*
//!
//! El inset comprime las primarias hacia el blanco antes de la curva; el outset
//! las vuelve a abrir después. Si el outset fuera exactamente `inverse(inset)`,
//! el par sería una identidad alrededor de la curva y **desaparecería el
//! desplazamiento de tono** que es justamente el look de AgX (el rojo saturado
//! que se va a naranja al quemarse, en vez de recortarse a rojo puro).
//!
//! Blender usa un outset deliberadamente distinto del inverso del inset. Aquí se
//! reproduce ese desajuste tal cual. Hay un test (`agx.rs`) que comprueba que
//! `outset * inset != I`: si alguien «arregla» esto con `inverse(inset)`, el test
//! falla y explica por qué.
//!
//! ## Las matrices
//!
//! Las matrices de abajo son las de Blender/Filament tal como circulan en
//! Filament y three.js: llevan **plegada** la conversión de primarias
//! Rec.709 → Rec.2020, de modo que la entrada es RGB lineal Rec.709 y la curva
//! opera sobre Rec.2020 comprimido, que es lo que pide la especificación.

/// Rango logarítmico de la codificación, en stops.
///
/// No son números mágicos: `-12.47393 = log2(0.18) - 10` y
/// `4.026069 = log2(0.18) + 6.5`. Es decir, 10 stops por debajo del gris medio
/// y 6.5 por encima. Se dejan como constantes derivadas comprobables en test.
pub const MIN_EV: f32 = -12.473_93;
pub const MAX_EV: f32 = 4.026_069;

/// Gris medio: el ancla de todo el rango.
pub const GRIS_MEDIO: f32 = 0.18;

/// Inset (Rec.709 lineal → AgX comprimido), por filas.
pub const INSET: [[f32; 3]; 3] = [
    [0.856_627_15, 0.137_318_97, 0.111_898_21],
    [0.095_121_24, 0.761_242_0, 0.076_799_42],
    [0.048_251_606, 0.101_439_04, 0.811_302_4],
];

/// Outset. **No** es `inverse(INSET)`; ver la nota del módulo.
pub const OUTSET: [[f32; 3]; 3] = [
    [1.127_100_6, -0.141_329_76, -0.141_329_76],
    [-0.110_606_64, 1.157_823_7, -0.110_606_64],
    [-0.016_493_939, -0.016_493_939, 1.251_936_4],
];

#[inline]
fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Producto de matrices 3x3, expuesto para que el test pueda comprobar que
/// `OUTSET · INSET` **no** es la identidad.
pub fn mul_mat3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut o = [[0.0f32; 3]; 3];
    for (i, fila) in o.iter_mut().enumerate() {
        for (j, c) in fila.iter_mut().enumerate() {
            *c = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

/// Aproximación polinómica de grado 6 de la sigmoide de contraste de AgX.
///
/// Es la que se ejecuta por píxel. Su error contra la sigmoide analítica está
/// acotado **por los dos lados** en el test: un límite superior detecta que
/// alguien la empeoró; el inferior detecta que alguien la «mejoró» y con ello
/// descalibró todo lo que se sintonizó mirando esta curva.
#[inline]
pub fn contraste_polinomico(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x
        - 0.002_32
}

// ---------------------------------------------------------------------------
// La sigmoide analítica de referencia
// ---------------------------------------------------------------------------
//
// AgX define su curva como dos ramas hiperbólicas (pie y hombro) unidas en el
// pivote con pendiente continua. El polinomio de arriba es una aproximación de
// *esta* función; tenerla implementada es lo que convierte el test de AgX en un
// test de respuesta conocida y no en «los píxeles no cambiaron».

/// Pivote en x: la posición del gris medio dentro del rango log normalizado.
/// `(log2(0.18) - MIN_EV) / (MAX_EV - MIN_EV)` = 10 / 16.5.
pub const PIVOTE_X: f32 = 10.0 / 16.5;
pub const PIVOTE_Y: f32 = 0.5;

/// Pendiente en el pivote de la curva de referencia.
///
/// **Medida, no copiada.** La derivada del polinomio en el pivote vale 2.069
/// (lo comprueba `agx.rs`), no los 2.4 que suele citarse para la curva nominal
/// de AgX. El polinomio de grado 6 que circula aproxima una curva de contraste
/// algo más suave que la nominal; usar 2.4 aquí daría un error de 0.0335 y
/// convertiría el test en un test de «el polinomio está mal», que no es cierto.
/// El test guarda ese contraste explícitamente.
pub const PENDIENTE: f32 = 2.0;
pub const POTENCIA_PIE: f32 = 3.0;
pub const POTENCIA_HOMBRO: f32 = 3.0;

fn escala(x_pivote: f32, y_pivote: f32, pendiente: f32, potencia: f32) -> f32 {
    let a = (pendiente * x_pivote).powf(-potencia);
    let b = (pendiente * (x_pivote / y_pivote)).powf(potencia) - 1.0;
    (a * b).powf(-1.0 / potencia)
}

#[inline]
fn hiperbolica(x: f32, potencia: f32) -> f32 {
    x / (1.0 + x.powf(potencia)).powf(1.0 / potencia)
}

/// Sigmoide analítica de AgX sobre el rango log normalizado `[0, 1]`.
///
/// Dos ramas hiperbólicas —pie y hombro— unidas en el pivote con pendiente
/// continua. Es la forma cerrada que define AgX; el polinomio es su
/// aproximación barata.
pub fn contraste_analitico(x: f32) -> f32 {
    contraste_analitico_con(x, PENDIENTE, POTENCIA_PIE, POTENCIA_HOMBRO)
}

/// La misma curva con parámetros explícitos, para que el test pueda medir el
/// error contra otras lecturas de la especificación y no solo contra la elegida.
pub fn contraste_analitico_con(x: f32, pendiente: f32, pie: f32, hombro: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let s_pie = -escala(PIVOTE_X, PIVOTE_Y, pendiente, pie);
    let s_hombro = escala(1.0 - PIVOTE_X, 1.0 - PIVOTE_Y, pendiente, hombro);
    let (s, p) = if x < PIVOTE_X { (s_pie, pie) } else { (s_hombro, hombro) };
    let t = pendiente * (x - PIVOTE_X) / s;
    (s * hiperbolica(t, p) + PIVOTE_Y).clamp(0.0, 1.0)
}

/// Derivada del polinomio, para comprobar la pendiente en el pivote.
#[inline]
pub fn derivada_polinomica(x: f32) -> f32 {
    let x2 = x * x;
    let x3 = x2 * x;
    let x4 = x2 * x2;
    93.0 * x4 * x - 200.7 * x4 + 127.84 * x3 - 20.604 * x2 + 0.8596 * x + 0.1191
}

/// AgX completo: radiancia lineal Rec.709 → valor lineal listo para la OETF.
///
/// Devuelve **lineal**, no sRGB: la codificación a byte la hace el destino, que
/// es lo único que sabe si el formato es sRGB o no.
pub fn agx(rgb: [f32; 3]) -> [f32; 3] {
    let v = mul3(&INSET, [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)]);

    // Codificación log2 normalizada al rango de la especificación.
    let mut n = [0.0f32; 3];
    for i in 0..3 {
        // 1e-10 y no 0: log2(0) es -inf y contamina el resto del canal.
        let l = v[i].max(1e-10).log2();
        n[i] = ((l - MIN_EV) / (MAX_EV - MIN_EV)).clamp(0.0, 1.0);
    }

    let c = [
        contraste_polinomico(n[0]),
        contraste_polinomico(n[1]),
        contraste_polinomico(n[2]),
    ];

    let o = mul3(&OUTSET, c);
    // El resultado del outset está en un espacio con gamma ~2.2 implícita; se
    // linealiza para que la OETF de salida no aplique la corrección dos veces.
    [o[0].max(0.0).powf(2.2), o[1].max(0.0).powf(2.2), o[2].max(0.0).powf(2.2)]
}

/// OETF de sRGB. Separada de `agx` a propósito: el rasterizador escribe RGBA8
/// sRGB, pero un destino HDR usaría la misma `agx` sin esta función.
#[inline]
pub fn srgb_oetf(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// Cuantización a byte. Redondeo al más cercano, determinista.
#[inline]
pub fn a_byte(x: f32) -> u8 {
    (srgb_oetf(x) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}
