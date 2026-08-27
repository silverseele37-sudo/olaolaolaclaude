//! Criterios de aceptación del Pilar 4.
//!
//! Cada test de este archivo tiene una **respuesta conocida**: un recuento
//! exacto sobre un conjunto construido a mano, no una comprobación de que la
//! llamada no revienta. Y donde hay un mecanismo que podría estar apagado —la
//! recolección, la detección de ciclos, el escape de `LIKE`, la reconstrucción—
//! hay control positivo *y* negativo en el mismo test: uno solo de los dos lo
//! pasaría también una implementación que no hace nada.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use forge_assets::*;
use forge_store::{BlobHash, BlobStore};
use tempfile::TempDir;

/// Un instante fijo y redondo. Con reloj de sistema no se podrían afirmar
/// recuentos exactos en los filtros de fecha.
const T0: i64 = 1_700_000_000_000;

struct Banco {
    s: AssetStore,
    reloj: Arc<FixedClock>,
    dir: TempDir,
}

fn banco() -> Banco {
    let dir = tempfile::tempdir().expect("tempdir");
    let reloj = Arc::new(FixedClock::new(T0));
    let s = AssetStore::open_with(dir.path(), reloj.clone()).expect("abrir almacen");
    Banco { s, reloj, dir }
}

fn contenido(marca: u8, tam: usize) -> Vec<u8> {
    vec![marca; tam]
}

// ---------------------------------------------------------------------------
// Deduplicación
// ---------------------------------------------------------------------------

/// El mismo archivo por dos rutas: **un** blob, dos activos que lo comparten.
///
/// No hay código de deduplicación en este crate. Lo que se prueba aquí es que la
/// propiedad de `forge-store` —el nombre de un blob es su contenido— llega
/// intacta hasta el almacén de activos.
#[test]
fn el_mismo_archivo_por_dos_rutas_es_un_solo_blob() {
    let mut b = banco();
    let bytes = contenido(0xAB, 4096);

    let ruta1 = b.dir.path().join("copia-uno.png");
    let ruta2 = b.dir.path().join("copia-dos.png");
    std::fs::write(&ruta1, &bytes).expect("escribir 1");
    std::fs::write(&ruta2, &bytes).expect("escribir 2");

    let a = b.s.import(&ruta1, AssetMeta::new("copia uno", AssetType::Textura)).expect("import 1");
    let c = b.s.import(&ruta2, AssetMeta::new("copia dos", AssetType::Textura)).expect("import 2");

    assert_ne!(a, c, "dos rutas son dos activos distintos");
    assert_eq!(b.s.len().expect("len"), 2);
    assert_eq!(b.s.blobs().list().expect("listar blobs").len(), 1, "se duplico el contenido");

    let ha = b.s.get(a).expect("get a").expect("a existe").hash;
    let hc = b.s.get(c).expect("get c").expect("c existe").hash;
    assert_eq!(ha, hc, "los dos activos tienen que compartir el blob");
    assert_eq!(ha, BlobHash::of(&bytes));

    // Control negativo: un contenido distinto sí añade un blob. Sin esto, un
    // almacén que no guardara nada pasaría la mitad de arriba.
    let ruta3 = b.dir.path().join("otra.png");
    std::fs::write(&ruta3, contenido(0xCD, 4096)).expect("escribir 3");
    b.s.import(&ruta3, AssetMeta::new("otra", AssetType::Textura)).expect("import 3");
    assert_eq!(b.s.blobs().list().expect("listar blobs").len(), 2);
}

// ---------------------------------------------------------------------------
// Versiones
// ---------------------------------------------------------------------------

/// Tres contenidos distintos son tres versiones. Un cuarto idéntico al segundo
/// **no crea nada**: ni versión ni blob, porque el hash ya existía.
#[test]
fn tres_contenidos_distintos_son_tres_versiones_y_el_cuarto_repetido_ninguna() {
    let mut b = banco();
    let ruta = b.dir.path().join("pieza.stl");
    let c1 = contenido(1, 100);
    let c2 = contenido(2, 200);
    let c3 = contenido(3, 300);

    // Funcion y no cierre: un cierre que captura `b` por mutable lo mantiene
    // prestado hasta el final del ambito, y entonces no se puede leer `b.s`
    // entre importaciones.
    fn importar(b: &mut Banco, ruta: &std::path::Path, bytes: &[u8]) -> AssetId {
        std::fs::write(ruta, bytes).expect("escribir");
        b.reloj.advance(1_000);
        b.s.import(ruta, AssetMeta::new("pieza", AssetType::Modelo)).expect("importar")
    }

    let id = importar(&mut b, &ruta, &c1);
    assert_eq!(importar(&mut b, &ruta, &c2), id, "la misma ruta es el mismo activo");
    assert_eq!(importar(&mut b, &ruta, &c3), id);

    let v = b.s.versions(id).expect("versiones");
    assert_eq!(v.len(), 3, "tres contenidos distintos, tres versiones");
    assert_eq!(v.iter().map(|x| x.version.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(v[0].hash, BlobHash::of(&c1));
    assert_eq!(v[1].hash, BlobHash::of(&c2));
    assert_eq!(v[2].hash, BlobHash::of(&c3));
    assert_eq!(v[1].size, 200);
    assert_eq!(b.s.blobs().list().expect("blobs").len(), 3);

    // El cuarto: contenido idéntico al segundo.
    assert_eq!(importar(&mut b, &ruta, &c2), id);
    let v2 = b.s.versions(id).expect("versiones");
    assert_eq!(v2.len(), 3, "un contenido ya conocido no crea version");
    assert_eq!(b.s.blobs().list().expect("blobs").len(), 3, "ni blob");
    assert_eq!(v2, v, "el historial es exactamente el mismo");

    // Y el contenido vigente es el del segundo, que es lo que se pidió importar.
    let a = b.s.get(id).expect("get").expect("existe");
    assert_eq!(a.hash, BlobHash::of(&c2));
    assert_eq!(a.version, VersionId(2));
    assert_eq!(a.size, 200);
}

/// `revert` mueve la vigente y **no pierde** las versiones posteriores.
#[test]
fn revert_deja_el_contenido_correcto_y_conserva_las_versiones_posteriores() {
    let mut b = banco();
    let ruta = b.dir.path().join("pieza.stl");
    let contenidos = [contenido(1, 10), contenido(2, 20), contenido(3, 30)];
    let mut id = None;
    for c in &contenidos {
        std::fs::write(&ruta, c).expect("escribir");
        b.reloj.advance(1_000);
        id = Some(b.s.import(&ruta, AssetMeta::new("pieza", AssetType::Modelo)).expect("importar"));
    }
    let id = id.expect("hubo importaciones");
    assert_eq!(b.s.get(id).expect("get").expect("existe").version, VersionId(3));

    b.s.revert(id, VersionId(1)).expect("revert a v1");

    let a = b.s.get(id).expect("get").expect("existe");
    assert_eq!(a.version, VersionId(1));
    assert_eq!(a.hash, BlobHash::of(&contenidos[0]));
    assert_eq!(a.size, 10);
    assert_eq!(&*b.s.content(id).expect("content").expect("hay"), &contenidos[0][..]);

    // Lo que hace falta probar de verdad: la historia sigue entera.
    let v = b.s.versions(id).expect("versiones");
    assert_eq!(v.len(), 3, "revert borro historia");
    assert_eq!(v.iter().map(|x| x.version.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(&*b.s.content_of(id, VersionId(3)).expect("v3").expect("hay"), &contenidos[2][..]);

    // Y se puede volver hacia delante.
    b.s.revert(id, VersionId(3)).expect("revert a v3");
    assert_eq!(&*b.s.content(id).expect("content").expect("hay"), &contenidos[2][..]);

    // Control negativo: una versión que no existe se rechaza con un error que
    // dice cuál, en vez de dejar el activo apuntando a la nada.
    match b.s.revert(id, VersionId(9)) {
        Err(AssetError::VersionDesconocida { version, .. }) => assert_eq!(version, VersionId(9)),
        otro => panic!("se acepto una version inexistente: {otro:?}"),
    }
    assert_eq!(b.s.get(id).expect("get").expect("existe").version, VersionId(3));
}

// ---------------------------------------------------------------------------
// Búsqueda: conjunto construido a mano, recuentos exactos
// ---------------------------------------------------------------------------

/// Siete activos elegidos para que cada filtro tenga una respuesta distinta y
/// conocida. Se devuelven los ids en orden de importación, que es el orden en
/// que `search` los devuelve.
///
/// | # | nombre               | tipo       | etiquetas             | notas                       | tam |
/// |---|----------------------|------------|-----------------------|-----------------------------|-----|
/// | 0 | Válvula de bola      | Modelo     | mecanica, metal       | pieza de prueba             |  10 |
/// | 1 | valvula de compuerta | Modelo     | mecanica              | sin notas                   |  20 |
/// | 2 | Ladrillo rojo        | Textura    | arquitectura, metal   | textura de válvula antigua  |  30 |
/// | 3 | Acero pulido         | Material   | metal                 | (vacías)                    |  40 |
/// | 4 | Croquis              | Referencia | arquitectura          | boceto                      |  50 |
/// | 5 | Manual               | Documento  | (ninguna)             | instrucciones               |  60 |
/// | 6 | Recordatorio         | Nota       | mecanica, arquitectura| revisar valvula al 50%      |  70 |
fn corpus() -> (Banco, Vec<AssetId>) {
    let filas: [(&str, AssetType, &[&str], &str, usize); 7] = [
        ("Válvula de bola", AssetType::Modelo, &["mecanica", "metal"], "pieza de prueba", 10),
        ("valvula de compuerta", AssetType::Modelo, &["mecanica"], "sin notas", 20),
        (
            "Ladrillo rojo",
            AssetType::Textura,
            &["arquitectura", "metal"],
            "textura de válvula antigua",
            30,
        ),
        ("Acero pulido", AssetType::Material, &["metal"], "", 40),
        ("Croquis", AssetType::Referencia, &["arquitectura"], "boceto", 50),
        ("Manual", AssetType::Documento, &[], "instrucciones", 60),
        (
            "Recordatorio",
            AssetType::Nota,
            &["mecanica", "arquitectura"],
            "revisar valvula al 50%",
            70,
        ),
    ];

    let mut b = banco();
    let mut ids = Vec::new();
    for (i, (nombre, tipo, tags, notas, tam)) in filas.iter().enumerate() {
        b.reloj.set(T0 + i as i64 * 1_000);
        let meta = AssetMeta::new(*nombre, *tipo).with_tags(tags.iter().copied()).with_notes(*notas);
        let id = b
            .s
            .import_bytes(format!("/proyecto/{i}.dat"), &contenido(i as u8, *tam), meta)
            .expect("importar");
        ids.push(id);
    }
    assert_eq!(b.s.len().expect("len"), 7);
    (b, ids)
}

fn buscar(b: &Banco, q: &AssetQuery) -> Vec<AssetId> {
    b.s.search(q).expect("buscar")
}

/// Los ids del corpus que ocupan las posiciones dadas, en orden.
fn esperados(ids: &[AssetId], pos: &[usize]) -> Vec<AssetId> {
    pos.iter().map(|i| ids[*i]).collect()
}

#[test]
fn busqueda_sin_filtros_devuelve_todo_en_orden_de_importacion() {
    let (b, ids) = corpus();
    assert_eq!(buscar(&b, &AssetQuery::default()), ids);
    assert_eq!(buscar(&b, &AssetQuery::new().with_limit(3)), esperados(&ids, &[0, 1, 2]));
}

#[test]
fn busqueda_por_texto_mira_nombre_y_notas_y_no_las_etiquetas() {
    let (b, ids) = corpus();

    // "valvula" aparece en el nombre de 0 y 1 y en las notas de 2 y 6.
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("valvula")), esperados(&ids, &[0, 1, 2, 6]));
    // Sin distinguir mayúsculas ni acentos: la misma respuesta.
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("VÁLVULA")), esperados(&ids, &[0, 1, 2, 6]));

    // Control negativo: "metal" es una etiqueta de tres activos y no aparece en
    // ningún nombre ni nota. Si el texto buscara también en las etiquetas, esto
    // devolvería tres.
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("metal")).len(), 0);

    // Control del escape de `LIKE`: '%' es un carácter, no un comodín. Sin
    // escapar devolvería los siete.
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("50%")), esperados(&ids, &[6]));
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("%")), esperados(&ids, &[6]));
    assert_eq!(buscar(&b, &AssetQuery::new().with_text("_")).len(), 0);
}

#[test]
fn busqueda_por_etiquetas_con_y_y_con_o() {
    let (b, ids) = corpus();

    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["metal"])), esperados(&ids, &[0, 2, 3]));
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_all_tags(["mecanica"])),
        esperados(&ids, &[0, 1, 6])
    );

    // Y: solo el 0 tiene las dos.
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_all_tags(["mecanica", "metal"])),
        esperados(&ids, &[0])
    );
    // O: cinco tienen alguna.
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_any_tags(["mecanica", "metal"])),
        esperados(&ids, &[0, 1, 2, 3, 6])
    );

    // Las etiquetas se normalizan: `Metal` y `metal` son la misma.
    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["Metal"])), esperados(&ids, &[0, 2, 3]));
    // Repetir una etiqueta en el Y no cambia la respuesta (el `= n` cuenta
    // etiquetas distintas, no repeticiones).
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_all_tags(["metal", "metal"])),
        esperados(&ids, &[0, 2, 3])
    );
    // Control negativo: una etiqueta que no lleva nadie.
    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["inexistente"])).len(), 0);
}

#[test]
fn busqueda_por_tipo() {
    let (b, ids) = corpus();
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_types([AssetType::Modelo])),
        esperados(&ids, &[0, 1])
    );
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_types([AssetType::Modelo, AssetType::Nota])),
        esperados(&ids, &[0, 1, 6])
    );
    // Los seis tipos juntos son el corpus entero: no hay ningún activo con un
    // tipo fuera de la enumeración.
    assert_eq!(buscar(&b, &AssetQuery::new().with_types(AssetType::TODOS)), ids);
}

#[test]
fn busqueda_por_rango_de_fechas_y_de_tamano() {
    let (b, ids) = corpus();

    assert_eq!(
        buscar(&b, &AssetQuery::new().imported_between(T0 + 2_000, T0 + 4_000)),
        esperados(&ids, &[2, 3, 4]),
        "el rango es inclusivo por los dos extremos"
    );
    assert_eq!(buscar(&b, &AssetQuery::new().imported_between(T0 - 10, T0 - 1)).len(), 0);

    assert_eq!(buscar(&b, &AssetQuery::new().size_between(30, 50)), esperados(&ids, &[2, 3, 4]));
    assert_eq!(buscar(&b, &AssetQuery::new().size_between(0, 9)).len(), 0);
    assert_eq!(buscar(&b, &AssetQuery::new().size_between(0, u64::MAX)), ids);
}

#[test]
fn la_fecha_de_modificacion_se_mueve_y_la_de_importacion_no() {
    let (mut b, ids) = corpus();
    let antes = b.s.get(ids[5]).expect("get").expect("existe");
    assert_eq!(antes.imported, T0 + 5_000);
    assert_eq!(antes.modified, T0 + 5_000);

    b.reloj.set(T0 + 900_000);
    b.s.tag(ids[5], "revisado").expect("etiquetar");

    let despues = b.s.get(ids[5]).expect("get").expect("existe");
    assert_eq!(despues.imported, T0 + 5_000, "importar no vuelve a pasar");
    assert_eq!(despues.modified, T0 + 900_000);

    // Y el filtro por modificación lo ve: exactamente uno.
    assert_eq!(
        buscar(&b, &AssetQuery::new().modified_between(T0 + 100_000, T0 + 1_000_000)),
        esperados(&ids, &[5])
    );
    // Mientras que el de importación sigue sin verlo.
    assert_eq!(buscar(&b, &AssetQuery::new().imported_between(T0 + 100_000, T0 + 1_000_000)).len(), 0);
}

#[test]
fn los_filtros_se_combinan_con_y_logico() {
    let (b, ids) = corpus();

    // texto + tipo
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_text("valvula").with_types([AssetType::Modelo])),
        esperados(&ids, &[0, 1])
    );
    // etiqueta + tamaño
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_all_tags(["metal"]).size_between(30, 50)),
        esperados(&ids, &[2, 3])
    );
    // texto + etiqueta O
    assert_eq!(
        buscar(&b, &AssetQuery::new().with_text("valvula").with_any_tags(["arquitectura"])),
        esperados(&ids, &[2, 6])
    );
    // los cinco filtros a la vez: solo queda el 2
    let q = AssetQuery::new()
        .with_text("valvula")
        .with_types([AssetType::Textura, AssetType::Nota])
        .with_all_tags(["metal"])
        .with_any_tags(["arquitectura", "mecanica"])
        .imported_between(T0, T0 + 3_000)
        .size_between(25, 75);
    assert_eq!(buscar(&b, &q), esperados(&ids, &[2]));

    // Control negativo: cambiar un solo filtro por uno incompatible vacía el
    // resultado. Un `AND` que no filtrara devolvería lo mismo que antes.
    let q_imposible = q.clone().size_between(0, 20);
    assert_eq!(buscar(&b, &q_imposible).len(), 0);
}

// ---------------------------------------------------------------------------
// Etiquetas, metadatos, miniaturas, baja
// ---------------------------------------------------------------------------

#[test]
fn etiquetar_desetiquetar_y_reemplazar_metadatos() {
    let (mut b, ids) = corpus();
    let id = ids[5]; // el Manual, sin etiquetas

    assert!(b.s.get(id).expect("get").expect("existe").meta.tags.is_empty());
    b.s.tag(id, "Urgente").expect("etiquetar");
    b.s.tag(id, "urgente").expect("etiquetar repetido");
    assert_eq!(
        b.s.get(id).expect("get").expect("existe").meta.tags,
        BTreeSet::from(["urgente".to_string()]),
        "etiquetar dos veces la misma no la duplica"
    );

    b.s.untag(id, "URGENTE").expect("desetiquetar");
    assert!(b.s.get(id).expect("get").expect("existe").meta.tags.is_empty());

    // set_meta reemplaza el conjunto entero de etiquetas, no lo acumula.
    b.s.tag(id, "vieja").expect("etiquetar");
    b.s.set_meta(id, AssetMeta::new("Manual v2", AssetType::Nota).with_tags(["nueva"]))
        .expect("set_meta");
    let a = b.s.get(id).expect("get").expect("existe");
    assert_eq!(a.meta.name, "Manual v2");
    assert_eq!(a.meta.kind, AssetType::Nota);
    assert_eq!(a.meta.tags, BTreeSet::from(["nueva".to_string()]));
    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["vieja"])).len(), 0);
    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["nueva"])), vec![id]);

    // Control negativo: un id que no existe no se etiqueta en silencio.
    let fantasma = AssetId::from_u128(7);
    assert!(matches!(b.s.tag(fantasma, "x"), Err(AssetError::Desconocido(_))));
    assert!(matches!(b.s.set_meta(fantasma, AssetMeta::new("x", AssetType::Nota)),
                     Err(AssetError::Desconocido(_))));
}

/// Se guarda el **hash** de la miniatura, no la imagen: este crate no genera
/// imágenes. La prueba de que es así es que el almacén no crea ningún blob al
/// asignarla — el blob lo puso quien calculó la miniatura.
#[test]
fn la_miniatura_es_un_hash_y_no_una_imagen() {
    let (mut b, ids) = corpus();
    let id = ids[0];
    assert_eq!(b.s.thumbnail(id).expect("miniatura"), None);

    let antes = b.s.blobs().list().expect("blobs").len();
    let h = b.s.blobs().put(b"PNG falso de 128x128").expect("poner miniatura");
    assert_eq!(b.s.blobs().list().expect("blobs").len(), antes + 1);

    b.s.set_thumbnail(id, Some(h)).expect("asignar");
    assert_eq!(b.s.thumbnail(id).expect("miniatura"), Some(h));
    assert_eq!(
        b.s.blobs().list().expect("blobs").len(),
        antes + 1,
        "asignar la miniatura no debe crear blobs"
    );
    assert_eq!(b.s.get(id).expect("get").expect("existe").thumbnail, Some(h));

    b.s.set_thumbnail(id, None).expect("quitar");
    assert_eq!(b.s.thumbnail(id).expect("miniatura"), None);
}

#[test]
fn dar_de_baja_quita_el_activo_de_todas_las_vistas() {
    let (mut b, ids) = corpus();
    b.s.remove(ids[3]).expect("baja");

    assert_eq!(b.s.len().expect("len"), 6);
    assert_eq!(b.s.get(ids[3]).expect("get"), None);
    assert_eq!(buscar(&b, &AssetQuery::default()), esperados(&ids, &[0, 1, 2, 4, 5, 6]));
    assert_eq!(buscar(&b, &AssetQuery::new().with_all_tags(["metal"])), esperados(&ids, &[0, 2]));
    assert!(matches!(b.s.versions(ids[3]), Err(AssetError::Desconocido(_))));
    assert!(matches!(b.s.remove(ids[3]), Err(AssetError::Desconocido(_))));
}

// ---------------------------------------------------------------------------
// Dependencias
// ---------------------------------------------------------------------------

/// Grafo conocido:
///
/// ```text
///   b ──► a        d ──► b        c ──► a        e (suelto)
/// ```
///
/// («x ──► y» = x depende de y.)
#[test]
fn dependientes_sobre_un_grafo_conocido() {
    let mut b = banco();
    let mut nuevo = |n: &str| {
        b.s.import_bytes(format!("/g/{n}"), n.as_bytes(), AssetMeta::new(n, AssetType::Modelo))
            .expect("importar")
    };
    let (a, bb, c, d, e) = (nuevo("a"), nuevo("b"), nuevo("c"), nuevo("d"), nuevo("e"));

    b.s.add_dependency(bb, a).expect("b->a");
    b.s.add_dependency(c, a).expect("c->a");
    b.s.add_dependency(d, bb).expect("d->b");

    let mut esp = vec![bb, c];
    esp.sort();
    assert_eq!(b.s.dependents(a).expect("dependientes de a"), esp);
    assert_eq!(b.s.dependents(bb).expect("dependientes de b"), vec![d]);
    assert_eq!(b.s.dependents(e).expect("dependientes de e"), Vec::<AssetId>::new());
    assert_eq!(b.s.dependencies(bb).expect("dependencias de b"), vec![a]);
    assert_eq!(b.s.dependencies(a).expect("dependencias de a"), Vec::<AssetId>::new());

    let mut trans = vec![bb, c, d];
    trans.sort();
    assert_eq!(b.s.transitive_dependents(a).expect("cierre"), trans);

    // Control negativo del detector de ciclos: a -> d cerraría a->d->b->a.
    match b.s.add_dependency(a, d) {
        Err(AssetError::Ciclo { de, a: hacia }) => {
            assert_eq!(de, a);
            assert_eq!(hacia, d);
        }
        otro => panic!("se acepto un ciclo: {otro:?}"),
    }
    // Y el rechazo no dejó la arista a medias.
    assert_eq!(b.s.dependencies(a).expect("dependencias de a"), Vec::<AssetId>::new());
    assert_eq!(b.s.transitive_dependents(a).expect("cierre"), trans);

    // Control positivo: una arista que no cierra ciclo sí entra.
    b.s.add_dependency(e, a).expect("e->a");
    let mut esp2 = vec![bb, c, e];
    esp2.sort();
    assert_eq!(b.s.dependents(a).expect("dependientes de a"), esp2);

    // Dar de baja un activo se lleva sus aristas por los dos lados.
    b.s.remove(bb).expect("baja de b");
    assert_eq!(b.s.dependents(a).expect("dependientes de a"), {
        let mut v = vec![c, e];
        v.sort();
        v
    });
    assert_eq!(b.s.dependents(d).expect("dependientes de d"), Vec::<AssetId>::new());
    assert_eq!(b.s.dependencies(d).expect("dependencias de d"), Vec::<AssetId>::new());
}

// ---------------------------------------------------------------------------
// Recolección
// ---------------------------------------------------------------------------

/// Control positivo y negativo en el mismo test: un blob huérfano **se borra**,
/// uno referenciado **no**. Un `gc` que no borrara nada pasaría solo la segunda
/// mitad; uno que borrara todo, solo la primera.
#[test]
fn gc_borra_los_huerfanos_y_conserva_lo_referenciado() {
    let mut b = banco();
    let ca = contenido(0xA1, 64);
    let cb = contenido(0xB2, 64);
    let cd1 = contenido(0xD1, 64);
    let cd2 = contenido(0xD2, 64);

    // `a` y `c` comparten contenido: dar de baja `c` NO puede llevarse el blob.
    let a = b.s.import_bytes("/g/a", &ca, AssetMeta::new("a", AssetType::Modelo)).expect("a");
    let bb = b.s.import_bytes("/g/b", &cb, AssetMeta::new("b", AssetType::Modelo)).expect("b");
    let c = b.s.import_bytes("/g/c", &ca, AssetMeta::new("c", AssetType::Modelo)).expect("c");

    // `d` tiene dos versiones: el blob de la vieja tampoco se puede recolectar.
    let d = b.s.import_bytes("/g/d", &cd1, AssetMeta::new("d", AssetType::Modelo)).expect("d1");
    b.s.import_bytes("/g/d", &cd2, AssetMeta::new("d", AssetType::Modelo)).expect("d2");
    assert_eq!(b.s.versions(d).expect("versiones").len(), 2);

    // Una miniatura: también cuenta como referencia.
    let mini = b.s.blobs().put(b"miniatura").expect("miniatura");
    b.s.set_thumbnail(a, Some(mini)).expect("asignar miniatura");

    // Y un blob que no referencia nadie desde el principio.
    let suelto = b.s.blobs().put(b"nadie me referencia").expect("suelto");

    assert_eq!(b.s.blobs().list().expect("blobs").len(), 6, "ca, cb, cd1, cd2, mini, suelto");

    b.s.remove(bb).expect("baja de b"); // deja cb huérfano
    b.s.remove(c).expect("baja de c"); // NO deja ca huérfano: `a` lo comparte

    let rep = b.s.gc().expect("gc");
    assert_eq!(rep.examined, 6);
    assert_eq!(rep.removed, 2, "solo cb y suelto son huerfanos");
    assert_eq!(rep.kept, 4);
    assert_eq!(rep.freed_bytes, 64 + b"nadie me referencia".len() as u64);

    let quedan: BTreeSet<BlobHash> = b.s.blobs().list().expect("blobs").into_iter().collect();
    // Negativo: lo referenciado sigue ahí.
    assert!(quedan.contains(&BlobHash::of(&ca)), "se borro un blob compartido");
    assert!(quedan.contains(&BlobHash::of(&cd1)), "se borro la historia de d");
    assert!(quedan.contains(&BlobHash::of(&cd2)));
    assert!(quedan.contains(&mini), "se borro la miniatura");
    // Positivo: lo huérfano ya no está.
    assert!(!quedan.contains(&BlobHash::of(&cb)), "no se recolecto el huerfano");
    assert!(!quedan.contains(&suelto), "no se recolecto el blob suelto");
    assert_eq!(quedan.len(), 4);

    // El contenido que se conservó se sigue pudiendo leer, y el historial de `d`
    // sigue siendo recuperable. Que el archivo exista no basta.
    assert_eq!(&*b.s.content(a).expect("content").expect("hay"), &ca[..]);
    assert_eq!(&*b.s.content_of(d, VersionId(1)).expect("v1").expect("hay"), &cd1[..]);
    assert!(b.s.blobs().verify().expect("verify").is_empty());

    // Un segundo `gc` no tiene nada que hacer: es idempotente.
    let rep2 = b.s.gc().expect("gc otra vez");
    assert_eq!(rep2.removed, 0);
    assert_eq!(rep2.kept, 4);
}

// ---------------------------------------------------------------------------
// El índice es caché
// ---------------------------------------------------------------------------

/// Estado completo del almacén, en forma comparable. Si `reindex` olvidara un
/// campo —una nota, una fecha, una miniatura, una arista— esta foto cambiaría.
fn foto(s: &AssetStore, consultas: &[AssetQuery]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, q) in consultas.iter().enumerate() {
        let r = s.search(q).expect("buscar");
        out.push(format!("q{i} -> {r:?}"));
    }
    for id in s.search(&AssetQuery::default()).expect("todos") {
        let a = s.get(id).expect("get").expect("existe");
        out.push(format!("{id} = {a:?}"));
        out.push(format!("{id} versiones = {:?}", s.versions(id).expect("versiones")));
        out.push(format!("{id} depende de = {:?}", s.dependencies(id).expect("deps")));
        out.push(format!("{id} dependientes = {:?}", s.dependents(id).expect("dependientes")));
        out.push(format!("{id} miniatura = {:?}", s.thumbnail(id).expect("miniatura")));
    }
    out
}

fn consultas_de_control() -> Vec<AssetQuery> {
    vec![
        AssetQuery::default(),
        AssetQuery::new().with_text("valvula"),
        AssetQuery::new().with_all_tags(["metal"]),
        AssetQuery::new().with_any_tags(["arquitectura", "mecanica"]),
        AssetQuery::new().with_types([AssetType::Modelo, AssetType::Textura]),
        AssetQuery::new().imported_between(T0 + 1_000, T0 + 4_000),
        AssetQuery::new().size_between(20, 60),
        AssetQuery::new().with_text("valvula").with_all_tags(["metal"]).size_between(25, 75),
    ]
}

/// Deja el corpus con historial, etiquetas, dependencias, miniatura y una baja,
/// para que la foto tenga algo que perder.
fn corpus_rico() -> (Banco, Vec<AssetId>) {
    let (mut b, ids) = corpus();
    b.reloj.set(T0 + 10_000);
    b.s.import_bytes("/proyecto/0.dat", &contenido(0x55, 111), AssetMeta::new("Válvula de bola v2", AssetType::Modelo).with_tags(["mecanica", "metal", "revisado"]).with_notes("pieza de prueba, corregida"))
        .expect("v2 del 0");
    b.reloj.set(T0 + 11_000);
    b.s.revert(ids[0], VersionId(1)).expect("revert");
    b.s.add_dependency(ids[1], ids[0]).expect("1->0");
    b.s.add_dependency(ids[2], ids[0]).expect("2->0");
    b.s.add_dependency(ids[4], ids[2]).expect("4->2");
    let mini = b.s.blobs().put(b"miniatura de la valvula").expect("miniatura");
    b.s.set_thumbnail(ids[0], Some(mini)).expect("asignar");
    b.s.tag(ids[5], "sin-clasificar").expect("etiquetar");
    b.s.remove(ids[3]).expect("baja");
    (b, ids)
}

/// **El test que sostiene «el índice es caché».** Se borra el archivo SQLite
/// entero y se reconstruye: la búsqueda tiene que devolver exactamente lo mismo,
/// y todo lo demás también.
#[test]
fn borrar_el_indice_y_reconstruirlo_no_pierde_nada() {
    let (mut b, _ids) = corpus_rico();
    let consultas = consultas_de_control();
    let antes = foto(&b.s, &consultas);
    assert!(antes.len() > 30, "la foto tiene que mirar algo: {}", antes.len());

    let ruta_indice = b.dir.path().join(NOMBRE_INDICE);
    assert!(ruta_indice.exists());
    std::fs::remove_file(&ruta_indice).expect("borrar el indice");
    assert!(!ruta_indice.exists());

    let rep = b.s.reindex().expect("reindex");
    assert_eq!(rep.assets, 6, "siete importados menos uno dado de baja");
    assert_eq!(rep.versions, 7, "seis vivos con una version, mas la v2 del primero");
    assert_eq!(rep.unreadable, 0);
    assert_eq!(rep.incomplete_tail_bytes, 0);
    assert_eq!(rep.dependencies, 3);
    assert!(rep.missing_blobs.is_empty(), "faltan blobs: {:?}", rep.missing_blobs);
    assert!(rep.records >= 13, "registros aplicados: {}", rep.records);

    assert_eq!(foto(&b.s, &consultas), antes, "la reconstruccion perdio algo");
}

/// La otra mitad: el índice no solo se puede borrar, se puede corromper. Al
/// abrir, el almacén lo rehace sin que el llamante tenga que saber nada.
#[test]
fn un_indice_corrupto_se_rehace_solo_al_abrir() {
    let (b, _ids) = corpus_rico();
    let consultas = consultas_de_control();
    let antes = foto(&b.s, &consultas);
    let raiz = b.dir.path().to_path_buf();
    let dir = b.dir; // conservar el temporal vivo
    drop(b.s);

    // Basura donde había una base de datos.
    std::fs::write(raiz.join(NOMBRE_INDICE), vec![0x7f; 65_536]).expect("corromper");

    let s2 = AssetStore::open(&raiz).expect("abrir con el indice corrupto");
    assert_eq!(foto(&s2, &consultas), antes, "no se recupero el estado");
    drop(dir);
}

/// Reabrir sin borrar nada no reconstruye ni cambia nada: el índice se pone al
/// día comparando un solo número. Control negativo del camino anterior.
#[test]
fn reabrir_conserva_el_indice_y_el_estado() {
    let (b, _ids) = corpus_rico();
    let consultas = consultas_de_control();
    let antes = foto(&b.s, &consultas);
    let raiz = b.dir.path().to_path_buf();
    let dir = b.dir;
    drop(b.s);

    let bytes_antes = std::fs::metadata(raiz.join(NOMBRE_INDICE)).expect("stat").len();
    let s2 = AssetStore::open(&raiz).expect("reabrir");
    assert_eq!(foto(&s2, &consultas), antes);
    let bytes_despues = std::fs::metadata(raiz.join(NOMBRE_INDICE)).expect("stat").len();
    assert_eq!(bytes_antes, bytes_despues, "reabrir rehizo el indice sin necesidad");
    drop(dir);
}

/// Una línea corrupta del diario cuesta **un** evento, no el archivo. Es la
/// propiedad por la que el diario es texto por líneas y no un binario.
#[test]
fn una_linea_ilegible_del_diario_no_se_lleva_el_resto() {
    let (mut b, ids) = corpus();
    let antes = foto(&b.s, &consultas_de_control());
    let ruta = b.dir.path().join(NOMBRE_DIARIO);

    // Se ensucia una línea del medio, no la última.
    let texto = std::fs::read_to_string(&ruta).expect("leer diario");
    let mut lineas: Vec<&str> = texto.lines().collect();
    assert_eq!(lineas.len(), 7);
    lineas[3] = "{esto no es json valido";
    std::fs::write(&ruta, lineas.join("\n") + "\n").expect("escribir diario");

    let rep = b.s.reindex().expect("reindex");
    assert_eq!(rep.unreadable, 1, "no conto la linea rota");
    assert_eq!(rep.records, 6, "los otros seis eventos siguen aplicandose");
    assert_eq!(rep.assets, 6);

    // Se perdió exactamente el activo de esa línea; los seis restantes están
    // idénticos a como estaban.
    let despues = foto(&b.s, &consultas_de_control());
    assert_ne!(despues, antes);
    assert_eq!(b.s.get(ids[3]).expect("get"), None, "se perdio el activo equivocado");
    for i in [0usize, 1, 2, 4, 5, 6] {
        assert!(b.s.get(ids[i]).expect("get").is_some(), "se perdio de mas: {i}");
    }
}

/// Una escritura interrumpida deja una cola sin `\n`. Se cuenta, se ignora y no
/// se consume: la aplicación siguiente puede completarla.
#[test]
fn una_escritura_a_medias_del_diario_se_distingue_de_una_corrupta() {
    let (mut b, ids) = corpus();
    let ruta = b.dir.path().join(NOMBRE_DIARIO);
    let mut texto = std::fs::read_to_string(&ruta).expect("leer diario");
    texto.push_str("{\"evento\":\"borrado\",\"id\":\"");
    std::fs::write(&ruta, &texto).expect("escribir diario");

    let rep = b.s.reindex().expect("reindex");
    assert_eq!(rep.records, 7, "las siete lineas completas siguen valiendo");
    assert_eq!(rep.unreadable, 0, "una cola a medias no es una linea corrupta");
    // `{"evento":"borrado","id":"` son 26 bytes, no 25.
    assert_eq!(rep.incomplete_tail_bytes, 26);
    assert_eq!(rep.assets, 7);
    assert!(b.s.get(ids[6]).expect("get").is_some());
}

/// La otra mitad de la fuente de verdad. Si falta un blob, el informe lo dice
/// con nombre y apellidos en vez de dejar un activo que no se puede abrir.
#[test]
fn reindex_denuncia_los_blobs_que_faltan() {
    let (mut b, ids) = corpus();
    // Control negativo: con todo en su sitio, la lista está vacía.
    assert!(b.s.reindex().expect("reindex").missing_blobs.is_empty());

    let h = b.s.get(ids[2]).expect("get").expect("existe").hash;
    let p: PathBuf = b.dir.path().join("blobs").join(h.shard()).join(h.to_hex());
    std::fs::remove_file(&p).expect("borrar el blob a mano");

    let rep = b.s.reindex().expect("reindex");
    assert_eq!(rep.missing_blobs, vec![h], "no detecto el blob ausente");
    assert_eq!(rep.assets, 7, "el activo sigue en el indice: falta el contenido, no el registro");
    assert_eq!(b.s.content(ids[2]).expect("content"), None);
}

// ---------------------------------------------------------------------------
// Lote
// ---------------------------------------------------------------------------

#[test]
fn un_lote_que_falla_deja_el_indice_al_dia_al_reabrir() {
    let mut b = banco();
    b.s.import_bytes("/l/a", b"a", AssetMeta::new("a", AssetType::Nota)).expect("a");

    let r: std::result::Result<(), AssetError> = b.s.batch(|s| {
        s.import_bytes("/l/b", b"b", AssetMeta::new("b", AssetType::Nota))?;
        s.import_bytes("/l/c", b"c", AssetMeta::new("c", AssetType::Nota))?;
        Err(AssetError::RegistroIlegible("fallo simulado".into()))
    });
    assert!(r.is_err());

    // El índice deshizo la transacción...
    assert_eq!(b.s.len().expect("len"), 1);
    // ...pero el diario ya tenía los hechos, así que reabrir los recupera. El
    // diario es la verdad; el índice, lo que se pone al día.
    let raiz = b.dir.path().to_path_buf();
    let dir = b.dir;
    drop(b.s);
    let s2 = AssetStore::open(&raiz).expect("reabrir");
    assert_eq!(s2.len().expect("len"), 3);
    drop(dir);
}
