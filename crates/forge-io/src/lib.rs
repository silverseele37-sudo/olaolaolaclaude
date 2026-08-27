//! El contenedor `.forge`.
//!
//! Un documento es un ZIP con esta disposición:
//!
//! ```text
//! documento.forge
//! ├── manifest.json          version de formato, unidades, ejes, tolerancias
//! ├── document.cbor          el grafo: entidades y almacenes de componentes
//! ├── document.json          (opcional) el mismo grafo legible, para diff
//! ├── refs/head              version vigente
//! ├── refs/history           etiquetas del historial, para mostrar
//! └── blobs/<aa>/<hash>      payloads inmutables direccionados por contenido
//! ```
//!
//! ZIP y no un binario propio porque un formato abierto que nadie puede
//! inspeccionar no es abierto: con `unzip` se ve la estructura entera, y
//! `document.json` produce diffs legibles en control de versiones. La misma
//! disposición sin comprimir es la forma explotada.
//!
//! **Lo que el archivo NO guarda:** la pila de undo. El historial que persiste
//! son las etiquetas, para poder mostrarlas; el estado que persiste es el de la
//! versión vigente. Es coherente con ADR-0004: el undo es *meta*, no datos del
//! documento.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forge_doc::{ComponentRegistryRef, Document, EntityId, Snapshot, VersionId};
use forge_store::{BlobHash, BlobStore};
use serde::{Deserialize, Serialize};

pub mod atomic;

/// Versión del formato en disco. Cada incremento trae su función de migración
/// y su test. Nunca se lee un documento de versión desconocida "a ver si va".
pub const FORMAT_VERSION: u32 = 1;

pub const MAGIC: &str = "forge";

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("E/S en {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("el archivo no es un documento FORGE: {0}")]
    NoEsForge(String),
    #[error(
        "version de formato {encontrada} desconocida (esta build lee hasta la {soportada}). \
         Actualiza FORGE para abrir este documento."
    )]
    VersionFutura { encontrada: u32, soportada: u32 },
    #[error("documento corrupto: {0}")]
    Corrupto(String),
    #[error(transparent)]
    Doc(#[from] forge_doc::DocError),
    #[error(transparent)]
    Store(#[from] forge_store::StoreError),
    #[error("blob {0} referenciado por el documento pero ausente del archivo y del almacen")]
    BlobAusente(BlobHash),
}

impl IoError {
    pub fn at(p: impl Into<PathBuf>, e: std::io::Error) -> Self {
        IoError::Io {
            path: p.into(),
            source: e,
        }
    }
}

pub type Result<T> = std::result::Result<T, IoError>;

// ---------------------------------------------------------------------------
// Manifiesto
// ---------------------------------------------------------------------------

/// Va primero y sin comprimir, para poder identificar un archivo leyendo sus
/// primeros kilobytes sin descomprimir nada.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    /// Unidad interna. Siempre `mm` en v1; el campo existe para que un lector
    /// futuro no tenga que adivinarlo.
    pub units: String,
    /// Eje vertical del documento. Siempre `Z` (convención CAD/STEP).
    pub up_axis: String,
    pub tolerance_confusion_mm: f64,
    pub generator: String,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            format: MAGIC.into(),
            format_version: FORMAT_VERSION,
            units: "mm".into(),
            up_axis: "Z".into(),
            tolerance_confusion_mm: forge_math_confusion(),
            generator: format!("forge {}", env!("CARGO_PKG_VERSION")),
        }
    }
}

fn forge_math_confusion() -> f64 {
    1e-7
}

impl Manifest {
    fn validar(&self) -> Result<()> {
        if self.format != MAGIC {
            return Err(IoError::NoEsForge(format!(
                "campo `format` = {:?}",
                self.format
            )));
        }
        if self.format_version > FORMAT_VERSION {
            return Err(IoError::VersionFutura {
                encontrada: self.format_version,
                soportada: FORMAT_VERSION,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cuerpo
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct DocumentFile {
    entities: Vec<EntityId>,
    stores: Vec<StoreEntry>,
}

#[derive(Serialize, Deserialize)]
struct StoreEntry {
    name: String,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Default)]
struct Refs {
    head: u64,
    head_label: String,
    history: Vec<(u64, String)>,
}

// ---------------------------------------------------------------------------
// Guardar
// ---------------------------------------------------------------------------

pub struct SaveOptions {
    /// Escribe además `document.json`, legible y diffeable. Cuesta tamaño.
    pub include_json: bool,
    /// Empaqueta los blobs referenciados. Apagarlo produce un archivo pequeño
    /// que solo abre donde el almacén ya tiene los blobs.
    pub include_blobs: bool,
    /// Etiquetas del historial, solo para mostrar.
    pub history: Vec<(VersionId, String)>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions {
            include_json: true,
            include_blobs: true,
            history: Vec::new(),
        }
    }
}

pub fn save(
    path: impl AsRef<Path>,
    snap: &Snapshot,
    blobs: &dyn BlobStore,
    opts: &SaveOptions,
) -> Result<()> {
    let path = path.as_ref();
    atomic::write_atomic(path, |file| escribir_zip(file, snap, blobs, opts))
}

fn escribir_zip<W: Write + Seek>(
    w: &mut W,
    snap: &Snapshot,
    blobs: &dyn BlobStore,
    opts: &SaveOptions,
) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let mut zw = zip::ZipWriter::new(w);
    let guardado = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let comprimido =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let corrupto = |e: std::io::Error| IoError::Corrupto(e.to_string());

    // 1. manifiesto, primero y sin comprimir
    let manifest = Manifest::default();
    zw.start_file("manifest.json", guardado)
        .map_err(|e| IoError::Corrupto(e.to_string()))?;
    zw.write_all(
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| IoError::Corrupto(e.to_string()))?
            .as_bytes(),
    )
    .map_err(corrupto)?;

    // 2. el grafo
    let mut stores = Vec::new();
    for name in snap.store_names() {
        let data = snap
            .encode_store(name)
            .expect("store_names devolvio un nombre que no existe")?;
        stores.push(StoreEntry {
            name: name.to_string(),
            data,
        });
    }
    let df = DocumentFile {
        entities: snap.entity_ids(),
        stores,
    };

    let mut cbor = Vec::new();
    ciborium::into_writer(&df, &mut cbor).map_err(|e| IoError::Corrupto(e.to_string()))?;
    zw.start_file("document.cbor", comprimido)
        .map_err(|e| IoError::Corrupto(e.to_string()))?;
    zw.write_all(&cbor).map_err(corrupto)?;

    if opts.include_json {
        zw.start_file("document.json", comprimido)
            .map_err(|e| IoError::Corrupto(e.to_string()))?;
        zw.write_all(vista_legible(&df).as_bytes())
            .map_err(corrupto)?;
    }

    // 3. refs
    let refs = Refs {
        head: snap.version().0,
        head_label: opts
            .history
            .last()
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| "guardado".into()),
        history: opts.history.iter().map(|(v, l)| (v.0, l.clone())).collect(),
    };
    zw.start_file("refs/history", comprimido)
        .map_err(|e| IoError::Corrupto(e.to_string()))?;
    zw.write_all(
        serde_json::to_string_pretty(&refs)
            .map_err(|e| IoError::Corrupto(e.to_string()))?
            .as_bytes(),
    )
    .map_err(corrupto)?;

    // 4. blobs referenciados
    if opts.include_blobs {
        for h in snap.referenced_blobs() {
            let bytes = blobs.get(h)?.ok_or(IoError::BlobAusente(h))?;
            zw.start_file(format!("blobs/{}/{}", h.shard(), h.to_hex()), guardado)
                .map_err(|e| IoError::Corrupto(e.to_string()))?;
            zw.write_all(&bytes).map_err(corrupto)?;
        }
    }

    zw.finish().map_err(|e| IoError::Corrupto(e.to_string()))?;
    Ok(())
}

/// Mejor esfuerzo: convierte el CBOR a JSON para poder leerlo y diffearlo.
/// No es normativo — `document.cbor` es la fuente de verdad.
fn vista_legible(df: &DocumentFile) -> String {
    #[derive(Serialize)]
    struct Vista<'a> {
        entities: Vec<String>,
        stores: Vec<VistaStore<'a>>,
    }
    #[derive(Serialize)]
    struct VistaStore<'a> {
        name: &'a str,
        entries: serde_json::Value,
    }
    let vista = Vista {
        entities: df.entities.iter().map(|e| e.to_string()).collect(),
        stores: df
            .stores
            .iter()
            .map(|s| VistaStore {
                name: &s.name,
                entries: ciborium::from_reader::<ciborium::Value, _>(&s.data[..])
                    .ok()
                    .and_then(|v| serde_json::to_value(v).ok())
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&vista).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Cargar
// ---------------------------------------------------------------------------

/// Lee un `.forge`. Los blobs que traiga el archivo se depositan en `blobs`.
pub fn load(
    path: impl AsRef<Path>,
    registry: ComponentRegistryRef,
    blobs: &dyn BlobStore,
) -> Result<Document> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|e| IoError::at(path, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| IoError::NoEsForge(e.to_string()))?;

    // 1. manifiesto
    let manifest: Manifest = {
        let mut f = zip
            .by_name("manifest.json")
            .map_err(|_| IoError::NoEsForge("falta manifest.json".into()))?;
        let mut s = String::new();
        f.read_to_string(&mut s).map_err(|e| IoError::at(path, e))?;
        serde_json::from_str(&s).map_err(|e| IoError::NoEsForge(e.to_string()))?
    };
    manifest.validar()?;
    migrar(&manifest)?;

    // 2. blobs primero: el grafo puede referenciarlos.
    let nombres: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    for n in nombres.iter().filter(|n| n.starts_with("blobs/")) {
        let mut f = zip
            .by_name(n)
            .map_err(|e| IoError::Corrupto(e.to_string()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| IoError::at(path, e))?;
        let esperado = n.rsplit('/').next().unwrap_or_default();
        let h = blobs.put(&buf)?;
        // El nombre del blob ES su hash: si no coincide, el archivo está
        // adulterado y hay que decirlo, no cargarlo igual.
        if h.to_hex() != esperado {
            return Err(IoError::Corrupto(format!(
                "el blob {esperado} no reproduce su hash (contenido -> {})",
                h.to_hex()
            )));
        }
    }

    // 3. el grafo
    let df: DocumentFile = {
        let mut f = zip
            .by_name("document.cbor")
            .map_err(|_| IoError::NoEsForge("falta document.cbor".into()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| IoError::at(path, e))?;
        ciborium::from_reader(&buf[..]).map_err(|e| IoError::Corrupto(e.to_string()))?
    };

    let etiqueta = zip
        .by_name("refs/history")
        .ok()
        .and_then(|mut f| {
            let mut s = String::new();
            f.read_to_string(&mut s).ok()?;
            serde_json::from_str::<Refs>(&s).ok()
        })
        .map(|r| r.head_label)
        .unwrap_or_else(|| "documento cargado".into());

    let stores = df.stores.into_iter().map(|s| (s.name, s.data)).collect();
    let doc = Document::from_parts(registry, df.entities, stores, etiqueta)?;

    // 4. comprobar que todo lo referenciado está presente
    let snap = doc.snapshot();
    for h in snap.referenced_blobs() {
        if !blobs.has(h)? {
            return Err(IoError::BlobAusente(h));
        }
    }
    Ok(doc)
}

/// Lee solo el manifiesto. Barato: sirve para listar una carpeta de documentos
/// sin abrirlos.
pub fn read_manifest(path: impl AsRef<Path>) -> Result<Manifest> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|e| IoError::at(path, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| IoError::NoEsForge(e.to_string()))?;
    let mut f = zip
        .by_name("manifest.json")
        .map_err(|_| IoError::NoEsForge("falta manifest.json".into()))?;
    let mut s = String::new();
    f.read_to_string(&mut s).map_err(|e| IoError::at(path, e))?;
    let m: Manifest = serde_json::from_str(&s).map_err(|e| IoError::NoEsForge(e.to_string()))?;
    m.validar()?;
    Ok(m)
}

/// Migraciones dirigidas. Hoy no hay ninguna porque solo existe la versión 1;
/// la función existe para que añadir la primera sea rellenar un `match` y no
/// rediseñar la carga.
fn migrar(m: &Manifest) -> Result<()> {
    match m.format_version {
        1 => Ok(()),
        v => Err(IoError::VersionFutura {
            encontrada: v,
            soportada: FORMAT_VERSION,
        }),
    }
}

/// Re-export para que quien use `forge-io` no tenga que importar `forge-doc`
/// solo para construir el registro.
pub fn registro_por_defecto() -> ComponentRegistryRef {
    Arc::new(forge_doc::component::ComponentRegistry::new())
}
