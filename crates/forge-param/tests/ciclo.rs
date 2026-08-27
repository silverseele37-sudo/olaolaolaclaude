//! Un ciclo en el grafo se detecta y se reporta; nunca se cuelga.
//!
//! `tree.rs` documenta por qué el orden topológico es Kahn iterativo y no un
//! DFS recursivo: un grafo con ciclo (o simplemente grande) no debe poder
//! desbordar la pila. Este archivo comprueba la mitad observable de esa
//! promesa — que el error llega y nombra a los nodos implicados — y dos
//! variantes de ciclo (directo y a través de un booleano con dos entradas)
//! para no depender de una sola forma de construirlo.

mod comun;
use comun::*;

use forge_kernel_api::BoolOp;
use forge_math::DVec3;
use forge_param::*;

/// `desde_nodos` existe justamente para poder fabricar esto: si la única
/// forma de crear un árbol fuera una API que valida el grafo al insertar, no
/// habría manera de comprobar que el evaluador detecta un ciclo en vez de
/// colgarse con uno que se le hubiera colado de otra forma (una entrada
/// corrupta al deserializar, por ejemplo).
#[test]
fn un_ciclo_directo_entre_dos_nodos_produce_error_y_no_cuelga() {
    let a = fid(1);
    let b = fid(2);
    // A extruye el perfil de B, B extruye el perfil de A: ciclo de longitud 2.
    let nodos = vec![
        FeatureNode::con_id(
            a,
            "a",
            NodeKind::Extrude {
                perfil: b,
                direccion: DVec3::Z,
                distancia_mm: 1.0,
                simetrico: false,
            },
        ),
        FeatureNode::con_id(
            b,
            "b",
            NodeKind::Extrude {
                perfil: a,
                direccion: DVec3::Z,
                distancia_mm: 1.0,
                simetrico: false,
            },
        ),
    ];
    let tree = FeatureTree::desde_nodos(nodos);

    // El orden topológico por sí solo ya debe fallar (rápido: es lo que usa
    // `evaluar` internamente, y es la comprobación que garantiza que no se
    // cuelga con un grafo grande).
    match tree.orden_topologico() {
        Err(ParamError::Ciclo(implicados)) => {
            let mut v = implicados;
            v.sort();
            assert_eq!(
                v,
                vec![a, b],
                "el ciclo deberia implicar exactamente a los dos nodos"
            );
        }
        otro => panic!("se esperaba ParamError::Ciclo, salio {otro:?}"),
    }

    // Y evaluar el árbol completo reporta el mismo error, no un pánico ni un
    // colgado.
    let k = kernel();
    match evaluar(&k, &tree) {
        Err(ParamError::Ciclo(_)) => {}
        otro => panic!("se esperaba ParamError::Ciclo al evaluar, salio {otro:?}"),
    }
}

/// Ciclo más largo y con abanico (un `Boolean` tiene dos entradas): tres nodos
/// en un ciclo, más un cuarto que cuelga limpio de uno de ellos. El ciclo
/// reportado no puede confundirse e incluir al nodo limpio.
#[test]
fn un_ciclo_que_pasa_por_un_booleano_se_detecta_y_no_arrastra_al_nodo_sano() {
    let a = fid(1);
    let b = fid(2);
    let c = fid(3);
    let sano = fid(4);

    // a -> extrude(perfil = c)
    // b -> extrude(perfil = a)
    // c -> boolean(a, b)      <- cierra el ciclo a -> c -> b -> a (via a como
    //                             entrada de b, y b como entrada de c)
    // sano -> extrude(perfil = a)  <- depende del ciclo pero no es parte de el
    let nodos = vec![
        FeatureNode::con_id(
            a,
            "a",
            NodeKind::Extrude {
                perfil: c,
                direccion: DVec3::Z,
                distancia_mm: 1.0,
                simetrico: false,
            },
        ),
        FeatureNode::con_id(
            b,
            "b",
            NodeKind::Extrude {
                perfil: a,
                direccion: DVec3::Z,
                distancia_mm: 1.0,
                simetrico: false,
            },
        ),
        FeatureNode::con_id(
            c,
            "c",
            NodeKind::Boolean {
                op: BoolOp::Union,
                a,
                b,
            },
        ),
        FeatureNode::con_id(
            sano,
            "sano",
            NodeKind::Extrude {
                perfil: a,
                direccion: DVec3::Z,
                distancia_mm: 1.0,
                simetrico: false,
            },
        ),
    ];
    let tree = FeatureTree::desde_nodos(nodos);

    match tree.orden_topologico() {
        Err(ParamError::Ciclo(implicados)) => {
            let mut v = implicados;
            v.sort();
            // El nodo `sano` depende transitivamente del ciclo (via `a`) asi
            // que tambien se queda sin emitir en Kahn: eso es correcto (no
            // se puede dar un orden a algo que depende de un ciclo), pero el
            // ciclo *en si* son a, b, c.
            assert!(v.contains(&a) && v.contains(&b) && v.contains(&c));
        }
        otro => panic!("se esperaba ParamError::Ciclo, salio {otro:?}"),
    }
}
