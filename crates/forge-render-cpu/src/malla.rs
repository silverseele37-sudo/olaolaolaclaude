//! Mallas y materiales del lado CPU, y cómo se resuelven los hashes.
//!
//! # La decisión: `MeshProvider` es un *trait*, no un mapa
//!
//! `DrawInstance` referencia la malla por [`BlobHash`] y el contrato de
//! `forge-render-api` es explícito en que **el renderer no conoce el
//! documento**. Así que hacía falta algo que traduzca hash → geometría, y había
//! tres opciones:
//!
//! 1. Meter la geometría en `SceneView`. Rompe el contrato: `SceneView` está
//!    diseñada precisamente para que el diff entre frames sea comparar enteros.
//! 2. Un `HashMap<BlobHash, CpuMesh>` como campo del renderer. Funciona, pero
//!    obliga a materializar **toda** la geometría antes de dibujar y ata al
//!    renderer a una única política de residencia.
//! 3. Un trait ([`MeshProvider`]) que el renderer consulta.
//!
//! Se elige (3). El coste es un parámetro de tipo en [`crate::SoftwareRenderer`];
//! la ganancia es que la política de residencia —todo en memoria, caché LRU
//! sobre `forge-store`, teselado bajo demanda— es del llamante y no del
//! rasterizador. Para el caso fácil se incluye [`MapaDeMallas`], que es la
//! opción (2) implementando el trait, así que quien quiera el mapa lo tiene sin
//! que el rasterizador lo imponga.
//!
//! El trait devuelve `Option`: un hash que no se resuelve **no es un panic**.
//! Es lo normal mientras un teselado está en vuelo, y se contabiliza como
//! instancia descartada.

use forge_math::{Aabb, DVec3};
use forge_render_api::MaterialId;
use forge_store::BlobHash;
use std::collections::HashMap;

/// Malla triangulada lista para rasterizar.
///
/// `f64` porque es geometría de documento (ADR: milímetros, `f64`); el paso a
/// `f32` ocurre dentro del rasterizador y relativo a la cámara.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuMesh {
    pub positions: Vec<DVec3>,
    /// Normales por vértice. Si está vacío se usa la normal geométrica de cada
    /// triángulo, que es lo correcto para geometría facetada.
    pub normals: Vec<DVec3>,
    pub indices: Vec<u32>,
}

impl CpuMesh {
    pub fn nueva(positions: Vec<DVec3>, normals: Vec<DVec3>, indices: Vec<u32>) -> Self {
        CpuMesh {
            positions,
            normals,
            indices,
        }
    }

    /// Caja del contenido.
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.positions.iter().copied())
    }

    /// Hash del **contenido**, con la misma semántica que en `forge-store`: dos
    /// mallas iguales dan el mismo hash y comparten recursos sin que el
    /// renderer tenga que saber por qué.
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
    /// ausentes o con una por vértice.
    pub fn es_valida(&self) -> bool {
        if !self.indices.len().is_multiple_of(3) {
            return false;
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return false;
        }
        let n = self.positions.len() as u32;
        self.indices.iter().all(|&i| i < n)
    }
}

/// Traduce el hash de una instancia a geometría. Ver la nota del módulo.
pub trait MeshProvider {
    /// `None` cuando la malla todavía no está disponible. No es un error: la
    /// instancia se cuenta como descartada y el frame sigue.
    fn malla(&self, hash: BlobHash) -> Option<&CpuMesh>;
}

/// Implementación de conveniencia: todo residente en memoria.
#[derive(Clone, Debug, Default)]
pub struct MapaDeMallas {
    mallas: HashMap<BlobHash, CpuMesh>,
}

impl MapaDeMallas {
    pub fn nuevo() -> Self {
        Self::default()
    }

    /// Inserta indexando por el hash del **contenido decodificado**, y lo
    /// devuelve.
    ///
    /// Sirve para montar una escena a mano (tests, demos): quien llama pone ese
    /// hash en `DrawInstance::mesh` y cuadra. **No sirve para una escena que
    /// venga de un documento**: ahí el hash de la instancia es el del blob del
    /// documento —los bytes del GLB— y no coincide con este. Para ese caso está
    /// [`MapaDeMallas::insertar_con_hash`].
    pub fn insertar(&mut self, m: CpuMesh) -> BlobHash {
        let h = m.hash();
        self.mallas.insert(h, m);
        h
    }

    /// Inserta bajo un hash que decide quien llama.
    ///
    /// Es el camino del documento: `DrawInstance::mesh` lleva el hash del blob
    /// tal y como está en el `.forge`, así que la malla ya decodificada tiene
    /// que quedar indexada por **ese** hash y no por el suyo propio. Cruzarlos
    /// no da ningún error: `malla()` devuelve `None`, la instancia se descarta
    /// y el frame sale en negro.
    pub fn insertar_con_hash(&mut self, h: BlobHash, m: CpuMesh) {
        self.mallas.insert(h, m);
    }

    pub fn len(&self) -> usize {
        self.mallas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mallas.is_empty()
    }
}

impl MeshProvider for MapaDeMallas {
    fn malla(&self, hash: BlobHash) -> Option<&CpuMesh> {
        self.mallas.get(&hash)
    }
}

/// Material del rasterizador.
///
/// `forge-render-api` solo transporta un [`MaterialId`]; la tabla de materiales
/// vive aquí por la misma razón que las mallas: el renderer no conoce el
/// documento. Es deliberadamente pequeño —no hay texturas— porque este
/// rasterizador existe para verificar, no para producir.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuMaterial {
    /// Reflectancia difusa lineal Rec.709.
    pub base_color: [f32; 3],
    /// 0 = espejo perfecto, 1 = completamente rugoso.
    pub roughness: f32,
    pub metallic: f32,
}

impl Default for CpuMaterial {
    fn default() -> Self {
        CpuMaterial {
            base_color: [0.8, 0.8, 0.8],
            roughness: 0.5,
            metallic: 0.0,
        }
    }
}

impl CpuMaterial {
    /// Reflectancia especular a incidencia normal.
    ///
    /// 0.04 es el valor de los dieléctricos comunes (índice de refracción ~1.5);
    /// en un metal, el propio color base hace de F0 y no hay difusa.
    #[inline]
    pub fn f0(&self) -> [f32; 3] {
        let m = self.metallic.clamp(0.0, 1.0);
        [
            0.04 * (1.0 - m) + self.base_color[0] * m,
            0.04 * (1.0 - m) + self.base_color[1] * m,
            0.04 * (1.0 - m) + self.base_color[2] * m,
        ]
    }
}

/// Tabla de materiales por id. Determinista: `BTreeMap` y no `HashMap`, porque
/// aunque hoy solo se consulte, cualquier recorrido futuro tiene que dar el
/// mismo orden en cada ejecución.
#[derive(Clone, Debug, Default)]
pub struct TablaDeMateriales {
    tabla: std::collections::BTreeMap<u64, CpuMaterial>,
    por_defecto: CpuMaterial,
}

impl TablaDeMateriales {
    pub fn nueva() -> Self {
        Self::default()
    }

    pub fn con_defecto(m: CpuMaterial) -> Self {
        TablaDeMateriales {
            tabla: Default::default(),
            por_defecto: m,
        }
    }

    pub fn insertar(&mut self, id: MaterialId, m: CpuMaterial) {
        self.tabla.insert(id.0, m);
    }

    /// Un id desconocido devuelve el material por defecto: dibujar en gris es
    /// una respuesta mucho más útil que abortar el frame.
    pub fn material(&self, id: MaterialId) -> CpuMaterial {
        self.tabla.get(&id.0).copied().unwrap_or(self.por_defecto)
    }
}
