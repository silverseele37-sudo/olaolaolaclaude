//! Test de humo: valida el andamiaje de `comun` antes de construir la suite
//! grande encima. Si esto no compila o no pasa, nada de lo demás vale nada.

mod comun;
use comun::*;

use forge_param::*;

#[test]
fn arbol_simple_evalua_y_produce_forma() {
    let k = kernel();
    let tree = arbol_extrude_poligono(
        fid(1),
        fid(2),
        6,
        10.0,
        Plano::default(),
        forge_math::DVec3::Z,
        5.0,
    );
    let outcome = evaluar(&k, &tree).unwrap();
    assert!(outcome.shape(fid(2)).is_some());
}

#[test]
fn fillet_capturado_se_revincula_tras_un_cambio_de_distancia() {
    let k = kernel();
    let mut tree = arbol_extrude_poligono(
        fid(1),
        fid(2),
        6,
        10.0,
        Plano::default(),
        forge_math::DVec3::Z,
        5.0,
    );
    let original = agregar_fillet_sobre(&k, &mut tree, fid(2), fid(3), 1.0, |t| t.edges[0].clone());

    set_distancia(&mut tree, fid(2), 8.0);
    let r = medir("humo", &k, &tree, fid(3));
    assert!(
        r.exacta(),
        "se esperaba genealogia exacta, salio {:?}",
        r.binding
    );
    assert_eq!(r.valor(), Some(original.objetivo));
}
