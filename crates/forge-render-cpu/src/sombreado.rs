//! Iluminación: armónicos esféricos, BRDF y exposición medida.
//!
//! Todo en `f32` y en radiancia lineal Rec.709. El mapeo a píxel es cosa de
//! [`crate::agx`]; aquí no se recorta nada, porque recortar antes del mapeo de
//! tono es exactamente lo que hace que un render parezca de plástico.

use crate::malla::CpuMaterial;
use forge_math::DVec3;
use forge_render_api::{Ibl, Light, SceneView};

/// Coeficientes de la evaluación de irradiancia de Ramamoorthi–Hanrahan.
const C1: f32 = 0.429_043;
const C2: f32 = 0.511_664;
const C3: f32 = 0.743_125;
const C4: f32 = 0.886_227;
const C5: f32 = 0.247_708;

/// `Y00`, para pasar del coeficiente `l=0` a radiancia media.
const Y00: f32 = 0.282_095;

/// Rota una dirección `-rotation_rad` alrededor de `+Z`.
///
/// Z y no Y: el entorno gira sobre el eje vertical, y aquí el eje vertical es Z.
/// El signo es negativo porque girar el entorno `+θ` equivale a consultar la
/// dirección girada `-θ`.
#[inline]
fn desrotar(d: [f32; 3], rot: f32) -> [f32; 3] {
    let (s, c) = (-rot).sin_cos();
    [c * d[0] - s * d[1], s * d[0] + c * d[1], d[2]]
}

/// Radiancia del entorno en una dirección: evaluación directa de los 9 SH.
///
/// Se usa para el fondo y para el reflejo especular. **Con `max(·, 0)`**: el
/// ringing de armónicos esféricos da valores negativos en entornos con mucho
/// contraste, y una radiancia negativa se ve como un agujero negro que parece un
/// bug de geometría y no de iluminación.
pub fn radiancia_sh(ibl: &Ibl, dir: DVec3) -> [f32; 3] {
    let d = dir.normalize_or_zero();
    let [x, y, z] = desrotar([d.x as f32, d.y as f32, d.z as f32], ibl.rotation_rad);
    let l = &ibl.sh;

    // Base real de SH hasta l=2, en el orden que documenta `Ibl`:
    // l=0; l=1 (y, z, x); l=2 (xy, yz, 3z²-1, xz, x²-y²).
    let b = [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ];

    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let mut v = 0.0;
        for i in 0..9 {
            v += l[i][c] * b[i];
        }
        out[c] = (v * ibl.intensity).max(0.0);
    }
    out
}

/// Irradiancia difusa `E(n)` en W/m² a partir de los SH de radiancia.
///
/// Es la forma cerrada de Ramamoorthi–Hanrahan: convolucionar la radiancia con
/// el coseno cuesta un cubemap; con 9 coeficientes cuesta 20 multiplicaciones y
/// el error en entornos de estudio queda por debajo del 1 %.
///
/// **`max(E, 0)` obligatorio.** Igual que arriba, y aquí es peor: una
/// irradiancia negativa recorre toda la difusa de la cara.
pub fn irradiancia_sh(ibl: &Ibl, n: DVec3) -> [f32; 3] {
    let d = n.normalize_or_zero();
    let [x, y, z] = desrotar([d.x as f32, d.y as f32, d.z as f32], ibl.rotation_rad);
    let l = &ibl.sh;
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let (l00, l1m1, l10, l11) = (l[0][c], l[1][c], l[2][c], l[3][c]);
        let (l2m2, l2m1, l20, l21, l22) = (l[4][c], l[5][c], l[6][c], l[7][c], l[8][c]);
        let e = C1 * l22 * (x * x - y * y) + C3 * l20 * z * z + C4 * l00 - C5 * l20
            + 2.0 * C1 * (l2m2 * x * y + l21 * x * z + l2m1 * y * z)
            + 2.0 * C2 * (l11 * x + l1m1 * y + l10 * z);
        out[c] = (e * ibl.intensity).max(0.0);
    }
    out
}

/// Irradiancia media sobre todas las direcciones.
///
/// Los términos con `l >= 1` promedian cero sobre la esfera, así que la media es
/// exactamente `C4 * L00`. Es un número exacto, no una integración numérica.
pub fn irradiancia_media(ibl: &Ibl) -> [f32; 3] {
    [
        (C4 * ibl.sh[0][0] * ibl.intensity).max(0.0),
        (C4 * ibl.sh[0][1] * ibl.intensity).max(0.0),
        (C4 * ibl.sh[0][2] * ibl.intensity).max(0.0),
    ]
}

/// SH de un entorno de radiancia constante `l`. Solo el término `l=0`.
///
/// `L00 = l / Y00` porque `∫ l · Y00 dω = l · Y00 · 4π = l / Y00`.
pub fn sh_constante(l: [f32; 3]) -> [[f32; 3]; 9] {
    let mut sh = [[0.0f32; 3]; 9];
    sh[0] = [l[0] / Y00, l[1] / Y00, l[2] / Y00];
    sh
}

#[inline]
fn luminancia(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Exposición **medida** de la escena.
///
/// `π / irradiancia_media`: con eso una superficie difusa blanca bajo el entorno
/// sale a 1.0, que es el ancla que hace que cambiar las luces no invalide en
/// silencio ningún número escondido en el código.
///
/// Las luces puntuales y direccionales entran con su irradiancia nominal sobre
/// una superficie encarada. Es una aproximación —no sabe a qué distancia está la
/// geometría— y por eso está aquí documentada y no repartida por el shader.
pub fn exposicion_medida(view: &SceneView<'_>) -> f32 {
    if let Some(e) = view.exposure {
        return e;
    }
    let mut e_media = 0.0f32;
    if let Some(ibl) = view.environment {
        e_media += luminancia(irradiancia_media(ibl));
    }
    for l in view.lights {
        e_media += match *l {
            Light::Directional { color, intensity, .. } => luminancia(color) * intensity,
            Light::Point { color, intensity, .. } => luminancia(color) * intensity,
        };
    }
    if e_media > 1e-6 {
        std::f32::consts::PI / e_media
    } else {
        // Escena sin iluminación: cualquier exposición da negro. 1.0 evita
        // infinitos y deja el resto del pipeline verificable.
        1.0
    }
}

// ---------------------------------------------------------------------------
// BRDF
// ---------------------------------------------------------------------------

/// Fresnel de Schlick con rugosidad (Lagarde): sin el `max(1-rugosidad, F0)` los
/// bordes de un objeto rugoso se ponen blancos y parecen un halo.
#[inline]
fn fresnel_rugoso(cos_theta: f32, f0: [f32; 3], rugosidad: f32) -> [f32; 3] {
    let f = (1.0 - cos_theta.clamp(0.0, 1.0)).powi(5);
    let mut o = [0.0f32; 3];
    for i in 0..3 {
        let techo = (1.0 - rugosidad).max(f0[i]);
        o[i] = f0[i] + (techo - f0[i]) * f;
    }
    o
}

/// Aproximación analítica del término DFG de la suma separada (Karis).
///
/// Devuelve `(A, B)` tales que la reflectancia especular del entorno es
/// `F0·A + B`. **No conserva energía**: es un único rebote, así que a rugosidad
/// alta pierde energía de verdad. Esa pérdida es la que mide el test del horno
/// blanco, y no es un bug del rasterizador sino el límite del modelo.
#[inline]
pub fn env_brdf(n_dot_v: f32, rugosidad: f32) -> (f32, f32) {
    let n_dot_v = n_dot_v.clamp(0.0, 1.0);
    let c0 = [-1.0f32, -0.0275, -0.572, 0.022];
    let c1 = [1.0f32, 0.0425, 1.04, -0.04];
    let r = [
        rugosidad * c0[0] + c1[0],
        rugosidad * c0[1] + c1[1],
        rugosidad * c0[2] + c1[2],
        rugosidad * c0[3] + c1[3],
    ];
    let a004 = (r[0] * r[0]).min((-9.28 * n_dot_v).exp2()) * r[0] + r[1];
    (-1.04 * a004 + r[2], 1.04 * a004 + r[3])
}

/// Exponente de Blinn-Phong equivalente a una rugosidad GGX.
#[inline]
fn brillo(rugosidad: f32) -> f32 {
    let a = (rugosidad.clamp(0.02, 1.0)).powi(2);
    (2.0 / (a * a) - 2.0).clamp(1.0, 1.0e6)
}

/// Radiancia saliente en un punto.
///
/// `n` y `v` normalizados y en mundo; `v` apunta del punto **hacia** la cámara.
pub fn sombrear(
    p: DVec3,
    n: DVec3,
    v: DVec3,
    mat: &CpuMaterial,
    lights: &[Light],
    env: Option<&Ibl>,
) -> [f32; 3] {
    let rug = mat.roughness.clamp(0.02, 1.0);
    let f0 = mat.f0();
    let kd_metal = 1.0 - mat.metallic.clamp(0.0, 1.0);
    let n_dot_v = n.dot(v).max(0.0) as f32;
    let albedo = mat.base_color;

    let mut out = [0.0f32; 3];

    // --- luces analíticas: Lambert + Blinn-Phong -----------------------------
    let esp = brillo(rug);
    let norm_esp = (esp + 8.0) / (8.0 * std::f32::consts::PI);
    for luz in lights {
        let (dir_l, radiancia) = match *luz {
            Light::Directional { direction, color, intensity } => {
                // `direction` es hacia dónde viaja la luz; la BRDF quiere el
                // vector hacia la fuente.
                let d = -direction.normalize_or_zero();
                (d, [color[0] * intensity, color[1] * intensity, color[2] * intensity])
            }
            Light::Point { position, color, intensity, radius_mm } => {
                let delta = position - p;
                let dist = delta.length();
                if dist < 1e-9 {
                    continue;
                }
                // Caída cuadrática con la distancia en metros, y el radio de la
                // fuente como distancia mínima: sin esa cota, un punto sobre la
                // propia luz da infinito.
                let d_m = (dist.max(radius_mm.max(1e-3)) / 1000.0) as f32;
                let att = 1.0 / (d_m * d_m);
                (
                    delta / dist,
                    [color[0] * intensity * att, color[1] * intensity * att, color[2] * intensity * att],
                )
            }
        };
        let n_dot_l = n.dot(dir_l).max(0.0) as f32;
        if n_dot_l <= 0.0 {
            continue;
        }
        let h = (dir_l + v).normalize_or_zero();
        let n_dot_h = n.dot(h).max(0.0) as f32;
        let f = fresnel_rugoso(dir_l.dot(h).max(0.0) as f32, f0, rug);
        let s = norm_esp * n_dot_h.powf(esp);
        for i in 0..3 {
            let difusa = kd_metal * (1.0 - f[i]) * albedo[i] / std::f32::consts::PI;
            out[i] += (difusa + f[i] * s) * radiancia[i] * n_dot_l;
        }
    }

    // --- entorno ------------------------------------------------------------
    if let Some(ibl) = env {
        let e = irradiancia_sh(ibl, n); // ya lleva max(·, 0)
        let f = fresnel_rugoso(n_dot_v, f0, rug);
        let (a, b) = env_brdf(n_dot_v, rug);
        // Reflejo especular sobre la dirección espejo. Sin cubemap prefiltrado
        // se muestrea la propia SH: es exacto para entornos de baja frecuencia
        // (que es para lo que sirve una SH) y honesto para el resto.
        let r = (2.0 * n.dot(v) * n - v).normalize_or_zero();
        let l_esp = radiancia_sh(ibl, r);
        for i in 0..3 {
            out[i] += kd_metal * (1.0 - f[i]) * albedo[i] * e[i] / std::f32::consts::PI;
            out[i] += l_esp[i] * (f0[i] * a + b);
        }
    }

    out
}
