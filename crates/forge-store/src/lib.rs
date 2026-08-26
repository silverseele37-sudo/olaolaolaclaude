//! Almacén de blobs direccionado por contenido.
//!
//! Es la columna vertebral de FORGE, y conviene tenerlo presente al leer este
//! crate tan corto: el **mismo** mecanismo sirve a cuatro cosas que en otros
//! sistemas son cuatro subsistemas distintos.
//!
//! - **undo/redo** — dos versiones del documento que comparten una malla
//!   comparten el blob, sin copiar (ADR-0004).
//! - **versiones del documento** — el historial referencia blobs inmutables.
//! - **caché de evaluación** — la salida de un nodo se indexa por el hash de sus
//!   entradas y parámetros; si el hash coincide, no se recalcula.
//! - **deduplicación del almacén de activos** — importar la misma textura por dos
//!   rutas produce un solo blob, sin código extra (Pilar 4).
//!
//! Reglas invariantes:
//!
//! - Un blob es **inmutable** y su nombre **es** el hash de su contenido.
//! - Escribir el mismo contenido dos veces es un no-op observable.
//! - La escritura a disco es **atómica**: temporal en el mismo volumen y
//!   `rename`. Un corte de energía deja el estado anterior intacto, nunca uno a
//!   medias.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("E/S en {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("el blob {0} está corrupto: su contenido no reproduce su hash")]
    Corrupt(BlobHash),
    #[error("hash mal formado: {0}")]
    BadHash(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Identidad de un blob: BLAKE3 de su contenido.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobHash([u8; 32]);

impl BlobHash {
    /// Hash del contenido. Es la única forma de fabricar un `BlobHash` a partir
    /// de datos: no hay constructor que acepte un hash arbitrario junto a un
    /// contenido que no lo produce.
    pub fn of(bytes: &[u8]) -> Self {
        BlobHash(*blake3::hash(bytes).as_bytes())
    }

    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            fmt::Write::write_fmt(&mut s, format_args!("{b:02x}")).unwrap();
        }
        s
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        if s.len() != 64 {
            return Err(StoreError::BadHash(s.to_string()));
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
                .map_err(|_| StoreError::BadHash(s.to_string()))?;
        }
        Ok(BlobHash(out))
    }

    /// Prefijo de dos caracteres usado para repartir los blobs en subdirectorios.
    /// Sin esto, un almacén con cien mil activos deja cien mil entradas en un
    /// solo directorio, que es patológico en varios sistemas de archivos.
    pub fn shard(self) -> String {
        format!("{:02x}", self.0[0])
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Suficiente para leer un log sin que ocupe 64 caracteres.
        write!(f, "{}…", &self.to_hex()[..12])
    }
}

impl fmt::Debug for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobHash({})", self.to_hex())
    }
}

// En el archivo, un hash es su hexadecimal: legible en `document.json` y
// diffeable en control de versiones.
impl Serialize for BlobHash {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for BlobHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        BlobHash::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Contrato del almacén. Deliberadamente diminuto: todo lo que hace falta para
/// que undo, versiones, caché y activos compartan mecanismo.
pub trait BlobStore: Send + Sync {
    /// Idempotente. Devuelve el hash del contenido.
    fn put(&self, bytes: &[u8]) -> Result<BlobHash>;
    fn get(&self, h: BlobHash) -> Result<Option<Arc<[u8]>>>;
    fn has(&self, h: BlobHash) -> Result<bool>;
    /// Hashes presentes. Para empaquetar y para recolección.
    fn list(&self) -> Result<Vec<BlobHash>>;
}

/// Almacén en memoria. El de la sesión de trabajo y el de los tests.
#[derive(Default)]
pub struct MemoryBlobStore {
    map: RwLock<HashMap<BlobHash, Arc<[u8]>>>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.map.read().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Bytes totales almacenados, ya deduplicados.
    pub fn bytes(&self) -> usize {
        self.map.read().unwrap().values().map(|v| v.len()).sum()
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&self, bytes: &[u8]) -> Result<BlobHash> {
        let h = BlobHash::of(bytes);
        let mut m = self.map.write().unwrap();
        m.entry(h).or_insert_with(|| Arc::from(bytes));
        Ok(h)
    }
    fn get(&self, h: BlobHash) -> Result<Option<Arc<[u8]>>> {
        Ok(self.map.read().unwrap().get(&h).cloned())
    }
    fn has(&self, h: BlobHash) -> Result<bool> {
        Ok(self.map.read().unwrap().contains_key(&h))
    }
    fn list(&self) -> Result<Vec<BlobHash>> {
        let mut v: Vec<_> = self.map.read().unwrap().keys().copied().collect();
        v.sort();
        Ok(v)
    }
}

/// Almacén en disco: `<root>/<aa>/<hash>`.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| StoreError::Io { path: root.clone(), source: e })?;
        Ok(FsBlobStore { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_of(&self, h: BlobHash) -> PathBuf {
        self.root.join(h.shard()).join(h.to_hex())
    }

    /// Relee cada blob y comprueba que su contenido reproduce su nombre.
    /// Es lo que hace que el índice del almacén de activos sea reconstruible: si
    /// los blobs están sanos, todo lo demás se deriva.
    pub fn verify(&self) -> Result<Vec<BlobHash>> {
        let mut malos = Vec::new();
        for h in self.list()? {
            let p = self.path_of(h);
            let bytes = std::fs::read(&p).map_err(|e| StoreError::Io { path: p, source: e })?;
            if BlobHash::of(&bytes) != h {
                malos.push(h);
            }
        }
        Ok(malos)
    }
}

impl BlobStore for FsBlobStore {
    fn put(&self, bytes: &[u8]) -> Result<BlobHash> {
        let h = BlobHash::of(bytes);
        let dst = self.path_of(h);
        if dst.exists() {
            return Ok(h); // inmutable: escribir lo mismo otra vez es un no-op
        }
        let dir = dst.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| StoreError::Io { path: dir.into(), source: e })?;

        // Temporal en el MISMO directorio, para que el rename sea dentro del
        // mismo sistema de archivos y por tanto atómico.
        let tmp = dir.join(format!(".{}.tmp", h.to_hex()));
        std::fs::write(&tmp, bytes).map_err(|e| StoreError::Io { path: tmp.clone(), source: e })?;
        match std::fs::rename(&tmp, &dst) {
            Ok(()) => Ok(h),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(StoreError::Io { path: dst, source: e })
            }
        }
    }

    fn get(&self, h: BlobHash) -> Result<Option<Arc<[u8]>>> {
        let p = self.path_of(h);
        match std::fs::read(&p) {
            Ok(b) => Ok(Some(Arc::from(b.into_boxed_slice()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io { path: p, source: e }),
        }
    }

    fn has(&self, h: BlobHash) -> Result<bool> {
        Ok(self.path_of(h).exists())
    }

    fn list(&self) -> Result<Vec<BlobHash>> {
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(StoreError::Io { path: self.root.clone(), source: e }),
        };
        for shard in rd.flatten() {
            if !shard.path().is_dir() {
                continue;
            }
            for f in std::fs::read_dir(shard.path()).into_iter().flatten().flatten() {
                if let Some(name) = f.file_name().to_str() {
                    // Los temporales empiezan por punto y no son blobs.
                    if let Ok(h) = BlobHash::from_hex(name) {
                        out.push(h);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Respuesta conocida: BLAKE3 del string vacío. Si esto cambia, es que
    /// cambió el algoritmo de hash y el formato de archivo dejó de ser estable.
    #[test]
    fn hash_de_referencia() {
        assert_eq!(
            BlobHash::of(b"").to_hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn hex_ida_y_vuelta() {
        let h = BlobHash::of(b"forge");
        assert_eq!(BlobHash::from_hex(&h.to_hex()).unwrap(), h);
        assert!(BlobHash::from_hex("corto").is_err());
        assert!(BlobHash::from_hex(&"z".repeat(64)).is_err());
    }

    fn suite_de_contrato(s: &dyn BlobStore) {
        let a = s.put(b"alfa").unwrap();
        let b = s.put(b"beta").unwrap();
        assert_ne!(a, b);
        assert_eq!(&*s.get(a).unwrap().unwrap(), b"alfa");
        assert!(s.has(b).unwrap());
        assert!(!s.has(BlobHash::of(b"jamas escrito")).unwrap());
        assert!(s.get(BlobHash::of(b"jamas escrito")).unwrap().is_none());
        assert_eq!(s.list().unwrap(), { let mut v = vec![a, b]; v.sort(); v });
    }

    #[test]
    fn contrato_en_memoria() {
        suite_de_contrato(&MemoryBlobStore::new());
    }

    #[test]
    fn contrato_en_disco() {
        let d = tempfile::tempdir().unwrap();
        suite_de_contrato(&FsBlobStore::open(d.path()).unwrap());
    }

    /// La deduplicación no es una función: es una consecuencia de que el nombre
    /// sea el contenido. Este test lo fija como contrato.
    #[test]
    fn deduplicacion_por_construccion() {
        let s = MemoryBlobStore::new();
        let datos = vec![7u8; 4096];
        let h1 = s.put(&datos).unwrap();
        let h2 = s.put(&datos).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(s.len(), 1, "dos put del mismo contenido son un blob");
        assert_eq!(s.bytes(), 4096, "y ocupan una sola vez");
    }

    #[test]
    fn deduplicacion_en_disco_no_deja_temporales() {
        let d = tempfile::tempdir().unwrap();
        let s = FsBlobStore::open(d.path()).unwrap();
        for _ in 0..5 {
            s.put(b"el mismo activo importado por cinco rutas").unwrap();
        }
        assert_eq!(s.list().unwrap().len(), 1);

        let mut temporales = 0;
        for shard in std::fs::read_dir(d.path()).unwrap().flatten() {
            if shard.path().is_dir() {
                for f in std::fs::read_dir(shard.path()).unwrap().flatten() {
                    if f.file_name().to_str().unwrap().starts_with('.') {
                        temporales += 1;
                    }
                }
            }
        }
        assert_eq!(temporales, 0, "quedaron temporales de escritura");
        assert!(s.verify().unwrap().is_empty());
    }

    /// Control positivo de `verify`: si se corrompe un blob a mano, tiene que
    /// detectarlo. Sin este control, `verify` podría estar devolviendo siempre
    /// la lista vacía y el test anterior pasaría igual.
    #[test]
    fn verify_detecta_corrupcion() {
        let d = tempfile::tempdir().unwrap();
        let s = FsBlobStore::open(d.path()).unwrap();
        let h = s.put(b"contenido bueno").unwrap();
        assert!(s.verify().unwrap().is_empty());

        let p = d.path().join(h.shard()).join(h.to_hex());
        std::fs::write(&p, b"contenido adulterado").unwrap();
        assert_eq!(s.verify().unwrap(), vec![h], "verify no detecto la corrupcion");
    }

    #[test]
    fn hash_serializa_como_hexadecimal_legible() {
        let h = BlobHash::of(b"x");
        let j = serde_json::to_string(&h).unwrap();
        assert_eq!(j, format!("\"{}\"", h.to_hex()));
        let de: BlobHash = serde_json::from_str(&j).unwrap();
        assert_eq!(de, h);
    }
}
