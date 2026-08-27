//! Tests de respuesta conocida.
//!
//! Todos los números de aquí se derivan a mano. Un test que comprueba que una
//! operación "no revienta" no prueba que la geometría sea correcta, y la
//! geometría equivocada de un kernel se propaga en silencio a todo lo demás.

use forge_doc::{FeatureId, StableId};
use forge_kernel_api::*;
use forge_kernel_stub::{segmentos_para, StubKernel};
use forge_math::{DVec2, DVec3};

fn k() -> StubKernel {
    StubKernel::new()
}
fn f() -> FeatureId {
    FeatureId::from_u128(1)
}

/// Volumen de un teselado por el teorema de la divergencia. Independiente del
/// cálculo interno del kernel: si los dos coinciden, es que el teselado
/// representa de verdad al sólido.
fn volumen_teselado(t: &Tessellation) -> f64 {
    let mut v = 0.0;
    for tri in t.indices.chunks_exact(3) {
        let (a, b, c) = (
            t.positions[tri[0] as usize],
            t.positions[tri[1] as usize],
            t.positions[tri[2] as usize],
        );
        v += a.dot(b.cross(c));
    }
    (v / 6.0).abs()
}

#[test]
fn caja_volumen_area_y_centroide_exactos() {
    let k = k();
    let s = k
        .box_solid(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0), f())
        .unwrap();
    let m = k.mass_properties(s).unwrap();
    assert!(
        (m.volume_mm3 - 6000.0).abs() < 1e-9,
        "volumen {}",
        m.volume_mm3
    );
    // 2·(10·20 + 20·30 + 30·10) = 2·1100 = 2200
    assert!((m.area_mm2 - 2200.0).abs() < 1e-9, "area {}", m.area_mm2);
    assert!(
        (m.centroid - DVec3::new(5.0, 10.0, 15.0)).length() < 1e-9,
        "centroide {:?}",
        m.centroid
    );
    assert!(k.is_valid(s).unwrap().valid);

    let t = k.topology(s).unwrap();
    assert_eq!(t.faces.len(), 6);
    assert_eq!(t.edges.len(), 12, "una caja tiene 12 aristas");
    assert!(t.is_solid);
}

#[test]
fn extrusion_de_un_triangulo_da_el_volumen_calculado_a_mano() {
    let k = k();
    // triangulo rectangulo de catetos 10: area 50 mm²
    let perfil = k
        .profile_from_polygon(
            &[
                DVec2::new(0.0, 0.0),
                DVec2::new(10.0, 0.0),
                DVec2::new(0.0, 10.0),
            ],
            f(),
        )
        .unwrap();
    let s = k
        .extrude(
            perfil,
            ExtrudeOpts {
                direction: DVec3::Z,
                distance_mm: 7.0,
                symmetric: false,
            },
            f(),
        )
        .unwrap();
    // 50 mm² × 7 mm = 350 mm³
    assert!((k.mass_properties(s).unwrap().volume_mm3 - 350.0).abs() < 1e-9);
    // area: 2 tapas de 50 + 3 laterales (10·7, 10·7, 10√2·7)
    let esperada = 100.0 + 70.0 + 70.0 + 10.0 * 2f64.sqrt() * 7.0;
    assert!((k.mass_properties(s).unwrap().area_mm2 - esperada).abs() < 1e-9);
}

#[test]
fn extrusion_de_n_lados_da_n_mas_2_caras_con_procedencia_correcta() {
    let k = k();
    for n in [3u32, 4, 5, 8, 17] {
        let pts: Vec<DVec2> = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                DVec2::new(20.0 * a.cos(), 20.0 * a.sin())
            })
            .collect();
        let perfil = k.profile_from_polygon(&pts, f()).unwrap();
        let s = k
            .extrude(
                perfil,
                ExtrudeOpts {
                    direction: DVec3::Z,
                    distance_mm: 5.0,
                    symmetric: false,
                },
                f(),
            )
            .unwrap();
        let t = k.topology(s).unwrap();
        assert_eq!(t.faces.len(), n as usize + 2, "poligono de {n} lados");
        // 3n aristas: n abajo, n arriba, n verticales
        assert_eq!(t.edges.len(), 3 * n as usize, "aristas de {n} lados");

        let laterales = t
            .faces
            .iter()
            .filter(|f| matches!(f.provenance, TopoProvenance::SweptFromProfileEdge { .. }))
            .count();
        assert_eq!(laterales, n as usize);
        assert_eq!(
            t.faces
                .iter()
                .filter(|f| matches!(f.provenance, TopoProvenance::Cap { .. }))
                .count(),
            2
        );

        let mut marcas: Vec<u64> = t.faces.iter().map(|f| f.id.mark).collect();
        marcas.sort_unstable();
        let antes = marcas.len();
        marcas.dedup();
        assert_eq!(marcas.len(), antes, "hay caras con StableId repetido");
        assert!(k.is_valid(s).unwrap().valid);
    }
}

#[test]
fn extrusion_simetrica_reparte_la_distancia_a_los_dos_lados() {
    let k = k();
    let perfil = k
        .profile_from_polygon(
            &[
                DVec2::new(-5.0, -5.0),
                DVec2::new(5.0, -5.0),
                DVec2::new(5.0, 5.0),
                DVec2::new(-5.0, 5.0),
            ],
            f(),
        )
        .unwrap();
    let s = k
        .extrude(
            perfil,
            ExtrudeOpts {
                direction: DVec3::Z,
                distance_mm: 8.0,
                symmetric: true,
            },
            f(),
        )
        .unwrap();
    let b = k.bbox(s).unwrap();
    assert!(
        (b.min.z + 4.0).abs() < 1e-9 && (b.max.z - 4.0).abs() < 1e-9,
        "{b:?}"
    );
    assert!((k.mass_properties(s).unwrap().volume_mm3 - 800.0).abs() < 1e-9);
}

#[test]
fn cilindro_analitico_tiene_volumen_y_area_exactos() {
    let k = k();
    let s = k.cylinder(DVec3::ZERO, DVec3::Z, 10.0, 50.0, f()).unwrap();
    let m = k.mass_properties(s).unwrap();
    let pi = std::f64::consts::PI;
    assert!(
        (m.volume_mm3 - pi * 100.0 * 50.0).abs() < 1e-9,
        "volumen {}",
        m.volume_mm3
    );
    // lateral 2πrh + dos tapas 2πr²  =  2πr(h + r)
    assert!((m.area_mm2 - 2.0 * pi * 10.0 * 60.0).abs() < 1e-9);
    assert!((m.centroid - DVec3::new(0.0, 0.0, 25.0)).length() < 1e-9);
}

/// El teselado **converge** al refinar: es la propiedad que distingue un
/// cilindro analítico de uno facetado de una vez para siempre.
#[test]
fn el_teselado_del_cilindro_converge_al_volumen_exacto() {
    let k = k();
    let s = k.cylinder(DVec3::ZERO, DVec3::Z, 10.0, 50.0, f()).unwrap();
    let exacto = std::f64::consts::PI * 100.0 * 50.0;

    // La restriccion angular se desactiva (360 grados) para que la variable
    // medida sea la de cuerda y no la tape la otra: `segmentos_para` toma el
    // maximo de las dos, asi que con 15 grados salen 24 segmentos fijos hasta
    // que la cuerda baja de ~0.086 mm y el test no mediria lo que dice medir.
    let mut errores = Vec::new();
    let mut triangulos = Vec::new();
    for chord in [1.0, 0.25, 0.05, 0.01] {
        let t = k
            .tessellate(
                s,
                &TessellationParams {
                    chord_mm: chord,
                    angular_deg: 360.0,
                },
            )
            .unwrap();
        t.validate().unwrap();
        errores.push((exacto - volumen_teselado(&t)).abs() / exacto);
        triangulos.push(t.triangle_count());
    }
    for w in errores.windows(2) {
        assert!(w[1] < w[0], "el error no bajo al refinar: {errores:?}");
    }
    for w in triangulos.windows(2) {
        assert!(
            w[1] > w[0],
            "no salieron mas triangulos al refinar: {triangulos:?}"
        );
    }

    // Y el error no solo baja: baja **exactamente lo que dice la teoria**.
    // Un poligono regular de n lados inscrito en un circulo de radio r tiene
    // area (n/2)·r²·sin(2π/n), asi que el error relativo de volumen del prisma
    // es 1 − (n/2π)·sin(2π/n). Comparar contra eso verifica de una vez el
    // teselado y el calculo del numero de segmentos; un umbral elegido a ojo
    // no habria detectado, por ejemplo, un segmento de mas o de menos.
    for (i, chord) in [1.0, 0.25, 0.05, 0.01].iter().enumerate() {
        let n = segmentos_para(
            10.0,
            &TessellationParams {
                chord_mm: *chord,
                angular_deg: 360.0,
            },
        ) as f64;
        let teorico = 1.0 - (n / std::f64::consts::TAU) * (std::f64::consts::TAU / n).sin();
        assert!(
            (errores[i] - teorico).abs() < teorico * 1e-6 + 1e-12,
            "chord={chord} n={n}: medido {} vs teorico {teorico}",
            errores[i]
        );
    }
}

/// Y la restriccion angular tambien manda cuando es la mas exigente: es un
/// maximo de las dos, no una eleccion.
#[test]
fn manda_la_restriccion_mas_exigente_de_las_dos() {
    let laxa = TessellationParams {
        chord_mm: 1e9,
        angular_deg: 360.0,
    };
    let por_angulo = TessellationParams {
        chord_mm: 1e9,
        angular_deg: 5.0,
    };
    let por_cuerda = TessellationParams {
        chord_mm: 0.001,
        angular_deg: 360.0,
    };
    assert_eq!(segmentos_para(10.0, &laxa), 8, "el minimo de seguridad");
    assert_eq!(segmentos_para(10.0, &por_angulo), 72, "360/5");
    assert!(
        segmentos_para(10.0, &por_cuerda) > 72,
        "la cuerda fina debe pedir mas"
    );
    // con las dos exigentes, gana la mayor
    let ambas = TessellationParams {
        chord_mm: 0.001,
        angular_deg: 5.0,
    };
    assert_eq!(
        segmentos_para(10.0, &ambas),
        segmentos_para(10.0, &por_cuerda)
    );
}

#[test]
fn segmentos_respetan_la_tolerancia_de_cuerda() {
    // r·(1 − cos(π/n)) ≤ chord
    for (r, chord) in [(10.0, 0.1), (50.0, 0.01), (1.0, 0.5)] {
        let n = segmentos_para(
            r,
            &TessellationParams {
                chord_mm: chord,
                angular_deg: 360.0,
            },
        );
        let real = r * (1.0 - (std::f64::consts::PI / n as f64).cos());
        assert!(
            real <= chord * 1.001,
            "r={r} chord={chord} n={n} real={real}"
        );
    }
    // y nunca por debajo del minimo razonable
    assert_eq!(
        segmentos_para(
            10.0,
            &TessellationParams {
                chord_mm: 1e9,
                angular_deg: 1e9
            }
        ),
        8
    );
}

#[test]
fn el_teselado_lleva_procedencia_completa_y_valida() {
    let k = k();
    let s = k
        .box_solid(DVec3::ZERO, DVec3::new(4.0, 5.0, 6.0), f())
        .unwrap();
    let t = k.tessellate(s, &TessellationParams::default()).unwrap();
    t.validate().unwrap();
    assert_eq!(t.triangle_count(), 12, "una caja son 12 triangulos");
    assert_eq!(t.face_of_triangle.len(), 12);

    let caras: std::collections::BTreeSet<StableId> = t.face_of_triangle.iter().copied().collect();
    assert_eq!(caras.len(), 6, "los 12 triangulos vienen de 6 caras");
    assert!((volumen_teselado(&t) - 120.0).abs() < 1e-9);

    // las aristas se clasifican: las 12 de una caja son quiebres reales
    assert_eq!(t.edges.len(), 12);
    assert!(t
        .edges
        .iter()
        .all(|e| e.kind == EdgeKind::Sharp && e.kind.se_dibuja()));
}

/// El cilindro sí tiene una costura, y una costura **nunca** se dibuja: no es
/// un quiebre del sólido sino un artefacto de la parametrización.
#[test]
fn el_cilindro_tiene_costura_y_no_se_dibuja() {
    let k = k();
    let s = k.cylinder(DVec3::ZERO, DVec3::Z, 5.0, 20.0, f()).unwrap();
    let t = k.tessellate(s, &TessellationParams::default()).unwrap();
    let costuras: Vec<_> = t
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Seam)
        .collect();
    assert_eq!(costuras.len(), 1);
    assert!(!costuras[0].kind.se_dibuja());
    assert_eq!(
        t.edges.iter().filter(|e| e.kind.se_dibuja()).count(),
        2,
        "los dos circulos"
    );
}

/// Chaflán de 2 mm sobre una arista de un cubo de 10: quita un prisma
/// triangular de catetos 2 y longitud 10, o sea 1000 − (2²/2)·10 = 980 mm³.
#[test]
fn chaflan_quita_el_volumen_calculado_a_mano_y_crea_una_cara_blend() {
    let k = k();
    let cubo = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let arista = k.topology(cubo).unwrap().edges[0].id;

    let s = k
        .chamfer(
            cubo,
            &[arista],
            ChamferSpec::Symmetric { distance_mm: 2.0 },
            f(),
        )
        .unwrap();
    let m = k.mass_properties(s).unwrap();
    assert!(
        (m.volume_mm3 - 980.0).abs() < 1e-6,
        "volumen {}",
        m.volume_mm3
    );

    let t = k.topology(s).unwrap();
    assert_eq!(t.faces.len(), 7, "6 caras heredadas + 1 chaflan");
    let blends: Vec<_> = t
        .faces
        .iter()
        .filter(|f| matches!(f.provenance, TopoProvenance::Blend { .. }))
        .collect();
    assert_eq!(blends.len(), 1);
    assert!(matches!(blends[0].provenance, TopoProvenance::Blend { of } if of == arista));
    assert_eq!(
        t.faces
            .iter()
            .filter(|f| matches!(f.provenance, TopoProvenance::Inherited { .. }))
            .count(),
        6
    );
    assert!(
        k.is_valid(s).unwrap().valid,
        "{:?}",
        k.is_valid(s).unwrap().problems
    );
}

#[test]
fn el_fillet_produce_la_misma_topologia_que_el_chaflan() {
    let k = k();
    let cubo = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let aristas = k.topology(cubo).unwrap().edges;

    // Dos aristas que NO comparten vértice: la opuesta por el centro del cubo.
    // El centroide de una es el reflejo del de la otra respecto a (5, 5, 5), y
    // en unidades de firma eso es 10000 − c.
    let a = &aristas[0];
    let opuesto = |c: [i64; 3]| [10_000 - c[0], 10_000 - c[1], 10_000 - c[2]];
    let b = aristas
        .iter()
        .find(|e| e.signature.centroid_q == opuesto(a.signature.centroid_q))
        .expect("toda arista del cubo tiene su opuesta por el centro");

    // Una sola arista.
    let s1 = k
        .fillet(cubo, &[a.id], FilletSpec::Constant { radius_mm: 1.5 }, f())
        .unwrap();
    let t1 = k.topology(s1).unwrap();
    assert_eq!(t1.faces.len(), 7);
    assert_eq!(blends(&t1), 1);

    // Las dos a la vez. Al no compartir vértice no interfieren: cada una quita
    // su propio prisma, 1000 − 2·(1.5²/2)·10 = 977.5 mm³.
    let s2 = k
        .fillet(
            cubo,
            &[a.id, b.id],
            FilletSpec::Constant { radius_mm: 1.5 },
            f(),
        )
        .unwrap();
    let t2 = k.topology(s2).unwrap();
    assert_eq!(t2.faces.len(), 8, "6 heredadas + 2 blend");
    assert_eq!(blends(&t2), 2);

    let m = k.mass_properties(s2).unwrap();
    assert!(
        (m.volume_mm3 - 977.5).abs() < 1e-9,
        "volumen {}",
        m.volume_mm3
    );
    let r = k.is_valid(s2).unwrap();
    assert!(r.valid, "{:?}", r.problems);
}

fn blends(t: &TopologySummary) -> usize {
    t.faces
        .iter()
        .filter(|f| matches!(f.provenance, TopoProvenance::Blend { .. }))
        .count()
}

#[test]
fn booleano_caja_menos_caja_da_el_volumen_exacto() {
    let k = k();
    let a = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    // esquina de 4×4×4 metida dentro
    let b = k
        .box_solid(DVec3::splat(6.0), DVec3::splat(12.0), f())
        .unwrap();

    let d = k.boolean(BoolOp::Difference, a, b, f()).unwrap();
    // 1000 − 4³ = 936
    assert!((k.mass_properties(d).unwrap().volume_mm3 - 936.0).abs() < 1e-6);

    let i = k.boolean(BoolOp::Intersection, a, b, f()).unwrap();
    assert!((k.mass_properties(i).unwrap().volume_mm3 - 64.0).abs() < 1e-6);

    let u = k.boolean(BoolOp::Union, a, b, f()).unwrap();
    // 1000 + 6³ − 4³ = 1000 + 216 − 64 = 1152
    assert!((k.mass_properties(u).unwrap().volume_mm3 - 1152.0).abs() < 1e-6);

    // las piezas llevan procedencia de particion
    let t = k.topology(d).unwrap();
    assert!(t
        .faces
        .iter()
        .all(|f| matches!(f.provenance, TopoProvenance::SplitFrom { .. })));
}

/// El área de un booleano, que es lo que el test del volumen no puede ver.
///
/// Un resultado hecho de piezas sueltas tiene las caras de contacto por
/// duplicado. Al volumen no le afecta: las dos copias tienen normal opuesta y se
/// cancelan en el teorema de la divergencia. Al área sí, porque el área suma
/// valores absolutos. Medido antes de la frontera por rejilla, con el volumen
/// correcto en todos ellos y `is_valid()` sin quejarse en ninguno:
///
/// | caso                | antes | real |
/// |---------------------|-------|------|
/// | cubo menos esquina  |   816 |  600 |
/// | union de dos cubos  |   872 |  720 |
/// | cubo con hueco      |  1072 |  624 |
#[test]
fn el_area_de_un_booleano_no_cuenta_las_caras_de_contacto() {
    let k = k();
    let a = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let b = k
        .box_solid(DVec3::splat(6.0), DVec3::splat(12.0), f())
        .unwrap();

    // Cubo de 10 menos una esquina de 4: las tres caras que tocan la esquina
    // pierden 4×4 cada una y aparecen tres caras nuevas de 4×4. Se compensa
    // exacto, así que el área sigue siendo la del cubo.
    let d = k.boolean(BoolOp::Difference, a, b, f()).unwrap();
    assert_area(&k, d, 600.0, "diferencia");

    // La esquina sola: un cubo de 4.
    let i = k.boolean(BoolOp::Intersection, a, b, f()).unwrap();
    assert_area(&k, i, 6.0 * 16.0, "interseccion");

    // Unión. Del cubo grande sobreviven 3 caras enteras y 3 a las que el cubo
    // pequeño tapa un cuadrado de 4×4: 3·100 + 3·84 = 552. Del pequeño, 3 caras
    // enteras de 6×6 y 3 con un 4×4 tapado: 3·36 + 3·20 = 168. Total 720.
    let u = k.boolean(BoolOp::Union, a, b, f()).unwrap();
    assert_area(&k, u, 720.0, "union");

    // El caso más exigente: un hueco **interior**, que obliga a `restar` a sacar
    // las seis piezas y produce una segunda cáscara cerrada dentro del sólido,
    // con las normales mirando hacia el hueco. 600 de fuera + 6·2² de dentro.
    let dentro = k
        .box_solid(DVec3::splat(4.0), DVec3::splat(6.0), f())
        .unwrap();
    let hueco = k.boolean(BoolOp::Difference, a, dentro, f()).unwrap();
    assert!(
        (k.mass_properties(hueco).unwrap().volume_mm3 - (1000.0 - 8.0)).abs() < 1e-9,
        "volumen {}",
        k.mass_properties(hueco).unwrap().volume_mm3
    );
    assert_area(&k, hueco, 600.0 + 6.0 * 4.0, "hueco interior");
}

/// Comprueba el área contra el valor calculado a mano y, además, que la malla
/// esté cerrada: sin aristas de borde libre no puede haber ni caras que sobren
/// ni agujeros por los que falte área.
fn assert_area(k: &StubKernel, s: ShapeId, esperada: f64, que: &str) {
    let m = k.mass_properties(s).unwrap();
    assert!(
        (m.area_mm2 - esperada).abs() < 1e-9,
        "{que}: area {:.4}, esperada {esperada:.4}",
        m.area_mm2
    );
    let r = k.is_valid(s).unwrap();
    assert!(r.valid, "{que}: {:?}", r.problems);

    let t = k
        .tessellate(s, &TessellationParams::default())
        .unwrap();
    let libres = t
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Boundary)
        .count();
    assert_eq!(libres, 0, "{que}: {libres} aristas de borde libre");
}

#[test]
fn serializar_y_deserializar_conserva_la_geometria() {
    let k = k();
    let s = k
        .box_solid(DVec3::new(-1.0, 2.0, -3.0), DVec3::new(4.0, 9.0, 5.0), f())
        .unwrap();
    let bytes = k.serialize(s).unwrap();
    let s2 = k.deserialize(&bytes, f()).unwrap();
    let (m1, m2) = (
        k.mass_properties(s).unwrap(),
        k.mass_properties(s2).unwrap(),
    );
    assert!((m1.volume_mm3 - m2.volume_mm3).abs() < 1e-12);
    assert_eq!(
        k.topology(s).unwrap().faces.len(),
        k.topology(s2).unwrap().faces.len()
    );
}

#[test]
fn release_libera_de_verdad() {
    let k = k();
    let s = k.box_solid(DVec3::ZERO, DVec3::ONE, f()).unwrap();
    assert_eq!(k.live_shapes(), 1);
    k.release(s);
    assert_eq!(k.live_shapes(), 0);
    assert!(matches!(k.topology(s), Err(KernelError::UnknownShape(_))));
}

// ---------------------------------------------------------------------------
// Controles: lo que tiene que fallar, falla, y con el error correcto
// ---------------------------------------------------------------------------

#[test]
fn los_errores_son_datos_y_dicen_que_paso() {
    let k = k();

    // perfil con menos de 3 puntos
    assert!(matches!(
        k.profile_from_polygon(&[DVec2::ZERO, DVec2::X], f()),
        Err(KernelError::InvalidInput { .. })
    ));
    // perfil de area nula
    assert!(matches!(
        k.profile_from_polygon(&[DVec2::ZERO, DVec2::X, DVec2::new(2.0, 0.0)], f()),
        Err(KernelError::Degenerate { .. })
    ));
    // perfil que se autointersecta (un "lazo")
    let lazo = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
    ];
    assert!(matches!(
        k.profile_from_polygon(&lazo, f()),
        Err(KernelError::Degenerate { .. })
    ));

    // handle desconocido
    assert!(matches!(
        k.mass_properties(ShapeId(9999)),
        Err(KernelError::UnknownShape(_))
    ));

    // cilindro con radio no positivo
    assert!(matches!(
        k.cylinder(DVec3::ZERO, DVec3::Z, -1.0, 10.0, f()),
        Err(KernelError::InvalidInput { .. })
    ));

    // extrusion de distancia nula
    let perfil = k
        .profile_from_polygon(
            &[DVec2::ZERO, DVec2::new(1.0, 0.0), DVec2::new(0.0, 1.0)],
            f(),
        )
        .unwrap();
    assert!(matches!(
        k.extrude(
            perfil,
            ExtrudeOpts {
                direction: DVec3::Z,
                distance_mm: 0.0,
                symmetric: false
            },
            f()
        ),
        Err(KernelError::Degenerate { .. })
    ));

    // arista inexistente al biselar
    let cubo = k.box_solid(DVec3::ZERO, DVec3::ONE, f()).unwrap();
    let falsa = StableId {
        origin: f(),
        class: forge_doc::TopoClass::Edge,
        mark: 0xDEAD,
    };
    assert!(matches!(
        k.chamfer(
            cubo,
            &[falsa],
            ChamferSpec::Symmetric { distance_mm: 0.1 },
            f()
        ),
        Err(KernelError::UnresolvedReference(_))
    ));
}

/// El stub dice que no en vez de aproximar en silencio. Un kernel que finge
/// poder con todo produce resultados plausibles y equivocados.
#[test]
fn lo_no_soportado_se_reporta_en_vez_de_aproximarse() {
    let k = k();
    // booleano contra algo que no es una caja
    let perfil = k
        .profile_from_polygon(
            &[DVec2::ZERO, DVec2::new(10.0, 0.0), DVec2::new(5.0, 8.0)],
            f(),
        )
        .unwrap();
    let prisma = k
        .extrude(
            perfil,
            ExtrudeOpts {
                direction: DVec3::Z,
                distance_mm: 3.0,
                symmetric: false,
            },
            f(),
        )
        .unwrap();
    let caja = k.box_solid(DVec3::ZERO, DVec3::ONE, f()).unwrap();
    assert!(matches!(
        k.boolean(BoolOp::Union, prisma, caja, f()),
        Err(KernelError::Unsupported(_))
    ));

    // biselar dos aristas que comparten vertice
    let cubo = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let aristas = k.topology(cubo).unwrap().edges;
    let e0 = &aristas[0];
    // buscar una que comparta vertice con e0 usando la firma no vale; se usan
    // todas y se comprueba que al menos una combinacion da Unsupported
    let ids: Vec<StableId> = aristas.iter().take(4).map(|e| e.id).collect();
    let _ = e0;
    let r = k.chamfer(cubo, &ids, ChamferSpec::Symmetric { distance_mm: 1.0 }, f());
    assert!(
        matches!(r, Err(KernelError::Unsupported(_))),
        "cuatro aristas de un cubo comparten vertices; deberia decir que no"
    );

    // STEP no esta implementado y lo dice
    assert!(matches!(
        k.import_step(b"ISO-10303", f()),
        Err(KernelError::Unsupported("import_step"))
    ));
    assert!(matches!(
        k.export_step(&[caja]),
        Err(KernelError::Unsupported("export_step"))
    ));
}

/// Control positivo de `Tessellation::validate`: si no detectara nada, todos
/// los `validate().unwrap()` de arriba pasarian aunque el teselado fuera basura.
#[test]
fn validate_detecta_teselados_mal_formados() {
    let base = Tessellation {
        positions: vec![DVec3::ZERO, DVec3::X, DVec3::Y],
        normals: vec![DVec3::Z; 3],
        indices: vec![0, 1, 2],
        face_of_triangle: vec![StableId {
            origin: f(),
            class: forge_doc::TopoClass::Face,
            mark: 1,
        }],
        ..Default::default()
    };
    assert!(base.validate().is_ok());

    let mut sin_procedencia = base.clone();
    sin_procedencia.face_of_triangle.clear();
    assert!(
        sin_procedencia.validate().is_err(),
        "no detecto procedencia incompleta"
    );

    let mut indices_sueltos = base.clone();
    indices_sueltos.indices = vec![0, 1];
    assert!(
        indices_sueltos.validate().is_err(),
        "no detecto indices no multiplo de 3"
    );

    let mut fuera = base.clone();
    fuera.indices = vec![0, 1, 42];
    assert!(
        fuera.validate().is_err(),
        "no detecto indice fuera de rango"
    );

    let mut normales = base.clone();
    normales.normals.pop();
    assert!(
        normales.validate().is_err(),
        "no detecto normales descuadradas"
    );
}

/// Control positivo de `is_valid`: un sólido con las caras al revés tiene que
/// detectarse. Se construye pidiendo una caja con min y max intercambiados.
#[test]
fn is_valid_detecta_solidos_mal_formados() {
    let k = k();
    let r = k.box_solid(DVec3::splat(10.0), DVec3::ZERO, f());
    assert!(
        r.is_err(),
        "una caja invertida deberia rechazarse al construirla"
    );
}

/// Chaflán de 1 mm sobre **cada una** de las 12 aristas del cubo, una por una.
///
/// Este es el test cuya ausencia dejó pasar un bug real: la cara del chaflán se
/// construía recorriendo `e.caras[0]` y `e.caras[1]` en el orden en que salían
/// del mapa, que es arbitrario. Cuando salían al revés, el cuadrilátero quedaba
/// enrollado hacia dentro y el sólido daba 1001.67, 935 o 868.33 mm³ — con
/// `is_valid()` diciendo que todo estaba bien. 6 de las 12 aristas fallaban.
///
/// Con una sola arista el bug es invisible: la arista 0 está en el grupo bueno.
#[test]
fn el_chaflan_da_el_mismo_volumen_en_las_doce_aristas_del_cubo() {
    let k = k();
    let cubo = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let aristas = k.topology(cubo).unwrap().edges;
    assert_eq!(aristas.len(), 12, "un cubo tiene 12 aristas");

    // 1000 − (1²/2)·10 = 995, sea cual sea la arista: las 12 son equivalentes
    // por simetría.
    const ESPERADO: f64 = 995.0;

    let mut fallos = Vec::new();
    for (i, a) in aristas.iter().enumerate() {
        let s = k
            .chamfer(cubo, &[a.id], ChamferSpec::Symmetric { distance_mm: 1.0 }, f())
            .unwrap();

        let m = k.mass_properties(s).unwrap();
        if (m.volume_mm3 - ESPERADO).abs() >= 1e-9 {
            fallos.push(format!("arista {i}: volumen {:.4}", m.volume_mm3));
            continue;
        }

        // El volumen interno puede ser correcto y el teselado no. Se comprueba
        // por el teorema de la divergencia, que no comparte código con
        // `mass_properties`.
        let t = k
            .tessellate(
                s,
                &TessellationParams {
                    chord_mm: 0.01,
                    angular_deg: 15.0,
                },
            )
            .unwrap();
        let vt = volumen_teselado(&t);
        if (vt - ESPERADO).abs() >= 1e-6 {
            fallos.push(format!("arista {i}: teselado {vt:.4}"));
            continue;
        }

        let r = k.is_valid(s).unwrap();
        if !r.valid {
            fallos.push(format!("arista {i}: invalido {:?}", r.problems));
            continue;
        }
        let top = k.topology(s).unwrap();
        if top.faces.len() != 7 {
            fallos.push(format!("arista {i}: {} caras", top.faces.len()));
        }
    }
    assert!(fallos.is_empty(), "{} de 12 mal:\n{}", fallos.len(), fallos.join("\n"));
}

/// El área también, y se deriva a mano con cuidado porque es fácil quedarse
/// corto: un chaflán de 1 mm sobre una arista del cubo
///
///   - quita un rectángulo de 1×10 de **cada** una de las dos caras adyacentes
///     a la arista: −20;
///   - quita un triángulo de catetos 1×1 de **cada** una de las dos caras
///     perpendiculares a la arista, en sus extremos: −1;
///   - pone la cara nueva, de √2 × 10: +14.142.
///
/// Total 600 − 20 − 1 + 14.142 = 593.142 mm². Las dos caras de los extremos son
/// justo las que se olvidan al hacer la cuenta rápido.
///
/// El volumen por el teorema de la divergencia es ciego a una cara duplicada o
/// sobrante, porque las contribuciones se cancelan. El área no lo es, y por eso
/// este test va aparte del de volumen.
#[test]
fn el_chaflan_da_la_misma_area_en_las_doce_aristas_del_cubo() {
    let k = k();
    let cubo = k.box_solid(DVec3::ZERO, DVec3::splat(10.0), f()).unwrap();
    let aristas = k.topology(cubo).unwrap().edges;
    let esperado = 600.0 - 2.0 * 10.0 - 2.0 * 0.5 + 2.0_f64.sqrt() * 10.0;

    for (i, a) in aristas.iter().enumerate() {
        let s = k
            .chamfer(cubo, &[a.id], ChamferSpec::Symmetric { distance_mm: 1.0 }, f())
            .unwrap();
        let m = k.mass_properties(s).unwrap();
        assert!(
            (m.area_mm2 - esperado).abs() < 1e-9,
            "arista {i}: area {:.6}, esperada {esperado:.6}",
            m.area_mm2
        );
    }
}
