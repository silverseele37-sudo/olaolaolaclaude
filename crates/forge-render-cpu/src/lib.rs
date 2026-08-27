//! Rasterizador por software: implementa [`forge_render_api::Renderer`] sin GPU.
//!
//! Existe por una razón muy concreta: los tests por imagen de referencia y la
//! comparación editor/runtime necesitan un camino de render que sea
//! **bit a bit reproducible** en cualquier máquina, sin depender de un driver
//! de GPU. Un rasterizador de software es lento comparado con wgpu, pero es
//! exactamente eso lo que permite verificar que la mitad wgpu hace lo mismo.
//!
//! # Mapa del crate
//!
//! - [`agx`]: el mapeo de tono, radiancia lineal → byte sRGB.
//! - [`camara`]: matrices de cámara, planos de frustum y orientación de caras.
//! - [`raster`]: el rasterizador propiamente dicho (z-buffer, recorte, rayos).
//! - [`malla`]: mallas y materiales del lado CPU (`MeshProvider`).
//! - [`sombreado`]: BRDF, armónicos esféricos y exposición medida.
//! - [`renderer`]: cablea todo lo anterior detrás del trait `Renderer`.

pub mod agx;
pub mod camara;
pub mod malla;
pub mod raster;
mod renderer;
pub mod sombreado;

pub use malla::{CpuMaterial, CpuMesh, MapaDeMallas, MeshProvider, TablaDeMateriales};
pub use renderer::SoftwareRenderer;
