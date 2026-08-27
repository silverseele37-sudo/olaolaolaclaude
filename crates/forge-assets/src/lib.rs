//! Pilar 4 — almacén de activos local y versionado.
//!
//! # La idea que ordena el crate entero
//!
//! **El índice SQLite es una caché. No es la fuente de verdad.**
//!
//! La verdad son dos cosas y solo dos: los **blobs** (`<raíz>/blobs/…`, el
//! almacén direccionado por contenido de `forge-store`) y el **diario**
//! (`<raíz>/registro.jsonl`, un registro mínimo de hechos, solo añadidos). El
//! índice (`<raíz>/indice.sqlite`) no contiene ni un dato que no se pueda
//! derivar de esos dos, y [`AssetStore::reindex`] lo reconstruye entero desde
//! cero.
//!
//! Esto no es purismo. Es la diferencia entre un archivo SQLite corrupto que
//! cuesta un `reindex` de unos segundos y uno que se lleva por delante la
//! biblioteca de activos del usuario. De hecho el almacén lo hace solo: si al
//! abrir el índice resulta ilegible, se rehace desde el diario sin avisar,
//! porque no había nada que perder. Y por el mismo motivo el índice se abre con
//! `synchronous = OFF`: un corte de energía puede dejarlo a medias y da igual.
//!
//! Hay un único camino de escritura del índice —la función interna que aplica un
//! registro del diario— y lo comparten las mutaciones en vivo y la
//! reconstrucción. Un índice reconstruido no *se parece* al que había: se
//! construye con el mismo código, y por eso el test que borra el SQLite y
//! compara búsquedas prueba algo de verdad.
//!
//! # La deduplicación no se implementa aquí
//!
//! Importar la misma textura por dos rutas produce **un** blob porque el nombre
//! de un blob *es* su contenido (ADR-0003). Este crate no tiene una sola línea
//! de código de deduplicación: llama a [`forge_store::BlobStore::put`] y ya está
//! deduplicado. Lo mismo vale para las versiones: reimportar un contenido que ya
//! era una versión conocida no crea nada porque el hash coincide.
//!
//! # Disposición en disco
//!
//! ```text
//! <raíz>/
//! ├── registro.jsonl     el diario: un hecho por línea, solo añadidos
//! ├── indice.sqlite      caché reconstruible; se puede borrar sin miedo
//! └── blobs/<aa>/<hash>  contenidos inmutables, deduplicados
//! ```

mod consulta;
mod esquema;
mod registro;

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use forge_store::{BlobHash, BlobStore, FsBlobStore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use consulta::normalizar;
use registro::{Diario, Registro};

pub use consulta::AssetQuery;
pub use esquema::ESQUEMA_VERSION;
pub use registro::Durability;

/// Nombre del diario dentro de la raíz del almacén.
pub const NOMBRE_DIARIO: &str = "registro.jsonl";
/// Nombre del índice dentro de la raíz del almacén.
pub const NOMBRE_INDICE: &str = "indice.sqlite";
/// Clave del desplazamiento de diario ya volcado al índice.
const CLAVE_OFFSET: &str = "registro_offset";

// ---------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("E/S en {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("el indice fallo: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Store(#[from] forge_store::StoreError),
    #[error(
        "version {encontrada} del esquema del indice desconocida (esta build entiende hasta la \
         {soportada}). El indice es cache: `AssetStore::open_rebuilding` lo reconstruye desde el \
         diario sin perder nada."
    )]
    EsquemaFuturo { encontrada: u32, soportada: u32 },
    #[error("no hay ningun activo con id {0}")]
    Desconocido(AssetId),
    #[error("el activo {id} no tiene la version {version}")]
    VersionDesconocida { id: AssetId, version: VersionId },
    #[error("anadir la dependencia {de} -> {a} cerraria un ciclo")]
    Ciclo { de: AssetId, a: AssetId },
    #[error("registro del diario ilegible: {0}")]
    RegistroIlegible(String),
    #[error("el indice contiene un valor que no se puede interpretar: {0}")]
    IndiceCorrupto(String),
}

impl AssetError {
    pub(crate) fn io(p: impl Into<PathBuf>, e: std::io::Error) -> Self {
        AssetError::Io {
            path: p.into(),
            source: e,
        }
    }
}

pub type Result<T> = std::result::Result<T, AssetError>;

// ---------------------------------------------------------------------------
// Identidades
// ---------------------------------------------------------------------------

/// Generador monótono de ULID, por el mismo motivo que en `forge-doc`: si los
/// bits bajos fueran aleatorios dentro del mismo milisegundo, el orden de id no
/// sería el orden de importación y una búsqueda ordenada por id devolvería los
/// activos barajados.
static GEN: Mutex<Option<ulid::Generator>> = Mutex::new(None);

fn siguiente_ulid() -> ulid::Ulid {
    let mut g = match GEN.lock() {
        Ok(g) => g,
        // Un mutex envenenado no puede impedir importar: se degrada al
        // generador no monótono en vez de propagar el pánico.
        Err(e) => e.into_inner(),
    };
    let g = g.get_or_insert_with(ulid::Generator::new);
    g.generate().unwrap_or_else(|_| ulid::Ulid::new())
}

/// Identidad de un activo. ULID: ordenable por tiempo y estable entre sesiones.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId(pub ulid::Ulid);

impl AssetId {
    pub fn new() -> Self {
        AssetId(siguiente_ulid())
    }

    /// Determinista, para tests.
    pub fn from_u128(v: u128) -> Self {
        AssetId(ulid::Ulid(v))
    }

    pub fn parse(s: &str) -> Result<Self> {
        ulid::Ulid::from_string(s)
            .map(AssetId)
            .map_err(|e| AssetError::IndiceCorrupto(format!("id {s:?}: {e}")))
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Sin prefijo: esta cadena es también la clave primaria del índice, y
        // que sea el ULID pelado la mantiene ordenable en SQL.
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Debug for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AssetId({})", self.0)
    }
}

/// Versión de un activo. Empieza en 1 y crece de uno en uno **por activo**.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct VersionId(pub u64);

impl std::fmt::Display for VersionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Metadatos
// ---------------------------------------------------------------------------

/// Qué clase de activo es. Se guarda como entero en el índice y como texto en
/// el diario: el diario se lee con `tail`, el índice no lo lee nadie a mano.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetType {
    Modelo,
    Textura,
    Material,
    Referencia,
    Documento,
    Nota,
}

impl AssetType {
    /// Código estable en el índice. **No reordenar**: cambiarlo obligaría a una
    /// migración de esquema.
    pub fn code(self) -> u8 {
        match self {
            AssetType::Modelo => 0,
            AssetType::Textura => 1,
            AssetType::Material => 2,
            AssetType::Referencia => 3,
            AssetType::Documento => 4,
            AssetType::Nota => 5,
        }
    }

    pub fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => AssetType::Modelo,
            1 => AssetType::Textura,
            2 => AssetType::Material,
            3 => AssetType::Referencia,
            4 => AssetType::Documento,
            5 => AssetType::Nota,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AssetType::Modelo => "modelo",
            AssetType::Textura => "textura",
            AssetType::Material => "material",
            AssetType::Referencia => "referencia",
            AssetType::Documento => "documento",
            AssetType::Nota => "nota",
        }
    }

    /// Todos los tipos, para poblar interfaces y para tests exhaustivos.
    pub const TODOS: [AssetType; 6] = [
        AssetType::Modelo,
        AssetType::Textura,
        AssetType::Material,
        AssetType::Referencia,
        AssetType::Documento,
        AssetType::Nota,
    ];
}

impl std::fmt::Display for AssetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lo que el usuario escribe sobre un activo. Tamaño y fechas **no** están aquí:
/// los deriva el almacén (del contenido y del reloj), y dejar que el llamante
/// los invente sería abrir la puerta a que mientan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub name: String,
    pub kind: AssetType,
    /// Se normalizan (minúsculas, sin acentos): `Arte` y `arte` son la misma.
    #[serde(default)]
    pub tags: BTreeSet<String>,
    #[serde(default)]
    pub notes: String,
}

impl AssetMeta {
    pub fn new(name: impl Into<String>, kind: AssetType) -> Self {
        AssetMeta {
            name: name.into(),
            kind,
            tags: BTreeSet::new(),
            notes: String::new(),
        }
    }

    pub fn with_tags<S: Into<String>>(mut self, tags: impl IntoIterator<Item = S>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_notes(mut self, n: impl Into<String>) -> Self {
        self.notes = n.into();
        self
    }

    /// Deja las etiquetas en forma canónica. Se aplica en la frontera pública
    /// para que diario e índice guarden exactamente lo mismo y comparar
    /// metadatos no dé falsos cambios.
    fn canonica(mut self) -> Self {
        self.tags = self
            .tags
            .iter()
            .map(|t| normalizar(t))
            .filter(|t| !t.is_empty())
            .collect();
        self
    }
}

/// Un activo tal y como está ahora mismo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub id: AssetId,
    pub meta: AssetMeta,
    /// Bytes del contenido vigente.
    pub size: u64,
    /// Milisegundos epoch de la primera importación.
    pub imported: i64,
    /// Milisegundos epoch del último cambio (contenido, metadatos o etiquetas).
    pub modified: i64,
    /// Ruta de la que se importó. Es la identidad de reimportación.
    pub origin: PathBuf,
    pub version: VersionId,
    pub hash: BlobHash,
    pub thumbnail: Option<BlobHash>,
}

/// Una entrada del historial. Nunca se borra por revertir.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AssetVersion {
    pub version: VersionId,
    pub hash: BlobHash,
    pub size: u64,
    pub created: i64,
}

// ---------------------------------------------------------------------------
// Reloj
// ---------------------------------------------------------------------------

/// De dónde salen las fechas. Se inyecta para que los tests de rango de fechas
/// puedan afirmar recuentos exactos en vez de "parece razonable".
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            // Reloj del sistema anterior a 1970: raro pero no es motivo para
            // que falle una importación.
            Err(e) => -(e.duration().as_millis() as i64),
        }
    }
}

/// Reloj gobernado a mano.
pub struct FixedClock(AtomicI64);

impl FixedClock {
    pub fn new(ms: i64) -> Self {
        FixedClock(AtomicI64::new(ms))
    }
    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    /// Adelanta el reloj y devuelve el valor nuevo.
    pub fn advance(&self, ms: i64) -> i64 {
        self.0.fetch_add(ms, Ordering::SeqCst) + ms
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Informes
// ---------------------------------------------------------------------------

/// Qué encontró y qué construyó una reconstrucción del índice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReindexReport {
    /// Registros del diario aplicados.
    pub records: u64,
    /// Líneas completas que no se pudieron interpretar. Cada una es un evento
    /// perdido, no el archivo entero: por eso el diario es texto por líneas.
    pub unreadable: u64,
    /// Bytes de cola sin `\n`: una escritura interrumpida por un corte.
    pub incomplete_tail_bytes: u64,
    pub assets: u64,
    pub versions: u64,
    pub tags: u64,
    pub dependencies: u64,
    /// Versiones cuyo contenido no está en el almacén de blobs. Con esto vacío,
    /// el almacén está completo; con algo dentro, hay historia irrecuperable.
    pub missing_blobs: Vec<BlobHash>,
    pub millis: u128,
}

/// Qué hizo la recolección de basura.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub examined: u64,
    pub removed: u64,
    pub kept: u64,
    pub freed_bytes: u64,
}

// ---------------------------------------------------------------------------
// El almacén
// ---------------------------------------------------------------------------

pub struct AssetStore {
    raiz: PathBuf,
    ruta_indice: PathBuf,
    raiz_blobs: PathBuf,
    blobs: Arc<FsBlobStore>,
    conn: Connection,
    diario: Diario,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AssetStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetStore")
            .field("raiz", &self.raiz)
            .field("diario_bytes", &self.diario.offset())
            .finish()
    }
}

fn abrir_conexion(p: &Path) -> Result<Connection> {
    let c = Connection::open(p)?;
    // `synchronous = OFF` es exactamente la consecuencia de que el índice sea
    // caché: si un corte lo deja inconsistente, la apertura siguiente lo rehace
    // desde el diario. Pagar fsync por transacción aquí sería pagar por una
    // durabilidad que ya da el diario.
    c.pragma_update(None, "synchronous", "OFF")?;
    // Journal de rollback clásico, no WAL: así el índice entero es UN archivo, y
    // "borra el índice y reconstruye" no tiene letra pequeña.
    let _: String = c.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    c.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(c)
}

fn borrar_indice(p: &Path) -> Result<()> {
    for sufijo in ["", "-journal", "-wal", "-shm"] {
        let mut ruta = p.as_os_str().to_os_string();
        ruta.push(sufijo);
        let ruta = PathBuf::from(ruta);
        match std::fs::remove_file(&ruta) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AssetError::io(ruta, e)),
        }
    }
    Ok(())
}

/// Clave de reimportación. Se canoniza cuando el archivo existe para que
/// `./textura.png` y `/casa/proyecto/textura.png` sean el mismo activo; cuando
/// no existe (contenido generado en memoria) se toma la ruta tal cual.
fn clave_de_origen(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

impl AssetStore {
    /// Abre o crea el almacén en `raiz`.
    ///
    /// Si el índice falta o está ilegible, se reconstruye desde el diario. Si el
    /// índice es de una versión de esquema *futura*, **no** se reconstruye: se
    /// devuelve [`AssetError::EsquemaFuturo`], porque lo escribió una build más
    /// nueva y borrarlo en silencio destruiría lo que esa build sí entiende.
    pub fn open(raiz: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(raiz, Arc::new(SystemClock))
    }

    pub fn open_with(raiz: impl AsRef<Path>, clock: Arc<dyn Clock>) -> Result<Self> {
        let raiz = raiz.as_ref();
        match Self::intentar_abrir(raiz, clock.clone(), false) {
            Ok(s) => Ok(s),
            // Índice ilegible: es caché, se rehace. Este es el caso que el
            // diseño entero existe para hacer aburrido.
            Err(AssetError::Sqlite(_)) => Self::intentar_abrir(raiz, clock, true),
            Err(e) => Err(e),
        }
    }

    /// Abre tirando el índice y reconstruyéndolo. La salida de emergencia para
    /// un esquema desconocido.
    pub fn open_rebuilding(raiz: impl AsRef<Path>) -> Result<Self> {
        Self::intentar_abrir(raiz.as_ref(), Arc::new(SystemClock), true)
    }

    fn intentar_abrir(raiz: &Path, clock: Arc<dyn Clock>, rehacer: bool) -> Result<Self> {
        std::fs::create_dir_all(raiz).map_err(|e| AssetError::io(raiz, e))?;
        let raiz_blobs = raiz.join("blobs");
        let blobs = Arc::new(FsBlobStore::open(&raiz_blobs)?);
        let diario = Diario::abrir(raiz.join(NOMBRE_DIARIO))?;
        let ruta_indice = raiz.join(NOMBRE_INDICE);
        if rehacer {
            borrar_indice(&ruta_indice)?;
        }
        let conn = abrir_conexion(&ruta_indice)?;
        let mut s = AssetStore {
            raiz: raiz.to_path_buf(),
            ruta_indice,
            raiz_blobs,
            blobs,
            conn,
            diario,
            clock,
        };
        s.poner_al_dia()?;
        Ok(s)
    }

    /// Deja el índice a la altura del diario. Barato en el caso normal: se
    /// compara un solo número.
    fn poner_al_dia(&mut self) -> Result<()> {
        match esquema::migrar(&self.conn)? {
            esquema::Estado::Creado => {
                self.aplicar_desde(0)?;
            }
            esquema::Estado::AlDia => {
                let aplicado = self.offset_aplicado()?;
                let real = self.diario.offset();
                if aplicado > real {
                    // El índice dice haber consumido más diario del que existe:
                    // o el diario se truncó, o este índice es de otro almacén.
                    // En los dos casos hay que empezar de cero.
                    self.reindex()?;
                } else if aplicado < real {
                    self.aplicar_desde(aplicado)?;
                }
            }
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.raiz
    }

    /// El almacén de blobs. Expuesto para poder contarlos y verificarlos: son
    /// la mitad de la fuente de verdad.
    pub fn blobs(&self) -> &FsBlobStore {
        &self.blobs
    }

    /// Cuánto se compromete cada escritura del diario. Bajarlo a
    /// [`Durability::Deferred`] solo para cargas masivas.
    pub fn set_durability(&mut self, d: Durability) {
        self.diario.durabilidad = d;
    }

    /// Agrupa muchas mutaciones en una sola transacción del índice.
    ///
    /// No se anida: dentro del cierre no se puede volver a llamar. El diario se
    /// escribe igual dentro del lote; si la transacción del índice se deshace,
    /// el índice queda atrasado y la apertura siguiente lo pone al día, porque
    /// el desplazamiento consumido también se deshace.
    pub fn batch<R>(&mut self, f: impl FnOnce(&mut AssetStore) -> Result<R>) -> Result<R> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(r) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(r)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // --- escritura ---------------------------------------------------------

    /// Anota un hecho. **El diario primero**: es la verdad, y si el índice
    /// fallara después, la apertura siguiente lo reconstruye a partir de él.
    fn registrar(&mut self, r: Registro) -> Result<()> {
        self.diario.append(&r)?;
        aplicar(&self.conn, &r)?;
        self.guardar_offset(self.diario.offset())
    }

    fn guardar_offset(&self, off: u64) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO meta_indice(clave, valor) VALUES(?1, ?2)
             ON CONFLICT(clave) DO UPDATE SET valor = excluded.valor",
            )?
            .execute(params![CLAVE_OFFSET, off.to_string()])?;
        Ok(())
    }

    fn offset_aplicado(&self) -> Result<u64> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT valor FROM meta_indice WHERE clave = ?1",
                [CLAVE_OFFSET],
                |r| r.get(0),
            )
            .optional()?;
        match v {
            None => Ok(0),
            Some(s) => s
                .parse()
                .map_err(|_| AssetError::IndiceCorrupto(format!("offset {s:?} no es un numero"))),
        }
    }

    /// Importa un archivo. Reimportar la misma ruta con contenido distinto crea
    /// una versión; con contenido que ya era una versión conocida no crea nada.
    pub fn import(&mut self, path: &Path, meta: AssetMeta) -> Result<AssetId> {
        let bytes = std::fs::read(path).map_err(|e| AssetError::io(path, e))?;
        self.import_bytes(path, &bytes, meta)
    }

    /// Igual que [`AssetStore::import`] pero con el contenido ya en memoria.
    ///
    /// `origin` sigue siendo la identidad de reimportación aunque no exista como
    /// archivo: sirve para activos generados por la aplicación y para poblar
    /// conjuntos de prueba grandes sin escribir cien mil archivos sueltos.
    pub fn import_bytes(
        &mut self,
        origin: impl AsRef<Path>,
        bytes: &[u8],
        meta: AssetMeta,
    ) -> Result<AssetId> {
        // Aquí está toda la deduplicación del crate: una llamada. El nombre del
        // blob es su contenido, así que importar lo mismo dos veces no escribe
        // dos veces (ADR-0003).
        let hash = self.blobs.put(bytes)?;
        let meta = meta.canonica();
        let origen = clave_de_origen(origin.as_ref());
        let ts = self.clock.now_ms();
        let tam = bytes.len() as u64;

        let Some(id) = self.id_por_origen(&origen)? else {
            let id = AssetId::new();
            self.registrar(Registro::Importado {
                id: id.to_string(),
                version: 1,
                hash: hash.to_hex(),
                tam,
                origen,
                meta,
                ts,
            })?;
            return Ok(id);
        };

        if let Some(v) = self.version_con_hash(id, hash)? {
            // Contenido idéntico a una versión que ya existe: no se crea
            // ninguna. Lo que sí puede cambiar es cuál es la vigente: el usuario
            // pidió "este archivo tiene ahora este contenido", y ese contenido
            // ya lo teníamos. Es un revert, no una versión nueva.
            if self.version_vigente(id)? != v {
                self.registrar(Registro::Vigente {
                    id: id.to_string(),
                    version: v,
                    ts,
                })?;
            }
            if self.meta_de(id)? != meta {
                self.registrar(Registro::Meta {
                    id: id.to_string(),
                    meta,
                    ts,
                })?;
            }
            return Ok(id);
        }

        let siguiente = self.max_version(id)? + 1;
        self.registrar(Registro::Importado {
            id: id.to_string(),
            version: siguiente,
            hash: hash.to_hex(),
            tam,
            origen,
            meta,
            ts,
        })?;
        Ok(id)
    }

    pub fn set_meta(&mut self, id: AssetId, meta: AssetMeta) -> Result<()> {
        self.exigir(id)?;
        let ts = self.clock.now_ms();
        self.registrar(Registro::Meta {
            id: id.to_string(),
            meta: meta.canonica(),
            ts,
        })
    }

    pub fn tag(&mut self, id: AssetId, etiqueta: &str) -> Result<()> {
        self.exigir(id)?;
        let e = normalizar(etiqueta);
        if e.is_empty() {
            return Ok(());
        }
        let ts = self.clock.now_ms();
        self.registrar(Registro::Etiqueta {
            id: id.to_string(),
            etiqueta: e,
            pon: true,
            ts,
        })
    }

    pub fn untag(&mut self, id: AssetId, etiqueta: &str) -> Result<()> {
        self.exigir(id)?;
        let ts = self.clock.now_ms();
        self.registrar(Registro::Etiqueta {
            id: id.to_string(),
            etiqueta: normalizar(etiqueta),
            pon: false,
            ts,
        })
    }

    /// Asocia una miniatura ya calculada. **Se guarda el hash, no la imagen**:
    /// generar miniaturas no es trabajo de este crate.
    pub fn set_thumbnail(&mut self, id: AssetId, h: Option<BlobHash>) -> Result<()> {
        self.exigir(id)?;
        let ts = self.clock.now_ms();
        self.registrar(Registro::Miniatura {
            id: id.to_string(),
            hash: h.map(|h| h.to_hex()),
            ts,
        })
    }

    /// Da de baja el activo. Su contenido queda huérfano hasta el próximo
    /// [`AssetStore::gc`]; si otro activo comparte el mismo blob, no se pierde.
    pub fn remove(&mut self, id: AssetId) -> Result<()> {
        self.exigir(id)?;
        let ts = self.clock.now_ms();
        self.registrar(Registro::Borrado {
            id: id.to_string(),
            ts,
        })
    }

    /// Mueve la versión vigente. **No borra historia**: las versiones
    /// posteriores siguen ahí y se puede volver a ellas.
    pub fn revert(&mut self, id: AssetId, to: VersionId) -> Result<()> {
        self.exigir(id)?;
        if !self.existe_version(id, to)? {
            return Err(AssetError::VersionDesconocida { id, version: to });
        }
        let ts = self.clock.now_ms();
        self.registrar(Registro::Vigente {
            id: id.to_string(),
            version: to.0,
            ts,
        })
    }

    /// Declara que `de` depende de `a`. Rechaza el ciclo antes de crearlo.
    pub fn add_dependency(&mut self, de: AssetId, a: AssetId) -> Result<()> {
        self.exigir(de)?;
        self.exigir(a)?;
        if de == a || self.alcanzable(a, de)? {
            return Err(AssetError::Ciclo { de, a });
        }
        let ts = self.clock.now_ms();
        self.registrar(Registro::Dependencia {
            de: de.to_string(),
            a: a.to_string(),
            pon: true,
            ts,
        })
    }

    pub fn remove_dependency(&mut self, de: AssetId, a: AssetId) -> Result<()> {
        let ts = self.clock.now_ms();
        self.registrar(Registro::Dependencia {
            de: de.to_string(),
            a: a.to_string(),
            pon: false,
            ts,
        })
    }

    // --- lectura -----------------------------------------------------------

    pub fn search(&self, q: &AssetQuery) -> Result<Vec<AssetId>> {
        let (sql, par) = consulta::construir(q);
        let mut st = self.conn.prepare_cached(&sql)?;
        let filas = st.query_map(rusqlite::params_from_iter(par), |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for f in filas {
            out.push(AssetId::parse(&f?)?);
        }
        Ok(out)
    }

    pub fn get(&self, id: AssetId) -> Result<Option<Asset>> {
        let mut st = self.conn.prepare_cached(
            "SELECT nombre, tipo, notas, tam, importado, modificado, origen,
                    version_vigente, hash_vigente, miniatura
               FROM activos WHERE id = ?1",
        )?;
        let fila = st
            .query_row([id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            })
            .optional()?;
        let Some((nombre, tipo, notas, tam, imp, modif, origen, ver, hash, mini)) = fila else {
            return Ok(None);
        };
        let kind = AssetType::from_code(tipo as u8)
            .ok_or_else(|| AssetError::IndiceCorrupto(format!("tipo {tipo}")))?;
        let thumbnail = match mini {
            Some(h) => Some(BlobHash::from_hex(&h)?),
            None => None,
        };
        Ok(Some(Asset {
            id,
            meta: AssetMeta {
                name: nombre,
                kind,
                tags: self.etiquetas(id)?,
                notes: notas,
            },
            size: tam as u64,
            imported: imp,
            modified: modif,
            origin: PathBuf::from(origen),
            version: VersionId(ver as u64),
            hash: BlobHash::from_hex(&hash)?,
            thumbnail,
        }))
    }

    /// Historial completo, de la 1 a la última, incluidas las posteriores a la
    /// vigente si se ha revertido.
    pub fn versions(&self, id: AssetId) -> Result<Vec<AssetVersion>> {
        self.exigir(id)?;
        let mut st = self.conn.prepare_cached(
            "SELECT version, hash, tam, creada FROM versiones WHERE id = ?1 ORDER BY version",
        )?;
        let filas = st.query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for f in filas {
            let (v, h, t, c) = f?;
            out.push(AssetVersion {
                version: VersionId(v as u64),
                hash: BlobHash::from_hex(&h)?,
                size: t as u64,
                created: c,
            });
        }
        Ok(out)
    }

    pub fn thumbnail(&self, id: AssetId) -> Result<Option<BlobHash>> {
        self.exigir(id)?;
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT miniatura FROM activos WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        match v {
            Some(h) => Ok(Some(BlobHash::from_hex(&h)?)),
            None => Ok(None),
        }
    }

    /// Quién depende de `id` (sentido inverso del grafo).
    pub fn dependents(&self, id: AssetId) -> Result<Vec<AssetId>> {
        self.exigir(id)?;
        self.vecinos("SELECT de FROM dependencias WHERE a = ?1 ORDER BY de", id)
    }

    /// De qué depende `id`.
    pub fn dependencies(&self, id: AssetId) -> Result<Vec<AssetId>> {
        self.exigir(id)?;
        self.vecinos("SELECT a FROM dependencias WHERE de = ?1 ORDER BY a", id)
    }

    /// Cierre transitivo del sentido inverso: todo lo que se rompería si `id`
    /// cambiara. Lleva conjunto de visitados, así que **no se cuelga** aunque el
    /// grafo almacenado contenga un ciclo (por ejemplo, un diario editado a
    /// mano). Rechazar ciclos al escribir no basta: hay que sobrevivirlos.
    pub fn transitive_dependents(&self, id: AssetId) -> Result<Vec<AssetId>> {
        self.exigir(id)?;
        let mut vistos = HashSet::new();
        let mut cola = VecDeque::from([id]);
        vistos.insert(id);
        let mut out = Vec::new();
        while let Some(n) = cola.pop_front() {
            for d in self.vecinos("SELECT de FROM dependencias WHERE a = ?1 ORDER BY de", n)? {
                if vistos.insert(d) {
                    out.push(d);
                    cola.push_back(d);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Contenido de la versión vigente.
    pub fn content(&self, id: AssetId) -> Result<Option<Arc<[u8]>>> {
        let Some(a) = self.get(id)? else {
            return Ok(None);
        };
        Ok(self.blobs.get(a.hash)?)
    }

    /// Contenido de una versión concreta del historial.
    pub fn content_of(&self, id: AssetId, v: VersionId) -> Result<Option<Arc<[u8]>>> {
        // Una versión que no cabe en `i64` no puede estar en el índice: el lado
        // de escritura las rechaza. Se sale aquí en vez de dejar que el `as
        // i64` la convierta en otra cosa y consultar por un número que no es el
        // que se pidió.
        let Ok(vi) = i64::try_from(v.0) else {
            return Ok(None);
        };
        let h: Option<String> = self
            .conn
            .query_row(
                "SELECT hash FROM versiones WHERE id = ?1 AND version = ?2",
                params![id.to_string(), vi],
                |r| r.get(0),
            )
            .optional()?;
        let Some(h) = h else {
            return Err(AssetError::VersionDesconocida { id, version: v });
        };
        Ok(self.blobs.get(BlobHash::from_hex(&h)?)?)
    }

    /// Cuántos activos vivos hay.
    pub fn len(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM activos", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    // --- mantenimiento -----------------------------------------------------

    /// Tira el índice y lo reconstruye entero desde el diario.
    ///
    /// Es la operación que hace cierto lo que promete la documentación del
    /// crate. Si esto no funcionara, el índice sería fuente de verdad aunque
    /// dijéramos lo contrario.
    pub fn reindex(&mut self) -> Result<ReindexReport> {
        let t0 = std::time::Instant::now();

        // Cerrar la conexión ANTES de borrar: si el archivo ya fue borrado por
        // fuera, el descriptor abierto sigue apuntando a un inodo fantasma y
        // todo lo que escribiéramos ahí se perdería en silencio.
        let vieja = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        drop(vieja);
        borrar_indice(&self.ruta_indice)?;
        self.conn = abrir_conexion(&self.ruta_indice)?;
        esquema::crear(&self.conn)?;

        let (records, unreadable, incomplete_tail_bytes) = self.aplicar_desde(0)?;

        let cuenta = |sql: &str| -> Result<u64> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(n as u64)
        };

        // Un solo recorrido del directorio de blobs en vez de una llamada al
        // sistema por versión: con cien mil versiones la diferencia es de
        // segundos a milisegundos.
        let presentes: HashSet<BlobHash> = self.blobs.list()?.into_iter().collect();
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT hash FROM versiones ORDER BY hash")?;
        let filas = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut missing_blobs = Vec::new();
        for f in filas {
            let h = BlobHash::from_hex(&f?)?;
            if !presentes.contains(&h) {
                missing_blobs.push(h);
            }
        }
        drop(st);

        Ok(ReindexReport {
            records,
            unreadable,
            incomplete_tail_bytes,
            assets: cuenta("SELECT COUNT(*) FROM activos")?,
            versions: cuenta("SELECT COUNT(*) FROM versiones")?,
            tags: cuenta("SELECT COUNT(*) FROM etiquetas")?,
            dependencies: cuenta("SELECT COUNT(*) FROM dependencias")?,
            missing_blobs,
            millis: t0.elapsed().as_millis(),
        })
    }

    /// Borra los blobs que ya no referencia nadie.
    ///
    /// «Nadie» incluye **toda** la historia, no solo las versiones vigentes: si
    /// recolectar borrara el contenido de una versión anterior, `revert` dejaría
    /// de funcionar y el historial sería decorativo. Las miniaturas también
    /// cuentan como referencia.
    pub fn gc(&mut self) -> Result<GcReport> {
        let mut vivos: HashSet<BlobHash> = HashSet::new();
        {
            let mut st = self.conn.prepare("SELECT DISTINCT hash FROM versiones")?;
            let filas = st.query_map([], |r| r.get::<_, String>(0))?;
            for f in filas {
                vivos.insert(BlobHash::from_hex(&f?)?);
            }
        }
        {
            let mut st = self
                .conn
                .prepare("SELECT DISTINCT miniatura FROM activos WHERE miniatura IS NOT NULL")?;
            let filas = st.query_map([], |r| r.get::<_, String>(0))?;
            for f in filas {
                vivos.insert(BlobHash::from_hex(&f?)?);
            }
        }

        let mut rep = GcReport::default();
        for h in self.blobs.list()? {
            rep.examined += 1;
            if vivos.contains(&h) {
                rep.kept += 1;
                continue;
            }
            // La disposición `<raiz>/<aa>/<hash>` es parte del contrato público
            // de `FsBlobStore` (ADR-0003). `forge-store` no ofrece borrado —un
            // blob es inmutable y nadie lo borra— pero la recolección de activos
            // sí necesita quitar los huérfanos, y este directorio es nuestro.
            let p = self.raiz_blobs.join(h.shard()).join(h.to_hex());
            let tam = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            match std::fs::remove_file(&p) {
                Ok(()) => {
                    rep.removed += 1;
                    rep.freed_bytes += tam;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(AssetError::io(p, e)),
            }
        }
        Ok(rep)
    }

    // --- interno -----------------------------------------------------------

    fn aplicar_desde(&mut self, desde: u64) -> Result<(u64, u64, u64)> {
        let l = self.diario.leer_desde(desde)?;
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            for reg in &l.registros {
                aplicar(&self.conn, reg)?;
            }
            self.guardar_offset(l.offset)
        })();
        match r {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok((l.registros.len() as u64, l.ilegibles, l.cola_incompleta))
    }

    fn exigir(&self, id: AssetId) -> Result<()> {
        let hay: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM activos WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        hay.map(|_| ()).ok_or(AssetError::Desconocido(id))
    }

    fn vecinos(&self, sql: &str, id: AssetId) -> Result<Vec<AssetId>> {
        let mut st = self.conn.prepare_cached(sql)?;
        let filas = st.query_map([id.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for f in filas {
            out.push(AssetId::parse(&f?)?);
        }
        Ok(out)
    }

    /// ¿Se llega de `desde` a `objetivo` siguiendo aristas `de -> a`?
    /// Con conjunto de visitados: termina aunque ya haya un ciclo almacenado.
    fn alcanzable(&self, desde: AssetId, objetivo: AssetId) -> Result<bool> {
        let mut vistos = HashSet::from([desde]);
        let mut cola = VecDeque::from([desde]);
        while let Some(n) = cola.pop_front() {
            if n == objetivo {
                return Ok(true);
            }
            for d in self.vecinos("SELECT a FROM dependencias WHERE de = ?1", n)? {
                if vistos.insert(d) {
                    cola.push_back(d);
                }
            }
        }
        Ok(false)
    }

    fn id_por_origen(&self, origen: &str) -> Result<Option<AssetId>> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT id FROM activos WHERE origen = ?1", [origen], |r| {
                r.get(0)
            })
            .optional()?;
        v.map(|s| AssetId::parse(&s)).transpose()
    }

    fn version_con_hash(&self, id: AssetId, h: BlobHash) -> Result<Option<u64>> {
        let v: Option<i64> = self
            .conn
            .query_row(
                "SELECT version FROM versiones WHERE id = ?1 AND hash = ?2 ORDER BY version LIMIT 1",
                params![id.to_string(), h.to_hex()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.map(|v| v as u64))
    }

    fn existe_version(&self, id: AssetId, v: VersionId) -> Result<bool> {
        let Ok(vi) = i64::try_from(v.0) else {
            return Ok(false);
        };
        let x: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM versiones WHERE id = ?1 AND version = ?2",
                params![id.to_string(), vi],
                |r| r.get(0),
            )
            .optional()?;
        Ok(x.is_some())
    }

    fn max_version(&self, id: AssetId) -> Result<u64> {
        let v: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM versiones WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )?;
        Ok(v as u64)
    }

    fn version_vigente(&self, id: AssetId) -> Result<u64> {
        let v: i64 = self.conn.query_row(
            "SELECT version_vigente FROM activos WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )?;
        Ok(v as u64)
    }

    fn etiquetas(&self, id: AssetId) -> Result<BTreeSet<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT etiqueta FROM etiquetas WHERE id = ?1 ORDER BY etiqueta")?;
        let filas = st.query_map([id.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = BTreeSet::new();
        for f in filas {
            out.insert(f?);
        }
        Ok(out)
    }

    fn meta_de(&self, id: AssetId) -> Result<AssetMeta> {
        self.get(id)?
            .map(|a| a.meta)
            .ok_or(AssetError::Desconocido(id))
    }

    /// Mete una arista **sin** comprobar ciclos. Solo para probar que las
    /// lecturas del grafo sobreviven a un grafo ya corrupto.
    #[cfg(test)]
    fn arista_cruda(&self, de: AssetId, a: AssetId) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO dependencias(de, a) VALUES(?1, ?2)",
            params![de.to_string(), a.to_string()],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// El único camino de escritura del índice
// ---------------------------------------------------------------------------

/// Aplica un hecho del diario al índice.
///
/// Es la **única** función que escribe en el índice, y la comparten las
/// mutaciones en vivo y la reconstrucción. Por eso un índice reconstruido no se
/// parece al original: se construye con el mismo código.
///
/// Los eventos sobre activos que ya no existen se ignoran en vez de fallar: un
/// diario largo contiene etiquetados de activos borrados después, y eso es
/// historia normal, no corrupción.
/// Convierte un `u64` del diario a `i64` para SQLite, o falla.
///
/// SQLite no tiene enteros sin signo: su `INTEGER` es un `i64`. Un `as i64` a
/// secas convierte `u64::MAX` en `-1` sin avisar, y a partir de ahí el tamaño
/// que se le enseña al usuario es negativo y los filtros por rango no casan con
/// nada. Es el mismo fallo que ya se arregló en el lado de la consulta, donde
/// `size_between(0, u64::MAX)` producía `tam BETWEEN 0 AND -1` y devolvía cero
/// resultados en silencio.
///
/// Aquí no se satura, se rechaza: en la consulta `u64::MAX` es un centinela
/// legítimo de «sin tope», pero un tamaño o una versión que no cabe en `i64`
/// solo puede venir de un diario adulterado, y el diario es la fuente de verdad
/// del almacén. Saturar guardaría un valor que no es el que dice el registro.
fn a_i64(v: u64, que: &str) -> Result<i64> {
    i64::try_from(v).map_err(|_| {
        AssetError::RegistroIlegible(format!(
            "{que} = {v} no cabe en el entero con signo de SQLite; el diario esta adulterado"
        ))
    })
}

fn aplicar(c: &Connection, r: &Registro) -> Result<()> {
    match r {
        Registro::Importado {
            id,
            version,
            hash,
            tam,
            origen,
            meta,
            ts,
        } => {
            // `importado` solo se fija en el alta: en las versiones siguientes
            // el `ON CONFLICT` no lo toca, y por eso conserva la fecha original.
            c.prepare_cached(
                "INSERT INTO activos
                   (id, nombre, nombre_norm, tipo, notas, notas_norm, tam,
                    importado, modificado, origen, version_vigente, hash_vigente, miniatura)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11, NULL)
                 ON CONFLICT(id) DO UPDATE SET
                   nombre          = excluded.nombre,
                   nombre_norm     = excluded.nombre_norm,
                   tipo            = excluded.tipo,
                   notas           = excluded.notas,
                   notas_norm      = excluded.notas_norm,
                   tam             = excluded.tam,
                   modificado      = excluded.modificado,
                   origen          = excluded.origen,
                   version_vigente = excluded.version_vigente,
                   hash_vigente    = excluded.hash_vigente",
            )?
            .execute(params![
                id,
                meta.name,
                normalizar(&meta.name),
                i64::from(meta.kind.code()),
                meta.notes,
                normalizar(&meta.notes),
                a_i64(*tam, "tam")?,
                ts,
                origen,
                a_i64(*version, "version")?,
                hash,
            ])?;

            c.prepare_cached(
                "INSERT OR REPLACE INTO versiones(id, version, hash, tam, creada)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![
                id,
                a_i64(*version, "version")?,
                hash,
                a_i64(*tam, "tam")?,
                ts
            ])?;

            poner_etiquetas(c, id, &meta.tags)?;
        }

        Registro::Vigente { id, version, ts } => {
            // La condición `EXISTS` evita dejar el activo apuntando a una
            // versión inexistente si el diario viniera adulterado.
            c.prepare_cached(
                "UPDATE activos SET
                   version_vigente = ?2,
                   modificado      = ?3,
                   hash_vigente    = (SELECT hash FROM versiones WHERE id = ?1 AND version = ?2),
                   tam             = (SELECT tam  FROM versiones WHERE id = ?1 AND version = ?2)
                 WHERE id = ?1
                   AND EXISTS (SELECT 1 FROM versiones WHERE id = ?1 AND version = ?2)",
            )?
            .execute(params![id, a_i64(*version, "version")?, ts])?;
        }

        Registro::Meta { id, meta, ts } => {
            let n = c
                .prepare_cached(
                    "UPDATE activos SET
                       nombre = ?2, nombre_norm = ?3, tipo = ?4,
                       notas = ?5, notas_norm = ?6, modificado = ?7
                     WHERE id = ?1",
                )?
                .execute(params![
                    id,
                    meta.name,
                    normalizar(&meta.name),
                    i64::from(meta.kind.code()),
                    meta.notes,
                    normalizar(&meta.notes),
                    ts,
                ])?;
            if n > 0 {
                poner_etiquetas(c, id, &meta.tags)?;
            }
        }

        Registro::Etiqueta {
            id,
            etiqueta,
            pon,
            ts,
        } => {
            if *pon {
                c.prepare_cached(
                    "INSERT OR IGNORE INTO etiquetas(id, etiqueta)
                     SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM activos WHERE id = ?1)",
                )?
                .execute(params![id, etiqueta])?;
            } else {
                c.prepare_cached("DELETE FROM etiquetas WHERE id = ?1 AND etiqueta = ?2")?
                    .execute(params![id, etiqueta])?;
            }
            c.prepare_cached("UPDATE activos SET modificado = ?2 WHERE id = ?1")?
                .execute(params![id, ts])?;
        }

        Registro::Miniatura { id, hash, ts } => {
            c.prepare_cached("UPDATE activos SET miniatura = ?2, modificado = ?3 WHERE id = ?1")?
                .execute(params![id, hash, ts])?;
        }

        Registro::Dependencia { de, a, pon, .. } => {
            if *pon {
                // Las dos comprobaciones de existencia impiden que un diario con
                // eventos de activos ya borrados deje aristas colgando.
                c.prepare_cached(
                    "INSERT OR IGNORE INTO dependencias(de, a)
                     SELECT ?1, ?2
                      WHERE EXISTS (SELECT 1 FROM activos WHERE id = ?1)
                        AND EXISTS (SELECT 1 FROM activos WHERE id = ?2)",
                )?
                .execute(params![de, a])?;
            } else {
                c.prepare_cached("DELETE FROM dependencias WHERE de = ?1 AND a = ?2")?
                    .execute(params![de, a])?;
            }
        }

        Registro::Borrado { id, .. } => {
            c.prepare_cached("DELETE FROM activos WHERE id = ?1")?
                .execute([id])?;
            c.prepare_cached("DELETE FROM versiones WHERE id = ?1")?
                .execute([id])?;
            c.prepare_cached("DELETE FROM etiquetas WHERE id = ?1")?
                .execute([id])?;
            c.prepare_cached("DELETE FROM dependencias WHERE de = ?1 OR a = ?1")?
                .execute([id])?;
        }
    }
    Ok(())
}

fn poner_etiquetas(c: &Connection, id: &str, tags: &BTreeSet<String>) -> Result<()> {
    c.prepare_cached("DELETE FROM etiquetas WHERE id = ?1")?
        .execute([id])?;
    let mut ins =
        c.prepare_cached("INSERT OR IGNORE INTO etiquetas(id, etiqueta) VALUES(?1, ?2)")?;
    for t in tags {
        ins.execute(params![id, t])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests que necesitan ver las tripas
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn almacen() -> (tempfile::TempDir, AssetStore) {
        let d = tempfile::tempdir().expect("tempdir");
        let s = AssetStore::open(d.path()).expect("abrir");
        (d, s)
    }

    fn meter(s: &mut AssetStore, n: &str) -> AssetId {
        s.import_bytes(
            format!("/virtual/{n}"),
            n.as_bytes(),
            AssetMeta::new(n, AssetType::Modelo),
        )
        .expect("importar")
    }

    /// Un diario adulterado no puede colar un tamaño negativo en el índice.
    ///
    /// SQLite guarda enteros con signo. `u64::MAX as i64` es `-1`, así que un
    /// registro con `tam: u64::MAX` se guardaría como un activo de tamaño −1: el
    /// usuario vería un tamaño negativo y `size_between` no lo encontraría
    /// jamás. Es el mismo fallo que ya se arregló en el lado de la consulta,
    /// pero por la puerta de la escritura.
    ///
    /// El control negativo de abajo es lo que hace que este test valga algo: si
    /// `a_i64` saturara en vez de rechazar —que fue la primera idea— el registro
    /// entraría con `i64::MAX` y la primera mitad del test pasaría igual.
    #[test]
    fn el_indice_rechaza_un_registro_con_un_tamano_que_no_cabe() {
        let c = Connection::open_in_memory().expect("sqlite");
        esquema::crear(&c).expect("esquema");

        let malo = Registro::Importado {
            id: "01ABCDEFGHIJKLMNOPQRSTUVWX".into(),
            version: 1,
            hash: "0".repeat(64),
            tam: u64::MAX,
            origen: "/virtual/hostil".into(),
            meta: AssetMeta::new("hostil", AssetType::Modelo),
            ts: 0,
        };
        match aplicar(&c, &malo) {
            Err(AssetError::RegistroIlegible(m)) => {
                assert!(m.contains("tam"), "{m}");
            }
            otro => panic!("se acepto un tamano imposible: {otro:?}"),
        }

        // Control negativo: no se rechazó nada, no se guardó nada. Si el
        // `execute` se hubiera colado antes del error, aquí habría una fila.
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM activos", [], |r| r.get(0))
            .expect("contar");
        assert_eq!(n, 0, "quedo una fila de un registro rechazado");

        // Control positivo: el mismo registro con un tamaño normal sí entra, o
        // sea que lo que falla es el tamaño y no el resto del registro.
        let bueno = Registro::Importado {
            id: "01ABCDEFGHIJKLMNOPQRSTUVWX".into(),
            version: 1,
            hash: "0".repeat(64),
            tam: 4096,
            origen: "/virtual/hostil".into(),
            meta: AssetMeta::new("hostil", AssetType::Modelo),
            ts: 0,
        };
        aplicar(&c, &bueno).expect("un tamano normal tiene que entrar");
        let t: i64 = c
            .query_row("SELECT tam FROM activos", [], |r| r.get(0))
            .expect("leer tam");
        assert_eq!(t, 4096);
    }

    /// Respuesta conocida del normalizador. Sin este test, "no distingue
    /// acentos" sería una afirmación de la documentación y nada más.
    #[test]
    fn normalizar_quita_acentos_y_mayusculas() {
        assert_eq!(
            normalizar("Válvula Ñ Çedilla ÀÊÎÕÜ"),
            "valvula n cedilla aeiou"
        );
        assert_eq!(normalizar("ya-normal_123"), "ya-normal_123");
    }

    /// Control positivo y negativo del constructor de SQL: un filtro apagado no
    /// añade cláusula, uno encendido añade exactamente la suya.
    #[test]
    fn el_sql_solo_lleva_los_filtros_pedidos() {
        let (sql, par) = consulta::construir(&AssetQuery::default());
        assert!(!sql.contains("LIKE"), "{sql}");
        assert!(!sql.contains("etiquetas"), "{sql}");
        assert!(!sql.contains("BETWEEN"), "{sql}");
        assert!(par.is_empty());

        let q = AssetQuery::new()
            .with_text("x")
            .with_all_tags(["a", "b"])
            .size_between(1, 2);
        let (sql, par) = consulta::construir(&q);
        assert_eq!(sql.matches("LIKE").count(), 2, "nombre y notas: {sql}");
        assert_eq!(sql.matches("BETWEEN").count(), 1, "{sql}");
        assert!(sql.contains("COUNT(*)"), "el Y de etiquetas: {sql}");
        // 2 del texto + 2 etiquetas + 1 cuenta + 2 del rango de tamaño
        assert_eq!(par.len(), 7, "{sql}");
    }

    /// Un ciclo se rechaza al escribir **y** se sobrevive al leer. La segunda
    /// mitad hace falta: un diario editado a mano o un índice de otra versión
    /// pueden contener el ciclo que `add_dependency` nunca habría creado, y una
    /// travesía sin visitados se colgaría para siempre.
    #[test]
    fn un_ciclo_se_detecta_y_las_lecturas_no_se_cuelgan() {
        let (_d, mut s) = almacen();
        let a = meter(&mut s, "a");
        let b = meter(&mut s, "b");
        let c = meter(&mut s, "c");

        s.add_dependency(a, b).expect("a->b");
        s.add_dependency(b, c).expect("b->c");

        // Control negativo del detector: cerrar el triángulo es un ciclo.
        assert!(matches!(
            s.add_dependency(c, a),
            Err(AssetError::Ciclo { .. })
        ));
        assert!(matches!(
            s.add_dependency(a, a),
            Err(AssetError::Ciclo { .. })
        ));
        // Control positivo: una arista que no cierra nada sí entra.
        s.add_dependency(a, c).expect("a->c no es ciclo");

        // Y ahora el ciclo por la fuerza, saltándose la comprobación.
        s.arista_cruda(c, a).expect("arista cruda");
        assert!(
            s.alcanzable(a, a).expect("alcanzable"),
            "el ciclo esta ahi de verdad"
        );
        // Si esto termina, la travesía sobrevive al ciclo. El recuento exacto
        // además fija que el nodo de partida no se cuenta a sí mismo.
        assert_eq!(s.transitive_dependents(c).expect("cierre"), {
            let mut v = vec![a, b];
            v.sort();
            v
        });
    }

    /// El índice no puede abrirse "a ver si va" si su esquema es desconocido.
    /// Control positivo (la versión de esta build abre) y negativo (una futura
    /// no) en el mismo test.
    #[test]
    fn un_esquema_futuro_no_se_abre_a_ver_si_va() {
        let (d, s) = almacen();
        drop(s);
        assert!(
            AssetStore::open(d.path()).is_ok(),
            "la version propia tiene que abrir"
        );

        let c = Connection::open(d.path().join(NOMBRE_INDICE)).expect("abrir sqlite");
        c.pragma_update(None, "user_version", 99_i64)
            .expect("marcar futuro");
        drop(c);

        match AssetStore::open(d.path()) {
            Err(AssetError::EsquemaFuturo {
                encontrada,
                soportada,
            }) => {
                assert_eq!(encontrada, 99);
                assert_eq!(soportada, ESQUEMA_VERSION);
            }
            otro => panic!("se abrio un esquema futuro: {otro:?}"),
        }

        // Y la salida de emergencia sí lo rehace, porque el diario sigue ahí.
        assert!(AssetStore::open_rebuilding(d.path()).is_ok());
    }
}
