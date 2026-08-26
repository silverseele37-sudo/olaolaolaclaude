//! glTF 2.0, escritura en contenedor binario (`.glb`).
//!
//! glTF es el formato de referencia del pilar de runtime: describe mallas,
//! materiales PBR metallic-roughness y jerarquía, y lo lee todo el mundo. No
//! tiene B-Rep, así que exportar aquí es siempre cruzar al dominio discreto.
//!
//! # Dos conversiones obligatorias, y las dos son fuente clásica de errores
//!
//! 1. **Ejes.** glTF es Y arriba **por especificación**. FORGE es Z arriba.
//! 2. **Unidades.** La especificación dice que la unidad lineal es el **metro**.
//!    FORGE trabaja en milímetros. Exportar sin dividir produce modelos mil veces
//!    más grandes, que es el bug que hace que una pieza aparezca a un kilómetro
//!    de la cámara en el visor de destino.
//!
//! Las dos son opcionales en [`GltfOptions`] porque hay flujos que prefieren los
//! números crudos, pero **el valor por defecto es el que cumple la
//! especificación**. Un exportador que incumple el estándar en silencio traslada
//! el problema a quien recibe el archivo.

use forge_math::DVec3;
use serde_json::{json, Value};
use std::path::Path;

use crate::{z_up_to_y_up, InteropError, Result, TriangleSoup};

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

const COMPONENT_FLOAT: u32 = 5126;
const COMPONENT_UINT: u32 = 5125;
const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;

#[derive(Clone, Copy, Debug)]
pub struct GltfOptions {
    /// Convertir Z-up → Y-up. Por defecto sí: lo exige la especificación.
    pub y_up: bool,
    /// Convertir milímetros → metros. Por defecto sí: lo exige la especificación.
    pub to_meters: bool,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
}

impl Default for GltfOptions {
    fn default() -> Self {
        GltfOptions {
            y_up: true,
            to_meters: true,
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.5,
        }
    }
}

impl GltfOptions {
    /// Sin conversiones: los números tal cual están en FORGE. Incumple la
    /// especificación a propósito; úsalo solo cuando el receptor lo espere.
    pub fn crudo() -> Self {
        GltfOptions { y_up: false, to_meters: false, ..Default::default() }
    }

    fn convertir(&self, v: DVec3) -> DVec3 {
        let v = if self.y_up { z_up_to_y_up(v) } else { v };
        if self.to_meters {
            v / 1000.0
        } else {
            v
        }
    }

    /// Las normales rotan pero **no** se escalan: son direcciones.
    fn convertir_normal(&self, v: DVec3) -> DVec3 {
        if self.y_up {
            z_up_to_y_up(v)
        } else {
            v
        }
    }
}

/// Acumula datos binarios respetando la alineación que exige glTF.
#[derive(Default)]
struct Bin {
    bytes: Vec<u8>,
}

impl Bin {
    /// Devuelve `(offset, longitud)`. Alinea el inicio a 4 bytes: los
    /// `byteOffset` de accessor y bufferView deben ser múltiplos del tamaño de
    /// componente, y con 4 se cumple para `f32` y `u32` a la vez.
    fn push_f32(&mut self, vals: impl Iterator<Item = f32>) -> (usize, usize) {
        self.align(4);
        let inicio = self.bytes.len();
        for v in vals {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        (inicio, self.bytes.len() - inicio)
    }

    fn push_u32(&mut self, vals: impl Iterator<Item = u32>) -> (usize, usize) {
        self.align(4);
        let inicio = self.bytes.len();
        for v in vals {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        (inicio, self.bytes.len() - inicio)
    }

    fn align(&mut self, a: usize) {
        while self.bytes.len() % a != 0 {
            self.bytes.push(0);
        }
    }
}

/// Construye el `.glb` en memoria.
pub fn to_glb(soup: &TriangleSoup, opts: GltfOptions) -> Result<Vec<u8>> {
    soup.validate()?;

    let mut bin = Bin::default();
    let mut accessors: Vec<Value> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut attrs = serde_json::Map::new();

    // --- POSITION ---
    let pos: Vec<DVec3> = soup.positions.iter().map(|p| opts.convertir(*p)).collect();
    // La especificación **exige** min y max en el accessor de POSITION: los
    // visores los usan para encuadrar sin leer el buffer.
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in &pos {
        for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
            min[k] = min[k].min(c);
            max[k] = max[k].max(c);
        }
    }
    let (off, len) = bin.push_f32(pos.iter().flat_map(|p| [p.x as f32, p.y as f32, p.z as f32]));
    views.push(json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}));
    accessors.push(json!({
        "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
        "count": pos.len(), "type": "VEC3",
        "min": min.map(|v| v as f32), "max": max.map(|v| v as f32)
    }));
    attrs.insert("POSITION".into(), json!(accessors.len() - 1));

    // --- NORMAL ---
    if !soup.normals.is_empty() {
        let ns: Vec<DVec3> = soup.normals.iter().map(|n| opts.convertir_normal(*n)).collect();
        let (off, len) = bin.push_f32(ns.iter().flat_map(|n| [n.x as f32, n.y as f32, n.z as f32]));
        views.push(json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}));
        accessors.push(json!({
            "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
            "count": ns.len(), "type": "VEC3"
        }));
        attrs.insert("NORMAL".into(), json!(accessors.len() - 1));
    }

    // --- TEXCOORD_0 ---
    if !soup.uvs.is_empty() {
        let (off, len) = bin.push_f32(soup.uvs.iter().flat_map(|t| [t.x as f32, t.y as f32]));
        views.push(json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}));
        accessors.push(json!({
            "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
            "count": soup.uvs.len(), "type": "VEC2"
        }));
        attrs.insert("TEXCOORD_0".into(), json!(accessors.len() - 1));
    }

    // --- índices ---
    let (off, len) = bin.push_u32(soup.indices.iter().copied());
    views.push(json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ELEMENT_ARRAY_BUFFER}));
    accessors.push(json!({
        "bufferView": views.len()-1, "componentType": COMPONENT_UINT,
        "count": soup.indices.len(), "type": "SCALAR"
    }));
    let idx_accessor = accessors.len() - 1;

    let nombre = if soup.name.is_empty() { "malla" } else { &soup.name };
    let doc = json!({
        "asset": { "version": "2.0", "generator": format!("FORGE {}", env!("CARGO_PKG_VERSION")) },
        "scene": 0,
        "scenes": [ { "nodes": [0] } ],
        "nodes": [ { "mesh": 0, "name": nombre } ],
        "meshes": [ { "name": nombre, "primitives": [ {
            "attributes": Value::Object(attrs),
            "indices": idx_accessor,
            "material": 0,
            "mode": 4
        } ] } ],
        "materials": [ { "name": "material", "pbrMetallicRoughness": {
            "baseColorFactor": opts.base_color,
            "metallicFactor": opts.metallic,
            "roughnessFactor": opts.roughness
        } } ],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [ { "byteLength": bin.bytes.len() } ]
    });

    let mut json_bytes =
        serde_json::to_vec(&doc).map_err(|e| InteropError::Json(e.to_string()))?;
    // El chunk JSON se rellena con **espacios**, el binario con ceros. Lo dice
    // la especificación y algunos lectores estrictos lo comprueban.
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin.bytes;
    while bin_bytes.len() % 4 != 0 {
        bin_bytes.push(0);
    }

    let total = 12 + 8 + json_bytes.len() + if bin_bytes.is_empty() { 0 } else { 8 + bin_bytes.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    if !bin_bytes.is_empty() {
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&bin_bytes);
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

pub fn write_glb(path: impl AsRef<Path>, soup: &TriangleSoup, opts: GltfOptions) -> Result<()> {
    let path = path.as_ref();
    let bytes = to_glb(soup, opts)?;
    std::fs::write(path, bytes).map_err(|e| InteropError::Io { path: path.into(), source: e })
}

/// Extrae el JSON de un `.glb`. Para tests y para inspeccionar sin herramientas.
pub fn glb_json(glb: &[u8]) -> Result<Value> {
    if glb.len() < 20 {
        return Err(InteropError::Malformed { line: 0, detail: "glb demasiado corto".into() });
    }
    let leer = |o: usize| u32::from_le_bytes([glb[o], glb[o + 1], glb[o + 2], glb[o + 3]]);
    if leer(0) != GLB_MAGIC {
        return Err(InteropError::Malformed { line: 0, detail: "magic no es glTF".into() });
    }
    let json_len = leer(12) as usize;
    if leer(16) != CHUNK_JSON {
        return Err(InteropError::Malformed { line: 0, detail: "el primer chunk no es JSON".into() });
    }
    let fin = 20 + json_len;
    if fin > glb.len() {
        return Err(InteropError::Malformed { line: 0, detail: "chunk JSON truncado".into() });
    }
    serde_json::from_slice(&glb[20..fin]).map_err(|e| InteropError::Json(e.to_string()))
}
