//! Criterio de aceptación de la Fase 1 para la serialización.
//!
//! «Round-trip guardar/cargar con igualdad estructural sobre 20 documentos
//! generados.»
//!
//! La igualdad se comprueba por huella canónica, no campo a campo: la huella
//! cubre entidades, componentes y su contenido de una vez, y falla igual si se
//! pierde un dato que si se reordena algo que debería ser estable.

use std::sync::Arc;

use forge_doc::*;
use forge_io::*;
use forge_math::{DQuat, DVec3};
use forge_store::{BlobHash, BlobStore, MemoryBlobStore};

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
    fn f64(&mut self) -> f64 {
        (self.below(2_000_000) as f64) / 1000.0 - 1000.0
    }
}

/// 20 documentos con formas deliberadamente distintas. Los seis primeros son
/// casos límite elegidos a mano; el resto, mezclas pseudoaleatorias.
fn documento(n: usize, blobs: &dyn BlobStore) -> Document {
    let mut doc = Document::new();
    let mut rng = Rng::new(0xD0C_u64.wrapping_mul(n as u64 + 1));
    let poner_blob = |datos: &[u8]| -> BlobHash { blobs.put(datos).unwrap() };

    match n {
        // vacio: el caso que casi nadie prueba y que casi siempre falla
        0 => {}
        // una sola entidad con un solo componente
        1 => {
            doc.edit("una", |tx| {
                let e = tx.spawn_with_id(EntityId::from_u128(1));
                tx.set(e, Name("solitaria".into()));
            });
        }
        // entidad sin ningun componente: existe pero no tiene datos
        2 => {
            doc.edit("desnuda", |tx| {
                tx.spawn_with_id(EntityId::from_u128(1));
            });
        }
        // cadena profunda de padres
        3 => {
            doc.edit("cadena", |tx| {
                for i in 1..=50u128 {
                    let e = tx.spawn_with_id(EntityId::from_u128(i));
                    tx.set(e, Transform::from_translation(DVec3::new(0.0, 0.0, 10.0)));
                    if i > 1 {
                        tx.set(e, Parent(EntityId::from_u128(i - 1)));
                    }
                }
            });
        }
        // muchas referencias al MISMO blob: la deduplicacion tiene que notarse
        4 => {
            let h = poner_blob(b"la misma malla instanciada cien veces");
            doc.edit("instancias", |tx| {
                for i in 1..=100u128 {
                    let e = tx.spawn_with_id(EntityId::from_u128(i));
                    tx.set(e, Geometry(GeometryPayload::Mesh(h)));
                    tx.set(e, Visible(i % 3 != 0));
                }
            });
        }
        // texto no ASCII y valores f64 extremos
        5 => {
            doc.edit("bordes", |tx| {
                let e = tx.spawn_with_id(EntityId::from_u128(1));
                tx.set(e, Name("pieza «ñandú» — 図面 · 🔩 · \"comillas\" y \\barras".into()));
                let mut t = Transform::IDENTITY;
                t.translation = DVec3::new(f64::MAX, f64::MIN_POSITIVE, -0.0);
                t.scale = DVec3::new(1e-300, 1e300, 1.0);
                t.rotation = DQuat::from_xyzw(0.0, 0.0, 0.0, 1.0);
                tx.set(e, t);
                let f = tx.spawn_with_id(EntityId::from_u128(2));
                tx.set(f, Name(String::new()));
                let mut t2 = Transform::IDENTITY;
                t2.translation = DVec3::new(f64::INFINITY, f64::NEG_INFINITY, 0.0);
                tx.set(f, t2);
            });
        }
        // mezclas
        _ => {
            let cuantas = 5 + rng.below(120);
            let blobs_unicos: Vec<BlobHash> =
                (0..1 + rng.below(6)).map(|k| poner_blob(format!("blob-{n}-{k}").as_bytes())).collect();
            doc.edit("generado", |tx| {
                for i in 1..=cuantas as u128 {
                    let e = tx.spawn_with_id(EntityId::from_u128(i));
                    if rng.below(10) < 8 {
                        tx.set(e, Name(format!("pieza {i}")));
                    }
                    if rng.below(10) < 9 {
                        let mut t = Transform::IDENTITY;
                        t.translation = DVec3::new(rng.f64(), rng.f64(), rng.f64());
                        t.rotation = DQuat::from_rotation_z(rng.f64() * 0.001);
                        tx.set(e, t);
                    }
                    if rng.below(10) < 5 {
                        tx.set(e, Visible(rng.below(2) == 0));
                    }
                    if rng.below(10) < 6 {
                        let h = blobs_unicos[rng.below(blobs_unicos.len() as u64)];
                        let g = if rng.below(2) == 0 {
                            GeometryPayload::Brep(h)
                        } else {
                            GeometryPayload::Mesh(h)
                        };
                        tx.set(e, Geometry(g));
                    }
                    if i > 1 && rng.below(10) < 4 {
                        tx.set(e, Parent(EntityId::from_u128(1 + rng.below(i as u64) as u128)));
                    }
                }
            });
        }
    }
    doc
}

#[test]
fn ida_y_vuelta_con_igualdad_estructural_en_20_documentos() {
    let d = tempfile::tempdir().unwrap();
    let mut huellas = Vec::new();

    for n in 0..20 {
        let escritos = MemoryBlobStore::new();
        let doc = documento(n, &escritos);
        let snap = doc.snapshot();
        let huella = snap.fingerprint();
        huellas.push(huella);

        let p = d.path().join(format!("doc{n}.forge"));
        save(&p, &snap, &escritos, &SaveOptions::default()).unwrap();

        // Almacen NUEVO y vacio: el archivo tiene que traerse sus propios blobs.
        // Reusar el almacen de escritura haria pasar el test aunque no se
        // guardara ni un byte de geometria.
        let leidos = MemoryBlobStore::new();
        let cargado = load(&p, Arc::new(ComponentRegistry::new()), &leidos).unwrap();

        assert_eq!(
            cargado.snapshot().fingerprint(),
            huella,
            "el documento {n} no sobrevivio la ida y vuelta"
        );
        assert_eq!(cargado.snapshot().entity_count(), snap.entity_count());

        // El archivo empaqueta EXACTAMENTE los blobs referenciados: ni menos
        // (faltaria geometria) ni mas (arrastraria basura del almacen de
        // sesion, que puede tener importaciones descartadas).
        assert_eq!(
            leidos.list().unwrap(),
            snap.referenced_blobs(),
            "el documento {n} no empaqueto exactamente sus blobs referenciados"
        );
        for h in snap.referenced_blobs() {
            assert!(escritos.has(h).unwrap());
        }
    }

    // Los 20 documentos tienen que ser realmente distintos entre si.
    let distintas: std::collections::BTreeSet<_> = huellas.iter().collect();
    assert_eq!(distintas.len(), 20, "hay documentos generados identicos: el test cubre menos de lo que dice");
}

/// La deduplicación del almacén se propaga al archivo sin código extra: cien
/// entidades que instancian la misma malla guardan **un** blob.
#[test]
fn cien_instancias_de_una_malla_guardan_un_solo_blob() {
    let d = tempfile::tempdir().unwrap();
    let escritos = MemoryBlobStore::new();
    let doc = documento(4, &escritos);
    let p = d.path().join("instancias.forge");
    save(&p, &doc.snapshot(), &escritos, &SaveOptions::default()).unwrap();

    let f = std::fs::File::open(&p).unwrap();
    let zip = zip::ZipArchive::new(f).unwrap();
    let blobs: Vec<&str> = zip.file_names().filter(|n| n.starts_with("blobs/")).collect();
    assert_eq!(blobs.len(), 1, "se guardaron {} blobs para una sola malla", blobs.len());
    assert_eq!(doc.snapshot().iter::<Geometry>().count(), 100);
}

#[test]
fn el_manifiesto_declara_unidades_y_ejes() {
    let d = tempfile::tempdir().unwrap();
    let s = MemoryBlobStore::new();
    let p = d.path().join("m.forge");
    save(&p, &documento(1, &s).snapshot(), &s, &SaveOptions::default()).unwrap();

    let m = read_manifest(&p).unwrap();
    assert_eq!(m.format, "forge");
    assert_eq!(m.format_version, FORMAT_VERSION);
    assert_eq!(m.units, "mm");
    assert_eq!(m.up_axis, "Z", "la convencion de ejes tiene que estar EN el archivo");
    assert_eq!(m.tolerance_confusion_mm, 1e-7);
}

/// El formato es abierto de verdad: se puede abrir con `unzip` y ver la
/// estructura sin ninguna herramienta de FORGE.
#[test]
fn el_archivo_es_inspeccionable_con_herramientas_estandar() {
    let d = tempfile::tempdir().unwrap();
    let s = MemoryBlobStore::new();
    let p = d.path().join("abierto.forge");
    save(&p, &documento(7, &s).snapshot(), &s, &SaveOptions::default()).unwrap();

    let f = std::fs::File::open(&p).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let nombres: Vec<String> = zip.file_names().map(String::from).collect();
    for esperado in ["manifest.json", "document.cbor", "document.json", "refs/history"] {
        assert!(nombres.iter().any(|n| n == esperado), "falta {esperado} en el contenedor");
    }
    // el manifiesto va primero y sin comprimir
    assert_eq!(zip.by_index(0).unwrap().name(), "manifest.json");
    assert_eq!(zip.by_index(0).unwrap().compression(), zip::CompressionMethod::Stored);
    // y document.json es JSON valido
    let mut s2 = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("document.json").unwrap(), &mut s2).unwrap();
    serde_json::from_str::<serde_json::Value>(&s2).unwrap();
}

/// Un componente que esta build no conoce es un error explícito, no un campo
/// que se ignora. Ignorarlo perdería datos del usuario al volver a guardar.
#[test]
fn componente_desconocido_falla_en_vez_de_perder_datos() {
    let d = tempfile::tempdir().unwrap();
    let s = MemoryBlobStore::new();
    let p = d.path().join("x.forge");
    save(&p, &documento(1, &s).snapshot(), &s, &SaveOptions::default()).unwrap();

    // registro vacio: no conoce ni forge.name
    let r = load(&p, Arc::new(ComponentRegistry::empty()), &MemoryBlobStore::new());
    match r {
        Err(IoError::Doc(DocError::UnknownComponent(n))) => assert_eq!(n, "forge.name"),
        otro => panic!("se esperaba UnknownComponent, salio {otro:?}"),
    }
}

#[test]
fn version_de_formato_futura_se_rechaza_con_un_mensaje_util() {
    let d = tempfile::tempdir().unwrap();
    let s = MemoryBlobStore::new();
    let p = d.path().join("futuro.forge");
    save(&p, &documento(1, &s).snapshot(), &s, &SaveOptions::default()).unwrap();

    // reescribir el zip con format_version = 999
    let contenido = std::fs::read(&p).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(contenido)).unwrap();
    let salida = std::fs::File::create(&p).unwrap();
    let mut zout = zip::ZipWriter::new(salida);
    let op = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let nombre = f.name().to_string();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
        if nombre == "manifest.json" {
            let mut m: serde_json::Value = serde_json::from_slice(&buf).unwrap();
            m["format_version"] = serde_json::json!(999);
            buf = serde_json::to_vec_pretty(&m).unwrap();
        }
        zout.start_file(nombre, op).unwrap();
        std::io::Write::write_all(&mut zout, &buf).unwrap();
    }
    zout.finish().unwrap();

    match read_manifest(&p) {
        Err(IoError::VersionFutura { encontrada, soportada }) => {
            assert_eq!(encontrada, 999);
            assert_eq!(soportada, FORMAT_VERSION);
        }
        otro => panic!("se esperaba VersionFutura, salio {otro:?}"),
    }
}

/// Sin blobs empaquetados, el archivo es pequeño pero solo abre donde el
/// almacén ya los tiene. El error tiene que decir exactamente eso.
#[test]
fn guardar_sin_blobs_falla_al_cargar_en_un_almacen_vacio() {
    let d = tempfile::tempdir().unwrap();
    let escritos = MemoryBlobStore::new();
    let doc = documento(4, &escritos);
    let p = d.path().join("flaco.forge");
    let opts = SaveOptions { include_blobs: false, ..Default::default() };
    save(&p, &doc.snapshot(), &escritos, &opts).unwrap();

    match load(&p, Arc::new(ComponentRegistry::new()), &MemoryBlobStore::new()) {
        Err(IoError::BlobAusente(_)) => {}
        otro => panic!("se esperaba BlobAusente, salio {otro:?}"),
    }
    // pero con el almacen que ya los tiene, carga perfecto
    let ok = load(&p, Arc::new(ComponentRegistry::new()), &escritos).unwrap();
    assert_eq!(ok.snapshot().fingerprint(), doc.snapshot().fingerprint());
}
