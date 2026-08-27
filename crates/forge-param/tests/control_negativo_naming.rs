//! Control negativo del nombrado persistente.
//!
//! Sin esto, la suite de regresión topológica no prueba nada: un resolvedor
//! que devolviera siempre la primera cara pasaría los ~30 casos de
//! `regresion_topologica.rs` igual de bien que uno correcto, porque esos
//! casos solo comprueban que *algo* sigue resuelto. Este archivo comprueba lo
//! contrario: cuando la entidad referenciada **deja de existir de verdad** y
//! nada se le parece, la referencia tiene que salir `Binding::Broken` — nunca
//! re-vinculada en silencio a la candidata más parecida (ADR-0002 §4).

mod comun;
use comun::*;

use forge_doc::Binding;
use forge_kernel_api::GeometryKernel;
use forge_math::DVec3;
use forge_param::*;

/// Referencia a la cara lateral 7 de un octógono (solo existe con `n >= 8`),
/// capturada con normal en ~337.5°. Se reconstruye el sketch como un
/// triángulo (`n = 3`) cuyas caras quedan deliberadamente giradas para que su
/// normal más cercana esté a 60° de la original — muy por debajo del coseno
/// mínimo del resolver (0.90, ⇒ ~25.8°). Ni la genealogía (el índice 7 no
/// existe con 3 lados) ni la firma geométrica (ninguna normal se parece)
/// pueden salvar la referencia: es un caso de ruptura genuina, no un fallo
/// del test.
#[test]
fn una_cara_que_desaparece_de_verdad_sale_binding_broken() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);

    // --- construcción inicial: octógono, capturamos la cara lateral 7 ---
    let mut tree = arbol_extrude_poligono(
        sketch_id,
        extrude_id,
        8,
        10.0,
        Plano::default(),
        DVec3::Z,
        5.0,
    );
    let outcome = evaluar(&k, &tree).unwrap();
    let shape0 = outcome.shape(extrude_id).unwrap();
    let topo0 = k.topology(shape0).unwrap();
    let cara7 = cara_lateral(&topo0, 7).expect("el octogono debe tener cara lateral de indice 7");
    let referencia = TopoRef::capturar(extrude_id, &cara7);
    // Control de cordura: una cara lateral de un prisma tiene normal en el
    // plano XY (perpendicular al eje de extrusion), nunca alineada con Z.
    assert!(
        referencia.firma.normal_q[2].abs() < 50,
        "la cara 7 no deberia mirar hacia Z"
    );

    // --- edicion aguas arriba: el sketch pasa a ser un triangulo girado ---
    let angulos = [15.0, 135.0, 255.0];
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.modelo.points = puntos_poligono_en(&angulos, 10.0);
        s.perfil = (0..3u32).map(forge_kernel_api::sketch::PointId).collect();
    }

    let outcome2 = evaluar(&k, &tree).unwrap();
    let shape1 = outcome2.shape(extrude_id).unwrap();
    let topo1 = k.topology(shape1).unwrap();

    // Capa 1: el indice 7 no existe con un triangulo.
    assert!(
        cara_lateral(&topo1, 7).is_none(),
        "un triangulo no deberia tener cara de indice 7"
    );
    assert!(
        !topo1.faces.iter().any(|f| f.id == referencia.objetivo),
        "el StableId original no deberia sobrevivir"
    );

    // Capa 2 + resultado: sin genealogia y sin firma parecida, Broken.
    let resolucion = Resolver::default().resolver(&referencia, &topo1);
    assert!(
        matches!(resolucion.binding, Binding::Broken),
        "se esperaba Binding::Broken, salio {:?} (puntuacion {:?})",
        resolucion.binding,
        resolucion.puntuacion
    );
    assert!(resolucion.rota());
    assert_eq!(resolucion.valor(), None);

    // Y el criterio "un resolvedor que siempre devuelve la primera cara
    // pasaria los 30 casos anteriores" es exactamente lo que este assert
    // descarta: si el resolver fuera ese, devolveria Bound/Rebound a
    // topo1.faces[0], no Broken.
    assert_ne!(resolucion.valor(), topo1.faces.first().map(|f| f.id));
}

/// El mismo caso, pero a través del árbol completo: un nodo que no puede
/// re-vincular su referencia **aborta la evaluación entera** en vez de
/// calcular con una arista vecina. ADR-0002 §4: "el árbol no se evalúa con
/// referencias rotas". Se usa una referencia de clase `Face` dentro de un
/// `Chamfer` a propósito: lo único que importa aquí es la resolución, que
/// ocurre *antes* de que `calcular` llegue a invocar al kernel — por eso el
/// caso es válido aunque un chaflán real nunca seleccionaría una cara.
#[test]
fn el_arbol_no_se_evalua_con_una_referencia_rota() {
    let k = kernel();
    let sketch_id = fid(1);
    let extrude_id = fid(2);
    let chamfer_id = fid(3);

    let mut tree = arbol_extrude_poligono(
        sketch_id,
        extrude_id,
        8,
        10.0,
        Plano::default(),
        DVec3::Z,
        5.0,
    );
    let _referencia = agregar_chamfer_sobre(&k, &mut tree, extrude_id, chamfer_id, 0.5, |topo| {
        cara_lateral(topo, 7).expect("cara lateral 7")
    });

    let angulos = [15.0, 135.0, 255.0];
    if let NodeKind::Sketch(s) = &mut tree.nodo_mut(sketch_id).unwrap().kind {
        s.modelo.points = puntos_poligono_en(&angulos, 10.0);
        s.perfil = (0..3u32).map(forge_kernel_api::sketch::PointId).collect();
    }

    let resultado = evaluar(&k, &tree);
    match resultado {
        Err(ParamError::ReferenciaRota { nodo, rotas, total }) => {
            assert_eq!(nodo, chamfer_id);
            assert_eq!((rotas, total), (1, 1));
        }
        otro => panic!("se esperaba ParamError::ReferenciaRota, salio {otro:?}"),
    }
}
