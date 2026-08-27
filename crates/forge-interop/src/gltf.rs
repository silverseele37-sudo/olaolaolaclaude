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

use forge_math::{DVec2, DVec3};
use serde_json::{json, Value};
use std::path::Path;

use crate::{y_up_to_z_up, z_up_to_y_up, InteropError, Result, TriangleSoup};

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
        GltfOptions {
            y_up: false,
            to_meters: false,
            ..Default::default()
        }
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

    /// Inversa de `convertir`: Y-up → Z-up, metros → milímetros.
    /// Usado por `read_glb` para deshacer las transformaciones aplicadas en `to_glb`.
    fn invertir(&self, v: DVec3) -> DVec3 {
        let v = if self.to_meters { v * 1000.0 } else { v };
        if self.y_up { y_up_to_z_up(v) } else { v }
    }

    /// Inversa de `convertir_normal`: las normales solo rotan, sin escala.
    fn invertir_normal(&self, v: DVec3) -> DVec3 {
        if self.y_up {
            y_up_to_z_up(v)
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
        while !self.bytes.len().is_multiple_of(a) {
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
    let (off, len) = bin.push_f32(
        pos.iter()
            .flat_map(|p| [p.x as f32, p.y as f32, p.z as f32]),
    );
    views.push(json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}));
    accessors.push(json!({
        "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
        "count": pos.len(), "type": "VEC3",
        "min": min.map(|v| v as f32), "max": max.map(|v| v as f32)
    }));
    attrs.insert("POSITION".into(), json!(accessors.len() - 1));

    // --- NORMAL ---
    if !soup.normals.is_empty() {
        let ns: Vec<DVec3> = soup
            .normals
            .iter()
            .map(|n| opts.convertir_normal(*n))
            .collect();
        let (off, len) = bin.push_f32(ns.iter().flat_map(|n| [n.x as f32, n.y as f32, n.z as f32]));
        views.push(
            json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}),
        );
        accessors.push(json!({
            "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
            "count": ns.len(), "type": "VEC3"
        }));
        attrs.insert("NORMAL".into(), json!(accessors.len() - 1));
    }

    // --- TEXCOORD_0 ---
    if !soup.uvs.is_empty() {
        let (off, len) = bin.push_f32(soup.uvs.iter().flat_map(|t| [t.x as f32, t.y as f32]));
        views.push(
            json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ARRAY_BUFFER}),
        );
        accessors.push(json!({
            "bufferView": views.len()-1, "componentType": COMPONENT_FLOAT,
            "count": soup.uvs.len(), "type": "VEC2"
        }));
        attrs.insert("TEXCOORD_0".into(), json!(accessors.len() - 1));
    }

    // --- índices ---
    let (off, len) = bin.push_u32(soup.indices.iter().copied());
    views.push(
        json!({"buffer":0,"byteOffset":off,"byteLength":len,"target":TARGET_ELEMENT_ARRAY_BUFFER}),
    );
    accessors.push(json!({
        "bufferView": views.len()-1, "componentType": COMPONENT_UINT,
        "count": soup.indices.len(), "type": "SCALAR"
    }));
    let idx_accessor = accessors.len() - 1;

    let nombre = if soup.name.is_empty() {
        "malla"
    } else {
        &soup.name
    };
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

    let mut json_bytes = serde_json::to_vec(&doc).map_err(|e| InteropError::Json(e.to_string()))?;
    // El chunk JSON se rellena con **espacios**, el binario con ceros. Lo dice
    // la especificación y algunos lectores estrictos lo comprueban.
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin.bytes;
    while bin_bytes.len() % 4 != 0 {
        bin_bytes.push(0);
    }

    let total = 12
        + 8
        + json_bytes.len()
        + if bin_bytes.is_empty() {
            0
        } else {
            8 + bin_bytes.len()
        };
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
    std::fs::write(path, bytes).map_err(|e| InteropError::Io {
        path: path.into(),
        source: e,
    })
}

/// Extrae el JSON de un `.glb`. Para tests y para inspeccionar sin herramientas.
pub fn glb_json(glb: &[u8]) -> Result<Value> {
    if glb.len() < 20 {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "glb demasiado corto".into(),
        });
    }
    let leer = |o: usize| u32::from_le_bytes([glb[o], glb[o + 1], glb[o + 2], glb[o + 3]]);
    if leer(0) != GLB_MAGIC {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "magic no es glTF".into(),
        });
    }
    let json_len = leer(12) as usize;
    if leer(16) != CHUNK_JSON {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "el primer chunk no es JSON".into(),
        });
    }
    let fin = 20 + json_len;
    if fin > glb.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "chunk JSON truncado".into(),
        });
    }
    serde_json::from_slice(&glb[20..fin]).map_err(|e| InteropError::Json(e.to_string()))
}

/// Lee un `.glb` y devuelve la malla.
///
/// # Conversiones inversas
/// Aplica las transformaciones inversas especificadas en `opts`:
/// - Si `opts.y_up`, convierte Y-up → Z-up.
/// - Si `opts.to_meters`, convierte metros → milímetros.
///
/// **Importante:** Los `opts` deben ser los mismos que se usaron para escribir el archivo.
/// La ida y vuelta (escribir con `opts` → leer con `opts`) cierra exacta.
pub fn read_glb(bytes: &[u8], opts: GltfOptions) -> Result<TriangleSoup> {
    // Parsear header y verificar estructura
    if bytes.len() < 20 {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "glb demasiado corto".into(),
        });
    }

    let leer_u32 =
        |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);

    if leer_u32(0) != GLB_MAGIC {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "magic no es glTF".into(),
        });
    }
    if leer_u32(4) != GLB_VERSION {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "version no es 2".into(),
        });
    }

    let total_len = leer_u32(8) as usize;
    if total_len != bytes.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "longitud declarada no coincide con el archivo".into(),
        });
    }

    // Leer JSON chunk
    let json_len = leer_u32(12) as usize;
    if leer_u32(16) != CHUNK_JSON {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "el primer chunk no es JSON".into(),
        });
    }
    let json_fin = 20 + json_len;
    if json_fin > bytes.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "chunk JSON truncado".into(),
        });
    }

    let json: Value = serde_json::from_slice(&bytes[20..json_fin])
        .map_err(|e| InteropError::Json(e.to_string()))?;

    // Leer BIN chunk
    if json_fin + 8 > bytes.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "header de BIN chunk truncado".into(),
        });
    }

    let bin_len = leer_u32(json_fin) as usize;
    if leer_u32(json_fin + 4) != CHUNK_BIN {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "el segundo chunk no es BIN".into(),
        });
    }

    let bin_start = json_fin + 8;
    let bin_end = bin_start + bin_len;
    if bin_end > bytes.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "chunk BIN truncado".into(),
        });
    }

    let buffer = &bytes[bin_start..bin_end];

    // Extraer accesores y bufferViews del JSON
    let accessors = json["accessors"]
        .as_array()
        .ok_or_else(|| InteropError::Malformed {
            line: 0,
            detail: "accessors no es un array".into(),
        })?;

    let buffer_views = json["bufferViews"]
        .as_array()
        .ok_or_else(|| InteropError::Malformed {
            line: 0,
            detail: "bufferViews no es un array".into(),
        })?;

    // Encontrar los índices de POSITION, NORMAL, TEXCOORD_0 e índices
    let primitives = json["meshes"][0]["primitives"]
        .as_array()
        .and_then(|p| p.first())
        .ok_or_else(|| InteropError::InvalidMesh("no hay primitives".into()))?;

    let attrs = primitives["attributes"]
        .as_object()
        .ok_or_else(|| InteropError::InvalidMesh("no hay attributes".into()))?;

    let idx_accessor_idx = primitives["indices"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("no hay índices".into()))?
        as usize;

    let pos_idx = attrs
        .get("POSITION")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| InteropError::InvalidMesh("no hay POSITION".into()))?
        as usize;

    let normal_idx = attrs
        .get("NORMAL")
        .and_then(|v| v.as_u64())
        .map(|i| i as usize);

    let uv_idx = attrs
        .get("TEXCOORD_0")
        .and_then(|v| v.as_u64())
        .map(|i| i as usize);

    // Leer datos de accesores
    let mut soup = TriangleSoup {
        name: json["meshes"][0]["name"]
            .as_str()
            .unwrap_or("malla")
            .to_string(),
        ..Default::default()
    };

    // Leer POSITION
    read_positions(&mut soup, buffer, &accessors[pos_idx], buffer_views, opts)?;

    // Leer NORMAL si existe
    if let Some(ni) = normal_idx {
        read_normals(&mut soup, buffer, &accessors[ni], buffer_views, opts)?;
    }

    // Leer TEXCOORD_0 si existe
    if let Some(ui) = uv_idx {
        read_uvs(&mut soup, buffer, &accessors[ui], buffer_views)?;
    }

    // Leer índices
    read_indices(
        &mut soup,
        buffer,
        &accessors[idx_accessor_idx],
        buffer_views,
    )?;

    soup.validate()?;
    Ok(soup)
}

/// Lee posiciones desde el accessor. Invierte las conversiones especificadas en `opts`.
fn read_positions(
    soup: &mut TriangleSoup,
    buffer: &[u8],
    accessor: &Value,
    buffer_views: &[Value],
    opts: GltfOptions,
) -> Result<()> {
    let buffer_view_idx = accessor["bufferView"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("POSITION sin bufferView".into()))?
        as usize;

    let buffer_view = buffer_views
        .get(buffer_view_idx)
        .ok_or_else(|| InteropError::InvalidMesh("bufferView out of range".into()))?;

    let byte_offset = buffer_view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let byte_length = buffer_view["byteLength"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("byteLength missing".into()))?
        as usize;

    let component_type = accessor["componentType"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("componentType missing".into()))?;

    let count = accessor["count"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("count missing".into()))? as usize;

    if component_type != COMPONENT_FLOAT as u64 {
        return Err(InteropError::Unsupported("componentType no es float32"));
    }

    if byte_offset + byte_length > buffer.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "POSITION data truncado".into(),
        });
    }

    let data = &buffer[byte_offset..byte_offset + byte_length];

    for i in 0..count {
        let off = i * 12; // 3 floats × 4 bytes
        if off + 12 > data.len() {
            return Err(InteropError::Malformed {
                line: 0,
                detail: "POSITION data truncado".into(),
            });
        }

        let x = f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64;
        let y =
            f32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]) as f64;
        let z = f32::from_le_bytes([data[off + 8], data[off + 9], data[off + 10], data[off + 11]])
            as f64;

        // Invertir las conversiones especificadas en opts.
        let v = DVec3::new(x, y, z);
        let v = opts.invertir(v);

        soup.positions.push(v);
    }

    Ok(())
}

/// Lee normales desde el accessor. Invierte la rotación de ejes especificada en `opts`.
fn read_normals(
    soup: &mut TriangleSoup,
    buffer: &[u8],
    accessor: &Value,
    buffer_views: &[Value],
    opts: GltfOptions,
) -> Result<()> {
    let buffer_view_idx = accessor["bufferView"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("NORMAL sin bufferView".into()))?
        as usize;

    let buffer_view = buffer_views
        .get(buffer_view_idx)
        .ok_or_else(|| InteropError::InvalidMesh("bufferView out of range".into()))?;

    let byte_offset = buffer_view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let byte_length = buffer_view["byteLength"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("byteLength missing".into()))?
        as usize;

    let component_type = accessor["componentType"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("componentType missing".into()))?;

    let count = accessor["count"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("count missing".into()))? as usize;

    if component_type != COMPONENT_FLOAT as u64 {
        return Err(InteropError::Unsupported("componentType no es float32"));
    }

    if byte_offset + byte_length > buffer.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "NORMAL data truncado".into(),
        });
    }

    let data = &buffer[byte_offset..byte_offset + byte_length];

    for i in 0..count {
        let off = i * 12;
        if off + 12 > data.len() {
            return Err(InteropError::Malformed {
                line: 0,
                detail: "NORMAL data truncado".into(),
            });
        }

        let x = f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64;
        let y =
            f32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]) as f64;
        let z = f32::from_le_bytes([data[off + 8], data[off + 9], data[off + 10], data[off + 11]])
            as f64;

        // Invertir la rotación especificada en opts.
        let v = DVec3::new(x, y, z);
        let v = opts.invertir_normal(v);

        soup.normals.push(v);
    }

    Ok(())
}

/// Lee UVs desde el accessor.
fn read_uvs(
    soup: &mut TriangleSoup,
    buffer: &[u8],
    accessor: &Value,
    buffer_views: &[Value],
) -> Result<()> {
    let buffer_view_idx = accessor["bufferView"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("TEXCOORD_0 sin bufferView".into()))?
        as usize;

    let buffer_view = buffer_views
        .get(buffer_view_idx)
        .ok_or_else(|| InteropError::InvalidMesh("bufferView out of range".into()))?;

    let byte_offset = buffer_view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let byte_length = buffer_view["byteLength"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("byteLength missing".into()))?
        as usize;

    let component_type = accessor["componentType"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("componentType missing".into()))?;

    let count = accessor["count"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("count missing".into()))? as usize;

    if component_type != COMPONENT_FLOAT as u64 {
        return Err(InteropError::Unsupported("componentType no es float32"));
    }

    if byte_offset + byte_length > buffer.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "TEXCOORD_0 data truncado".into(),
        });
    }

    let data = &buffer[byte_offset..byte_offset + byte_length];

    for i in 0..count {
        let off = i * 8; // 2 floats × 4 bytes
        if off + 8 > data.len() {
            return Err(InteropError::Malformed {
                line: 0,
                detail: "TEXCOORD_0 data truncado".into(),
            });
        }

        let u = f32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as f64;
        let v =
            f32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]) as f64;

        soup.uvs.push(DVec2::new(u, v));
    }

    Ok(())
}

/// Lee índices desde el accessor. Soporta u16 y u32.
fn read_indices(
    soup: &mut TriangleSoup,
    buffer: &[u8],
    accessor: &Value,
    buffer_views: &[Value],
) -> Result<()> {
    let buffer_view_idx = accessor["bufferView"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("indices sin bufferView".into()))?
        as usize;

    let buffer_view = buffer_views
        .get(buffer_view_idx)
        .ok_or_else(|| InteropError::InvalidMesh("bufferView out of range".into()))?;

    let byte_offset = buffer_view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let byte_length = buffer_view["byteLength"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("byteLength missing".into()))?
        as usize;

    let component_type = accessor["componentType"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("componentType missing".into()))?;

    let count = accessor["count"]
        .as_u64()
        .ok_or_else(|| InteropError::InvalidMesh("count missing".into()))? as usize;

    if byte_offset + byte_length > buffer.len() {
        return Err(InteropError::Malformed {
            line: 0,
            detail: "indices data truncado".into(),
        });
    }

    let data = &buffer[byte_offset..byte_offset + byte_length];

    match component_type {
        5123 => {
            // UNSIGNED_SHORT (u16)
            for i in 0..count {
                let off = i * 2;
                if off + 2 > data.len() {
                    return Err(InteropError::Malformed {
                        line: 0,
                        detail: "indices data truncado".into(),
                    });
                }
                let val = u16::from_le_bytes([data[off], data[off + 1]]) as u32;
                soup.indices.push(val);
            }
        }
        5125 => {
            // UNSIGNED_INT (u32)
            for i in 0..count {
                let off = i * 4;
                if off + 4 > data.len() {
                    return Err(InteropError::Malformed {
                        line: 0,
                        detail: "indices data truncado".into(),
                    });
                }
                let val =
                    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                soup.indices.push(val);
            }
        }
        _ => {
            return Err(InteropError::Unsupported(
                "componentType de índices no soportado",
            ));
        }
    }

    Ok(())
}

pub fn read_glb_file(path: impl AsRef<Path>, opts: GltfOptions) -> Result<TriangleSoup> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| InteropError::Io {
        path: path.into(),
        source: e,
    })?;
    read_glb(&bytes, opts)
}
