//! El reproductor, contra un documento con geometría de verdad.
//!
//! Los dos tests que había usaban un documento **vacío**, así que pasaban con
//! una implementación que no cargaba nada: `cargar_geometria` se saltaba en
//! silencio los blobs que no sabía decodificar, `renderizar` construía la lista
//! de instancias vacía a propósito («sin mallas cargadas»), y `main` cargaba los
//! blobs en un almacén que dejaba morir antes de dibujar. Con una escena vacía
//! los tres fallos son invisibles.

use forge_doc::{Document, Geometry, GeometryPayload, Name, Transform, Visible};
use forge_interop::gltf::{to_glb, GltfOptions};
use forge_interop::TriangleSoup;
use forge_math::DVec3;
use forge_store::{BlobStore, MemoryBlobStore};

/// Un tetraedro de 100 mm: 4 triángulos, sin dos caras coplanares, así que
/// cualquier permutación de ejes se nota en la caja.
fn tetraedro() -> TriangleSoup {
    TriangleSoup {
        name: "tetraedro".into(),
        positions: vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(0.0, 60.0, 0.0),
            DVec3::new(0.0, 0.0, 30.0),
        ],
        indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
        ..Default::default()
    }
}

/// Escribe la malla como un blob del documento: GLB con las unidades de FORGE.
fn blob_de_malla(blobs: &MemoryBlobStore, m: &TriangleSoup) -> forge_store::BlobHash {
    let glb = to_glb(m, GltfOptions::crudo()).expect("escribir glb");
    blobs.put(&glb).expect("guardar blob")
}

fn escena_de_una_malla() -> (Document, MemoryBlobStore) {
    let blobs = MemoryBlobStore::new();
    let h = blob_de_malla(&blobs, &tetraedro());
    let mut doc = Document::new();
    doc.edit("pieza", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("pieza".into()));
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Mesh(h)));
        tx.set(e, Visible(true));
    });
    (doc, blobs)
}

#[test]
fn las_estadisticas_cuentan_los_triangulos_y_la_caja_de_verdad() {
    let (doc, blobs) = escena_de_una_malla();
    let s = forge_runtime::calcular_estadisticas(&doc.snapshot(), &blobs).unwrap();

    assert_eq!(s.entidades, 1);
    assert_eq!(s.instancias, 1);
    assert_eq!(s.triangulos, 4, "el tetraedro tiene 4 caras");

    // La caja es la del tetraedro **en milímetros y con Z arriba**. Si el blob
    // se hubiera leído con las opciones de glTF saldría permutada y ×1000.
    assert!(
        (s.caja.min - DVec3::ZERO).length() < 1e-3
            && (s.caja.max - DVec3::new(100.0, 60.0, 30.0)).length() < 1e-3,
        "caja {:?} .. {:?}",
        s.caja.min,
        s.caja.max
    );
}

#[test]
fn una_malla_instanciada_tres_veces_son_tres_instancias_y_un_solo_blob() {
    let blobs = MemoryBlobStore::new();
    let h = blob_de_malla(&blobs, &tetraedro());
    let mut doc = Document::new();
    doc.edit("tres", |tx| {
        for i in 0..3 {
            let e = tx.spawn();
            tx.set(e, Transform::from_translation(DVec3::new(200.0 * i as f64, 0.0, 0.0)));
            tx.set(e, Geometry(GeometryPayload::Mesh(h)));
            tx.set(e, Visible(true));
        }
    });

    let s = forge_runtime::calcular_estadisticas(&doc.snapshot(), &blobs).unwrap();
    assert_eq!(s.instancias, 3);
    assert_eq!(s.triangulos, 12, "3 instancias x 4 caras");
    // La caja abarca las tres, o sea que la transformada de mundo se aplicó.
    assert!(
        (s.caja.max.x - 500.0).abs() < 1e-3,
        "la caja no llega a la tercera copia: {:?}",
        s.caja.max
    );
}

#[test]
fn una_entidad_invisible_no_se_dibuja() {
    let blobs = MemoryBlobStore::new();
    let h = blob_de_malla(&blobs, &tetraedro());
    let mut doc = Document::new();
    doc.edit("oculta", |tx| {
        let e = tx.spawn();
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Mesh(h)));
        tx.set(e, Visible(false));
    });
    let s = forge_runtime::calcular_estadisticas(&doc.snapshot(), &blobs).unwrap();
    assert_eq!(s.instancias, 0);
    assert_eq!(s.triangulos, 0);
}

/// El test que importa: la imagen no puede salir en blanco.
///
/// Es lo único que separa «el reproductor funciona» de «el reproductor abre el
/// archivo, no dibuja nada y dice que ha ido bien», que es exactamente lo que
/// hacía. Se compara contra el render de un documento vacío: si la escena con
/// geometría diera lo mismo que la escena sin nada, no se estaría dibujando.
#[test]
fn renderizar_una_escena_con_geometria_no_da_una_imagen_vacia() {
    let (doc, blobs) = escena_de_una_malla();
    let con = forge_runtime::renderizar(&doc.snapshot(), &blobs, (64, 64)).unwrap();

    let vacio = Document::new();
    let sin = forge_runtime::renderizar(&vacio.snapshot(), &MemoryBlobStore::new(), (64, 64))
        .unwrap();

    assert_eq!(con.len(), 64 * 64 * 4);
    assert_ne!(con, sin, "la escena con geometria salio igual que una vacia");

    let distintos = con
        .chunks_exact(4)
        .zip(sin.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        distintos > 64 * 64 / 100,
        "solo {distintos} pixeles de {} cambiaron: la malla apenas se ve",
        64 * 64
    );
}

#[test]
fn renderizar_es_determinista() {
    let (doc, blobs) = escena_de_una_malla();
    let a = forge_runtime::renderizar(&doc.snapshot(), &blobs, (48, 48)).unwrap();
    let b = forge_runtime::renderizar(&doc.snapshot(), &blobs, (48, 48)).unwrap();
    assert_eq!(a, b);
}

/// Un blob que falta es un error, no un objeto que se salta.
///
/// La versión anterior devolvía `Ok` con la escena incompleta y sin decir nada.
#[test]
fn un_blob_que_falta_es_un_error_y_no_una_escena_a_medias() {
    let (doc, _) = escena_de_una_malla();
    let vacio = MemoryBlobStore::new(); // el almacen equivocado, sin los blobs
    match forge_runtime::calcular_estadisticas(&doc.snapshot(), &vacio) {
        Err(forge_runtime::RuntimeError::BlobAusente { .. }) => {}
        otro => panic!("se acepto un documento sin sus blobs: {otro:?}"),
    }
}

/// Y un blob que no es un GLB tampoco pasa por bueno.
#[test]
fn un_blob_que_no_es_glb_da_un_error_que_dice_cual() {
    let blobs = MemoryBlobStore::new();
    let h = blobs.put(b"<malla de la columna: 12k triangulos>").unwrap();
    let mut doc = Document::new();
    doc.edit("basura", |tx| {
        let e = tx.spawn();
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Mesh(h)));
        tx.set(e, Visible(true));
    });
    match forge_runtime::calcular_estadisticas(&doc.snapshot(), &blobs) {
        Err(forge_runtime::RuntimeError::MallaIlegible { hash, .. }) => assert_eq!(hash, h),
        otro => panic!("se acepto un blob que no es un GLB: {otro:?}"),
    }
}

/// Control de la decisión de formato: leer el blob con las opciones de glTF en
/// vez de las de FORGE da una escena distinta, no un fallo.
///
/// Por eso §4.1 del formato es normativa: el error no avisa, solo deja la
/// geometría en otro sitio y a otra escala.
#[test]
fn leer_el_blob_con_las_opciones_de_gltf_daria_otra_geometria() {
    let m = tetraedro();
    let glb = to_glb(&m, GltfOptions::crudo()).unwrap();

    let bien = forge_interop::gltf::read_glb(&glb, GltfOptions::crudo()).unwrap();
    let mal = forge_interop::gltf::read_glb(&glb, GltfOptions::default()).unwrap();

    assert!((bien.positions[1] - m.positions[1]).length() < 1e-3);
    assert!(
        (mal.positions[1] - m.positions[1]).length() > 1.0,
        "las dos lecturas coinciden: el control no vale"
    );
}
