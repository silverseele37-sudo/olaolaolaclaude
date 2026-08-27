//! Proyección de un punto de mundo a coordenadas de pantalla.
//!
//! Comparte la cámara del contrato (`forge_render_api::Camera`) tanto el
//! renderizador provisional ([`crate::render_marcador`]) como el picking de
//! [`crate::seleccion`]: los dos necesitan la misma matriz vista-proyección,
//! así que se calcula una sola vez aquí.

use forge_math::{DMat4, DVec3};
use forge_render_api::Camera;

/// Vista × proyección para una cámara y un lienzo de `ancho`×`alto` píxeles.
pub fn vista_proyeccion(camara: &Camera, ancho_px: u32, alto_px: u32) -> DMat4 {
    let vista = DMat4::look_at_rh(camara.eye, camara.target, camara.up);
    let aspecto = ancho_px.max(1) as f64 / alto_px.max(1) as f64;
    let proyeccion =
        DMat4::perspective_rh(camara.fov_y_rad, aspecto, camara.near_mm, camara.far_mm);
    proyeccion * vista
}

/// Proyecta un punto de mundo a píxeles de pantalla (origen arriba-izquierda,
/// `y` creciendo hacia abajo, como espera `egui` y como se direcciona un
/// framebuffer).
///
/// `None` si el punto cae detrás de la cámara o fuera del volumen de recorte
/// en profundidad: no tiene una posición de pantalla que dibujar.
pub fn proyectar(vp: &DMat4, punto: DVec3, ancho_px: u32, alto_px: u32) -> Option<([f32; 2], f32)> {
    let clip = *vp * punto.extend(1.0);
    if clip.w <= 0.0 {
        return None; // detrás del ojo
    }
    let ndc = clip.truncate() / clip.w;
    if !(-1.0..=1.0).contains(&ndc.z) {
        return None;
    }
    let x = (ndc.x * 0.5 + 0.5) * ancho_px as f64;
    let y = (1.0 - (ndc.y * 0.5 + 0.5)) * alto_px as f64;
    Some(([x as f32, y as f32], ndc.z as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn camara_de_prueba() -> Camera {
        Camera {
            eye: DVec3::new(10.0, 0.0, 0.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: FRAC_PI_2,
            near_mm: 0.1,
            far_mm: 1000.0,
        }
    }

    #[test]
    fn el_objetivo_proyecta_al_centro_del_lienzo() {
        let c = camara_de_prueba();
        let vp = vista_proyeccion(&c, 800, 600);
        let (px, _) = proyectar(&vp, c.target, 800, 600).expect("el objetivo es visible");
        assert!((px[0] - 400.0).abs() < 1e-3, "x: {}", px[0]);
        assert!((px[1] - 300.0).abs() < 1e-3, "y: {}", px[1]);
    }

    #[test]
    fn un_punto_detras_de_la_camara_no_proyecta() {
        let c = camara_de_prueba();
        let vp = vista_proyeccion(&c, 800, 600);
        // el eje mira hacia -X desde (10,0,0); un punto en +X más allá del ojo
        // queda detrás.
        let detras = DVec3::new(20.0, 0.0, 0.0);
        assert!(proyectar(&vp, detras, 800, 600).is_none());
    }

    #[test]
    fn un_punto_arriba_proyecta_por_encima_del_centro() {
        let c = camara_de_prueba();
        let vp = vista_proyeccion(&c, 800, 600);
        let arriba = DVec3::new(0.0, 0.0, 1.0);
        let (px, _) = proyectar(&vp, arriba, 800, 600).unwrap();
        assert!(
            px[1] < 300.0,
            "en pantalla, arriba en mundo es menor y: {}",
            px[1]
        );
        assert!((px[0] - 400.0).abs() < 1e-3);
    }
}
