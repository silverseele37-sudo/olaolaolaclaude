//! Creación del dispositivo wgpu.
//!
//! # La trampa de los gráficos híbridos (docs/construir.md §2)
//!
//! En un portátil con integrada AMD y discreta NVIDIA, pedir `Backends::all()`
//! incluye el backend GL junto a Vulkan y DX12, y **eso revienta dentro de
//! `wgpu::Instance::new`**, antes siquiera de poder enumerar adaptadores: no es
//! un fallo de esta aplicación, es la convivencia de un contexto GL legacy con
//! un sistema que ya tiene Vulkan corriendo en dos GPUs distintas.
//!
//! Por eso aquí se pide **[`wgpu::Backends::PRIMARY`]** (Vulkan + DX12 + Metal)
//! y nunca `all()`. Es además lo correcto para este programa: sin Vulkan ni
//! DX12, un motor de render moderno no tiene nada que hacer, así que no vale
//! la pena cargar el backend que causa el crash a cambio de un fallback que de
//! todas formas sería inutilizable.
//!
//! Este módulo no se puede ejercitar en el contenedor de desarrollo (sin
//! `/dev/dri`, `request_adapter` siempre deniega), así que la verificación real
//! —que efectivamente evita el crash y encuentra la RTX 5050— queda pendiente
//! de la máquina del usuario. Ver el doc del crate para el resto de la tabla de
//! verificación.

use thiserror::Error;

/// Fallos al preparar la GPU. Ninguno es un `panic!`: pedir un dispositivo que
/// no existe (este contenedor, o un portátil sin drivers Vulkan/DX12 al día)
/// es una condición esperable en tiempo de ejecución, no un bug.
#[derive(Debug, Error)]
pub enum ErrorDispositivo {
    #[error(
        "ningun adaptador Vulkan/DX12/Metal disponible; con graficos hibridos, comprobar \
         `vulkaninfo --summary` y drivers al dia (ver docs/construir.md #2)"
    )]
    SinAdaptador,
    #[error("no se pudo abrir el dispositivo logico: {0}")]
    Dispositivo(#[from] wgpu::RequestDeviceError),
}

/// Dispositivo listo para renderizar, sin superficie de ventana.
pub struct Dispositivo {
    pub adaptador: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Dispositivo {
    /// Crea instancia, adaptador y dispositivo lógico. Bloquea sobre las
    /// futuras de wgpu con `pollster`: no hay ejecutor async en este crate ni
    /// falta hace uno para una operación que ocurre una vez al arrancar el
    /// visor.
    pub fn nuevo() -> Result<Dispositivo, ErrorDispositivo> {
        let mut descriptor_instancia = wgpu::InstanceDescriptor::new_without_display_handle();
        // Ver la nota del módulo: nunca `Backends::all()`.
        descriptor_instancia.backends = wgpu::Backends::PRIMARY;
        let instancia = wgpu::Instance::new(descriptor_instancia);

        let opciones = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            // Sin superficie: este crate no presenta a una ventana (ver el
            // doc de `crate::renderer` sobre por qué `render_offscreen` es la
            // primitiva real y `render` no recibe un handle de ventana).
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        };
        let adaptador = pollster::block_on(instancia.request_adapter(&opciones))
            .map_err(|_| ErrorDispositivo::SinAdaptador)?;

        let descriptor = wgpu::DeviceDescriptor {
            label: Some("forge-render device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        };
        let (device, queue) = pollster::block_on(adaptador.request_device(&descriptor))?;

        Ok(Dispositivo {
            adaptador,
            device,
            queue,
        })
    }
}
