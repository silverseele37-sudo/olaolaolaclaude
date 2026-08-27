//! Mallas del lado GPU y cómo se resuelven los hashes de `DrawInstance`.
//!
//! # `MeshProvider` es un *trait*, no un mapa — la misma decisión que en `forge-render-cpu`
//!
//! `DrawInstance` referencia la malla por [`BlobHash`] y `forge-render-api` es
//! explícito en que **el renderer no conoce el documento**. `forge-render-cpu`
//! resolvió esto con un trait `MeshProvider` propio (que este crate no puede
//! nombrar ni depender de él: los pilares no se conocen entre sí, ADR-0006) en
//! vez de con un `HashMap` fijo como campo del renderer. Aquí se reproduce **la
//! misma decisión**, con su mismo trait de forma (`Option<&Malla>` por hash) y
//! su misma implementación de conveniencia (`MapaDeMallas`), porque es el
//! enfoque correcto y no algo específico de CPU: la política de residencia
//! —todo en memoria, caché LRU sobre `forge-store`, teselado bajo demanda—
//! sigue siendo del llamante, nunca del renderer.
//!
//! # Lo que el renderer añade encima: la caché de buffers de GPU
//!
//! `MeshProvider` resuelve hash → geometría en `f64` (formato de documento).
//! Subir esa geometría a un `wgpu::Buffer` en cada frame sería tirar la mitad
//! del punto de referenciar mallas por hash (ver el módulo de `forge-render-api`:
//! "el diff entre frames es comparar enteros"). Por eso [`crate::renderer::GpuRenderer`]
//! mantiene su propia caché `BlobHash -> buffers de GPU` y solo llama a
//! [`MeshProvider::malla`] y sube datos la primera vez que ve un hash nuevo.

use forge_math::{Aabb, DVec3};
use forge_store::BlobHash;
use std::collections::HashMap;

/// Malla triangulada lista para subir a GPU.
///
/// `f64`: es geometría de documento. El paso a `f32` ocurre al construir el
/// buffer de vértices, sobre coordenadas **locales a la malla** (típicamente
/// pequeñas, cerca de su propio origen) — la parte que sí necesita ser grande
/// para piezas lejos del origen del documento es la traslación de la
/// instancia, que viaja aparte y relativa a la cámara (ver `crate::camara`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Malla {
    pub positions: Vec<DVec3>,
    /// Normales por vértice. Si está vacío se usa la normal geométrica de cada
    /// triángulo (facetado), igual que en `forge-render-cpu`.
    pub normals: Vec<DVec3>,
    pub indices: Vec<u32>,
}

impl Malla {
    pub fn nueva(positions: Vec<DVec3>, normals: Vec<DVec3>, indices: Vec<u32>) -> Self {
        Malla {
            positions,
            normals,
            indices,
        }
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.positions.iter().copied())
    }

    /// Hash del contenido. Misma semántica que en `forge-store` y en el
    /// `CpuMesh` de `forge-render-cpu`: dos mallas iguales dan el mismo hash y
    /// comparten recursos de GPU sin que el renderer tenga que saber por qué.
    pub fn hash(&self) -> BlobHash {
        let mut bytes = Vec::with_capacity(
            self.positions.len() * 24 + self.normals.len() * 24 + self.indices.len() * 4 + 12,
        );
        bytes.extend_from_slice(&(self.positions.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.normals.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.indices.len() as u64).to_le_bytes());
        for p in self.positions.iter().chain(self.normals.iter()) {
            bytes.extend_from_slice(&p.x.to_le_bytes());
            bytes.extend_from_slice(&p.y.to_le_bytes());
            bytes.extend_from_slice(&p.z.to_le_bytes());
        }
        for i in &self.indices {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        BlobHash::of(&bytes)
    }

    /// Coherencia mínima: índices dentro de rango, múltiplo de 3, y normales
    /// ausentes o una por vértice. Sin esto, una malla corrupta llegaría hasta
    /// el punto de construir un buffer de índices fuera de rango, que en wgpu
    /// es un error de validación en tiempo de dibujo — mucho más caro de
    /// diagnosticar que rechazarla aquí.
    pub fn es_valida(&self) -> bool {
        if self.indices.len() % 3 != 0 {
            return false;
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return false;
        }
        let n = self.positions.len() as u32;
        self.indices.iter().all(|&i| i < n)
    }

    /// Normal geométrica por triángulo, expandida a una normal por vértice
    /// duplicando vértices compartidos. Se usa cuando `normals` está vacío:
    /// wgpu no tiene el modo "normal geométrica implícita" del rasterizador
    /// por software (que la calcula por fragmento a partir del triángulo ya
    /// transformado), así que aquí hay que materializarla de antemano, una
    /// vez por malla y no una vez por frame.
    // Todavia sin llamar: la usara el pase PBR al cablear el pipeline. Se
    // conserva porque materializar normales geometricas es lo que evita que una
    // malla sin normales salga plana y negra.
    #[allow(dead_code)]
    pub(crate) fn normales_o_geometricas(&self) -> Vec<DVec3> {
        if !self.normals.is_empty() {
            return self.normals.clone();
        }
        let mut out = vec![DVec3::ZERO; self.positions.len()];
        for tri in self.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let (p0, p1, p2) = (self.positions[i0], self.positions[i1], self.positions[i2]);
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            out[i0] = n;
            out[i1] = n;
            out[i2] = n;
        }
        out
    }
}

/// Traduce el hash de una instancia a geometría. Ver la nota del módulo: es la
/// misma decisión de diseño que `forge_render_cpu::MeshProvider`, reproducida
/// aquí porque ese crate no es una dependencia disponible.
pub trait MeshProvider {
    /// `None` cuando la malla todavía no está disponible. No es un error: la
    /// instancia se cuenta como descartada y el frame sigue.
    fn malla(&self, hash: BlobHash) -> Option<&Malla>;
}

/// Implementación de conveniencia: todo residente en memoria.
#[derive(Clone, Debug, Default)]
pub struct MapaDeMallas {
    mallas: HashMap<BlobHash, Malla>,
}

impl MapaDeMallas {
    pub fn nuevo() -> Self {
        Self::default()
    }

    /// Inserta y devuelve el hash de contenido, que es lo que hay que poner en
    /// `DrawInstance::mesh`.
    pub fn insertar(&mut self, m: Malla) -> BlobHash {
        let h = m.hash();
        self.mallas.insert(h, m);
        h
    }

    pub fn len(&self) -> usize {
        self.mallas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mallas.is_empty()
    }
}

impl MeshProvider for MapaDeMallas {
    fn malla(&self, hash: BlobHash) -> Option<&Malla> {
        self.mallas.get(&hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cubo() -> Malla {
        let p = [
            DVec3::new(-1.0, -1.0, -1.0),
            DVec3::new(1.0, -1.0, -1.0),
            DVec3::new(1.0, 1.0, -1.0),
            DVec3::new(-1.0, 1.0, -1.0),
        ];
        Malla::nueva(p.to_vec(), Vec::new(), vec![0, 1, 2, 0, 2, 3])
    }

    #[test]
    fn el_hash_es_estable_y_distingue_contenido_distinto() {
        let a = cubo();
        let mut b = cubo();
        assert_eq!(a.hash(), b.hash());
        b.positions[0].x += 1e-6;
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn mapa_de_mallas_resuelve_por_el_hash_devuelto_al_insertar() {
        let mut mapa = MapaDeMallas::nuevo();
        let h = mapa.insertar(cubo());
        assert!(mapa.malla(h).is_some());
        assert!(mapa.malla(BlobHash::of(b"no existe")).is_none());
    }

    #[test]
    fn malla_invalida_por_indice_fuera_de_rango() {
        let mut m = cubo();
        m.indices[0] = 99;
        assert!(!m.es_valida());
    }

    #[test]
    fn normales_geometricas_son_perpendiculares_a_la_cara() {
        let m = cubo();
        let normales = m.normales_o_geometricas();
        // Las cuatro son la cara z=-1: la normal geométrica tiene que apuntar
        // en +Z o -Z según el sentido de bobinado, y ser unitaria.
        for n in &normales {
            assert!((n.length() - 1.0).abs() < 1e-9);
            assert!(n.x.abs() < 1e-9 && n.y.abs() < 1e-9);
        }
    }
}
