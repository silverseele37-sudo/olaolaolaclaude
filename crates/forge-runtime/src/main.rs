//! Reproductor de escenas .forge sin editor.
//!
//! Uso:
//!   forge-runtime archivo.forge --ppm salida.ppm
//!   forge-runtime archivo.forge --stats
//!   forge-runtime archivo.forge --ppm salida.ppm --size 1600x1000

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;

use forge_io;
use forge_runtime;
use forge_store::MemoryBlobStore;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Uso: {} <archivo.forge> [opciones]", args[0]);
        eprintln!("  --ppm <salida.ppm>    Renderizar a PPM");
        eprintln!("  --size WxH             Tamaño de salida (ej: 1600x1000)");
        eprintln!("  --stats                Mostrar estadísticas sin renderizar");
        process::exit(1);
    }

    let archivo = &args[1];

    // Valores por defecto
    let mut salida_ppm: Option<PathBuf> = None;
    let mut tamaño = (800u32, 600u32);
    let mut solo_stats = false;

    // Parsear argumentos
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--ppm" => {
                if i + 1 < args.len() {
                    salida_ppm = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("Error: --ppm requiere un archivo de salida");
                    process::exit(1);
                }
            }
            "--size" => {
                if i + 1 < args.len() {
                    if let Some((w, h)) = parsear_tamaño(&args[i + 1]) {
                        tamaño = (w, h);
                        i += 2;
                    } else {
                        eprintln!("Error: tamaño inválido '{}', use WxH", args[i + 1]);
                        process::exit(1);
                    }
                } else {
                    eprintln!("Error: --size requiere un tamaño");
                    process::exit(1);
                }
            }
            "--stats" => {
                solo_stats = true;
                i += 1;
            }
            _ => {
                eprintln!("Error: argumento desconocido '{}'", args[i]);
                process::exit(1);
            }
        }
    }

    // Cargar el archivo
    let ruta = PathBuf::from(archivo);
    let doc = match cargar_documento(&ruta) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error al cargar '{}': {}", archivo, e);
            process::exit(1);
        }
    };

    let snap = doc.snapshot();
    let blobs = MemoryBlobStore::new();

    // Calcular estadísticas
    let stats = match forge_runtime::calcular_estadisticas(&snap, &blobs) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error al calcular estadísticas: {}", e);
            process::exit(1);
        }
    };

    println!("Documento: {}", ruta.display());
    println!("  Entidades: {}", stats.entidades);
    println!("  Instancias: {}", stats.instancias);
    println!("  Triángulos: {}", stats.triangulos);
    if let Some(bbox) = stats.bounding_box {
        println!("  Bounding box: {:?}", bbox);
    }

    // Si solo queremos estadísticas, terminamos aquí
    if solo_stats {
        return;
    }

    // Renderizar si se especificó salida
    if let Some(salida) = salida_ppm {
        match renderizar_y_guardar(&snap, &blobs, tamaño, &salida) {
            Ok(()) => {
                println!("Renderizado a: {}", salida.display());
            }
            Err(e) => {
                eprintln!("Error al renderizar: {}", e);
                process::exit(1);
            }
        }
    }
}

/// Carga un documento .forge.
fn cargar_documento(ruta: &std::path::Path) -> forge_runtime::Result<forge_doc::Document> {
    // Crear un blob store para los blobs del archivo
    let blobs = MemoryBlobStore::new();
    let registry = forge_io::registro_por_defecto();

    forge_io::load(ruta, registry, &blobs)
        .map_err(|e| format!("Error de E/S: {}", e).into())
}

/// Renderiza el snapshot y lo guarda como PPM.
fn renderizar_y_guardar(
    snap: &forge_doc::Snapshot,
    blobs: &dyn forge_store::BlobStore,
    tamaño: (u32, u32),
    salida: &std::path::Path,
) -> forge_runtime::Result<()> {
    let bytes_rgba = forge_runtime::renderizar(snap, blobs, tamaño)?;
    guardar_ppm(salida, tamaño.0, tamaño.1, &bytes_rgba)?;
    Ok(())
}

/// Parsea un string de tamaño "WxH" a (ancho, alto).
fn parsear_tamaño(s: &str) -> Option<(u32, u32)> {
    let partes: Vec<&str> = s.split('x').collect();
    if partes.len() == 2 {
        let ancho = partes[0].parse::<u32>().ok()?;
        let alto = partes[1].parse::<u32>().ok()?;
        if ancho > 0 && alto > 0 {
            return Some((ancho, alto));
        }
    }
    None
}

/// Escribe una imagen PPM en formato P6 (raw RGB).
/// Entrada: bytes RGBA (4 bytes por píxel).
fn guardar_ppm(
    ruta: &std::path::Path,
    ancho: u32,
    alto: u32,
    rgba: &[u8],
) -> forge_runtime::Result<()> {
    let mut archivo = fs::File::create(ruta)
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("No se puede crear {}: {}", ruta.display(), e).into()
        })?;

    // Encabezado PPM P6
    write!(archivo, "P6\n")?;
    write!(archivo, "{} {}\n", ancho, alto)?;
    write!(archivo, "255\n")?;

    // Datos RGB (descartar alpha)
    let n_pixeles = (ancho * alto) as usize;
    let mut rgb = Vec::with_capacity(n_pixeles * 3);

    for i in 0..n_pixeles {
        rgb.push(rgba[i * 4]);     // R
        rgb.push(rgba[i * 4 + 1]); // G
        rgb.push(rgba[i * 4 + 2]); // B
    }

    archivo.write_all(&rgb)?;
    Ok(())
}
