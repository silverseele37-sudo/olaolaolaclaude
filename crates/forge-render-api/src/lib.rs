//! Contrato del motor de render.
//!
//! La regla que sostiene todo lo demás: **el renderer no conoce el documento.**
//! Recibe una [`SceneView`] —una vista aplanada de lo que hay que dibujar— que
//! produce una capa de extracción que sí lo conoce.
//!
//! De ahí salen tres cosas que de otro modo cuestan meses:
//!
//! - `forge-runtime` puede compartir **exactamente** el mismo camino de render
//!   que el editor, porque el editor no le añade nada al renderer: le añade
//!   instancias.
//! - El render puede correr en su propio hilo leyendo un snapshot inmutable,
//!   sin bloquear al hilo de documento.
//! - Las instancias referencian mallas y materiales **por hash**, así que el
//!   diff contra el frame anterior es comparar enteros y solo se sube a GPU lo
//!   que cambió de verdad.

use forge_doc::EntityId;
use forge_math::{Aabb, DAffine3, DVec3};
use forge_store::BlobHash;
use serde::{Deserialize, Serialize};

/// Identidad de un material dentro de la vista.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct MaterialId(pub u64);

impl MaterialId {
    pub const DEFAULT: MaterialId = MaterialId(0);
}

/// Una cosa que dibujar.
///
/// Sin punteros: malla y material van por hash e id, y la transformada va
/// resuelta a mundo. Es lo que hace barato el diff entre frames.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DrawInstance {
    /// De dónde vino, para el picking y para resaltar la selección.
    pub entity: EntityId,
    /// Hash del contenido de la malla. Dos instancias con el mismo hash
    /// comparten buffers de GPU sin que el renderer tenga que saber por qué.
    pub mesh: BlobHash,
    pub material: MaterialId,
    pub transform: DAffine3,
    pub bounds: Aabb,
    pub visible: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    pub eye: DVec3,
    pub target: DVec3,
    /// Arriba de la cámara. Por defecto `+Z`: el documento es Z-up y el visor
    /// no es el sitio donde cambiar de convención.
    pub up: DVec3,
    pub fov_y_rad: f64,
    pub near_mm: f64,
    pub far_mm: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            eye: DVec3::new(300.0, -400.0, 250.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: 45f64.to_radians(),
            near_mm: 0.1,
            far_mm: 100_000.0,
        }
    }
}

impl Camera {
    pub fn distance(&self) -> f64 {
        (self.eye - self.target).length()
    }

    /// Deflexión de teselado adecuada para esta cámara (ADR-0002, R1b).
    pub fn chord_deflection(&self, height_px: f64, px_error: f64) -> f64 {
        forge_math::chord_deflection(self.distance(), self.fov_y_rad, height_px, px_error)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Light {
    Directional { direction: DVec3, color: [f32; 3], intensity: f32 },
    Point { position: DVec3, color: [f32; 3], intensity: f32, radius_mm: f64 },
}

/// Iluminación basada en imagen.
///
/// La irradiancia difusa va en 9 coeficientes de armónicos esféricos y no en un
/// cubemap: en entornos de estudio —que es lo que usa un visor CAD— el error es
/// inferior al 1 %, ocupa 144 bytes en vez de 48 KB, no gasta un binding ni un
/// sampler, y cambiar de entorno es instantáneo.
///
/// Cláusula de seguridad obligatoria en el shader: `max(irradiancia, 0)`. Con
/// luces muy intensas el ringing de SH puede dar irradiancia negativa.
#[derive(Clone, Debug, PartialEq)]
pub struct Ibl {
    /// 9 coeficientes RGB, orden `l=0`, luego `l=1` (y, z, x), luego `l=2`.
    pub sh: [[f32; 3]; 9],
    /// Cubemap especular prefiltrado, por hash. `None` = solo difusa.
    pub prefiltered: Option<BlobHash>,
    pub intensity: f32,
    pub rotation_rad: f32,
}

/// Lo que el renderer dibuja. No conoce features, sketches ni activos.
#[derive(Clone, Debug)]
pub struct SceneView<'a> {
    pub camera: Camera,
    pub instances: &'a [DrawInstance],
    pub lights: &'a [Light],
    pub environment: Option<&'a Ibl>,
    /// Exposición. Si es `None`, el renderer la **mide** del entorno
    /// (`π / irradiancia_media`) en vez de usar una constante sintonizada a ojo.
    /// Con eso, cambiar las luces no invalida en silencio números repartidos por
    /// el código.
    pub exposure: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderTarget {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct RenderStats {
    pub instances_submitted: u32,
    pub instances_culled: u32,
    pub triangles: u64,
    pub draw_calls: u32,
    pub gpu_uploads: u32,
    pub cpu_time_us: u64,
}

pub trait Renderer {
    fn name(&self) -> &'static str;
    fn render(&mut self, view: &SceneView<'_>, target: RenderTarget) -> RenderStats;

    /// Render sin ventana, determinista.
    ///
    /// **Es una primitiva, no una función accesoria.** De ella dependen los
    /// tests por imagen de referencia, la comparación editor/runtime y el modo
    /// batch. En `cadviz` existe desde el primer milestone, y la superficie de
    /// ventana usa formato no-sRGB precisamente para que lo que se ve sea
    /// idéntico byte a byte a lo que se escribe: si divergieran, la verificación
    /// por imagen no significaría nada.
    fn render_offscreen(&mut self, view: &SceneView<'_>, target: RenderTarget) -> Vec<u8>;
}
