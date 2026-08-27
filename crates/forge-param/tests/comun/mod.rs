//! Andamiaje compartido por los tests de `forge-param`.
//!
//! Nada de esto es parte de la API del crate: cada archivo de test de
//! `tests/` es su propio binario, y este módulo se incluye con `mod comun;`
//! en cada uno. No todos los archivos usan todo lo que hay aquí, de ahí el
//! `allow(dead_code)`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use forge_doc::{FeatureId, StableId};
use forge_kernel_api::sketch::{Constraint, DimId, PointId, SketchModel};
use forge_kernel_api::*;
use forge_kernel_stub::StubKernel;
use forge_math::{DAffine3, DVec2, DVec3};
use forge_param::*;

pub fn kernel() -> StubKernel {
    StubKernel::new()
}

/// Identidad determinista. Los tests no crean features de verdad: fabrican un
/// árbol a mano, así que un `FeatureId` legible (1, 2, 3...) hace los mensajes
/// de fallo comprensibles sin perder nada — `from_u128` es exactamente para
/// esto (ver `forge-doc::id`).
pub fn fid(n: u128) -> FeatureId {
    FeatureId::from_u128(n)
}

/// Evalúa con un evaluador nuevo (caché vacía). Es lo que quiere la mayoría de
/// los tests: aislar una evaluación de la siguiente.
///
/// Toma `&dyn GeometryKernel` y no `&StubKernel` a propósito: el test de
/// caché quiere pasar un [`KernelContado`], que envuelve al stub para contar
/// llamadas desde fuera, y no es del mismo tipo concreto.
pub fn evaluar(kernel: &dyn GeometryKernel, tree: &FeatureTree) -> Result<EvalOutcome> {
    Evaluator::new(kernel).evaluar(tree)
}

// ---------------------------------------------------------------------------
// Sketches
// ---------------------------------------------------------------------------

/// Vértices de un polígono regular de `n` lados y radio `r`, centrado en el
/// origen del plano local del sketch.
pub fn puntos_poligono(n: u32, r: f64) -> Vec<DVec2> {
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            DVec2::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

/// Sketch **sin restricciones**: los puntos ya son la solución. El solver, con
/// `m == 0` (sin ecuaciones), la devuelve intacta — ver `solver.rs::solve`.
/// Sirve para todos los casos que no necesitan editar una cota a través del
/// solver (mover el sketch, cambiar el número de lados, etc.).
pub fn sketch_libre(pts: Vec<DVec2>, plano: Plano) -> SketchNode {
    let n = pts.len() as u32;
    SketchNode {
        modelo: SketchModel {
            points: pts,
            ..Default::default()
        },
        perfil: (0..n).map(PointId).collect(),
        plano,
    }
}

/// Un rectángulo totalmente restringido: ancla en `p0`, horizontal/vertical en
/// los cuatro lados, dos cotas de distancia editables. Es a la vez el sketch de
/// "respuesta conocida" del solver y el que usan los casos de "cambiar una
/// cota" de la suite de regresión.
pub struct Rectangulo {
    pub sketch: SketchNode,
    pub dim_ancho: DimId,
    pub dim_alto: DimId,
}

pub fn sketch_rectangulo(ancho: f64, alto: f64, plano: Plano) -> Rectangulo {
    let mut m = SketchModel::default();
    let p0 = m.add_point(DVec2::new(0.0, 0.0));
    let p1 = m.add_point(DVec2::new(ancho, 0.0));
    let p2 = m.add_point(DVec2::new(ancho, alto));
    let p3 = m.add_point(DVec2::new(0.0, alto));
    let dim_ancho = m.add_dimension(ancho);
    let dim_alto = m.add_dimension(alto);
    m.constraints = vec![
        Constraint::Fixed(p0),
        Constraint::Horizontal(p0, p1),
        Constraint::Vertical(p1, p2),
        Constraint::Horizontal(p3, p2),
        Constraint::Vertical(p0, p3),
        Constraint::Distance {
            a: p0,
            b: p1,
            dim: dim_ancho,
        },
        Constraint::Distance {
            a: p0,
            b: p3,
            dim: dim_alto,
        },
    ];
    let sketch = SketchNode {
        modelo: m,
        perfil: vec![p0, p1, p2, p3],
        plano,
    };
    Rectangulo {
        sketch,
        dim_ancho,
        dim_alto,
    }
}

/// Traslada y rota (alrededor de Z) el plano de un sketch. Mover un sketch no
/// debe tocar ni una coordenada del modelo 2D (ADR-0002 §4 / doc de `tree::Plano`):
/// esta función construye el plano nuevo, nunca los puntos.
pub fn mover_plano(base: Plano, traslacion: DVec3, angulo_z_rad: f64) -> Plano {
    let (s, c) = angulo_z_rad.sin_cos();
    Plano {
        origen: base.origen + traslacion,
        eje_x: DVec3::new(c, s, 0.0),
        eje_y: DVec3::new(-s, c, 0.0),
    }
}

// ---------------------------------------------------------------------------
// Construcción de árboles
// ---------------------------------------------------------------------------

/// Sketch (polígono libre) → Extrude. La base de la mayoría de los casos de
/// la suite de regresión: lo que se edita luego es un parámetro de uno de
/// estos dos nodos.
pub fn arbol_extrude_poligono(
    sketch_id: FeatureId,
    extrude_id: FeatureId,
    n: u32,
    radio: f64,
    plano: Plano,
    direccion: DVec3,
    distancia_mm: f64,
) -> FeatureTree {
    let mut t = FeatureTree::new();
    t.insertar(FeatureNode::con_id(
        sketch_id,
        "sketch",
        NodeKind::Sketch(sketch_libre(puntos_poligono(n, radio), plano)),
    ));
    t.insertar(FeatureNode::con_id(
        extrude_id,
        "extrude",
        NodeKind::Extrude {
            perfil: sketch_id,
            direccion,
            distancia_mm,
            simetrico: false,
        },
    ));
    t
}

/// Igual que arriba, pero con un rectángulo totalmente restringido (para los
/// casos que editan una cota vía el solver). Devuelve también los `DimId`.
pub fn arbol_extrude_rectangulo(
    sketch_id: FeatureId,
    extrude_id: FeatureId,
    ancho: f64,
    alto: f64,
    plano: Plano,
    direccion: DVec3,
    distancia_mm: f64,
) -> (FeatureTree, DimId, DimId) {
    let mut t = FeatureTree::new();
    let r = sketch_rectangulo(ancho, alto, plano);
    t.insertar(FeatureNode::con_id(
        sketch_id,
        "sketch",
        NodeKind::Sketch(r.sketch),
    ));
    t.insertar(FeatureNode::con_id(
        extrude_id,
        "extrude",
        NodeKind::Extrude {
            perfil: sketch_id,
            direccion,
            distancia_mm,
            simetrico: false,
        },
    ));
    (t, r.dim_ancho, r.dim_alto)
}

/// Cambia el ancho de un sketch rectangular ya insertado en el árbol.
pub fn set_ancho(tree: &mut FeatureTree, sketch_id: FeatureId, dim: DimId, valor: f64) {
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).expect("sketch").kind {
        assert!(s.modelo.set_dimension(dim, valor), "dimension desconocida");
    } else {
        panic!("el nodo {sketch_id} no es un sketch");
    }
}

/// Cambia la dirección de un Extrude ya insertado.
pub fn set_direccion(tree: &mut FeatureTree, extrude_id: FeatureId, nueva: DVec3) {
    if let NodeKind::Extrude { direccion, .. } =
        &mut tree.nodo_mut(extrude_id).expect("extrude").kind
    {
        *direccion = nueva;
    } else {
        panic!("el nodo {extrude_id} no es un extrude");
    }
}

/// Cambia la distancia de un Extrude ya insertado.
pub fn set_distancia(tree: &mut FeatureTree, extrude_id: FeatureId, nueva: f64) {
    if let NodeKind::Extrude { distancia_mm, .. } =
        &mut tree.nodo_mut(extrude_id).expect("extrude").kind
    {
        *distancia_mm = nueva;
    } else {
        panic!("el nodo {extrude_id} no es un extrude");
    }
}

// ---------------------------------------------------------------------------
// Selección de entidades topológicas (lo que haría un usuario al hacer clic)
// ---------------------------------------------------------------------------

/// Des-cuantiza el centroide de una firma. Duplicado deliberado de la función
/// privada de `naming.rs`: los tests no deben depender de internals privados,
/// y esto son tres líneas sobre campos públicos de `GeometrySignature`.
pub fn centro_de(s: &GeometrySignature) -> DVec3 {
    let q = GeometrySignature::QUANTUM_MM;
    DVec3::new(
        s.centroid_q[0] as f64 * q,
        s.centroid_q[1] as f64 * q,
        s.centroid_q[2] as f64 * q,
    )
}

/// La arista de `topo` cuyo centroide está más lejos del punto `de`. Sirve
/// para elegir, sin hardcodear índices, una arista que una operación
/// localizada en *otra parte* del sólido (un fillet, un chaflán) no vaya a
/// tocar — para eso está garantizada su supervivencia por construcción.
pub fn arista_mas_lejana(topo: &TopologySummary, de: DVec3) -> TopoEntity {
    topo.edges
        .iter()
        .max_by(|a, b| {
            let da = (centro_de(&a.signature) - de).length();
            let db = (centro_de(&b.signature) - de).length();
            da.partial_cmp(&db).unwrap()
        })
        .cloned()
        .expect("la topologia no tiene aristas")
}

/// Vértices en ángulos arbitrarios (grados), mismo radio. Para construir
/// polígonos "girados" a propósito, en vez del regular que da
/// [`puntos_poligono`].
pub fn puntos_poligono_en(angulos_grados: &[f64], r: f64) -> Vec<DVec2> {
    angulos_grados
        .iter()
        .map(|&g| {
            let a = g.to_radians();
            DVec2::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

/// La cara lateral (`SweptFromProfileEdge`) nacida de la arista `indice` del
/// perfil. `None` si ese índice no existe en esta topología — que es
/// exactamente la situación que produce el control negativo del naming.
pub fn cara_lateral(topo: &TopologySummary, indice: u32) -> Option<TopoEntity> {
    topo.faces
        .iter()
        .find(|f| matches!(f.provenance, TopoProvenance::SweptFromProfileEdge { edge_index } if edge_index == indice))
        .cloned()
}

/// La cara de `topo` cuyo centroide está más cerca de `de`.
pub fn cara_mas_cercana(topo: &TopologySummary, de: DVec3) -> TopoEntity {
    topo.faces
        .iter()
        .min_by(|a, b| {
            let da = (centro_de(&a.signature) - de).length();
            let db = (centro_de(&b.signature) - de).length();
            da.partial_cmp(&db).unwrap()
        })
        .cloned()
        .expect("la topologia no tiene caras")
}

/// La arista de `topo` cuyo centroide está más cerca de `de`. Simétrica de
/// [`arista_mas_lejana`]; sirve de oráculo **independiente** del resolver en
/// la suite de regresión: para comprobar que una `Rebound` apunta a la
/// candidata geométricamente correcta hace falta un criterio de "correcto"
/// que no sea la propia fórmula de `Resolver::parecido`, o el test no
/// comprobaría nada que la implementación no pudiera fallar consigo misma.
pub fn arista_mas_cercana(topo: &TopologySummary, de: DVec3) -> TopoEntity {
    topo.edges
        .iter()
        .min_by(|a, b| {
            let da = (centro_de(&a.signature) - de).length();
            let db = (centro_de(&b.signature) - de).length();
            da.partial_cmp(&db).unwrap()
        })
        .cloned()
        .expect("la topologia no tiene aristas")
}

/// La cara de `topo` nacida directamente de una primitiva (caja o cilindro),
/// por su índice de `TopoProvenance::Primitive`. Análogo a [`cara_lateral`]
/// pero para las caras que no vienen de una extrusión.
pub fn cara_primitiva(topo: &TopologySummary, indice: u32) -> Option<TopoEntity> {
    topo.faces
        .iter()
        .find(|f| matches!(f.provenance, TopoProvenance::Primitive { index } if index == indice))
        .cloned()
}

/// Evalúa `tree` (evaluador nuevo), toma la topología de la forma que produjo
/// `entrada_id`, elige una entidad con `elegir`, y añade un nodo `Fillet` que
/// la referencia. Es la simulación de "el usuario selecciona una arista en el
/// visor y aplica un redondeo".
pub fn agregar_fillet_sobre(
    kernel: &dyn GeometryKernel,
    tree: &mut FeatureTree,
    entrada_id: FeatureId,
    fillet_id: FeatureId,
    radio_mm: f64,
    elegir: impl Fn(&TopologySummary) -> TopoEntity,
) -> TopoRef {
    let outcome = evaluar(kernel, tree).expect("el arbol base debe evaluar limpio");
    let shape = outcome
        .shape(entrada_id)
        .expect("la entrada no produjo forma");
    let topo = kernel.topology(shape).expect("topologia de la entrada");
    let entidad = elegir(&topo);
    let referencia = TopoRef::capturar(entrada_id, &entidad);
    tree.insertar(FeatureNode::con_id(
        fillet_id,
        "fillet",
        NodeKind::Fillet {
            entrada: entrada_id,
            aristas: vec![referencia],
            radio_mm,
        },
    ));
    referencia
}

/// Igual que [`agregar_fillet_sobre`] pero con un `Chamfer`.
pub fn agregar_chamfer_sobre(
    kernel: &dyn GeometryKernel,
    tree: &mut FeatureTree,
    entrada_id: FeatureId,
    chamfer_id: FeatureId,
    distancia_mm: f64,
    elegir: impl Fn(&TopologySummary) -> TopoEntity,
) -> TopoRef {
    let outcome = evaluar(kernel, tree).expect("el arbol base debe evaluar limpio");
    let shape = outcome
        .shape(entrada_id)
        .expect("la entrada no produjo forma");
    let topo = kernel.topology(shape).expect("topologia de la entrada");
    let entidad = elegir(&topo);
    let referencia = TopoRef::capturar(entrada_id, &entidad);
    tree.insertar(FeatureNode::con_id(
        chamfer_id,
        "chamfer",
        NodeKind::Chamfer {
            entrada: entrada_id,
            aristas: vec![referencia],
            distancia_mm,
        },
    ));
    referencia
}

/// Re-evalúa `tree` (evaluador nuevo) y devuelve la única `Resolucion` que
/// registró el nodo `nodo_ref` en esta pasada. Entra en pánico con un mensaje
/// legible si la evaluación falla o si el nodo no re-vinculó ninguna
/// referencia — que son, ambos, fallos de la construcción del caso de test y
/// no algo que un caso "típico" de la suite deba producir.
pub fn medir(
    nombre: &str,
    kernel: &dyn GeometryKernel,
    tree: &FeatureTree,
    nodo_ref: FeatureId,
) -> Resolucion {
    let outcome =
        evaluar(kernel, tree).unwrap_or_else(|e| panic!("{nombre}: la reevaluacion fallo: {e}"));
    let salida = outcome.salidas.get(&nodo_ref).unwrap_or_else(|| {
        panic!("{nombre}: el nodo de referencia {nodo_ref} no aparece en la salida")
    });
    assert_eq!(
        salida.resoluciones.len(),
        1,
        "{nombre}: se esperaba exactamente una resolucion en {nodo_ref}"
    );
    salida.resoluciones[0].clone()
}

// ---------------------------------------------------------------------------
// Kernel que cuenta llamadas — verificación independiente de la caché
// ---------------------------------------------------------------------------

/// Envuelve un `StubKernel` y cuenta las operaciones **constructivas** (las
/// que crean o modifican forma). Es la vara de medir independiente de la
/// caché: `EvalStats` es la contabilidad del propio sistema bajo prueba, y un
/// contador que viviera dentro de él no probaría nada por sí solo.
pub struct KernelContado<'k> {
    inner: &'k StubKernel,
    llamadas: AtomicUsize,
}

impl<'k> KernelContado<'k> {
    pub fn nuevo(inner: &'k StubKernel) -> Self {
        KernelContado {
            inner,
            llamadas: AtomicUsize::new(0),
        }
    }
    pub fn cuenta(&self) -> usize {
        self.llamadas.load(Ordering::SeqCst)
    }
    fn marca(&self) {
        self.llamadas.fetch_add(1, Ordering::SeqCst);
    }
}

impl<'k> GeometryKernel for KernelContado<'k> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn profile_from_polygon(&self, pts: &[DVec2], owner: FeatureId) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.profile_from_polygon(pts, owner)
    }
    fn extrude(
        &self,
        profile: ShapeId,
        opts: ExtrudeOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.extrude(profile, opts, owner)
    }
    fn revolve(
        &self,
        profile: ShapeId,
        opts: RevolveOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.revolve(profile, opts, owner)
    }
    fn box_solid(&self, min: DVec3, max: DVec3, owner: FeatureId) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.box_solid(min, max, owner)
    }
    fn cylinder(
        &self,
        base: DVec3,
        axis: DVec3,
        radius_mm: f64,
        height_mm: f64,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.cylinder(base, axis, radius_mm, height_mm, owner)
    }
    fn fillet(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: FilletSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.fillet(solid, edges, spec, owner)
    }
    fn chamfer(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: ChamferSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.chamfer(solid, edges, spec, owner)
    }
    fn boolean(
        &self,
        op: BoolOp,
        a: ShapeId,
        b: ShapeId,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.boolean(op, a, b, owner)
    }
    fn transform(&self, s: ShapeId, m: &DAffine3, owner: FeatureId) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.transform(s, m, owner)
    }
    fn topology(&self, s: ShapeId) -> KernelResult<TopologySummary> {
        self.inner.topology(s)
    }
    fn mass_properties(&self, s: ShapeId) -> KernelResult<MassProperties> {
        self.inner.mass_properties(s)
    }
    fn bbox(&self, s: ShapeId) -> KernelResult<forge_math::Aabb> {
        self.inner.bbox(s)
    }
    fn is_valid(&self, s: ShapeId) -> KernelResult<ValidationReport> {
        self.inner.is_valid(s)
    }
    fn tessellate(&self, s: ShapeId, p: &TessellationParams) -> KernelResult<Tessellation> {
        self.inner.tessellate(s, p)
    }
    fn serialize(&self, s: ShapeId) -> KernelResult<Vec<u8>> {
        self.inner.serialize(s)
    }
    fn deserialize(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<ShapeId> {
        self.marca();
        self.inner.deserialize(bytes, owner)
    }
    fn release(&self, s: ShapeId) {
        self.inner.release(s)
    }
}
