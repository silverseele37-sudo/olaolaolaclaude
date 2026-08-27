//! Suite de regresión topológica. Riesgo R1 del proyecto.
//!
//! `naming.rs` documenta la estrategia; este archivo la **mide**. La pregunta
//! de cada caso es siempre la misma: se construye un árbol, se selecciona una
//! entidad topológica aguas abajo (una cara, una arista), se edita un
//! parámetro aguas arriba, se re-evalúa, y se comprueba que la referencia
//! sigue apuntando **a lo correcto** — no solo que resuelve a algo.
//!
//! # Disciplina de "respuesta conocida"
//!
//! Un test que solo comprueba `!resolucion.rota()` no distingue un resolver
//! correcto de uno que siempre devuelve la primera entidad de la lista (ver
//! `control_negativo_naming.rs`). Cada caso de aquí comprueba, además,
//! **contra qué** se comparó: o bien que el `StableId` es exactamente el
//! capturado (capa 1: el caso típico, cota u orientación editada), o bien que
//! coincide con una entidad recalculada **de forma independiente** por
//! geometría (capa 2: un cambio de topología) — nunca con el propio criterio
//! interno del resolver, porque comprobar el resolver contra sí mismo no
//! comprobaría nada.
//!
//! # Cómo se agrupan los casos
//!
//! - **Sección 1** — cambios de cota típicos (distancia de extrusión,
//!   posición del sketch, dimensiones de una caja o de un cilindro, dirección
//!   de extrusión). La genealogía (capa 1) tiene que sostenerse siempre: los
//!   `mark` del stub son función de índices estructurales, nunca de los
//!   valores numéricos editados (ver el comentario grande más abajo). Es el
//!   grupo que alimenta el `assert` de aceptación de ADR-0002 §6.
//! - **Sección 2** — número de lados de un polígono, en las dos direcciones.
//!   Aumentar conserva la genealogía por índice aunque la arista física
//!   cambie (un límite real, documentado con su propio test). Reducir la
//!   quita de la lista y fuerza capa 2 o ruptura, según quede o no una
//!   candidata sin ambigüedad.
//! - **Sección 3** — supresión de un nodo intermedio: la referencia de un
//!   nodo aguas abajo sobrevive por firma, no por linaje, porque el nodo que
//!   generó su linaje deja de estar en la cadena de evaluación.
//! - **Sección 4** — reordenación de dos nodos consecutivos de una cadena: el
//!   caso que la documentación de `tree.rs` señala como "el peor caso del
//!   nombrado persistente", con geometría idéntica y genealogía distinta.
//! - **Sección 5** — combinaciones, para no medir cada tipo de edición de
//!   forma aislada de las demás.
//! - **Sección 6** — la medición agregada y el `assert` del criterio de
//!   aceptación.
//!
//! # Por qué la capa 1 sobrevive tan bien a los cambios de cota
//!
//! Vale la pena decirlo una vez, en voz alta, porque explica el resultado de
//! casi toda la sección 1: en `forge-kernel-stub`, el `mark` de un
//! `StableId` es una función de **posición estructural** (el índice de la
//! arista del perfil, el índice fijo de una cara de caja, el `mark` de la
//! cara anterior en una operación de bisel) y **nunca** de los valores
//! numéricos concretos (una distancia, un radio, una posición). Editar una
//! cota cambia dónde está la geometría, no qué índice ocupa, así que la
//! genealogía se sostiene exacta a cualquier profundidad de operaciones
//! encadenadas. Esto **no** es un accidente del test: es la propiedad de
//! diseño que hace que la capa 1 sea barata y determinista, y es precisamente
//! lo que las secciones 2 a 4 ponen a prueba desde el otro lado, forzando los
//! casos donde el índice estructural sí cambia.

mod comun;
use comun::*;

use forge_doc::{Binding, FeatureId};
use forge_kernel_api::*;
use forge_math::{DVec2, DVec3};
use forge_param::naming::tasa_de_revinculacion;
use forge_param::*;

// ---------------------------------------------------------------------------
// Utilidades propias de este archivo
// ---------------------------------------------------------------------------

/// Evalúa `tree` con un evaluador nuevo y devuelve la topología del nodo
/// `nodo`. Es el "recalcular y mirar" que repite cada caso de esta suite tras
/// una edición.
fn topologia_de(kernel: &dyn GeometryKernel, tree: &FeatureTree, nodo: FeatureId) -> TopologySummary {
    let outcome = evaluar(kernel, tree).expect("el arbol debe re-evaluar limpio tras la edicion");
    let shape = outcome
        .shape(nodo)
        .unwrap_or_else(|| panic!("el nodo {nodo} no produjo forma"));
    kernel
        .topology(shape)
        .expect("topologia de la forma re-evaluada")
}

/// Acumulador de la tasa de re-vinculación medida por esta suite. Separa
/// "cambios de cota típicos" de "cambios de topología" porque el criterio de
/// aceptación de ADR-0002 §6 (≥95 %) solo se predica de los primeros — mezclar
/// los dos grupos escondería justo la distinción que hace útil la métrica.
#[derive(Default)]
struct Medidor {
    tipicos: Vec<Resolucion>,
    topologicos: Vec<Resolucion>,
}

impl Medidor {
    fn tipico(&mut self, r: Resolucion) {
        self.tipicos.push(r);
    }
    fn topologico(&mut self, r: Resolucion) {
        self.topologicos.push(r);
    }
    fn tasa_tipicos(&self) -> f64 {
        tasa_de_revinculacion(&self.tipicos)
    }
    fn tasa_topologicos(&self) -> f64 {
        tasa_de_revinculacion(&self.topologicos)
    }
}

/// Construye un árbol de caja: `BoxPrimitive` solo, sin sketch. Las
/// dimensiones de una caja no pasan por el solver ni por un perfil: es la
/// variación más directa de "editar una cota" que pide el enunciado.
fn arbol_caja(id: FeatureId, min: DVec3, max: DVec3) -> FeatureTree {
    let mut t = FeatureTree::new();
    t.insertar(FeatureNode::con_id(
        id,
        "caja",
        NodeKind::BoxPrimitive { min, max },
    ));
    t
}

fn set_caja(tree: &mut FeatureTree, id: FeatureId, min: DVec3, max: DVec3) {
    if let NodeKind::BoxPrimitive { min: m, max: x } = &mut tree.nodo_mut(id).expect("caja").kind {
        *m = min;
        *x = max;
    } else {
        panic!("el nodo {id} no es una caja");
    }
}

/// Árbol de cilindro solo.
fn arbol_cilindro(id: FeatureId, base: DVec3, eje: DVec3, radio: f64, altura: f64) -> FeatureTree {
    let mut t = FeatureTree::new();
    t.insertar(FeatureNode::con_id(
        id,
        "cilindro",
        NodeKind::Cylinder {
            base,
            eje,
            radio_mm: radio,
            altura_mm: altura,
        },
    ));
    t
}

fn set_cilindro(
    tree: &mut FeatureTree,
    id: FeatureId,
    base: DVec3,
    eje: DVec3,
    radio: f64,
    altura: f64,
) {
    if let NodeKind::Cylinder {
        base: b,
        eje: e,
        radio_mm: r,
        altura_mm: a,
    } = &mut tree.nodo_mut(id).expect("cilindro").kind
    {
        *b = base;
        *e = eje;
        *r = radio;
        *a = altura;
    } else {
        panic!("el nodo {id} no es un cilindro");
    }
}

/// Des-cuantiza la normal de una firma. Duplicado deliberado (como
/// `comun::centro_de`) de la función privada de `naming.rs`: los tests no
/// deben depender de internals privados.
fn normal_de(s: &GeometrySignature) -> DVec3 {
    DVec3::new(
        s.normal_q[0] as f64,
        s.normal_q[1] as f64,
        s.normal_q[2] as f64,
    ) / 1000.0
}

fn medida_de(s: &GeometrySignature) -> f64 {
    s.measure_q as f64 * GeometrySignature::QUANTUM_MM
}

/// La cara cuya normal es más parecida a `objetivo`, por producto escalar.
/// Oráculo **independiente** de `Resolver::parecido`: sirve para confirmar
/// que una selección recayó en la candidata geométricamente correcta con un
/// método de cálculo distinto al que se está poniendo a prueba — comprobar
/// el resolver contra su propia fórmula no comprobaría nada.
fn cara_normal_mas_parecida(topo: &TopologySummary, objetivo: DVec3) -> TopoEntity {
    topo.faces
        .iter()
        .max_by(|a, b| {
            let da = normal_de(&a.signature).dot(objetivo);
            let db = normal_de(&b.signature).dot(objetivo);
            da.partial_cmp(&db).unwrap()
        })
        .cloned()
        .expect("la topologia no tiene caras")
}

/// La cara `Cap` de inicio (`true`) o de fin (`false`) de una extrusión.
fn tapa(topo: &TopologySummary, inicio: bool) -> TopoEntity {
    topo.faces
        .iter()
        .find(|f| matches!(f.provenance, TopoProvenance::Cap { start } if start == inicio))
        .cloned()
        .expect("la topologia no tiene esa tapa")
}

/// La arista **vertical** de un prisma (longitud = `altura`, para no
/// confundirla con las del lado del polígono) cuyo centro, proyectado sobre
/// el plano XY, apunta lo más cerca posible de `angulo_deg`.
///
/// Determinista por construcción, a diferencia de "la más lejana de tal
/// punto": eso hace falta para elegir con precisión dos aristas que no
/// compartan vértice (el requisito de `biselar` para aceptar selecciones
/// separadas) sin depender de que la geometría alrededor de un punto de
/// referencia no cambie de forma inesperada entre topologías.
fn arista_vertical_en(topo: &TopologySummary, altura: f64, angulo_deg: f64) -> TopoEntity {
    let obj = DVec2::new(angulo_deg.to_radians().cos(), angulo_deg.to_radians().sin());
    topo.edges
        .iter()
        .filter(|e| (medida_de(&e.signature) - altura).abs() < 1e-6)
        .max_by(|a, b| {
            let ca = centro_de(&a.signature);
            let cb = centro_de(&b.signature);
            let da = DVec2::new(ca.x, ca.y).normalize_or_zero().dot(obj);
            let db = DVec2::new(cb.x, cb.y).normalize_or_zero().dot(obj);
            da.partial_cmp(&db).unwrap()
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("no hay arista vertical de altura {altura} cerca de {angulo_deg} grados")
        })
}

// ---------------------------------------------------------------------------
// Sección 1 — cambios de cota típicos: la genealogía (capa 1) se sostiene
// exacta, a cualquier profundidad. Alimentan el `assert` de la sección 6.
// ---------------------------------------------------------------------------

#[test]
fn incrementar_la_distancia_de_extrusion_conserva_la_cara_lateral_referenciada() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 3).expect("hexagono: cara lateral 3");
    let referencia = TopoRef::capturar(extrude_id, &cara0);
    // Respuesta conocida: lado de un hexagono de radio 10 = 2*10*sin(pi/6) = 10.
    assert!((medida_de(&cara0.signature) - 10.0 * 5.0).abs() < 1e-9);

    set_distancia(&mut tree, extrude_id, 12.0);
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));
    let cara1 = cara_lateral(&topo1, 3).expect("cara lateral 3 tras el cambio");
    assert!(
        (medida_de(&cara1.signature) - 10.0 * 12.0).abs() < 1e-6,
        "area esperada {}, salio {}",
        10.0 * 12.0,
        medida_de(&cara1.signature)
    );
}

#[test]
fn reducir_la_distancia_de_extrusion_conserva_la_arista_lateral_referenciada() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, altura,
    );
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let arista0 = arista_vertical_en(&topo0, altura, 90.0);
    let referencia = TopoRef::capturar(extrude_id, &arista0);

    set_distancia(&mut tree, extrude_id, 2.0);
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));
}

/// Cambiar la dirección de extrusión no cambia qué índice de perfil generó
/// cada cara lateral, así que la capa 1 la conserva exacta — aunque la
/// orientación de esa cara en el espacio cambie por completo. Es la otra
/// cara de la moneda de la sección 2: aquí el cambio geométrico es enorme y
/// la referencia sigue siendo correcta, porque el usuario sigue señalando
/// "la cara nacida del lado 2 del perfil", que no se movió de índice.
#[test]
fn cambiar_la_direccion_de_extrusion_conserva_la_cara_lateral_y_reorienta_su_normal() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 2).expect("cara lateral 2");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    set_direccion(&mut tree, extrude_id, DVec3::X);
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_lateral(&topo1, 2).expect("cara lateral 2 tras el cambio");
    let cos = normal_de(&cara0.signature).dot(normal_de(&cara1.signature));
    assert!(
        cos < 0.7,
        "la normal deberia haber cambiado mucho al extruir hacia X en vez de Z, dot={cos}"
    );
}

#[test]
fn alternar_extrusion_simetrica_conserva_identidad_de_las_dos_tapas() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 8.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let inicio0 = tapa(&topo0, true);
    let fin0 = tapa(&topo0, false);
    let ref_inicio = TopoRef::capturar(extrude_id, &inicio0);
    let ref_fin = TopoRef::capturar(extrude_id, &fin0);
    // Respuesta conocida antes del cambio: no simetrico, tapas en z=0 y z=8.
    assert!((centro_de(&inicio0.signature).z).abs() < 1e-9);
    assert!((centro_de(&fin0.signature).z - 8.0).abs() < 1e-9);

    if let NodeKind::Extrude { simetrico, .. } = &mut tree.nodo_mut(extrude_id).unwrap().kind {
        *simetrico = true;
    } else {
        panic!("el nodo {extrude_id} no es un extrude");
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);

    let r_inicio = Resolver::default().resolver(&ref_inicio, &topo1);
    let r_fin = Resolver::default().resolver(&ref_fin, &topo1);
    assert!(r_inicio.exacta() && r_fin.exacta());
    assert_eq!(r_inicio.valor(), Some(ref_inicio.objetivo));
    assert_eq!(r_fin.valor(), Some(ref_fin.objetivo));

    // Respuesta conocida despues del cambio: simetrico con distancia 8
    // reparte 4 mm a cada lado.
    let inicio1 = tapa(&topo1, true);
    let fin1 = tapa(&topo1, false);
    assert!((centro_de(&inicio1.signature).z - (-4.0)).abs() < 1e-6);
    assert!((centro_de(&fin1.signature).z - 4.0).abs() < 1e-6);
}

#[test]
fn trasladar_el_sketch_conserva_la_cara_lateral_y_desplaza_su_centroide_exactamente() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 4).expect("cara lateral 4");
    let referencia = TopoRef::capturar(extrude_id, &cara0);
    let centro0 = centro_de(&cara0.signature);

    let traslacion = DVec3::new(3.0, -2.0, 7.0);
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.plano = mover_plano(s.plano, traslacion, 0.0);
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_lateral(&topo1, 4).expect("cara lateral 4 tras mover el sketch");
    let centro1 = centro_de(&cara1.signature);
    assert!(
        (centro1 - (centro0 + traslacion)).length() < 5e-3,
        "centro0={centro0:?} traslacion={traslacion:?} centro1={centro1:?}"
    );
}

#[test]
fn rotar_el_sketch_en_z_conserva_la_cara_lateral_y_rota_su_normal_el_mismo_angulo() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 1).expect("cara lateral 1");
    let referencia = TopoRef::capturar(extrude_id, &cara0);
    let n0 = normal_de(&cara0.signature);

    let angulo = 40f64.to_radians();
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.plano = mover_plano(s.plano, DVec3::ZERO, angulo);
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_lateral(&topo1, 1).expect("cara lateral 1 tras rotar el sketch");
    let n1 = normal_de(&cara1.signature);
    let (s, c) = angulo.sin_cos();
    let n0_rotada = DVec3::new(n0.x * c - n0.y * s, n0.x * s + n0.y * c, n0.z);
    assert!(
        (n1 - n0_rotada).length() < 1e-3,
        "n0={n0:?} rotada 40 grados deberia dar {n0_rotada:?}, salio {n1:?}"
    );
}

#[test]
fn editar_el_ancho_de_un_rectangulo_conserva_su_cara_lateral_y_actualiza_el_area() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let (mut tree, dim_ancho, _dim_alto) =
        arbol_extrude_rectangulo(sketch_id, extrude_id, 10.0, 6.0, Plano::default(), DVec3::Z, 4.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    // Cara 0: el lado p0-p1, el que mide "ancho" de largo.
    let cara0 = cara_lateral(&topo0, 0).expect("cara lateral 0");
    assert!((medida_de(&cara0.signature) - 10.0 * 4.0).abs() < 1e-9);
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    set_ancho(&mut tree, sketch_id, dim_ancho, 25.0);
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_lateral(&topo1, 0).expect("cara lateral 0 tras el cambio");
    assert!(
        (medida_de(&cara1.signature) - 25.0 * 4.0).abs() < 1e-6,
        "area esperada {}, salio {}",
        25.0 * 4.0,
        medida_de(&cara1.signature)
    );
}

#[test]
fn editar_el_alto_de_un_rectangulo_conserva_su_cara_lateral_y_actualiza_el_area() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let (mut tree, _dim_ancho, dim_alto) =
        arbol_extrude_rectangulo(sketch_id, extrude_id, 10.0, 6.0, Plano::default(), DVec3::Z, 4.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    // Cara 1: el lado p1-p2, el que mide "alto" de largo.
    let cara0 = cara_lateral(&topo0, 1).expect("cara lateral 1");
    assert!((medida_de(&cara0.signature) - 6.0 * 4.0).abs() < 1e-9);
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        assert!(s.modelo.set_dimension(dim_alto, 22.0), "dimension desconocida");
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_lateral(&topo1, 1).expect("cara lateral 1 tras el cambio");
    assert!(
        (medida_de(&cara1.signature) - 22.0 * 4.0).abs() < 1e-6,
        "area esperada {}, salio {}",
        22.0 * 4.0,
        medida_de(&cara1.signature)
    );
}

#[test]
fn editar_las_dimensiones_de_una_caja_conserva_sus_seis_caras_por_indice() {
    let k = kernel();
    let caja_id = fid(1);
    let mut tree = arbol_caja(caja_id, DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let topo0 = topologia_de(&k, &tree, caja_id);

    // Normales esperadas, fijas por el contrato del stub (`ops::caja`, doc de
    // la funcion): 0=-Z, 1=+Z, 2=-Y, 3=+Y, 4=+X, 5=-X.
    let normales_esperadas = [
        DVec3::NEG_Z,
        DVec3::Z,
        DVec3::NEG_Y,
        DVec3::Y,
        DVec3::X,
        DVec3::NEG_X,
    ];
    let referencias: Vec<_> = (0..6u32)
        .map(|i| {
            let c = cara_primitiva(&topo0, i).unwrap_or_else(|| panic!("cara primitiva {i}"));
            TopoRef::capturar(caja_id, &c)
        })
        .collect();

    set_caja(
        &mut tree,
        caja_id,
        DVec3::new(1.0, 2.0, 3.0),
        DVec3::new(15.0, 9.0, 50.0),
    );
    let topo1 = topologia_de(&k, &tree, caja_id);

    for (i, referencia) in referencias.iter().enumerate() {
        let resolucion = Resolver::default().resolver(referencia, &topo1);
        assert!(
            resolucion.exacta(),
            "cara {i}: se esperaba genealogia exacta, salio {:?}",
            resolucion.binding
        );
        assert_eq!(resolucion.valor(), Some(referencia.objetivo));
        let cara1 = cara_primitiva(&topo1, i as u32).unwrap();
        assert!(
            (normal_de(&cara1.signature) - normales_esperadas[i]).length() < 1e-6,
            "cara {i}: normal deberia seguir siendo {:?}, salio {:?}",
            normales_esperadas[i],
            normal_de(&cara1.signature)
        );
    }
}

#[test]
fn trasladar_una_caja_manteniendo_su_tamano_conserva_una_cara_y_desplaza_su_centroide() {
    let k = kernel();
    let caja_id = fid(1);
    let tam = DVec3::new(10.0, 20.0, 30.0);
    let mut tree = arbol_caja(caja_id, DVec3::ZERO, tam);
    let topo0 = topologia_de(&k, &tree, caja_id);
    let cara0 = cara_primitiva(&topo0, 4).unwrap(); // +X
    let referencia = TopoRef::capturar(caja_id, &cara0);
    let centro0 = centro_de(&cara0.signature);

    let despl = DVec3::new(5.0, -3.0, 12.0);
    set_caja(&mut tree, caja_id, despl, despl + tam);
    let topo1 = topologia_de(&k, &tree, caja_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let cara1 = cara_primitiva(&topo1, 4).unwrap();
    let centro1 = centro_de(&cara1.signature);
    assert!((centro1 - (centro0 + despl)).length() < 5e-3);
}

#[test]
fn editar_el_radio_de_un_cilindro_conserva_sus_tres_caras_y_actualiza_el_area_lateral() {
    let k = kernel();
    let cil_id = fid(1);
    let mut tree = arbol_cilindro(cil_id, DVec3::ZERO, DVec3::Z, 10.0, 20.0);
    let topo0 = topologia_de(&k, &tree, cil_id);
    let lateral0 = cara_primitiva(&topo0, 0).unwrap();
    let referencia = TopoRef::capturar(cil_id, &lateral0);
    let pi = std::f64::consts::PI;
    // Tolerancia al nivel de la cuantizacion de la firma (QUANTUM_MM=1e-3),
    // no del calculo: el area exacta es analitica, pero `medida_de` pasa por
    // la firma cuantizada antes de llegar aqui.
    assert!((medida_de(&lateral0.signature) - 2.0 * pi * 10.0 * 20.0).abs() < 1e-3);

    set_cilindro(&mut tree, cil_id, DVec3::ZERO, DVec3::Z, 15.0, 20.0);
    let topo1 = topologia_de(&k, &tree, cil_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let lateral1 = cara_primitiva(&topo1, 0).unwrap();
    assert!((medida_de(&lateral1.signature) - 2.0 * pi * 15.0 * 20.0).abs() < 1e-3);
}

#[test]
fn editar_la_altura_de_un_cilindro_conserva_la_tapa_superior_y_sube_su_centroide() {
    let k = kernel();
    let cil_id = fid(1);
    let mut tree = arbol_cilindro(cil_id, DVec3::ZERO, DVec3::Z, 10.0, 20.0);
    let topo0 = topologia_de(&k, &tree, cil_id);
    let tapa0 = cara_primitiva(&topo0, 2).unwrap(); // tapa superior
    let referencia = TopoRef::capturar(cil_id, &tapa0);
    assert!((centro_de(&tapa0.signature) - DVec3::new(0.0, 0.0, 20.0)).length() < 1e-6);

    set_cilindro(&mut tree, cil_id, DVec3::ZERO, DVec3::Z, 10.0, 35.0);
    let topo1 = topologia_de(&k, &tree, cil_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let tapa1 = cara_primitiva(&topo1, 2).unwrap();
    assert!((centro_de(&tapa1.signature) - DVec3::new(0.0, 0.0, 35.0)).length() < 1e-6);
}

#[test]
fn rotar_el_eje_de_un_cilindro_conserva_identidad_y_reorienta_la_normal_de_la_tapa() {
    let k = kernel();
    let cil_id = fid(1);
    let mut tree = arbol_cilindro(cil_id, DVec3::ZERO, DVec3::Z, 10.0, 20.0);
    let topo0 = topologia_de(&k, &tree, cil_id);
    let tapa0 = cara_primitiva(&topo0, 2).unwrap();
    let referencia = TopoRef::capturar(cil_id, &tapa0);

    set_cilindro(&mut tree, cil_id, DVec3::ZERO, DVec3::X, 10.0, 20.0);
    let topo1 = topologia_de(&k, &tree, cil_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    let tapa1 = cara_primitiva(&topo1, 2).unwrap();
    assert!(
        (normal_de(&tapa1.signature) - DVec3::X).length() < 1e-3,
        "salio {:?}",
        normal_de(&tapa1.signature)
    );
}

/// Sketch → Extrude → Fillet → Chamfer, y una edición dos niveles aguas
/// arriba de la operación más profunda. Vale la pena comprobarlo a esta
/// profundidad y no solo con un nivel: es donde la propiedad "el mark es
/// estructural, no numérico" (comentario grande al principio del archivo)
/// deja de ser una afirmación de diseño y pasa a ser algo medido.
#[test]
fn una_cadena_de_fillet_y_chamfer_sobrevive_intacta_un_cambio_de_cota_dos_niveles_aguas_arriba() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, altura,
    );
    let ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let _ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.3, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    // Una referencia adicional, capturada dos niveles mas abajo, sobre una
    // cara que ninguna de las dos operaciones toca (normal hacia 90 grados,
    // lejos de las aristas biseladas a 0 y 180).
    let topo_c0 = topologia_de(&k, &tree, chamfer_id);
    let sonda0 = cara_normal_mas_parecida(
        &topo_c0,
        DVec3::new(90f64.to_radians().cos(), 90f64.to_radians().sin(), 0.0),
    );
    let referencia_sonda = TopoRef::capturar(chamfer_id, &sonda0);

    set_distancia(&mut tree, extrude_id, 11.0);

    // La arista propia del fillet, dos pasos aguas abajo de la cota editada.
    let r_fillet = medir("cadena_profunda_fillet", &k, &tree, fillet_id);
    assert!(r_fillet.exacta(), "salio {:?}", r_fillet.binding);
    assert_eq!(r_fillet.valor(), Some(ref_fillet.objetivo));

    // La sonda, capturada sobre la salida del chaflan (tres pasos aguas
    // abajo de la cota editada, y detras de dos operaciones que reasignan
    // el StableId de *todas* las caras heredadas, la toquen o no).
    let topo_c1 = topologia_de(&k, &tree, chamfer_id);
    let resolucion_sonda = Resolver::default().resolver(&referencia_sonda, &topo_c1);
    assert!(
        resolucion_sonda.exacta(),
        "la sonda deberia seguir resolviendo por linaje exacto tras un simple cambio de cota, salio {:?}",
        resolucion_sonda.binding
    );
    assert_eq!(resolucion_sonda.valor(), Some(referencia_sonda.objetivo));
}
