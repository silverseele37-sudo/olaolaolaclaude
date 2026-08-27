//! Ida y vuelta y conformidad con las especificaciones.
//!
//! Los dos formatos se prueban con respuestas conocidas calculadas a mano, y en
//! particular con una caja **asimétrica**: una caja simétrica no distingue una
//! conversión de ejes correcta de una equivocada, que es exactamente el bug que
//! este crate existe para evitar.

use forge_interop::{gltf, obj, usd, InteropError, TriangleSoup};
use forge_math::{DVec2, DVec3};

/// Caja con esquinas en `min` y `max`, 12 triángulos, normales por vértice.
/// Asimétrica a propósito.
fn caja(min: DVec3, max: DVec3) -> TriangleSoup {
    let c = [
        DVec3::new(min.x, min.y, min.z),
        DVec3::new(max.x, min.y, min.z),
        DVec3::new(max.x, max.y, min.z),
        DVec3::new(min.x, max.y, min.z),
        DVec3::new(min.x, min.y, max.z),
        DVec3::new(max.x, min.y, max.z),
        DVec3::new(max.x, max.y, max.z),
        DVec3::new(min.x, max.y, max.z),
    ];
    let caras: [[usize; 4]; 6] = [
        [0, 3, 2, 1], // abajo  (-Z)
        [4, 5, 6, 7], // arriba (+Z)
        [0, 1, 5, 4], // -Y
        [2, 3, 7, 6], // +Y
        [1, 2, 6, 5], // +X
        [0, 4, 7, 3], // -X
    ];
    let normales = [
        -DVec3::Z,
        DVec3::Z,
        -DVec3::Y,
        DVec3::Y,
        DVec3::X,
        -DVec3::X,
    ];

    let mut s = TriangleSoup {
        name: "caja".into(),
        ..Default::default()
    };
    for (f, cara) in caras.iter().enumerate() {
        let base = s.positions.len() as u32;
        for &i in cara {
            s.positions.push(c[i]);
            s.normals.push(normales[f]);
            s.uvs.push(DVec2::new(0.25, 0.75));
        }
        s.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    s
}

// ---------------------------------------------------------------------------
// OBJ
// ---------------------------------------------------------------------------

#[test]
fn obj_ida_y_vuelta_conserva_geometria() {
    let original = caja(DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 20.0, 30.0));
    let txt = obj::to_string(&original, obj::ObjOptions::completo()).unwrap();
    let vuelta = obj::from_str(&txt, obj::ObjOptions::completo()).unwrap();

    assert_eq!(vuelta.triangle_count(), 12);
    assert_eq!(vuelta.positions.len(), original.positions.len());
    assert_eq!(vuelta.indices, original.indices);
    for (a, b) in original.positions.iter().zip(&vuelta.positions) {
        assert!((*a - *b).length() < 1e-7, "{a:?} != {b:?}");
    }
    for (a, b) in original.normals.iter().zip(&vuelta.normals) {
        assert!((*a - *b).length() < 1e-7);
    }
    // el bounding box tiene que ser exactamente el mismo
    assert_eq!(vuelta.bbox().min, original.bbox().min);
    assert_eq!(vuelta.bbox().max, original.bbox().max);
}

#[test]
fn obj_ida_y_vuelta_por_y_arriba_vuelve_al_origen() {
    let original = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let opts = obj::ObjOptions {
        y_up: true,
        ..obj::ObjOptions::completo()
    };
    let txt = obj::to_string(&original, opts).unwrap();
    // en el archivo, el alto (30) tiene que estar en la coordenada Y
    assert!(
        txt.contains("30.000000000"),
        "el alto no aparece en el archivo"
    );
    let vuelta = obj::from_str(&txt, opts).unwrap();
    assert_eq!(
        vuelta.bbox().max,
        original.bbox().max,
        "la ida y vuelta por Y-up no cerro"
    );
}

#[test]
fn obj_acepta_indices_negativos_y_caras_de_cuatro() {
    // indices negativos = relativos al final; cara de 4 vertices = 2 triangulos
    let txt = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f -4 -3 -2 -1
";
    let s = obj::from_str(txt, obj::ObjOptions::default()).unwrap();
    assert_eq!(s.positions.len(), 4);
    assert_eq!(s.triangle_count(), 2, "la cara de 4 no se triangulo");
}

/// Control positivo: cada forma de estar mal formado se detecta con su línea.
#[test]
fn obj_detecta_archivos_mal_formados() {
    let casos = [
        ("v 0 0\nf 1 1 1\n", "vertice incompleto"),
        ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2\n", "cara con 2 vertices"),
        ("v 0 0 0\nf 1 2 99\n", "indice fuera de rango"),
        ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 x\n", "indice no numerico"),
    ];
    for (txt, que) in casos {
        let r = obj::from_str(txt, obj::ObjOptions::default());
        assert!(r.is_err(), "no detecto: {que}");
        if let Err(InteropError::Malformed { line, .. }) = r {
            assert!(line > 0, "el error no dice en que linea ({que})");
        }
    }
}

// ---------------------------------------------------------------------------
// glTF / GLB
// ---------------------------------------------------------------------------

#[test]
fn glb_cumple_la_estructura_del_contenedor() {
    let s = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&s, gltf::GltfOptions::default()).unwrap();

    let u32_en = |o: usize| u32::from_le_bytes([glb[o], glb[o + 1], glb[o + 2], glb[o + 3]]);
    assert_eq!(u32_en(0), 0x4654_6C67, "magic");
    assert_eq!(u32_en(4), 2, "version");
    assert_eq!(
        u32_en(8) as usize,
        glb.len(),
        "la longitud declarada no es la real"
    );

    let json_len = u32_en(12) as usize;
    assert_eq!(u32_en(16), 0x4E4F_534A, "el primer chunk debe ser JSON");
    assert_eq!(json_len % 4, 0, "el chunk JSON no esta alineado a 4");

    let bin_off = 20 + json_len;
    let bin_len = u32_en(bin_off) as usize;
    assert_eq!(
        u32_en(bin_off + 4),
        0x004E_4942,
        "el segundo chunk debe ser BIN"
    );
    assert_eq!(bin_len % 4, 0, "el chunk BIN no esta alineado a 4");
    assert_eq!(
        bin_off + 8 + bin_len,
        glb.len(),
        "sobran o faltan bytes al final"
    );

    // el relleno del JSON son espacios, no ceros: lo exige la especificacion
    assert_eq!(glb[20 + json_len - 1] as char as u8, glb[20 + json_len - 1]);
    assert!(
        glb[20..20 + json_len].iter().all(|&b| b != 0),
        "el JSON se relleno con ceros"
    );
}

#[test]
fn glb_declara_lo_que_exige_la_especificacion() {
    let s = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&s, gltf::GltfOptions::default()).unwrap();
    let j = gltf::glb_json(&glb).unwrap();

    assert_eq!(j["asset"]["version"], "2.0");
    assert!(j["scenes"].is_array() && j["nodes"].is_array());
    assert!(j["meshes"][0]["primitives"][0]["attributes"]["POSITION"].is_number());
    assert_eq!(
        j["meshes"][0]["primitives"][0]["mode"], 4,
        "modo debe ser TRIANGLES"
    );
    assert!(j["materials"][0]["pbrMetallicRoughness"].is_object());

    // POSITION exige min y max: los visores los usan para encuadrar
    let a = &j["accessors"][0];
    assert_eq!(a["type"], "VEC3");
    assert_eq!(a["componentType"], 5126);
    assert!(
        a["min"].is_array() && a["max"].is_array(),
        "POSITION sin min/max"
    );

    let idx = j["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize;
    assert_eq!(j["accessors"][idx]["count"], s.indices.len());
    assert_eq!(j["accessors"][idx]["type"], "SCALAR");
}

/// Respuesta conocida de las dos conversiones a la vez, con una caja asimetrica
/// para que un error de ejes no pueda esconderse.
///
/// FORGE:  x∈[0,10]  y∈[0,20]  z∈[0,30]  milimetros
/// glTF:   x∈[0,10]  y∈[0,30]  z∈[-20,0]  metros → dividido por 1000
#[test]
fn glb_convierte_ejes_y_unidades_con_valores_exactos() {
    let s = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&s, gltf::GltfOptions::default()).unwrap();
    let j = gltf::glb_json(&glb).unwrap();

    let leer = |v: &serde_json::Value| -> [f64; 3] {
        let a = v.as_array().unwrap();
        [
            a[0].as_f64().unwrap(),
            a[1].as_f64().unwrap(),
            a[2].as_f64().unwrap(),
        ]
    };
    let min = leer(&j["accessors"][0]["min"]);
    let max = leer(&j["accessors"][0]["max"]);

    let cerca = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(
        cerca(min[0], 0.000) && cerca(max[0], 0.010),
        "X: {min:?} {max:?}"
    );
    assert!(
        cerca(min[1], 0.000) && cerca(max[1], 0.030),
        "el alto (30 mm) debe ir a +Y = 0.030 m: {min:?} {max:?}"
    );
    assert!(
        cerca(min[2], -0.020) && cerca(max[2], 0.000),
        "la profundidad (20 mm) debe ir a -Z: {min:?} {max:?}"
    );
}

/// Control: sin conversiones, los numeros salen tal cual. Si este test y el
/// anterior dieran lo mismo, la conversion no estaria ocurriendo.
#[test]
fn glb_crudo_no_convierte_nada() {
    let s = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&s, gltf::GltfOptions::crudo()).unwrap();
    let j = gltf::glb_json(&glb).unwrap();
    let max = j["accessors"][0]["max"].as_array().unwrap();
    assert_eq!(max[0].as_f64().unwrap(), 10.0);
    assert_eq!(max[1].as_f64().unwrap(), 20.0);
    assert_eq!(max[2].as_f64().unwrap(), 30.0);
}

/// El min/max declarado tiene que ser el real, no uno aproximado: se recalcula
/// aqui de forma independiente a partir del propio buffer binario.
#[test]
fn glb_el_min_max_declarado_coincide_con_el_buffer() {
    let s = caja(DVec3::new(-3.0, 7.0, -11.0), DVec3::new(5.0, 19.0, 2.0));
    let opts = gltf::GltfOptions::crudo();
    let glb = gltf::to_glb(&s, opts).unwrap();
    let j = gltf::glb_json(&glb).unwrap();

    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let bin = &glb[20 + json_len + 8..];
    let view = &j["bufferViews"][0];
    let off = view["byteOffset"].as_u64().unwrap() as usize;
    let len = view["byteLength"].as_u64().unwrap() as usize;

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in bin[off..off + len]
        .chunks_exact(4)
        .collect::<Vec<_>>()
        .chunks_exact(3)
    {
        for (k, comp) in v.iter().enumerate() {
            let f = f32::from_le_bytes([comp[0], comp[1], comp[2], comp[3]]);
            min[k] = min[k].min(f);
            max[k] = max[k].max(f);
        }
    }
    let decl_min = j["accessors"][0]["min"].as_array().unwrap();
    let decl_max = j["accessors"][0]["max"].as_array().unwrap();
    for k in 0..3 {
        assert_eq!(decl_min[k].as_f64().unwrap() as f32, min[k], "min[{k}]");
        assert_eq!(decl_max[k].as_f64().unwrap() as f32, max[k], "max[{k}]");
    }
}

#[test]
fn glb_rechaza_mallas_invalidas_antes_de_escribir() {
    let mut mala = caja(DVec3::ZERO, DVec3::ONE);
    mala.indices.push(9999);
    assert!(gltf::to_glb(&mala, gltf::GltfOptions::default()).is_err());
    assert!(obj::to_string(&mala, obj::ObjOptions::completo()).is_err());
}

#[test]
fn glb_json_detecta_contenedores_corruptos() {
    let s = caja(DVec3::ZERO, DVec3::ONE);
    let mut glb = gltf::to_glb(&s, gltf::GltfOptions::default()).unwrap();
    assert!(gltf::glb_json(&glb).is_ok());

    glb[0] = 0; // magic roto
    assert!(
        gltf::glb_json(&glb).is_err(),
        "no detecto el magic corrupto"
    );
    assert!(
        gltf::glb_json(&[1, 2, 3]).is_err(),
        "no detecto un archivo truncado"
    );
}

#[test]
fn glb_ida_y_vuelta_conserva_geometria() {
    let original = caja(DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&original, gltf::GltfOptions::default()).unwrap();
    let vuelta = gltf::read_glb(&glb, gltf::GltfOptions::default()).unwrap();

    assert_eq!(vuelta.triangle_count(), 12);
    assert_eq!(vuelta.positions.len(), original.positions.len());
    assert_eq!(vuelta.indices, original.indices);

    // las posiciones deben coincidir dentro de tolerancia (perdida por float32)
    for (a, b) in original.positions.iter().zip(&vuelta.positions) {
        assert!((*a - *b).length() < 1e-5, "{a:?} != {b:?}");
    }

    // el bounding box debe coincidir dentro de tolerancia (perdida por float32 y conversiones)
    let orig_bbox = original.bbox();
    let vuelta_bbox = vuelta.bbox();
    assert!(
        (orig_bbox.min - vuelta_bbox.min).length() < 1e-5,
        "min: {:?} != {:?}",
        orig_bbox.min,
        vuelta_bbox.min
    );
    assert!(
        (orig_bbox.max - vuelta_bbox.max).length() < 1e-5,
        "max: {:?} != {:?}",
        orig_bbox.max,
        vuelta_bbox.max
    );
}

#[test]
fn glb_lee_normal_correctamente() {
    // Verifica que se leen normales y UVs
    let original = caja(DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 20.0, 30.0));
    let glb = gltf::to_glb(&original, gltf::GltfOptions::default()).unwrap();
    let vuelta = gltf::read_glb(&glb, gltf::GltfOptions::default()).unwrap();

    // Debe tener normales
    assert!(!vuelta.normals.is_empty(), "no se leyeron las normales");
    assert_eq!(vuelta.normals.len(), vuelta.positions.len());

    // Debe tener UVs
    assert!(!vuelta.uvs.is_empty(), "no se leyeron los UVs");
    assert_eq!(vuelta.uvs.len(), vuelta.positions.len());

    // Las normales deben ser aproximadamente las mismas (con tolerancia por float32)
    for (a, b) in original.normals.iter().zip(&vuelta.normals) {
        assert!((*a - *b).length() < 1e-5, "{a:?} != {b:?}");
    }
}

/// Prueba que el round-trip con GltfOptions::crudo() cierra exacto.
/// Este es el test que faltaba y permitía que el bug pasara desapercibido.
#[test]
fn glb_ida_y_vuelta_crudo_cierra_exacto() {
    let original = caja(DVec3::new(1.0, 2.0, 3.0), DVec3::new(10.0, 20.0, 30.0));
    let opts = gltf::GltfOptions::crudo();
    let glb = gltf::to_glb(&original, opts).unwrap();
    let vuelta = gltf::read_glb(&glb, opts).unwrap();

    assert_eq!(vuelta.triangle_count(), original.triangle_count());
    assert_eq!(vuelta.positions.len(), original.positions.len());
    assert_eq!(vuelta.indices, original.indices);

    // Con crudo(), las posiciones deben ser idénticas dentro de tolerancia f32
    for (a, b) in original.positions.iter().zip(&vuelta.positions) {
        let diff = (*a - *b).length();
        assert!(
            diff < 1e-5,
            "posiciones no coinciden: {} != {}, diferencia: {}",
            a, b, diff
        );
    }

    // Las normales también deben ser idénticas
    assert_eq!(vuelta.normals.len(), original.normals.len());
    for (a, b) in original.normals.iter().zip(&vuelta.normals) {
        let diff = (*a - *b).length();
        assert!(
            diff < 1e-5,
            "normales no coinciden: {} != {}, diferencia: {}",
            a, b, diff
        );
    }
}

/// Prueba que escribir con crudo() y leer con default() da valores **distintos**.
/// Sin este test, alguien podría volver a cablear la conversión y los round-trips
/// seguirían pasando (porque ambos usarían las mismas conversiones hardcodeadas).
#[test]
fn glb_crudo_vs_default_produce_diferencias() {
    let original = caja(DVec3::new(1.0, 2.0, 3.0), DVec3::new(10.0, 20.0, 30.0));

    // Escribir con crudo(): sin conversiones
    let glb = gltf::to_glb(&original, gltf::GltfOptions::crudo()).unwrap();

    // Leer con default(): aplica las conversiones por defecto
    let vuelta = gltf::read_glb(&glb, gltf::GltfOptions::default()).unwrap();

    // Las posiciones **deben** ser diferentes: escribimos mm/Z-up,
    // pero leemos aplicando conversión mm→m y Z→Y.
    // El original tiene z=3.0 mm en FORGE (Z-up), que en glb crudo sigue siendo 3.0
    // En glb default, eso se interpreta como Y=3.0 en metros = 3000 mm en FORGE Y.
    let mut hay_diferencia_positiva = false;
    for (orig, leido) in original.positions.iter().zip(&vuelta.positions) {
        if (orig - leido).length() > 1.0 {
            hay_diferencia_positiva = true;
            break;
        }
    }
    assert!(
        hay_diferencia_positiva,
        "crudo() vs default() debería producir diferencias"
    );

    // Las normales también deben diferir por la permutación de ejes
    let mut hay_diferencia_normal = false;
    for (orig, leido) in original.normals.iter().zip(&vuelta.normals) {
        if (orig - leido).length() > 0.01 {
            hay_diferencia_normal = true;
            break;
        }
    }
    assert!(
        hay_diferencia_normal,
        "las normales deberían diferir con crudo vs default"
    );
}

// ---------------------------------------------------------------------------
// USD
// ---------------------------------------------------------------------------

#[test]
fn usd_escribe_geometria_estatica_solo() {
    let s = caja(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0));
    let usda = usd::to_string(&s).unwrap();

    // Encabezado
    assert!(usda.contains("#usda 1.0"), "falta header USDA");
    assert!(
        usda.contains("upAxis = \"Z\""),
        "upAxis no es Z (ventaja sobre glTF)"
    );
    assert!(
        usda.contains("metersPerUnit = 0.001"),
        "metersPerUnit debe ser 0.001 para mm"
    );

    // Estructura: geometría estática, sin composición
    assert!(usda.contains("def Xform \"stage\""), "no hay stage");
    assert!(usda.contains("def Mesh"), "no hay Mesh");
    assert!(!usda.contains("def Layer"), "composición no permitida");
    assert!(!usda.contains("variantSet"), "variantes no permitidas");
    assert!(!usda.contains("ref "), "referencias no permitidas");

    // Atributos de malla
    assert!(usda.contains("point3f[]"), "no hay points");
    assert!(usda.contains("faceVertexCounts"), "no hay faceVertexCounts");
    assert!(
        usda.contains("faceVertexIndices"),
        "no hay faceVertexIndices"
    );
    assert!(usda.contains("normal3f[]"), "no hay normales");

    // Valores: todos deben tener 9 decimales de precisión
    assert!(
        usda.contains("10.000000000"),
        "no contiene valores esperados"
    );
    assert!(
        usda.contains("20.000000000"),
        "no contiene valores esperados"
    );
    assert!(
        usda.contains("30.000000000"),
        "no contiene valores esperados"
    );
}

#[test]
fn usd_sin_conversiones_mantiene_precision_exacta() {
    // USD permite Z arriba, así que **no hay conversión de ejes**, a diferencia de glTF
    let s = caja(DVec3::new(1.5, 2.5, 3.5), DVec3::new(11.5, 22.5, 33.5));
    let usda = usd::to_string(&s).unwrap();

    // Los valores deben aparecer sin cambios (Z arriba en los datos y en el formato)
    assert!(usda.contains("1.500000000"), "coordenadas inexactas");
    assert!(usda.contains("3.500000000"), "coordenadas Z no preservadas");

    // Los números están en el mismo rango que en FORGE: sin conversión de metros
    assert!(
        usda.contains("11.500000000"),
        "conversión de unidades no debería ocurrir"
    );
}

#[test]
fn usd_rechaza_mallas_invalidas() {
    let mut s = caja(DVec3::ZERO, DVec3::ONE);
    s.positions.clear();
    assert!(
        usd::to_string(&s).is_err(),
        "debería rechazar malla sin vertices"
    );
}
