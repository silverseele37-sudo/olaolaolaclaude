//! Contrato del sketch 2D con restricciones.
//!
//! El sketch vive en el plano local `Z = 0`; su colocación en el espacio la pone
//! el nodo del árbol de features, no el sketch.
//!
//! Lo difícil de un solver de restricciones **no es converger**: es diagnosticar.
//! Un solver que devuelve «no pude» sin decir qué restricciones están en
//! conflicto ni cuántos grados de libertad quedan sueltos es inservible para el
//! usuario, que no tiene forma de arreglar su sketch. Por eso [`SolveStatus`]
//! lleva el diagnóstico y no es un booleano.

use forge_math::DVec2;
use serde::{Deserialize, Serialize};

/// Índice de un punto dentro del sketch.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct PointId(pub u32);

/// Índice de una cota con nombre. Es lo que la interfaz edita y lo que un
/// comando `SetDimension` referencia.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct DimId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SketchEntity {
    Line {
        a: PointId,
        b: PointId,
    },
    /// Círculo con centro y un punto del borde: así el radio es geometría y no
    /// un parámetro suelto, y las restricciones lo alcanzan igual que a todo lo
    /// demás.
    Circle {
        center: PointId,
        rim: PointId,
    },
    Arc {
        center: PointId,
        start: PointId,
        end: PointId,
    },
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Constraint {
    /// Dos puntos ocupan la misma posición.
    Coincident(PointId, PointId),
    /// El punto no se mueve. Es lo que ancla el sketch: sin al menos uno, el
    /// sistema siempre queda con 2 grados de libertad de traslación.
    Fixed(PointId),
    Horizontal(PointId, PointId),
    Vertical(PointId, PointId),
    /// Distancia con cota editable.
    Distance {
        a: PointId,
        b: PointId,
        dim: DimId,
    },
    /// Radio con cota editable.
    Radius {
        center: PointId,
        rim: PointId,
        dim: DimId,
    },
    Parallel {
        a: (PointId, PointId),
        b: (PointId, PointId),
    },
    Perpendicular {
        a: (PointId, PointId),
        b: (PointId, PointId),
    },
    /// Los dos segmentos miden lo mismo.
    EqualLength {
        a: (PointId, PointId),
        b: (PointId, PointId),
    },
    /// Ángulo entre dos segmentos, con cota editable.
    Angle {
        a: (PointId, PointId),
        b: (PointId, PointId),
        dim: DimId,
    },
    /// Simetría respecto del segmento `axis`.
    Symmetric {
        a: PointId,
        b: PointId,
        axis: (PointId, PointId),
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SketchModel {
    /// Posiciones iniciales. El solver las usa como semilla, así que determinan
    /// a qué rama de la solución converge: un sistema de restricciones bien
    /// planteado suele tener varias, y la semilla es lo que elige la que el
    /// usuario dibujó.
    pub points: Vec<DVec2>,
    pub entities: Vec<SketchEntity>,
    pub constraints: Vec<Constraint>,
    /// Valor de cada cota, por `DimId`.
    pub dimensions: Vec<f64>,
}

impl SketchModel {
    pub fn add_point(&mut self, p: DVec2) -> PointId {
        self.points.push(p);
        PointId(self.points.len() as u32 - 1)
    }

    pub fn add_dimension(&mut self, value: f64) -> DimId {
        self.dimensions.push(value);
        DimId(self.dimensions.len() as u32 - 1)
    }

    pub fn point(&self, id: PointId) -> Option<DVec2> {
        self.points.get(id.0 as usize).copied()
    }

    pub fn dimension(&self, id: DimId) -> Option<f64> {
        self.dimensions.get(id.0 as usize).copied()
    }

    pub fn set_dimension(&mut self, id: DimId, v: f64) -> bool {
        match self.dimensions.get_mut(id.0 as usize) {
            Some(d) => {
                *d = v;
                true
            }
            None => false,
        }
    }

    /// Grados de libertad brutos: `2·puntos − ecuaciones`. Es una cota, no un
    /// diagnóstico: no tiene en cuenta restricciones redundantes.
    pub fn nominal_dof(&self) -> i64 {
        let vars = 2 * self.points.len() as i64;
        let eqs: i64 = self
            .constraints
            .iter()
            .map(|c| c.equation_count() as i64)
            .sum();
        vars - eqs
    }
}

impl Constraint {
    /// Cuántas ecuaciones escalares aporta.
    pub fn equation_count(&self) -> usize {
        match self {
            Constraint::Coincident(..) => 2,
            Constraint::Fixed(_) => 2,
            Constraint::Horizontal(..) | Constraint::Vertical(..) => 1,
            Constraint::Distance { .. } => 1,
            Constraint::Radius { .. } => 1,
            Constraint::Parallel { .. } | Constraint::Perpendicular { .. } => 1,
            Constraint::EqualLength { .. } => 1,
            Constraint::Angle { .. } => 1,
            Constraint::Symmetric { .. } => 2,
        }
    }
}

/// Diagnóstico del solver.
///
/// El usuario no puede arreglar lo que no ve. Cada variante lleva lo que hace
/// falta para señalarlo en pantalla.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SolveStatus {
    /// Resuelto y completamente restringido.
    Ok,
    /// Resuelto, pero quedan grados de libertad. No es un error: es el estado
    /// normal de un sketch a medio hacer, y hay que mostrarlo sin alarmar.
    UnderConstrained { dof: usize },
    /// Hay restricciones en conflicto. Los índices son los de
    /// `SketchModel::constraints`, para poder pintarlas en rojo.
    OverConstrained { conflicting: Vec<usize> },
    /// El sistema es consistente pero el método no convergió.
    NoConvergence { residual: f64, iterations: u32 },
}

impl SolveStatus {
    /// Si las posiciones devueltas son utilizables.
    pub fn usable(&self) -> bool {
        matches!(self, SolveStatus::Ok | SolveStatus::UnderConstrained { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolveResult {
    pub positions: Vec<DVec2>,
    pub status: SolveStatus,
    pub residual: f64,
    pub iterations: u32,
}

pub trait SketchSolver: Send + Sync {
    fn name(&self) -> &'static str;
    fn solve(&self, sketch: &SketchModel) -> SolveResult;
}
