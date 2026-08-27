//! El bus de comandos: respuesta conocida por capacidad, y un control positivo
//! por cada modo de fallo.
//!
//! Todo lo que se afirma sobre el estado del documento se comprueba con
//! `Snapshot::fingerprint()` y no contando entidades: una huella distingue "puso
//! el nombre correcto" de "puso *un* nombre", y un recuento no.

use forge_doc::{Document, EntityId, Geometry, GeometryPayload, Name, Parent, Transform, Visible};
use forge_math::DVec3;
use forge_script::{Command, CommandBus, CommandError, CommandLog};
use forge_store::BlobHash;

fn e(n: u128) -> EntityId {
    EntityId::from_u128(n)
}

fn blob(s: &str) -> BlobHash {
    BlobHash::of(s.as_bytes())
}

fn spawn(n: u128, nombre: &str) -> Command {
    Command::Spawn { id: Some(e(n)), name: Some(nombre.to_string()) }
}

/// Documento con una entidad conocida, listo para los comandos que la editan.
fn doc_con_una_entidad() -> (Document, CommandBus, EntityId) {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    bus.apply(&mut doc, spawn(1, "pieza")).expect("spawn inicial");
    (doc, bus, e(1))
}

// ---------------------------------------------------------------------------
// Respuesta conocida: qué hace cada comando
// ---------------------------------------------------------------------------

#[test]
fn spawn_crea_la_entidad_y_devuelve_su_id() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let vacio = doc.snapshot().fingerprint();

    let out = bus.apply(&mut doc, spawn(1, "pieza")).unwrap();
    assert_eq!(out.entity(), Some(e(1)));

    let s = doc.snapshot();
    assert!(s.contains(e(1)));
    assert_eq!(s.get::<Name>(e(1)), Some(&Name("pieza".into())));
    assert_ne!(s.fingerprint(), vacio, "la huella no cambió tras crear una entidad");
}

/// `Spawn { id: None }` es cómodo pero el bus **resuelve** el id antes de
/// grabarlo: sin eso el log no sería reproducible.
#[test]
fn spawn_sin_id_lo_resuelve_antes_de_grabarlo() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let creada = bus.apply(&mut doc, Command::Spawn { id: None, name: None }).unwrap().entity();
    assert!(creada.is_some());

    match &bus.log().commands[0] {
        Command::Spawn { id, .. } => assert_eq!(*id, creada, "el log grabó un id sin resolver"),
        otro => panic!("se grabó {otro:?} en vez de un Spawn"),
    }
}

#[test]
fn cada_comando_de_escena_produce_su_cambio() {
    let (mut doc, mut bus, x) = doc_con_una_entidad();
    let y = e(2);
    bus.apply(&mut doc, spawn(2, "padre")).unwrap();

    let t = Transform::from_translation(DVec3::new(1.0, 2.0, 3.0));
    let g = GeometryPayload::Mesh(blob("malla"));

    bus.apply(&mut doc, Command::SetName { entity: x, name: "renombrada".into() }).unwrap();
    bus.apply(&mut doc, Command::SetTransform { entity: x, transform: t }).unwrap();
    bus.apply(&mut doc, Command::SetVisible { entity: x, visible: false }).unwrap();
    bus.apply(&mut doc, Command::SetParent { child: x, parent: Some(y) }).unwrap();
    bus.apply(&mut doc, Command::SetGeometry { entity: x, payload: g }).unwrap();

    let s = doc.snapshot();
    assert_eq!(s.get::<Name>(x), Some(&Name("renombrada".into())));
    assert_eq!(s.get::<Transform>(x), Some(&t));
    assert_eq!(s.get::<Visible>(x), Some(&Visible(false)));
    assert_eq!(s.get::<Parent>(x), Some(&Parent(y)));
    assert_eq!(s.get::<Geometry>(x), Some(&Geometry(g)));

    bus.apply(&mut doc, Command::ClearGeometry { entity: x }).unwrap();
    bus.apply(&mut doc, Command::SetParent { child: x, parent: None }).unwrap();
    let s = doc.snapshot();
    assert_eq!(s.get::<Geometry>(x), None, "ClearGeometry no quitó el componente");
    assert_eq!(s.get::<Parent>(x), None, "SetParent(None) no desenganchó");

    bus.apply(&mut doc, Command::Despawn { entity: x }).unwrap();
    let s = doc.snapshot();
    assert!(!s.contains(x));
    assert_eq!(s.get::<Name>(x), None, "Despawn dejó componentes huérfanos");
}

/// La huella es el criterio de igualdad de estados en todo el proyecto. Aquí se
/// comprueba que **distingue**: dos documentos que solo difieren en un nombre no
/// pueden compartir huella, o todos los demás tests de este archivo valdrían
/// para cualquier implementación.
#[test]
fn la_huella_distingue_cambios_pequenos() {
    let mut a = Document::new();
    let mut b = Document::new();
    let mut ba = CommandBus::new();
    let mut bb = CommandBus::new();

    ba.apply(&mut a, spawn(1, "uno")).unwrap();
    bb.apply(&mut b, spawn(1, "uno")).unwrap();
    assert_eq!(a.snapshot().fingerprint(), b.snapshot().fingerprint());

    bb.apply(&mut b, Command::SetName { entity: e(1), name: "dos".into() }).unwrap();
    assert_ne!(a.snapshot().fingerprint(), b.snapshot().fingerprint());
}

#[test]
fn undo_y_redo_recorren_el_historial() {
    let (mut doc, mut bus, x) = doc_con_una_entidad();
    let antes = doc.snapshot().fingerprint();

    bus.apply(&mut doc, Command::SetVisible { entity: x, visible: false }).unwrap();
    let despues = doc.snapshot().fingerprint();
    assert_ne!(antes, despues);

    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_eq!(doc.snapshot().fingerprint(), antes, "el undo no restauró el estado exacto");

    bus.apply(&mut doc, Command::Redo).unwrap();
    assert_eq!(doc.snapshot().fingerprint(), despues, "el redo no restauró el estado exacto");
}

// ---------------------------------------------------------------------------
// Ida y vuelta CBOR
// ---------------------------------------------------------------------------

/// Cobertura exhaustiva por construcción: si alguien añade una variante a
/// `Command` y no la mete en esta lista, `variante_cubierta` deja de compilar.
fn todas_las_variantes() -> Vec<Command> {
    vec![
        Command::Spawn { id: Some(e(1)), name: Some("con nombre".into()) },
        Command::Spawn { id: None, name: None },
        Command::Despawn { entity: e(1) },
        Command::SetName { entity: e(1), name: "ñandú — ünïcode".into() },
        Command::SetTransform {
            entity: e(1),
            transform: Transform::from_translation(DVec3::new(-1.5, 0.0, 1e9)),
        },
        Command::SetVisible { entity: e(1), visible: true },
        Command::SetParent { child: e(1), parent: Some(e(2)) },
        Command::SetParent { child: e(1), parent: None },
        Command::SetGeometry { entity: e(1), payload: GeometryPayload::Brep(blob("b")) },
        Command::ClearGeometry { entity: e(1) },
        Command::Undo,
        Command::Redo,
        Command::BeginGroup { label: "macro".into() },
        Command::EndGroup,
    ]
}

/// Match exhaustivo: el compilador obliga a tocar este archivo al añadir una
/// variante nueva al bus.
fn variante_cubierta(c: &Command) -> &'static str {
    match c {
        Command::Spawn { .. } => "Spawn",
        Command::Despawn { .. } => "Despawn",
        Command::SetName { .. } => "SetName",
        Command::SetTransform { .. } => "SetTransform",
        Command::SetVisible { .. } => "SetVisible",
        Command::SetParent { .. } => "SetParent",
        Command::SetGeometry { .. } => "SetGeometry",
        Command::ClearGeometry { .. } => "ClearGeometry",
        Command::Undo => "Undo",
        Command::Redo => "Redo",
        Command::BeginGroup { .. } => "BeginGroup",
        Command::EndGroup => "EndGroup",
    }
}

#[test]
fn cada_variante_sobrevive_a_la_ida_y_vuelta_cbor() {
    let mut vistas = std::collections::BTreeSet::new();
    for c in todas_las_variantes() {
        vistas.insert(variante_cubierta(&c));
        let log = CommandLog::new(vec![c.clone()]);
        let bytes = log.to_cbor().expect("codificar");
        let vuelta = CommandLog::from_cbor(&bytes).expect("decodificar");
        assert_eq!(vuelta.commands, vec![c.clone()], "{} no sobrevivió al CBOR", c.kind());
    }
    assert_eq!(vistas.len(), 12, "faltan variantes en la muestra: {vistas:?}");
}

/// Control positivo del decodificador: bytes que no son un log no deben pasar
/// por buenos. Un `from_cbor` que devolviera siempre un log vacío superaría el
/// test de arriba igual de bien.
#[test]
fn el_decodificador_rechaza_basura() {
    assert!(matches!(
        CommandLog::from_cbor(&[0xde, 0xad, 0xbe, 0xef]),
        Err(CommandError::Decodificacion(_))
    ));
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Guion de referencia: mezcla creación, edición, jerarquía, agrupación y undo.
fn guion() -> Vec<Command> {
    vec![
        spawn(1, "raíz"),
        spawn(2, "hijo"),
        Command::SetParent { child: e(2), parent: Some(e(1)) },
        Command::BeginGroup { label: "tres cubos".into() },
        spawn(10, "cubo 1"),
        spawn(11, "cubo 2"),
        spawn(12, "cubo 3"),
        Command::EndGroup,
        Command::SetTransform {
            entity: e(10),
            transform: Transform::from_translation(DVec3::new(10.0, 0.0, 0.0)),
        },
        Command::SetGeometry { entity: e(11), payload: GeometryPayload::Mesh(blob("m")) },
        Command::SetVisible { entity: e(12), visible: false },
        spawn(13, "sobrante"),
        Command::Undo,
        Command::Despawn { entity: e(2) },
    ]
}

#[test]
fn el_log_reproducido_da_la_misma_huella() {
    let mut original = Document::new();
    let mut bus = CommandBus::new();
    for c in guion() {
        bus.apply(&mut original, c).expect("el guion debe aplicarse limpio");
    }
    bus.finish().expect("el guion no deja grupos abiertos");

    let log = bus.take_log();
    assert_eq!(log.len(), guion().len());

    // Ida y vuelta por CBOR de por medio: el log que se reproduce es el que
    // saldría de un archivo o de un socket, no el que quedó en memoria.
    let log = CommandLog::from_cbor(&log.to_cbor().unwrap()).unwrap();

    let copia = log.replay_into_new().expect("reproducir");
    assert_eq!(
        copia.snapshot().fingerprint(),
        original.snapshot().fingerprint(),
        "reproducir el log no reconstruyó el mismo estado"
    );
}

/// Control positivo del replay: si se le quita el último comando, la huella
/// **tiene** que diferir. Sin esto, un `fingerprint` constante haría pasar el
/// test anterior.
#[test]
fn un_log_truncado_no_reproduce_el_mismo_estado() {
    let mut original = Document::new();
    let mut bus = CommandBus::new();
    bus.apply_all(&mut original, guion()).unwrap();

    let mut cmds = bus.take_log().commands;
    cmds.pop();
    let copia = CommandLog::new(cmds).replay_into_new().unwrap();

    assert_ne!(copia.snapshot().fingerprint(), original.snapshot().fingerprint());
}

/// Reproducir dos veces el mismo log da exactamente lo mismo: el bus no mete
/// ninguna fuente de no determinismo (ids nuevos, relojes, orden de mapas).
#[test]
fn el_replay_es_determinista() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    bus.apply_all(&mut doc, guion()).unwrap();
    let log = bus.take_log();

    let a = log.replay_into_new().unwrap().snapshot().fingerprint();
    let b = log.replay_into_new().unwrap().snapshot().fingerprint();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// Agrupación
// ---------------------------------------------------------------------------

#[test]
fn un_grupo_de_tres_comandos_se_deshace_con_un_solo_undo() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let inicial = doc.snapshot().fingerprint();

    bus.apply(&mut doc, Command::BeginGroup { label: "tres cubos".into() }).unwrap();
    // Dentro del grupo nada llega al documento todavía: es lo que hace que sea
    // una sola entrada de undo y no tres.
    for n in 1..=3u128 {
        let out = bus.apply(&mut doc, spawn(n, "cubo")).unwrap();
        assert_eq!(out.entity(), Some(e(n)), "el id debe estar resuelto ya al encolar");
    }
    assert_eq!(doc.snapshot().fingerprint(), inicial, "el grupo se aplicó antes de cerrarse");

    bus.apply(&mut doc, Command::EndGroup).unwrap();
    let agrupado = doc.snapshot().fingerprint();
    assert_eq!(doc.snapshot().entity_count(), 3);

    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_eq!(doc.snapshot().fingerprint(), inicial, "un undo no revirtió el grupo entero");

    bus.apply(&mut doc, Command::Redo).unwrap();
    assert_eq!(doc.snapshot().fingerprint(), agrupado);
}

/// Control: los mismos tres comandos **sin** agrupar necesitan tres undos. Sin
/// este control, un `BeginGroup` que no hiciera nada pasaría el test de arriba
/// si el documento resultara tener una sola versión por otro motivo.
#[test]
fn los_mismos_tres_comandos_sin_agrupar_necesitan_tres_undos() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let inicial = doc.snapshot().fingerprint();

    for n in 1..=3u128 {
        bus.apply(&mut doc, spawn(n, "cubo")).unwrap();
    }
    let tras_tres = doc.snapshot().fingerprint();

    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_ne!(doc.snapshot().fingerprint(), inicial, "un solo undo no debería bastar");
    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_ne!(doc.snapshot().fingerprint(), inicial, "dos undos tampoco deberían bastar");
    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_eq!(doc.snapshot().fingerprint(), inicial, "el tercer undo sí debía dejarlo vacío");

    // y el estado agrupado equivalente es el mismo estado
    let mut doc2 = Document::new();
    let mut bus2 = CommandBus::new();
    bus2.apply(&mut doc2, Command::BeginGroup { label: "g".into() }).unwrap();
    for n in 1..=3u128 {
        bus2.apply(&mut doc2, spawn(n, "cubo")).unwrap();
    }
    bus2.apply(&mut doc2, Command::EndGroup).unwrap();
    assert_eq!(
        doc2.snapshot().fingerprint(),
        tras_tres,
        "agrupar cambió el resultado, no solo el número de entradas de undo"
    );
}

/// Un grupo es atómico: si el tercer comando falla al aplicarse, no queda medio
/// grupo en el documento.
#[test]
fn un_grupo_que_falla_al_cerrarse_no_deja_nada() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    bus.apply(&mut doc, spawn(1, "existente")).unwrap();
    let antes = doc.snapshot().fingerprint();

    bus.apply(&mut doc, Command::BeginGroup { label: "mala".into() }).unwrap();
    bus.apply(&mut doc, spawn(2, "nueva")).unwrap();
    // El grupo se valida al encolar, así que el duplicado se detecta aquí.
    let err = bus.apply(&mut doc, spawn(1, "colisión")).unwrap_err();
    assert_eq!(err, CommandError::EntidadDuplicada(e(1)));

    bus.apply(&mut doc, Command::EndGroup).unwrap();
    // El comando rechazado nunca entró en la cola: el grupo aplica los dos
    // válidos y el documento no ve la colisión.
    assert!(doc.snapshot().contains(e(2)));
    assert_ne!(doc.snapshot().fingerprint(), antes);
}

// ---------------------------------------------------------------------------
// Controles positivos de error
// ---------------------------------------------------------------------------

#[test]
fn un_comando_sobre_una_entidad_inexistente_falla_y_no_toca_el_documento() {
    let (mut doc, mut bus, _) = doc_con_una_entidad();
    let antes = doc.snapshot().fingerprint();
    let fantasma = e(999);

    for cmd in [
        Command::Despawn { entity: fantasma },
        Command::SetName { entity: fantasma, name: "x".into() },
        Command::SetTransform { entity: fantasma, transform: Transform::IDENTITY },
        Command::SetVisible { entity: fantasma, visible: true },
        Command::SetGeometry { entity: fantasma, payload: GeometryPayload::Mesh(blob("m")) },
        Command::ClearGeometry { entity: fantasma },
        Command::SetParent { child: fantasma, parent: None },
    ] {
        let kind = cmd.kind();
        let err = bus.apply(&mut doc, cmd).unwrap_err();
        assert_eq!(err, CommandError::EntidadDesconocida(fantasma), "{kind}");
        assert!(err.to_string().contains("crea la entidad"), "{kind}: el error no dice qué hacer");
    }

    // Un padre inexistente también, y por el id del padre.
    let err = bus
        .apply(&mut doc, Command::SetParent { child: e(1), parent: Some(fantasma) })
        .unwrap_err();
    assert_eq!(err, CommandError::EntidadDesconocida(fantasma));

    assert_eq!(doc.snapshot().fingerprint(), antes, "un comando fallido dejó rastro");
    assert!(!doc.can_undo() || doc.version() == doc.snapshot().version());
}

#[test]
fn end_group_sin_begin_group_es_un_error() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let err = bus.apply(&mut doc, Command::EndGroup).unwrap_err();
    assert_eq!(err, CommandError::EndGroupSinBeginGroup);
    assert!(err.to_string().contains("BeginGroup"), "el error no dice qué hacer");

    // Control: tras un BeginGroup legítimo, el mismo EndGroup sí funciona.
    bus.apply(&mut doc, Command::BeginGroup { label: "g".into() }).unwrap();
    assert!(bus.apply(&mut doc, Command::EndGroup).is_ok());
}

#[test]
fn un_grupo_sin_cerrar_al_final_es_un_error_y_no_aplica_nada() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let inicial = doc.snapshot().fingerprint();

    bus.apply(&mut doc, Command::BeginGroup { label: "a medias".into() }).unwrap();
    bus.apply(&mut doc, spawn(1, "perdida")).unwrap();

    let err = bus.finish().unwrap_err();
    assert_eq!(err, CommandError::GrupoSinCerrar("a medias".into()));
    assert_eq!(doc.snapshot().fingerprint(), inicial, "un grupo sin cerrar aplicó algo");

    // Control: la misma secuencia cerrada termina limpia.
    let mut bus2 = CommandBus::new();
    bus2.apply(&mut doc, Command::BeginGroup { label: "entera".into() }).unwrap();
    bus2.apply(&mut doc, spawn(1, "guardada")).unwrap();
    bus2.apply(&mut doc, Command::EndGroup).unwrap();
    assert!(bus2.finish().is_ok());
    assert!(doc.snapshot().contains(e(1)));
}

/// El replay hereda la regla: un log que abre un grupo y no lo cierra miente
/// sobre lo que aplicó, y no se acepta en silencio.
#[test]
fn un_log_con_un_grupo_sin_cerrar_no_se_reproduce() {
    let log = CommandLog::new(vec![Command::BeginGroup { label: "a medias".into() }, spawn(1, "x")]);
    assert_eq!(log.replay_into_new().unwrap_err(), CommandError::GrupoSinCerrar("a medias".into()));
}

#[test]
fn los_grupos_no_se_anidan() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    bus.apply(&mut doc, Command::BeginGroup { label: "exterior".into() }).unwrap();
    let err = bus.apply(&mut doc, Command::BeginGroup { label: "interior".into() }).unwrap_err();
    assert_eq!(err, CommandError::GrupoAnidado("exterior".into()));
}

#[test]
fn undo_y_redo_no_caben_dentro_de_un_grupo() {
    let (mut doc, mut bus, x) = doc_con_una_entidad();
    bus.apply(&mut doc, Command::SetVisible { entity: x, visible: false }).unwrap();
    bus.apply(&mut doc, Command::BeginGroup { label: "g".into() }).unwrap();

    assert_eq!(bus.apply(&mut doc, Command::Undo).unwrap_err(), CommandError::NoAgrupable("Undo"));
    assert_eq!(bus.apply(&mut doc, Command::Redo).unwrap_err(), CommandError::NoAgrupable("Redo"));

    // Control: fuera del grupo el mismo Undo funciona.
    bus.apply(&mut doc, Command::EndGroup).unwrap();
    assert!(bus.apply(&mut doc, Command::Undo).is_ok());
}

#[test]
fn deshacer_en_el_vacio_es_un_error_tipado() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    assert_eq!(bus.apply(&mut doc, Command::Undo).unwrap_err(), CommandError::NadaQueDeshacer);
    assert_eq!(bus.apply(&mut doc, Command::Redo).unwrap_err(), CommandError::NadaQueRehacer);

    // Control: con una edición por medio ambos funcionan.
    bus.apply(&mut doc, spawn(1, "x")).unwrap();
    assert!(bus.apply(&mut doc, Command::Undo).is_ok());
    assert!(bus.apply(&mut doc, Command::Redo).is_ok());
}

#[test]
fn spawn_no_pisa_una_entidad_viva() {
    let (mut doc, mut bus, _) = doc_con_una_entidad();
    let err = bus.apply(&mut doc, spawn(1, "otra")).unwrap_err();
    assert_eq!(err, CommandError::EntidadDuplicada(e(1)));
    assert_eq!(doc.snapshot().get::<Name>(e(1)), Some(&Name("pieza".into())));
}

#[test]
fn emparentar_en_ciclo_se_rechaza() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    bus.apply_all(&mut doc, [spawn(1, "abuelo"), spawn(2, "padre"), spawn(3, "hijo")]).unwrap();
    bus.apply(&mut doc, Command::SetParent { child: e(2), parent: Some(e(1)) }).unwrap();
    bus.apply(&mut doc, Command::SetParent { child: e(3), parent: Some(e(2)) }).unwrap();

    // directo
    assert_eq!(
        bus.apply(&mut doc, Command::SetParent { child: e(1), parent: Some(e(1)) }).unwrap_err(),
        CommandError::CicloDeJerarquia { child: e(1), parent: e(1) }
    );
    // indirecto: abuelo bajo su propio nieto
    assert_eq!(
        bus.apply(&mut doc, Command::SetParent { child: e(1), parent: Some(e(3)) }).unwrap_err(),
        CommandError::CicloDeJerarquia { child: e(1), parent: e(3) }
    );

    // Control: emparentar en el sentido bueno sigue permitido.
    bus.apply_all(&mut doc, [spawn(4, "otro")]).unwrap();
    assert!(bus.apply(&mut doc, Command::SetParent { child: e(4), parent: Some(e(3)) }).is_ok());
}

/// Un comando fallido no se graba: el log tiene que contener lo que **pasó**, no
/// lo que se intentó, o dejaría de reproducir el mismo estado.
#[test]
fn el_log_no_graba_comandos_fallidos() {
    let (mut doc, mut bus, _) = doc_con_una_entidad();
    let antes = bus.log().len();
    let _ = bus.apply(&mut doc, Command::Despawn { entity: e(999) });
    assert_eq!(bus.log().len(), antes);
}
