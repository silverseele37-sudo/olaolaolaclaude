//! El documento de FORGE.
//!
//! # La distinción que hay que tener clara antes de leer nada
//!
//! **El árbol de historia paramétrico y la pila de undo son cosas distintas.**
//! Es el error de diseño más común en este tipo de aplicaciones y contamina todo
//! si se comete.
//!
//! - El **árbol de features** (Pilar 1) es *datos*: una descripción del modelo
//!   que el usuario edita, reordena y suprime. Vive en el documento y se guarda
//!   en el archivo.
//! - El **undo** es *meta*: deshace cambios al documento, **incluidos los
//!   cambios al árbol de features**. No se guarda en el archivo.
//!
//! Deshacer "añadir un fillet" no es lo mismo que suprimir el nodo fillet: lo
//! primero devuelve el documento a un estado anterior, lo segundo *es* una
//! edición que a su vez es deshacible.
//!
//! # Por qué el undo es unificado sin coordinar nada
//!
//! Ningún pilar tiene pila propia. Un pilar no puede deshacer: solo produce
//! comandos que el documento aplica. Editar un vértice, cambiar una cota,
//! recablear un nodo de material y renombrar una etiqueta de activo son, a nivel
//! de documento, la misma operación: producir una versión nueva. Un `Ctrl+Z`
//! después de una operación mixta las revierte todas juntas porque no hay nada
//! que coordinar (ADR-0004).

use std::sync::Arc;

use forge_store::BlobHash;

pub mod component;
pub mod id;

use component::{AnyStore, Component, Map, TypedStore};
pub use component::{Domain, Geometry, GeometryPayload, Name, Parent, Transform, Visible};
pub use id::{Binding, EntityId, FeatureId, StableId, TopoClass, VersionId};

/// Registro compartido de componentes. Los pilares lo reciben, no lo construyen.
pub use component::ComponentRegistry;
pub type ComponentRegistryRef = Arc<ComponentRegistry>;

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("componente desconocido en el documento: {0}. Registrarlo antes de cargar, o el guardado siguiente perdería sus datos.")]
    UnknownComponent(String),
    #[error("no se pudo codificar el documento: {0}")]
    Encode(String),
    #[error("no se pudo decodificar el documento: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, DocError>;

// ---------------------------------------------------------------------------
// Datos
// ---------------------------------------------------------------------------

/// El estado del documento en un instante. Inmutable y con compartición
/// estructural: clonarlo es copiar dos punteros.
#[derive(Clone)]
struct DocData {
    entities: Map<EntityId, ()>,
    stores: Map<&'static str, Arc<dyn AnyStore>>,
}

impl DocData {
    fn new() -> Self {
        DocData {
            entities: Map::new_sync(),
            stores: Map::new_sync(),
        }
    }
}

struct Version {
    id: VersionId,
    label: String,
    data: DocData,
}

/// Qué pasó en el documento. Los pilares se enteran por aquí, nunca leyendo el
/// estado de otro pilar.
#[derive(Clone, Debug, PartialEq)]
pub enum DocEvent {
    Committed { version: VersionId, label: String },
    Undone { to: VersionId, undid: String },
    Redone { to: VersionId, redid: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SubId(u64);

type Subscriber = Box<dyn Fn(&DocEvent) + Send + Sync>;

// ---------------------------------------------------------------------------
// Documento
// ---------------------------------------------------------------------------

pub struct Document {
    registry: Arc<ComponentRegistry>,
    versions: Vec<Version>,
    /// Índice de la versión vigente. Deshacer es moverlo, nada más.
    cursor: usize,
    next_version: u64,
    subs: Vec<(SubId, Subscriber)>,
    next_sub: u64,
    /// Techo del historial en memoria. Al superarlo se podan las versiones más
    /// antiguas; el `cursor` se corrige para seguir apuntando al mismo estado.
    pub history_limit: usize,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = &self.versions[self.cursor];
        f.debug_struct("Document")
            .field("version", &v.id)
            .field("label", &v.label)
            .field("entities", &v.data.entities.size())
            .field("component_stores", &v.data.stores.size())
            .field("undo_depth", &self.cursor)
            .field("redo_depth", &(self.versions.len() - 1 - self.cursor))
            .finish()
    }
}

impl Document {
    pub fn new() -> Self {
        Document::with_registry(Arc::new(ComponentRegistry::new()))
    }

    pub fn with_registry(registry: Arc<ComponentRegistry>) -> Self {
        Document {
            registry,
            versions: vec![Version {
                id: VersionId(0),
                label: "documento nuevo".into(),
                data: DocData::new(),
            }],
            cursor: 0,
            next_version: 1,
            subs: Vec::new(),
            next_sub: 0,
            history_limit: 512,
        }
    }

    pub fn registry(&self) -> &Arc<ComponentRegistry> {
        &self.registry
    }

    /// Vista inmutable del estado actual. Es lo único que ven los pilares.
    pub fn snapshot(&self) -> Snapshot {
        let v = &self.versions[self.cursor];
        Snapshot {
            data: v.data.clone(),
            version: v.id,
        }
    }

    pub fn version(&self) -> VersionId {
        self.versions[self.cursor].id
    }

    /// Abre una transacción. Toda mutación pasa por aquí (invariante I4).
    pub fn begin(&mut self) -> Transaction<'_> {
        let data = self.versions[self.cursor].data.clone();
        Transaction {
            doc: self,
            data,
            done: false,
        }
    }

    /// Atajo para una edición de una sola llamada.
    pub fn edit<R>(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut Transaction<'_>) -> R,
    ) -> R {
        let mut tx = self.begin();
        let r = f(&mut tx);
        tx.commit(label);
        r
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.versions.len()
    }

    pub fn undo(&mut self) -> Option<VersionId> {
        if !self.can_undo() {
            return None;
        }
        let undid = self.versions[self.cursor].label.clone();
        self.cursor -= 1;
        let to = self.versions[self.cursor].id;
        self.emit(DocEvent::Undone { to, undid });
        Some(to)
    }

    pub fn redo(&mut self) -> Option<VersionId> {
        if !self.can_redo() {
            return None;
        }
        self.cursor += 1;
        let v = &self.versions[self.cursor];
        let (to, redid) = (v.id, v.label.clone());
        self.emit(DocEvent::Redone { to, redid });
        Some(to)
    }

    /// Etiquetas del historial vigente, de la más antigua a la actual.
    pub fn history(&self) -> Vec<(VersionId, &str)> {
        self.versions[..=self.cursor]
            .iter()
            .map(|v| (v.id, v.label.as_str()))
            .collect()
    }

    pub fn subscribe(&mut self, f: impl Fn(&DocEvent) + Send + Sync + 'static) -> SubId {
        let id = SubId(self.next_sub);
        self.next_sub += 1;
        self.subs.push((id, Box::new(f)));
        id
    }

    pub fn unsubscribe(&mut self, id: SubId) {
        self.subs.retain(|(i, _)| *i != id);
    }

    fn emit(&self, ev: DocEvent) {
        for (_, f) in &self.subs {
            f(&ev);
        }
    }

    /// Reconstruye un documento a partir de lo leído de un archivo.
    /// La usa `forge-io`; no hay otra vía de entrada que salte las
    /// transacciones.
    pub fn from_parts(
        registry: Arc<ComponentRegistry>,
        entities: Vec<EntityId>,
        stores: Vec<(String, Vec<u8>)>,
        label: impl Into<String>,
    ) -> Result<Self> {
        let mut data = DocData::new();
        for e in entities {
            data.entities.insert_mut(e, ());
        }
        for (name, bytes) in stores {
            let st = registry.decode(&name, &bytes)?;
            data.stores.insert_mut(st.name(), st);
        }
        let mut doc = Document::with_registry(registry);
        doc.versions[0] = Version {
            id: VersionId(0),
            label: label.into(),
            data,
        };
        Ok(doc)
    }
}

// ---------------------------------------------------------------------------
// Transacción
// ---------------------------------------------------------------------------

/// Una transacción es **una** entrada de undo, cruce los pilares que cruce.
///
/// Si no se hace `commit`, no pasó nada: el `Drop` descarta los cambios. Es
/// deliberado que olvidarse de confirmar sea inocuo y olvidarse de deshacer sea
/// imposible.
pub struct Transaction<'d> {
    doc: &'d mut Document,
    data: DocData,
    done: bool,
}

impl<'d> Transaction<'d> {
    pub fn spawn(&mut self) -> EntityId {
        let e = EntityId::new();
        self.data.entities.insert_mut(e, ());
        e
    }

    /// Para documentos generados y tests: entidad con id determinista.
    pub fn spawn_with_id(&mut self, e: EntityId) -> EntityId {
        self.data.entities.insert_mut(e, ());
        e
    }

    pub fn despawn(&mut self, e: EntityId) {
        self.data.entities.remove_mut(&e);
        let names: Vec<&'static str> = self
            .data
            .stores
            .iter()
            .filter(|(_, s)| s.contains(e))
            .map(|(n, _)| *n)
            .collect();
        for n in names {
            let s = self.data.stores.get(n).unwrap().without(e);
            if s.len() == 0 {
                self.data.stores.remove_mut(&n);
            } else {
                self.data.stores.insert_mut(n, s);
            }
        }
    }

    pub fn set<C: Component>(&mut self, e: EntityId, c: C) {
        self.data.entities.insert_mut(e, ());
        let map = match self.data.stores.get(&C::NAME) {
            Some(s) => {
                let t: &TypedStore<C> = s
                    .as_any()
                    .downcast_ref()
                    .expect("dos componentes distintos comparten NAME");
                t.map.insert(e, c)
            }
            None => TypedStore::<C>::empty().map.insert(e, c),
        };
        self.data
            .stores
            .insert_mut(C::NAME, Arc::new(TypedStore { map }));
    }

    pub fn remove<C: Component>(&mut self, e: EntityId) {
        if let Some(s) = self.data.stores.get(&C::NAME) {
            let s = s.without(e);
            if s.len() == 0 {
                self.data.stores.remove_mut(&C::NAME);
            } else {
                self.data.stores.insert_mut(C::NAME, s);
            }
        }
    }

    pub fn get<C: Component>(&self, e: EntityId) -> Option<&C> {
        let s = self.data.stores.get(&C::NAME)?;
        s.as_any().downcast_ref::<TypedStore<C>>()?.map.get(&e)
    }

    pub fn contains(&self, e: EntityId) -> bool {
        self.data.entities.contains_key(&e)
    }

    pub fn entity_count(&self) -> usize {
        self.data.entities.size()
    }

    /// Confirma. Todo lo que quede por delante en el historial se descarta:
    /// editar después de deshacer abre una rama nueva y la vieja se pierde, que
    /// es lo que espera cualquiera que haya usado un editor.
    pub fn commit(mut self, label: impl Into<String>) -> VersionId {
        self.done = true;
        let data = std::mem::replace(&mut self.data, DocData::new());
        let doc = &mut *self.doc;

        doc.versions.truncate(doc.cursor + 1);
        let id = VersionId(doc.next_version);
        doc.next_version += 1;
        let label = label.into();
        doc.versions.push(Version {
            id,
            label: label.clone(),
            data,
        });
        doc.cursor = doc.versions.len() - 1;

        if doc.versions.len() > doc.history_limit {
            let podar = doc.versions.len() - doc.history_limit;
            doc.versions.drain(0..podar);
            doc.cursor = doc.cursor.saturating_sub(podar);
        }

        doc.emit(DocEvent::Committed { version: id, label });
        id
    }

    pub fn rollback(mut self) {
        self.done = true;
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // Sin commit no hay versión nueva. Nada que limpiar: la copia de trabajo
        // se va con la transacción y la compartición estructural hace el resto.
        let _ = self.done;
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Vista de solo lectura del documento en una versión.
///
/// Los pilares reciben esto, nunca una referencia mutable. De ahí sale que la
/// evaluación pueda correr en paralelo y que el render lea sin bloquear al hilo
/// de documento: no hay estado mutable compartido que proteger.
#[derive(Clone)]
pub struct Snapshot {
    data: DocData,
    version: VersionId,
}

impl Snapshot {
    pub fn version(&self) -> VersionId {
        self.version
    }

    pub fn get<C: Component>(&self, e: EntityId) -> Option<&C> {
        let s = self.data.stores.get(&C::NAME)?;
        s.as_any().downcast_ref::<TypedStore<C>>()?.map.get(&e)
    }

    /// Itera las entidades que tienen `C`, ordenadas por id.
    pub fn iter<C: Component>(&self) -> impl Iterator<Item = (EntityId, &C)> + '_ {
        self.data
            .stores
            .get(&C::NAME)
            .and_then(|s| s.as_any().downcast_ref::<TypedStore<C>>())
            .into_iter()
            .flat_map(|t| t.map.iter().map(|(e, c)| (*e, c)))
    }

    pub fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.data.entities.keys().copied()
    }

    pub fn entity_count(&self) -> usize {
        self.data.entities.size()
    }

    pub fn contains(&self, e: EntityId) -> bool {
        self.data.entities.contains_key(&e)
    }

    /// Transformada acumulada hasta la raíz.
    ///
    /// Corta ante un ciclo en lugar de colgarse: un `Parent` mal formado —de un
    /// archivo corrupto o de un plugin— no debe congelar la aplicación.
    pub fn world_transform(&self, e: EntityId) -> forge_math::DAffine3 {
        let mut cadena = Vec::new();
        let mut actual = Some(e);
        let mut guarda = 0;
        while let Some(id) = actual {
            if guarda > 4096 {
                break;
            }
            guarda += 1;
            cadena.push(
                self.get::<Transform>(id)
                    .copied()
                    .unwrap_or(Transform::IDENTITY),
            );
            actual = self.get::<Parent>(id).map(|p| p.0);
            if actual == Some(e) {
                break; // ciclo
            }
        }
        cadena
            .iter()
            .rev()
            .fold(forge_math::DAffine3::IDENTITY, |acc, t| acc * t.to_affine())
    }

    /// Blobs que el documento referencia, sin repetidos y ordenados.
    /// Es lo que hay que empaquetar al guardar y lo que sobrevive a la
    /// recolección.
    pub fn referenced_blobs(&self) -> Vec<BlobHash> {
        let mut out = Vec::new();
        for (_, s) in self.data.stores.iter() {
            s.collect_blobs(&mut out);
        }
        out.sort();
        out.dedup();
        out
    }

    // --- superficie para `forge-io` ---

    pub fn store_names(&self) -> Vec<&'static str> {
        self.data.stores.keys().copied().collect()
    }

    pub fn encode_store(&self, name: &str) -> Option<Result<Vec<u8>>> {
        self.data.stores.get(name).map(|s| s.encode())
    }

    pub fn entity_ids(&self) -> Vec<EntityId> {
        self.data.entities.keys().copied().collect()
    }

    /// Huella canónica del documento.
    ///
    /// Dos snapshots con la misma huella tienen el mismo contenido. Se apoya en
    /// que los mapas iteran ordenados, así que no hay que ordenar nada aquí. La
    /// usan el test de propiedad del undo, la detección de cambios y el
    /// "¿hay algo sin guardar?".
    pub fn fingerprint(&self) -> BlobHash {
        let mut buf = Vec::new();
        for e in self.data.entities.keys() {
            buf.extend_from_slice(e.0 .0.to_le_bytes().as_slice());
        }
        for (name, s) in self.data.stores.iter() {
            buf.push(0xff);
            buf.extend_from_slice(name.as_bytes());
            buf.push(0x00);
            if let Ok(b) = s.encode() {
                buf.extend_from_slice(&b);
            }
        }
        BlobHash::of(&buf)
    }
}
