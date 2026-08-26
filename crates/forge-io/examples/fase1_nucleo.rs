//! Demo reproducible de la Fase 1 — núcleo de datos.
//!
//!     cargo run --example fase1_nucleo
//!
//! Recorre el camino completo sin GPU: construir una escena, ver la
//! deduplicación, guardar, cargar en un almacén vacío, comprobar igualdad
//! estructural, deshacer y rehacer, y ver que un fallo de escritura no toca el
//! archivo anterior.

use std::sync::Arc;

use forge_doc::{
    ComponentRegistry, DocEvent, Document, Geometry, GeometryPayload, Name, Parent, Transform,
    Visible,
};
use forge_io::{atomic, load, save, IoError, SaveOptions};
use forge_math::{chord_deflection, DVec3};
use forge_store::{BlobStore, MemoryBlobStore};

fn titulo(t: &str) {
    println!("\n\x1b[1m{t}\x1b[0m\n{}", "─".repeat(t.chars().count()));
}

fn main() -> forge_io::Result<()> {
    let blobs = MemoryBlobStore::new();
    let mut doc = Document::new();

    doc.subscribe(|ev| match ev {
        DocEvent::Committed { version, label } => println!("   · commit {version}  {label}"),
        DocEvent::Undone { to, undid } => println!("   · deshecho «{undid}» → {to}"),
        DocEvent::Redone { to, redid } => println!("   · rehecho  «{redid}» → {to}"),
    });

    // ------------------------------------------------------------------
    titulo("1. Construir una escena");

    // Una malla que se instancia tres veces, y un solido exacto.
    let malla = blobs.put(b"<malla de la columna: 12k triangulos>").unwrap();
    let solido = blobs.put(b"<B-Rep de la base: 1 solido, 14 caras>").unwrap();

    let base = doc.edit("crear base", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("base".into()));
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Brep(solido)));
        tx.set(e, Visible(true));
        e
    });

    for i in 0..3 {
        doc.edit(format!("columna {}", i + 1), |tx| {
            let e = tx.spawn();
            tx.set(e, Name(format!("columna {}", i + 1)));
            tx.set(e, Parent(base));
            tx.set(e, Transform::from_translation(DVec3::new(120.0 * i as f64, 0.0, 40.0)));
            tx.set(e, Geometry(GeometryPayload::Mesh(malla)));
            tx.set(e, Visible(true));
        });
    }

    let snap = doc.snapshot();
    println!("   entidades          {}", snap.entity_count());
    println!("   con geometria      {}", snap.iter::<Geometry>().count());
    for (e, g) in snap.iter::<Geometry>() {
        let nombre = snap.get::<Name>(e).map(|n| n.0.as_str()).unwrap_or("(sin nombre)");
        println!("     {:<12} {:<6} dominio {:?}", nombre, g.0.kind(), g.0.domain());
    }

    // ------------------------------------------------------------------
    titulo("2. Deduplicacion: cuatro geometrias, dos blobs");
    println!("   componentes Geometry     {}", snap.iter::<Geometry>().count());
    println!("   blobs referenciados      {}", snap.referenced_blobs().len());
    println!("   blobs en el almacen      {}", blobs.list().unwrap().len());
    println!("   bytes almacenados        {}", blobs.bytes());
    println!("   (las 3 columnas comparten un solo blob: el nombre ES el contenido)");

    // ------------------------------------------------------------------
    titulo("3. Jerarquia: la transformada acumulada");
    for (e, n) in snap.iter::<Name>() {
        let m = snap.world_transform(e);
        let p = m.transform_point3(DVec3::ZERO);
        println!("   {:<12} origen en mundo ({:7.1}, {:7.1}, {:7.1}) mm", n.0, p.x, p.y, p.z);
    }

    // ------------------------------------------------------------------
    titulo("4. Guardar");
    let dir = std::path::Path::new("target/demo");
    std::fs::create_dir_all(dir).map_err(|e| IoError::at(dir, e))?;
    let ruta = dir.join("soporte.forge");
    let opts = SaveOptions {
        history: doc.history().iter().map(|(v, l)| (*v, l.to_string())).collect(),
        ..Default::default()
    };
    save(&ruta, &snap, &blobs, &opts)?;
    let tam = std::fs::metadata(&ruta).map_err(|e| IoError::at(&ruta, e))?.len();
    println!("   {} ({tam} bytes)", ruta.display());
    let f = std::fs::File::open(&ruta).map_err(|e| IoError::at(&ruta, e))?;
    let zip = zip::ZipArchive::new(f).map_err(|e| IoError::Corrupto(e.to_string()))?;
    for n in zip.file_names() {
        println!("     {n}");
    }
    println!("   (es un ZIP: `unzip -l {}` funciona sin FORGE)", ruta.display());

    // ------------------------------------------------------------------
    titulo("5. Cargar en un almacen vacio y comparar");
    let frescos = MemoryBlobStore::new();
    let cargado = load(&ruta, Arc::new(ComponentRegistry::new()), &frescos)?;
    println!("   huella original    {}", snap.fingerprint());
    println!("   huella cargada     {}", cargado.snapshot().fingerprint());
    println!(
        "   igualdad estructural: {}",
        if cargado.snapshot().fingerprint() == snap.fingerprint() { "SI" } else { "NO" }
    );
    println!("   blobs traidos por el archivo: {}", frescos.list().unwrap().len());

    // ------------------------------------------------------------------
    titulo("6. Undo unificado");
    let antes = doc.snapshot().fingerprint();
    println!("   historial ({} pasos):", doc.history().len());
    for (v, l) in doc.history() {
        println!("     {v}  {l}");
    }
    println!("   deshaciendo dos veces…");
    doc.undo();
    doc.undo();
    println!("   entidades ahora    {}", doc.snapshot().entity_count());
    println!("   rehaciendo dos veces…");
    doc.redo();
    doc.redo();
    println!(
        "   vuelta al estado original: {}",
        if doc.snapshot().fingerprint() == antes { "SI" } else { "NO" }
    );

    titulo("7. Una operacion que cruza pilares es UN solo undo");
    let f0 = doc.snapshot().fingerprint();
    doc.edit("importar pieza", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("tornillo".into()));                          // escena
        tx.set(e, Transform::from_translation(DVec3::Z * 80.0));     // escena
        tx.set(e, Geometry(GeometryPayload::Brep(solido)));          // kernel
        tx.set(e, Visible(true));                                    // render
    });
    println!("   cuatro componentes de tres pilares en una transaccion");
    doc.undo();
    println!(
        "   un Ctrl+Z los revierte todos: {}",
        if doc.snapshot().fingerprint() == f0 { "SI" } else { "NO" }
    );

    // ------------------------------------------------------------------
    titulo("8. La escritura no puede corromper el archivo anterior");
    let antes_bytes = std::fs::read(&ruta).map_err(|e| IoError::at(&ruta, e))?;
    let r = atomic::write_atomic(&ruta, |f| {
        use std::io::Write;
        f.write_all(b"basura a medio escribir").unwrap();
        Err(IoError::Corrupto("fallo inyectado a proposito".into()))
    });
    println!("   escritura con fallo inyectado -> {}", r.unwrap_err());
    let despues = std::fs::read(&ruta).map_err(|e| IoError::at(&ruta, e))?;
    println!(
        "   archivo intacto: {}   temporales huerfanos: {}",
        if antes_bytes == despues { "SI" } else { "NO" },
        atomic::temporales_en(dir).len()
    );

    // ------------------------------------------------------------------
    titulo("9. Deflexion adaptativa (ADR-0002 R1b)");
    let fov = 45f64.to_radians();
    println!("   {:>8}  {:>14}  {:>14}", "zoom", "distancia mm", "deflexion mm");
    for z in [1.0, 4.0, 16.0, 64.0] {
        let d = 1000.0 / z;
        println!("   {:>7}x  {:>14.2}  {:>14.6}", z, d, chord_deflection(d, fov, 1080.0, 0.4));
    }
    println!("   el error geometrico se mantiene en 0.4 px a cualquier zoom");

    println!();
    Ok(())
}
