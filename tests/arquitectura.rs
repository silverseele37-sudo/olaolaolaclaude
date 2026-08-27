//! Guardia de la arquitectura.
//!
//! El documento maestro dice: «ningún pilar depende de otro pilar; un test de CI
//! recorre el grafo de dependencias y falla el build si aparece una arista
//! prohibida. Sin esa verificación mecánica, la regla dura seis semanas.»
//!
//! Esto es esa verificación. Se escribe con cuatro crates para que exista antes
//! de que haya doce.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn raiz() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Dependencias `forge-*` declaradas por cada crate del workspace.
fn grafo() -> BTreeMap<String, BTreeSet<String>> {
    let mut g = BTreeMap::new();
    let dir = raiz().join("crates");
    for e in std::fs::read_dir(&dir).expect("falta crates/").flatten() {
        let manifiesto = e.path().join("Cargo.toml");
        if !manifiesto.exists() {
            continue;
        }
        let txt = std::fs::read_to_string(&manifiesto).unwrap();
        let v: toml::Table = toml::from_str(&txt)
            .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", manifiesto.display()));
        let nombre = v["package"]["name"].as_str().unwrap().to_string();

        let mut deps = BTreeSet::new();
        for seccion in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(t) = v.get(seccion).and_then(|d| d.as_table()) {
                for k in t.keys() {
                    if k.starts_with("forge-") {
                        deps.insert(k.clone());
                    }
                }
            }
        }
        g.insert(nombre, deps);
    }
    g
}

/// Qué puede depender de qué. Cualquier arista fuera de esta tabla rompe el
/// build, y añadirla exige tocar este archivo — que es exactamente el punto:
/// que abrir una frontera sea una decisión visible en el diff, no un `use`.
fn permitido() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let m: &[(&str, &[&str])] = &[
        // --- núcleo ---
        ("forge-math", &[]),
        ("forge-store", &[]),
        ("forge-doc", &["forge-math", "forge-store"]),
        ("forge-io", &["forge-doc", "forge-store", "forge-math"]),
        // --- contratos (sin lógica) ---
        (
            "forge-kernel-api",
            &["forge-math", "forge-doc", "forge-store"],
        ),
        (
            "forge-render-api",
            &["forge-math", "forge-doc", "forge-store"],
        ),
        ("forge-material-api", &["forge-math"]),
        // --- los cuatro pilares: ninguno menciona a otro ---
        (
            "forge-param",
            &[
                "forge-math",
                "forge-store",
                "forge-doc",
                "forge-kernel-api",
                "forge-kernel-stub",
            ],
        ),
        (
            "forge-mesh",
            &["forge-math", "forge-store", "forge-doc", "forge-kernel-api"],
        ),
        (
            "forge-render",
            &[
                "forge-math",
                "forge-store",
                "forge-doc",
                "forge-render-api",
                "forge-material-api",
            ],
        ),
        // Rasterizador por software: implementa el mismo trait Renderer que la
        // version wgpu. No es un juguete -- es lo que permite tests por imagen
        // de referencia sin GPU, que es el hueco de verificacion mas caro que
        // tiene un motor de render (y el que cadviz identifico como el suyo).
        (
            "forge-render-cpu",
            &["forge-math", "forge-store", "forge-doc", "forge-render-api"],
        ),
        ("forge-assets", &["forge-math", "forge-store", "forge-doc"]),
        // --- implementaciones y servicios ---
        (
            "forge-kernel-occt",
            &["forge-math", "forge-doc", "forge-kernel-api"],
        ),
        (
            "forge-kernel-stub",
            &["forge-math", "forge-doc", "forge-kernel-api"],
        ),
        ("forge-material", &["forge-math", "forge-material-api"]),
        ("forge-assets", &["forge-math", "forge-store", "forge-doc"]),
        (
            "forge-interop",
            &["forge-math", "forge-store", "forge-doc", "forge-kernel-api"],
        ),
        ("forge-script", &["forge-math", "forge-doc", "forge-store"]),
        // --- aplicación: es la única que puede verlo todo ---
        // `forge-escena` es la traduccion Snapshot -> DrawInstance. Vive sola
        // porque la usan los dos consumidores: el editor y el reproductor sin
        // editor. No puede depender de ninguna implementacion de render ni de
        // ningun kernel -- si lo hiciera dejaria de ser una frontera y pasaria a
        // ser una capa mas del editor.
        (
            "forge-escena",
            &["forge-math", "forge-store", "forge-doc", "forge-render-api"],
        ),
        (
            "forge-ui",
            &[
                "forge-math",
                "forge-store",
                "forge-doc",
                "forge-escena",
                "forge-param",
                "forge-mesh",
                "forge-render",
                "forge-assets",
                "forge-script",
                "forge-render-api",
            ],
        ),
        (
            "forge-app",
            &[
                "forge-math",
                "forge-store",
                "forge-doc",
                "forge-io",
                "forge-ui",
                "forge-param",
                "forge-mesh",
                "forge-render",
                "forge-assets",
                "forge-script",
                "forge-interop",
                "forge-kernel-occt",
                "forge-material",
            ],
        ),
        // El runtime depende del CONTRATO de render, no de una implementacion
        // concreta. Esa es justamente la propiedad que lo hace valioso: carga el
        // mismo documento y lo renderiza con el rasterizador por software o con
        // wgpu sin cambiar una linea, que es lo que demuestra que el render no
        // depende del editor.
        //
        // La tabla decia `forge-render` a secas y un agente choco contra ello.
        // Tenia razon: la frontera estaba mal descrita, no el codigo. Ese es el
        // valor de un guardia -- obliga a decidir la regla en vez de dejar que
        // se decida sola con el primer `use`.
        (
            "forge-runtime",
            &[
                "forge-math",
                "forge-store",
                "forge-doc",
                "forge-io",
                "forge-render-api",
                "forge-render",
                "forge-render-cpu",
                "forge-material",
                "forge-escena",
                // El reproductor decodifica los blobs de malla del documento, y
                // esos blobs son GLB (docs/formato/README.md, 4.1). Leer un
                // formato de archivo es exactamente lo que `forge-interop`
                // hace; la alternativa era un segundo lector de GLB dentro del
                // runtime, que es peor por donde se mire.
                "forge-interop",
            ],
        ),
    ];
    m.iter()
        .map(|(k, v)| (*k, v.iter().copied().collect()))
        .collect()
}

/// Funcion pura, para poder probar que el guardia **detecta** algo y no solo
/// que no encuentra nada. Un verificador que siempre devuelve la lista vacia
/// pasa el test de arriba igual de bien que uno correcto.
fn violaciones(
    g: &BTreeMap<String, BTreeSet<String>>,
    p: &BTreeMap<&'static str, BTreeSet<&'static str>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (crate_, deps) in g {
        let Some(ok) = p.get(crate_.as_str()) else {
            out.push(format!(
                "el crate `{crate_}` no esta en la tabla de fronteras de tests/arquitectura.rs; \
                 anadelo con sus dependencias permitidas"
            ));
            continue;
        };
        for d in deps {
            if !ok.contains(d.as_str()) {
                out.push(format!("arista prohibida: {crate_} -> {d}"));
            }
        }
    }
    out
}

#[test]
fn ningun_crate_cruza_una_frontera_no_declarada() {
    let g = grafo();
    assert!(!g.is_empty(), "no se leyo ningun crate");
    let v = violaciones(&g, &permitido());
    assert!(v.is_empty(), "fronteras rotas:\n  {}", v.join("\n  "));
}

/// Control positivo del guardia.
#[test]
fn el_guardia_detecta_las_aristas_prohibidas() {
    let p = permitido();

    // un pilar mirando a otro pilar
    let mut malo = BTreeMap::new();
    malo.insert(
        "forge-render".to_string(),
        ["forge-param".to_string()].into_iter().collect(),
    );
    let v = violaciones(&malo, &p);
    assert_eq!(v.len(), 1, "no detecto pilar -> pilar");
    assert!(v[0].contains("forge-render -> forge-param"));

    // el nucleo mirando hacia arriba
    let mut malo2 = BTreeMap::new();
    malo2.insert(
        "forge-doc".to_string(),
        ["forge-render".to_string()].into_iter().collect(),
    );
    assert_eq!(
        violaciones(&malo2, &p).len(),
        1,
        "no detecto nucleo -> pilar"
    );

    // la fuga de cadviz: render arrastrando el kernel
    let mut malo3 = BTreeMap::new();
    malo3.insert(
        "forge-render".to_string(),
        ["forge-kernel-occt".to_string()].into_iter().collect(),
    );
    assert_eq!(
        violaciones(&malo3, &p).len(),
        1,
        "no detecto render -> kernel"
    );

    // un crate nuevo sin declarar tambien es una violacion: anadir un crate
    // obliga a decidir sus fronteras en el mismo commit
    let mut malo4 = BTreeMap::new();
    malo4.insert("forge-sorpresa".to_string(), BTreeSet::new());
    assert_eq!(
        violaciones(&malo4, &p).len(),
        1,
        "no detecto crate sin declarar"
    );

    // y el grafo real sigue limpio
    assert!(violaciones(&grafo(), &p).is_empty());
}

/// La regla central de ADR-0006, dicha aparte para que el mensaje de error
/// explique *por que* cuando alguien la rompa.
#[test]
fn los_cuatro_pilares_no_se_conocen_entre_si() {
    let pilares = [
        "forge-param",
        "forge-mesh",
        "forge-render",
        "forge-render-cpu",
        "forge-assets",
    ];
    let g = grafo();
    for (crate_, deps) in &g {
        if !pilares.contains(&crate_.as_str()) {
            continue;
        }
        for d in deps {
            assert!(
                !pilares.contains(&d.as_str()),
                "{crate_} depende de {d}. Los pilares se comunican por comandos sobre \
                 forge-doc y por eventos, nunca por dependencia directa (ADR-0006). \
                 Si hace falta compartir un tipo, va a un crate de contratos."
            );
        }
    }
}

/// La fuga que encontramos en el `Cargo.lock` de cadviz: alli `render` depende de
/// `kernel`, que arrastra `occt-sys` y `cc`, con lo que el renderer no se puede
/// compilar ni testear sin la cadena de C++ en el grafo de build. En un visor es
/// un detalle; aqui seria una frontera rota. Ver ADR-0007.
#[test]
fn el_render_no_arrastra_el_kernel_ni_su_cadena_de_cpp() {
    let g = grafo();
    for r in ["forge-render", "forge-render-cpu"] {
        if let Some(deps) = g.get(r) {
            for d in deps {
                assert!(
                    !d.starts_with("forge-kernel"),
                    "{r} -> {d}: el renderer consume una vista aplanada de la escena, \
                 no el kernel. Los tipos de datos compartidos van a forge-render-api."
                );
            }
        }
    }
}

/// El nucleo no puede depender de nada de arriba: es lo que permite testearlo
/// sin GPU, sin kernel y sin interfaz.
#[test]
fn el_nucleo_no_depende_de_nadie_por_encima() {
    let g = grafo();
    for base in ["forge-math", "forge-store"] {
        if let Some(deps) = g.get(base) {
            assert!(
                deps.is_empty(),
                "{base} deberia no tener dependencias forge-*, tiene {deps:?}"
            );
        }
    }
    if let Some(deps) = g.get("forge-doc") {
        let ok: BTreeSet<&str> = ["forge-math", "forge-store"].into_iter().collect();
        for d in deps {
            assert!(
                ok.contains(d.as_str()),
                "forge-doc -> {d}: el documento no conoce ningun pilar"
            );
        }
    }
}
