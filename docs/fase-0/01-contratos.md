# Contratos entre módulos

Estas interfaces son la parte del proyecto que más cuesta cambiar después de la
Fase 2. Se diseñan ahora, con cuidado, y las funcionalidades vistosas se acomodan
a ellas — no al revés.

El código es ilustrativo (Fase 0 no compila nada), pero las firmas están pensadas
para sobrevivir tal cual a la implementación. Donde una firma tenga una decisión
difícil escondida, está anotada.

---

## 1. Identidad y referencias

```rust
/// Identidad de una entidad de la escena. ULID: ordenable por tiempo,
/// generable sin coordinación, estable entre sesiones y entre máquinas.
pub struct EntityId(Ulid);

/// Identidad de un nodo del árbol de features.
pub struct FeatureId(Ulid);

/// Referencia a una sub-entidad topológica (una cara, una arista, un vértice).
/// NO es un índice: los índices del kernel cambian con cualquier recálculo.
pub struct StableId {
    pub origin: FeatureId,   // qué nodo del árbol creó esta entidad
    pub class:  TopoClass,   // Face | Edge | Vertex
    pub mark:   u64,         // discriminador semántico, generado por el nodo
}

/// Estado de resolución de una referencia tras un recálculo.
/// `Broken` es un estado de primera clase: se muestra en la UI y no se
/// re-vincula nunca en silencio a la candidata más parecida.
pub enum Binding<T> {
    Bound(T),
    Rebound { value: T, confidence: f32 },  // resuelto por firma geométrica
    Broken  { last_known: GeometrySignature },
}
```

> **Decisión escondida.** `mark` la genera el nodo que crea la geometría con su
> propia semántica ("cara lateral generada por la arista E3 del sketch"), no el
> kernel. Es lo que separa un naming que aguanta de uno que no. Ver
> [ADR-0002 §4](adr/0002-representacion-dual.md#4-el-mapa-de-procedencia-y-el-naming-persistente).

---

## 2. Payload geométrico y la frontera de dominio

```rust
/// Lo que una entidad puede contener como geometría.
/// El dominio es explícito en el tipo — no es un detalle de implementación.
pub enum GeometryPayload {
    // --- dominio exacto ---
    Sketch(BlobHash),
    Curve(BlobHash),
    Brep(BlobHash),

    // --- dominio discreto ---
    Mesh(BlobHash),
    PointCloud(BlobHash),
}

impl GeometryPayload {
    pub fn domain(&self) -> Domain { /* Exact | Discrete */ }
}

/// Resultado del teselado. NUNCA es editable por el usuario (invariante I1).
/// Se identifica por hash(brep, params) y vive en la caché, no en el documento.
pub struct Tessellation {
    pub positions: Vec<[f64; 3]>,
    pub normals:   Vec<[f32; 3]>,
    pub indices:   Vec<u32>,

    /// El mapa de procedencia: sin esto, la frontera de dominio no funciona.
    /// Es lo que permite "selecciona esta cara del sólido y biséllala".
    pub face_of_triangle: Vec<StableId>,       // len == indices.len() / 3
    pub edge_of_mesh_edge: HashMap<(u32, u32), StableId>,
    pub vertex_of_mesh_vertex: HashMap<u32, StableId>,
}

pub struct TessellationParams {
    pub chord_deviation_mm: f64,   // por defecto 0.05 o relativa al bbox
    pub angular_deviation_deg: f64, // por defecto 15.0
    pub relative: bool,
}
```

---

## 3. Kernel geométrico

**Este es el contrato más importante del proyecto.** Es la frontera con
OpenCASCADE, y está diseñado para poder ejecutarse fuera de proceso sin que ningún
llamante se entere: nada de punteros a objetos del kernel cruzan; entran parámetros
serializables, salen handles y blobs.

```rust
pub trait GeometryKernel: Send + Sync {
    // --- construcción ---
    fn extrude(&self, profile: &BrepHandle, dir: Vec3, dist: f64,
               opts: ExtrudeOpts) -> KernelResult<BrepHandle>;
    fn revolve(&self, profile: &BrepHandle, axis: Axis, angle: f64)
               -> KernelResult<BrepHandle>;
    fn sweep(&self, profile: &BrepHandle, path: &CurveHandle, opts: SweepOpts)
               -> KernelResult<BrepHandle>;
    fn loft(&self, sections: &[BrepHandle], opts: LoftOpts)
               -> KernelResult<BrepHandle>;

    // --- modificación ---
    fn fillet(&self, solid: &BrepHandle, edges: &[StableId], radius: FilletSpec)
               -> KernelResult<BrepHandle>;
    fn chamfer(&self, solid: &BrepHandle, edges: &[StableId], spec: ChamferSpec)
               -> KernelResult<BrepHandle>;
    fn shell(&self, solid: &BrepHandle, open_faces: &[StableId], thickness: f64)
               -> KernelResult<BrepHandle>;
    fn boolean(&self, op: BoolOp, a: &BrepHandle, b: &BrepHandle)
               -> KernelResult<BrepHandle>;

    // --- consulta ---
    fn topology(&self, s: &BrepHandle) -> KernelResult<TopologySummary>;
    fn mass_properties(&self, s: &BrepHandle) -> KernelResult<MassProperties>;
    fn is_valid(&self, s: &BrepHandle) -> KernelResult<ValidationReport>;

    // --- cruce de dominio: la única salida del dominio exacto ---
    fn tessellate(&self, s: &BrepHandle, p: &TessellationParams)
               -> KernelResult<Tessellation>;

    // --- persistencia e intercambio ---
    fn serialize(&self, s: &BrepHandle) -> KernelResult<Vec<u8>>;
    fn deserialize(&self, bytes: &[u8]) -> KernelResult<BrepHandle>;
    fn import_step(&self, bytes: &[u8]) -> KernelResult<Vec<BrepHandle>>;
    fn export_step(&self, solids: &[BrepHandle], opts: StepOpts)
               -> KernelResult<Vec<u8>>;
}

/// Los errores del kernel son datos, no pánicos. Las excepciones de C++ se
/// capturan en la frontera `cxx` y se traducen aquí.
pub enum KernelError {
    InvalidInput { detail: String },
    OperationFailed { op: &'static str, detail: String },
    Degenerate { hint: String },
    ToleranceExceeded { required: f64, achieved: f64 },
    Timeout,
    KernelPanic { backtrace: String },  // el kernel abortó: aislable
}
```

> **Nota de diseño.** No hay `fn faces(&self) -> Vec<&Face>` ni nada que devuelva
> referencias al interior del kernel. Todo sale copiado o por handle. Es más
> verboso y es lo que permite mover el kernel a otro proceso, cachear por hash y
> aislar sus caídas. Ver [ADR-0001](adr/0001-lenguaje-del-nucleo.md).

```rust
pub trait SketchSolver: Send + Sync {
    fn solve(&self, sketch: &SketchModel) -> SolveResult;
}

pub struct SolveResult {
    pub positions: Vec<Point2>,
    /// Diagnóstico honesto: el usuario necesita saber POR QUÉ no resuelve.
    pub status: SolveStatus,  // Ok | UnderConstrained(dof) |
                              // OverConstrained(conflicting) | NoConvergence
}
```

---

## 4. Documento, transacciones y undo

```rust
/// Snapshot inmutable. Los pilares reciben esto; nunca una referencia mutable.
pub struct Snapshot { root: Arc<DocNode>, version: VersionId }

impl Snapshot {
    pub fn entity(&self, id: EntityId) -> Option<EntityView<'_>>;
    pub fn feature_tree(&self, id: EntityId) -> Option<&FeatureTree>;
    pub fn query<C: Component>(&self) -> impl Iterator<Item = (EntityId, &C)>;
}

/// Toda mutación pasa por aquí (invariante I4).
/// Una transacción = una entrada de undo, cruce los pilares que cruce.
pub struct Transaction<'d> { /* ... */ }

impl<'d> Transaction<'d> {
    pub fn spawn(&mut self) -> EntityId;
    pub fn set<C: Component>(&mut self, e: EntityId, c: C);
    pub fn remove<C: Component>(&mut self, e: EntityId);
    pub fn commit(self, label: impl Into<String>) -> VersionId;
    pub fn rollback(self);
}

pub trait Document {
    fn snapshot(&self) -> Snapshot;
    fn begin(&mut self) -> Transaction<'_>;
    fn undo(&mut self) -> Option<VersionId>;
    fn redo(&mut self) -> Option<VersionId>;
    fn subscribe(&mut self, f: impl Fn(&DocEvent) + Send + 'static) -> SubId;
}
```

**Por qué el undo es unificado sin esfuerzo de coordinación:** ningún pilar tiene
pila propia. Un pilar solo produce comandos; el documento los aplica y publica una
versión. `Ctrl+Z` tras una operación que tocó una cota, una malla y una etiqueta de
asset las revierte juntas porque, a nivel de documento, fueron una sola cosa. Ver
[ADR-0004](adr/0004-undo-unificado.md).

---

## 5. Evaluación del grafo

```rust
/// Un nodo del árbol de features. La implementa forge-param;
/// forge-doc no sabe qué es un fillet.
pub trait FeatureNode: Send + Sync {
    fn kind(&self) -> &'static str;
    fn inputs(&self) -> Vec<FeatureId>;

    /// Debe ser una función pura de (params, inputs). De eso depende que el
    /// cacheado por hash sea correcto. Un nodo que lee reloj, aleatoriedad o
    /// estado global rompe la reproducibilidad de todo el documento.
    fn evaluate(&self, ctx: &EvalContext) -> EvalResult<FeatureOutput>;

    /// Hash de los parámetros propios. Junto con el hash de las entradas
    /// forma la clave de caché.
    fn params_hash(&self) -> Hash;
}

pub struct FeatureOutput {
    pub payload: GeometryPayload,
    pub domain: Domain,
    /// Los StableId que este nodo creó, con su semántica. Lo que hace que
    /// las referencias aguas abajo sobrevivan a una edición aguas arriba.
    pub created: Vec<(StableId, TopoProvenance)>,
}
```

**Invariante I2, verificado en `EvalContext`:** un nodo cuyo dominio de salida es
`Exact` no puede tener ninguna entrada de dominio `Discrete`. El único nodo que
cambia de dominio es `ToMesh`, y solo en el sentido Exact → Discrete.

```rust
/// La puerta de un solo sentido, como nodo normal del grafo.
pub struct ToMesh {
    pub source: FeatureId,
    pub params: TessellationParams,
}
```

---

## 6. Modificadores de malla

```rust
pub trait Modifier: Send + Sync {
    fn kind(&self) -> &'static str;
    fn params_hash(&self) -> Hash;

    fn apply(&self, input: &Mesh, ctx: &ModifierContext) -> ModifierResult<Mesh>;

    /// OBLIGATORIO. Cada modificador debe decir cómo se propaga la procedencia
    /// a la geometría que crea. Un bevel que subdivide una cara tiene que
    /// asignar procedencia a los triángulos nuevos.
    ///
    /// Un modificador que no lo implemente correctamente rompe la selección
    /// aguas abajo tras cualquier edición paramétrica aguas arriba. Es la
    /// deuda técnica más cara del pilar 2, así que está en el trait y no
    /// es opcional.
    fn remap_provenance(&self, input: &ProvenanceMap) -> ProvenanceMap;
}
```

---

## 7. Render

```rust
/// El render consume snapshots. No conoce features, sketches ni assets.
pub trait Renderer {
    fn render(&mut self, view: &SceneView, target: &RenderTarget) -> RenderStats;
}

/// Vista aplanada de lo que hay que dibujar. La produce una capa de extracción
/// que sí conoce el documento; el renderer, no. Es lo que permite que el
/// runtime headless comparta exactamente el mismo camino de render.
pub struct SceneView<'a> {
    pub camera: Camera,
    pub instances: &'a [DrawInstance],  // hash de malla + material + transformada
    pub lights: &'a [Light],
    pub environment: Option<&'a Ibl>,
}
```

`DrawInstance` referencia mallas y materiales **por hash**, no por puntero. Así el
render diffea contra el frame anterior y sube a GPU solo lo que cambió de verdad,
y el undo tras una operación pesada es instantáneo porque los recursos ya están en
caché bajo ese mismo hash.

---

## 8. Almacén de activos

```rust
/// Todo lo pesado del sistema pasa por aquí. Es la columna vertebral:
/// undo, versiones, caché de evaluación y dedup de assets son el mismo
/// mecanismo. Ver ADR-0003 y ADR-0004.
pub trait BlobStore: Send + Sync {
    fn put(&self, bytes: &[u8]) -> BlobHash;   // idempotente
    fn get(&self, h: BlobHash) -> Option<Arc<[u8]>>;
    fn has(&self, h: BlobHash) -> bool;
}

pub trait AssetStore {
    fn import(&mut self, path: &Path, meta: AssetMeta) -> AssetId;
    fn search(&self, q: &AssetQuery) -> Vec<AssetId>;
    fn versions(&self, id: AssetId) -> Vec<AssetVersion>;
    fn revert(&mut self, id: AssetId, to: VersionId) -> Result<()>;
    fn dependents(&self, id: AssetId) -> Vec<AssetId>;
    fn thumbnail(&self, id: AssetId) -> Option<BlobHash>;
    /// El índice es caché, no fuente de verdad: se reconstruye desde los blobs.
    fn reindex(&mut self) -> ReindexReport;
}
```

---

## 9. Bus de comandos — la API pública

Es la superficie que usan la UI, los scripts Lua, el cliente Python y, en v2, los
plugins WASM. Si algo no se puede hacer por aquí, es un hueco de la API — y se ve
enseguida porque la propia UI lo sufre. Ver
[ADR-0006](adr/0006-plugins-y-scripting.md).

```rust
#[derive(Serialize, Deserialize)]  // serializable: eso es lo que permite
pub enum Command {                 // IPC, macros, replay y tests
    // pilar 1
    CreateSketch { plane: PlaneRef },
    AddConstraint { sketch: EntityId, c: Constraint },
    SetDimension { sketch: EntityId, dim: DimId, value: f64 },
    AddFeature { target: EntityId, feature: FeatureSpec },
    SuppressFeature { id: FeatureId, suppressed: bool },
    ReorderFeature { id: FeatureId, before: FeatureId },

    // frontera de dominio
    ConvertToMesh { entity: EntityId, params: TessellationParams },

    // pilar 2
    EditMesh { entity: EntityId, op: MeshOp },
    PushModifier { entity: EntityId, modifier: ModifierSpec },

    // pilar 3
    SetMaterial { entity: EntityId, material: MaterialId },
    EditMaterialGraph { material: MaterialId, edit: GraphEdit },

    // pilar 4
    ImportAsset { path: PathBuf, meta: AssetMeta },
    TagAsset { id: AssetId, tags: Vec<String> },

    // transversal
    Undo, Redo,
    BeginGroup { label: String }, EndGroup,
}
```

`BeginGroup`/`EndGroup` permiten que un script componga varias operaciones en una
sola entrada de undo — lo que hace que una macro se comporte como un comando
nativo desde el punto de vista del usuario.
