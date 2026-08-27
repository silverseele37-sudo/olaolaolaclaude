//! Reproductor de escenas .forge sin editor.

use forge_doc::{Geometry, Snapshot, Visible};
use forge_math::Aabb;
use forge_render_api::{Camera, Light, RenderTarget, Renderer, SceneView};
use forge_render_cpu::{MapaDeMallas, CpuMesh, TablaDeMateriales, SoftwareRenderer};
use forge_store::BlobStore;
use std::error::Error;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Convierte un documento snapshot a una escena renderizable.
/// Esta conversión no conoce nada del editor: solo forge-doc y forge-render-api.
pub struct SceneConverter {
    malla_provider: MapaDeMallas,
    materiales: TablaDeMateriales,
}

impl SceneConverter {
    /// Crea un convertidor vacío.
    pub fn new() -> Self {
        SceneConverter {
            malla_provider: MapaDeMallas::nuevo(),
            materiales: TablaDeMateriales::default(),
        }
    }

    /// Carga geometría de malla del snapshot.
    pub fn cargar_geometria(&mut self, snap: &Snapshot, blobs: &dyn BlobStore) -> Result<()> {
        for (entity, geometry) in snap.iter::<Geometry>() {
            let visible = snap
                .get::<Visible>(entity)
                .map(|v| v.0)
                .unwrap_or(true);

            if !visible {
                continue;
            }

            match geometry.0 {
                forge_doc::GeometryPayload::Mesh(hash) => {
                    // Intenta cargar la malla del blob store
                    if let Some(_malla_bytes) = blobs.get(hash)? {
                        // Intenta parsear como glTF/GLB
                        // Por ahora, usamos una malla mínima de prueba
                        // En producción, esto llamaría a un parser GLB real
                        let malla = CpuMesh {
                            positions: vec![],
                            normals: vec![],
                            indices: vec![],
                        };
                        if malla.es_valida() {
                            self.malla_provider.insertar(malla);
                        }
                    }
                }
                // Otros tipos de geometría (exacta) se ignoran por ahora
                _ => {}
            }
        }
        Ok(())
    }

    /// Crea un renderizador listo para usar con snapshots.
    pub fn renderizador(self) -> SoftwareRenderer<MapaDeMallas> {
        SoftwareRenderer::nueva(self.malla_provider, self.materiales)
    }
}

/// Estadísticas de una escena.
#[derive(Clone, Debug, Default)]
pub struct SceneStats {
    pub entidades: usize,
    pub instancias: usize,
    pub triangulos: u64,
    pub bounding_box: Option<Aabb>,
}

/// Analiza el snapshot sin renderizar.
pub fn calcular_estadisticas(snap: &Snapshot, blobs: &dyn BlobStore) -> Result<SceneStats> {
    let mut stats = SceneStats {
        entidades: snap.entity_count(),
        ..Default::default()
    };

    let _bbox: Option<Aabb> = None;

    for (entity, geometry) in snap.iter::<Geometry>() {
        let visible = snap
            .get::<Visible>(entity)
            .map(|v| v.0)
            .unwrap_or(true);

        if !visible {
            continue;
        }

        match geometry.0 {
            forge_doc::GeometryPayload::Mesh(hash) => {
                if let Some(_malla_bytes) = blobs.get(hash)? {
                    // Contar instancia (una malla = una instancia en este contexto)
                    stats.instancias += 1;
                    // Aquí iría el conteo de triángulos real si parseamos la malla
                }
            }
            _ => {}
        }
    }

    stats.bounding_box = _bbox;
    Ok(stats)
}

/// Renderiza un snapshot a bytes RGBA.
pub fn renderizar(
    snap: &Snapshot,
    blobs: &dyn BlobStore,
    size: (u32, u32),
) -> Result<Vec<u8>> {
    let mut converter = SceneConverter::new();
    converter.cargar_geometria(snap, blobs)?;

    let mut renderer = converter.renderizador();

    // Crear instancias vacías por ahora (sin mallas cargadas)
    let instances = Vec::new();
    let lights = vec![
        Light::Directional {
            direction: forge_math::DVec3::new(-1.0, -1.0, 1.0).normalize(),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        },
    ];

    let view = SceneView {
        camera: Camera::default(),
        instances: &instances,
        lights: &lights,
        environment: None,
        exposure: None,
    };

    let target = RenderTarget {
        width: size.0,
        height: size.1,
        samples: 1,
    };

    Ok(renderer.render_offscreen(&view, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_store::MemoryBlobStore;

    #[test]
    fn test_escena_vacia() {
        let snap = forge_doc::Document::new().snapshot();
        let blobs = MemoryBlobStore::new();
        let stats = calcular_estadisticas(&snap, &blobs).unwrap();
        assert_eq!(stats.entidades, 0);
        assert_eq!(stats.instancias, 0);
    }

    #[test]
    fn test_renderizar_determinista() {
        let snap = forge_doc::Document::new().snapshot();
        let blobs = MemoryBlobStore::new();

        let bytes1 = renderizar(&snap, &blobs, (100, 100)).unwrap();
        let bytes2 = renderizar(&snap, &blobs, (100, 100)).unwrap();

        assert_eq!(bytes1, bytes2, "renders no son deterministas");
    }
}
