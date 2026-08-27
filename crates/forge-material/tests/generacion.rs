//! Tests de respuesta conocida sobre el generador de WGSL.
//!
//! Cada nodo de `NodeKind` tiene su propio test: si uno se rompe, el fallo
//! señala directamente cuál. Todo grafo que se declara "válido" además se
//! valida de verdad con el frontend y el `Validator` de `naga`, que es lo que
//! hace esto verificable sin una GPU. Un generador que produce texto con
//! forma de WGSL pero que no compila pasaría cualquier test que solo mirase
//! el `String`; por eso cada test de un grafo válido llama a
//! `generar_valido`, no a `WgslGenerator::generate` a secas.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use forge_material::WgslGenerator;
use forge_material_api::{
    GeneratedShader, Link, MaterialError, MaterialGraph, Node, NodeKind, ShaderGenerator,
    SocketType, Value,
};

// ---------------------------------------------------------------------------
// Construcción de grafos
// ---------------------------------------------------------------------------

fn nodo(id: u32, kind: NodeKind) -> Node {
    Node { id, kind, defaults: Vec::new(), constant: None }
}

fn constante(id: u32, valor: Value) -> Node {
    Node { id, kind: NodeKind::Constant, defaults: Vec::new(), constant: Some(valor) }
}

fn con_default(mut n: Node, nombre: &str, valor: Value) -> Node {
    n.defaults.push((nombre.to_string(), valor));
    n
}

fn enlace(from: u32, to: u32, to_input: &str) -> Link {
    Link { from, to, to_input: to_input.to_string() }
}

/// `StandardSurface` con las cuatro entradas resueltas por defecto. Los tests
/// añaden, en `links`, solo la que quieren ejercitar — `resolver_entrada`
/// prioriza el enlace sobre el valor por defecto, así que dejar los cuatro
/// defaults puestos siempre es inofensivo.
fn superficie(id: u32) -> Node {
    Node {
        id,
        kind: NodeKind::StandardSurface,
        defaults: vec![
            ("base_color".to_string(), Value::Color([0.5, 0.5, 0.5])),
            ("metallic".to_string(), Value::Float(0.0)),
            ("roughness".to_string(), Value::Float(0.5)),
            ("emissive".to_string(), Value::Color([0.0, 0.0, 0.0])),
        ],
        constant: None,
    }
}

fn grafo_minimo() -> MaterialGraph {
    MaterialGraph {
        name: "minimo".to_string(),
        nodes: vec![constante(1, Value::Color([0.8, 0.2, 0.2])), superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    }
}

/// Grafo que toca seis de los catorce `NodeKind` a la vez: sirve para el test
/// de determinismo, donde interesa un grafo con bastante estructura interna
/// (varios `HashMap` en juego durante el análisis) y no el caso trivial.
fn grafo_rico() -> MaterialGraph {
    let uv = nodo(1, NodeKind::TexCoord);
    let checker = con_default(nodo(2, NodeKind::Checker), "scale", Value::Float(8.0));
    let tinte = con_default(
        con_default(nodo(3, NodeKind::Mix), "fg", Value::Vec3([1.0, 0.0, 0.0])),
        "bg",
        Value::Vec3([0.0, 0.0, 1.0]),
    );
    let potencia = con_default(nodo(4, NodeKind::Power), "b", Value::Float(2.2));
    let normal = nodo(5, NodeKind::Normal);
    let dp = con_default(nodo(6, NodeKind::DotProduct), "b", Value::Vec3([0.0, 0.0, 1.0]));

    MaterialGraph {
        name: "rico".to_string(),
        nodes: vec![uv, checker, tinte, potencia, normal, dp, superficie(7)],
        links: vec![
            enlace(1, 2, "uv"),
            enlace(2, 3, "mix"),
            enlace(3, 4, "a"),
            enlace(4, 7, "base_color"),
            enlace(5, 6, "a"),
            enlace(6, 7, "roughness"),
        ],
        output: Some(7),
    }
}

// ---------------------------------------------------------------------------
// Verificación con naga: lo que hace esto comprobable sin GPU
// ---------------------------------------------------------------------------

/// Parsea `wgsl` con el frontend de naga y lo pasa por su `Validator`
/// completo. Cualquier fallo aborta el test mostrando el texto generado, para
/// no tener que reproducirlo a mano.
fn validar_con_naga(wgsl: &str) {
    let modulo = naga::front::wgsl::parse_str(wgsl)
        .unwrap_or_else(|e| panic!("wgsl invalido segun el parser de naga:\n{wgsl}\n\n{e}"));
    let mut validador = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validador
        .validate(&modulo)
        .unwrap_or_else(|e| panic!("naga rechazo el wgsl generado:\n{wgsl}\n\n{e}"));
}

/// Genera el shader y exige que naga lo acepte. El punto de partida de casi
/// todos los tests de grafos válidos.
fn generar_valido(g: &MaterialGraph) -> GeneratedShader {
    let shader = WgslGenerator.generate(g).unwrap_or_else(|e| panic!("grafo invalido: {e}"));
    validar_con_naga(&shader.wgsl);
    shader
}

// ---------------------------------------------------------------------------
// Grafo mínimo
// ---------------------------------------------------------------------------

#[test]
fn grafo_minimo_constante_a_salida_genera_wgsl_que_naga_valida() {
    let shader = generar_valido(&grafo_minimo());
    assert!(shader.wgsl.contains("struct SurfaceProperties {"));
    assert!(shader.wgsl.contains("fn material_surface() -> SurfaceProperties {"));
    assert_eq!(shader.nodes_used, 2);
}

// ---------------------------------------------------------------------------
// Un test por cada uno de los 14 `NodeKind`
// ---------------------------------------------------------------------------

#[test]
fn nodo_constant_genera_el_literal_exacto() {
    let g = MaterialGraph {
        name: "constant".to_string(),
        nodes: vec![constante(1, Value::Color([0.8, 0.2, 0.2])), superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains("let n1: vec3<f32> = vec3<f32>(0.8, 0.2, 0.2);"),
        "wgsl generado:\n{}",
        shader.wgsl
    );
}

#[test]
fn nodo_texcoord_expone_uv_como_parametro_de_la_funcion() {
    let g = MaterialGraph {
        name: "texcoord".to_string(),
        nodes: vec![nodo(1, NodeKind::TexCoord), nodo(2, NodeKind::Gradient), superficie(3)],
        links: vec![enlace(1, 2, "uv"), enlace(2, 3, "roughness")],
        output: Some(3),
    };
    let shader = generar_valido(&g);
    assert!(shader.wgsl.contains("fn material_surface(uv: vec2<f32>)"), "{}", shader.wgsl);
    assert!(shader.wgsl.contains("let n1: vec2<f32> = uv;"), "{}", shader.wgsl);
}

#[test]
fn nodo_normal_expone_normal_como_parametro_de_la_funcion() {
    let g = MaterialGraph {
        name: "normal".to_string(),
        nodes: vec![
            nodo(1, NodeKind::Normal),
            con_default(nodo(2, NodeKind::DotProduct), "b", Value::Vec3([0.0, 0.0, 1.0])),
            superficie(3),
        ],
        links: vec![enlace(1, 2, "a"), enlace(2, 3, "roughness")],
        output: Some(3),
    };
    let shader = generar_valido(&g);
    assert!(shader.wgsl.contains("fn material_surface(normal: vec3<f32>)"), "{}", shader.wgsl);
    assert!(shader.wgsl.contains("let n1: vec3<f32> = normal;"), "{}", shader.wgsl);
    assert!(
        shader.wgsl.contains("let n2: f32 = dot(n1, vec3<f32>(0.0, 0.0, 1.0));"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_position_expone_position_como_parametro_de_la_funcion() {
    let g = MaterialGraph {
        name: "position".to_string(),
        nodes: vec![
            nodo(1, NodeKind::Position),
            con_default(nodo(2, NodeKind::DotProduct), "b", Value::Vec3([1.0, 0.0, 0.0])),
            superficie(3),
        ],
        links: vec![enlace(1, 2, "a"), enlace(2, 3, "metallic")],
        output: Some(3),
    };
    let shader = generar_valido(&g);
    assert!(shader.wgsl.contains("fn material_surface(position: vec3<f32>)"), "{}", shader.wgsl);
    assert!(shader.wgsl.contains("let n1: vec3<f32> = position;"), "{}", shader.wgsl);
}

#[test]
fn nodo_add_genera_la_suma_componente_a_componente() {
    let suma = con_default(
        con_default(nodo(1, NodeKind::Add), "a", Value::Vec3([1.0, 0.0, 0.0])),
        "b",
        Value::Vec3([0.0, 1.0, 0.0]),
    );
    let g = MaterialGraph {
        name: "add".to_string(),
        nodes: vec![suma, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader
            .wgsl
            .contains("let n1: vec3<f32> = vec3<f32>(1.0, 0.0, 0.0) + vec3<f32>(0.0, 1.0, 0.0);"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_multiply_genera_el_producto_componente_a_componente() {
    let prod = con_default(
        con_default(nodo(1, NodeKind::Multiply), "a", Value::Vec3([2.0, 3.0, 4.0])),
        "b",
        Value::Vec3([0.5, 0.5, 0.5]),
    );
    let g = MaterialGraph {
        name: "multiply".to_string(),
        nodes: vec![prod, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader
            .wgsl
            .contains("let n1: vec3<f32> = vec3<f32>(2.0, 3.0, 4.0) * vec3<f32>(0.5, 0.5, 0.5);"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_mix_usa_la_sobrecarga_vector_vector_escalar() {
    let mezcla = con_default(
        con_default(
            con_default(nodo(1, NodeKind::Mix), "fg", Value::Vec3([1.0, 0.0, 0.0])),
            "bg",
            Value::Vec3([0.0, 0.0, 1.0]),
        ),
        "mix",
        Value::Float(0.25),
    );
    let g = MaterialGraph {
        name: "mix".to_string(),
        nodes: vec![mezcla, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains(
            "let n1: vec3<f32> = mix(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), 0.25);"
        ),
        "{}",
        shader.wgsl
    );
}

/// A diferencia de `mix`, `clamp` en WGSL no tiene sobrecarga vector+escalar:
/// los dos límites, declarados `Float` en el contrato, se ensanchan a
/// `vec3<f32>` antes de llamar a `clamp`. Sin esto naga rechazaría el WGSL
/// por tipos no coincidentes en los tres argumentos.
#[test]
fn nodo_clamp_ensancha_los_limites_escalares_a_vec3() {
    let recorte = con_default(
        con_default(
            con_default(nodo(1, NodeKind::Clamp), "in", Value::Vec3([2.0, -1.0, 0.5])),
            "low",
            Value::Float(0.0),
        ),
        "high",
        Value::Float(1.0),
    );
    let g = MaterialGraph {
        name: "clamp".to_string(),
        nodes: vec![recorte, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains(
            "let n1: vec3<f32> = clamp(vec3<f32>(2.0, -1.0, 0.5), vec3<f32>(0.0), vec3<f32>(1.0));"
        ),
        "{}",
        shader.wgsl
    );
}

/// La trampa documentada: `pow` es indefinido en WGSL cuando la base es
/// negativa. El generador debe recortarla a `[0, +inf)` con `max` antes de
/// elevarla — este test comprueba tanto el texto exacto como, por separado,
/// la presencia de la guarda `max(`, que es la propiedad que de verdad
/// importa (independiente de cómo se formatee el resto de la expresión).
#[test]
fn nodo_power_protege_la_base_negativa_con_max() {
    let potencia = con_default(
        con_default(nodo(1, NodeKind::Power), "a", Value::Vec3([-2.0, 4.0, -0.5])),
        "b",
        Value::Float(2.0),
    );
    let g = MaterialGraph {
        name: "power".to_string(),
        nodes: vec![potencia, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains(
            "let n1: vec3<f32> = pow(max(vec3<f32>(-2.0, 4.0, -0.5), vec3<f32>(0.0)), vec3<f32>(2.0));"
        ),
        "{}",
        shader.wgsl
    );
    assert!(shader.wgsl.contains("max("), "falta la guarda protectora de pow");
}

#[test]
fn nodo_dotproduct_genera_dot() {
    let dp = con_default(
        con_default(nodo(1, NodeKind::DotProduct), "a", Value::Vec3([1.0, 2.0, 3.0])),
        "b",
        Value::Vec3([4.0, 5.0, 6.0]),
    );
    let g = MaterialGraph {
        name: "dotproduct".to_string(),
        nodes: vec![dp, superficie(2)],
        links: vec![enlace(1, 2, "roughness")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader
            .wgsl
            .contains("let n1: f32 = dot(vec3<f32>(1.0, 2.0, 3.0), vec3<f32>(4.0, 5.0, 6.0));"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_remap_genera_la_formula_de_reasignacion_lineal() {
    let mut remap = nodo(1, NodeKind::Remap);
    for (nombre, valor) in [
        ("in", 0.5f32),
        ("in_lo", 0.0),
        ("in_hi", 1.0),
        ("out_lo", 0.2),
        ("out_hi", 0.8),
    ] {
        remap = con_default(remap, nombre, Value::Float(valor));
    }
    let g = MaterialGraph {
        name: "remap".to_string(),
        nodes: vec![remap, superficie(2)],
        links: vec![enlace(1, 2, "roughness")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains("let n1: f32 = (0.2 + (0.5 - 0.0) * (0.8 - 0.2) / (1.0 - 0.0));"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_checker_genera_el_patron_entero() {
    let checker = con_default(
        con_default(nodo(1, NodeKind::Checker), "uv", Value::Vec2([0.3, 0.7])),
        "scale",
        Value::Float(4.0),
    );
    let g = MaterialGraph {
        name: "checker".to_string(),
        nodes: vec![checker, superficie(2)],
        links: vec![enlace(1, 2, "metallic")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains(
            "let n1: f32 = f32((abs(i32(floor(vec2<f32>(0.3, 0.7).x * 4.0))) + \
             abs(i32(floor(vec2<f32>(0.3, 0.7).y * 4.0)))) % 2);"
        ),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_gradient_genera_la_rampa_vertical_en_v() {
    let gradiente = con_default(nodo(1, NodeKind::Gradient), "uv", Value::Vec2([0.1, 0.9]));
    let g = MaterialGraph {
        name: "gradient".to_string(),
        nodes: vec![gradiente, superficie(2)],
        links: vec![enlace(1, 2, "roughness")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains("let n1: f32 = clamp(vec2<f32>(0.1, 0.9).y, 0.0, 1.0);"),
        "{}",
        shader.wgsl
    );
}

#[test]
fn nodo_standard_surface_construye_la_struct_de_salida() {
    let g = MaterialGraph {
        name: "standard_surface".to_string(),
        nodes: vec![
            constante(1, Value::Color([0.1, 0.2, 0.3])),
            Node {
                id: 2,
                kind: NodeKind::StandardSurface,
                defaults: vec![
                    ("metallic".to_string(), Value::Float(0.3)),
                    ("roughness".to_string(), Value::Float(0.6)),
                    ("emissive".to_string(), Value::Color([0.05, 0.05, 0.05])),
                ],
                constant: None,
            },
        ],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(
        shader.wgsl.contains(
            "let n2: SurfaceProperties = SurfaceProperties(n1, 0.3, 0.6, vec3<f32>(0.05, 0.05, 0.05));"
        ),
        "{}",
        shader.wgsl
    );
    assert!(shader.wgsl.contains("return n2;"), "{}", shader.wgsl);
}

// ---------------------------------------------------------------------------
// Código muerto
// ---------------------------------------------------------------------------

/// Un nodo sin camino hasta la salida no es un error — pero tampoco debe
/// generar código: si apareciera, cada edición del grafo en un editor visual
/// (donde sobran nodos sueltos todo el tiempo) inflaría cada shader con ramas
/// que nunca se evalúan.
#[test]
fn un_nodo_inalcanzable_no_aparece_en_el_wgsl_generado() {
    let g = MaterialGraph {
        name: "con_nodo_muerto".to_string(),
        nodes: vec![
            constante(1, Value::Color([0.8, 0.2, 0.2])),
            superficie(2),
            constante(99, Value::Vec3([777.0, 777.0, 777.0])),
        ],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let shader = generar_valido(&g);
    assert!(!shader.wgsl.contains("n99"), "el nodo inalcanzable no deberia tener variable propia");
    assert!(!shader.wgsl.contains("777"), "el literal del nodo muerto no deberia aparecer");
    assert_eq!(shader.nodes_used, 2, "solo cuentan como usados los nodos alcanzables");
}

// ---------------------------------------------------------------------------
// Determinismo: sin esto no hay caché de shaders posible
// ---------------------------------------------------------------------------

#[test]
fn generar_el_mismo_grafo_dos_veces_da_texto_identico_byte_a_byte() {
    let g = grafo_rico();
    let a = WgslGenerator.generate(&g).unwrap();
    let b = WgslGenerator.generate(&g).unwrap();
    assert_eq!(a.wgsl.as_bytes(), b.wgsl.as_bytes());
    assert_eq!(a.permutation_key, b.permutation_key);
    validar_con_naga(&a.wgsl);
}

// ---------------------------------------------------------------------------
// permutation_key
// ---------------------------------------------------------------------------

#[test]
fn permutation_key_del_mismo_grafo_es_igual() {
    let g = grafo_minimo();
    let a = WgslGenerator.generate(&g).unwrap();
    let b = WgslGenerator.generate(&g).unwrap();
    assert_eq!(a.permutation_key, b.permutation_key);
}

/// Reordenar el `Vec<Node>` sin tocar qué se conecta con qué es un cambio de
/// almacenamiento, no de topología: la clave (y el WGSL) no deben cambiar.
#[test]
fn permutation_key_no_cambia_al_reordenar_el_vec_de_nodos() {
    let g1 = grafo_minimo();
    let mut g2 = grafo_minimo();
    g2.nodes.reverse();

    let a = WgslGenerator.generate(&g1).unwrap();
    let b = WgslGenerator.generate(&g2).unwrap();
    assert_eq!(a.permutation_key, b.permutation_key);
    assert_eq!(a.wgsl, b.wgsl, "reordenar nodes no deberia cambiar ni el texto generado");
}

/// Y el caso contrario: cambiar un literal (misma topología) sí debe cambiar
/// la clave, porque son dos permutaciones de shader distintas que hay que
/// compilar por separado.
#[test]
fn permutation_key_cambia_si_cambia_un_literal() {
    let g1 = grafo_minimo();
    let mut g2 = grafo_minimo();
    g2.nodes[0].constant = Some(Value::Color([0.1, 0.9, 0.1]));

    let a = WgslGenerator.generate(&g1).unwrap();
    let b = WgslGenerator.generate(&g2).unwrap();
    assert_ne!(a.permutation_key, b.permutation_key);
    assert_ne!(a.wgsl, b.wgsl);
}

/// Control negativo del propio test de arriba: dos grafos genuinamente
/// distintos no deberían colisionar por accidente con un hasher razonable.
/// No prueba ausencia de colisiones en general (imposible con un `u64`), solo
/// que este par concreto, elegido para parecerse todo lo posible salvo en un
/// literal, no colisiona.
#[test]
fn permutation_key_no_es_una_constante() {
    let mut vistas: Vec<u64> = Vec::new();
    for grafo in [grafo_minimo(), grafo_rico()] {
        let clave = WgslGenerator.generate(&grafo).unwrap().permutation_key;
        assert!(!vistas.contains(&clave), "clave repetida entre grafos distintos");
        vistas.push(clave);
    }
    // y no es trivialmente 0 ni el hash de una cadena vacía
    let mut h = DefaultHasher::new();
    "".hash(&mut h);
    assert_ne!(vistas[0], h.finish());
    assert_ne!(vistas[0], 0);
}

// ---------------------------------------------------------------------------
// Controles positivos: cada variante de `MaterialError`, con el error correcto
// ---------------------------------------------------------------------------
//
// Un validador que nunca detectara nada pasaría todos los tests de arriba
// igual de bien que uno correcto. Estos son los controles que lo descartan.

#[test]
fn control_un_ciclo_se_detecta_y_no_cuelga() {
    // A.a <- B, B.a <- A: dependencia mutua real, no una coincidencia de ids.
    let a = con_default(nodo(10, NodeKind::Add), "b", Value::Vec3([0.0, 0.0, 0.0]));
    let b = con_default(nodo(20, NodeKind::Add), "b", Value::Vec3([0.0, 0.0, 0.0]));
    let g = MaterialGraph {
        name: "ciclo".to_string(),
        nodes: vec![a, b, superficie(30)],
        links: vec![
            enlace(20, 10, "a"),
            enlace(10, 20, "a"),
            enlace(10, 30, "base_color"),
        ],
        output: Some(30),
    };
    // La propia ejecución del test (que termina) ya demuestra "no cuelgues";
    // el `match` comprueba además que el error es el correcto.
    match WgslGenerator.generate(&g) {
        Err(MaterialError::Cycle(id)) => assert!(id == 10 || id == 20, "id inesperado: {id}"),
        otro => panic!("no se detecto el ciclo: {otro:?}"),
    }
}

#[test]
fn control_tipos_incompatibles_se_detectan() {
    // DotProduct da Float; Add.a exige Vec3. Es un TypeMismatch real.
    let dp = con_default(
        con_default(nodo(1, NodeKind::DotProduct), "a", Value::Vec3([1.0, 0.0, 0.0])),
        "b",
        Value::Vec3([0.0, 1.0, 0.0]),
    );
    let add = con_default(nodo(2, NodeKind::Add), "b", Value::Vec3([0.0, 0.0, 0.0]));
    let g = MaterialGraph {
        name: "tipos".to_string(),
        nodes: vec![dp, add, superficie(3)],
        links: vec![enlace(1, 2, "a"), enlace(2, 3, "base_color")],
        output: Some(3),
    };
    let r = WgslGenerator.generate(&g);
    assert!(
        matches!(
            r,
            Err(MaterialError::TypeMismatch { from: SocketType::Float, to: SocketType::Vec3 })
        ),
        "se esperaba TypeMismatch Float -> Vec3, salio {r:?}"
    );
}

#[test]
fn control_entrada_obligatoria_sin_conectar_se_detecta() {
    // Add exige "a" y "b"; solo se da "a".
    let add = con_default(nodo(1, NodeKind::Add), "a", Value::Vec3([1.0, 0.0, 0.0]));
    let g = MaterialGraph {
        name: "falta_entrada".to_string(),
        nodes: vec![add, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let r = WgslGenerator.generate(&g);
    assert!(
        matches!(r, Err(MaterialError::MissingInput { node: 1, input: "b" })),
        "se esperaba MissingInput(1, \"b\"), salio {r:?}"
    );
}

#[test]
fn control_grafo_sin_nodo_de_salida_se_detecta() {
    let g = MaterialGraph {
        name: "sin_salida".to_string(),
        nodes: vec![constante(1, Value::Color([1.0, 1.0, 1.0]))],
        links: vec![],
        output: None,
    };
    assert!(matches!(WgslGenerator.generate(&g), Err(MaterialError::NoOutput)));
}

/// El contrato exige que la salida sea `StandardSurface`; apuntar `output` a
/// cualquier otro nodo no tiene una lectura razonable distinta de "no hay
/// salida válida".
#[test]
fn control_salida_que_no_es_standard_surface_tambien_cuenta_como_sin_salida() {
    let g = MaterialGraph {
        name: "salida_equivocada".to_string(),
        nodes: vec![constante(1, Value::Color([1.0, 1.0, 1.0]))],
        links: vec![],
        output: Some(1),
    };
    assert!(matches!(WgslGenerator.generate(&g), Err(MaterialError::NoOutput)));
}

#[test]
fn control_referencia_a_nodo_inexistente_se_detecta() {
    let g = MaterialGraph {
        name: "nodo_fantasma".to_string(),
        nodes: vec![superficie(2)],
        links: vec![enlace(999, 2, "base_color")],
        output: Some(2),
    };
    assert!(matches!(WgslGenerator.generate(&g), Err(MaterialError::UnknownNode(999))));
}

// ---------------------------------------------------------------------------
// `Color` contra `Vec3`: la segunda trampa documentada
// ---------------------------------------------------------------------------
//
// El contrato distingue `Color` de `Vec3` a propósito, pero permite cruzar de
// uno a otro explícitamente (`SocketType::compatible_with`) porque en WGSL no
// hay un tipo de color aparte — ambos son `vec3<f32>`. Los dos tests de abajo
// prueban las dos mitades de esa regla: cruzar cuando `compatible_with` lo
// permite tiene que funcionar, y no hacerlo cuando no lo permite (una tercera
// mitad, distinta de `Float -> Vec3`, ya cubierta por
// `control_tipos_incompatibles_se_detectan`).

/// Mitad "sí": una constante `Color` alimentando una entrada `Vec3` (la de
/// `Add`) tiene que aceptarse y generar WGSL válido — es justo el cruce que
/// `compatible_with` autoriza explícitamente.
#[test]
fn una_constante_color_alimenta_una_entrada_vec3_sin_problema() {
    let tinte = constante(1, Value::Color([0.9, 0.1, 0.1]));
    let suma = con_default(nodo(2, NodeKind::Add), "b", Value::Vec3([0.0, 0.0, 0.0]));
    let g = MaterialGraph {
        name: "color_a_vec3".to_string(),
        nodes: vec![tinte, suma, superficie(3)],
        links: vec![enlace(1, 2, "a"), enlace(2, 3, "base_color")],
        output: Some(3),
    };
    let shader = generar_valido(&g);
    assert!(shader.wgsl.contains("let n1: vec3<f32> = vec3<f32>(0.9, 0.1, 0.1);"), "{}", shader.wgsl);
    assert!(
        shader.wgsl.contains("let n2: vec3<f32> = n1 + vec3<f32>(0.0, 0.0, 0.0);"),
        "{}",
        shader.wgsl
    );
}

/// Mitad "no": `Vec2` (la salida de `TexCoord`) no es intercambiable con
/// `Vec3`/`Color` bajo ninguna regla — conectarlo a `base_color` tiene que
/// rechazarse, no colarse como si fuera un `vec3<f32>` cualquiera por
/// compartir la palabra "vec".
#[test]
fn control_vec2_no_es_compatible_con_color_aunque_ambos_sean_vectores() {
    let uv = nodo(1, NodeKind::TexCoord);
    let g = MaterialGraph {
        name: "vec2_no_es_color".to_string(),
        nodes: vec![uv, superficie(2)],
        links: vec![enlace(1, 2, "base_color")],
        output: Some(2),
    };
    let r = WgslGenerator.generate(&g);
    assert!(
        matches!(
            r,
            Err(MaterialError::TypeMismatch { from: SocketType::Vec2, to: SocketType::Color })
        ),
        "se esperaba TypeMismatch Vec2 -> Color, salio {r:?}"
    );
}
