//! Identidades.
//!
//! ULID y no un entero autoincremental: ordenable por tiempo, generable sin
//! coordinación entre hilos ni entre máquinas, y estable entre sesiones. Un
//! índice de vector como identidad es lo que hace que fusionar dos documentos
//! sea imposible más adelante.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

/// Generador monótono de ULID.
///
/// `Ulid::new()` a secas **no** es monótono dentro del mismo milisegundo: los
/// bits bajos son aleatorios, así que tres entidades creadas seguidas pueden
/// quedar en cualquier orden. Como los almacenes de componentes iteran ordenados
/// por id, eso se ve directamente en el árbol de escena: el usuario crea A, B, C
/// y la lista muestra B, A, C.
///
/// El generador monótono hace que el orden de id **sea** el orden de creación,
/// que es lo que la documentación de estos tipos afirma.
static GEN: Mutex<Option<ulid::Generator>> = Mutex::new(None);

fn siguiente_ulid() -> ulid::Ulid {
    let mut g = match GEN.lock() {
        Ok(g) => g,
        // Un envenenamiento del mutex no debe impedir crear entidades: se
        // degrada al generador no monótono en vez de propagar el pánico.
        Err(e) => e.into_inner(),
    };
    let g = g.get_or_insert_with(ulid::Generator::new);
    // `generate` falla solo si se agotan los bits de aleatoriedad dentro del
    // mismo milisegundo (~2^80 ids). Ahí el no monótono es correcto igual.
    g.generate().unwrap_or_else(|_| ulid::Ulid::new())
}

macro_rules! id_type {
    ($name:ident, $tag:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub ulid::Ulid);

        impl $name {
            pub fn new() -> Self {
                $name(siguiente_ulid())
            }
            /// Determinista, para tests y para documentos generados.
            pub fn from_u128(v: u128) -> Self {
                $name(ulid::Ulid(v))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}:{}", $tag, self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self)
            }
        }
    };
}

id_type!(EntityId, "e", "Una entidad de la escena.");
id_type!(FeatureId, "f", "Un nodo del árbol de features (Pilar 1).");

/// Una versión del documento. Monótona dentro de una sesión.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct VersionId(pub u64);

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Clase de una entidad topológica.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum TopoClass {
    Face,
    Edge,
    Vertex,
}

/// Referencia estable a una sub-entidad topológica (ADR-0002, R3).
///
/// **No es un índice.** Los índices que devuelve el kernel cambian con cualquier
/// recálculo. `mark` lo genera el nodo que crea la geometría, con su propia
/// semántica ("cara lateral generada por la arista E3 del sketch"), y es lo que
/// permite que una selección sobreviva a una edición aguas arriba.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct StableId {
    pub origin: FeatureId,
    pub class: TopoClass,
    pub mark: u64,
}

/// Estado de resolución de una referencia tras un recálculo.
///
/// `Broken` es un estado de primera clase y se muestra en la interfaz. Nunca se
/// re-vincula en silencio a la candidata más parecida: un modelo plausible pero
/// incorrecto es peor que un error visible.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Binding<T> {
    Bound(T),
    /// Resuelto por firma geométrica tras un cambio de topología.
    Rebound { value: T, confidence: f32 },
    Broken,
}

impl<T: Copy> Binding<T> {
    pub fn value(&self) -> Option<T> {
        match self {
            Binding::Bound(v) => Some(*v),
            Binding::Rebound { value, .. } => Some(*value),
            Binding::Broken => None,
        }
    }
    pub fn is_broken(&self) -> bool {
        matches!(self, Binding::Broken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Respuesta conocida: el orden de id **es** el orden de creación.
    /// Sin generador monótono este test falla en cuanto se crean varias
    /// entidades dentro del mismo milisegundo, que es el caso normal.
    #[test]
    fn los_ids_salen_en_orden_de_creacion() {
        let ids: Vec<EntityId> = (0..10_000).map(|_| EntityId::new()).collect();
        for par in ids.windows(2) {
            assert!(par[0] < par[1], "{:?} no precede a {:?}", par[0], par[1]);
        }
        let mut ordenados = ids.clone();
        ordenados.sort();
        assert_eq!(ids, ordenados);
    }

    #[test]
    fn los_ids_deterministas_son_estables() {
        assert_eq!(EntityId::from_u128(7), EntityId::from_u128(7));
        assert!(EntityId::from_u128(7) < EntityId::from_u128(8));
    }

    #[test]
    fn binding_roto_no_tiene_valor() {
        let b: Binding<u32> = Binding::Broken;
        assert!(b.is_broken() && b.value().is_none());
        assert_eq!(Binding::Bound(3u32).value(), Some(3));
        assert_eq!(Binding::Rebound { value: 4u32, confidence: 0.8 }.value(), Some(4));
    }
}
