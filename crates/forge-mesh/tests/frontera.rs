//! La frontera de dominio y la pila de modificadores.
//!
//! Las mallas de prueba se construyen **a mano**, sin depender de
//! `forge-kernel-stub`. No es por purismo: probar contra teselados fabricados
//! aquí demuestra que este pilar está genuinamente desacoplado del kernel, que
//! es lo que ADR-0006 exige y lo que un `dev-dependency` habría dejado sin
//! comprobar.

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_kernel_api::Tessellation;
use forge_math::{Aabb, DVec3};
use forge_mesh::*;

fn f() -> FeatureId {
    FeatureId::from_u128(1)
}

fn cara(n: u64) -> StableId {
    StableId { origin: f(), class: TopoClass::Face, mark: n }
}

/// Teselado de un cubo de lado `l`: 12 triángulos, 6 caras identificables,
/// con vértices duplicados por cara como los produce un kernel de verdad.
fn teselado_de_cubo(l: f64) -> Tessellation {
    let h = l * 0.5;
    let c = [
        DVec3::new(-h, -h, -h), DVec3::new(h, -h, -h), DVec3::new(h, h, -h), DVec3::new(-h, h, -h),
        DVec3::new(-h, -h, h), DVec3::new(h, -h, h), DVec3::new(h, h, h), DVec3::new(-h, h, h),
    ];
    let caras: [[usize; 4]; 6] = [
        [0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3],
    ];
    let normales = [-DVec3::Z, DVec3::Z, -DVec3::Y, DVec3::Y, DVec3::X, -DVec3::X];

    let mut t = Tessellation::default();
    for (fi, cara_) in caras.iter().enumerate() {
        let base = t.positions.len() as u32;
        for &i in cara_ {
            t.positions.push(c[i]);
            t.normals.push(normales[fi]);
        }
        t.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        t.face_of_triangle.push(cara(fi as u64));
        t.face_of_triangle.push(cara(fi as u64));
    }
    t.bbox = Aabb::from_points(t.positions.iter().copied());
    t
}

// ---------------------------------------------------------------------------
// La puerta
// ---------------------------------------------------------------------------

#[test]
fn to_mesh_conserva_la_identidad_de_cada_cara() {
    let t = teselado_de_cubo(10.0);
    let m = to_mesh(&t).unwrap();

    // El teselado trae 24 vertices duplicados por cara; la malla los suelda a 8
    // porque sin adyacencia ningun modificador funciona.
    assert_eq!(m.vertex_count(), 8, "no se soldaron los duplicados del teselado");
    assert_eq!(m.face_count(), 12, "12 triangulos");
    assert_eq!(m.euler(), 2, "V - E + F debe ser 2 en una malla cerrada");

    assert_eq!(m.prov.cobertura(), 1.0, "alguna cara perdio procedencia al cruzar");
    for i in 0..6u64 {
        assert_eq!(m.prov.caras_de(cara(i)).len(), 2, "la cara {i} deberia dar 2 triangulos");
    }

    assert!((m.area() - 600.0).abs() < 1e-9, "area {}", m.area());
    assert!((m.signed_volume() - 1000.0).abs() < 1e-9, "volumen {}", m.signed_volume());
}

/// R1: el teselado que entra **no se toca**. Es caché derivada, no geometría
/// editable, y confundirlo es la clase de bug que produce dos representaciones
/// divergentes del mismo objeto.
#[test]
fn to_mesh_no_modifica_el_teselado_de_entrada() {
    let t = teselado_de_cubo(4.0);
    let antes = (t.positions.clone(), t.indices.clone(), t.face_of_triangle.clone());
    let _ = to_mesh(&t).unwrap();
    assert_eq!(t.positions, antes.0);
    assert_eq!(t.indices, antes.1);
    assert_eq!(t.face_of_triangle, antes.2);
}

#[test]
fn to_mesh_rechaza_teselados_invalidos() {
    let mut t = teselado_de_cubo(1.0);
    t.face_of_triangle.pop();
    assert!(to_mesh(&t).is_err(), "procedencia incompleta deberia rechazarse");

    let mut t2 = teselado_de_cubo(1.0);
    t2.indices.push(9999);
    assert!(to_mesh(&t2).is_err());
}

// ---------------------------------------------------------------------------
// Catmull-Clark: recuentos exactos
// ---------------------------------------------------------------------------

#[test]
fn subdivision_de_un_cubo_da_los_recuentos_exactos() {
    let c = cubo(2.0, f());
    assert_eq!((c.vertex_count(), c.edge_count(), c.face_count()), (8, 12, 6));
    assert_eq!(c.euler(), 2);

    // Nivel 1: 6 puntos de cara + 12 de arista + 8 de vertice = 26
    //          6 caras x 4 = 24 cuadrilateros ·  24x4/2 = 48 aristas
    let n1 = Subdivide::new(1).apply(&c).unwrap();
    n1.validate().unwrap();
    assert_eq!((n1.vertex_count(), n1.edge_count(), n1.face_count()), (26, 48, 24));
    assert_eq!(n1.euler(), 2);

    // Nivel 2: 24 + 48 + 26 = 98 ·  24x4 = 96 caras ·  96x4/2 = 192 aristas
    let n2 = Subdivide::new(2).apply(&c).unwrap();
    n2.validate().unwrap();
    assert_eq!((n2.vertex_count(), n2.edge_count(), n2.face_count()), (98, 192, 96));
    assert_eq!(n2.euler(), 2);

    // La superficie limite esta inscrita: el volumen baja y sigue siendo
    // positivo. Si subiera, los bucles estarian invertidos en algun sitio.
    let (v0, v1, v2) = (c.signed_volume(), n1.signed_volume(), n2.signed_volume());
    assert!(v2 < v1 && v1 < v0, "volumenes {v0} {v1} {v2}");
    assert!(v2 > 0.0, "volumen negativo: caras invertidas");
    // y converge: el salto del nivel 2 es menor que el del nivel 1
    assert!((v1 - v2) < (v0 - v1), "no converge: {v0} {v1} {v2}");
}

#[test]
fn la_subdivision_conserva_la_procedencia_de_cada_cara_original() {
    let c = cubo(2.0, f());
    let n2 = Subdivide::new(2).apply(&c).unwrap();
    assert_eq!(n2.prov.cobertura(), 1.0);
    // cada cara original produjo 16 caras al segundo nivel
    for i in 0..6u64 {
        assert_eq!(
            n2.prov.caras_de(StableId { origin: f(), class: TopoClass::Face, mark: 1000 + i }).len(),
            16,
            "la cara {i} deberia dar 4^2 = 16 caras"
        );
    }
}

/// Catmull-Clark da **n** cuadrilateros por cada n-gono. Es facil asumir que
/// siempre multiplica por 4 -- cierto solo si la malla ya es de cuadrilateros --
/// y esa suposicion produce recuentos equivocados en cuanto entra un triangulo.
#[test]
fn la_subdivision_produce_n_cuadrilateros_por_cada_n_gono() {
    // tetraedro: 4 caras triangulares
    let tetra = {
        let mut m = cubo(2.0, f());
        m.positions = vec![
            DVec3::new(1.0, 1.0, 1.0), DVec3::new(1.0, -1.0, -1.0),
            DVec3::new(-1.0, 1.0, -1.0), DVec3::new(-1.0, -1.0, 1.0),
        ];
        m.normals.clear();
        m.faces = vec![
            forge_mesh::Face { verts: vec![0, 2, 1] },
            forge_mesh::Face { verts: vec![0, 1, 3] },
            forge_mesh::Face { verts: vec![0, 3, 2] },
            forge_mesh::Face { verts: vec![1, 2, 3] },
        ];
        m.prov.face_origin.truncate(4);
        m
    };
    tetra.validate().unwrap();
    assert_eq!(tetra.euler(), 2, "4 - 6 + 4");

    let n1 = Subdivide::new(1).apply(&tetra).unwrap();
    assert_eq!(n1.face_count(), 12, "4 triangulos x 3 = 12 cuadrilateros, no 16");
    assert_eq!(n1.euler(), 2);
    // y el segundo nivel si multiplica por 4, porque ya son cuadrilateros
    let n2 = Subdivide::new(2).apply(&tetra).unwrap();
    assert_eq!(n2.face_count(), 48);
}

#[test]
fn la_subdivision_acota_los_niveles() {
    let c = cubo(1.0, f());
    assert!(Subdivide::new(7).apply(&c).is_err(), "7 niveles son 4^7 caras: deberia negarse");
}

// ---------------------------------------------------------------------------
// Resto de modificadores
// ---------------------------------------------------------------------------

#[test]
fn espejo_duplica_las_caras_y_el_volumen() {
    let c = cubo(2.0, f());
    let v0 = c.signed_volume();
    let m = Mirror { punto: DVec3::new(3.0, 0.0, 0.0), normal: DVec3::X, soldar_costura: false }
        .apply(&c)
        .unwrap();
    m.validate().unwrap();
    assert_eq!(m.face_count(), 12);
    assert_eq!(m.prov.cobertura(), 1.0);
    // El reflejo invierte la orientacion; si el bucle no se diera la vuelta,
    // el volumen de la mitad espejada restaria en vez de sumar.
    assert!((m.signed_volume() - 2.0 * v0).abs() < 1e-9, "volumen {}", m.signed_volume());
}

#[test]
fn el_espejo_rechaza_un_plano_degenerado() {
    let c = cubo(1.0, f());
    assert!(Mirror { punto: DVec3::ZERO, normal: DVec3::ZERO, soldar_costura: false }
        .apply(&c)
        .is_err());
}

#[test]
fn array_multiplica_caras_y_volumen_por_n() {
    let c = cubo(2.0, f());
    let v0 = c.signed_volume();
    for n in [1u32, 2, 5] {
        let a = Array { copias: n, desplazamiento: DVec3::new(10.0, 0.0, 0.0) }.apply(&c).unwrap();
        a.validate().unwrap();
        assert_eq!(a.face_count(), 6 * n as usize);
        assert!((a.signed_volume() - v0 * n as f64).abs() < 1e-9);
        assert_eq!(a.prov.cobertura(), 1.0);
    }
    assert!(Array { copias: 0, desplazamiento: DVec3::X }.apply(&c).is_err());
}

#[test]
fn triangular_convierte_cada_cuadrilatero_en_dos() {
    let c = cubo(2.0, f());
    let t = Triangulate.apply(&c).unwrap();
    t.validate().unwrap();
    assert_eq!(t.face_count(), 12, "6 cuadrilateros -> 12 triangulos");
    assert_eq!(t.vertex_count(), 8, "triangular no anade vertices");
    assert!((t.signed_volume() - c.signed_volume()).abs() < 1e-9, "el volumen no debe cambiar");
    assert_eq!(t.prov.cobertura(), 1.0);
}

#[test]
fn soldar_colapsa_duplicados_y_descarta_caras_degeneradas() {
    let c = cubo(2.0, f());
    // dos copias exactamente encima: la soldadura las funde
    let doble = Array { copias: 2, desplazamiento: DVec3::ZERO }.apply(&c).unwrap();
    assert_eq!(doble.vertex_count(), 16);
    let w = Weld { epsilon_mm: 1e-6 }.apply(&doble).unwrap();
    assert_eq!(w.vertex_count(), 8, "no se soldaron los duplicados");
    assert_eq!(w.face_count(), 12, "las caras siguen siendo 12, ahora compartiendo vertices");
    assert!(Weld { epsilon_mm: 0.0 }.apply(&c).is_err(), "epsilon nulo deberia rechazarse");
}

// ---------------------------------------------------------------------------
// La pila, y el control que la hace valer
// ---------------------------------------------------------------------------

#[test]
fn la_pila_completa_conserva_toda_la_procedencia() {
    let m = to_mesh(&teselado_de_cubo(10.0)).unwrap();
    let pila = ModifierStack::new()
        .push(Subdivide::new(2))
        .push(Mirror { punto: DVec3::new(20.0, 0.0, 0.0), normal: DVec3::X, soldar_costura: false })
        .push(Array { copias: 3, desplazamiento: DVec3::new(0.0, 50.0, 0.0) })
        .push(Triangulate);

    let salida = pila.apply(&m).unwrap();
    salida.validate().unwrap();
    assert_eq!(
        salida.prov.cobertura(),
        1.0,
        "la pila completa perdio procedencia; una seleccion no sobreviviria"
    );
    // Catmull-Clark produce **n** cuadrilateros para un n-gono, no siempre 4:
    // un triangulo da 3. Asi que 12 triangulos -> 36 quads (nivel 1) -> 144
    // (nivel 2) -> 288 (espejo) -> 864 (array) -> 1728 al triangular.
    assert_eq!(salida.face_count(), 12 * 3 * 4 * 2 * 3 * 2);

    // y las 6 caras originales del solido siguen siendo localizables
    for i in 0..6u64 {
        assert!(!salida.prov.caras_de(cara(i)).is_empty(), "se perdio la cara {i}");
    }
}

/// **El control que hace que todo lo anterior signifique algo.**
///
/// Un modificador que devuelve un mapa de procedencia vacío compila igual de
/// bien que uno correcto. Si la pila no lo detectara, los tests de arriba
/// pasarían con una implementación que no propaga nada.
#[test]
fn la_pila_detecta_un_modificador_que_no_propaga_procedencia() {
    struct ModificadorRoto;
    impl Modifier for ModificadorRoto {
        fn kind(&self) -> &'static str {
            "roto_a_proposito"
        }
        fn params_hash(&self) -> u64 {
            0
        }
        fn apply(&self, input: &Mesh) -> forge_mesh::Result<Mesh> {
            let mut m = input.clone();
            // tira la procedencia, que es justo lo prohibido
            m.prov = ProvenanceMap::con_capacidad(m.faces.len());
            Ok(m)
        }
    }

    let c = cubo(2.0, f());
    // aplicado a mano no se entera nadie...
    let suelto = ModificadorRoto.apply(&c).unwrap();
    assert_eq!(suelto.prov.cobertura(), 0.0);

    // ...pero la pila lo caza, y dice cuantas caras y cual modificador
    let r = ModifierStack::new().push(ModificadorRoto).apply(&c);
    match r {
        Err(MeshError::ProcedenciaPerdida(kind, perdidas, total)) => {
            assert_eq!(kind, "roto_a_proposito");
            assert_eq!((perdidas, total), (6, 6));
        }
        otro => panic!("la pila no detecto la perdida de procedencia: {otro:?}"),
    }
}

#[test]
fn la_pila_tiene_clave_de_cache_estable() {
    let a = ModifierStack::new().push(Subdivide::new(2)).push(Triangulate);
    let b = ModifierStack::new().push(Subdivide::new(2)).push(Triangulate);
    let c = ModifierStack::new().push(Subdivide::new(3)).push(Triangulate);
    let d = ModifierStack::new().push(Triangulate).push(Subdivide::new(2));
    assert_eq!(a.params_hash(), b.params_hash());
    assert_ne!(a.params_hash(), c.params_hash(), "cambiar un parametro debe cambiar la clave");
    assert_ne!(a.params_hash(), d.params_hash(), "el orden de la pila es semantica");
}

// ---------------------------------------------------------------------------
// Re-vinculación
// ---------------------------------------------------------------------------

#[test]
fn una_seleccion_sobrevive_a_un_reteselado_mas_fino() {
    // seleccion hecha sobre la malla original
    let m0 = to_mesh(&teselado_de_cubo(10.0)).unwrap();
    let seleccion: Vec<StableId> = (0..6).map(cara).collect();
    assert_eq!(tasa_de_revinculacion(&seleccion, &m0), 1.0);

    // el usuario cambia una cota aguas arriba: llega un teselado distinto,
    // y ademas se aplican modificadores
    let m1 = ModifierStack::new()
        .push(Subdivide::new(1))
        .push(Triangulate)
        .apply(&to_mesh(&teselado_de_cubo(14.0)).unwrap())
        .unwrap();

    let tasa = tasa_de_revinculacion(&seleccion, &m1);
    assert_eq!(tasa, 1.0, "tasa de re-vinculacion {tasa}, se exige >= 0.95");
    for r in rebind(&seleccion, &m1) {
        assert!(!r.binding.is_broken());
        assert!(!r.faces.is_empty());
    }
}

/// **Control negativo.** Si la cara referenciada deja de existir, la referencia
/// tiene que salir `Broken` — no re-vinculada a la más parecida. Sin este test,
/// un `rebind` que devolviera siempre la cara 0 pasaría el test anterior.
#[test]
fn una_referencia_a_algo_que_ya_no_existe_sale_rota() {
    let m = to_mesh(&teselado_de_cubo(10.0)).unwrap();
    let fantasma = cara(999);
    let r = rebind(&[fantasma], &m);
    assert_eq!(r.len(), 1);
    assert!(r[0].binding.is_broken(), "deberia estar Broken, salio {:?}", r[0].binding);
    assert!(r[0].faces.is_empty());
    assert_eq!(tasa_de_revinculacion(&[fantasma], &m), 0.0);

    // mezcla: 6 buenas y 2 fantasmas -> 0.75 exacto
    let mezcla: Vec<StableId> = (0..6).map(cara).chain([cara(998), cara(999)]).collect();
    assert!((tasa_de_revinculacion(&mezcla, &m) - 0.75).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// Control positivo de validate
// ---------------------------------------------------------------------------

#[test]
fn validate_detecta_las_formas_de_estar_rota() {
    let base = cubo(2.0, f());
    base.validate().unwrap();

    let mut sin_prov = base.clone();
    sin_prov.prov.face_origin.pop();
    assert!(sin_prov.validate().is_err(), "no detecto mapa de procedencia descuadrado");

    let mut fuera = base.clone();
    fuera.faces[0].verts[0] = 99;
    assert!(fuera.validate().is_err(), "no detecto indice fuera de rango");

    let mut repetido = base.clone();
    repetido.faces[0].verts[1] = repetido.faces[0].verts[0];
    assert!(repetido.validate().is_err(), "no detecto vertice repetido en un bucle");

    let mut normales = base.clone();
    normales.normals = vec![DVec3::Z; 3];
    assert!(normales.validate().is_err(), "no detecto normales descuadradas");

    // orientacion incoherente: dos caras recorriendo la misma arista igual
    let mut orientacion = base.clone();
    orientacion.faces[1].verts.reverse();
    assert!(
        orientacion.validate().is_err(),
        "no detecto media arista repetida: la orientacion esta incoherente"
    );
}
