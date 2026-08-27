//! Solver de restricciones 2D: Gauss-Newton amortiguado (Levenberg-Marquardt).
//!
//! # Lo difícil no es converger
//!
//! Converger sobre un sistema bien planteado es un ejercicio de libro. Lo que
//! decide si un solver de sketch sirve o no es el **diagnóstico**: un sketch a
//! medio hacer está sub-restringido casi siempre, y uno mal hecho está en
//! conflicto. Devolver «no pude» sin decir cuántos grados de libertad quedan ni
//! qué restricciones se pelean deja al usuario sin nada que hacer.
//!
//! Por eso las tres respuestas difíciles se calculan de verdad y no se estiman:
//!
//! - **`UnderConstrained { dof }`** sale del **rango del jacobiano**, no del
//!   recuento nominal `2·puntos − ecuaciones`. La diferencia importa: tres
//!   restricciones que aportan «3 ecuaciones» pueden aportar 2 de rango si una
//!   es consecuencia de las otras, y entonces el recuento nominal dice 0 grados
//!   de libertad mientras el sketch todavía se puede arrastrar por pantalla.
//!   `nominal_dof` está en el contrato precisamente etiquetado como cota.
//!
//! - **`OverConstrained { conflicting }`** sale del **espacio nulo por la
//!   izquierda** del jacobiano. Si existe `y` con `yᵀJ = 0` y `y·r ≠ 0`, esa
//!   combinación de ecuaciones es imposible de satisfacer *sea cual sea* el
//!   movimiento de los puntos: hay conflicto, y las componentes no nulas de `y`
//!   dicen exactamente **qué restricciones** lo forman. Esto es lo que permite
//!   pintarlas en rojo en vez de decir «algo va mal».
//!
//!   Y distingue el caso que un solver ingenuo confunde: una restricción
//!   redundante pero **consistente** también produce `yᵀJ = 0`, pero con
//!   `y·r = 0`. Eso no es un conflicto, es información repetida, y no se marca.
//!
//! - **`NoConvergence { residual, iterations }`** es el resto: el sistema es
//!   consistente en el linealizado pero el método se quedó atascado. Lleva el
//!   residuo para que la interfaz pueda decir «te falta 0,3 mm» en vez de nada.
//!
//! # Detalles numéricos que no son opcionales
//!
//! - **Jacobiano numérico** por diferencias centradas. Es más lento que uno
//!   analítico y no cambia el diagnóstico; se puede sustituir sin tocar nada más.
//! - **Escalado de filas** antes de la eliminación: una fila de ángulos (radianes,
//!   O(1)) y una de distancias (milímetros, O(100)) no son comparables, y sin
//!   normalizar, el rango numérico depende de las unidades del usuario.
//! - **Amortiguación** de Levenberg-Marquardt. Gauss-Newton puro diverge en
//!   cuanto el jacobiano está mal condicionado, que es el caso normal de un
//!   sketch con dos puntos casi coincidentes.

use forge_kernel_api::sketch::{
    Constraint, PointId, SketchModel, SketchSolver, SolveResult, SolveStatus,
};
use forge_math::DVec2;

/// Parámetros del método. Expuestos porque son la diferencia entre «no converge»
/// y «tarda un segundo», y esa decisión debe ser visible.
#[derive(Clone, Copy, Debug)]
pub struct GaussNewtonSolver {
    pub max_iteraciones: u32,
    /// Norma infinito del residuo por debajo de la cual se da por resuelto.
    pub tol_residuo: f64,
    /// Paso de la diferencia centrada, relativo a la magnitud de la variable.
    pub paso_derivada: f64,
    /// Amortiguación inicial de Levenberg-Marquardt.
    pub lambda0: f64,
    /// Tolerancia relativa para decidir el rango numérico.
    pub tol_rango: f64,
}

impl Default for GaussNewtonSolver {
    fn default() -> Self {
        GaussNewtonSolver {
            max_iteraciones: 200,
            tol_residuo: 1e-10,
            paso_derivada: 1e-7,
            lambda0: 1e-4,
            tol_rango: 1e-9,
        }
    }
}

/// Una ecuación escalar: de qué restricción salió y cuánto vale su residuo.
struct Fila {
    restriccion: usize,
}

impl GaussNewtonSolver {
    /// Residuos del sistema en la configuración `x` (pares `x,y` consecutivos).
    ///
    /// El orden de las filas es el orden de `constraints`, y cada restricción
    /// aporta exactamente `equation_count()` filas: el contrato lo promete y
    /// aquí se cumple, porque el mapa fila→restricción es lo que después
    /// convierte un vector del espacio nulo en una lista de índices que la
    /// interfaz pueda pintar.
    fn residuos(&self, m: &SketchModel, x: &[f64], r: &mut Vec<f64>) {
        r.clear();
        let p = |i: PointId| {
            let k = i.0 as usize * 2;
            DVec2::new(x.get(k).copied().unwrap_or(0.0), x.get(k + 1).copied().unwrap_or(0.0))
        };
        let dim = |d: forge_kernel_api::DimId| m.dimension(d).unwrap_or(0.0);
        // Dirección unitaria de un segmento, con guarda: un segmento degenerado
        // no puede aportar dirección, y devolver `NaN` envenenaría todo el
        // sistema en vez de dejar esa ecuación sin fuerza.
        let dir = |a: PointId, b: PointId| {
            let v = p(b) - p(a);
            if v.length_squared() < 1e-24 {
                DVec2::ZERO
            } else {
                v.normalize()
            }
        };

        for c in &m.constraints {
            match *c {
                Constraint::Coincident(a, b) => {
                    let d = p(b) - p(a);
                    r.push(d.x);
                    r.push(d.y);
                }
                Constraint::Fixed(a) => {
                    // El ancla es la posición **semilla**: es lo que el usuario
                    // clavó al dibujar.
                    let s = m.point(a).unwrap_or(DVec2::ZERO);
                    let d = p(a) - s;
                    r.push(d.x);
                    r.push(d.y);
                }
                Constraint::Horizontal(a, b) => r.push(p(b).y - p(a).y),
                Constraint::Vertical(a, b) => r.push(p(b).x - p(a).x),
                Constraint::Distance { a, b, dim: d } => {
                    r.push((p(b) - p(a)).length() - dim(d));
                }
                Constraint::Radius { center, rim, dim: d } => {
                    r.push((p(rim) - p(center)).length() - dim(d));
                }
                Constraint::Parallel { a, b } => {
                    let (u, v) = (dir(a.0, a.1), dir(b.0, b.1));
                    r.push(u.x * v.y - u.y * v.x);
                }
                Constraint::Perpendicular { a, b } => {
                    let (u, v) = (dir(a.0, a.1), dir(b.0, b.1));
                    r.push(u.dot(v));
                }
                Constraint::EqualLength { a, b } => {
                    r.push((p(a.1) - p(a.0)).length() - (p(b.1) - p(b.0)).length());
                }
                Constraint::Angle { a, b, dim: d } => {
                    let (u, v) = (dir(a.0, a.1), dir(b.0, b.1));
                    let ang = (u.x * v.y - u.y * v.x).atan2(u.dot(v));
                    r.push(ang - dim(d));
                }
                Constraint::Symmetric { a, b, axis } => {
                    let (o, e) = (p(axis.0), dir(axis.0, axis.1));
                    let medio = (p(a) + p(b)) * 0.5;
                    // 1) el punto medio cae sobre el eje
                    let w = medio - o;
                    r.push(e.x * w.y - e.y * w.x);
                    // 2) el segmento a-b es perpendicular al eje
                    r.push(e.dot(p(b) - p(a)));
                }
            }
        }
    }

    fn filas(m: &SketchModel) -> Vec<Fila> {
        let mut v = Vec::new();
        for (i, c) in m.constraints.iter().enumerate() {
            for _ in 0..c.equation_count() {
                v.push(Fila { restriccion: i });
            }
        }
        v
    }

    /// Jacobiano numérico por diferencias centradas, en fila mayor (`m × n`).
    fn jacobiano(&self, m: &SketchModel, x: &[f64], filas: usize) -> Vec<f64> {
        let n = x.len();
        let mut j = vec![0.0; filas * n];
        let mut xp = x.to_vec();
        let (mut rp, mut rm) = (Vec::new(), Vec::new());
        for c in 0..n {
            let h = self.paso_derivada * x[c].abs().max(1.0);
            xp[c] = x[c] + h;
            self.residuos(m, &xp, &mut rp);
            xp[c] = x[c] - h;
            self.residuos(m, &xp, &mut rm);
            xp[c] = x[c];
            for f in 0..filas {
                j[f * n + c] = (rp[f] - rm[f]) / (2.0 * h);
            }
        }
        j
    }
}

/// Resuelve `A z = b` por eliminación gaussiana con pivoteo parcial.
/// `None` si la matriz es singular al nivel de tolerancia dado.
fn resolver_denso(a: &mut [f64], b: &mut [f64], n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let mut piv = col;
        for f in col + 1..n {
            if a[f * n + col].abs() > a[piv * n + col].abs() {
                piv = f;
            }
        }
        if a[piv * n + col].abs() < 1e-14 {
            return None;
        }
        if piv != col {
            for c in 0..n {
                a.swap(col * n + c, piv * n + c);
            }
            b.swap(col, piv);
        }
        let d = a[col * n + col];
        for f in col + 1..n {
            let factor = a[f * n + col] / d;
            if factor == 0.0 {
                continue;
            }
            for c in col..n {
                a[f * n + c] -= factor * a[col * n + c];
            }
            b[f] -= factor * b[col];
        }
    }
    let mut z = vec![0.0; n];
    for f in (0..n).rev() {
        let mut s = b[f];
        for c in f + 1..n {
            s -= a[f * n + c] * z[c];
        }
        z[f] = s / a[f * n + f];
    }
    if z.iter().all(|v| v.is_finite()) {
        Some(z)
    } else {
        None
    }
}

/// Rango numérico y base del **espacio nulo por la izquierda**.
///
/// Devuelve `(rango, vectores y tales que yᵀJ ≈ 0)`. El truco es eliminar sobre
/// `[J | I]`: las filas que quedan a cero en la parte de `J` traen en la parte de
/// `I` la combinación exacta de filas originales que se anula. Eso es la
/// dependencia lineal, con nombres y coeficientes.
fn eliminacion(j: &[f64], m: usize, n: usize, tol: f64) -> (usize, Vec<Vec<f64>>) {
    // Escalado de filas: sin él el rango depende de si el usuario modela en
    // milímetros o en metros, porque una fila de ángulos y otra de longitudes no
    // son comparables en magnitud.
    let mut a = vec![0.0; m * n];
    let mut y = vec![0.0; m * m];
    for f in 0..m {
        let norma: f64 = (0..n).map(|c| j[f * n + c] * j[f * n + c]).sum::<f64>().sqrt();
        let s = if norma > 1e-300 { 1.0 / norma } else { 1.0 };
        for c in 0..n {
            a[f * n + c] = j[f * n + c] * s;
        }
        y[f * m + f] = s;
    }

    let mut fila = 0usize;
    for col in 0..n {
        if fila >= m {
            break;
        }
        let mut piv = None;
        let mut mejor = tol;
        for f in fila..m {
            if a[f * n + col].abs() > mejor {
                mejor = a[f * n + col].abs();
                piv = Some(f);
            }
        }
        let Some(piv) = piv else { continue };
        if piv != fila {
            for c in 0..n {
                a.swap(fila * n + c, piv * n + c);
            }
            for c in 0..m {
                y.swap(fila * m + c, piv * m + c);
            }
        }
        let d = a[fila * n + col];
        for f in 0..m {
            if f == fila {
                continue;
            }
            let factor = a[f * n + col] / d;
            if factor == 0.0 {
                continue;
            }
            for c in 0..n {
                a[f * n + c] -= factor * a[fila * n + c];
            }
            for c in 0..m {
                y[f * m + c] -= factor * y[fila * m + c];
            }
        }
        fila += 1;
    }

    let rango = fila;
    let mut nulos = Vec::new();
    for f in 0..m {
        if f < rango {
            continue;
        }
        // Fila nula en la parte de J: su fila en Y es un vector del espacio
        // nulo por la izquierda.
        let residual: f64 = (0..n).map(|c| a[f * n + c].abs()).fold(0.0, f64::max);
        if residual <= tol * 10.0 {
            nulos.push(y[f * m..f * m + m].to_vec());
        }
    }
    (rango, nulos)
}

impl SketchSolver for GaussNewtonSolver {
    fn name(&self) -> &'static str {
        "gauss-newton amortiguado"
    }

    fn solve(&self, sketch: &SketchModel) -> SolveResult {
        let n = sketch.points.len() * 2;
        let mut x: Vec<f64> = sketch.points.iter().flat_map(|p| [p.x, p.y]).collect();

        let filas = Self::filas(sketch);
        let m = filas.len();
        let posiciones = |x: &[f64]| -> Vec<DVec2> {
            x.chunks_exact(2).map(|c| DVec2::new(c[0], c[1])).collect()
        };

        if m == 0 || n == 0 {
            // Nada que resolver. El diagnóstico sigue siendo obligatorio: un
            // sketch sin restricciones tiene 2 grados de libertad por punto.
            let status = if n == 0 {
                SolveStatus::Ok
            } else {
                SolveStatus::UnderConstrained { dof: n }
            };
            return SolveResult { positions: posiciones(&x), status, residual: 0.0, iterations: 0 };
        }

        let mut r = Vec::with_capacity(m);
        self.residuos(sketch, &x, &mut r);
        let mut norma = r.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        let mut lambda = self.lambda0;
        let mut it = 0u32;

        while it < self.max_iteraciones && norma > self.tol_residuo {
            it += 1;
            let j = self.jacobiano(sketch, &x, m);

            // Ecuaciones normales amortiguadas: (JᵀJ + λI) dx = −Jᵀr.
            let mut ata = vec![0.0; n * n];
            let mut atb = vec![0.0; n];
            for f in 0..m {
                for a in 0..n {
                    let ja = j[f * n + a];
                    if ja == 0.0 {
                        continue;
                    }
                    atb[a] -= ja * r[f];
                    for b in 0..n {
                        ata[a * n + b] += ja * j[f * n + b];
                    }
                }
            }
            // Amortiguación de Marquardt: proporcional a la diagonal, no
            // constante, para que no dependa de la escala del modelo.
            let mut aceptado = false;
            for _ in 0..12 {
                let mut a = ata.clone();
                for d in 0..n {
                    a[d * n + d] += lambda * (ata[d * n + d].max(1e-12));
                }
                let mut b = atb.clone();
                let Some(dx) = resolver_denso(&mut a, &mut b, n) else {
                    lambda *= 10.0;
                    continue;
                };
                let xn: Vec<f64> = x.iter().zip(&dx).map(|(a, d)| a + d).collect();
                let mut rn = Vec::with_capacity(m);
                self.residuos(sketch, &xn, &mut rn);
                let nn = rn.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                if nn.is_finite() && nn < norma {
                    x = xn;
                    r = rn;
                    norma = nn;
                    lambda = (lambda * 0.3).max(1e-12);
                    aceptado = true;
                    break;
                }
                lambda *= 10.0;
            }
            if !aceptado {
                // Ni amortiguando mucho se mejora: o estamos en el mínimo de
                // mínimos cuadrados (sistema inconsistente) o el paso es ruido
                // numérico. En ambos casos seguir iterando no aporta nada.
                break;
            }
        }

        // --- diagnóstico, que es la parte que sirve para algo ---
        let j = self.jacobiano(sketch, &x, m);
        let (rango, nulos) = eliminacion(&j, m, n, self.tol_rango);

        // Residuo escalado igual que las filas, para que `y·r` sea comparable
        // con el vector `y` que salió de la eliminación escalada.
        let mut conflicto: Vec<usize> = Vec::new();
        for y in &nulos {
            let mut c = 0.0;
            for (f, yf) in y.iter().enumerate() {
                c += yf * r[f];
            }
            if c.abs() > 1e-7 {
                for (f, yf) in y.iter().enumerate() {
                    if yf.abs() > 1e-6 {
                        conflicto.push(filas[f].restriccion);
                    }
                }
            }
        }
        conflicto.sort_unstable();
        conflicto.dedup();

        let residual = r.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        let dof = n.saturating_sub(rango);
        let status = if !conflicto.is_empty() {
            SolveStatus::OverConstrained { conflicting: conflicto }
        } else if residual > self.tol_residuo.max(1e-8) {
            SolveStatus::NoConvergence { residual, iterations: it }
        } else if dof > 0 {
            SolveStatus::UnderConstrained { dof }
        } else {
            SolveStatus::Ok
        };

        SolveResult { positions: posiciones(&x), status, residual, iterations: it }
    }
}
