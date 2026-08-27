//! Nombrado persistente. **Este archivo es el riesgo R1 del proyecto.**
//!
//! El enunciado, sin adornos: el usuario selecciona una cara, aplica cinco
//! operaciones más, y luego edita una cota quince pasos más arriba. La topología
//! se regenera entera. ¿A qué apunta ahora aquella selección?
//!
//! # La estrategia, en dos capas y un fallo honesto
//!
//! **Capa 1 — genealogía ([`forge_doc::StableId`]).** Es la primaria y la que
//! resuelve la inmensa mayoría de los casos. Un `StableId` no es un índice: lo
//! genera el nodo que crea la geometría, con su propia semántica («cara lateral
//! nacida de la arista 3 del perfil»). Si el nodo sigue existiendo con el mismo
//! `FeatureId` y su semántica interna no cambió, el identificador **es
//! literalmente el mismo** y la búsqueda es una igualdad exacta. Cambiar una
//! cota, mover el sketch o alargar una extrusión caen todos aquí.
//!
//! **Capa 2 — firma geométrica ([`GeometrySignature`]).** Solo cuando la capa 1
//! falla: se suprimió un nodo intermedio, se reordenó la cadena, un booleano
//! partió la cara. Entonces se busca por centroide, normal y medida. El
//! resultado **nunca** es `Bound`: es [`forge_doc::Binding::Rebound`] con una
//! confianza. La distinción no es cosmética — la interfaz necesita poder pintar
//! de otro color una referencia que sobrevivió por parecido en vez de por
//! linaje, porque es la que hay que revisar.
//!
//! **Fallo — [`forge_doc::Binding::Broken`], y se reporta.** Si ninguna
//! candidata supera el umbral, o si dos empatan, la referencia se rompe. Es la
//! decisión de diseño más importante del archivo y va contra el instinto:
//! **nunca se re-vincula en silencio a la candidata más parecida.** Un modelo
//! plausible pero incorrecto es peor que un error visible, porque el usuario lo
//! descubre después de mecanizar la pieza (ADR-0002, §4 y R3).
//!
//! # Límites conocidos, dichos en voz alta
//!
//! - La firma cuantiza a `GeometrySignature::QUANTUM_MM`; dos caras coplanares
//!   del mismo área a menos de esa distancia son indistinguibles para la capa 2.
//!   Se resuelven como ambiguas, es decir, `Broken`.
//! - No hay seguimiento de **partición**: si un booleano parte una cara en tres
//!   trozos, la referencia original no se convierte en tres referencias. Se
//!   re-vincula al trozo dominante o se rompe. Resolverlo bien exige que el
//!   kernel emita `SplitFrom` con el orden de los trozos estabilizado, y el
//!   contrato lo prevé (`TopoProvenance::SplitFrom`) pero el stub no lo puebla.
//! - La confianza es una puntuación heurística comparable entre candidatas del
//!   mismo caso, **no** una probabilidad. No se debe umbralizar en la interfaz
//!   con más precisión que «alta / revísala».

use forge_doc::{Binding, FeatureId, StableId, TopoClass};
use forge_kernel_api::{GeometrySignature, TopoEntity, TopologySummary};
use forge_math::DVec3;
use serde::{Deserialize, Serialize};

/// Una referencia de usuario a una sub-entidad topológica.
///
/// Lleva **las dos capas a la vez**, y por eso se captura en el momento de la
/// selección y no se recalcula: el `StableId` es el linaje y la firma es la
/// fotografía geométrica de aquel instante. Guardar solo el `StableId` deja sin
/// respaldo el día que la topología cambia; guardar solo la firma tira el linaje,
/// que es lo único exacto que hay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopoRef {
    /// Nodo del árbol sobre cuyo resultado se hizo la selección.
    pub sobre: FeatureId,
    /// Capa 1.
    pub objetivo: StableId,
    /// Capa 2, capturada al seleccionar.
    pub firma: GeometrySignature,
}

impl TopoRef {
    /// Captura una referencia a partir de una entidad viva de la topología.
    pub fn capturar(sobre: FeatureId, e: &TopoEntity) -> Self {
        TopoRef { sobre, objetivo: e.id, firma: e.signature }
    }

    pub fn clase(&self) -> TopoClass {
        self.objetivo.class
    }
}

/// Cómo se resolvió una referencia, con lo que hace falta para explicárselo al
/// usuario.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolucion {
    pub referencia: TopoRef,
    pub binding: Binding<StableId>,
    /// Mejor puntuación de la capa 2, cuando se llegó a usar.
    pub puntuacion: Option<f64>,
    /// Segunda mejor. Es lo que decide si hubo ambigüedad, y por eso se guarda:
    /// «rota por ambigua» y «rota por no encontrada» son dos problemas distintos
    /// para el usuario.
    pub segunda: Option<f64>,
    /// Por qué salió lo que salió, en una línea, en español.
    pub motivo: &'static str,
}

impl Resolucion {
    pub fn valor(&self) -> Option<StableId> {
        self.binding.value()
    }
    pub fn rota(&self) -> bool {
        self.binding.is_broken()
    }
    /// `true` solo si se resolvió por genealogía exacta.
    pub fn exacta(&self) -> bool {
        matches!(self.binding, Binding::Bound(_))
    }
}

/// Umbrales de la capa 2.
///
/// Están expuestos porque son la perilla que decide el compromiso entre
/// «re-vincula poco» y «re-vincula mal», y esa decisión debe ser visible y
/// medible, no una constante escondida en una función.
#[derive(Clone, Copy, Debug)]
pub struct Resolver {
    /// Puntuación mínima para aceptar una candidata. Por debajo: rota.
    pub umbral: f64,
    /// Distancia mínima entre la mejor y la segunda. Por debajo: **ambigua**,
    /// y ambigua es rota. Esta es la constante que impide re-vincular «a la más
    /// parecida» cuando hay dos igual de parecidas.
    pub margen: f64,
    /// Coseno mínimo entre normales para siquiera considerar una candidata.
    /// Una cara que mira a otro lado no es la misma cara, por mucho que su área
    /// coincida.
    pub coseno_minimo: f64,
}

impl Default for Resolver {
    fn default() -> Self {
        Resolver { umbral: 0.60, margen: 0.15, coseno_minimo: 0.90 }
    }
}

/// Des-cuantiza el centroide de una firma a milímetros.
fn centroide(s: &GeometrySignature) -> DVec3 {
    let q = GeometrySignature::QUANTUM_MM;
    DVec3::new(
        s.centroid_q[0] as f64 * q,
        s.centroid_q[1] as f64 * q,
        s.centroid_q[2] as f64 * q,
    )
}

/// Des-cuantiza la normal. No se re-normaliza a propósito: si el kernel guardó
/// una normal no unitaria, es información y no un error a tapar.
fn normal(s: &GeometrySignature) -> DVec3 {
    DVec3::new(s.normal_q[0] as f64, s.normal_q[1] as f64, s.normal_q[2] as f64) / 1000.0
}

fn medida(s: &GeometrySignature) -> f64 {
    s.measure_q as f64 * GeometrySignature::QUANTUM_MM
}

impl Resolver {
    /// Parecido entre dos firmas, en `[0, 1]`. `None` si son incomparables.
    ///
    /// Los pesos: la normal manda (una cara que mira a otro lado no es la
    /// misma), la medida sigue (el área cambia poco ante una edición típica de
    /// cota), y el centroide pesa lo menos porque es justo lo que una edición
    /// aguas arriba desplaza.
    pub fn parecido(&self, a: &GeometrySignature, b: &GeometrySignature) -> Option<f64> {
        if a.class != b.class {
            return None;
        }
        let (na, nb) = (normal(a), normal(b));
        let cos = if na.length_squared() < 1e-12 || nb.length_squared() < 1e-12 {
            // Sin normal utilizable (un vértice) no se puede filtrar por
            // orientación: se le da paso neutro en vez de descartar.
            1.0
        } else {
            na.normalize().dot(nb.normalize())
        };
        // Para aristas la dirección no tiene sentido orientado: una arista
        // recorrida al revés es la misma arista.
        let cos = if a.class == TopoClass::Edge { cos.abs() } else { cos };
        if cos < self.coseno_minimo {
            return None;
        }
        let s_normal = ((cos - self.coseno_minimo) / (1.0 - self.coseno_minimo)).clamp(0.0, 1.0);

        let (ma, mb) = (medida(a).abs(), medida(b).abs());
        let s_medida = if ma.max(mb) < 1e-12 {
            1.0
        } else {
            (1.0 - (ma - mb).abs() / ma.max(mb)).clamp(0.0, 1.0)
        };

        // Escala de longitud propia de la entidad, para que el término de
        // posición sea adimensional: una cara de 1 mm² y otra de 10 000 mm² no
        // pueden compartir «a cuántos milímetros deja de parecerse».
        let escala = match a.class {
            TopoClass::Face => ma.sqrt().max(1.0),
            TopoClass::Edge => ma.max(1.0),
            TopoClass::Vertex => 1.0,
        };
        let d = (centroide(a) - centroide(b)).length();
        let s_pos = 1.0 / (1.0 + d / escala);

        Some(0.45 * s_normal + 0.35 * s_medida + 0.20 * s_pos)
    }

    /// Resuelve una referencia contra una topología recién calculada.
    pub fn resolver(&self, r: &TopoRef, topo: &TopologySummary) -> Resolucion {
        let lista: &[TopoEntity] = match r.clase() {
            TopoClass::Face => &topo.faces,
            TopoClass::Edge => &topo.edges,
            TopoClass::Vertex => &topo.vertices,
        };

        // --- capa 1: genealogía ---
        if lista.iter().any(|e| e.id == r.objetivo) {
            return Resolucion {
                referencia: *r,
                binding: Binding::Bound(r.objetivo),
                puntuacion: None,
                segunda: None,
                motivo: "el identificador de genealogia sigue existiendo",
            };
        }

        // --- capa 2: firma geométrica ---
        let mut mejor: Option<(f64, StableId)> = None;
        let mut segunda: Option<f64> = None;
        for e in lista {
            let Some(p) = self.parecido(&r.firma, &e.signature) else { continue };
            match mejor {
                Some((m, _)) if p <= m => {
                    if segunda.map(|s| p > s).unwrap_or(true) {
                        segunda = Some(p);
                    }
                }
                Some((m, _)) => {
                    segunda = Some(m);
                    mejor = Some((p, e.id));
                }
                None => mejor = Some((p, e.id)),
            }
        }

        let Some((p, id)) = mejor else {
            return Resolucion {
                referencia: *r,
                binding: Binding::Broken,
                puntuacion: None,
                segunda: None,
                motivo: "ninguna candidata comparable: ni linaje ni firma",
            };
        };

        if p < self.umbral {
            return Resolucion {
                referencia: *r,
                binding: Binding::Broken,
                puntuacion: Some(p),
                segunda,
                motivo: "la mejor candidata no llega al umbral de parecido",
            };
        }
        if let Some(s) = segunda {
            if p - s < self.margen {
                // El caso que justifica todo el archivo: hay candidata, es
                // buena, y **hay otra igual de buena**. Elegir una sería
                // inventarse la intención del usuario.
                return Resolucion {
                    referencia: *r,
                    binding: Binding::Broken,
                    puntuacion: Some(p),
                    segunda,
                    motivo: "dos candidatas empatan: ambigua, y ambigua es rota",
                };
            }
        }

        Resolucion {
            referencia: *r,
            binding: Binding::Rebound { value: id, confidence: p as f32 },
            puntuacion: Some(p),
            segunda,
            motivo: "re-vinculada por firma geometrica; revisala",
        }
    }

    /// Resuelve una lista y devuelve además cuántas quedaron rotas.
    pub fn resolver_todas(&self, refs: &[TopoRef], topo: &TopologySummary) -> (Vec<Resolucion>, usize) {
        let v: Vec<Resolucion> = refs.iter().map(|r| self.resolver(r, topo)).collect();
        let rotas = v.iter().filter(|r| r.rota()).count();
        (v, rotas)
    }
}

/// Tasa de re-vinculación de un conjunto de referencias: fracción que sigue
/// apuntando a algo. Es la métrica de aceptación de ADR-0002 §6 (≥ 0,95), y
/// existe como función para que un test pueda **medirla** en vez de afirmarla.
pub fn tasa_de_revinculacion(res: &[Resolucion]) -> f64 {
    if res.is_empty() {
        return 1.0;
    }
    res.iter().filter(|r| !r.rota()).count() as f64 / res.len() as f64
}
