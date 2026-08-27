//! Pila de modificadores no destructivos.
//!
//! # La obligación que define este módulo
//!
//! Todo modificador debe propagar la procedencia a la geometría que crea. Un
//! bevel que subdivide una cara tiene que asignar procedencia a las caras
//! nuevas; una subdivisión que multiplica por cuatro el número de caras,
//! también.
//!
//! No se hace cumplir con una firma de trait, y es deliberado: un método
//! `remap_provenance` puede implementarse como un no-op y compilar
//! perfectamente. Se hace cumplir **numéricamente**, con
//! [`ProvenanceMap::cobertura`] y con [`verificar_propagacion`], que es lo que
//! un modificador roto no puede sortear.

use std::collections::HashMap;

use forge_math::DVec3;

use crate::mesh::{Adjacency, Face, Mesh, ProvenanceMap};
use crate::{soldar_conservando_procedencia, MeshError, Result};

pub trait Modifier: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Hash de los parámetros propios. Junto con el de la entrada forma la
    /// clave de caché del nodo: sin él no hay evaluación perezosa.
    fn params_hash(&self) -> u64;
    fn apply(&self, input: &Mesh) -> Result<Mesh>;
}

/// Comprueba que un modificador no perdió procedencia.
///
/// Es el control que convierte «propagá la procedencia» de una recomendación en
/// un requisito verificable. Se llama desde [`ModifierStack::apply`], así que
/// un modificador que no propague rompe el build de quien lo use, no de quien
/// lo escribió tres meses después.
pub fn verificar_propagacion(kind: &'static str, entrada: &Mesh, salida: &Mesh) -> Result<()> {
    // Si la entrada ya venía sin procedencia, no se le puede exigir a la salida.
    if entrada.prov.cobertura() < 1.0 {
        return Ok(());
    }
    let perdidas = salida
        .prov
        .face_origin
        .iter()
        .filter(|o| o.is_none())
        .count();
    if perdidas > 0 {
        return Err(MeshError::ProcedenciaPerdida(
            kind,
            perdidas,
            salida.faces.len(),
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct ModifierStack {
    modificadores: Vec<Box<dyn Modifier>>,
}

impl ModifierStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(mut self, m: impl Modifier + 'static) -> Self {
        self.modificadores.push(Box::new(m));
        self
    }

    pub fn len(&self) -> usize {
        self.modificadores.len()
    }
    pub fn is_empty(&self) -> bool {
        self.modificadores.is_empty()
    }

    /// Clave de caché de la pila entera.
    ///
    /// Mezcla el `kind()` de cada modificador además de su `params_hash()`. Sin
    /// el `kind()` dos modificadores **de tipos distintos** cuyos parámetros
    /// hashean igual dan la misma clave, y como esta clave decide si se
    /// reutiliza un resultado cacheado, el segundo se serviría con la malla del
    /// primero. `Triangulate` devuelve la constante `0x7A1` y `Weld` hashea un
    /// solo `f64`: no hace falta rebuscar mucho para que dos coincidan, y basta
    /// con que alguien de fuera implemente el trait -- es público -- para que
    /// sea trivial.
    ///
    /// El orden ya importaba (FNV es secuencial) y sigue importando.
    pub fn params_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mezclar = |bytes: &[u8]| {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        };
        for m in &self.modificadores {
            mezclar(m.kind().as_bytes());
            // Separador: sin él, kind "ab" con params X y kind "a" con params
            // "b"+X caerían en la misma secuencia de bytes.
            mezclar(&[0]);
            mezclar(&m.params_hash().to_le_bytes());
        }
        h
    }

    /// Aplica la pila. Valida la malla **y la propagación de procedencia**
    /// después de cada paso: así un fallo señala al modificador que lo causó y
    /// no a tres pasos más abajo.
    pub fn apply(&self, entrada: &Mesh) -> Result<Mesh> {
        let mut actual = entrada.clone();
        for m in &self.modificadores {
            let salida = m.apply(&actual)?;
            salida.validate()?;
            verificar_propagacion(m.kind(), &actual, &salida)?;
            actual = salida;
        }
        Ok(actual)
    }
}

fn hash_f64(vals: &[f64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in vals {
        for b in v.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

// ---------------------------------------------------------------------------
// Subdivisión de Catmull–Clark
// ---------------------------------------------------------------------------

/// Catmull–Clark, implementación propia en CPU.
///
/// No se integra OpenSubdiv a propósito: la subdivisión básica es de las pocas
/// cosas de este proyecto que sí son razonables de escribir —unos cientos de
/// líneas, bien documentada, fácil de testear con recuentos exactos— y evitar
/// una dependencia grande de C++ en el pilar de malla vale más que las
/// superficies límite y la evaluación en GPU, que llegarán cuando el
/// rendimiento las exija.
pub struct Subdivide {
    pub niveles: u32,
}

impl Subdivide {
    pub fn new(niveles: u32) -> Self {
        Subdivide { niveles }
    }

    fn un_nivel(&self, m: &Mesh) -> Result<Mesh> {
        let ady = Adjacency::build(m);
        let mut positions: Vec<DVec3> = Vec::new();

        // 1. Punto de cara: el centroide.
        let mut punto_cara = Vec::with_capacity(m.faces.len());
        for f in &m.faces {
            punto_cara.push(positions.len() as u32);
            positions.push(m.centroid_of(f));
        }

        // 2. Punto de arista: media de los dos extremos y los dos puntos de
        //    cara. En un borde libre no hay segunda cara, y la regla correcta
        //    es el punto medio: usar la media a secas encogeria el borde.
        let mut punto_arista: HashMap<(u32, u32), u32> = HashMap::new();
        for (&(a, b), caras) in &ady.edge_faces {
            let pa = m.positions[a as usize];
            let pb = m.positions[b as usize];
            let p = if caras.len() == 2 {
                let f0 = positions[punto_cara[caras[0] as usize] as usize];
                let f1 = positions[punto_cara[caras[1] as usize] as usize];
                (pa + pb + f0 + f1) * 0.25
            } else {
                (pa + pb) * 0.5
            };
            punto_arista.insert((a, b), positions.len() as u32);
            positions.push(p);
        }

        // 3. Punto de vértice.
        let mut punto_vert = Vec::with_capacity(m.positions.len());
        for v in 0..m.positions.len() as u32 {
            let p = m.positions[v as usize];
            let vecinos = &ady.vertex_neighbors[v as usize];
            let bordes: Vec<u32> = vecinos
                .iter()
                .copied()
                .filter(|&w| ady.es_borde(v, w))
                .collect();

            // Un vértice de borde bien formado tiene **exactamente** dos
            // aristas de borde, y no por convenio: el número de aristas de
            // borde en un vértice es siempre par. Cada cara incidente usa dos
            // de las aristas que salen de él, así que sumando caras por arista
            // se llega a `k = 2·(grado − caras)`. Dos es el caso del ciclo de
            // borde normal; cero es un vértice interior.
            //
            // Más de dos significa que ahí se tocan dos ciclos de borde
            // distintos: un pellizco no manifold. No existe «el borde» cuya
            // curva mantener, existen dos, y la regla de borde no está definida.
            // La versión anterior cogía `.take(2)` de una lista de vecinos cuyo
            // orden nadie fija, así que el resultado dependía del orden de
            // iteración —la peor clase de error, porque no es reproducible—.
            // Se trata como esquina y se deja quieto, que es la regla estándar
            // (la de OpenSubdiv) y lo único que no privilegia arbitrariamente a
            // uno de los dos bordes.
            //
            // Uno es imposible por la paridad de arriba mientras ninguna arista
            // tenga tres caras. Si las tiene, la malla ya estaba rota antes de
            // llegar aquí; se trata como esquina por la misma razón: dejar el
            // punto quieto no inventa una dirección.
            let nuevo = if bordes.len() == 2 {
                // Regla de borde: (v_prev + 6·v + v_next) / 8. Mantiene la
                // curva del borde en vez de arrastrarla hacia el interior.
                let s: DVec3 = bordes.iter().map(|&w| m.positions[w as usize]).sum();
                (p * 6.0 + s) / 8.0
            } else if !bordes.is_empty() {
                p
            } else {
                let caras = &ady.vertex_faces[v as usize];
                let n = vecinos.len() as f64;
                if n < 3.0 || caras.is_empty() {
                    p
                } else {
                    let f: DVec3 = caras
                        .iter()
                        .map(|&c| positions[punto_cara[c as usize] as usize])
                        .sum::<DVec3>()
                        / caras.len() as f64;
                    let r: DVec3 = vecinos
                        .iter()
                        .map(|&w| (p + m.positions[w as usize]) * 0.5)
                        .sum::<DVec3>()
                        / n;
                    (f + r * 2.0 + p * (n - 3.0)) / n
                }
            };
            punto_vert.push(positions.len() as u32);
            positions.push(nuevo);
        }

        // 4. Un cuadrilátero por vértice de cada cara original.
        let mut faces = Vec::with_capacity(m.faces.len() * 4);
        let mut prov = ProvenanceMap::default();
        let arista = |a: u32, b: u32| punto_arista[&(a.min(b), a.max(b))];

        for (fi, f) in m.faces.iter().enumerate() {
            let n = f.verts.len();
            for i in 0..n {
                let v = f.verts[i];
                let sig = f.verts[(i + 1) % n];
                let ant = f.verts[(i + n - 1) % n];
                faces.push(Face {
                    verts: vec![
                        punto_vert[v as usize],
                        arista(v, sig),
                        punto_cara[fi],
                        arista(ant, v),
                    ],
                });
                // Los cuatro cuadriláteros heredan la procedencia de la cara
                // que los originó. Esta línea es la que hace que una selección
                // sobreviva a la subdivisión.
                prov.face_origin
                    .push(m.prov.face_origin.get(fi).copied().flatten());
            }
        }

        Ok(Mesh {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            faces,
            prov,
        })
    }
}

impl Modifier for Subdivide {
    fn kind(&self) -> &'static str {
        "subdivide"
    }
    fn params_hash(&self) -> u64 {
        hash_f64(&[self.niveles as f64, 1.0])
    }
    fn apply(&self, input: &Mesh) -> Result<Mesh> {
        if self.niveles > 6 {
            return Err(MeshError::Parametro {
                modificador: "subdivide",
                detalle: format!(
                    "{} niveles es demasiado. El primero convierte cada n-gono en \
                     n cuadrilateros -- no en 4, eso solo vale si la malla ya era \
                     de cuadrilateros -- y a partir de ahi cada nivel multiplica \
                     por 4, o sea 4^{} veces la cuenta de despues del primero",
                    self.niveles,
                    self.niveles - 1
                ),
            });
        }
        let mut m = input.clone();
        for _ in 0..self.niveles {
            m = self.un_nivel(&m)?;
        }
        Ok(m)
    }
}

// ---------------------------------------------------------------------------
// Espejo
// ---------------------------------------------------------------------------

pub struct Mirror {
    pub punto: DVec3,
    /// Normal del plano de simetría. Se normaliza al aplicar.
    pub normal: DVec3,
    /// Suelda los vértices que caen sobre el plano.
    pub soldar_costura: bool,
}

impl Modifier for Mirror {
    fn kind(&self) -> &'static str {
        "mirror"
    }
    fn params_hash(&self) -> u64 {
        hash_f64(&[
            self.punto.x,
            self.punto.y,
            self.punto.z,
            self.normal.x,
            self.normal.y,
            self.normal.z,
            self.soldar_costura as u8 as f64,
        ])
    }
    fn apply(&self, input: &Mesh) -> Result<Mesh> {
        let n = self.normal.normalize_or_zero();
        if n == DVec3::ZERO {
            return Err(MeshError::Parametro {
                modificador: "mirror",
                detalle: "el plano de simetria tiene normal nula".into(),
            });
        }
        let base = input.positions.len() as u32;
        let mut m = input.clone();
        for p in &input.positions {
            let d = (*p - self.punto).dot(n);
            m.positions.push(*p - n * (2.0 * d));
        }
        if !input.normals.is_empty() {
            for v in &input.normals {
                m.normals.push(*v - n * (2.0 * v.dot(n)));
            }
        }
        for (fi, f) in input.faces.iter().enumerate() {
            // Reflejar invierte la orientación: sin dar la vuelta al bucle, la
            // mitad espejada queda con las caras del revés y el volumen sale mal.
            let mut vs: Vec<u32> = f.verts.iter().rev().map(|&v| v + base).collect();
            vs.rotate_right(1);
            m.faces.push(Face { verts: vs });
            m.prov
                .face_origin
                .push(input.prov.face_origin.get(fi).copied().flatten());
        }
        if self.soldar_costura {
            soldar_conservando_procedencia(&m, forge_math::tol::CONFUSION_MM * 10.0)
        } else {
            Ok(m)
        }
    }
}

// ---------------------------------------------------------------------------
// Repetición
// ---------------------------------------------------------------------------

pub struct Array {
    pub copias: u32,
    pub desplazamiento: DVec3,
}

impl Modifier for Array {
    fn kind(&self) -> &'static str {
        "array"
    }
    fn params_hash(&self) -> u64 {
        hash_f64(&[
            self.copias as f64,
            self.desplazamiento.x,
            self.desplazamiento.y,
            self.desplazamiento.z,
        ])
    }
    fn apply(&self, input: &Mesh) -> Result<Mesh> {
        if self.copias == 0 {
            return Err(MeshError::Parametro {
                modificador: "array",
                detalle: "cero copias deja la malla vacia; usa 1 para dejarla igual".into(),
            });
        }
        let nv = input.positions.len() as u32;
        let mut m = Mesh::default();
        for k in 0..self.copias {
            let d = self.desplazamiento * k as f64;
            m.positions.extend(input.positions.iter().map(|p| *p + d));
            m.normals.extend(input.normals.iter().copied());
            for (fi, f) in input.faces.iter().enumerate() {
                m.faces.push(Face {
                    verts: f.verts.iter().map(|&v| v + k * nv).collect(),
                });
                m.prov
                    .face_origin
                    .push(input.prov.face_origin.get(fi).copied().flatten());
            }
        }
        Ok(m)
    }
}

// ---------------------------------------------------------------------------
// Soldadura y triangulación
// ---------------------------------------------------------------------------

pub struct Weld {
    pub epsilon_mm: f64,
}

impl Modifier for Weld {
    fn kind(&self) -> &'static str {
        "weld"
    }
    fn params_hash(&self) -> u64 {
        hash_f64(&[self.epsilon_mm])
    }
    fn apply(&self, input: &Mesh) -> Result<Mesh> {
        soldar_conservando_procedencia(input, self.epsilon_mm)
    }
}

/// Triangula por abanico.
///
/// Correcto para caras convexas, que es lo que producen la subdivisión y el
/// teselado. **No** es correcto para caras cóncavas: ahí haría falta recorte de
/// orejas. Está dicho aquí en vez de escondido porque una triangulación mal
/// hecha se ve bien hasta que se ilumina.
pub struct Triangulate;

impl Modifier for Triangulate {
    fn kind(&self) -> &'static str {
        "triangulate"
    }
    fn params_hash(&self) -> u64 {
        0x7A1
    }
    fn apply(&self, input: &Mesh) -> Result<Mesh> {
        let mut m = Mesh {
            positions: input.positions.clone(),
            normals: input.normals.clone(),
            uvs: input.uvs.clone(),
            faces: Vec::new(),
            prov: ProvenanceMap::default(),
        };
        for (fi, f) in input.faces.iter().enumerate() {
            let origen = input.prov.face_origin.get(fi).copied().flatten();
            for k in 1..f.verts.len() - 1 {
                m.faces.push(Face {
                    verts: vec![f.verts[0], f.verts[k], f.verts[k + 1]],
                });
                m.prov.face_origin.push(origen);
            }
        }
        Ok(m)
    }
}
