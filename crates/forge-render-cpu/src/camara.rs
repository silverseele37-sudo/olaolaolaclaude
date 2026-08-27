//! Cámara y frustum.
//!
//! # El handedness, que es donde se pierde un día
//!
//! El documento es **Z arriba y diestro**. La proyección es
//! `perspective_rh` con profundidad `[0, 1]` —la convención de wgpu— y la vista
//! es `look_at_rh`, así que la cámara mira hacia `-Z` de su propio espacio.
//!
//! El punto delicado no es ninguna de esas dos, sino el paso a píxeles: en NDC
//! `+Y` es **arriba**, y en un buffer de imagen la fila 0 es la de **arriba**.
//! Hay que invertir Y ahí y solo ahí. Un signo de más o de menos en cualquiera
//! de los tres sitios produce una escena espejada que *casi* se ve bien —la
//! iluminación sigue siendo plausible, las siluetas siguen cerrando— y que solo
//! delata un texto o una pieza asimétrica. Por eso hay un test de orientación
//! con control positivo y no una inspección visual.
//!
//! Consecuencia de invertir Y en píxeles: un triángulo antihorario en NDC (el
//! frente, por la regla de la mano derecha) queda **horario** en píxeles, o sea
//! con área con signo negativa. De ahí sale [`Facing`].

use forge_math::{DMat4, DVec3, DVec4};
use forge_render_api::Camera;

/// Matrices derivadas de una [`Camera`], listas para transformar.
#[derive(Clone, Copy, Debug)]
pub struct Camara {
    pub vista: DMat4,
    pub proyeccion: DMat4,
    pub vista_proyeccion: DMat4,
    pub ojo: DVec3,
}

impl Camara {
    /// Construye las matrices. **No entra en pánico** con cámaras degeneradas:
    /// una cámara con `eye == target` o con `up` paralelo a la dirección de
    /// vista es un estado transitorio normal mientras el usuario orbita, no un
    /// bug del que haya que abortar.
    pub fn nueva(c: &Camera, aspecto: f64) -> Camara {
        let mut adelante = c.target - c.eye;
        if adelante.length_squared() < 1e-18 {
            // Cámara sobre su propio objetivo: se mira hacia -Y, que en Z-up es
            // la vista frontal habitual de CAD.
            adelante = -DVec3::Y;
        }
        let adelante = adelante.normalize();

        let mut arriba = if c.up.length_squared() < 1e-18 { DVec3::Z } else { c.up.normalize() };
        // `up` paralelo a la vista (mirar en vertical desde arriba) degenera el
        // producto vectorial. Se elige otra referencia en vez de dar NaN.
        if arriba.cross(adelante).length_squared() < 1e-12 {
            arriba = if adelante.z.abs() > 0.9 { DVec3::Y } else { DVec3::Z };
        }

        let near = c.near_mm.max(1e-6);
        let far = c.far_mm.max(near * (1.0 + 1e-6));
        let fov = c.fov_y_rad.clamp(1e-4, std::f64::consts::PI - 1e-4);
        let aspecto = if aspecto.is_finite() && aspecto > 1e-9 { aspecto } else { 1.0 };

        let vista = DMat4::look_at_rh(c.eye, c.eye + adelante, arriba);
        let proyeccion = DMat4::perspective_rh(fov, aspecto, near, far);
        Camara { vista, proyeccion, vista_proyeccion: proyeccion * vista, ojo: c.eye }
    }

    /// Los 6 planos del frustum en mundo, `(a, b, c, d)` con dentro = `>= 0`.
    ///
    /// Extracción de Gribb–Hartmann sobre la matriz combinada. La fila del plano
    /// cercano es `r2` sin sumar `r3` porque la profundidad es `[0, 1]` y no
    /// `[-1, 1]`; con la fórmula de OpenGL el plano cercano queda mal colocado y
    /// el culling empieza a comerse geometría delante de la cámara.
    pub fn planos(&self) -> [DVec4; 6] {
        let m = self.vista_proyeccion;
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let p = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        let mut out = [DVec4::ZERO; 6];
        for (i, q) in p.into_iter().enumerate() {
            let n = DVec3::new(q.x, q.y, q.z).length();
            out[i] = if n > 1e-12 { q / n } else { q };
        }
        out
    }
}

/// Orientación de un triángulo ya proyectado a píxeles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing {
    Frontal,
    Trasera,
    /// Degenerado: área nula. No se dibuja y no cuenta como cara trasera.
    Nula,
}

/// Área con signo en espacio de píxeles y su lectura.
///
/// Ver la nota del módulo: con la Y invertida, **frontal = área negativa**.
#[inline]
pub fn facing(area: f32) -> Facing {
    if area < 0.0 {
        Facing::Frontal
    } else if area > 0.0 {
        Facing::Trasera
    } else {
        Facing::Nula
    }
}

/// ¿La caja queda completamente fuera de algún plano?
///
/// Conservador: puede aceptar cajas que en realidad no se ven (esquinas del
/// frustum), nunca rechaza una que sí. El error en esa dirección cuesta píxeles;
/// en la otra, cuesta geometría que desaparece.
pub fn fuera_del_frustum(planos: &[DVec4; 6], esquinas: &[DVec3; 8]) -> bool {
    planos.iter().any(|p| {
        esquinas.iter().all(|c| p.x * c.x + p.y * c.y + p.z * c.z + p.w < 0.0)
    })
}
