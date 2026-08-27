//! Pilar 1: el árbol de features y su grafo de evaluación.
//!
//! Tres cosas viven aquí, y las tres son la misma pregunta vista desde ángulos
//! distintos: **¿a qué apunta una referencia después de editar algo aguas
//! arriba?**
//!
//! 1. [`tree`] — el árbol editable (insertar, suprimir sin borrar, reordenar,
//!    borrar) y el DAG que forma. El orden de evaluación es topológico y los
//!    ciclos se detectan; nunca se cuelga.
//! 2. [`eval`] — evaluación perezosa con caché por **hash de contenido**. La
//!    clave es `hash(kind, params, hashes de las entradas)`, así que dos
//!    subárboles que producen la misma geometría comparten resultado y un
//!    parámetro que no cambia la geometría no dispara recálculo aguas abajo.
//! 3. [`naming`] — el nombrado persistente, que es el riesgo R1 del proyecto.
//!
//! # La regla que hace reproducible el documento
//!
//! `evaluate` es una **función pura de (parámetros, entradas)**. Un nodo que
//! leyese el reloj, un contador global o el estado de la interfaz rompería la
//! reproducibilidad del documento entero: abrir el mismo archivo dos veces daría
//! geometrías distintas y la caché por hash de contenido pasaría a mentir. Por
//! eso el evaluador no expone ningún gancho para inyectar estado y todo lo que
//! un nodo necesita viaja en su `NodeKind`.
//!
//! # Lo que este crate **no** hace
//!
//! - No habla con OpenCASCADE ni con ningún kernel concreto: consume el trait
//!   [`forge_kernel_api::GeometryKernel`].
//! - No conoce ningún otro pilar (ADR-0006).
//! - No persiste nada: los nodos son datos serializables y quien los guarda es
//!   el documento.

use forge_doc::FeatureId;

pub mod eval;
pub mod hash;
pub mod naming;
pub mod solver;
pub mod tree;

pub use eval::{EvalOutcome, EvalStats, Evaluator, NodeOutput};
pub use naming::{Resolucion, Resolver, TopoRef};
pub use solver::GaussNewtonSolver;
pub use tree::{FeatureNode, FeatureTree, NodeKind, Plano, SketchNode};

/// Errores del pilar paramétrico.
///
/// Son datos, no pánicos: un árbol de features es contenido del usuario y
/// cualquiera de estas condiciones se alcanza escribiendo un modelo raro.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParamError {
    #[error("el nodo {0} no existe en el arbol")]
    NodoDesconocido(FeatureId),
    #[error("el grafo tiene un ciclo; nodos implicados: {0:?}")]
    Ciclo(Vec<FeatureId>),
    #[error("el nodo {nodo} referencia a {entrada}, que aparece despues en el orden del arbol")]
    OrdenIncoherente { nodo: FeatureId, entrada: FeatureId },
    #[error("no se puede borrar {0}: lo referencian {1:?}")]
    TieneDependientes(FeatureId, Vec<FeatureId>),
    #[error("el nodo {0} esta suprimido y no tiene entrada por la que pasar")]
    SuprimidoSinEntrada(FeatureId),
    #[error("el nodo {nodo} no pudo re-vincular {rotas} de {total} referencias; el arbol no se evalua con referencias rotas")]
    ReferenciaRota { nodo: FeatureId, rotas: usize, total: usize },
    #[error("el sketch {0} no converge o su perfil es degenerado: {1}")]
    SketchInvalido(FeatureId, String),
    #[error(transparent)]
    Kernel(#[from] forge_kernel_api::KernelError),
}

pub type Result<T> = std::result::Result<T, ParamError>;
