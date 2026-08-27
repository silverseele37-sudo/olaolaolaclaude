//! Material metallic-roughness del lado GPU, y la tabla que traduce `MaterialId`.
//!
//! `forge-render-api` solo transporta un [`MaterialId`] en cada `DrawInstance`
//! (ver la nota de `SceneView`: el renderer no conoce el documento). Igual que
//! `forge-render-cpu` resuelve eso con su propia tabla de materiales en vez de
//! esperarla en la vista, aquí se hace lo mismo: [`TablaDeMateriales`] vive en
//! el renderer, la puebla quien construye la escena, y un id sin registrar cae
//! al material por defecto en vez de abortar el frame.
//!
//! Es deliberadamente pequeño (sin texturas, sin generador de grafos de
//! `forge-material-api`): este crate implementa el contrato de render, no el
//! de autoría de materiales. Cuando haga falta un material generado desde un
//! grafo, el punto de extensión es reemplazar cómo se puebla esta tabla, no
//! esta tabla misma.

use forge_render_api::MaterialId;
use std::collections::BTreeMap;

/// Material metallic-roughness, en el mismo espacio (radiancia lineal
/// Rec.709) que usa `crate::ibl` y el shader `pbr.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub base_color: [f32; 3],
    /// 0 = espejo perfecto, 1 = completamente rugoso.
    pub roughness: f32,
    pub metallic: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            base_color: [0.8, 0.8, 0.8],
            roughness: 0.5,
            metallic: 0.0,
        }
    }
}

/// Tabla de materiales por id. `BTreeMap` y no `HashMap` por el mismo motivo
/// que en `forge-render-cpu`: determinismo de recorrido si algún día hace
/// falta iterarla, aunque hoy solo se consulte por clave.
#[derive(Clone, Debug, Default)]
pub struct TablaDeMateriales {
    tabla: BTreeMap<u64, Material>,
    por_defecto: Material,
}

impl TablaDeMateriales {
    pub fn nueva() -> Self {
        Self::default()
    }

    pub fn con_defecto(m: Material) -> Self {
        TablaDeMateriales {
            tabla: BTreeMap::new(),
            por_defecto: m,
        }
    }

    pub fn insertar(&mut self, id: MaterialId, m: Material) {
        self.tabla.insert(id.0, m);
    }

    /// Un id desconocido devuelve el material por defecto: dibujar en gris es
    /// una respuesta mucho más útil que abortar el frame.
    pub fn material(&self, id: MaterialId) -> Material {
        self.tabla.get(&id.0).copied().unwrap_or(self.por_defecto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_desconocido_cae_al_material_por_defecto() {
        let t = TablaDeMateriales::nueva();
        assert_eq!(t.material(MaterialId(42)), Material::default());
    }

    #[test]
    fn id_registrado_devuelve_lo_insertado() {
        let mut t = TablaDeMateriales::nueva();
        let rojo = Material {
            base_color: [1.0, 0.0, 0.0],
            roughness: 0.3,
            metallic: 1.0,
        };
        t.insertar(MaterialId(7), rojo);
        assert_eq!(t.material(MaterialId(7)), rojo);
        assert_eq!(t.material(MaterialId(8)), Material::default());
    }
}
