//! El árbol de features y el DAG que forma.
//!
//! «Árbol» es el nombre que ve el usuario —una lista ordenada de operaciones—
//! pero la estructura real es un **grafo dirigido acíclico**: un booleano tiene
//! dos entradas, y varios nodos pueden consumir el mismo sketch. Las dos vistas
//! conviven: [`FeatureTree::orden`] es lo que se dibuja, y las entradas
//! declaradas en cada [`NodeKind`] son lo que se evalúa.
//!
//! Que sea acíclico no es un detalle de implementación: es lo que permite un
//! orden topológico trivial, evaluación perezosa y reproducibilidad. En cuanto
//! se admite un ciclo, evaluar deja de ser recorrer un grafo y pasa a ser buscar
//! un punto fijo (ADR-0002, R3). Por eso el ciclo se **detecta y se reporta**,
//! nunca se intenta resolver, y la detección es iterativa (Kahn) para que un
//! grafo malicioso no desborde la pila.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use forge_doc::FeatureId;
use forge_kernel_api::sketch::{PointId, SketchModel};
use forge_kernel_api::BoolOp;
use forge_math::{DAffine3, DVec3};
use serde::{Deserialize, Serialize};

use crate::hash::Hasher;
use crate::naming::TopoRef;
use crate::{ParamError, Result};

/// Colocación del plano de un sketch en el espacio.
///
/// El sketch vive en `Z = 0` local (contrato de `forge-kernel-api::sketch`); es
/// este plano el que lo pone en el mundo. Separarlo así es lo que hace que mover
/// el sketch **no** toque ni una coordenada del modelo 2D, y por tanto no
/// invalide el trabajo de restricciones.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plano {
    pub origen: DVec3,
    pub eje_x: DVec3,
    pub eje_y: DVec3,
}

impl Default for Plano {
    /// Plano XY con Z arriba, que es el sistema del proyecto (diestro, Z arriba).
    fn default() -> Self {
        Plano { origen: DVec3::ZERO, eje_x: DVec3::X, eje_y: DVec3::Y }
    }
}

impl Plano {
    pub fn en_z(z: f64) -> Self {
        Plano { origen: DVec3::new(0.0, 0.0, z), ..Plano::default() }
    }

    /// Normal del plano. Diestra: `x × y`.
    pub fn normal(&self) -> DVec3 {
        self.eje_x.cross(self.eje_y).normalize_or_zero()
    }

    pub fn a_afin(&self) -> DAffine3 {
        DAffine3::from_cols(self.eje_x, self.eje_y, self.normal(), self.origen)
    }
}

/// Un sketch como nodo del árbol: el modelo con restricciones, qué puntos forman
/// el perfil cerrado, y dónde está el plano.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SketchNode {
    pub modelo: SketchModel,
    /// Bucle exterior cerrado, por índice de punto. El último se une al primero.
    pub perfil: Vec<PointId>,
    pub plano: Plano,
}

/// Los nodos que este pilar sabe evaluar.
///
/// Las entradas son `FeatureId` y no punteros ni índices: un índice se invalida
/// al reordenar y un puntero no se serializa.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NodeKind {
    Sketch(SketchNode),
    Extrude {
        perfil: FeatureId,
        direccion: DVec3,
        distancia_mm: f64,
        simetrico: bool,
    },
    Revolve {
        perfil: FeatureId,
        eje_origen: DVec3,
        eje_dir: DVec3,
        angulo_rad: f64,
    },
    BoxPrimitive {
        min: DVec3,
        max: DVec3,
    },
    Cylinder {
        base: DVec3,
        eje: DVec3,
        radio_mm: f64,
        altura_mm: f64,
    },
    Fillet {
        entrada: FeatureId,
        aristas: Vec<TopoRef>,
        radio_mm: f64,
    },
    Chamfer {
        entrada: FeatureId,
        aristas: Vec<TopoRef>,
        distancia_mm: f64,
    },
    Boolean {
        op: BoolOp,
        a: FeatureId,
        b: FeatureId,
    },
    Transform {
        entrada: FeatureId,
        matriz: DAffine3,
    },
}

impl NodeKind {
    /// Etiqueta estable del tipo. Entra en el hash de contenido, así que
    /// **no se puede cambiar** sin invalidar cachés persistidas.
    pub fn etiqueta(&self) -> &'static str {
        match self {
            NodeKind::Sketch(_) => "sketch",
            NodeKind::Extrude { .. } => "extrude",
            NodeKind::Revolve { .. } => "revolve",
            NodeKind::BoxPrimitive { .. } => "box",
            NodeKind::Cylinder { .. } => "cylinder",
            NodeKind::Fillet { .. } => "fillet",
            NodeKind::Chamfer { .. } => "chamfer",
            NodeKind::Boolean { .. } => "boolean",
            NodeKind::Transform { .. } => "transform",
        }
    }

    /// Entradas del nodo, en orden. La primera es la «entrada principal»: es la
    /// que hereda un nodo suprimido.
    pub fn entradas(&self) -> Vec<FeatureId> {
        match self {
            NodeKind::Sketch(_) | NodeKind::BoxPrimitive { .. } | NodeKind::Cylinder { .. } => {
                Vec::new()
            }
            NodeKind::Extrude { perfil, .. } | NodeKind::Revolve { perfil, .. } => vec![*perfil],
            NodeKind::Fillet { entrada, .. }
            | NodeKind::Chamfer { entrada, .. }
            | NodeKind::Transform { entrada, .. } => vec![*entrada],
            NodeKind::Boolean { a, b, .. } => vec![*a, *b],
        }
    }

    /// Referencias topológicas que el nodo tiene que re-vincular en cada
    /// evaluación. Es lo que convierte al fillet en el caso de prueba del
    /// nombrado persistente y no en una operación más.
    pub fn referencias(&self) -> &[TopoRef] {
        match self {
            NodeKind::Fillet { aristas, .. } | NodeKind::Chamfer { aristas, .. } => aristas,
            _ => &[],
        }
    }

    fn entradas_mut(&mut self) -> Vec<&mut FeatureId> {
        match self {
            NodeKind::Sketch(_) | NodeKind::BoxPrimitive { .. } | NodeKind::Cylinder { .. } => {
                Vec::new()
            }
            NodeKind::Extrude { perfil, .. } | NodeKind::Revolve { perfil, .. } => vec![perfil],
            NodeKind::Fillet { entrada, .. }
            | NodeKind::Chamfer { entrada, .. }
            | NodeKind::Transform { entrada, .. } => vec![entrada],
            NodeKind::Boolean { a, b, .. } => vec![a, b],
        }
    }

    /// Hash de los parámetros propios, **sin** las entradas.
    ///
    /// Para un sketch se hashea el modelo completo. Ojo con la sutileza: el
    /// evaluador **no** usa este valor para el sketch, sino el hash del perfil
    /// ya resuelto, porque una cota que ninguna restricción lee no cambia la
    /// geometría y no debe invalidar nada aguas abajo. Aquí queda por
    /// completitud y para comparar dos nodos como datos.
    pub fn params_hash(&self) -> u64 {
        let mut h = Hasher::new();
        h.texto(self.etiqueta());
        match self {
            NodeKind::Sketch(s) => {
                h.usize(s.modelo.points.len());
                for p in &s.modelo.points {
                    h.vec2(*p);
                }
                h.usize(s.modelo.dimensions.len());
                for d in &s.modelo.dimensions {
                    h.f64(*d);
                }
                h.usize(s.modelo.constraints.len());
                for c in &s.modelo.constraints {
                    hash_restriccion(&mut h, c);
                }
                h.usize(s.perfil.len());
                for p in &s.perfil {
                    h.u64(p.0 as u64);
                }
                h.vec3(s.plano.origen).vec3(s.plano.eje_x).vec3(s.plano.eje_y);
            }
            NodeKind::Extrude { direccion, distancia_mm, simetrico, .. } => {
                h.vec3(*direccion).f64(*distancia_mm).bool(*simetrico);
            }
            NodeKind::Revolve { eje_origen, eje_dir, angulo_rad, .. } => {
                h.vec3(*eje_origen).vec3(*eje_dir).f64(*angulo_rad);
            }
            NodeKind::BoxPrimitive { min, max } => {
                h.vec3(*min).vec3(*max);
            }
            NodeKind::Cylinder { base, eje, radio_mm, altura_mm } => {
                h.vec3(*base).vec3(*eje).f64(*radio_mm).f64(*altura_mm);
            }
            NodeKind::Fillet { radio_mm, .. } => {
                h.f64(*radio_mm);
            }
            NodeKind::Chamfer { distancia_mm, .. } => {
                h.f64(*distancia_mm);
            }
            NodeKind::Boolean { op, .. } => {
                h.byte(match op {
                    BoolOp::Union => 0,
                    BoolOp::Difference => 1,
                    BoolOp::Intersection => 2,
                });
            }
            NodeKind::Transform { matriz, .. } => {
                h.afin(matriz);
            }
        }
        h.valor()
    }
}

fn hash_restriccion(h: &mut Hasher, c: &forge_kernel_api::Constraint) {
    use forge_kernel_api::Constraint as C;
    let p = |h: &mut Hasher, x: PointId| {
        h.u64(x.0 as u64);
    };
    match c {
        C::Coincident(a, b) => {
            h.byte(1);
            p(h, *a);
            p(h, *b);
        }
        C::Fixed(a) => {
            h.byte(2);
            p(h, *a);
        }
        C::Horizontal(a, b) => {
            h.byte(3);
            p(h, *a);
            p(h, *b);
        }
        C::Vertical(a, b) => {
            h.byte(4);
            p(h, *a);
            p(h, *b);
        }
        C::Distance { a, b, dim } => {
            h.byte(5);
            p(h, *a);
            p(h, *b);
            h.u64(dim.0 as u64);
        }
        C::Radius { center, rim, dim } => {
            h.byte(6);
            p(h, *center);
            p(h, *rim);
            h.u64(dim.0 as u64);
        }
        C::Parallel { a, b } => {
            h.byte(7);
            p(h, a.0);
            p(h, a.1);
            p(h, b.0);
            p(h, b.1);
        }
        C::Perpendicular { a, b } => {
            h.byte(8);
            p(h, a.0);
            p(h, a.1);
            p(h, b.0);
            p(h, b.1);
        }
        C::EqualLength { a, b } => {
            h.byte(9);
            p(h, a.0);
            p(h, a.1);
            p(h, b.0);
            p(h, b.1);
        }
        C::Angle { a, b, dim } => {
            h.byte(10);
            p(h, a.0);
            p(h, a.1);
            p(h, b.0);
            p(h, b.1);
            h.u64(dim.0 as u64);
        }
        C::Symmetric { a, b, axis } => {
            h.byte(11);
            p(h, *a);
            p(h, *b);
            p(h, axis.0);
            p(h, axis.1);
        }
    }
}

/// Un nodo del árbol.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureNode {
    pub id: FeatureId,
    /// Nombre visible. **No entra en el hash de contenido**: renombrar un nodo
    /// no cambia su geometría, y por tanto no puede disparar un recálculo aguas
    /// abajo. Es la comprobación más simple de que la caché es de contenido y no
    /// de identidad.
    pub nombre: String,
    /// Suprimido: sigue en el árbol, con sus parámetros intactos, pero no se
    /// ejecuta. No es lo mismo que borrarlo (ADR-0004: el árbol es datos).
    pub suprimido: bool,
    pub kind: NodeKind,
}

impl FeatureNode {
    pub fn nuevo(nombre: impl Into<String>, kind: NodeKind) -> Self {
        FeatureNode { id: FeatureId::new(), nombre: nombre.into(), suprimido: false, kind }
    }

    /// Con identidad determinista, para tests y documentos generados.
    pub fn con_id(id: FeatureId, nombre: impl Into<String>, kind: NodeKind) -> Self {
        FeatureNode { id, nombre: nombre.into(), suprimido: false, kind }
    }
}

/// El árbol editable.
#[derive(Clone, Debug, Default)]
pub struct FeatureTree {
    nodos: HashMap<FeatureId, FeatureNode>,
    /// Orden que ve el usuario. La evaluación usa el orden topológico, que se
    /// deriva; este es el de presentación y el que `reordenar` mueve.
    orden: Vec<FeatureId>,
}

impl FeatureTree {
    pub fn new() -> Self {
        FeatureTree::default()
    }

    /// Construye sin validar nada.
    ///
    /// Existe para poder **fabricar un grafo con ciclo en un test**: si la única
    /// forma de crear un ciclo fuese una API que lo rechaza, no habría manera de
    /// comprobar que el evaluador lo detecta en vez de colgarse.
    pub fn desde_nodos(nodos: Vec<FeatureNode>) -> Self {
        let orden = nodos.iter().map(|n| n.id).collect();
        let mapa = nodos.into_iter().map(|n| (n.id, n)).collect();
        FeatureTree { nodos: mapa, orden }
    }

    pub fn len(&self) -> usize {
        self.orden.len()
    }
    pub fn is_empty(&self) -> bool {
        self.orden.is_empty()
    }
    pub fn orden(&self) -> &[FeatureId] {
        &self.orden
    }
    pub fn nodo(&self, id: FeatureId) -> Option<&FeatureNode> {
        self.nodos.get(&id)
    }
    pub fn nodo_mut(&mut self, id: FeatureId) -> Option<&mut FeatureNode> {
        self.nodos.get_mut(&id)
    }

    /// Índice de presentación de un nodo.
    pub fn indice(&self, id: FeatureId) -> Option<usize> {
        self.orden.iter().position(|x| *x == id)
    }

    /// Inserta al final. Devuelve el id.
    pub fn insertar(&mut self, nodo: FeatureNode) -> FeatureId {
        let id = nodo.id;
        self.nodos.insert(id, nodo);
        self.orden.push(id);
        id
    }

    /// Inserta en una posición concreta del orden de presentación.
    pub fn insertar_en(&mut self, indice: usize, nodo: FeatureNode) -> FeatureId {
        let id = nodo.id;
        self.nodos.insert(id, nodo);
        self.orden.insert(indice.min(self.orden.len()), id);
        id
    }

    /// Suprime o des-suprime **sin borrar**: los parámetros siguen ahí.
    pub fn suprimir(&mut self, id: FeatureId, valor: bool) -> Result<()> {
        self.nodos
            .get_mut(&id)
            .map(|n| n.suprimido = valor)
            .ok_or(ParamError::NodoDesconocido(id))
    }

    /// Quién referencia a `id`.
    pub fn dependientes(&self, id: FeatureId) -> Vec<FeatureId> {
        let mut v: Vec<FeatureId> = self
            .nodos
            .values()
            .filter(|n| n.kind.entradas().contains(&id))
            .map(|n| n.id)
            .collect();
        v.sort();
        v
    }

    /// Borra de verdad. Falla si alguien lo referencia: dejar entradas colgando
    /// convertiría el error en un fallo a distancia al evaluar, en vez de aquí,
    /// donde el usuario todavía sabe qué estaba haciendo.
    pub fn borrar(&mut self, id: FeatureId) -> Result<FeatureNode> {
        if !self.nodos.contains_key(&id) {
            return Err(ParamError::NodoDesconocido(id));
        }
        let deps = self.dependientes(id);
        if !deps.is_empty() {
            return Err(ParamError::TieneDependientes(id, deps));
        }
        self.orden.retain(|x| *x != id);
        self.nodos.remove(&id).ok_or(ParamError::NodoDesconocido(id))
    }

    /// Borra un nodo de una cadena re-cableando a sus dependientes hacia su
    /// entrada principal. Es lo que el usuario espera de «borrar el fillet del
    /// medio»: la cadena se cierra sola.
    pub fn borrar_de_la_cadena(&mut self, id: FeatureId) -> Result<FeatureNode> {
        let nodo = self.nodos.get(&id).ok_or(ParamError::NodoDesconocido(id))?;
        let sustituto = nodo.kind.entradas().first().copied();
        for dep in self.dependientes(id) {
            let Some(d) = self.nodos.get_mut(&dep) else { continue };
            for e in d.kind.entradas_mut() {
                if *e == id {
                    match sustituto {
                        Some(s) => *e = s,
                        None => return Err(ParamError::SuprimidoSinEntrada(id)),
                    }
                }
            }
        }
        self.borrar(id)
    }

    /// Mueve un nodo en el orden de presentación.
    ///
    /// **Solo mueve la vista.** El grafo lo definen las entradas declaradas, así
    /// que esto no puede cambiar la geometría; lo que sí puede es dejar el orden
    /// incoherente con el grafo, y eso se rechaza aquí en vez de producir un
    /// árbol que se dibuja al revés de como se evalúa.
    pub fn reordenar(&mut self, id: FeatureId, nuevo: usize) -> Result<()> {
        let i = self.indice(id).ok_or(ParamError::NodoDesconocido(id))?;
        let nuevo = nuevo.min(self.orden.len() - 1);
        let x = self.orden.remove(i);
        self.orden.insert(nuevo, x);
        if let Err(e) = self.comprobar_orden() {
            // Deshacer: un reordenamiento inválido no puede dejar el árbol peor
            // de como estaba.
            let j = self.indice(id).unwrap_or(nuevo);
            let x = self.orden.remove(j);
            self.orden.insert(i, x);
            return Err(e);
        }
        Ok(())
    }

    /// Intercambia dos nodos **consecutivos de una cadena**, re-cableando.
    ///
    /// Esto sí cambia la geometría, y es la reordenación que le importa al
    /// usuario: «aplica el chaflán antes que el redondeo». Es también el peor
    /// caso del nombrado persistente, porque cambia la genealogía de todo lo que
    /// hay debajo sin cambiar ni un parámetro.
    pub fn intercambiar_en_la_cadena(&mut self, primero: FeatureId, segundo: FeatureId) -> Result<()> {
        let n2 = self.nodos.get(&segundo).ok_or(ParamError::NodoDesconocido(segundo))?;
        if n2.kind.entradas().first() != Some(&primero) {
            return Err(ParamError::OrdenIncoherente { nodo: segundo, entrada: primero });
        }
        let n1 = self.nodos.get(&primero).ok_or(ParamError::NodoDesconocido(primero))?;
        let abuelo = match n1.kind.entradas().first() {
            Some(a) => *a,
            None => return Err(ParamError::SuprimidoSinEntrada(primero)),
        };

        // Los que colgaban de `segundo` pasan a colgar de `primero`.
        for dep in self.dependientes(segundo) {
            if dep == primero {
                continue;
            }
            if let Some(d) = self.nodos.get_mut(&dep) {
                for e in d.kind.entradas_mut() {
                    if *e == segundo {
                        *e = primero;
                    }
                }
            }
        }
        if let Some(n) = self.nodos.get_mut(&segundo) {
            if let Some(e) = n.kind.entradas_mut().into_iter().next() {
                *e = abuelo;
            }
        }
        if let Some(n) = self.nodos.get_mut(&primero) {
            if let Some(e) = n.kind.entradas_mut().into_iter().next() {
                *e = segundo;
            }
        }
        let (i, j) = (
            self.indice(primero).ok_or(ParamError::NodoDesconocido(primero))?,
            self.indice(segundo).ok_or(ParamError::NodoDesconocido(segundo))?,
        );
        self.orden.swap(i, j);
        Ok(())
    }

    /// El orden de presentación es coherente con el grafo.
    pub fn comprobar_orden(&self) -> Result<()> {
        let pos: BTreeMap<FeatureId, usize> =
            self.orden.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        for (i, id) in self.orden.iter().enumerate() {
            let Some(n) = self.nodos.get(id) else { continue };
            for e in n.kind.entradas() {
                if let Some(&j) = pos.get(&e) {
                    if j > i {
                        return Err(ParamError::OrdenIncoherente { nodo: *id, entrada: e });
                    }
                }
            }
        }
        Ok(())
    }

    /// Orden topológico por Kahn.
    ///
    /// Iterativo a propósito: un DFS recursivo sobre un grafo de miles de nodos
    /// desborda la pila, y desbordarla es un aborto del proceso, no un error que
    /// el usuario pueda ver. Si queda algún nodo sin emitir, hay ciclo, y los
    /// nodos que quedan **son** el ciclo (más lo que cuelga de él): se devuelven
    /// para poder señalarlos.
    pub fn orden_topologico(&self) -> Result<Vec<FeatureId>> {
        let mut grado: BTreeMap<FeatureId, usize> = BTreeMap::new();
        let mut salidas: BTreeMap<FeatureId, Vec<FeatureId>> = BTreeMap::new();
        for id in &self.orden {
            grado.entry(*id).or_insert(0);
            salidas.entry(*id).or_default();
        }
        for id in &self.orden {
            let Some(n) = self.nodos.get(id) else { continue };
            for e in n.kind.entradas() {
                if !self.nodos.contains_key(&e) {
                    return Err(ParamError::NodoDesconocido(e));
                }
                *grado.entry(*id).or_insert(0) += 1;
                salidas.entry(e).or_default().push(*id);
            }
        }

        // La cola se siembra en el orden de presentación: con ella, el orden
        // topológico de un árbol lineal coincide con lo que ve el usuario, que
        // es lo que hace legibles los mensajes de error.
        let mut cola: VecDeque<FeatureId> =
            self.orden.iter().copied().filter(|id| grado[id] == 0).collect();
        let mut salida = Vec::with_capacity(self.orden.len());
        while let Some(id) = cola.pop_front() {
            salida.push(id);
            for s in salidas.get(&id).cloned().unwrap_or_default() {
                let g = grado.entry(s).or_insert(0);
                *g -= 1;
                if *g == 0 {
                    cola.push_back(s);
                }
            }
        }

        if salida.len() != self.orden.len() {
            let emitidos: BTreeSet<FeatureId> = salida.into_iter().collect();
            let ciclo: Vec<FeatureId> =
                self.orden.iter().copied().filter(|id| !emitidos.contains(id)).collect();
            return Err(ParamError::Ciclo(ciclo));
        }
        Ok(salida)
    }
}
