//! Contrato del grafo de materiales.
//!
//! # La trampa que este crate existe para evitar
//!
//! MaterialX resuelve muy bien la parte aburrida: un modelo de grafo bien
//! especificado, una biblioteca estándar de nodos y un formato que otras
//! herramientas leen. Su generador de código, en cambio, emite GLSL, OSL, MDL y
//! MSL — **no WGSL**. Traducir GLSL a WGSL con la pasarela de `naga` es posible
//! y frágil para código generado.
//!
//! Decisión (ADR-0005): adoptar MaterialX como **modelo de documento e
//! intercambio**, y generar WGSL propio para un **subconjunto acotado** de
//! nodos.
//!
//! # La lista de nodos es parte del contrato público
//!
//! [`NodeKind`] es esa lista. Está cerrada a propósito: un subconjunto que
//! funciona de verdad vale más que soporte nominal completo con casos rotos, y
//! el riesgo R5 —«el generador crece sin control»— se controla obligando a que
//! añadir un nodo sea tocar este enum, o sea una decisión visible en el diff.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MaterialError {
    #[error("el grafo tiene un ciclo que pasa por el nodo {0}")]
    Cycle(u32),
    #[error("el nodo {node} no tiene entrada `{input}` conectada ni valor por defecto")]
    MissingInput { node: u32, input: &'static str },
    #[error("tipos incompatibles al conectar {from:?} -> {to:?}")]
    TypeMismatch { from: SocketType, to: SocketType },
    #[error("referencia a un nodo inexistente: {0}")]
    UnknownNode(u32),
    #[error("el grafo no tiene nodo de salida")]
    NoOutput,
    #[error("nodo no soportado por el generador de WGSL: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, MaterialError>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum SocketType {
    Float,
    Vec2,
    Vec3,
    /// Color lineal. Distinto de `Vec3` a propósito: mezclar colores con
    /// vectores es el origen de la mitad de los bugs de espacio de color, y el
    /// tipo lo hace imposible sin una conversión explícita.
    Color,
}

impl SocketType {
    pub fn wgsl(self) -> &'static str {
        match self {
            SocketType::Float => "f32",
            SocketType::Vec2 => "vec2<f32>",
            SocketType::Vec3 | SocketType::Color => "vec3<f32>",
        }
    }
    /// Si un valor de `self` puede alimentar una entrada de `to`.
    pub fn compatible_with(self, to: SocketType) -> bool {
        self == to
            || matches!(
                (self, to),
                (SocketType::Vec3, SocketType::Color) | (SocketType::Color, SocketType::Vec3)
            )
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Value {
    Float(f32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Color([f32; 3]),
}

impl Value {
    pub fn socket_type(self) -> SocketType {
        match self {
            Value::Float(_) => SocketType::Float,
            Value::Vec2(_) => SocketType::Vec2,
            Value::Vec3(_) => SocketType::Vec3,
            Value::Color(_) => SocketType::Color,
        }
    }
}

/// **La lista cerrada.** Añadir una variante es una decisión de producto: entra
/// en la documentación pública de nodos soportados y en el generador.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum NodeKind {
    // --- fuentes ---
    Constant,
    /// Coordenada UV interpolada.
    TexCoord,
    /// Normal en espacio mundo.
    Normal,
    /// Posición en espacio mundo.
    Position,
    // --- matemáticas ---
    Add,
    Multiply,
    Mix,
    Clamp,
    /// `a` elevado a `b`.
    Power,
    DotProduct,
    /// Reasigna de `[in_lo, in_hi]` a `[out_lo, out_hi]`.
    Remap,
    // --- procedurales ---
    /// Damero, para comprobar UVs sin depender de una textura.
    Checker,
    /// Rampa vertical en `v`.
    Gradient,
    // --- salida ---
    /// Superficie estándar metallic-roughness, compatible con glTF.
    StandardSurface,
}

impl NodeKind {
    pub fn nombre(self) -> &'static str {
        match self {
            NodeKind::Constant => "constant",
            NodeKind::TexCoord => "texcoord",
            NodeKind::Normal => "normal",
            NodeKind::Position => "position",
            NodeKind::Add => "add",
            NodeKind::Multiply => "multiply",
            NodeKind::Mix => "mix",
            NodeKind::Clamp => "clamp",
            NodeKind::Power => "power",
            NodeKind::DotProduct => "dotproduct",
            NodeKind::Remap => "remap",
            NodeKind::Checker => "checker",
            NodeKind::Gradient => "gradient",
            NodeKind::StandardSurface => "standard_surface",
        }
    }

    /// Nombres de las entradas, en orden. Vacío para las fuentes.
    pub fn inputs(self) -> &'static [(&'static str, SocketType)] {
        match self {
            NodeKind::Constant | NodeKind::TexCoord | NodeKind::Normal | NodeKind::Position => &[],
            NodeKind::Add | NodeKind::Multiply => {
                &[("a", SocketType::Vec3), ("b", SocketType::Vec3)]
            }
            NodeKind::Mix => &[
                ("fg", SocketType::Vec3),
                ("bg", SocketType::Vec3),
                ("mix", SocketType::Float),
            ],
            NodeKind::Clamp => &[
                ("in", SocketType::Vec3),
                ("low", SocketType::Float),
                ("high", SocketType::Float),
            ],
            NodeKind::Power => &[("a", SocketType::Vec3), ("b", SocketType::Float)],
            NodeKind::DotProduct => &[("a", SocketType::Vec3), ("b", SocketType::Vec3)],
            NodeKind::Remap => &[
                ("in", SocketType::Float),
                ("in_lo", SocketType::Float),
                ("in_hi", SocketType::Float),
                ("out_lo", SocketType::Float),
                ("out_hi", SocketType::Float),
            ],
            NodeKind::Checker => &[("uv", SocketType::Vec2), ("scale", SocketType::Float)],
            NodeKind::Gradient => &[("uv", SocketType::Vec2)],
            NodeKind::StandardSurface => &[
                ("base_color", SocketType::Color),
                ("metallic", SocketType::Float),
                ("roughness", SocketType::Float),
                ("emissive", SocketType::Color),
            ],
        }
    }

    pub fn output_type(self) -> SocketType {
        match self {
            NodeKind::TexCoord => SocketType::Vec2,
            NodeKind::Normal | NodeKind::Position => SocketType::Vec3,
            NodeKind::DotProduct | NodeKind::Remap | NodeKind::Gradient => SocketType::Float,
            NodeKind::Checker => SocketType::Float,
            NodeKind::Constant => SocketType::Vec3,
            NodeKind::StandardSurface => SocketType::Color,
            _ => SocketType::Vec3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub kind: NodeKind,
    /// Valor literal de cada entrada no conectada, por nombre.
    pub defaults: Vec<(String, Value)>,
    /// Solo para [`NodeKind::Constant`].
    pub constant: Option<Value>,
}

/// Enlace: la salida de `from` alimenta la entrada `to_input` de `to`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub from: u32,
    pub to: u32,
    pub to_input: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialGraph {
    pub name: String,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// El nodo cuya salida es el material. Debe ser un [`NodeKind::StandardSurface`].
    pub output: Option<u32>,
}

/// Código WGSL generado, listo para compilar.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedShader {
    pub wgsl: String,
    /// Clave de caché: mismo grafo, mismo shader, sin recompilar.
    pub permutation_key: u64,
    pub nodes_used: usize,
}

/// Genera WGSL a partir de un grafo.
///
/// Que esto sea un trait y no una función suelta importa: el generador se puede
/// sustituir sin tocar el modelo de datos, y el modelo de datos es lo que se
/// guarda en el archivo.
pub trait ShaderGenerator {
    fn name(&self) -> &'static str;
    fn generate(&self, graph: &MaterialGraph) -> Result<GeneratedShader>;
}
