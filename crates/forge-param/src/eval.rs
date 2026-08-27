//! Evaluación perezosa del grafo, con caché por hash de contenido.
//!
//! # La clave de caché
//!
//! `clave = hash(identidad, tipo, parámetros, referencias, claves de las
//! entradas)`. Como la clave de una entrada es a su vez su contenido, la clave
//! de un nodo resume **todo el subárbol** que hay por encima de él. Consecuencia
//! directa y buscada: si al re-evaluar la clave coincide, no hay nada que
//! recalcular ni aguas arriba ni en el propio nodo. Editar un parámetro que no
//! altera la geometría —renombrar un nodo, mover una cota que ninguna
//! restricción lee— no invalida nada.
//!
//! ## Por qué la identidad del nodo entra en la clave
//!
//! Parece contradecir «caché de contenido», y no lo hace: los `StableId` de la
//! geometría **llevan dentro el `FeatureId` del nodo que la creó**. Dos nodos
//! con parámetros idénticos producen la misma forma pero con identidades
//! topológicas distintas, así que compartir la forma cacheada entre ellos
//! rompería el nombrado persistente de uno de los dos. La identidad es parte del
//! contenido observable.
//!
//! # Pureza
//!
//! `evaluar` es función de `(árbol, kernel)`. No hay reloj, ni contador global,
//! ni orden de iteración de un `HashMap` que se filtre al resultado. Si un nodo
//! leyese estado externo, la caché por contenido dejaría de ser correcta —
//! devolvería un resultado viejo para una entrada que «cambió» sin que su hash
//! cambiara— y el documento dejaría de ser reproducible.

use std::collections::{BTreeMap, HashMap};

use forge_doc::{FeatureId, StableId};
use forge_kernel_api::{
    ChamferSpec, ExtrudeOpts, FilletSpec, GeometryKernel, RevolveOpts, ShapeId, SketchSolver,
    TopologySummary,
};
use forge_math::{DVec2, DVec3};

use crate::hash::Hasher;
use crate::naming::{Resolucion, Resolver, TopoRef};
use crate::solver::GaussNewtonSolver;
use crate::tree::{FeatureTree, NodeKind, SketchNode};
use crate::{ParamError, Result};

/// Contadores. Existen para que los tests puedan **medir** que la caché
/// funciona: «no recalcula» es una afirmación que solo significa algo si hay un
/// número que la contradiga cuando se rompe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvalStats {
    /// Operaciones **constructivas** del kernel (las caras).
    pub llamadas_kernel: usize,
    /// Consultas al kernel (`topology`), que son baratas pero no gratis.
    pub consultas_kernel: usize,
    /// Nodos que ejecutaron su cuerpo.
    pub nodos_calculados: usize,
    /// Nodos que salieron de la caché sin tocar el kernel.
    pub aciertos_cache: usize,
    /// Invocaciones al solver 2D.
    pub llamadas_solver: usize,
}

/// Resultado de un nodo.
#[derive(Clone, Debug)]
pub struct NodeOutput {
    pub shape: Option<ShapeId>,
    /// Clave de contenido. Igual clave ⇒ igual geometría.
    pub clave: u64,
    pub desde_cache: bool,
    /// Vacío si el nodo no tiene referencias, o si vino de caché (nada que
    /// re-resolver: la resolución es determinista dada la misma clave, y se
    /// guarda con ella).
    pub resoluciones: Vec<Resolucion>,
}

/// Resultado de evaluar el árbol entero.
#[derive(Clone, Debug)]
pub struct EvalOutcome {
    pub salidas: BTreeMap<FeatureId, NodeOutput>,
    pub orden: Vec<FeatureId>,
    pub stats: EvalStats,
}

impl EvalOutcome {
    pub fn shape(&self, id: FeatureId) -> Option<ShapeId> {
        self.salidas.get(&id).and_then(|s| s.shape)
    }
    pub fn clave(&self, id: FeatureId) -> Option<u64> {
        self.salidas.get(&id).map(|s| s.clave)
    }
    /// Última salida del orden topológico con forma: lo que el usuario ve.
    pub fn raiz(&self) -> Option<ShapeId> {
        self.orden.iter().rev().find_map(|id| self.shape(*id))
    }
    /// Todas las re-vinculaciones de esta evaluación, para pintarlas.
    pub fn resoluciones(&self) -> Vec<(FeatureId, &Resolucion)> {
        let mut v = Vec::new();
        for (id, s) in &self.salidas {
            for r in &s.resoluciones {
                v.push((*id, r));
            }
        }
        v
    }
}

#[derive(Clone, Debug)]
struct Entrada {
    shape: Option<ShapeId>,
    resoluciones: Vec<Resolucion>,
}

/// El evaluador.
///
/// Dueño de la caché, no del kernel: el kernel puede vivir en otro proceso y la
/// caché es local.
pub struct Evaluator<'k> {
    kernel: &'k dyn GeometryKernel,
    solver: Box<dyn SketchSolver>,
    resolver: Resolver,
    cache: HashMap<u64, Entrada>,
    stats: EvalStats,
}

impl<'k> Evaluator<'k> {
    pub fn new(kernel: &'k dyn GeometryKernel) -> Self {
        Evaluator {
            kernel,
            solver: Box::new(GaussNewtonSolver::default()),
            resolver: Resolver::default(),
            cache: HashMap::new(),
            stats: EvalStats::default(),
        }
    }

    pub fn con_solver(mut self, s: Box<dyn SketchSolver>) -> Self {
        self.solver = s;
        self
    }

    pub fn con_resolver(mut self, r: Resolver) -> Self {
        self.resolver = r;
        self
    }

    pub fn stats(&self) -> EvalStats {
        self.stats
    }

    pub fn reiniciar_stats(&mut self) {
        self.stats = EvalStats::default();
    }

    pub fn entradas_en_cache(&self) -> usize {
        self.cache.len()
    }

    /// Vacía la caché. La geometría cacheada sigue viva en el kernel: liberarla
    /// aquí sería liberar formas que el llamante quizá aún esté mirando.
    pub fn limpiar_cache(&mut self) {
        self.cache.clear();
    }

    /// Topología de una forma, contando la consulta.
    pub fn topologia(&mut self, s: ShapeId) -> Result<TopologySummary> {
        self.stats.consultas_kernel += 1;
        Ok(self.kernel.topology(s)?)
    }

    /// Evalúa el árbol completo.
    pub fn evaluar(&mut self, arbol: &FeatureTree) -> Result<EvalOutcome> {
        let orden = arbol.orden_topologico()?;
        let mut salidas: BTreeMap<FeatureId, NodeOutput> = BTreeMap::new();
        // Perfiles resueltos de esta pasada: el solver se invoca una sola vez
        // por sketch, y su salida sirve a la vez para la clave y para el kernel.
        let mut perfiles: HashMap<FeatureId, Vec<DVec2>> = HashMap::new();

        for id in &orden {
            let nodo = arbol.nodo(*id).ok_or(ParamError::NodoDesconocido(*id))?;

            // --- suprimido: pasa de largo, con los parámetros intactos ---
            if nodo.suprimido {
                let entrada = nodo
                    .kind
                    .entradas()
                    .first()
                    .copied()
                    .ok_or(ParamError::SuprimidoSinEntrada(*id))?;
                let prev = salidas
                    .get(&entrada)
                    .ok_or(ParamError::NodoDesconocido(entrada))?;
                salidas.insert(
                    *id,
                    NodeOutput {
                        shape: prev.shape,
                        clave: prev.clave,
                        desde_cache: true,
                        resoluciones: Vec::new(),
                    },
                );
                continue;
            }

            // --- clave de contenido ---
            let mut h = Hasher::new();
            h.feature(*id).texto(nodo.kind.etiqueta());
            let clave_params = match &nodo.kind {
                // El sketch se hashea por su **perfil resuelto**, no por sus
                // parámetros: una cota que ninguna restricción lee no cambia la
                // geometría y no debe invalidar nada aguas abajo.
                NodeKind::Sketch(s) => {
                    let pts = self.perfil_2d(*id, s)?;
                    let afin = s.plano.a_afin();
                    let mut hs = Hasher::new();
                    hs.usize(pts.len());
                    for p in &pts {
                        hs.vec3(afin.transform_point3(DVec3::new(p.x, p.y, 0.0)));
                    }
                    perfiles.insert(*id, pts);
                    hs.valor()
                }
                otro => otro.params_hash(),
            };
            h.u64(clave_params);
            for r in nodo.kind.referencias() {
                h.estable(r.objetivo).firma(&r.firma);
            }
            for e in nodo.kind.entradas() {
                let prev = salidas.get(&e).ok_or(ParamError::NodoDesconocido(e))?;
                h.u64(prev.clave);
            }
            let clave = h.valor();

            if let Some(hit) = self.cache.get(&clave) {
                self.stats.aciertos_cache += 1;
                salidas.insert(
                    *id,
                    NodeOutput {
                        shape: hit.shape,
                        clave,
                        desde_cache: true,
                        resoluciones: hit.resoluciones.clone(),
                    },
                );
                continue;
            }

            self.stats.nodos_calculados += 1;
            let (shape, resoluciones) =
                self.calcular(*id, nodo.kind.clone(), &salidas, &perfiles)?;
            self.cache.insert(
                clave,
                Entrada {
                    shape,
                    resoluciones: resoluciones.clone(),
                },
            );
            salidas.insert(
                *id,
                NodeOutput {
                    shape,
                    clave,
                    desde_cache: false,
                    resoluciones,
                },
            );
        }

        Ok(EvalOutcome {
            salidas,
            orden,
            stats: self.stats,
        })
    }

    /// Resuelve el sketch y devuelve el perfil en coordenadas del plano.
    fn perfil_2d(&mut self, id: FeatureId, s: &SketchNode) -> Result<Vec<DVec2>> {
        if s.perfil.len() < 3 {
            return Err(ParamError::SketchInvalido(
                id,
                format!("el perfil tiene {} puntos; hacen falta 3", s.perfil.len()),
            ));
        }
        self.stats.llamadas_solver += 1;
        let r = self.solver.solve(&s.modelo);
        if !r.status.usable() {
            return Err(ParamError::SketchInvalido(id, format!("{:?}", r.status)));
        }
        let mut out = Vec::with_capacity(s.perfil.len());
        for p in &s.perfil {
            out.push(r.positions.get(p.0 as usize).copied().ok_or_else(|| {
                ParamError::SketchInvalido(
                    id,
                    format!("el perfil cita el punto {} inexistente", p.0),
                )
            })?);
        }
        Ok(out)
    }

    fn shape_de(&self, salidas: &BTreeMap<FeatureId, NodeOutput>, e: FeatureId) -> Result<ShapeId> {
        salidas
            .get(&e)
            .and_then(|s| s.shape)
            .ok_or(ParamError::SuprimidoSinEntrada(e))
    }

    /// Resuelve las referencias de un nodo contra la topología de su entrada.
    ///
    /// Aquí es donde el nombrado persistente deja de ser teoría: si un fillet no
    /// encuentra su arista, el árbol **no se evalúa a medias con la arista de al
    /// lado**. Se para y se dice.
    fn revincular(
        &mut self,
        id: FeatureId,
        entrada: ShapeId,
        refs: &[TopoRef],
    ) -> Result<(Vec<StableId>, Vec<Resolucion>)> {
        let topo = self.topologia(entrada)?;
        let (res, rotas) = self.resolver.resolver_todas(refs, &topo);
        if rotas > 0 {
            return Err(ParamError::ReferenciaRota {
                nodo: id,
                rotas,
                total: refs.len(),
            });
        }
        let ids = res.iter().filter_map(|r| r.valor()).collect();
        Ok((ids, res))
    }

    fn calcular(
        &mut self,
        id: FeatureId,
        kind: NodeKind,
        salidas: &BTreeMap<FeatureId, NodeOutput>,
        perfiles: &HashMap<FeatureId, Vec<DVec2>>,
    ) -> Result<(Option<ShapeId>, Vec<Resolucion>)> {
        let k = self.kernel;
        match kind {
            NodeKind::Sketch(s) => {
                let pts = match perfiles.get(&id) {
                    Some(p) => p.clone(),
                    None => self.perfil_2d(id, &s)?,
                };
                self.stats.llamadas_kernel += 1;
                let perfil = k.profile_from_polygon(&pts, id)?;
                // Colocar el sketch en el espacio es una transformación rígida:
                // los `StableId` de las caras viajan intactos dentro del cuerpo,
                // que es justo lo que hace que mover un sketch no rompa ninguna
                // referencia aguas abajo.
                self.stats.llamadas_kernel += 1;
                let colocado = k.transform(perfil, &s.plano.a_afin(), id)?;
                k.release(perfil);
                Ok((Some(colocado), Vec::new()))
            }
            NodeKind::Extrude {
                perfil,
                direccion,
                distancia_mm,
                simetrico,
            } => {
                let p = self.shape_de(salidas, perfil)?;
                self.stats.llamadas_kernel += 1;
                let s = k.extrude(
                    p,
                    ExtrudeOpts {
                        direction: direccion,
                        distance_mm: distancia_mm,
                        symmetric: simetrico,
                    },
                    id,
                )?;
                Ok((Some(s), Vec::new()))
            }
            NodeKind::Revolve {
                perfil,
                eje_origen,
                eje_dir,
                angulo_rad,
            } => {
                let p = self.shape_de(salidas, perfil)?;
                self.stats.llamadas_kernel += 1;
                let s = k.revolve(
                    p,
                    RevolveOpts {
                        axis_origin: eje_origen,
                        axis_dir: eje_dir,
                        angle_rad: angulo_rad,
                    },
                    id,
                )?;
                Ok((Some(s), Vec::new()))
            }
            NodeKind::BoxPrimitive { min, max } => {
                self.stats.llamadas_kernel += 1;
                Ok((Some(k.box_solid(min, max, id)?), Vec::new()))
            }
            NodeKind::Cylinder {
                base,
                eje,
                radio_mm,
                altura_mm,
            } => {
                self.stats.llamadas_kernel += 1;
                Ok((
                    Some(k.cylinder(base, eje, radio_mm, altura_mm, id)?),
                    Vec::new(),
                ))
            }
            NodeKind::Fillet {
                entrada,
                aristas,
                radio_mm,
            } => {
                let e = self.shape_de(salidas, entrada)?;
                let (ids, res) = self.revincular(id, e, &aristas)?;
                self.stats.llamadas_kernel += 1;
                let s = k.fillet(
                    e,
                    &ids,
                    FilletSpec::Constant {
                        radius_mm: radio_mm,
                    },
                    id,
                )?;
                Ok((Some(s), res))
            }
            NodeKind::Chamfer {
                entrada,
                aristas,
                distancia_mm,
            } => {
                let e = self.shape_de(salidas, entrada)?;
                let (ids, res) = self.revincular(id, e, &aristas)?;
                self.stats.llamadas_kernel += 1;
                let s = k.chamfer(
                    e,
                    &ids,
                    ChamferSpec::Symmetric {
                        distance_mm: distancia_mm,
                    },
                    id,
                )?;
                Ok((Some(s), res))
            }
            NodeKind::Boolean { op, a, b } => {
                let (sa, sb) = (self.shape_de(salidas, a)?, self.shape_de(salidas, b)?);
                self.stats.llamadas_kernel += 1;
                Ok((Some(k.boolean(op, sa, sb, id)?), Vec::new()))
            }
            NodeKind::Transform { entrada, matriz } => {
                let e = self.shape_de(salidas, entrada)?;
                self.stats.llamadas_kernel += 1;
                Ok((Some(k.transform(e, &matriz, id)?), Vec::new()))
            }
        }
    }
}
