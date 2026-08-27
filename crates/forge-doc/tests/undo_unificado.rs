//! Criterio de aceptación de la Fase 1 para el undo.
//!
//! «10 000 operaciones aleatorias de undo/redo sin divergencia respecto a la
//! re-ejecución desde cero.»
//!
//! El test tiene dos mitades y las dos hacen falta:
//!
//! 1. **Re-ejecución desde cero.** Para cada k, un documento nuevo al que se le
//!    aplican las k primeras ediciones tiene que dar exactamente la misma huella
//!    que el documento vivo tras deshacer hasta k. Sin esta mitad, el test solo
//!    comprobaría que el undo es consistente consigo mismo — cosa que un undo
//!    que no hace nada también cumple.
//! 2. **Paseo aleatorio.** 10 000 undo/redo mezclados contra un cursor sombra
//!    llevado aparte.

use forge_doc::*;
use forge_math::{DQuat, DVec3};
use forge_store::BlobHash;

/// xorshift64*. Determinista y sin dependencias: un test de propiedad que no se
/// puede reproducir con exactitud no sirve para diagnosticar nada.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
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
}

/// Una edición, descrita de forma que se pueda reproducir idénticamente.
/// Deliberadamente cruza los cuatro pilares: nombre y jerarquía (escena),
/// geometría con blob (kernel/malla), visibilidad (render).
#[derive(Clone, Debug)]
enum Edicion {
    Crear {
        idx: u128,
        nombre: String,
    },
    Mover {
        idx: u128,
        d: f64,
    },
    Ocultar {
        idx: u128,
        v: bool,
    },
    Emparentar {
        hijo: u128,
        padre: u128,
    },
    PonerGeometria {
        idx: u128,
        exacta: bool,
        semilla: u64,
    },
    QuitarNombre {
        idx: u128,
    },
    Borrar {
        idx: u128,
    },
}

fn generar(rng: &mut Rng, vivos: &mut Vec<u128>, siguiente: &mut u128) -> Edicion {
    // 40% de crear mientras haya pocas entidades: hace falta poblacion para que
    // el resto de las operaciones sean interesantes.
    let crear = vivos.len() < 8 || rng.below(100) < 30;
    if crear {
        let idx = *siguiente;
        *siguiente += 1;
        vivos.push(idx);
        return Edicion::Crear {
            idx,
            nombre: format!("pieza-{idx}"),
        };
    }
    let pick = |r: &mut Rng, v: &Vec<u128>| v[r.below(v.len() as u64)];
    match rng.below(6) {
        0 => Edicion::Mover {
            idx: pick(rng, vivos),
            d: (rng.below(2000) as f64) / 10.0 - 100.0,
        },
        1 => Edicion::Ocultar {
            idx: pick(rng, vivos),
            v: rng.below(2) == 0,
        },
        2 => {
            let hijo = pick(rng, vivos);
            let padre = pick(rng, vivos);
            Edicion::Emparentar { hijo, padre }
        }
        3 => Edicion::PonerGeometria {
            idx: pick(rng, vivos),
            exacta: rng.below(2) == 0,
            semilla: rng.next(),
        },
        4 => Edicion::QuitarNombre {
            idx: pick(rng, vivos),
        },
        _ => {
            let i = rng.below(vivos.len() as u64);
            let idx = vivos.remove(i);
            Edicion::Borrar { idx }
        }
    }
}

fn aplicar(doc: &mut Document, ed: &Edicion) {
    let e = |i: u128| EntityId::from_u128(i);
    match ed {
        Edicion::Crear { idx, nombre } => doc.edit(format!("crear {nombre}"), |tx| {
            let id = tx.spawn_with_id(e(*idx));
            tx.set(id, Name(nombre.clone()));
            tx.set(id, Transform::IDENTITY);
            tx.set(id, Visible(true));
        }),
        Edicion::Mover { idx, d } => doc.edit("mover", |tx| {
            let mut t = tx
                .get::<Transform>(e(*idx))
                .copied()
                .unwrap_or(Transform::IDENTITY);
            t.translation += DVec3::new(*d, *d * 0.5, 0.0);
            t.rotation = DQuat::from_rotation_z(*d * 0.01);
            tx.set(e(*idx), t);
        }),
        Edicion::Ocultar { idx, v } => doc.edit("visibilidad", |tx| {
            tx.set(e(*idx), Visible(*v));
        }),
        Edicion::Emparentar { hijo, padre } => doc.edit("emparentar", |tx| {
            tx.set(e(*hijo), Parent(e(*padre)));
        }),
        Edicion::PonerGeometria {
            idx,
            exacta,
            semilla,
        } => doc.edit("geometria", |tx| {
            let h = BlobHash::of(&semilla.to_le_bytes());
            let p = if *exacta {
                GeometryPayload::Brep(h)
            } else {
                GeometryPayload::Mesh(h)
            };
            tx.set(e(*idx), Geometry(p));
        }),
        Edicion::QuitarNombre { idx } => doc.edit("quitar nombre", |tx| {
            tx.remove::<Name>(e(*idx));
        }),
        Edicion::Borrar { idx } => doc.edit("borrar", |tx| {
            tx.despawn(e(*idx));
        }),
    }
}

fn guion(seed: u64, n: usize) -> Vec<Edicion> {
    let mut rng = Rng::new(seed);
    let mut vivos = Vec::new();
    let mut siguiente = 1u128;
    (0..n)
        .map(|_| generar(&mut rng, &mut vivos, &mut siguiente))
        .collect()
}

#[test]
fn undo_redo_no_diverge_de_la_reejecucion_desde_cero() {
    const EDICIONES: usize = 200;
    const PASEOS: usize = 10_000;

    let guion = guion(0xF0_11_6E, EDICIONES);

    // --- documento vivo, huella tras cada commit ---
    let mut doc = Document::new();
    doc.history_limit = usize::MAX;
    let mut esperado = vec![doc.snapshot().fingerprint()]; // version 0
    for ed in &guion {
        aplicar(&mut doc, ed);
        esperado.push(doc.snapshot().fingerprint());
    }
    assert_eq!(esperado.len(), EDICIONES + 1);

    // Las huellas tienen que ser mayoritariamente distintas: si el documento no
    // cambiara, el resto del test pasaria trivialmente.
    let distintas: std::collections::BTreeSet<_> = esperado.iter().collect();
    assert!(
        distintas.len() > EDICIONES / 2,
        "las ediciones apenas cambian el documento ({} huellas distintas de {}); \
         el test no estaria probando nada",
        distintas.len(),
        esperado.len()
    );

    // --- mitad 1: re-ejecucion desde cero ---
    for k in 0..=EDICIONES {
        let mut fresco = Document::new();
        fresco.history_limit = usize::MAX;
        for ed in &guion[..k] {
            aplicar(&mut fresco, ed);
        }
        assert_eq!(
            fresco.snapshot().fingerprint(),
            esperado[k],
            "re-ejecutar las primeras {k} ediciones desde cero no reproduce el estado"
        );
    }

    // --- mitad 2: paseo aleatorio de undo/redo ---
    let mut rng = Rng::new(0xDEAD_BEEF);
    let mut cursor = EDICIONES; // el documento esta en la ultima version
    let mut undos = 0usize;
    let mut redos = 0usize;
    for paso in 0..PASEOS {
        if rng.below(2) == 0 {
            if doc.undo().is_some() {
                cursor -= 1;
                undos += 1;
            } else {
                assert_eq!(
                    cursor, 0,
                    "undo devolvio None sin estar en la version inicial"
                );
            }
        } else if doc.redo().is_some() {
            cursor += 1;
            redos += 1;
        } else {
            assert_eq!(
                cursor, EDICIONES,
                "redo devolvio None sin estar en la ultima version"
            );
        }

        assert_eq!(
            doc.snapshot().fingerprint(),
            esperado[cursor],
            "divergencia en el paso {paso} del paseo (cursor {cursor})"
        );
    }

    assert!(
        undos > 1000 && redos > 1000,
        "el paseo no ejercito las dos direcciones"
    );
}

#[test]
fn editar_despues_de_deshacer_descarta_el_rehacer() {
    let mut doc = Document::new();
    let a = doc.edit("a", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("a".into()));
        e
    });
    doc.edit("b", |tx| tx.set(a, Name("b".into())));
    assert_eq!(doc.snapshot().get::<Name>(a).unwrap().0, "b");

    doc.undo();
    assert_eq!(doc.snapshot().get::<Name>(a).unwrap().0, "a");
    assert!(doc.can_redo());

    doc.edit("c", |tx| tx.set(a, Name("c".into())));
    assert!(
        !doc.can_redo(),
        "una edicion tras deshacer abre rama y descarta el rehacer"
    );
    assert_eq!(doc.snapshot().get::<Name>(a).unwrap().0, "c");
}

/// El punto de ADR-0004: una operacion que toca varios pilares es **una** entrada
/// de undo. Aqui se tocan escena, geometria y render en la misma transaccion.
#[test]
fn una_operacion_mixta_es_un_solo_undo() {
    let mut doc = Document::new();
    let antes = doc.snapshot().fingerprint();

    let e = doc.edit("importar pieza", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("soporte".into())); // escena
        tx.set(e, Transform::from_translation(DVec3::new(1.0, 2.0, 3.0))); // escena
        tx.set(e, Geometry(GeometryPayload::Brep(BlobHash::of(b"solido")))); // kernel
        tx.set(e, Visible(true)); // render
        e
    });

    let s = doc.snapshot();
    assert!(s.get::<Name>(e).is_some() && s.get::<Geometry>(e).is_some());

    doc.undo();
    assert_eq!(
        doc.snapshot().fingerprint(),
        antes,
        "un solo Ctrl+Z revierte los cuatro cambios"
    );
    assert!(!doc.can_undo());
}

#[test]
fn transaccion_sin_commit_no_cambia_nada() {
    let mut doc = Document::new();
    let antes = doc.snapshot().fingerprint();
    {
        let mut tx = doc.begin();
        let e = tx.spawn();
        tx.set(e, Name("fantasma".into()));
        assert_eq!(tx.entity_count(), 1);
        // se cae del scope sin commit
    }
    assert_eq!(doc.snapshot().fingerprint(), antes);
    assert_eq!(doc.snapshot().entity_count(), 0);
    assert!(!doc.can_undo());

    let mut tx = doc.begin();
    let e = tx.spawn();
    tx.set(e, Name("descartado".into()));
    tx.rollback();
    assert_eq!(doc.snapshot().fingerprint(), antes);
}

#[test]
fn la_poda_del_historial_conserva_el_estado_actual() {
    let mut doc = Document::new();
    doc.history_limit = 8;
    let e = doc.edit("crear", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("x".into()));
        e
    });
    for i in 0..50 {
        doc.edit(format!("edicion {i}"), |tx| {
            tx.set(e, Name(format!("n{i}")))
        });
    }
    assert_eq!(doc.snapshot().get::<Name>(e).unwrap().0, "n49");
    // se puede deshacer hasta el limite, y ni un paso mas
    let mut pasos = 0;
    while doc.undo().is_some() {
        pasos += 1;
        assert!(pasos <= 8, "la poda no acoto el historial");
    }
    assert_eq!(pasos, 7, "con limite 8 quedan 7 pasos de deshacer");
    assert!(
        doc.snapshot().get::<Name>(e).is_some(),
        "la poda no debe perder el estado"
    );
}
