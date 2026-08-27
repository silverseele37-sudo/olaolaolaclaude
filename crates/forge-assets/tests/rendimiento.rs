//! Criterio de aceptación de la Fase 5: **búsqueda por debajo de 100 ms sobre
//! 100 000 activos**.
//!
//! Marcado `#[ignore]` porque construir el corpus tarda; se ejecuta con
//! `cargo test -p forge-assets --release -- --ignored --nocapture`.
//!
//! El número que sale por pantalla es el entregable, no el `assert`: si un día
//! deja de cumplirse, lo útil es saber por cuánto.
//!
//! # Lo que este test encontró
//!
//! La primera medición delató que `ORDER BY a.id` obligaba a ordenar **todas**
//! las coincidencias antes de aplicar el `LIMIT` cuando el filtro usaba un
//! índice de una sola columna. Los índices compuestos `(columna, id)` de la
//! versión 2 del esquema lo arreglaron, medido sobre 100 000 activos:
//!
//! ```text
//! consulta            acotada antes   acotada despues
//! un tipo                  14.44 ms          40.50 us     356x
//! rango de tamano          21.25 ms          11.24 ms       2x
//! dos etiquetas (OR)      162.71 us         172.43 us        =
//! ```
//!
//! `rango de tamano` sigue siendo lento porque `BETWEEN 0 AND u64::MAX` cubre
//! el corpus entero: recorrer el indice es recorrerlo todo, y ahi no hay indice
//! que ayude. `combinada` tampoco mejora: la domina el `EXISTS` correlacionado
//! de las etiquetas, una fila a la vez.

use std::time::Instant;

use forge_assets::{AssetMeta, AssetQuery, AssetStore, AssetType};

const N: usize = 100_000;

fn tipo_de(i: usize) -> AssetType {
    match i % 6 {
        0 => AssetType::Modelo,
        1 => AssetType::Textura,
        2 => AssetType::Material,
        3 => AssetType::Referencia,
        4 => AssetType::Documento,
        _ => AssetType::Nota,
    }
}

#[test]
#[ignore = "construye 100 000 activos; correr con --release --ignored"]
fn busqueda_por_debajo_de_100ms_sobre_cien_mil_activos() {
    let d = tempfile::tempdir().expect("tempdir");
    let mut s = AssetStore::open(d.path()).expect("abrir");

    let t0 = Instant::now();
    // En un solo lote: 100 000 transacciones sueltas medirían el `fsync` de
    // SQLite, no la importación.
    s.batch(|s| {
        for i in 0..N {
            // Contenido único por activo: si se repitiera, la deduplicación
            // colapsaría el corpus y el test mediría sobre muchos menos.
            let bytes = format!("activo-{i}").into_bytes();
            let meta = AssetMeta::new(format!("pieza {i} valvula"), tipo_de(i))
                .with_tags(if i % 3 == 0 {
                    vec!["mecanica", "metal"]
                } else if i % 3 == 1 {
                    vec!["arquitectura"]
                } else {
                    vec!["metal"]
                })
                .with_notes(if i % 7 == 0 { "revisar tolerancia" } else { "" });
            s.import_bytes(format!("/lote/{i}.dat"), &bytes, meta)?;
        }
        Ok(())
    })
    .expect("lote");

    let construccion = t0.elapsed();
    assert_eq!(s.len().expect("len"), N as u64, "el corpus no tiene el tamano esperado");
    println!("\n  construccion de {N} activos: {:.2?}", construccion);

    // Consultas de forma distinta: cada una ejercita índices diferentes.
    let casos: Vec<(&str, AssetQuery)> = vec![
        ("texto en nombre", AssetQuery::new().with_text("valvula")),
        ("texto raro", AssetQuery::new().with_text("tolerancia")),
        ("una etiqueta", AssetQuery::new().with_any_tags(["mecanica"])),
        ("dos etiquetas (OR)", AssetQuery::new().with_any_tags(["mecanica", "arquitectura"])),
        ("un tipo", AssetQuery::new().with_types([AssetType::Modelo])),
        ("rango de tamano", AssetQuery::new().size_between(0, u64::MAX)),
        (
            "combinada",
            AssetQuery::new()
                .with_text("valvula")
                .with_types([AssetType::Modelo, AssetType::Nota])
                .with_any_tags(["metal"])
                .with_limit(100),
        ),
    ];

    // Se mide cada consulta de dos formas, y la distincion importa:
    //
    // - **acotada** (`limit 100`): lo que hace de verdad una interfaz, que
    //   pinta una pagina de resultados. Es el criterio de los 100 ms.
    // - **sin acotar**: devuelve *todas* las coincidencias. Con una consulta
    //   poco selectiva eso son decenas de miles de filas, y el tiempo lo domina
    //   materializar el resultado -- alojar un `String` por ULID y parsearlo --
    //   no buscar. Llamar «busqueda» a un volcado del corpus mezcla dos cosas
    //   distintas, asi que se informa aparte en vez de meterlo en el criterio.
    println!(
        "  {:<24} {:>10} {:>12} {:>12}",
        "consulta", "resultados", "acotada", "completa"
    );
    let mut peor_acotada = std::time::Duration::ZERO;
    let mut peor_nombre = "";
    let mut peor_completa = std::time::Duration::ZERO;

    for (nombre, q) in &casos {
        // Una pasada en frio y otra medida: la primera calienta la cache de
        // paginas de SQLite, y medir la de frio mediria el disco.
        let _ = s.search(q).expect("busqueda");

        let acotada = q.clone().with_limit(100);
        let _ = s.search(&acotada).expect("busqueda");
        let t = Instant::now();
        let r_ac = s.search(&acotada).expect("busqueda");
        let dt_ac = t.elapsed();

        let t = Instant::now();
        let r = s.search(q).expect("busqueda");
        let dt = t.elapsed();

        println!("  {nombre:<24} {:>10} {:>12.2?} {:>12.2?}", r.len(), dt_ac, dt);
        let _ = r_ac;
        if dt_ac > peor_acotada {
            peor_acotada = dt_ac;
            peor_nombre = nombre;
        }
        peor_completa = peor_completa.max(dt);
    }

    println!(
        "\n  peor acotada: {peor_nombre} en {:.2?}   ·   peor completa: {:.2?}\n",
        peor_acotada, peor_completa
    );
    assert!(
        peor_acotada.as_millis() < 100,
        "la peor consulta acotada ({peor_nombre}) tardo {peor_acotada:.2?}, el criterio es <100 ms"
    );
    // Y un techo, mucho mas laxo, para el volcado completo: no es el criterio,
    // pero una regresion de un orden de magnitud tiene que saltar igual.
    assert!(
        peor_completa.as_millis() < 2_000,
        "el volcado completo tardo {peor_completa:.2?}: eso ya no es materializar, es un plan malo"
    );
}

/// El otro criterio de la fase: **el índice se reconstruye entero desde los
/// blobs y el diario**. Con 100 000 activos, para que el número signifique algo.
#[test]
#[ignore = "reconstruye un indice de 100 000 activos"]
fn el_indice_se_reconstruye_desde_cero_a_escala() {
    let d = tempfile::tempdir().expect("tempdir");
    let mut s = AssetStore::open(d.path()).expect("abrir");
    s.batch(|s| {
        for i in 0..N {
            let bytes = format!("activo-{i}").into_bytes();
            s.import_bytes(
                format!("/lote/{i}.dat"),
                &bytes,
                AssetMeta::new(format!("pieza {i}"), tipo_de(i)),
            )?;
        }
        Ok(())
    })
    .expect("lote");

    let antes = s.search(&AssetQuery::new()).expect("busqueda").len();
    let t = Instant::now();
    let rep = s.reindex().expect("reindex");
    let dt = t.elapsed();

    println!("\n  reindex de {N} activos: {:.2?}  ({rep:?})\n", dt);
    assert_eq!(s.len().expect("len"), N as u64, "la reconstruccion perdio activos");
    assert_eq!(
        s.search(&AssetQuery::new()).expect("busqueda").len(),
        antes,
        "la busqueda no devuelve lo mismo tras reconstruir: el indice no es reproducible"
    );
}
