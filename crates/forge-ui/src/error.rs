//! Errores de `forge-ui`.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("no se pudo crear la ventana: {0}")]
    Ventana(String),

    #[error("no se pudo inicializar wgpu (¿hay un adaptador Vulkan o DX12 disponible?): {0}")]
    Wgpu(String),

    #[error("no se pudo escribir la imagen en {ruta}: {fuente}")]
    Png {
        ruta: PathBuf,
        #[source]
        fuente: std::io::Error,
    },

    #[error("no se pudo codificar el PNG: {0}")]
    Codec(String),

    #[error("dimensiones de render inválidas: {ancho}x{alto} (ambas deben ser > 0)")]
    DimensionesInvalidas { ancho: u32, alto: u32 },
}

pub type Result<T> = std::result::Result<T, UiError>;
