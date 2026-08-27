//! El diario: el «registro mínimo» que, junto a los blobs, **es** la fuente de
//! verdad del almacén de activos.
//!
//! Formato: un objeto JSON por línea, solo añadidos, nunca reescrito. Tres
//! propiedades que importan y que un archivo binario no daría:
//!
//! - **Se inspecciona con `tail`.** Coherente con ADR-0003: un formato que nadie
//!   puede leer sin la aplicación no es abierto.
//! - **La corrupción es local.** Una línea ilegible pierde *un* evento; el resto
//!   del historial sigue siendo válido. Un índice binario corrupto pierde todo.
//! - **Una escritura interrumpida se distingue.** La cola sin `\n` final es un
//!   registro a medias: se ignora y no se consume, en vez de interpretarse mal.
//!
//! Los payloads no viven aquí: viven en el almacén de blobs, deduplicados por
//! contenido. El diario solo guarda hashes y metadatos, que es lo que lo mantiene
//! pequeño y lo que hace que reindexar sea releerlo entero sin tocar los blobs.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AssetError, AssetMeta, Result};

/// Cuánto se compromete cada escritura antes de devolver el control.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Durability {
    /// `sync_data` tras cada registro: un corte de energía no pierde nada
    /// confirmado. Es el valor por defecto porque el diario es la verdad.
    #[default]
    PerWrite,
    /// Sin `sync_data`: lo escrito puede perderse en un corte. Solo para cargas
    /// masivas (importar un directorio entero, poblar un conjunto de prueba),
    /// donde un fsync por activo domina el tiempo total.
    Deferred,
}

/// Un hecho registrado. Reproducir estos hechos en orden reconstruye el índice
/// entero: no hay ningún estado que viva solo en SQLite.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "evento", rename_all = "snake_case")]
pub enum Registro {
    /// Alta o versión nueva. Si el activo ya existe, `version` es la siguiente.
    Importado {
        id: String,
        version: u64,
        hash: String,
        tam: u64,
        origen: String,
        meta: AssetMeta,
        ts: i64,
    },
    /// Cambia la versión vigente sin tocar la historia (`revert`, o re-importar
    /// un contenido que ya era una versión conocida).
    Vigente { id: String, version: u64, ts: i64 },
    /// Metadatos nuevos, mismo contenido.
    Meta {
        id: String,
        meta: AssetMeta,
        ts: i64,
    },
    /// `pon = true` etiqueta, `false` desetiqueta.
    Etiqueta {
        id: String,
        etiqueta: String,
        pon: bool,
        ts: i64,
    },
    /// Hash de la miniatura. `None` la quita. Nunca la imagen: solo el hash.
    Miniatura {
        id: String,
        hash: Option<String>,
        ts: i64,
    },
    /// Arista dirigida `de -> a` ("de depende de a"). `pon = false` la quita.
    Dependencia {
        de: String,
        a: String,
        pon: bool,
        ts: i64,
    },
    /// Baja del activo. Sus blobs quedan huérfanos hasta el próximo `gc`.
    Borrado { id: String, ts: i64 },
}

/// Lo que se leyó del diario a partir de un desplazamiento.
pub struct Lectura {
    pub registros: Vec<Registro>,
    /// Líneas completas que no se pudieron interpretar. Se cuentan y se saltan:
    /// perder un evento no puede impedir leer los demás.
    pub ilegibles: u64,
    /// Bytes de la cola sin `\n`: una escritura interrumpida. No se consumen.
    pub cola_incompleta: u64,
    /// Desplazamiento tras la última línea completa.
    pub offset: u64,
}

pub struct Diario {
    ruta: PathBuf,
    f: File,
    /// Bytes escritos. Se mantiene aquí para no hacer `metadata()` por escritura.
    offset: u64,
    pub durabilidad: Durability,
}

impl Diario {
    pub fn abrir(ruta: impl Into<PathBuf>) -> Result<Self> {
        let ruta = ruta.into();
        let f = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&ruta)
            .map_err(|e| AssetError::io(&ruta, e))?;
        let offset = f.metadata().map_err(|e| AssetError::io(&ruta, e))?.len();
        Ok(Diario {
            ruta,
            f,
            offset,
            durabilidad: Durability::default(),
        })
    }

    /// Bytes escritos hasta ahora. El índice guarda este número para saber, en
    /// O(1) al abrir, si se quedó atrás respecto del diario.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn append(&mut self, r: &Registro) -> Result<()> {
        let mut linea =
            serde_json::to_string(r).map_err(|e| AssetError::RegistroIlegible(e.to_string()))?;
        linea.push('\n');
        self.f
            .write_all(linea.as_bytes())
            .map_err(|e| AssetError::io(&self.ruta, e))?;
        if self.durabilidad == Durability::PerWrite {
            self.f
                .sync_data()
                .map_err(|e| AssetError::io(&self.ruta, e))?;
        }
        self.offset += linea.len() as u64;
        Ok(())
    }

    /// Lee desde `desde` hasta el final. Solo consume líneas completas.
    pub fn leer_desde(&self, desde: u64) -> Result<Lectura> {
        let mut f = File::open(&self.ruta).map_err(|e| AssetError::io(&self.ruta, e))?;
        f.seek(SeekFrom::Start(desde))
            .map_err(|e| AssetError::io(&self.ruta, e))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
            .map_err(|e| AssetError::io(&self.ruta, e))?;

        let mut registros = Vec::new();
        let mut ilegibles = 0;
        let mut consumidos = 0usize;
        for linea in buf.split_inclusive(|b| *b == b'\n') {
            if linea.last() != Some(&b'\n') {
                break; // cola sin terminar: escritura a medias
            }
            consumidos += linea.len();
            let cuerpo = &linea[..linea.len() - 1];
            if cuerpo.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            match serde_json::from_slice::<Registro>(cuerpo) {
                Ok(r) => registros.push(r),
                Err(_) => ilegibles += 1,
            }
        }
        Ok(Lectura {
            registros,
            ilegibles,
            cola_incompleta: (buf.len() - consumidos) as u64,
            offset: desde + consumidos as u64,
        })
    }
}
