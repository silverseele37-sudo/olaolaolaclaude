//! Poliedro de caras planas: la representación interna del kernel stub.
//!
//! Un sólido es su frontera: una lista de vértices y una lista de caras, cada
//! cara un bucle de índices en orden antihorario **visto desde fuera**. Las
//! aristas y los vértices topológicos se **derivan** de las caras en vez de
//! almacenarse, porque derivarlos garantiza que no puedan quedar
//! desincronizados — que es la clase de corrupción que produce fallos a
//! distancia imposibles de diagnosticar.

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_kernel_api::{
    EdgeKind, GeometrySignature, KernelError, KernelResult, MassProperties, TopoProvenance,
};
use forge_math::{orthonormal_basis, tol, Aabb, DVec2, DVec3};
use serde::{Deserialize, Serialize};

/// Hash estable y explícito (FNV-1a de 64 bits).
///
/// No se usa `DefaultHasher`: su estabilidad entre versiones de la biblioteca
/// estándar no está garantizada, y de estos valores depende que un `StableId`
/// siga siendo el mismo entre sesiones y entre máquinas.
pub fn fnv(partes: &[u64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in partes {
        for b in p.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

pub fn marca(etiqueta: &str, indice: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in etiqueta.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    fnv(&[h, indice as u64])
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cara {
    pub id: StableId,
    pub prov: TopoProvenance,
    /// Índices a `Poly::verts`, antihorario visto desde fuera.
    pub bucle: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arista {
    pub id: StableId,
    pub prov: TopoProvenance,
    pub a: u32,
    pub b: u32,
    pub caras: Vec<u32>,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Poly {
    pub verts: Vec<DVec3>,
    pub caras: Vec<Cara>,
    /// `false` para perfiles abiertos (una sola cara sin volumen).
    pub solido: bool,
}

impl Poly {
    pub fn normal_de(&self, cara: &Cara) -> DVec3 {
        // Newell: robusta con polígonos no planos por error numérico, a
        // diferencia del producto vectorial de dos aristas cualesquiera, que
        // degenera cuando esas dos aristas son casi colineales.
        let mut n = DVec3::ZERO;
        let m = cara.bucle.len();
        for i in 0..m {
            let p = self.verts[cara.bucle[i] as usize];
            let q = self.verts[cara.bucle[(i + 1) % m] as usize];
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

    pub fn centroide_de(&self, cara: &Cara) -> DVec3 {
        let s: DVec3 = cara.bucle.iter().map(|&i| self.verts[i as usize]).sum();
        s / cara.bucle.len() as f64
    }

    pub fn area_de(&self, cara: &Cara) -> f64 {
        self.triangular(cara)
            .iter()
            .map(|t| {
                let (a, b, c) = (
                    self.verts[t[0] as usize],
                    self.verts[t[1] as usize],
                    self.verts[t[2] as usize],
                );
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    pub fn bbox(&self) -> Aabb {
        Aabb::from_points(self.verts.iter().copied())
    }

    /// Volumen con signo por el teorema de la divergencia. Exacto para
    /// poliedros. El signo dice si la orientación es coherente hacia fuera:
    /// negativo significa que los bucles están al revés.
    pub fn volumen_con_signo(&self) -> f64 {
        let mut v = 0.0;
        for cara in &self.caras {
            for t in self.triangular(cara) {
                let (a, b, c) = (
                    self.verts[t[0] as usize],
                    self.verts[t[1] as usize],
                    self.verts[t[2] as usize],
                );
                v += a.dot(b.cross(c));
            }
        }
        v / 6.0
    }

    /// Propiedades de masa exactas por descomposición en tetraedros.
    pub fn propiedades(&self) -> MassProperties {
        let mut vol = 0.0;
        let mut acc = DVec3::ZERO;
        let mut area = 0.0;
        for cara in &self.caras {
            let tris = self.triangular(cara);
            for t in &tris {
                let (a, b, c) = (
                    self.verts[t[0] as usize],
                    self.verts[t[1] as usize],
                    self.verts[t[2] as usize],
                );
                area += (b - a).cross(c - a).length() * 0.5;
                let vt = a.dot(b.cross(c)) / 6.0;
                vol += vt;
                acc += (a + b + c) / 4.0 * vt;
            }
        }
        let centroide = if vol.abs() > 1e-12 {
            acc / vol
        } else {
            DVec3::ZERO
        };
        MassProperties {
            volume_mm3: vol.abs(),
            area_mm2: area,
            centroid: centroide,
        }
    }

    /// Invierte todos los bucles. Se usa cuando el volumen sale negativo.
    pub fn invertir(&mut self) {
        for c in &mut self.caras {
            c.bucle.reverse();
        }
    }

    /// Triangulación por recorte de orejas, en el plano de la cara.
    ///
    /// No asume convexidad: un perfil de CAD real casi nunca lo es. Proyecta al
    /// plano con la base ortonormal **con guarda** de `forge-math` — sembrarla
    /// sin guarda produce `NaN` en las caras cuya normal cae en el eje semilla.
    pub fn triangular(&self, cara: &Cara) -> Vec<[u32; 3]> {
        let m = cara.bucle.len();
        if m < 3 {
            return Vec::new();
        }
        if m == 3 {
            return vec![[cara.bucle[0], cara.bucle[1], cara.bucle[2]]];
        }

        let n = self.normal_de(cara);
        let (t, b) = orthonormal_basis(n);
        let origen = self.verts[cara.bucle[0] as usize];
        let p2: Vec<DVec2> = cara
            .bucle
            .iter()
            .map(|&i| {
                let d = self.verts[i as usize] - origen;
                DVec2::new(d.dot(t), d.dot(b))
            })
            .collect();

        // Área con signo en 2D: si es negativa, el bucle proyectado va al revés
        // y hay que recorrerlo invertido para que el test de oreja funcione.
        let area2: f64 = (0..m)
            .map(|i| {
                let (p, q) = (p2[i], p2[(i + 1) % m]);
                p.x * q.y - q.x * p.y
            })
            .sum();
        let mut restantes: Vec<usize> = if area2 >= 0.0 {
            (0..m).collect()
        } else {
            (0..m).rev().collect()
        };

        let dentro = |a: DVec2, b: DVec2, c: DVec2, p: DVec2| {
            let s = |u: DVec2, v: DVec2, w: DVec2| {
                (v.x - u.x) * (w.y - u.y) - (v.y - u.y) * (w.x - u.x)
            };
            let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
            let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            !(neg && pos)
        };

        let mut tris = Vec::with_capacity(m - 2);
        let mut guarda = 0;
        while restantes.len() > 3 {
            guarda += 1;
            if guarda > m * m + 16 {
                break; // polígono degenerado: se corta en abanico más abajo
            }
            let k = restantes.len();
            let mut cortada = false;
            for i in 0..k {
                let (ia, ib, ic) = (
                    restantes[(i + k - 1) % k],
                    restantes[i],
                    restantes[(i + 1) % k],
                );
                let (a, bb, c) = (p2[ia], p2[ib], p2[ic]);
                let cruz = (bb.x - a.x) * (c.y - a.y) - (bb.y - a.y) * (c.x - a.x);
                if cruz <= 0.0 {
                    continue; // reflejo, no es oreja
                }
                if restantes
                    .iter()
                    .any(|&j| j != ia && j != ib && j != ic && dentro(a, bb, c, p2[j]))
                {
                    continue; // hay un vértice dentro
                }
                tris.push([cara.bucle[ia], cara.bucle[ib], cara.bucle[ic]]);
                restantes.remove(i);
                cortada = true;
                break;
            }
            if !cortada {
                break;
            }
        }
        if restantes.len() == 3 {
            tris.push([
                cara.bucle[restantes[0]],
                cara.bucle[restantes[1]],
                cara.bucle[restantes[2]],
            ]);
        } else if restantes.len() > 3 {
            // Degenerado: abanico. Peor calidad, pero cubre la superficie.
            for i in 1..restantes.len() - 1 {
                tris.push([
                    cara.bucle[restantes[0]],
                    cara.bucle[restantes[i]],
                    cara.bucle[restantes[i + 1]],
                ]);
            }
        }
        tris
    }

    /// Deriva aristas de las caras y las clasifica por ángulo diedro.
    ///
    /// La clasificación importa: un visor CAD **no** dibuja todas las aristas
    /// topológicas. Las tangentes taparían la pieza de líneas que ningún plano
    /// lleva.
    pub fn aristas(&self, owner: FeatureId) -> KernelResult<Vec<Arista>> {
        use std::collections::BTreeMap;
        let mut mapa: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
        for (fi, cara) in self.caras.iter().enumerate() {
            let m = cara.bucle.len();
            for i in 0..m {
                let (a, b) = (cara.bucle[i], cara.bucle[(i + 1) % m]);
                mapa.entry((a.min(b), a.max(b)))
                    .or_default()
                    .push(fi as u32);
            }
        }

        let mut por_par: BTreeMap<(u64, u64), u32> = BTreeMap::new();
        let mut out = Vec::with_capacity(mapa.len());
        for ((a, b), caras) in mapa {
            if caras.len() > 2 {
                return Err(KernelError::Degenerate {
                    hint: format!(
                        "arista {a}-{b} compartida por {} caras (no manifold)",
                        caras.len()
                    ),
                });
            }
            let kind = match caras.len() {
                1 => EdgeKind::Boundary,
                _ => {
                    let n0 = self.normal_de(&self.caras[caras[0] as usize]);
                    let n1 = self.normal_de(&self.caras[caras[1] as usize]);
                    // Tangente si las normales casi coinciden: es un redondeo
                    // con su cara, no un quiebre.
                    if n0.dot(n1) > 0.999_847 {
                        EdgeKind::Smooth
                    } else {
                        EdgeKind::Sharp
                    }
                }
            };
            // La identidad de la arista se deriva de la de sus caras: genealogía,
            // no índices. Si las caras conservan su marca al regenerar, la arista
            // también, que es justo lo que necesita el nombrado persistente.
            let m0 = self.caras[caras[0] as usize].id.mark;
            let m1 = caras
                .get(1)
                .map(|&f| self.caras[f as usize].id.mark)
                .unwrap_or(m0);
            let clave = (m0.min(m1), m0.max(m1));
            let k = por_par.entry(clave).or_insert(0);
            let mark = fnv(&[clave.0, clave.1, *k as u64]);
            *k += 1;

            out.push(Arista {
                id: StableId {
                    origin: owner,
                    class: TopoClass::Edge,
                    mark,
                },
                prov: TopoProvenance::Inherited {
                    from: self.caras[caras[0] as usize].id,
                },
                a,
                b,
                caras,
                kind,
            });
        }
        Ok(out)
    }

    pub fn firma_de_cara(&self, cara: &Cara) -> GeometrySignature {
        GeometrySignature::new(
            self.centroide_de(cara),
            self.normal_de(cara),
            self.area_de(cara),
            TopoClass::Face,
        )
    }

    pub fn firma_de_arista(&self, e: &Arista) -> GeometrySignature {
        let (a, b) = (self.verts[e.a as usize], self.verts[e.b as usize]);
        GeometrySignature::new(
            (a + b) * 0.5,
            (b - a).normalize(),
            (b - a).length(),
            TopoClass::Edge,
        )
    }

    /// Colapsa vértices coincidentes. Sin esto, un extrude produce vértices
    /// duplicados en las esquinas y las aristas no se derivan bien: dos caras
    /// que comparten una arista no la comparten si sus vértices son distintos
    /// aunque estén en el mismo sitio.
    pub fn soldar(&mut self) {
        let q = |v: f64| (v / tol::CONFUSION_MM).round() as i64;
        let mut mapa: std::collections::HashMap<(i64, i64, i64), u32> = Default::default();
        let mut nuevos = Vec::with_capacity(self.verts.len());
        let mut remap = vec![0u32; self.verts.len()];
        for (i, v) in self.verts.iter().enumerate() {
            let clave = (q(v.x), q(v.y), q(v.z));
            let idx = *mapa.entry(clave).or_insert_with(|| {
                nuevos.push(*v);
                nuevos.len() as u32 - 1
            });
            remap[i] = idx;
        }
        self.verts = nuevos;
        for c in &mut self.caras {
            let mut b: Vec<u32> = c.bucle.iter().map(|&i| remap[i as usize]).collect();
            b.dedup();
            if b.len() > 1 && b[0] == *b.last().unwrap() {
                b.pop();
            }
            c.bucle = b;
        }
    }
}
