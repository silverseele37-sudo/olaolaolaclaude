//! Escribe un `.forge` con geometría de verdad y lo dibuja, sin editor.
//!
//! Es el camino completo del pilar 3: documento → blobs GLB → extracción de
//! instancias → rasterizador → imagen. Sirve de demo reproducible y, sobre
//! todo, de comprobación de que las piezas encajan fuera de los tests.
//!
//! ```text
//! cargo run -p forge-runtime --example escena_demo
//! cargo run -p forge-runtime -- /tmp/forge-demo/escena.forge --ppm /tmp/forge-demo/escena.ppm
//! ```

use forge_doc::{Document, Geometry, GeometryPayload, Name, Parent, Transform, Visible};
use forge_interop::gltf::{to_glb, GltfOptions};
use forge_interop::TriangleSoup;
use forge_io::SaveOptions;
use forge_math::DVec3;
use forge_store::{BlobStore, MemoryBlobStore};

/// Caja de `lado` mm en el origen: 12 triángulos con normales por vértice.
fn caja(lado: f64) -> TriangleSoup {
    let h = lado * 0.5;
    let esquinas = [
        DVec3::new(-h, -h, -h),
        DVec3::new(h, -h, -h),
        DVec3::new(h, h, -h),
        DVec3::new(-h, h, -h),
        DVec3::new(-h, -h, h),
        DVec3::new(h, -h, h),
        DVec3::new(h, h, h),
        DVec3::new(-h, h, h),
    ];
    let caras: [([usize; 4], DVec3); 6] = [
        ([0, 3, 2, 1], -DVec3::Z),
        ([4, 5, 6, 7], DVec3::Z),
        ([0, 1, 5, 4], -DVec3::Y),
        ([2, 3, 7, 6], DVec3::Y),
        ([1, 2, 6, 5], DVec3::X),
        ([0, 4, 7, 3], -DVec3::X),
    ];
    let mut m = TriangleSoup {
        name: "caja".into(),
        ..Default::default()
    };
    for (bucle, n) in caras {
        let base = m.positions.len() as u32;
        for i in bucle {
            m.positions.push(esquinas[i]);
            m.normals.push(n);
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    m
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/tmp/forge-demo".into()),
    );
    std::fs::create_dir_all(&dir)?;
    let ruta = dir.join("escena.forge");

    let blobs = MemoryBlobStore::new();

    // El blob de una malla es un GLB con las unidades de FORGE: milímetros y Z
    // arriba. Ver docs/formato/README.md, §4.1 -- leerlo con las opciones de
    // glTF daría los ejes permutados y la escala mil veces mayor, sin error.
    let malla = blobs.put(&to_glb(&caja(80.0), GltfOptions::crudo())?)?;

    let mut doc = Document::new();
    let base = doc.edit("crear base", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("base".into()));
        tx.set(e, Transform::IDENTITY);
        e
    });
    doc.edit("tres columnas", |tx| {
        for i in 0..3 {
            let e = tx.spawn();
            tx.set(e, Name(format!("columna {}", i + 1)));
            tx.set(e, Parent(base));
            tx.set(
                e,
                Transform::from_translation(DVec3::new(120.0 * i as f64 - 120.0, 0.0, 40.0)),
            );
            // Las tres comparten blob: se decodifica una vez y se dibuja tres.
            tx.set(e, Geometry(GeometryPayload::Mesh(malla)));
            tx.set(e, Visible(true));
        }
    });

    let snap = doc.snapshot();
    forge_io::save(&ruta, &snap, &blobs, &SaveOptions::default())?;
    println!("escrito  {}", ruta.display());

    // Y ahora por el camino del reproductor: releer del disco, con su propio
    // almacén, como haría el binario.
    let leidos = MemoryBlobStore::new();
    let doc2 = forge_io::load(&ruta, forge_io::registro_por_defecto(), &leidos)?;
    let snap2 = doc2.snapshot();

    let s = forge_runtime::calcular_estadisticas(&snap2, &leidos)?;
    println!("entidades   {}", s.entidades);
    println!("instancias  {}", s.instancias);
    println!("triangulos  {}", s.triangulos);
    println!("caja        {:?} .. {:?} mm", s.caja.min, s.caja.max);

    let rgba = forge_runtime::renderizar(&snap2, &leidos, (400, 300))?;
    // El alfa sale a 255 en toda la imagen, fondo incluido, asi que no dice
    // nada. Lo que se cuenta es cuantos pixeles difieren del de la esquina, que
    // es fondo seguro.
    let fondo = &rgba[..4];
    let dibujados = rgba.chunks_exact(4).filter(|p| *p != fondo).count();
    println!("pixeles dibujados {dibujados} de {}", 400 * 300);

    let ppm = dir.join("escena.ppm");
    let mut bytes = b"P6\n400 300\n255\n".to_vec();
    for p in rgba.chunks_exact(4) {
        bytes.extend_from_slice(&p[..3]);
    }
    std::fs::write(&ppm, bytes)?;
    println!("escrito  {}", ppm.display());
    Ok(())
}
