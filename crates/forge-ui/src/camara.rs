//! Cámara orbital: órbita, pan, zoom y encuadre.
//!
//! Los cuatro son los idiomas de cualquier CAD (eDrawings, KeyShot, Fusion) y
//! deliberadamente no se reinventan aquí. Es matemática pura sobre coordenadas
//! esféricas alrededor de un punto de interés — nada de esto toca una ventana
//! ni un dispositivo, así que se verifica igual con o sin GPU.
//!
//! # Convención
//!
//! El documento es Z-arriba (`forge_math::UP`); la cámara guarda su posición
//! como `(objetivo, distancia, acimut, elevación)` en vez de como matriz para
//! que orbitar sea sumar un ángulo, no re-extraer una rotación.

use forge_math::{Aabb, DVec3};
use forge_render_api::Camera;

/// Elevación máxima antes del polo, en radianes. Con `PI/2` exacto el vector
/// "derecha" de la cámara degenera (arriba y dirección de vista quedan
/// paralelos); el margen evita ese caso sin que se note al orbitar.
const ELEVACION_MAX_RAD: f64 = std::f64::consts::FRAC_PI_2 - 1e-4;

/// Distancia mínima al objetivo. Sin piso, un zoom sostenido lleva la cámara a
/// `0` y de ahí a `-∞`, atravesando el objetivo.
const DISTANCIA_MIN_MM: f64 = 1e-3;

/// Cámara orbital, en coordenadas esféricas alrededor de `objetivo`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamaraOrbital {
    pub objetivo: DVec3,
    pub distancia_mm: f64,
    /// Ángulo en el plano XY, medido desde `+X` hacia `+Y`.
    pub acimut_rad: f64,
    /// Elevación sobre el plano XY. `0` = horizonte, `+PI/2` = cenit.
    pub elevacion_rad: f64,
    pub fov_y_rad: f64,
    pub near_mm: f64,
    pub far_mm: f64,
}

impl Default for CamaraOrbital {
    fn default() -> Self {
        let c = Camera::default();
        CamaraOrbital::desde_camara(&c)
    }
}

impl CamaraOrbital {
    /// Reconstruye los ángulos esféricos a partir de una `Camera` concreta.
    pub fn desde_camara(c: &Camera) -> Self {
        let offset = c.eye - c.target;
        let distancia = offset.length().max(DISTANCIA_MIN_MM);
        let elevacion = (offset.z / distancia).clamp(-1.0, 1.0).asin();
        let acimut = offset.y.atan2(offset.x);
        CamaraOrbital {
            objetivo: c.target,
            distancia_mm: distancia,
            acimut_rad: acimut,
            elevacion_rad: elevacion.clamp(-ELEVACION_MAX_RAD, ELEVACION_MAX_RAD),
            fov_y_rad: c.fov_y_rad,
            near_mm: c.near_mm,
            far_mm: c.far_mm,
        }
    }

    /// Proyecta el estado a la `Camera` que consume `forge-render-api`.
    pub fn camara(&self) -> Camera {
        let dir = DVec3::new(
            self.elevacion_rad.cos() * self.acimut_rad.cos(),
            self.elevacion_rad.cos() * self.acimut_rad.sin(),
            self.elevacion_rad.sin(),
        );
        Camera {
            eye: self.objetivo + dir * self.distancia_mm,
            target: self.objetivo,
            up: DVec3::Z,
            fov_y_rad: self.fov_y_rad,
            near_mm: self.near_mm,
            far_mm: self.far_mm,
        }
    }

    /// Arrastrar orbita: `dx`/`dy` en radianes por píxel de arrastre ya
    /// aplicado por quien llama (normalmente `delta_px * sensibilidad`).
    pub fn orbitar(&mut self, dx_rad: f64, dy_rad: f64) {
        self.acimut_rad -= dx_rad;
        self.elevacion_rad =
            (self.elevacion_rad + dy_rad).clamp(-ELEVACION_MAX_RAD, ELEVACION_MAX_RAD);
    }

    /// Botón medio panea: mueve el objetivo (y por tanto el ojo, que lo sigue
    /// a distancia fija) en el plano de la pantalla.
    ///
    /// `dx_mm`/`dy_mm` ya vienen convertidos a milímetros de mundo por quien
    /// llama (ver [`mundo_por_pixel`]), así que panear es una traslación en la
    /// base ortonormal derecha/arriba de la cámara.
    pub fn panear(&mut self, dx_mm: f64, dy_mm: f64) {
        let dir = self.camara().eye - self.objetivo;
        let adelante = if dir.length_squared() > 0.0 {
            dir.normalize()
        } else {
            DVec3::X
        };
        let (derecha, arriba) = forge_math::orthonormal_basis(adelante);
        self.objetivo += derecha * -dx_mm + arriba * dy_mm;
    }

    /// Rueda zoom: multiplica la distancia por `factor`. `factor < 1` acerca,
    /// `factor > 1` aleja. Es multiplicativo (no aditivo) para que el zoom se
    /// sienta igual de rápido cerca que lejos del objetivo.
    pub fn zoom(&mut self, factor: f64) {
        self.distancia_mm = (self.distancia_mm * factor).max(DISTANCIA_MIN_MM);
    }

    /// `F`: encuadra una caja de mundo.
    ///
    /// Mueve el objetivo al centro de la caja y ajusta la distancia para que
    /// la esfera que circunscribe la caja (radio = diagonal / 2) quede
    /// exactamente dentro del campo de visión vertical, sin margen:
    ///
    /// ```text
    /// distancia = radio / sin(fov_y / 2)
    /// ```
    ///
    /// Una caja vacía (nada que encuadrar, p. ej. selección vacía sobre un
    /// documento sin geometría resuelta) deja la cámara donde estaba: es
    /// preferible a saltar a un encuadre arbitrario.
    pub fn encuadrar(&mut self, caja: Aabb) {
        if caja.is_empty() {
            return;
        }
        self.objetivo = caja.center();
        let radio = (caja.diagonal() * 0.5).max(DISTANCIA_MIN_MM);
        self.distancia_mm = radio / (self.fov_y_rad * 0.5).sin();
    }
}

/// Milímetros de mundo que cubre un píxel de pantalla a la distancia actual.
///
/// Misma fórmula que [`forge_math::chord_deflection`] sin el término de error
/// admitido: aquí se usa para convertir un arrastre en píxeles a un
/// desplazamiento de pan en milímetros, no para decidir una tolerancia de
/// teselado.
pub fn mundo_por_pixel(distancia_mm: f64, fov_y_rad: f64, alto_px: f64) -> f64 {
    debug_assert!(alto_px > 0.0);
    2.0 * distancia_mm * (fov_y_rad * 0.5).tan() / alto_px
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn cerca(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn ida_y_vuelta_por_camara_reproduce_ojo_y_objetivo() {
        let c = Camera {
            eye: DVec3::new(300.0, -400.0, 250.0),
            target: DVec3::new(1.0, 2.0, 3.0),
            up: DVec3::Z,
            fov_y_rad: 45f64.to_radians(),
            near_mm: 0.1,
            far_mm: 1000.0,
        };
        let orbital = CamaraOrbital::desde_camara(&c);
        let c2 = orbital.camara();
        assert!(
            (c2.eye - c.eye).length() < 1e-9,
            "eye: {:?} vs {:?}",
            c2.eye,
            c.eye
        );
        assert!((c2.target - c.target).length() < 1e-9);
    }

    #[test]
    fn orbitar_no_cambia_la_distancia_al_objetivo() {
        let mut o = CamaraOrbital::default();
        let d0 = (o.camara().eye - o.objetivo).length();
        o.orbitar(0.7, 0.3);
        let d1 = (o.camara().eye - o.objetivo).length();
        assert!(cerca(d0, d1, 1e-9), "orbitar no debe alterar la distancia");
    }

    #[test]
    fn orbitar_no_traspasa_el_polo() {
        let mut o = CamaraOrbital::default();
        for _ in 0..100 {
            o.orbitar(0.0, 1.0); // empuja fuerte hacia el cenit
        }
        assert!(o.elevacion_rad <= ELEVACION_MAX_RAD + 1e-12);
        assert!(o.camara().eye.is_finite());
    }

    #[test]
    fn zoom_es_multiplicativo_y_tiene_piso() {
        let mut o = CamaraOrbital::default();
        let d0 = o.distancia_mm;
        o.zoom(0.5);
        assert!(cerca(o.distancia_mm, d0 * 0.5, 1e-9));
        for _ in 0..200 {
            o.zoom(0.01);
        }
        assert!(o.distancia_mm >= DISTANCIA_MIN_MM);
        assert!(o.distancia_mm.is_finite() && o.distancia_mm > 0.0);
    }

    #[test]
    fn panear_mueve_objetivo_y_ojo_por_igual() {
        let mut o = CamaraOrbital::default();
        let objetivo0 = o.objetivo;
        let offset0 = o.camara().eye - o.objetivo;
        o.panear(10.0, -5.0);
        let offset1 = o.camara().eye - o.objetivo;
        // panear traslada el par (ojo, objetivo) rígidamente: el offset entre
        // ambos no cambia...
        assert!(
            (offset0 - offset1).length() < 1e-9,
            "el offset ojo-objetivo debe conservarse al panear"
        );
        // ...pero el objetivo sí se mueve: si no, panear no haría nada.
        assert!((o.objetivo - objetivo0).length() > 1e-6);
    }

    /// Respuesta conocida, calculable a mano: encuadrar un cubo de 2×2×2
    /// centrado en el origen con FOV vertical de 90°.
    ///
    /// diagonal = 2·√3 ⇒ radio = √3; sin(45°) = √2/2
    /// distancia = √3 / (√2/2) = √3·2/√2 = 2·√(3/2) ≈ 2.449489743
    #[test]
    fn encuadrar_un_cubo_conocido_da_la_distancia_calculada_a_mano() {
        let mut o = CamaraOrbital {
            fov_y_rad: FRAC_PI_2,
            ..CamaraOrbital::default()
        };
        let caja = Aabb::new(DVec3::splat(-1.0), DVec3::splat(1.0));
        o.encuadrar(caja);

        assert!((o.objetivo - DVec3::ZERO).length() < 1e-12);
        let esperado = 2.0 * 1.5f64.sqrt();
        assert!(
            cerca(o.distancia_mm, esperado, 1e-9),
            "esperaba {esperado}, fue {}",
            o.distancia_mm
        );

        // y la esfera circunscrita queda justo en el borde del frustum: el
        // punto más lejano de la caja proyectado en Y de vista debe alinear
        // con el semiángulo vertical exacto.
        let radio = caja.diagonal() * 0.5;
        let semiangulo = (radio / o.distancia_mm).asin();
        assert!(cerca(semiangulo, FRAC_PI_2 * 0.5, 1e-9));
    }

    #[test]
    fn encuadrar_caja_vacia_no_mueve_la_camara() {
        let mut o = CamaraOrbital::default();
        let antes = o;
        o.encuadrar(Aabb::EMPTY);
        assert_eq!(o, antes);
    }

    #[test]
    fn mundo_por_pixel_escala_linealmente_con_la_distancia() {
        let fov = 45f64.to_radians();
        let a = mundo_por_pixel(1000.0, fov, 720.0);
        let b = mundo_por_pixel(2000.0, fov, 720.0);
        assert!(cerca(b / a, 2.0, 1e-12));
    }

    #[test]
    fn desde_camara_recorta_una_elevacion_extrema() {
        let c = Camera {
            eye: DVec3::new(0.0, 0.0, 1_000_000.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: 45f64.to_radians(),
            near_mm: 0.1,
            far_mm: 2_000_000.0,
        };
        let o = CamaraOrbital::desde_camara(&c);
        assert!(o.elevacion_rad <= ELEVACION_MAX_RAD);
        assert!(o.elevacion_rad > FRAC_PI_2 - 1e-2);
    }
}
