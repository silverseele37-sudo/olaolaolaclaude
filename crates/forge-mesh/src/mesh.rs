//! La malla editable y su adyacencia.
//!
//! # Por qué la adyacencia se deriva en vez de almacenarse
//!
//! Una estructura de medias aristas mantenida a mano a través de cada operación
//! es una fuente inagotable de corrupción silenciosa: un `twin` mal asignado no
//! falla donde se escribió, falla tres modificadores después. Aquí se guarda lo
//! mínimo —posiciones y bucles de cara— y la adyacencia se **construye** cuando
//! hace falta, en `O(E)`. Si se construye bien una vez, está bien siempre.
//!
//! Es la misma decisión que en `forge-kernel-stub::poly`, y por la misma razón.

use std::collections::HashMap;

use forge_doc::StableId;
use forge_math::{DVec2, DVec3};

use crate::{MeshError, Result};

/// Una cara: un bucle de índices a `positions`, en orden antihorario visto
/// desde fuera.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Face {
    pub verts: Vec<u32>,
}

/// De qué entidad del dominio exacto viene cada cara.
///
/// **Sin esto la frontera de dominio no existe** (ADR-0002, R3): es lo que
/// permite «selecciona esta cara del sólido y biséllala» y lo que hace que una
/// selección sobreviva a una edición paramétrica aguas arriba.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProvenanceMap {
    /// Un elemento por cara. `None` = procedencia perdida.
    pub face_origin: Vec<Option<StableId>>,
}

impl ProvenanceMap {
    pub fn con_capacidad(n: usize) -> Self {
        ProvenanceMap {
            face_origin: vec![None; n],
        }
    }

    /// Fracción de caras que conservan procedencia, de 0 a 1.
    ///
    /// Es la métrica de aceptación del pilar y la que delata a un modificador
    /// que no propaga: un `remap` que devuelve un mapa vacío compila igual de
    /// bien que uno correcto, así que la comprobación tiene que ser numérica.
    pub fn cobertura(&self) -> f64 {
        if self.face_origin.is_empty() {
            return 1.0;
        }
        let con = self.face_origin.iter().filter(|o| o.is_some()).count();
        con as f64 / self.face_origin.len() as f64
    }

    pub fn caras_de(&self, id: StableId) -> Vec<u32> {
        self.face_origin
            .iter()
            .enumerate()
            .filter(|(_, o)| **o == Some(id))
            .map(|(i, _)| i as u32)
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    pub faces: Vec<Face>,
    pub prov: ProvenanceMap,
}

impl Mesh {
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Aristas únicas, sin orientación.
    pub fn edge_count(&self) -> usize {
        let mut set = std::collections::BTreeSet::new();
        for f in &self.faces {
            let n = f.verts.len();
            for i in 0..n {
                let (a, b) = (f.verts[i], f.verts[(i + 1) % n]);
                set.insert((a.min(b), a.max(b)));
            }
        }
        set.len()
    }

    /// Característica de Euler `V − E + F`. Para una malla cerrada de género 0
    /// vale **2**, y es la respuesta conocida por excelencia de este dominio:
    /// una malla que la incumple está rota aunque se vea bien.
    pub fn euler(&self) -> i64 {
        self.vertex_count() as i64 - self.edge_count() as i64 + self.face_count() as i64
    }

    pub fn centroid_of(&self, f: &Face) -> DVec3 {
        let s: DVec3 = f.verts.iter().map(|&i| self.positions[i as usize]).sum();
        s / f.verts.len() as f64
    }

    /// Normal por el método de Newell: estable con caras no perfectamente
    /// planas, a diferencia del producto vectorial de dos aristas cualesquiera.
    pub fn normal_of(&self, f: &Face) -> DVec3 {
        let mut n = DVec3::ZERO;
        let m = f.verts.len();
        for i in 0..m {
            let p = self.positions[f.verts[i] as usize];
            let q = self.positions[f.verts[(i + 1) % m] as usize];
            n.x += (p.y - q.y) * (p.z + q.z);
            n.y += (p.z - q.z) * (p.x + q.x);
            n.z += (p.x - q.x) * (p.y + q.y);
        }
        if n.length_squared() < 1e-24 {
            DVec3::Z
        } else {
            n.normalize()
        }
    }

    pub fn area(&self) -> f64 {
        self.faces
            .iter()
            .map(|f| {
                let c = self.centroid_of(f);
                let m = f.verts.len();
                (0..m)
                    .map(|i| {
                        let a = self.positions[f.verts[i] as usize];
                        let b = self.positions[f.verts[(i + 1) % m] as usize];
                        (a - c).cross(b - c).length() * 0.5
                    })
                    .sum::<f64>()
            })
            .sum()
    }

    /// Volumen con signo por el teorema de la divergencia. Solo tiene sentido
    /// en una malla cerrada y orientada.
    pub fn signed_volume(&self) -> f64 {
        let mut v = 0.0;
        for f in &self.faces {
            let c = self.centroid_of(f);
            let m = f.verts.len();
            for i in 0..m {
                let a = self.positions[f.verts[i] as usize];
                let b = self.positions[f.verts[(i + 1) % m] as usize];
                v += c.dot(a.cross(b));
            }
        }
        v / 6.0
    }

    /// Invariantes estructurales.
    ///
    /// Se comprueba de verdad, no como formalidad: una malla corrupta produce
    /// fallos a distancia —tres modificadores más abajo— que son carísimos de
    /// diagnosticar desde el síntoma.
    pub fn validate(&self) -> Result<()> {
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(MeshError::Corrupta(
                "normales descuadradas con posiciones".into(),
            ));
        }
        if !self.uvs.is_empty() && self.uvs.len() != self.positions.len() {
            return Err(MeshError::Corrupta(
                "UVs descuadradas con posiciones".into(),
            ));
        }
        if self.prov.face_origin.len() != self.faces.len() {
            return Err(MeshError::Corrupta(format!(
                "el mapa de procedencia tiene {} entradas para {} caras",
                self.prov.face_origin.len(),
                self.faces.len()
            )));
        }
        let n = self.positions.len() as u32;
        for (i, f) in self.faces.iter().enumerate() {
            if f.verts.len() < 3 {
                return Err(MeshError::Corrupta(format!(
                    "la cara {i} tiene {} vertices",
                    f.verts.len()
                )));
            }
            if let Some(&mal) = f.verts.iter().find(|&&v| v >= n) {
                return Err(MeshError::Corrupta(format!(
                    "la cara {i} referencia el vertice {mal} de {n}"
                )));
            }
            let mut ordenados = f.verts.clone();
            ordenados.sort_unstable();
            let antes = ordenados.len();
            ordenados.dedup();
            if ordenados.len() != antes {
                return Err(MeshError::Corrupta(format!(
                    "la cara {i} repite un vertice en su bucle"
                )));
            }
        }
        // Media arista usada dos veces en el mismo sentido: la orientación es
        // incoherente y el volumen saldría mal sin que nada más lo delate.
        let mut vistas: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &self.faces {
            let m = f.verts.len();
            for i in 0..m {
                let k = (f.verts[i], f.verts[(i + 1) % m]);
                *vistas.entry(k).or_insert(0) += 1;
            }
        }
        if let Some((k, _)) = vistas.iter().find(|(_, &c)| c > 1) {
            return Err(MeshError::Corrupta(format!(
                "la media arista {k:?} aparece mas de una vez: orientacion incoherente"
            )));
        }
        Ok(())
    }
}

/// Adyacencia derivada. Se construye en `O(E)` y se tira.
pub struct Adjacency {
    /// `(a, b)` → índices de las caras que contienen esa arista.
    pub edge_faces: HashMap<(u32, u32), Vec<u32>>,
    /// Vértice → caras incidentes.
    pub vertex_faces: Vec<Vec<u32>>,
    /// Vértice → vecinos.
    pub vertex_neighbors: Vec<Vec<u32>>,
}

impl Adjacency {
    pub fn build(m: &Mesh) -> Self {
        let mut edge_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        let mut vertex_faces = vec![Vec::new(); m.positions.len()];
        let mut vecinos: Vec<std::collections::BTreeSet<u32>> =
            vec![Default::default(); m.positions.len()];

        for (fi, f) in m.faces.iter().enumerate() {
            let n = f.verts.len();
            for i in 0..n {
                let (a, b) = (f.verts[i], f.verts[(i + 1) % n]);
                edge_faces
                    .entry((a.min(b), a.max(b)))
                    .or_default()
                    .push(fi as u32);
                vertex_faces[a as usize].push(fi as u32);
                vecinos[a as usize].insert(b);
                vecinos[b as usize].insert(a);
            }
        }
        Adjacency {
            edge_faces,
            vertex_faces,
            vertex_neighbors: vecinos
                .into_iter()
                .map(|s| s.into_iter().collect())
                .collect(),
        }
    }

    pub fn es_borde(&self, a: u32, b: u32) -> bool {
        self.edge_faces.get(&(a.min(b), a.max(b))).map(|v| v.len()) == Some(1)
    }
}
