//! Hash de contenido, estable entre versiones y entre máquinas.
//!
//! No se usa `DefaultHasher`: la biblioteca estándar no garantiza que su valor
//! se mantenga entre versiones, y de estos números depende que la caché de
//! evaluación acierte al reabrir un documento guardado con otra compilación.
//! Mismo criterio (y misma función) que `forge-kernel-stub`.

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_kernel_api::GeometrySignature;
use forge_math::{DAffine3, DVec2, DVec3};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x100_0000_01b3;

/// Acumulador FNV-1a de 64 bits.
#[derive(Clone, Copy, Debug)]
pub struct Hasher(u64);

impl Default for Hasher {
    fn default() -> Self {
        Hasher(OFFSET)
    }
}

impl Hasher {
    pub fn new() -> Self {
        Hasher::default()
    }

    pub fn valor(self) -> u64 {
        self.0
    }

    pub fn byte(&mut self, b: u8) -> &mut Self {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(PRIME);
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
        self
    }

    pub fn u128(&mut self, v: u128) -> &mut Self {
        self.u64(v as u64).u64((v >> 64) as u64)
    }

    pub fn usize(&mut self, v: usize) -> &mut Self {
        self.u64(v as u64)
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.byte(v as u8)
    }

    pub fn texto(&mut self, s: &str) -> &mut Self {
        self.usize(s.len());
        for b in s.as_bytes() {
            self.byte(*b);
        }
        self
    }

    /// Hashea un `f64` por sus bits, **normalizando el cero**.
    ///
    /// `-0.0` y `0.0` son el mismo número y tienen que dar el mismo hash: si no,
    /// mover un sketch a `-0.0` invalidaría la caché de todo lo que hay debajo
    /// sin que la geometría cambiase. Los `NaN` se colapsan a un patrón único
    /// por la misma razón.
    pub fn f64(&mut self, v: f64) -> &mut Self {
        let v = if v == 0.0 {
            0.0
        } else if v.is_nan() {
            f64::NAN
        } else {
            v
        };
        self.u64(v.to_bits())
    }

    pub fn vec2(&mut self, v: DVec2) -> &mut Self {
        self.f64(v.x).f64(v.y)
    }

    pub fn vec3(&mut self, v: DVec3) -> &mut Self {
        self.f64(v.x).f64(v.y).f64(v.z)
    }

    pub fn afin(&mut self, m: &DAffine3) -> &mut Self {
        self.vec3(m.matrix3.x_axis)
            .vec3(m.matrix3.y_axis)
            .vec3(m.matrix3.z_axis)
            .vec3(m.translation)
    }

    pub fn feature(&mut self, f: FeatureId) -> &mut Self {
        self.u128(f.0 .0)
    }

    pub fn clase(&mut self, c: TopoClass) -> &mut Self {
        self.byte(match c {
            TopoClass::Face => 0,
            TopoClass::Edge => 1,
            TopoClass::Vertex => 2,
        })
    }

    pub fn estable(&mut self, id: StableId) -> &mut Self {
        self.feature(id.origin).clase(id.class).u64(id.mark)
    }

    pub fn firma(&mut self, s: &GeometrySignature) -> &mut Self {
        for v in s.centroid_q.iter().chain(s.normal_q.iter()) {
            self.u64(*v as u64);
        }
        self.u64(s.measure_q as u64).clase(s.class)
    }
}

/// Atajo para hashear una lista de `u64`.
pub fn de(partes: &[u64]) -> u64 {
    let mut h = Hasher::new();
    for p in partes {
        h.u64(*p);
    }
    h.valor()
}
