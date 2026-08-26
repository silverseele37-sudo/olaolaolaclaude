//! Componentes: los datos que cuelgan de una entidad.
//!
//! El almacén por componente es un mapa **persistente ordenado**. Persistente
//! porque una edición que toca 3 entidades de 100 000 comparte el resto de la
//! estructura con la versión anterior, y eso es lo que hace barato el undo
//! (ADR-0004). Ordenado —árbol rojinegro en vez de HAMT— porque la iteración
//! determinista sale gratis, y de ella dependen la serialización reproducible y
//! la huella del documento. Con miles de entidades, el `O(log n)` frente al
//! `O(1)` amortizado del HAMT no se mide.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use forge_store::BlobHash;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::id::EntityId;
use crate::{DocError, Result};

pub(crate) type Map<K, V> = rpds::RedBlackTreeMapSync<K, V>;

/// Un dato adjunto a una entidad.
///
/// `NAME` es la clave del componente en el archivo y **es parte del formato**:
/// cambiarlo rompe documentos existentes y exige una migración.
pub trait Component: Clone + Send + Sync + 'static + Serialize + DeserializeOwned {
    const NAME: &'static str;

    /// Blobs que este componente referencia.
    ///
    /// Obligatorio para cualquier componente que apunte al almacén. De esto
    /// dependen tres cosas sin código adicional: empaquetar un `.forge` con
    /// exactamente los blobs que usa, recolectar los que ya no referencia nadie,
    /// y responder "qué documentos usan esta textura" en el Pilar 4.
    fn blob_refs(&self, _out: &mut Vec<BlobHash>) {}
}

/// Vista sin tipo de un almacén de componentes. Interna: el mundo exterior
/// habla en `Component`, no en `dyn Any`.
pub(crate) trait AnyStore: Send + Sync {
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn len(&self) -> usize;
    fn contains(&self, e: EntityId) -> bool;
    fn without(&self, e: EntityId) -> Arc<dyn AnyStore>;
    fn encode(&self) -> Result<Vec<u8>>;
    fn collect_blobs(&self, out: &mut Vec<BlobHash>);
}

pub(crate) struct TypedStore<C: Component> {
    pub(crate) map: Map<EntityId, C>,
}

impl<C: Component> TypedStore<C> {
    pub(crate) fn empty() -> Self {
        TypedStore { map: Map::new_sync() }
    }
}

impl<C: Component> AnyStore for TypedStore<C> {
    fn name(&self) -> &'static str {
        C::NAME
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn len(&self) -> usize {
        self.map.size()
    }
    fn contains(&self, e: EntityId) -> bool {
        self.map.contains_key(&e)
    }
    fn without(&self, e: EntityId) -> Arc<dyn AnyStore> {
        Arc::new(TypedStore { map: self.map.remove(&e) })
    }
    fn encode(&self) -> Result<Vec<u8>> {
        // El mapa ya itera ordenado por EntityId, así que el CBOR es
        // reproducible byte a byte sin ordenar aquí.
        let entries: Vec<(&EntityId, &C)> = self.map.iter().collect();
        let mut out = Vec::new();
        ciborium::into_writer(&entries, &mut out).map_err(|e| DocError::Encode(e.to_string()))?;
        Ok(out)
    }
    fn collect_blobs(&self, out: &mut Vec<BlobHash>) {
        for (_, c) in self.map.iter() {
            c.blob_refs(out);
        }
    }
}

pub(crate) fn decode_store<C: Component>(bytes: &[u8]) -> Result<Arc<dyn AnyStore>> {
    let entries: Vec<(EntityId, C)> =
        ciborium::from_reader(bytes).map_err(|e| DocError::Decode(e.to_string()))?;
    let mut map = Map::new_sync();
    for (e, c) in entries {
        map.insert_mut(e, c);
    }
    Ok(Arc::new(TypedStore { map }))
}

/// Traduce el nombre de un componente del archivo a un almacén tipado.
///
/// Sin registro no hay carga: un documento que menciona un componente
/// desconocido es un error explícito, no un campo que se ignora en silencio.
/// Ignorarlo perdería datos del usuario al volver a guardar.
#[derive(Default)]
pub struct ComponentRegistry {
    decoders: BTreeMap<&'static str, fn(&[u8]) -> Result<Arc<dyn AnyStore>>>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        let mut r = ComponentRegistry::default();
        r.register::<Name>();
        r.register::<Transform>();
        r.register::<Visible>();
        r.register::<Parent>();
        r.register::<Geometry>();
        r
    }

    pub fn empty() -> Self {
        ComponentRegistry::default()
    }

    pub fn register<C: Component>(&mut self) -> &mut Self {
        self.decoders.insert(C::NAME, decode_store::<C>);
        self
    }

    pub fn knows(&self, name: &str) -> bool {
        self.decoders.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.decoders.keys().copied()
    }

    pub(crate) fn decode(&self, name: &str, bytes: &[u8]) -> Result<Arc<dyn AnyStore>> {
        let f = self
            .decoders
            .get(name)
            .ok_or_else(|| DocError::UnknownComponent(name.to_string()))?;
        f(bytes)
    }
}

// ---------------------------------------------------------------------------
// Componentes básicos de escena
// ---------------------------------------------------------------------------

/// Nombre legible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Name(pub String);

impl Component for Name {
    const NAME: &'static str = "forge.name";
}

pub use forge_math::Transform;

impl Component for Transform {
    const NAME: &'static str = "forge.transform";
}

/// Visibilidad en el viewport. Separada de la existencia: ocultar no es borrar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visible(pub bool);

impl Component for Visible {
    const NAME: &'static str = "forge.visible";
}

/// Padre en el grafo de escena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent(pub EntityId);

impl Component for Parent {
    const NAME: &'static str = "forge.parent";
}

/// Dominio de una geometría. El tipo lo hace explícito: no es un detalle de
/// implementación sino la frontera central del sistema (ADR-0002).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Domain {
    /// B-Rep, NURBS, sketches. Cotas exactas.
    Exact,
    /// Mallas, nubes de puntos. Sin tolerancia.
    Discrete,
}

/// Qué geometría lleva una entidad.
///
/// El payload es siempre un hash: la geometría pesada vive en el almacén de
/// blobs, nunca en el árbol del documento. Es lo que permite que dos versiones
/// que comparten una malla compartan el blob sin copiar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
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
    pub fn domain(self) -> Domain {
        match self {
            GeometryPayload::Sketch(_) | GeometryPayload::Curve(_) | GeometryPayload::Brep(_) => {
                Domain::Exact
            }
            GeometryPayload::Mesh(_) | GeometryPayload::PointCloud(_) => Domain::Discrete,
        }
    }

    pub fn blob(self) -> BlobHash {
        match self {
            GeometryPayload::Sketch(h)
            | GeometryPayload::Curve(h)
            | GeometryPayload::Brep(h)
            | GeometryPayload::Mesh(h)
            | GeometryPayload::PointCloud(h) => h,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            GeometryPayload::Sketch(_) => "sketch",
            GeometryPayload::Curve(_) => "curve",
            GeometryPayload::Brep(_) => "brep",
            GeometryPayload::Mesh(_) => "mesh",
            GeometryPayload::PointCloud(_) => "pointcloud",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Geometry(pub GeometryPayload);

impl Component for Geometry {
    const NAME: &'static str = "forge.geometry";
    fn blob_refs(&self, out: &mut Vec<BlobHash>) {
        out.push(self.0.blob());
    }
}
