//! Escritura atómica.
//!
//! Un editor que corrompe el archivo del usuario al fallar a mitad de guardado
//! pierde su confianza para siempre, y no hay funcionalidad que lo compense. El
//! patrón es viejo y sirve: escribir a un temporal **en el mismo directorio**
//! —para que el `rename` sea dentro del mismo sistema de archivos y por tanto
//! atómico—, sincronizar, y renombrar encima.
//!
//! Si algo falla en el camino, el temporal se borra y el archivo anterior queda
//! exactamente como estaba.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{IoError, Result};

/// Escribe `path` de forma atómica. `f` recibe el archivo temporal.
///
/// Garantías, y las tres se comprueban en los tests:
/// - si `f` devuelve error, `path` conserva su contenido anterior;
/// - no queda ningún temporal, ni en éxito ni en fallo;
/// - un lector que abra `path` concurrentemente ve el contenido viejo o el
///   nuevo, nunca uno a medias.
pub fn write_atomic<F>(path: &Path, f: F) -> Result<()>
where
    F: FnOnce(&mut File) -> Result<()>,
{
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| IoError::at(dir, e))?;

    let nombre = path.file_name().and_then(|n| n.to_str()).unwrap_or("forge");
    let tmp: PathBuf = dir.join(format!(".{}.{}.tmp", nombre, std::process::id()));

    let resultado = (|| -> Result<()> {
        let mut file = File::create(&tmp).map_err(|e| IoError::at(&tmp, e))?;
        f(&mut file)?;
        file.flush().map_err(|e| IoError::at(&tmp, e))?;
        // sync_all antes del rename: sin esto, un corte de energía puede dejar
        // el nombre nuevo apuntando a contenido sin escribir.
        file.sync_all().map_err(|e| IoError::at(&tmp, e))?;
        Ok(())
    })();

    match resultado {
        Ok(()) => {
            std::fs::rename(&tmp, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                IoError::at(path, e)
            })?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Temporales huérfanos que hayan quedado de un proceso muerto a lo bruto.
pub fn temporales_en(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(n) = e.file_name().to_str() {
                if n.starts_with('.') && n.ends_with(".tmp") {
                    out.push(e.path());
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn leer(p: &Path) -> String {
        let mut s = String::new();
        File::open(p).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn escritura_correcta_reemplaza_y_no_deja_temporales() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("doc.forge");
        write_atomic(&p, |f| {
            f.write_all(b"version 1").map_err(|e| IoError::at(&p, e))
        })
        .unwrap();
        assert_eq!(leer(&p), "version 1");

        write_atomic(&p, |f| {
            f.write_all(b"version 2 mas larga").map_err(|e| IoError::at(&p, e))
        })
        .unwrap();
        assert_eq!(leer(&p), "version 2 mas larga");
        assert!(temporales_en(d.path()).is_empty());
    }

    /// Inyección de fallo: el criterio de aceptación de la Fase 1.
    /// El escritor escribe la mitad y luego falla.
    #[test]
    fn fallo_a_mitad_de_escritura_deja_el_archivo_anterior_intacto() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("doc.forge");
        write_atomic(&p, |f| {
            f.write_all(b"contenido bueno").map_err(|e| IoError::at(&p, e))
        })
        .unwrap();

        let r = write_atomic(&p, |f| {
            f.write_all(b"basura a medio escribir").unwrap();
            Err(IoError::Corrupto("fallo inyectado".into()))
        });

        assert!(r.is_err(), "el fallo inyectado tiene que propagarse");
        assert_eq!(leer(&p), "contenido bueno", "el archivo anterior se corrompio");
        assert!(temporales_en(d.path()).is_empty(), "quedo un temporal huerfano");
    }

    /// Control: si el archivo no existía y la escritura falla, no debe quedar
    /// un archivo vacío ocupando el nombre.
    #[test]
    fn fallo_en_la_primera_escritura_no_crea_el_archivo() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nuevo.forge");
        let r = write_atomic(&p, |_| Err(IoError::Corrupto("fallo inyectado".into())));
        assert!(r.is_err());
        assert!(!p.exists(), "se creo un archivo vacio pese al fallo");
        assert!(temporales_en(d.path()).is_empty());
    }
}
