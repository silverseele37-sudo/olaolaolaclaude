//! Primitivas geométricas de FORGE.
//!
//! Tres convenciones, fijadas en la Fase 0 y no negociables dentro del núcleo
//! (`docs/fase-0/00-arquitectura.md` §4):
//!
//! - **Unidad interna: milímetro.** La unidad que ve el usuario es de presentación.
//! - **Tipo: `f64`.** El `f32` aparece solo al subir a GPU, con coordenadas
//!   relativas a la cámara.
//! - **Ejes: Z arriba, diestro.** Es la convención de CAD y STEP. La conversión
//!   desde/hacia Y-up (glTF, buena parte de la bibliografía de gráficos) ocurre
//!   únicamente en `forge-interop`.

pub use glam::{DAffine3, DMat3, DMat4, DQuat, DVec2, DVec3};

use serde::{Deserialize, Serialize};

/// Arriba, en coordenadas de documento.
pub const UP: DVec3 = DVec3::Z;

/// Tolerancias del núcleo.
pub mod tol {
    /// Distancia por debajo de la cual dos puntos son el mismo punto.
    ///
    /// Coincide con el valor por defecto de OpenCASCADE (`Precision::Confusion`)
    /// asumiendo modelo en milímetros. Consecuencia práctica documentada: por
    /// encima de ~1e5 mm la tolerancia relativa se degrada y los booleanos
    /// empiezan a fallar.
    pub const CONFUSION_MM: f64 = 1e-7;

    /// Ángulo por debajo del cual dos direcciones son la misma dirección.
    pub const ANGULAR_RAD: f64 = 1e-9;

    /// Escala por encima de la cual `CONFUSION_MM` deja de ser fiable.
    pub const SCALE_WARN_MM: f64 = 1.0e5;

    #[inline]
    pub fn eq(a: f64, b: f64) -> bool {
        (a - b).abs() <= CONFUSION_MM
    }
}

/// Base ortonormal alrededor de una normal.
///
/// Devuelve `(tangente, bitangente)` tales que `(t, b, n)` es diestra y
/// ortonormal.
///
/// **La guarda del caso degenerado no es opcional.** Construir la base con
/// `cross(semilla, n)` produce `NaN` cuando `n` es paralela a la semilla. El
/// código de gráficos que circula siembra con `(0,1,0)` porque asume Y-up, donde
/// esa dirección es un polo y se visita poco. En un mundo Z-up, `(0,1,0)` es una
/// dirección horizontal ordinaria a la que apuntan muchísimas caras, así que el
/// mismo código falla en el ecuador en vez de en los polos.
///
/// Encontrado como bug latente en la referencia técnica de M2 de `cadviz`
/// (`fs_irradiance` sembraba con `(0,1,0)` sin guarda).
#[inline]
pub fn orthonormal_basis(n: DVec3) -> (DVec3, DVec3) {
    let n = n.normalize();
    // Semilla Z, con salto a X cuando n es (casi) paralela a Z.
    let seed = if n.z.abs() >= 0.999 { DVec3::X } else { DVec3::Z };
    let t = seed.cross(n).normalize();
    let b = n.cross(t);
    (t, b)
}

/// Deflexión de cuerda derivada del tamaño de un píxel en el mundo.
///
/// Implementa la regla R1b de ADR-0002: el teselado es caché de *una vista*, así
/// que su tolerancia sale de la vista y no de una constante elegida a mano.
///
/// `px_error` es el error geométrico admitido, en píxeles. 0.4 es el valor de
/// `cadviz`, donde está medido: contra una constante elegida para la vista
/// lejana, el error real subía a 1.2 px a 3× de zoom y a 2.0 px a 5×.
#[inline]
pub fn chord_deflection(
    distance_mm: f64,
    fov_y_rad: f64,
    viewport_height_px: f64,
    px_error: f64,
) -> f64 {
    debug_assert!(viewport_height_px > 0.0);
    let world_per_px = 2.0 * distance_mm * (fov_y_rad * 0.5).tan() / viewport_height_px;
    world_per_px * px_error
}

/// Caja alineada a los ejes.
///
/// `EMPTY` es el elemento neutro de `union`: mínimo en `+inf`, máximo en `-inf`.
/// Cualquier otra representación de "vacío" obliga a un caso especial en cada
/// acumulación.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: DVec3::splat(f64::INFINITY),
        max: DVec3::splat(f64::NEG_INFINITY),
    };

    pub fn new(min: DVec3, max: DVec3) -> Self {
        Aabb { min, max }
    }

    pub fn from_points(pts: impl IntoIterator<Item = DVec3>) -> Self {
        pts.into_iter().fold(Aabb::EMPTY, |a, p| a.extended(p))
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    #[inline]
    pub fn extended(self, p: DVec3) -> Self {
        Aabb { min: self.min.min(p), max: self.max.max(p) }
    }

    #[inline]
    pub fn union(self, o: Aabb) -> Self {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }

    #[inline]
    pub fn contains(&self, p: DVec3) -> bool {
        !self.is_empty() && p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    #[inline]
    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    #[inline]
    pub fn size(&self) -> DVec3 {
        if self.is_empty() { DVec3::ZERO } else { self.max - self.min }
    }

    #[inline]
    pub fn diagonal(&self) -> f64 {
        self.size().length()
    }

    /// Los 8 vértices, en orden determinista.
    pub fn corners(&self) -> [DVec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            DVec3::new(a.x, a.y, a.z), DVec3::new(b.x, a.y, a.z),
            DVec3::new(a.x, b.y, a.z), DVec3::new(b.x, b.y, a.z),
            DVec3::new(a.x, a.y, b.z), DVec3::new(b.x, a.y, b.z),
            DVec3::new(a.x, b.y, b.z), DVec3::new(b.x, b.y, b.z),
        ]
    }

    /// Caja de la caja transformada. Conservadora: no es la caja mínima del
    /// contenido, sino la de los 8 vértices transformados.
    pub fn transformed(&self, m: &DAffine3) -> Self {
        if self.is_empty() {
            return Aabb::EMPTY;
        }
        Aabb::from_points(self.corners().into_iter().map(|c| m.transform_point3(c)))
    }
}

/// Posición, orientación y escala de una entidad respecto de su padre.
///
/// Se guarda descompuesta y no como matriz: una matriz obliga a re-extraer la
/// rotación en cada edición de la interfaz, y esa extracción no es estable
/// cuando hay escala no uniforme.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { translation: DVec3::ZERO, rotation: DQuat::IDENTITY, scale: DVec3::ONE }
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
        scale: DVec3::ONE,
    };

    pub fn from_translation(t: DVec3) -> Self {
        Transform { translation: t, ..Default::default() }
    }

    #[inline]
    pub fn to_affine(&self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Composición: `self` aplicada después de `parent`.
    pub fn then(&self, parent: &Transform) -> DAffine3 {
        parent.to_affine() * self.to_affine()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Respuesta conocida: la base tiene que ser ortonormal y finita para
    /// **toda** dirección, incluidas las que rompen el código Y-up sin guarda.
    #[test]
    fn base_ortonormal_es_finita_en_toda_direccion() {
        let mut casos = vec![
            DVec3::X, -DVec3::X,
            DVec3::Y, -DVec3::Y,   // el ecuador en Z-up: donde falla el codigo Y-up
            DVec3::Z, -DVec3::Z,   // los polos en Z-up: donde falla la semilla Z sin guarda
            DVec3::new(1.0, 1.0, 1.0),
        ];
        // barrido denso de la esfera
        let n = 64;
        for i in 0..n {
            for j in 0..n {
                let theta = std::f64::consts::PI * (i as f64 + 0.5) / n as f64;
                let phi = std::f64::consts::TAU * (j as f64) / n as f64;
                casos.push(DVec3::new(
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    theta.cos(),
                ));
            }
        }

        for d in casos {
            let n = d.normalize();
            let (t, b) = orthonormal_basis(n);
            assert!(t.is_finite(), "tangente no finita para {n:?}");
            assert!(b.is_finite(), "bitangente no finita para {n:?}");
            assert!((t.length() - 1.0).abs() < 1e-12, "tangente no unitaria para {n:?}");
            assert!((b.length() - 1.0).abs() < 1e-12, "bitangente no unitaria para {n:?}");
            assert!(t.dot(n).abs() < 1e-12, "t no perpendicular a n para {n:?}");
            assert!(b.dot(n).abs() < 1e-12, "b no perpendicular a n para {n:?}");
            assert!(t.dot(b).abs() < 1e-12, "t no perpendicular a b para {n:?}");
            // diestra: t x b == n
            assert!((t.cross(b) - n).length() < 1e-12, "base no diestra para {n:?}");
        }
    }

    /// Control negativo del test anterior: la version *sin* guarda que circula
    /// en la bibliografia Y-up produce NaN en el ecuador de un mundo Z-up.
    /// Si este test empieza a fallar es que alguien "arreglo" la semilla.
    #[test]
    fn control_negativo_la_version_sin_guarda_si_falla() {
        let sin_guarda = |n: DVec3| -> DVec3 { DVec3::Y.cross(n).normalize() };
        let en_el_ecuador = DVec3::Y;
        assert!(
            !sin_guarda(en_el_ecuador).is_finite(),
            "el control negativo dejo de reproducir el bug; revisar el test, no el codigo"
        );
        // y la version con guarda sobrevive exactamente al mismo caso
        assert!(orthonormal_basis(en_el_ecuador).0.is_finite());
    }

    /// La deflexion cae con el zoom: es el argumento del proyecto hecho numero.
    #[test]
    fn deflexion_escala_con_la_distancia() {
        let fov = 45f64.to_radians();
        let h = 1000.0;
        let lejos = chord_deflection(1000.0, fov, h, 0.4);
        let cerca = chord_deflection(250.0, fov, h, 0.4);
        assert!((lejos / cerca - 4.0).abs() < 1e-12, "debe ser lineal en la distancia");

        // Contraste contra una constante elegida para la vista lejana:
        // a 4x de zoom el error de la constante es 4x el presupuesto.
        let error_px_de_la_constante = lejos / (cerca / 0.4);
        assert!((error_px_de_la_constante - 1.6).abs() < 1e-12);
    }

    #[test]
    fn aabb_vacia_es_neutro_de_union() {
        let a = Aabb::new(DVec3::ZERO, DVec3::ONE);
        assert!(Aabb::EMPTY.is_empty());
        assert_eq!(Aabb::EMPTY.union(a), a);
        assert_eq!(a.union(Aabb::EMPTY), a);
        assert_eq!(Aabb::EMPTY.union(Aabb::EMPTY), Aabb::EMPTY);
        assert_eq!(Aabb::EMPTY.size(), DVec3::ZERO);
    }

    #[test]
    fn aabb_transformada_contiene_los_puntos_transformados() {
        let a = Aabb::new(DVec3::new(-1.0, -2.0, -3.0), DVec3::new(4.0, 5.0, 6.0));
        let m = DAffine3::from_rotation_z(0.7) * DAffine3::from_translation(DVec3::new(10.0, 0.0, 0.0));
        let t = a.transformed(&m);
        for c in a.corners() {
            assert!(t.contains(m.transform_point3(c)));
        }
        assert!(t.diagonal() >= a.diagonal() - 1e-9);
    }

    #[test]
    fn transform_compone_en_el_orden_correcto() {
        let padre = Transform::from_translation(DVec3::new(10.0, 0.0, 0.0));
        let hijo = Transform::from_translation(DVec3::new(0.0, 5.0, 0.0));
        let m = hijo.then(&padre);
        let p = m.transform_point3(DVec3::ZERO);
        assert!((p - DVec3::new(10.0, 5.0, 0.0)).length() < tol::CONFUSION_MM);
    }

    #[test]
    fn arriba_es_z() {
        assert_eq!(UP, DVec3::Z);
    }
}
