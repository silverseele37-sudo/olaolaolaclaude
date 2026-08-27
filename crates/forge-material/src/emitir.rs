//! Traducción del [`Analisis`] a texto WGSL.
//!
//! Este módulo asume que el grafo ya es válido: no vuelve a comprobar tipos ni
//! ciclos. Su única responsabilidad es imprimir, de forma determinista, un
//! `let` por nodo en orden topológico y una función que devuelve
//! `SurfaceProperties`.

use std::collections::HashMap;
use std::fmt::Write as _;

use forge_material_api::{MaterialGraph, Node, NodeKind, Value};

use crate::analisis::{tipo_salida, Analisis, Entrada};

/// Genera el módulo WGSL completo para `graph`, usando la resolución de
/// `analisis`.
///
/// Determinista por construcción: recorre `analisis.orden`, que es un
/// `Vec` con un orden fijo, nunca un `HashMap` u otra colección cuyo orden de
/// iteración pudiera variar. Sin esto, "generar el mismo grafo dos veces da el
/// mismo texto" dejaría de cumplirse y no habría caché de shaders posible.
pub(crate) fn emitir(
    graph: &MaterialGraph,
    analisis: &Analisis,
    nodos: &HashMap<u32, &Node>,
) -> String {
    let (uv, normal, position) = parametros_necesarios(analisis, nodos);

    let mut parametros = Vec::new();
    if uv {
        parametros.push("uv: vec2<f32>");
    }
    if normal {
        parametros.push("normal: vec3<f32>");
    }
    if position {
        parametros.push("position: vec3<f32>");
    }

    let mut w = String::new();
    // Comentario fijo, sin nada variable: no debe romper el byte-a-byte del
    // test de determinismo.
    w.push_str("// Generado por forge-material. No editar a mano.\n");
    w.push_str("struct SurfaceProperties {\n");
    w.push_str("    base_color: vec3<f32>,\n");
    w.push_str("    metallic: f32,\n");
    w.push_str("    roughness: f32,\n");
    w.push_str("    emissive: vec3<f32>,\n");
    w.push_str("}\n\n");
    let _ = writeln!(
        w,
        "fn material_surface({}) -> SurfaceProperties {{",
        parametros.join(", ")
    );

    for &id in &analisis.orden {
        let nodo = nodos[&id];
        let resueltas = &analisis.entradas[&id];
        let expr = expresion(nodo, resueltas);
        let tipo = tipo_wgsl_declarado(nodo);
        let _ = writeln!(w, "    let n{id}: {tipo} = {expr};");
    }

    let salida = graph
        .output
        .expect("emitir() solo se llama tras analizar() con éxito, que garantiza una salida");
    let _ = writeln!(w, "    return n{salida};");
    w.push_str("}\n");
    w
}

/// Qué parámetros (uv, normal, posición) necesita de verdad el subconjunto de
/// nodos usado. Un grafo que no lee `TexCoord` no debe obligar a la función
/// generada a declarar un parámetro `uv` que nunca se usa.
fn parametros_necesarios(analisis: &Analisis, nodos: &HashMap<u32, &Node>) -> (bool, bool, bool) {
    let (mut uv, mut normal, mut position) = (false, false, false);
    for &id in &analisis.orden {
        match nodos[&id].kind {
            NodeKind::TexCoord => uv = true,
            NodeKind::Normal => normal = true,
            NodeKind::Position => position = true,
            _ => {}
        }
    }
    (uv, normal, position)
}

/// Tipo WGSL declarado para el `let` de un nodo. Coincide con
/// `tipo_salida(nodo).wgsl()` salvo para la salida, cuyo "tipo" es en
/// realidad la estructura `SurfaceProperties` y no uno de los cuatro
/// [`SocketType`].
fn tipo_wgsl_declarado(nodo: &Node) -> &'static str {
    if let NodeKind::StandardSurface = nodo.kind {
        "SurfaceProperties"
    } else {
        tipo_salida(nodo).wgsl()
    }
}

/// Expresión WGSL para un nodo, dadas sus entradas ya resueltas.
fn expresion(nodo: &Node, resueltas: &[Entrada]) -> String {
    // Referencia a la entrada `i`: o bien el nombre de variable del nodo del
    // que viene, o el literal WGSL de su valor por defecto.
    let e = |i: usize| -> String {
        match resueltas[i] {
            Entrada::Nodo(id) => format!("n{id}"),
            Entrada::Literal(v) => literal_wgsl(v),
        }
    };

    match nodo.kind {
        NodeKind::Constant => {
            let v = nodo
                .constant
                .expect("analizar() rechaza con MissingInput una Constant sin valor");
            literal_wgsl(v)
        }
        NodeKind::TexCoord => "uv".to_string(),
        NodeKind::Normal => "normal".to_string(),
        NodeKind::Position => "position".to_string(),
        NodeKind::Add => format!("{} + {}", e(0), e(1)),
        NodeKind::Multiply => format!("{} * {}", e(0), e(1)),
        // `mix(vecN, vecN, escalar)` es una sobrecarga válida de WGSL: el
        // factor de mezcla no hace falta ensancharlo a vector.
        NodeKind::Mix => format!("mix({}, {}, {})", e(0), e(1), e(2)),
        // A diferencia de `mix`, `clamp` en WGSL exige que sus tres
        // argumentos compartan tipo exacto (no hay sobrecarga vector+escalar),
        // así que `low`/`high` —declarados `Float` en el contrato— se
        // ensanchan aquí con el constructor de un solo argumento, que en WGSL
        // repite el escalar en las tres componentes.
        NodeKind::Clamp => format!("clamp({}, vec3<f32>({}), vec3<f32>({}))", e(0), e(1), e(2)),
        // La trampa documentada: `pow` en WGSL (como en GLSL/HLSL) es
        // comportamiento indefinido cuando la base es negativa y el
        // exponente no es un entero. Se protege recortando la base a
        // `[0, +inf)` antes de elevarla. Por el mismo motivo que `clamp`,
        // el exponente escalar se ensancha a `vec3<f32>` porque `pow` de
        // WGSL tampoco tiene sobrecarga mixta.
        NodeKind::Power => format!("pow(max({}, vec3<f32>(0.0)), vec3<f32>({}))", e(0), e(1)),
        NodeKind::DotProduct => format!("dot({}, {})", e(0), e(1)),
        NodeKind::Remap => format!(
            "({o_lo} + ({v} - {i_lo}) * ({o_hi} - {o_lo}) / ({i_hi} - {i_lo}))",
            v = e(0),
            i_lo = e(1),
            i_hi = e(2),
            o_lo = e(3),
            o_hi = e(4),
        ),
        // Damero entero: `abs` antes del módulo evita que una coordenada UV
        // negativa (fuera de [0,1], perfectamente válida) dé un resto
        // negativo y rompa la alternancia 0/1 de las celdas.
        NodeKind::Checker => format!(
            "f32((abs(i32(floor({uv}.x * {esc}))) + abs(i32(floor({uv}.y * {esc})))) % 2)",
            uv = e(0),
            esc = e(1),
        ),
        // Rampa vertical en v, recortada a [0,1] para que sea un valor de
        // superficie utilizable directamente sin postprocesado.
        NodeKind::Gradient => format!("clamp({}.y, 0.0, 1.0)", e(0)),
        NodeKind::StandardSurface => {
            format!("SurfaceProperties({}, {}, {}, {})", e(0), e(1), e(2), e(3))
        }
    }
}

/// Literal WGSL para un valor de FORGE. `Vec3` y `Color` producen el mismo
/// `vec3<f32>(...)` — la distinción entre ambos es solo de FORGE, y en WGSL,
/// como señala el contrato, no hay tipo de color separado.
fn literal_wgsl(v: Value) -> String {
    match v {
        Value::Float(x) => fmt_f32(x),
        Value::Vec2([x, y]) => format!("vec2<f32>({}, {})", fmt_f32(x), fmt_f32(y)),
        Value::Vec3([x, y, z]) | Value::Color([x, y, z]) => {
            format!("vec3<f32>({}, {}, {})", fmt_f32(x), fmt_f32(y), fmt_f32(z))
        }
    }
}

/// Formatea un `f32` como literal WGSL válido. A diferencia de `Display`
/// (`{}`), que imprime `1` para `1.0_f32`, `Debug` (`{:?}`) siempre incluye el
/// punto decimal — y WGSL exige un punto decimal, un exponente o un sufijo
/// `f`/`h` para reconocer un literal como de punto flotante.
fn fmt_f32(x: f32) -> String {
    format!("{x:?}")
}
