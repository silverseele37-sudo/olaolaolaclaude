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

// ---------------------------------------------------------------------------
// Sección 2 — número de lados de un polígono, en las dos direcciones.
// ---------------------------------------------------------------------------

/// **Límite del nombrado, documentado.** Aumentar el número de lados de un
/// polígono conserva la genealogía por índice de perfil, aunque la arista
/// física que ese índice describe pase a ser otra completamente distinta.
/// No es un fallo del resolver: la capa 1 es, por diseño, identidad por
/// índice estructural, no por posición en el espacio (comentario grande al
/// principio del archivo). Pero es exactamente la clase de sorpresa
/// silenciosa contra la que ADR-0002 previene con la capa 2 — salvo que
/// aquí la capa 1 nunca llega a ceder el turno, porque el índice 3 sigue
/// existiendo. Vale la pena que quede escrito y comprobado, no solo asumido.
#[test]
fn aumentar_los_lados_de_un_poligono_conserva_la_identidad_por_indice_aunque_la_arista_fisica_cambie(
) {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 3).expect("hexagono: cara lateral 3");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.modelo.points = puntos_poligono(8, 10.0);
        s.perfil = (0..8u32).map(forge_kernel_api::sketch::PointId).collect();
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta por indice, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));

    // Y sin embargo: hexagono, lado 3, normal a 210 grados; octogono, lado
    // 3, normal a 157.5 grados. Mas de 50 grados de diferencia: claramente
    // otra arista fisica, con la misma identidad.
    let cara1 = cara_lateral(&topo1, 3).expect("octogono: cara lateral 3");
    let cos = normal_de(&cara0.signature).dot(normal_de(&cara1.signature));
    assert!(
        cos < 0.65,
        "las dos caras deberian mirar a sitios bien distintos, cos={cos}"
    );
}

/// El mismo tipo de edición, en la dirección que sí rompe la genealogía:
/// reducir un octógono a un hexágono hace que el índice 7 deje de existir
/// (solo quedan 0..5). No es un caso adversarial como el de
/// `control_negativo_naming.rs` (que gira el triángulo a propósito para
/// garantizar la ruptura): aquí el polígono resultante queda en su
/// orientación por defecto, y aun así hay una única cara del hexágono cuya
/// normal cae dentro del margen — la capa 2 la encuentra sola, sin ambigüedad.
///
/// Derivación (radio 10, altura 5, sin rotar nada):
/// - octógono, lado 7: normal a 337.5° (medio camino entre los vértices 7 y 0).
/// - hexágono, lado 5: normal a 330°. Diferencia 7.5°, cos ≈ 0.991.
/// - el siguiente lado más cercano (lado 0, a 30°) está a 52.5° de distancia
///   angular, cos ≈ 0.609 — por debajo del coseno mínimo (0.90): ni siquiera
///   entra en la lista de candidatas. Una sola candidata, sin ambigüedad
///   posible.
#[test]
fn reducir_un_octogono_a_hexagono_revincula_por_firma_a_la_cara_geometricamente_correcta() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 8, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 7).expect("octogono: cara lateral 7");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.modelo.points = puntos_poligono(6, 10.0);
        s.perfil = (0..6u32).map(forge_kernel_api::sketch::PointId).collect();
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    let esperada = cara_lateral(&topo1, 5).expect("hexagono: cara lateral 5");
    match resolucion.binding {
        Binding::Rebound { value, .. } => assert_eq!(value, esperada.id),
        otro => panic!("se esperaba Rebound a la cara lateral 5, salio {otro:?}"),
    }
    assert!(!resolucion.exacta());
    assert!(!resolucion.rota());
}

/// Reducir aún más — de octógono a triángulo, en sus ángulos por defecto,
/// sin girarlo a propósito — puede dejar sin candidata orientada a una cara
/// que antes existía. Se elige el lado 5 (no el 2) a propósito: con `n=3`
/// solo sobreviven por índice los lados 0, 1 y 2, así que hace falta uno que
/// desaparezca de la lista **y además** quede mal orientado, para que sea la
/// capa 2 —no la capa 1— la que decida. La normal del octógono, lado 5,
/// apunta a 247.5°; las tres normales del triángulo (vértices en 0°, 120°,
/// 240° ⇒ normales de lado a 60°, 180°, 300°) quedan todas a más de 52° de
/// distancia angular, y cos(52.5°) ≈ 0.609 no llega al coseno mínimo (0.90).
/// Ninguna candidata pasa el filtro de orientación: la referencia se rompe,
/// tal como `naming.rs` dice que debe pasar antes que inventar una respuesta.
#[test]
fn reducir_un_octogono_a_triangulo_sin_candidata_orientada_rompe_la_referencia() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 8, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 5).expect("octogono: cara lateral 5");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.modelo.points = puntos_poligono(3, 10.0);
        s.perfil = (0..3u32).map(forge_kernel_api::sketch::PointId).collect();
    }
    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);

    assert!(
        matches!(resolucion.binding, Binding::Broken),
        "se esperaba Binding::Broken, salio {:?} (puntuacion {:?})",
        resolucion.binding,
        resolucion.puntuacion
    );
    assert_eq!(resolucion.valor(), None);
}

// ---------------------------------------------------------------------------
// Sección 3 — supresión de un nodo intermedio.
//
// Nota sobre el radio elegido (30, no 10 como en el resto del archivo): con
// un hexágono de radio 10 y altura 6 este caso salía **`Broken` por
// ambigüedad**, no `Rebound` — un hallazgo real, no un error de cálculo.
// `Resolver::parecido` pesa la normal y la medida al 80 % combinado y la
// posición solo al 20 % (a propósito: la posición es justo lo que una
// edición típica desplaza). Pero las seis aristas verticales de *cualquier*
// prisma comparten dirección y longitud exactas entre sí — son la misma
// pieza repetida alrededor del eje — así que ahí la posición es el **único**
// término que puede distinguir una candidata de otra, y con un radio
// pequeño frente a la altura, la vecina más cercana (a 60°) queda demasiado
// cerca en puntuación (0.875 contra el 1.0 de la correcta) para superar el
// margen (0.15): dos candidatas empatan y la referencia se rompe, aunque una
// de las dos sea un acierto geométrico exacto. Subir el radio (más separación
// física entre aristas vecinas) lo despeja. Esto **no** es un bug de
// `forge-param` — es la consecuencia correcta, aunque no obvia, de una
// fórmula de pesos razonable — pero sí es un límite real: **las aristas de
// piezas con simetría rotacional son inusualmente propensas a romperse por
// ambigüedad en la capa 2**, incluso cuando la candidata correcta es un
// acierto perfecto. Ver el informe final para más detalle.
// ---------------------------------------------------------------------------

/// Suprimir un `Fillet` intermedio no lo borra: sus parámetros siguen ahí,
/// pero su salida pasa a ser la de su propia entrada sin tocarla (`tree.rs`,
/// doc de `suprimir`). El `Chamfer` que colgaba de él pasa entonces a leer
/// la topología **cruda** del extrude, sin el bisel de por medio. El mark
/// de las caras heredadas del fillet ya no existe — pero la geometría en ese
/// punto es idéntica (el fillet estaba en el lado opuesto del prisma), así
/// que la capa 2 la encuentra exacta.
#[test]
fn suprimir_un_fillet_intermedio_revincula_la_arista_del_chamfer_por_firma() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    let r0 = medir("antes_de_suprimir", &k, &tree, chamfer_id);
    assert!(r0.exacta());
    assert_eq!(r0.valor(), Some(ref_chamfer.objetivo));

    tree.suprimir(fillet_id, true).unwrap();

    let topo_extrude_cruda = topologia_de(&k, &tree, extrude_id);
    let esperado = arista_vertical_en(&topo_extrude_cruda, altura, 180.0);

    let r1 = medir("tras_suprimir_el_fillet", &k, &tree, chamfer_id);
    assert!(!r1.rota());
    assert!(
        !r1.exacta(),
        "deberia haber perdido el linaje exacto al suprimir el nodo intermedio, salio {:?}",
        r1.binding
    );
    assert_eq!(r1.valor(), Some(esperado.id));
}

/// La misma situación con los roles cambiados: un `Chamfer` suprimido en
/// medio de la cadena, y un `Fillet` posterior que pierde su linaje.
#[test]
fn suprimir_un_chamfer_intermedio_revincula_la_arista_de_un_fillet_posterior() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let chamfer_id = fid(3);
    let fillet_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, extrude_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let ref_fillet = agregar_fillet_sobre(&k, &mut tree, chamfer_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    let r0 = medir("antes_de_suprimir", &k, &tree, fillet_id);
    assert!(r0.exacta());
    assert_eq!(r0.valor(), Some(ref_fillet.objetivo));

    tree.suprimir(chamfer_id, true).unwrap();

    let topo_extrude_cruda = topologia_de(&k, &tree, extrude_id);
    let esperado = arista_vertical_en(&topo_extrude_cruda, altura, 180.0);

    let r1 = medir("tras_suprimir_el_chamfer", &k, &tree, fillet_id);
    assert!(!r1.rota());
    assert!(
        !r1.exacta(),
        "deberia haber perdido el linaje exacto al suprimir el nodo intermedio, salio {:?}",
        r1.binding
    );
    assert_eq!(r1.valor(), Some(esperado.id));
}

/// Suprimir no destruye parámetros (`tree.rs`): quitar la supresión tiene
/// que devolver **exactamente** la genealogía original, no una que
/// "casualmente" apunte al mismo sitio por firma.
#[test]
fn des_suprimir_un_nodo_restaura_la_genealogia_exacta_original() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    tree.suprimir(fillet_id, true).unwrap();
    let r_suprimido = medir("suprimido", &k, &tree, chamfer_id);
    assert!(
        !r_suprimido.exacta(),
        "control: debe haber perdido el linaje mientras esta suprimido"
    );

    tree.suprimir(fillet_id, false).unwrap();
    let r_restaurado = medir("restaurado", &k, &tree, chamfer_id);
    assert!(
        r_restaurado.exacta(),
        "al quitar la supresion deberia recuperar el linaje exacto, salio {:?}",
        r_restaurado.binding
    );
    assert_eq!(r_restaurado.valor(), Some(ref_chamfer.objetivo));
}

// ---------------------------------------------------------------------------
// Sección 4 — reordenación de dos nodos consecutivos de una cadena. El mismo
// radio grande que la sección 3, y por la misma razón (ver su comentario).
// ---------------------------------------------------------------------------

/// El "peor caso del nombrado persistente" que documenta `tree.rs`:
/// intercambiar dos nodos consecutivos de una cadena no cambia ni un
/// parámetro, pero cambia la genealogía de todo lo que hay debajo. Aquí, la
/// propia arista que el chaflán bisela: antes del intercambio la ve a través
/// del fillet (una capa de "mark" reindexado de más); después, directamente
/// sobre la extrusión cruda (ninguna). El mark no coincide, la geometría sí.
#[test]
fn intercambiar_fillet_y_chamfer_hace_que_la_propia_arista_del_chamfer_se_revincule_por_firma() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    let r0 = medir("antes_de_intercambiar", &k, &tree, chamfer_id);
    assert!(r0.exacta());
    assert_eq!(r0.valor(), Some(ref_chamfer.objetivo));

    tree.intercambiar_en_la_cadena(fillet_id, chamfer_id).unwrap();

    let topo_extrude_cruda = topologia_de(&k, &tree, extrude_id);
    let esperado = arista_vertical_en(&topo_extrude_cruda, altura, 180.0);

    let r1 = medir("tras_intercambiar", &k, &tree, chamfer_id);
    assert!(!r1.rota());
    assert!(
        !r1.exacta(),
        "el intercambio deberia haber roto el linaje exacto, salio {:?}",
        r1.binding
    );
    assert_eq!(r1.valor(), Some(esperado.id));
}

/// Y una referencia que no tiene nada que ver con ninguna de las dos
/// operaciones — una cara que ni el fillet ni el chaflán tocan — también
/// pierde la genealogía exacta con el intercambio (el mark de **todas** las
/// caras heredadas se re-deriva en cada bisel, la toquen o no), pero la
/// geometría en ese punto es bit a bit idéntica sea cual sea el orden: la
/// capa 2 la recupera con la confianza más alta posible.
#[test]
fn intercambiar_fillet_y_chamfer_revincula_por_firma_una_sonda_ajena_a_las_dos_operaciones() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let _ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    let topo_c0 = topologia_de(&k, &tree, chamfer_id);
    let sonda0 = arista_vertical_en(&topo_c0, altura, 90.0);
    let referencia = TopoRef::capturar(chamfer_id, &sonda0);

    tree.intercambiar_en_la_cadena(fillet_id, chamfer_id).unwrap();

    // Tras el intercambio el ultimo nodo de la cadena es el fillet.
    let topo_final = topologia_de(&k, &tree, fillet_id);
    let esperado = arista_vertical_en(&topo_final, altura, 90.0);
    let resolucion = Resolver::default().resolver(&referencia, &topo_final);

    assert!(!resolucion.rota());
    assert!(
        !resolucion.exacta(),
        "tambien deberia perder el linaje exacto, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(esperado.id));
    assert!(
        resolucion.puntuacion.unwrap() > 0.999,
        "geometria identica deberia dar practicamente 1.0 de parecido, salio {:?}",
        resolucion.puntuacion
    );
}

/// Intercambiar y deshacer el intercambio recupera exactamente el cableado
/// original (`entrada` de cada nodo y orden de presentación), sin tocar
/// ningún parámetro: por eso la genealogía tiene que volver a ser la misma
/// de antes, no una que "por casualidad" apunte al mismo sitio por firma.
#[test]
fn intercambiar_y_deshacer_el_intercambio_recupera_la_genealogia_original() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let chamfer_id = fid(4);
    let altura = 6.0;
    let mut tree = arbol_extrude_poligono(
        sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
    );
    let _ref_fillet = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        arista_vertical_en(t, altura, 0.0)
    });
    let ref_chamfer = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
        arista_vertical_en(t, altura, 180.0)
    });

    let r0 = medir("antes_de_intercambiar", &k, &tree, chamfer_id);
    assert!(r0.exacta());
    assert_eq!(r0.valor(), Some(ref_chamfer.objetivo));

    tree.intercambiar_en_la_cadena(fillet_id, chamfer_id).unwrap();
    let r_intermedio = medir("intercambiado", &k, &tree, chamfer_id);
    assert!(
        !r_intermedio.exacta(),
        "control: debe haber perdido el linaje mientras esta intercambiado"
    );

    tree.intercambiar_en_la_cadena(chamfer_id, fillet_id).unwrap();
    let r1 = medir("tras_ida_y_vuelta", &k, &tree, chamfer_id);
    assert!(
        r1.exacta(),
        "tras deshacer el intercambio deberia volver a resolver por linaje, salio {:?}",
        r1.binding
    );
    assert_eq!(r1.valor(), Some(ref_chamfer.objetivo));
}

// ---------------------------------------------------------------------------
// Sección 5 — combinaciones. Medir cada tipo de edición por separado no basta:
// un usuario real cambia varias cosas entre una regeneración y la siguiente.
// ---------------------------------------------------------------------------

#[test]
fn multiples_ediciones_de_cota_consecutivas_mantienen_estable_la_misma_referencia() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 5, 10.0, Plano::default(), DVec3::Z, 4.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 1).expect("cara lateral 1");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    for distancia in [7.0, 2.0, 15.0, 4.0, 9.5] {
        set_distancia(&mut tree, extrude_id, distancia);
        let topo = topologia_de(&k, &tree, extrude_id);
        let resolucion = Resolver::default().resolver(&referencia, &topo);
        assert!(
            resolucion.exacta(),
            "distancia={distancia}: salio {:?}",
            resolucion.binding
        );
        assert_eq!(resolucion.valor(), Some(referencia.objetivo));
    }
}

#[test]
fn editar_radio_y_altura_de_un_cilindro_a_la_vez_conserva_las_tres_caras() {
    let k = kernel();
    let cil_id = fid(1);
    let mut tree = arbol_cilindro(cil_id, DVec3::ZERO, DVec3::Z, 8.0, 12.0);
    let topo0 = topologia_de(&k, &tree, cil_id);
    let referencias: Vec<_> = (0..3u32)
        .map(|i| TopoRef::capturar(cil_id, &cara_primitiva(&topo0, i).unwrap()))
        .collect();

    set_cilindro(&mut tree, cil_id, DVec3::ZERO, DVec3::Z, 20.0, 3.0);
    let topo1 = topologia_de(&k, &tree, cil_id);
    for (i, referencia) in referencias.iter().enumerate() {
        let resolucion = Resolver::default().resolver(referencia, &topo1);
        assert!(
            resolucion.exacta(),
            "cara {i}: salio {:?}",
            resolucion.binding
        );
        assert_eq!(resolucion.valor(), Some(referencia.objetivo));
    }
}

#[test]
fn mover_el_sketch_y_cambiar_la_distancia_de_extrusion_a_la_vez_conserva_la_referencia() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 7, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let cara0 = cara_lateral(&topo0, 3).expect("cara lateral 3");
    let referencia = TopoRef::capturar(extrude_id, &cara0);

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.plano = mover_plano(s.plano, DVec3::new(2.0, 5.0, -3.0), 0.7);
    }
    set_distancia(&mut tree, extrude_id, 13.0);

    let topo1 = topologia_de(&k, &tree, extrude_id);
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        resolucion.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        resolucion.binding
    );
    assert_eq!(resolucion.valor(), Some(referencia.objetivo));
}

#[test]
fn cambiar_ancho_y_alto_de_un_rectangulo_a_la_vez_conserva_sus_dos_caras_laterales() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let (mut tree, dim_ancho, dim_alto) =
        arbol_extrude_rectangulo(sketch_id, extrude_id, 10.0, 6.0, Plano::default(), DVec3::Z, 4.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let ref0 = TopoRef::capturar(extrude_id, &cara_lateral(&topo0, 0).unwrap());
    let ref1 = TopoRef::capturar(extrude_id, &cara_lateral(&topo0, 1).unwrap());

    set_ancho(&mut tree, sketch_id, dim_ancho, 30.0);
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        assert!(s.modelo.set_dimension(dim_alto, 18.0), "dimension desconocida");
    }

    let topo1 = topologia_de(&k, &tree, extrude_id);
    for (i, referencia) in [(0, &ref0), (1, &ref1)] {
        let resolucion = Resolver::default().resolver(referencia, &topo1);
        assert!(
            resolucion.exacta(),
            "cara {i}: salio {:?}",
            resolucion.binding
        );
        assert_eq!(resolucion.valor(), Some(referencia.objetivo));
    }
}

#[test]
fn un_poligono_de_diecisiete_lados_conserva_sus_referencias_bajo_un_cambio_de_cota() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let mut tree =
        arbol_extrude_poligono(sketch_id, extrude_id, 17, 10.0, Plano::default(), DVec3::Z, 5.0);
    let topo0 = topologia_de(&k, &tree, extrude_id);
    let referencias: Vec<_> = [0u32, 8, 16]
        .iter()
        .map(|&i| TopoRef::capturar(extrude_id, &cara_lateral(&topo0, i).unwrap()))
        .collect();

    set_distancia(&mut tree, extrude_id, 21.0);
    let topo1 = topologia_de(&k, &tree, extrude_id);
    for referencia in &referencias {
        let resolucion = Resolver::default().resolver(referencia, &topo1);
        assert!(
            resolucion.exacta(),
            "salio {:?}",
            resolucion.binding
        );
        assert_eq!(resolucion.valor(), Some(referencia.objetivo));
    }
}

// ---------------------------------------------------------------------------
// Sección 6 — la medición agregada. Aquí es donde el criterio de aceptación
// de ADR-0002 §6 (≥95 % en cambios de cota típicos) deja de ser una promesa y
// pasa a ser un número que este test recalcula cada vez que corre.
// ---------------------------------------------------------------------------

/// Una batería de cambios de cota típicos — distancia de extrusión en
/// polígonos de distinto número de lados, posición del sketch, dimensiones
/// de caja, radio/altura de cilindro y dirección de extrusión — medida de
/// una sola vez con [`tasa_de_revinculacion`]. Es el `assert` que decide si
/// `forge-param` sirve para lo que dice ADR-0002 §6.
#[test]
fn la_tasa_de_revinculacion_en_cambios_de_cota_tipicos_alcanza_el_criterio_de_aceptacion() {
    let mut m = Medidor::default();
    let k = kernel();

    // 1) distancia de extrusion, en poligonos de distinto numero de lados,
    //    creciendo y encogiendo.
    for n in [3u32, 5, 6, 8, 12] {
        for (d0, d1) in [(5.0, 9.0), (5.0, 1.5), (10.0, 10.001)] {
            let sketch_id = fid(1);
            let extrude_id = fid(2);
            let mut tree = arbol_extrude_poligono(
                sketch_id, extrude_id, n, 10.0, Plano::default(), DVec3::Z, d0,
            );
            let topo0 = topologia_de(&k, &tree, extrude_id);
            let cara = cara_lateral(&topo0, n / 2).expect("cara lateral");
            let referencia = TopoRef::capturar(extrude_id, &cara);
            set_distancia(&mut tree, extrude_id, d1);
            let topo1 = topologia_de(&k, &tree, extrude_id);
            m.tipico(Resolver::default().resolver(&referencia, &topo1));
        }
    }

    // 2) posicion del sketch: traslaciones y rotaciones variadas.
    for (tras, ang) in [
        (DVec3::new(1.0, 0.0, 0.0), 0.0),
        (DVec3::ZERO, 0.3),
        (DVec3::new(-3.0, 4.0, 2.0), 1.1),
        (DVec3::new(0.0, 0.0, 50.0), 0.0),
    ] {
        let sketch_id = fid(1);
        let extrude_id = fid(2);
        let mut tree = arbol_extrude_poligono(
            sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0,
        );
        let topo0 = topologia_de(&k, &tree, extrude_id);
        let cara = cara_lateral(&topo0, 2).unwrap();
        let referencia = TopoRef::capturar(extrude_id, &cara);
        if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
            s.plano = mover_plano(s.plano, tras, ang);
        }
        let topo1 = topologia_de(&k, &tree, extrude_id);
        m.tipico(Resolver::default().resolver(&referencia, &topo1));
    }

    // 3) cajas: variaciones de min/max.
    for (min0, max0, min1, max1) in [
        (
            DVec3::ZERO,
            DVec3::new(10.0, 10.0, 10.0),
            DVec3::ZERO,
            DVec3::new(50.0, 10.0, 10.0),
        ),
        (
            DVec3::ZERO,
            DVec3::new(10.0, 10.0, 10.0),
            DVec3::new(-5.0, -5.0, -5.0),
            DVec3::new(5.0, 5.0, 5.0),
        ),
        (
            DVec3::new(1.0, 1.0, 1.0),
            DVec3::new(2.0, 2.0, 2.0),
            DVec3::new(1.0, 1.0, 1.0),
            DVec3::new(2.0, 20.0, 2.0),
        ),
    ] {
        let caja_id = fid(1);
        let mut tree = arbol_caja(caja_id, min0, max0);
        let topo0 = topologia_de(&k, &tree, caja_id);
        let cara = cara_primitiva(&topo0, 4).unwrap();
        let referencia = TopoRef::capturar(caja_id, &cara);
        set_caja(&mut tree, caja_id, min1, max1);
        let topo1 = topologia_de(&k, &tree, caja_id);
        m.tipico(Resolver::default().resolver(&referencia, &topo1));
    }

    // 4) cilindros: radio y altura.
    for (r0, h0, r1, h1) in [
        (10.0, 20.0, 15.0, 20.0),
        (10.0, 20.0, 10.0, 45.0),
        (5.0, 5.0, 30.0, 2.0),
    ] {
        let cil_id = fid(1);
        let mut tree = arbol_cilindro(cil_id, DVec3::ZERO, DVec3::Z, r0, h0);
        let topo0 = topologia_de(&k, &tree, cil_id);
        let cara = cara_primitiva(&topo0, 0).unwrap();
        let referencia = TopoRef::capturar(cil_id, &cara);
        set_cilindro(&mut tree, cil_id, DVec3::ZERO, DVec3::Z, r1, h1);
        let topo1 = topologia_de(&k, &tree, cil_id);
        m.tipico(Resolver::default().resolver(&referencia, &topo1));
    }

    // 5) direccion de extrusion.
    for dir in [
        DVec3::X,
        DVec3::new(1.0, 1.0, 1.0).normalize(),
        DVec3::new(0.0, 1.0, 0.0),
    ] {
        let sketch_id = fid(1);
        let extrude_id = fid(2);
        let mut tree = arbol_extrude_poligono(
            sketch_id, extrude_id, 6, 10.0, Plano::default(), DVec3::Z, 5.0,
        );
        let topo0 = topologia_de(&k, &tree, extrude_id);
        let cara = cara_lateral(&topo0, 1).unwrap();
        let referencia = TopoRef::capturar(extrude_id, &cara);
        set_direccion(&mut tree, extrude_id, dir);
        let topo1 = topologia_de(&k, &tree, extrude_id);
        m.tipico(Resolver::default().resolver(&referencia, &topo1));
    }

    let tasa = m.tasa_tipicos();
    let ok = m.tipicos.iter().filter(|r| !r.rota()).count();
    let total = m.tipicos.len();
    assert!(
        total >= 15,
        "la muestra ({total} casos) es demasiado pequena para que el porcentaje signifique algo"
    );
    assert!(
        tasa >= 0.95,
        "tasa de re-vinculacion en cambios de cota tipicos: {:.1}% ({ok} de {total}); \
         por debajo del 95% que exige ADR-0002 §6",
        tasa * 100.0
    );
    eprintln!(
        "tasa de re-vinculacion en cambios de cota tipicos: {:.1}% ({ok} de {total})",
        tasa * 100.0
    );
}

/// La misma medición, para cambios de **topología**: reducir el número de
/// lados de un polígono (la referencia puede sobrevivir por firma o
/// romperse, según quede o no una candidata sin ambigüedad — sección 2),
/// suprimir un nodo intermedio (sección 3) y reordenar dos nodos de una
/// cadena (sección 4).
///
/// ADR-0002 §6 no pide ningún porcentaje aquí — la sección 2 ya muestra un
/// caso natural de cada desenlace posible (`Rebound` y `Broken`), y eso es
/// exactamente lo esperable: un cambio de topología **puede** legítimamente
/// romper una referencia. Lo que este test aporta que los casos individuales
/// no muestran es el **número real** sobre una muestra más ancha, con un
/// piso de regresión: si una futura corrida saca menos que lo medido hoy,
/// algo empeoró.
#[test]
fn la_tasa_de_revinculacion_en_cambios_de_topologia_se_documenta_con_su_numero_real() {
    let mut m = Medidor::default();
    let k = kernel();

    // a) reducir el numero de lados de un poligono, referenciando el ultimo
    //    indice de la lista -- el que con mas probabilidad desaparece.
    for (n0, n1) in [(8u32, 6u32), (10, 4), (12, 3), (6, 3), (9, 5), (7, 4)] {
        let sketch_id = fid(1);
        let extrude_id = fid(2);
        let mut tree = arbol_extrude_poligono(
            sketch_id, extrude_id, n0, 10.0, Plano::default(), DVec3::Z, 5.0,
        );
        let topo0 = topologia_de(&k, &tree, extrude_id);
        let cara = cara_lateral(&topo0, n0 - 1).unwrap();
        let referencia = TopoRef::capturar(extrude_id, &cara);
        if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
            s.modelo.points = puntos_poligono(n1, 10.0);
            s.perfil = (0..n1).map(forge_kernel_api::sketch::PointId).collect();
        }
        let topo1 = topologia_de(&k, &tree, extrude_id);
        m.topologico(Resolver::default().resolver(&referencia, &topo1));
    }

    // b) supresion de un fillet intermedio, con la arista del chamfer en
    //    varios angulos distintos.
    for angulo_sonda in [60.0, 120.0, 240.0, 300.0] {
        let sketch_id = fid(1);
        let extrude_id = fid(2);
        let fillet_id = fid(3);
        let chamfer_id = fid(4);
        let altura = 6.0;
        let mut tree = arbol_extrude_poligono(
            sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
        );
        let _rf = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
            arista_vertical_en(t, altura, 0.0)
        });
        let _rc = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
            arista_vertical_en(t, altura, angulo_sonda)
        });
        tree.suprimir(fillet_id, true).unwrap();
        m.topologico(medir("supresion_agregada", &k, &tree, chamfer_id));
    }

    // c) reordenaciones, con las dos aristas en varios pares de angulos.
    for (angulo_a, angulo_b) in [(0.0, 180.0), (60.0, 240.0), (120.0, 300.0)] {
        let sketch_id = fid(1);
        let extrude_id = fid(2);
        let fillet_id = fid(3);
        let chamfer_id = fid(4);
        let altura = 6.0;
        let mut tree = arbol_extrude_poligono(
            sketch_id, extrude_id, 6, 30.0, Plano::default(), DVec3::Z, altura,
        );
        let _rf = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
            arista_vertical_en(t, altura, angulo_a)
        });
        let _rc = agregar_chamfer_sobre(&k, &mut tree, fillet_id, chamfer_id, 0.4, |t| {
            arista_vertical_en(t, altura, angulo_b)
        });
        tree.intercambiar_en_la_cadena(fillet_id, chamfer_id).unwrap();
        m.topologico(medir("reorden_agregado", &k, &tree, chamfer_id));
    }

    let tasa = m.tasa_topologicos();
    let ok = m.topologicos.iter().filter(|r| !r.rota()).count();
    let total = m.topologicos.len();
    eprintln!(
        "tasa de re-vinculacion en cambios de topologia: {:.1}% ({ok} de {total})",
        tasa * 100.0
    );
    // Piso de regresion, no promesa: medido hoy en 9/13 (~69.2%), casi todo
    // el deficit viene del grupo (a) -- reducir el numero de lados es, de
    // los tres tipos de cambio de topologia de esta suite, el que mas
    // referencias rompe de verdad (ver la seccion 2). Si esta cifra baja en
    // una corrida futura, algo en el resolver o en el stub cambio para peor
    // y hay que mirarlo -- no hay que subirlo "para que pase" sin entender
    // por que bajo.
    assert!(
        tasa >= 0.69,
        "la tasa en cambios de topologia bajo de lo medido hasta ahora: {:.1}% ({ok} de {total})",
        tasa * 100.0
    );
}
