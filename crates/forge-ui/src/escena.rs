//! Reexporta la extracción de instancias, que vive en `forge-escena`.
//!
//! Se movió allí cuando `forge-runtime` necesitó el mismo camino: un
//! reproductor sin editor no puede depender de `forge-ui`, y duplicar la regla
//! de qué entidades se dibujan habría dado dos versiones que se separan a la
//! primera. Este módulo se queda para no romper a quien importe
//! `forge_ui::escena::*`.

pub use forge_escena::{caja_de_conjunto, extraer_instancias, ResolutorDeMallas, SinResolucion};
