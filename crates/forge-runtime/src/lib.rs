//! Reproductor de escenas `.forge` sin editor.
//!
//! Carga un documento, decodifica sus blobs de malla y los dibuja. No conoce
//! `forge-ui` ni ninguna implementación concreta de renderer: extrae las
//! instancias con `forge-escena` —el mismo camino que usa el editor— y las pasa
//! por el contrato `forge-render-api`.
//!
//! # Qué hay dentro de un blob de malla
//!
//! Un GLB escrito con [`GltfOptions::crudo()`]: milímetros y Z arriba, las
//! unidades y los ejes de FORGE. Está en la especificación del formato
//! (`docs/formato/README.md`, §4.1) porque incumple glTF a propósito y leerlo
//! con las opciones por defecto daría los ejes permutados y la escala mil veces
//! mayor, en silencio.

use std::collections::HashMap;

use forge_doc::{Geometry, GeometryPayload, Snapshot, Visible};
use forge_escena::{caja_de_conjunto, extraer_instancias, ResolutorDeMallas};
use forge_interop::gltf::{read_glb, GltfOptions};
use forge_math::Aabb;
use forge_render_api::{Camera, DrawInstance, Light, RenderTarget, Renderer, SceneView};
use forge_render_cpu::{CpuMesh, MapaDeMallas, SoftwareRenderer, TablaDeMateriales};
use forge_store::{BlobHash, BlobStore};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("el almacen de blobs fallo: {0}")]
    Store(#[from] forge_store::StoreError),
    #[error(
        "el documento referencia el blob de malla {hash} y no esta en el almacen. Un `.forge` \
         guardado sin blobs solo abre donde el almacen ya los tiene."
    )]
    BlobAusente { hash: BlobHash },
    #[error(
        "el blob de malla {hash} no se pudo decodificar como GLB: {detalle}. Se esperaba un GLB \
         escrito con las unidades de FORGE (ver docs/formato/README.md §4.1)."
    )]
    MallaIlegible { hash: BlobHash, detalle: String },
    #[error("la malla del blob {hash} es incoherente: {detalle}")]
    MallaInvalida { hash: BlobHash, detalle: String },
    #[error("E/S en {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no se pudo abrir el documento: {0}")]
    Documento(String),
}

impl RuntimeError {
    pub fn io(p: impl Into<std::path::PathBuf>, e: std::io::Error) -> Self {
        RuntimeError::Io {
            path: p.into(),
            source: e,
        }
    }
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Convierte un snapshot del documento en una escena dibujable.
///
/// Solo conoce `forge-doc`, `forge-interop` y los contratos de render.
#[derive(Default)]
pub struct SceneConverter {
    mallas: MapaDeMallas,
    materiales: TablaDeMateriales,
    /// Caja local de cada blob ya cargado, para `DrawInstance::bounds`.
    cajas: HashMap<BlobHash, Aabb>,
    /// Triángulos de cada blob, para las estadísticas.
    triangulos: HashMap<BlobHash, u64>,
}

impl SceneConverter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodifica todos los blobs de malla que el documento referencia.
    ///
    /// Un blob que falte o que no se pueda decodificar es un **error**, no un
    /// objeto que se salta. La versión anterior de este archivo se los saltaba
    /// en silencio y devolvía `Ok`: el reproductor abría cualquier documento,
    /// dibujaba una imagen vacía y decía que todo había ido bien.
    pub fn cargar_geometria(&mut self, snap: &Snapshot, blobs: &dyn BlobStore) -> Result<()> {
        for (e, g) in snap.iter::<Geometry>() {
            let GeometryPayload::Mesh(hash) = g.0 else {
                // El dominio exacto (B-Rep, sketch) no tiene triángulos que
                // dibujar. Hace falta teselarlo antes, y eso es trabajo del
                // kernel, no del reproductor. Igual que en `forge-escena`.
                continue;
            };
            if !snap.get::<Visible>(e).map(|v| v.0).unwrap_or(true) {
                continue;
            }
            if self.cajas.contains_key(&hash) {
                // Instancias del mismo blob: se decodifica una vez.
                continue;
            }

            let bytes = blobs.get(hash)?.ok_or(RuntimeError::BlobAusente { hash })?;
            let soup = read_glb(&bytes, GltfOptions::crudo()).map_err(|e| {
                RuntimeError::MallaIlegible {
                    hash,
                    detalle: e.to_string(),
                }
            })?;

            let caja = Aabb::from_points(soup.positions.iter().copied());
            let triangulos = soup.triangle_count() as u64;
            let malla = CpuMesh::nueva(soup.positions, soup.normals, soup.indices);
            if !malla.es_valida() {
                return Err(RuntimeError::MallaInvalida {
                    hash,
                    detalle: "indices fuera de rango o normales descuadradas".into(),
                });
            }

            // Indexado por el hash **del documento**, que es el que llevará
            // `DrawInstance::mesh`. Con `insertar` se indexaría por el hash del
            // contenido decodificado y el renderer no encontraría ninguna.
            self.mallas.insertar_con_hash(hash, malla);
            self.cajas.insert(hash, caja);
            self.triangulos.insert(hash, triangulos);
        }
        Ok(())
    }

    /// Las instancias dibujables, por el mismo camino que el editor.
    pub fn instancias(&self, snap: &Snapshot) -> Vec<DrawInstance> {
        extraer_instancias(snap, &[], self)
    }

    /// Un renderizador por software listo para dibujar.
    pub fn renderizador(self) -> SoftwareRenderer<MapaDeMallas> {
        SoftwareRenderer::nueva(self.mallas, self.materiales)
    }
}

impl ResolutorDeMallas for SceneConverter {
    fn caja_local(&self, hash: BlobHash) -> Option<Aabb> {
        self.cajas.get(&hash).copied()
    }
}

/// Estadísticas de una escena.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneStats {
    pub entidades: usize,
    pub instancias: usize,
    pub triangulos: u64,
    /// Caja de mundo de todo lo dibujable. `Aabb::EMPTY` si no hay nada.
    pub caja: Aabb,
}

/// Analiza el documento sin dibujarlo.
///
/// Decodifica las mallas igual que el render, así que cuenta triángulos de
/// verdad y devuelve una caja de verdad. Antes devolvía siempre cero y `None`.
pub fn calcular_estadisticas(snap: &Snapshot, blobs: &dyn BlobStore) -> Result<SceneStats> {
    let mut conv = SceneConverter::new();
    conv.cargar_geometria(snap, blobs)?;
    let inst = conv.instancias(snap);
    Ok(SceneStats {
        entidades: snap.entity_count(),
        instancias: inst.len(),
        triangulos: inst
            .iter()
            .map(|i| conv.triangulos.get(&i.mesh).copied().unwrap_or(0))
            .sum(),
        caja: caja_de_conjunto(&inst),
    })
}

/// Encuadra la cámara sobre una caja de mundo.
///
/// Mira al centro desde una diagonal, a la distancia a la que la esfera que
/// envuelve la caja llena el campo de visión vertical. Sin esto, una escena que
/// no esté en el origen sale fuera de cuadro y el reproductor parece roto.
pub fn encuadrar(caja: Aabb) -> Camera {
    let mut c = Camera::default();
    if caja.is_empty() {
        return c;
    }
    let centro = caja.center();
    let radio = (caja.max - caja.min).length() * 0.5;
    let dist = (radio / (c.fov_y_rad * 0.5).sin()).max(1e-6);
    let dir = forge_math::DVec3::new(1.0, -1.0, 0.6).normalize();
    c.target = centro;
    c.eye = centro + dir * dist;
    c.near_mm = (dist - radio).max(dist * 1e-4);
    c.far_mm = dist + radio * 2.0;
    c
}

/// Dibuja un snapshot y devuelve los bytes RGBA.
pub fn renderizar(snap: &Snapshot, blobs: &dyn BlobStore, size: (u32, u32)) -> Result<Vec<u8>> {
    let mut conv = SceneConverter::new();
    conv.cargar_geometria(snap, blobs)?;
    let instancias = conv.instancias(snap);
    let camara = encuadrar(caja_de_conjunto(&instancias));

    // Dos luces, no una. Con una sola direccional y sin entorno, las caras que
    // no miran a la luz salen en negro sobre un fondo negro: la primera version
    // de esta demo parecia dibujar cajas huecas cuando en realidad la geometria
    // estaba bien y lo que faltaba era relleno. Un reproductor cuya imagen
    // parece rota no sirve de reproductor, asi que la de relleno viene puesta.
    let luces = vec![
        Light::Directional {
            direction: forge_math::DVec3::new(-1.0, -1.0, -1.0).normalize(),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        },
        Light::Directional {
            direction: forge_math::DVec3::new(1.0, 1.0, 1.0).normalize(),
            color: [1.0, 1.0, 1.0],
            intensity: 0.35,
        },
    ];
    let view = SceneView {
        camera: camara,
        instances: &instancias,
        lights: &luces,
        environment: None,
        exposure: None,
    };
    let target = RenderTarget {
        width: size.0,
        height: size.1,
        samples: 1,
    };
    Ok(conv.renderizador().render_offscreen(&view, target))
}
