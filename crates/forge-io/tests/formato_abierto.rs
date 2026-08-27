//! La especificación del formato no puede quedarse desfasada.
//!
//! `docs/formato/lector_referencia.py` lee un `.forge` con solo la biblioteca
//! estándar de Python. Este test lo ejecuta contra un documento recién
//! generado: si el formato cambia sin actualizar el lector —o si la
//! especificación que el lector implementa es incorrecta— el build falla.
//!
//! Es lo que separa «formato abierto» de una declaración de intenciones.

use std::process::Command;
use std::sync::Arc;

use forge_doc::{
    ComponentRegistry, Document, Geometry, GeometryPayload, Name, Parent, Transform, Visible,
};
use forge_io::{save, SaveOptions};
use forge_math::DVec3;
use forge_store::{BlobStore, MemoryBlobStore};

fn hay_python() -> Option<String> {
    for c in ["python3", "python"] {
        if Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Some(c.to_string());
        }
    }
    None
}

#[test]
fn el_lector_de_referencia_en_python_lee_un_documento_recien_generado() {
    let Some(python) = hay_python() else {
        eprintln!("aviso: sin interprete de Python; el test de la especificacion no corrio");
        return;
    };

    let blobs = MemoryBlobStore::new();
    let malla = blobs.put(b"<malla>").unwrap();
    let solido = blobs.put(b"<brep>").unwrap();

    let mut doc = Document::new();
    let raiz = doc.edit("raiz", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("conjunto".into()));
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Brep(solido)));
        tx.set(e, Visible(true));
        e
    });
    doc.edit("hijos", |tx| {
        for i in 0..3 {
            let e = tx.spawn();
            tx.set(e, Name(format!("pieza «ñ» {i}")));
            tx.set(e, Parent(raiz));
            tx.set(
                e,
                Transform::from_translation(DVec3::new(i as f64 * 10.0, 0.0, 0.0)),
            );
            tx.set(e, Geometry(GeometryPayload::Mesh(malla)));
            tx.set(e, Visible(i != 1));
        }
    });

    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("spec.forge");
    save(&p, &doc.snapshot(), &blobs, &SaveOptions::default()).unwrap();

    // el documento tiene que cargar tambien en FORGE, obviamente
    let recargado = forge_io::load(
        &p,
        Arc::new(ComponentRegistry::new()),
        &MemoryBlobStore::new(),
    )
    .unwrap();
    assert_eq!(
        recargado.snapshot().fingerprint(),
        doc.snapshot().fingerprint()
    );

    let lector = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/formato/lector_referencia.py");
    assert!(
        lector.exists(),
        "falta el lector de referencia en {}",
        lector.display()
    );

    let salida = Command::new(&python).arg(&lector).arg(&p).output().unwrap();
    let stdout = String::from_utf8_lossy(&salida.stdout);
    let stderr = String::from_utf8_lossy(&salida.stderr);
    assert!(
        salida.status.success(),
        "el lector de referencia fallo sobre un .forge valido.\n\
         O el formato cambio sin actualizar docs/formato/, o la especificacion es incorrecta.\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    // y tiene que haber entendido el contenido, no solo no reventar
    for esperado in [
        "forge v1",
        "eje vertical  Z",
        "entidades 4",
        "blobs     2",
        "conjunto",
    ] {
        assert!(
            stdout.contains(esperado),
            "el lector no reporto {esperado:?}:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("pieza «ñ» 0"),
        "el texto no ASCII no sobrevivio:\n{stdout}"
    );
}
