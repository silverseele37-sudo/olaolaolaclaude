//! Contrato del kernel geométrico.
//!
//! **Este crate no tiene lógica.** Es la frontera con OpenCASCADE —o con lo que
//! la reemplace— y está diseñada para que el kernel pueda ejecutarse fuera de
//! proceso sin que ningún llamante se entere.
//!
//! Tres reglas heredadas de ADR-0001 y validadas empíricamente en `cadviz`:
//!
//! 1. **Por la frontera cruzan triángulos, polilíneas e identificadores. Nada
//!    más.** Ningún tipo del kernel aparece del lado de FORGE.
//! 2. **Toda operación es serializable**: entran parámetros y handles, salen
//!    handles y datos propios. Nada de punteros a objetos del kernel.
//! 3. **Los errores son datos, no pánicos.** Las excepciones de C++ se capturan
//!    en el puente y se traducen a [`KernelError`].
//!
//! Que existan dos implementaciones intercambiables de este contrato —una sobre
//! OpenCASCADE y otra procedural— no es un lujo: es lo que permite desarrollar y
//! testear los demás pilares sin C++ en el grafo de build, y la prueba de que la
//! costura es real y no decorativa.

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_math::{Aabb, DVec3};
use serde::{Deserialize, Serialize};

pub mod sketch;
pub use sketch::{Constraint, DimId, SketchModel, SketchSolver, SolveResult, SolveStatus};

/// Handle opaco a una forma que vive **dentro** del kernel.
///
/// Deliberadamente un entero y no un puntero: es lo que hace que el kernel
/// pueda mudarse a otro proceso sin tocar a los llamantes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct ShapeId(pub u64);

impl std::fmt::Display for ShapeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "shape#{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum KernelError {
    #[error("entrada invalida: {detail}")]
    InvalidInput { detail: String },
    #[error("la operacion `{op}` fallo: {detail}")]
    OperationFailed { op: &'static str, detail: String },
    #[error("geometria degenerada: {hint}")]
    Degenerate { hint: String },
    #[error("tolerancia excedida: hacian falta {required}, se logro {achieved}")]
    ToleranceExceeded { required: f64, achieved: f64 },
    #[error("referencia no resuelta: {0:?}")]
    UnresolvedReference(StableId),
    #[error("handle desconocido: {0}")]
    UnknownShape(ShapeId),
    #[error("la operacion excedio el tiempo permitido")]
    Timeout,
    /// El kernel abortó. Aislable: con el kernel fuera de proceso esto es
    /// recuperable en vez de fatal.
    #[error("el kernel aborto: {backtrace}")]
    KernelPanic { backtrace: String },
    #[error("no implementado en esta implementacion del kernel: {0}")]
    Unsupported(&'static str),
}

pub type KernelResult<T> = std::result::Result<T, KernelError>;

// ---------------------------------------------------------------------------
// Teselado: la única salida del dominio exacto
// ---------------------------------------------------------------------------

/// Parámetros de teselado.
///
/// No son propiedades del modelo sino **de una vista**: por eso el helper
/// [`TessellationParams::for_view`], que implementa la regla R1b de ADR-0002.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TessellationParams {
    /// Desviación máxima de la superficie, en milímetros.
    pub chord_mm: f64,
    /// Desviación angular máxima, en grados.
    pub angular_deg: f64,
}

impl Default for TessellationParams {
    fn default() -> Self {
        TessellationParams {
            chord_mm: 0.05,
            angular_deg: 15.0,
        }
    }
}

impl TessellationParams {
    /// Deflexión derivada del tamaño de un píxel en el mundo (ADR-0002 R1b).
    pub fn for_view(distance_mm: f64, fov_y_rad: f64, height_px: f64, px_error: f64) -> Self {
        TessellationParams {
            chord_mm: forge_math::chord_deflection(distance_mm, fov_y_rad, height_px, px_error),
            angular_deg: 15.0,
        }
    }

    /// Clave de caché. El teselado es un artefacto derivado indexado por
    /// `hash(forma) + parametros` (ADR-0002 R1), así que los parámetros tienen
    /// que hashearse de forma estable pese a ser `f64`.
    pub fn cache_key(&self) -> u64 {
        let a = self.chord_mm.to_bits();
        let b = self.angular_deg.to_bits();
        a.rotate_left(17) ^ b
    }
}

/// Cómo nació una entidad topológica. Es la semántica que hace que un
/// [`StableId`] sea estable: la genera el nodo que crea la geometría, no el
/// kernel (ADR-0002, §4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TopoProvenance {
    /// Cara lateral de una extrusión, generada por una arista del perfil.
    SweptFromProfileEdge { edge_index: u32 },
    /// Tapa de una extrusión o revolución.
    Cap { start: bool },
    /// Cara de redondeo generada al filetear una arista referenciada.
    Blend { of: StableId },
    /// Cara heredada sin cambios de la forma de entrada.
    Inherited { from: StableId },
    /// Trozo de una cara partida por un booleano.
    SplitFrom { original: StableId, piece: u32 },
    /// Creada directamente por una primitiva.
    Primitive { index: u32 },
}

/// Clasificación de una arista para dibujado.
///
/// Un visor CAD **no** dibuja todas las aristas topológicas: las tangentes y las
/// costuras taparían la pieza de líneas que ningún plano lleva. Tomado de
/// `cadviz`, donde está medido (`linkrods.step`: 93 tangentes de 108 aristas).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Quiebre real entre dos caras. Se dibuja.
    Sharp,
    /// Borde libre, una sola cara. Se dibuja.
    Boundary,
    /// Unión tangente (un redondeo con su cara). No se dibuja.
    Smooth,
    /// Costura de parametrización de un cilindro. Nunca se dibuja.
    Seam,
    /// Polo de esfera, ápice de cono. Nunca se dibuja.
    Degenerate,
}

impl EdgeKind {
    pub fn se_dibuja(self) -> bool {
        matches!(self, EdgeKind::Sharp | EdgeKind::Boundary)
    }
}

/// Resultado del teselado.
///
/// **Nunca es editable por el usuario** (invariante I1). Es caché derivada,
/// indexada por `hash(forma) + parametros`.
///
/// El mapa de procedencia es lo que convierte esto de una sopa de triángulos en
/// la frontera de dominio de ADR-0002: sin él, «selecciona esta cara del sólido
/// y biséllala» no puede funcionar, y una selección no sobrevive a una edición
/// aguas arriba.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tessellation {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    /// Triples. `indices.len() % 3 == 0`.
    pub indices: Vec<u32>,
    /// Un `StableId` por triángulo: `face_of_triangle.len() == indices.len() / 3`.
    pub face_of_triangle: Vec<StableId>,
    /// Polilíneas de las aristas, sacadas de la **curva analítica** del kernel,
    /// no de las aristas de la malla: un contorno sacado de la malla hereda su
    /// facetado; uno sacado de la curva es liso a cualquier zoom.
    pub edges: Vec<EdgePolyline>,
    pub bbox: Aabb,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgePolyline {
    pub id: StableId,
    pub kind: EdgeKind,
    pub points: Vec<DVec3>,
}

impl Tessellation {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// El `StableId` de la cara que originó el triángulo `i`.
    pub fn face_of(&self, triangle: usize) -> Option<StableId> {
        self.face_of_triangle.get(triangle).copied()
    }

    /// Comprueba las invariantes estructurales. Barato y vale la pena llamarlo
    /// en los tests de cualquier implementación del kernel.
    pub fn validate(&self) -> KernelResult<()> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(KernelError::OperationFailed {
                op: "tessellate",
                detail: format!("indices no multiplo de 3: {}", self.indices.len()),
            });
        }
        if self.face_of_triangle.len() != self.triangle_count() {
            return Err(KernelError::OperationFailed {
                op: "tessellate",
                detail: format!(
                    "procedencia incompleta: {} triangulos, {} ids",
                    self.triangle_count(),
                    self.face_of_triangle.len()
                ),
            });
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(KernelError::OperationFailed {
                op: "tessellate",
                detail: "normales y posiciones descuadradas".into(),
            });
        }
        let n = self.positions.len() as u32;
        if let Some(&mal) = self.indices.iter().find(|&&i| i >= n) {
            return Err(KernelError::OperationFailed {
                op: "tessellate",
                detail: format!("indice {mal} fuera de rango ({n} vertices)"),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Descripción topológica
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TopologySummary {
    pub faces: Vec<TopoEntity>,
    pub edges: Vec<TopoEntity>,
    pub vertices: Vec<TopoEntity>,
    pub is_solid: bool,
    pub is_closed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopoEntity {
    pub id: StableId,
    pub provenance: TopoProvenance,
    /// Firma geométrica para re-vincular tras un cambio de topología.
    pub signature: GeometrySignature,
}

/// Firma geométrica cuantizada.
///
/// Es el **respaldo** del nombrado por genealogía, no su sustituto: solo se usa
/// cuando un [`StableId`] no resuelve. Y si tampoco resuelve por firma, la
/// referencia pasa a `Broken` y se muestra — nunca se re-vincula en silencio a
/// la candidata más parecida (ADR-0002, R3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GeometrySignature {
    pub centroid_q: [i64; 3],
    pub normal_q: [i64; 3],
    pub measure_q: i64,
    pub class: TopoClass,
}

impl GeometrySignature {
    /// Cuantización a tolerancia gruesa: la firma tiene que sobrevivir a que la
    /// geometría se mueva un poco, que es justo lo que pasa al editar una cota
    /// aguas arriba.
    pub const QUANTUM_MM: f64 = 1e-3;

    pub fn new(centroid: DVec3, normal: DVec3, measure: f64, class: TopoClass) -> Self {
        let q = |v: f64| (v / Self::QUANTUM_MM).round() as i64;
        GeometrySignature {
            centroid_q: [q(centroid.x), q(centroid.y), q(centroid.z)],
            normal_q: [
                (normal.x * 1000.0).round() as i64,
                (normal.y * 1000.0).round() as i64,
                (normal.z * 1000.0).round() as i64,
            ],
            measure_q: q(measure),
            class,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MassProperties {
    pub volume_mm3: f64,
    pub area_mm2: f64,
    pub centroid: DVec3,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub problems: Vec<String>,
}

// ---------------------------------------------------------------------------
// Operaciones
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ExtrudeOpts {
    pub direction: DVec3,
    pub distance_mm: f64,
    /// Extruye a ambos lados del perfil.
    pub symmetric: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RevolveOpts {
    pub axis_origin: DVec3,
    pub axis_dir: DVec3,
    pub angle_rad: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FilletSpec {
    Constant { radius_mm: f64 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ChamferSpec {
    Symmetric { distance_mm: f64 },
}

/// El contrato del kernel.
///
/// Ninguna función devuelve referencias al interior del kernel: todo sale
/// copiado o por handle. Es más verboso, y es lo que permite mover el kernel a
/// otro proceso, cachear por hash y aislar sus caídas.
pub trait GeometryKernel: Send + Sync {
    fn name(&self) -> &'static str;

    // --- construcción ---
    /// Perfil cerrado en el plano Z=0, en orden antihorario.
    fn profile_from_polygon(
        &self,
        pts: &[forge_math::DVec2],
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn extrude(
        &self,
        profile: ShapeId,
        opts: ExtrudeOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn revolve(
        &self,
        profile: ShapeId,
        opts: RevolveOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn box_solid(&self, min: DVec3, max: DVec3, owner: FeatureId) -> KernelResult<ShapeId>;
    fn cylinder(
        &self,
        base: DVec3,
        axis: DVec3,
        radius_mm: f64,
        height_mm: f64,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;

    // --- modificación ---
    fn fillet(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: FilletSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn chamfer(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: ChamferSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn boolean(
        &self,
        op: BoolOp,
        a: ShapeId,
        b: ShapeId,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;
    fn transform(
        &self,
        s: ShapeId,
        m: &forge_math::DAffine3,
        owner: FeatureId,
    ) -> KernelResult<ShapeId>;

    // --- consulta ---
    fn topology(&self, s: ShapeId) -> KernelResult<TopologySummary>;
    fn mass_properties(&self, s: ShapeId) -> KernelResult<MassProperties>;
    fn bbox(&self, s: ShapeId) -> KernelResult<Aabb>;
    fn is_valid(&self, s: ShapeId) -> KernelResult<ValidationReport>;

    // --- cruce de dominio: la unica salida ---
    fn tessellate(&self, s: ShapeId, p: &TessellationParams) -> KernelResult<Tessellation>;

    // --- persistencia ---
    fn serialize(&self, s: ShapeId) -> KernelResult<Vec<u8>>;
    fn deserialize(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<ShapeId>;

    // --- intercambio ---
    fn import_step(&self, _bytes: &[u8], _owner: FeatureId) -> KernelResult<Vec<ShapeId>> {
        Err(KernelError::Unsupported("import_step"))
    }
    fn export_step(&self, _shapes: &[ShapeId]) -> KernelResult<Vec<u8>> {
        Err(KernelError::Unsupported("export_step"))
    }

    /// Libera una forma. El kernel es dueño de su memoria.
    fn release(&self, s: ShapeId);
}
