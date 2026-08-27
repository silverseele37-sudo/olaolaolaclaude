//! Solver 2D: tests de respuesta conocida.
//!
//! `solver.rs` documenta por qué lo difícil no es converger sino
//! diagnosticar: `UnderConstrained` sale del **rango** del jacobiano (no del
//! recuento nominal) y `OverConstrained` sale del **espacio nulo por la
//! izquierda**, con los índices exactos de las restricciones en conflicto.
//! Los tres números de aquí se derivan a mano en el comentario de cada test;
//! ninguno es "parece razonable".

mod comun;
use comun::*;

use forge_kernel_api::sketch::{Constraint, SketchModel, SketchSolver, SolveStatus};
use forge_math::DVec2;
use forge_param::{GaussNewtonSolver, Plano};

/// Rectángulo de 30×12: cuatro puntos, un ancla, horizontal/vertical en los
/// cuatro lados y dos cotas de distancia — exactamente 8 ecuaciones para 8
/// grados de libertad (2 por punto × 4 puntos). Resuelve **exacto**: no hay
/// margen para "parece razonable", las cuatro esquinas tienen coordenadas
/// derivables a mano.
#[test]
fn el_rectangulo_totalmente_restringido_resuelve_exacto_a_las_cotas_pedidas() {
    let r = sketch_rectangulo(30.0, 12.0, Plano::default());
    let solver = GaussNewtonSolver::default();
    let resultado = solver.solve(&r.sketch.modelo);

    assert_eq!(resultado.status, SolveStatus::Ok, "deberia quedar completamente restringido: {:?}", resultado.status);
    let esperado = [DVec2::new(0.0, 0.0), DVec2::new(30.0, 0.0), DVec2::new(30.0, 12.0), DVec2::new(0.0, 12.0)];
    for (i, (p, e)) in resultado.positions.iter().zip(esperado).enumerate() {
        assert!((*p - e).length() < 1e-7, "punto {i}: {p:?} vs esperado {e:?}");
    }
}

/// El mismo rectángulo, pero sin `Fixed(p0)`: el sistema queda invariante
/// bajo traslación (nada fija la posición absoluta) pero no bajo rotación ni
/// escala (`Horizontal`/`Vertical` fijan la orientación; las dos `Distance`
/// fijan el tamaño). Grados de libertad esperados: **2**, no 0 y no un
/// número mayor por alguna restricción redundante — es la distinción exacta
/// que `solver.rs` dice que importa (rango del jacobiano, no recuento
/// nominal) y aquí los dos coinciden porque no hay redundancia.
#[test]
fn un_sketch_sin_anclar_reporta_underconstrained_con_los_dos_grados_de_libertad_de_traslacion() {
    let mut r = sketch_rectangulo(30.0, 12.0, Plano::default());
    r.sketch.modelo.constraints.retain(|c| !matches!(c, Constraint::Fixed(_)));
    assert_eq!(r.sketch.modelo.nominal_dof(), 2, "cuenta nominal: 8 vars - 6 ecuaciones = 2");

    let solver = GaussNewtonSolver::default();
    let resultado = solver.solve(&r.sketch.modelo);
    match resultado.status {
        SolveStatus::UnderConstrained { dof } => assert_eq!(dof, 2, "se esperaban 2 grados de libertad (traslacion en x,y)"),
        otro => panic!("se esperaba UnderConstrained{{dof:2}}, salio {otro:?}"),
    }
}

/// Dos cotas de distancia contradictorias sobre el mismo par de puntos: p0
/// anclado en el origen, p1 libre, con `Distance(p0,p1,10)` (índice de
/// restricción 1) y `Distance(p0,p1,20)` (índice 2) a la vez.
///
/// Derivación a mano: las dos restricciones tienen el **mismo jacobiano**
/// (∂|p1-p0|/∂p en ambas filas es idéntico: no depende del valor objetivo,
/// solo de la geometria), así que su diferencia vive en el espacio nulo por
/// la izquierda con vector `y=(0,0,1,-1)` sobre las cuatro filas (Fixed.x,
/// Fixed.y, Distance10, Distance20). `y·r = (|p1-p0|-10) - (|p1-p0|-20) = 10`
/// para cualquier posición de `p1`: no depende de dónde converja el
/// optimizador, así que la respuesta es exacta y no depende de la
/// implementación numérica. Los índices en conflicto son exactamente `[1,2]`
/// — las dos `Distance`, no el `Fixed` que las ancla.
#[test]
fn dos_cotas_de_distancia_contradictorias_reportan_overconstrained_con_los_indices_exactos() {
    let mut m = SketchModel::default();
    let p0 = m.add_point(DVec2::new(0.0, 0.0));
    let p1 = m.add_point(DVec2::new(5.0, 0.0));
    let d10 = m.add_dimension(10.0);
    let d20 = m.add_dimension(20.0);
    m.constraints = vec![
        Constraint::Fixed(p0),                                  // indice 0
        Constraint::Distance { a: p0, b: p1, dim: d10 },         // indice 1
        Constraint::Distance { a: p0, b: p1, dim: d20 },         // indice 2
    ];

    let solver = GaussNewtonSolver::default();
    let resultado = solver.solve(&m);
    match resultado.status {
        SolveStatus::OverConstrained { conflicting } => {
            assert_eq!(conflicting, vec![1, 2], "las restricciones en conflicto deberian ser las dos Distance, no el Fixed");
        }
        otro => panic!("se esperaba OverConstrained{{conflicting:[1,2]}}, salio {otro:?}"),
    }
}

/// Control positivo de los dos tests anteriores: el mismo rectángulo, con
/// todo consistente y anclado, converge a `Ok` sin grados de libertad ni
/// conflictos. Sin este control, un solver que devolviera siempre
/// `UnderConstrained` u `OverConstrained` pasaría los dos tests de arriba
/// igual de bien que uno correcto.
#[test]
fn un_sketch_bien_planteado_no_reporta_ni_underconstrained_ni_overconstrained() {
    let r = sketch_rectangulo(7.5, 3.25, Plano::default());
    let solver = GaussNewtonSolver::default();
    let resultado = solver.solve(&r.sketch.modelo);
    assert_eq!(resultado.status, SolveStatus::Ok);
}
