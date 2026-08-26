//! FORGE — fachada.
//!
//! Reexporta los crates del núcleo para que una aplicación dependa de uno solo.
//! **No añade lógica**: si algo necesita vivir aquí, vive en el crate que le
//! corresponde.
//!
//! Este crate es además el hogar de `tests/arquitectura.rs`, que hace cumplir
//! mecánicamente las fronteras entre módulos. Ver ADR-0006.

pub use forge_doc as doc;
pub use forge_io as io;
pub use forge_math as math;
pub use forge_store as store;

/// Lo que hace falta para trabajar con un documento.
pub mod prelude {
    pub use forge_doc::{
        ComponentRegistry, DocEvent, Document, Domain, EntityId, FeatureId, Geometry,
        GeometryPayload, Name, Parent, Snapshot, StableId, Transform, Visible,
    };
    pub use forge_math::{DVec3, Transform as MathTransform};
    pub use forge_store::{BlobHash, BlobStore};
}
