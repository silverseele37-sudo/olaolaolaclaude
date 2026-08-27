//! La caché es de **contenido**, no de identidad ni de "algo cambió".
//!
//! `eval.rs` lo dice en su doc de módulo: la clave es
//! `hash(identidad, tipo, parámetros, referencias, claves de las entradas)`.
//! Editar algo que no entra en esa clave —renombrar un nodo, mover una cota
//! que ninguna restricción lee— no puede disparar ni un recálculo ni una
//! llamada al kernel aguas abajo. Este archivo lo mide contando llamadas al
//! kernel **desde fuera** del evaluador (con [`KernelContado`]), no fiándose
//! solo de `EvalStats`, que es la contabilidad del propio sistema bajo
//! prueba: un evaluador que llevara mal la cuenta pasaría igual si nos
//! fiáramos solo de sus propios números.

mod comun;
use comun::*;

use forge_kernel_api::sketch::{Constraint, DimId, SketchModel};
use forge_math::{DVec2, DVec3};
use forge_param::*;

/// Renombrar un nodo no cambia su geometría. Es la comprobación más simple de
/// que la caché es de contenido: `nombre` ni siquiera entra en el hash
/// (`tree.rs`, doc de `FeatureNode::nombre`).
#[test]
fn renombrar_un_nodo_no_dispara_ningun_recalculo() {
    let inner = kernel();
    let k = KernelContado::nuevo(&inner);

    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);
    let mut tree = arbol_extrude_poligono(
        sketch_id,
        extrude_id,
        6,
        10.0,
        Plano::default(),
        DVec3::Z,
        5.0,
    );
    let _ = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        t.edges[0].clone()
    });

    let mut ev = Evaluator::new(&k);
    let out1 = ev.evaluar(&tree).unwrap();
    assert_eq!(
        out1.stats.nodos_calculados, 3,
        "sketch + extrude + fillet, los tres de cero"
    );
    let llamadas_tras_primera = k.cuenta();
    assert!(llamadas_tras_primera > 0);

    tree.nodo_mut(sketch_id).unwrap().nombre = "otro nombre completamente distinto".into();
    tree.nodo_mut(extrude_id).unwrap().nombre = "y este tambien".into();

    let out2 = ev.evaluar(&tree).unwrap();
    assert_eq!(
        out2.stats.aciertos_cache, 3,
        "los tres nodos deberian venir de cache"
    );
    assert_eq!(
        k.cuenta(),
        llamadas_tras_primera,
        "renombrar no debe llamar al kernel ni una vez mas"
    );
    // Y el resultado sigue siendo el mismo shape (misma clave de cache).
    assert_eq!(out1.shape(fillet_id), out2.shape(fillet_id));
    assert_eq!(out1.clave(fillet_id), out2.clave(fillet_id));
}

/// Una cota que ninguna restricción lee no cambia la geometría resuelta. Es
/// el caso que justifica que el sketch se hashee por su **perfil resuelto**
/// y no por sus parámetros crudos (`eval.rs`, comentario sobre
/// `NodeKind::Sketch`).
#[test]
fn una_cota_sin_uso_no_invalida_nada_aguas_abajo() {
    let inner = kernel();
    let k = KernelContado::nuevo(&inner);

    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let fillet_id = fid(3);

    // Rectangulo con las cuatro esquinas ancladas por posicion (Fixed en
    // los cuatro puntos: no hace falta el solver para nada), mas una cota de
    // distancia que **no participa en ninguna restriccion** -- una anotacion
    // huerfana, como la que deja un usuario que borro la linea pero no la
    // cota.
    let mut modelo = SketchModel::default();
    let p0 = modelo.add_point(DVec2::new(0.0, 0.0));
    let p1 = modelo.add_point(DVec2::new(10.0, 0.0));
    let p2 = modelo.add_point(DVec2::new(10.0, 10.0));
    let p3 = modelo.add_point(DVec2::new(0.0, 10.0));
    let huerfana: DimId = modelo.add_dimension(42.0);
    modelo.constraints = vec![
        Constraint::Fixed(p0),
        Constraint::Fixed(p1),
        Constraint::Fixed(p2),
        Constraint::Fixed(p3),
    ];
    let sketch = SketchNode {
        modelo,
        perfil: vec![p0, p1, p2, p3],
        plano: Plano::default(),
    };

    let mut tree = FeatureTree::new();
    tree.insertar(FeatureNode::con_id(
        sketch_id,
        "sketch",
        NodeKind::Sketch(sketch),
    ));
    tree.insertar(FeatureNode::con_id(
        extrude_id,
        "extrude",
        NodeKind::Extrude {
            perfil: sketch_id,
            direccion: DVec3::Z,
            distancia_mm: 5.0,
            simetrico: false,
        },
    ));
    let _ = agregar_fillet_sobre(&k, &mut tree, extrude_id, fillet_id, 1.0, |t| {
        t.edges[0].clone()
    });

    let mut ev = Evaluator::new(&k);
    let out1 = ev.evaluar(&tree).unwrap();
    let llamadas1 = k.cuenta();

    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        assert!(s.modelo.set_dimension(huerfana, 999.0));
    }

    let out2 = ev.evaluar(&tree).unwrap();
    assert_eq!(
        k.cuenta(),
        llamadas1,
        "una cota que nadie lee no debe tocar el kernel"
    );
    assert_eq!(out2.stats.aciertos_cache, 3);
    assert_eq!(out1.clave(fillet_id), out2.clave(fillet_id));
}

/// Control positivo del test anterior: si la cota **sí** participa en una
/// restricción, cambiarla sí debe invalidar la cache y llamar al kernel. Sin
/// este control, los dos tests de arriba pasarían igual con un evaluador que
/// jamás invalidara nada.
#[test]
fn en_cambio_una_cota_que_si_se_usa_si_invalida() {
    let inner = kernel();
    let k = KernelContado::nuevo(&inner);

    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let (mut tree, dim_ancho, _dim_alto) = arbol_extrude_rectangulo(
        sketch_id,
        extrude_id,
        10.0,
        10.0,
        Plano::default(),
        DVec3::Z,
        5.0,
    );

    let mut ev = Evaluator::new(&k);
    let out1 = ev.evaluar(&tree).unwrap();
    let llamadas1 = k.cuenta();

    set_ancho(&mut tree, sketch_id, dim_ancho, 20.0);
    let out2 = ev.evaluar(&tree).unwrap();

    assert!(
        k.cuenta() > llamadas1,
        "una cota que si se usa debe volver a llamar al kernel"
    );
    assert_eq!(out2.stats.aciertos_cache, 0);
    assert_ne!(out1.clave(extrude_id), out2.clave(extrude_id));
}
