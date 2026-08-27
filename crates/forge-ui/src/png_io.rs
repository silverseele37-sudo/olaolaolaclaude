//! Escritura de PNG para el modo sin ventana (`forge --png`).
//!
//! Delegada al crate `png`: un códec de imagen no es el objeto de esta tarea,
//! y una implementación de mano (zlib, CRC32, Adler32) es exactamente el tipo
//! de código que falla en silencio en un byte suelto. `png` es una dependencia
//! externa (no un crate `forge-*`), así que no la alcanza el guardia de
//! `tests/arquitectura.rs`.

use std::io::BufWriter;
use std::path::Path;

use crate::error::{Result, UiError};

/// Escribe un búfer RGBA8 (`ancho * alto * 4` bytes, fila a fila desde arriba)
/// como PNG en `ruta`.
pub fn escribir_png(ruta: &Path, ancho: u32, alto: u32, rgba: &[u8]) -> Result<()> {
    if ancho == 0 || alto == 0 {
        return Err(UiError::DimensionesInvalidas { ancho, alto });
    }
    let esperado = ancho as usize * alto as usize * 4;
    if rgba.len() != esperado {
        return Err(UiError::Codec(format!(
            "el búfer tiene {} bytes, se esperaban {esperado} para {ancho}x{alto} RGBA8",
            rgba.len()
        )));
    }

    let archivo = std::fs::File::create(ruta).map_err(|e| UiError::Png {
        ruta: ruta.to_path_buf(),
        fuente: e,
    })?;
    let w = BufWriter::new(archivo);

    let mut encoder = png::Encoder::new(w, ancho, alto);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| UiError::Codec(e.to_string()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| UiError::Codec(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribe_y_relee_un_png_identico_al_original() {
        let dir = std::env::temp_dir().join(format!("forge-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("prueba.png");

        let (ancho, alto) = (4u32, 3u32);
        let mut rgba = vec![0u8; (ancho * alto * 4) as usize];
        // patrón conocido: cada píxel lleva su índice en el canal rojo
        for (i, chunk) in rgba.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&[i as u8, 10, 20, 255]);
        }

        escribir_png(&ruta, ancho, alto, &rgba).expect("escritura");

        let archivo = std::fs::File::open(&ruta).expect("el archivo existe");
        // BufReader: png::Decoder exige BufRead, y un File a pelo no lo es.
        let decoder = png::Decoder::new(std::io::BufReader::new(archivo));
        let mut lector = decoder.read_info().expect("cabecera valida");
        assert_eq!(lector.info().width, ancho);
        assert_eq!(lector.info().height, alto);

        // En png 0.18 devuelve Option: el tamano puede desbordar usize.
        let tam = lector
            .output_buffer_size()
            .expect("tamano de buffer representable");
        let mut buf = vec![0u8; tam];
        let info = lector.next_frame(&mut buf).expect("frame valido");
        assert_eq!(&buf[..info.buffer_size()], &rgba[..]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rechaza_un_bufer_de_tamano_incorrecto() {
        let dir = std::env::temp_dir().join(format!("forge-ui-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("malo.png");
        let err = escribir_png(&ruta, 4, 4, &[0u8; 3]).unwrap_err();
        assert!(matches!(err, UiError::Codec(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rechaza_dimensiones_en_cero() {
        let ruta = std::env::temp_dir().join("no-deberia-crearse.png");
        let err = escribir_png(&ruta, 0, 10, &[]).unwrap_err();
        assert!(matches!(err, UiError::DimensionesInvalidas { .. }));
    }
}
