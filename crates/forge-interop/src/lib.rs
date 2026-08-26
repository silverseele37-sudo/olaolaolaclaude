//! Importación y exportación.
//!
//! # La conversión de ejes vive aquí, y solo aquí
//!
//! El documento de FORGE es **Z arriba**, la convención de CAD y de STEP. glTF
//! es **Y arriba por especificación**. Casi toda la bibliografía de gráficos es
//! Y-up también.
//!
//! La regla que evita que eso contamine el sistema: **la conversión ocurre en la
//! frontera de interoperabilidad y en ningún otro sitio.** El núcleo, los
//! pilares y el render trabajan siempre en Z-up. Si esta regla se rompe, el
//! resultado es geometría rotada 90° y espejada en un sitio y no en otro — un
//! bug que *casi* se ve bien, y por eso caro de diagnosticar. `cadviz` pisó esa
//! mina con el mapeo equirectangular y la documentó.
//!
//! Por eso [`z_up_to_y_up`] y su inversa son funciones públicas con tests de ida
//! y vuelta: la conversión tiene que ser visible y verificable, no un `swap` de
//! componentes escondido dentro de un exportador.

use forge_math::{Aabb, DVec2, DVec3};

pub mod gltf;
pub mod obj;

#[derive(Debug, thiserror::Error)]
pub enum InteropError {
    #[error("E/S en {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("el archivo esta mal formado en la linea {line}: {detail}")]
    Malformed { line: usize, detail: String },
    #[error("malla invalida: {0}")]
    InvalidMesh(String),
    #[error("no soportado: {0}")]
    Unsupported(&'static str),
    #[error("json: {0}")]
    Json(String),
}

pub type Result<T> = std::result::Result<T, InteropError>;

/// Malla neutra de intercambio.
///
/// Deliberadamente tonta: posiciones, normales, UV e índices, y nada más. Es el
/// mínimo común denominador de los formatos externos, y mantenerla así es lo que
/// impide que `forge-interop` acabe conociendo el modelo de datos de los pilares.
///
/// **Siempre en Z-up y en milímetros**, como todo el interior de FORGE. La
/// conversión a lo que quiera el formato de destino la hace el exportador.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriangleSoup {
    pub name: String,
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    /// Triples de índices a `positions`.
    pub indices: Vec<u32>,
}

impl TriangleSoup {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bbox(&self) -> Aabb {
        Aabb::from_points(self.positions.iter().copied())
    }

    /// Invariantes estructurales. Barato y evita exportar basura que el
    /// programa de destino rechace con un error incomprensible.
    pub fn validate(&self) -> Result<()> {
        if self.indices.len() % 3 != 0 {
            return Err(InteropError::InvalidMesh(format!(
                "indices no multiplo de 3: {}",
                self.indices.len()
            )));
        }
        if self.positions.is_empty() {
            return Err(InteropError::InvalidMesh("sin vertices".into()));
        }
        let n = self.positions.len() as u32;
        if let Some(&mal) = self.indices.iter().find(|&&i| i >= n) {
            return Err(InteropError::InvalidMesh(format!(
                "indice {mal} fuera de rango ({n} vertices)"
            )));
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(InteropError::InvalidMesh("normales descuadradas".into()));
        }
        if !self.uvs.is_empty() && self.uvs.len() != self.positions.len() {
            return Err(InteropError::InvalidMesh("UVs descuadradas".into()));
        }
        Ok(())
    }

    /// Desde el teselado del kernel. Pierde la procedencia a propósito: un
    /// formato externo no tiene dónde guardarla, y fingir que sí la conserva
    /// sería peor que perderla explícitamente.
    pub fn from_tessellation(t: &forge_kernel_api::Tessellation, name: impl Into<String>) -> Self {
        TriangleSoup {
            name: name.into(),
            positions: t.positions.clone(),
            normals: t.normals.clone(),
            uvs: Vec::new(),
            indices: t.indices.clone(),
        }
    }
}

/// Z-up (FORGE) → Y-up (glTF).
///
/// Rotación de −90° alrededor de X: `(x, y, z) → (x, z, −y)`.
/// Preserva la lateralidad, que es lo que importa: una conversión que espeje
/// invierte el sentido de las caras y deja el modelo del revés.
#[inline]
pub fn z_up_to_y_up(v: DVec3) -> DVec3 {
    DVec3::new(v.x, v.z, -v.y)
}

/// Y-up (glTF) → Z-up (FORGE). Inversa exacta de [`z_up_to_y_up`].
#[inline]
pub fn y_up_to_z_up(v: DVec3) -> DVec3 {
    DVec3::new(v.x, -v.z, v.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Respuesta conocida: arriba en FORGE tiene que ser arriba en glTF.
    #[test]
    fn arriba_sigue_siendo_arriba_al_convertir() {
        assert_eq!(z_up_to_y_up(DVec3::Z), DVec3::Y, "el eje vertical no se preservo");
        assert_eq!(y_up_to_z_up(DVec3::Y), DVec3::Z);
        // el eje X no se toca: es el que comparten las dos convenciones
        assert_eq!(z_up_to_y_up(DVec3::X), DVec3::X);
    }

    #[test]
    fn la_conversion_de_ejes_es_una_ida_y_vuelta_exacta() {
        for v in [
            DVec3::X, DVec3::Y, DVec3::Z,
            DVec3::new(1.0, -2.0, 3.0),
            DVec3::new(-1e6, 1e-6, 0.0),
        ] {
            assert_eq!(y_up_to_z_up(z_up_to_y_up(v)), v, "no es inversa para {v:?}");
            assert_eq!(z_up_to_y_up(y_up_to_z_up(v)), v);
        }
    }

    /// La conversión **no** debe espejar: si lo hiciera, las caras saldrían del
    /// revés en todos los programas de destino. Se comprueba con el producto
    /// mixto, que cambia de signo bajo una reflexión y lo conserva bajo una
    /// rotación.
    #[test]
    fn la_conversion_es_una_rotacion_y_no_una_reflexion() {
        let (a, b, c) = (DVec3::X, DVec3::Y, DVec3::Z);
        let antes = a.cross(b).dot(c);
        let despues = z_up_to_y_up(a).cross(z_up_to_y_up(b)).dot(z_up_to_y_up(c));
        assert_eq!(antes, 1.0);
        assert_eq!(despues, 1.0, "la conversion espeja: las caras saldrian invertidas");
    }

    #[test]
    fn validate_detecta_mallas_rotas() {
        let buena = TriangleSoup {
            name: "t".into(),
            positions: vec![DVec3::ZERO, DVec3::X, DVec3::Y],
            normals: vec![DVec3::Z; 3],
            uvs: vec![],
            indices: vec![0, 1, 2],
        };
        assert!(buena.validate().is_ok());

        // control positivo: cada forma de estar rota se detecta
        let mut sin_vertices = buena.clone();
        sin_vertices.positions.clear();
        assert!(sin_vertices.validate().is_err());

        let mut indices_sueltos = buena.clone();
        indices_sueltos.indices = vec![0, 1];
        assert!(indices_sueltos.validate().is_err());

        let mut fuera_de_rango = buena.clone();
        fuera_de_rango.indices = vec![0, 1, 99];
        assert!(fuera_de_rango.validate().is_err());

        let mut normales_mal = buena.clone();
        normales_mal.normals = vec![DVec3::Z; 2];
        assert!(normales_mal.validate().is_err());
    }
}
