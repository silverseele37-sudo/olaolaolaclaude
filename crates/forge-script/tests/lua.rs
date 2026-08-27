//! El host de Lua.
//!
//! Lo que aquí se prueba no es «Lua funciona» sino las tres afirmaciones de
//! ADR-0006 que justifican haberlo embebido: que un script solo puede hablar por
//! el bus, que un bucle infinito **se corta** en vez de colgar el editor, y que
//! un error de Lua vuelve como `Result` y nunca como pánico.

#![cfg(feature = "lua")]

use forge_doc::{Document, Name, Transform, Visible};
use forge_script::{Command, CommandBus, CommandError, Limits, LuaHost, ScriptError};

fn host() -> LuaHost {
    LuaHost::new(Limits::default()).expect("crear el intérprete")
}

// ---------------------------------------------------------------------------
// Respuesta conocida
// ---------------------------------------------------------------------------

#[test]
fn un_script_crea_tres_entidades() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    h.run(
        &mut doc,
        &mut bus,
        r#"
        for i = 1, 3 do
          local e = forge.spawn("cubo " .. i)
          forge.set_transform(e, { tx = i * 10.0 })
          forge.set_visible(e, i ~= 2)
        end
        "#,
    )
    .expect("el script debe correr limpio");
    bus.finish().unwrap();

    let s = doc.snapshot();
    assert_eq!(s.entity_count(), 3);

    let mut nombres: Vec<String> = s.iter::<Name>().map(|(_, n)| n.0.clone()).collect();
    nombres.sort();
    assert_eq!(nombres, ["cubo 1", "cubo 2", "cubo 3"]);

    // Los valores, no solo el recuento: un puente que ignorase los argumentos
    // pasaría un test que solo contara entidades.
    let ids: Vec<_> = s.entities().collect();
    let mut xs: Vec<f64> = ids
        .iter()
        .filter_map(|e| s.get::<Transform>(*e))
        .map(|t| t.translation.x)
        .collect();
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [10.0, 20.0, 30.0]);

    let ocultas = ids
        .iter()
        .filter(|e| s.get::<Visible>(**e) == Some(&Visible(false)))
        .count();
    assert_eq!(ocultas, 1, "solo el cubo 2 debía quedar oculto");
}

/// Una macro de Lua tiene que comportarse como un comando nativo: un `Ctrl+Z`.
#[test]
fn una_macro_agrupada_se_deshace_de_una_vez() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let inicial = doc.snapshot().fingerprint();

    h.run(
        &mut doc,
        &mut bus,
        r#"
        forge.begin_group("tres cubos")
        for i = 1, 3 do forge.spawn("cubo " .. i) end
        forge.end_group()
        "#,
    )
    .unwrap();
    bus.finish().unwrap();
    assert_eq!(doc.snapshot().entity_count(), 3);

    bus.apply(&mut doc, Command::Undo).unwrap();
    assert_eq!(
        doc.snapshot().fingerprint(),
        inicial,
        "la macro no era una sola entrada de undo"
    );
}

/// El log grabado desde Lua es un log como cualquier otro: se reproduce y da la
/// misma huella. Es lo que hace que una macro se pueda grabar y repetir.
#[test]
fn lo_que_hace_un_script_se_reproduce_desde_el_log() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    h.run(
        &mut doc,
        &mut bus,
        r#"
        local padre = forge.spawn("padre")
        for i = 1, 4 do
          local e = forge.spawn("hijo " .. i)
          forge.set_parent(e, padre)
        end
        "#,
    )
    .unwrap();
    bus.finish().unwrap();

    let log = bus.take_log();
    let copia = log.replay_into_new().unwrap();
    assert_eq!(copia.snapshot().fingerprint(), doc.snapshot().fingerprint());
}

// ---------------------------------------------------------------------------
// Límites de ejecución
// ---------------------------------------------------------------------------

#[test]
fn un_bucle_infinito_se_corta_por_instrucciones() {
    let h = LuaHost::new(Limits {
        max_instrucciones: 100_000,
        ..Limits::default()
    })
    .unwrap();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    let err = h
        .run(&mut doc, &mut bus, "while true do end")
        .expect_err("el bucle infinito debía cortarse");
    assert!(
        matches!(err, ScriptError::LimiteDeInstrucciones { limite: 100_000 }),
        "se cortó por otra razón: {err}"
    );
    assert!(
        err.to_string().contains("revisa el bucle"),
        "el error no dice qué hacer"
    );
}

/// Control positivo del límite: sin él, el test de arriba pasaría igual si el
/// script hubiera fallado por cualquier otro motivo. Este script hace **más**
/// trabajo que el presupuesto que se le da y también se corta; con un
/// presupuesto holgado, el mismo script termina.
#[test]
fn el_limite_de_instrucciones_esta_conectado_de_verdad() {
    let mut doc = Document::new();
    let mut bus = CommandBus::new();
    let script = "local s = 0 for i = 1, 200000 do s = s + i end";

    let apretado = LuaHost::new(Limits {
        max_instrucciones: 10_000,
        ..Limits::default()
    })
    .unwrap();
    assert!(
        matches!(
            apretado.run(&mut doc, &mut bus, script),
            Err(ScriptError::LimiteDeInstrucciones { .. })
        ),
        "con 10 000 instrucciones el bucle no debía terminar"
    );

    let holgado = LuaHost::new(Limits::default()).unwrap();
    holgado
        .run(&mut doc, &mut bus, script)
        .expect("con presupuesto holgado debía terminar");
    assert!(
        holgado.instrucciones_usadas() > 10_000,
        "el contador dice {} instrucciones: el hook no se está ejecutando",
        holgado.instrucciones_usadas()
    );
}

#[test]
fn un_script_que_devora_memoria_se_corta() {
    let h = LuaHost::new(Limits {
        max_instrucciones: 500_000_000,
        max_memoria_bytes: 1024 * 1024,
    })
    .unwrap();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    let err = h
        .run(
            &mut doc,
            &mut bus,
            "local t = {} local i = 1 while true do t[i] = i i = i + 1 end",
        )
        .expect_err("debía cortarse");
    assert!(
        matches!(err, ScriptError::LimiteDeMemoria { .. }),
        "se cortó por instrucciones y no por memoria: {err}"
    );
}

/// El corte deja el documento con lo que ya se había aplicado y nada más: no se
/// pierde trabajo confirmado ni aparece trabajo a medias.
#[test]
fn el_corte_no_deja_el_documento_a_medias() {
    let h = LuaHost::new(Limits {
        max_instrucciones: 200_000,
        ..Limits::default()
    })
    .unwrap();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    let _ = h.run(
        &mut doc,
        &mut bus,
        r#"
        forge.spawn("antes del bucle")
        while true do end
        "#,
    );
    assert_eq!(
        doc.snapshot().entity_count(),
        1,
        "el corte perdió o duplicó trabajo confirmado"
    );
}

// ---------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------

#[test]
fn un_error_de_lua_vuelve_como_err_y_no_como_panico() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    // error de sintaxis
    let err = h.run(&mut doc, &mut bus, "esto no ( es lua").unwrap_err();
    assert!(matches!(err, ScriptError::Lua(_)), "{err}");

    // error en tiempo de ejecución
    let err = h.run(&mut doc, &mut bus, "error('me rindo')").unwrap_err();
    assert!(err.to_string().contains("me rindo"), "{err}");

    // llamada a algo que no existe en la tabla `forge`
    let err = h.run(&mut doc, &mut bus, "forge.extruir(3)").unwrap_err();
    assert!(matches!(err, ScriptError::Lua(_)), "{err}");

    // Control: el intérprete sigue vivo y usable tras los tres fallos.
    h.run(&mut doc, &mut bus, "forge.spawn('viva')").unwrap();
    assert_eq!(doc.snapshot().entity_count(), 1);
}

/// Un error del bus no se degrada a texto: vuelve con su variante, que es lo
/// que la interfaz necesita para decidir qué hacer.
#[test]
fn un_error_del_bus_llega_tipado_a_traves_de_lua() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    let err = h.run(&mut doc, &mut bus, "forge.undo()").unwrap_err();
    match err {
        ScriptError::Comando(CommandError::NadaQueDeshacer) => {}
        otro => panic!("se perdió la variante del error del bus: {otro}"),
    }
}

/// Un id inventado no crea nada ni revienta: es un error con instrucciones.
#[test]
fn un_id_mal_formado_es_un_error_con_instrucciones() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    let err = h
        .run(&mut doc, &mut bus, "forge.set_name('no-soy-un-id', 'x')")
        .unwrap_err();
    assert!(err.to_string().contains("forge.spawn()"), "{err}");
    assert_eq!(doc.snapshot().entity_count(), 0);
}

/// La tabla `forge` no sobrevive a la ejecución: sus funciones prestan el
/// documento y morir con el `scope` es lo que impide que un script se guarde una
/// referencia para usarla después.
#[test]
fn el_script_no_puede_guardarse_la_tabla_forge_para_luego() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    h.run(&mut doc, &mut bus, "guardada = forge.spawn").unwrap();
    let err = h.run(&mut doc, &mut bus, "guardada('tarde')").unwrap_err();
    assert!(matches!(err, ScriptError::Lua(_)), "{err}");
    assert_eq!(
        doc.snapshot().entity_count(),
        0,
        "la función caducada llegó a crear algo"
    );
}

/// Un script que abre un grupo y no lo cierra no aplica nada, y `finish` lo
/// denuncia: la macro a medias no se cuela como si hubiera funcionado.
#[test]
fn un_script_que_no_cierra_su_grupo_no_aplica_nada() {
    let h = host();
    let mut doc = Document::new();
    let mut bus = CommandBus::new();

    h.run(
        &mut doc,
        &mut bus,
        "forge.begin_group('a medias') forge.spawn('perdida')",
    )
    .unwrap();
    assert_eq!(doc.snapshot().entity_count(), 0);
    assert_eq!(
        bus.finish().unwrap_err(),
        CommandError::GrupoSinCerrar("a medias".into())
    );
}
