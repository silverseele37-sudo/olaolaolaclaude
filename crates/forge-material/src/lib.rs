//! Generador de WGSL para el grafo de materiales de `forge-material-api`.
//!
//! # Por qué existe (ADR-0005)
//!
//! MaterialX resuelve el modelo de grafo — nodos, tipos de socket, enlaces —
//! y es un formato de intercambio real que otras herramientas leen. Pero su
//! generador de código emite GLSL, OSL, MDL y MSL, nunca WGSL, y traducir esos
//! lenguajes a WGSL con la pasarela de `naga` es posible en general y frágil
//! específicamente para código ya generado (construcciones que un humano no
//! escribiría y que la pasarela no anticipa). La decisión es usar MaterialX
//! como modelo de documento y escribir aquí un generador de WGSL propio, pero
//! solo para el subconjunto acotado que describe `NodeKind` — catorce
//! variantes, lista cerrada a propósito en `forge-material-api`.
//!
//! # Cómo está organizado
//!
//! - `analisis` valida el grafo y lo resuelve a un orden topológico más las
//!   entradas de cada nodo — ciclos, tipos, entradas sin resolver y ausencia
//!   de salida se detectan aquí, **antes** de imprimir una sola línea de
//!   WGSL.
//! - `emitir` traduce ese análisis, ya válido, a texto WGSL: un `let` por
//!   nodo en orden topológico, y nada más — no repite ninguna validación.
//! - `clave` deriva la clave de caché de permutación del mismo análisis.
//!
//! Separar "validar" de "imprimir" es lo que permite que un nodo roto se
//! localice solo: si el WGSL no compila, el bug está en `emitir`; si el grafo
//! se acepta cuando no debería (o al revés), está en `analisis`.
//!
//! # Verificación sin GPU
//!
//! Los tests de este crate parsean cada WGSL generado con el frontend de
//! `naga` y lo pasan por su `Validator`. Un generador que produce texto con
//! forma de WGSL pero que no compila es peor que no tener generador: falla en
//! el momento más caro (en el usuario, en tiempo de render) en vez del más
//! barato (`cargo test`).

mod analisis;
mod clave;
mod emitir;

use std::collections::HashMap;

use forge_material_api::{GeneratedShader, MaterialGraph, Node, Result, ShaderGenerator};

/// El único [`ShaderGenerator`] de este crate: MaterialX (o cualquier otro
/// origen) hacia el subconjunto de WGSL descrito en el módulo.
#[derive(Debug, Default, Clone, Copy)]
pub struct WgslGenerator;

impl ShaderGenerator for WgslGenerator {
    fn name(&self) -> &'static str {
        "forge-material::wgsl"
    }

    fn generate(&self, graph: &MaterialGraph) -> Result<GeneratedShader> {
        // Índice por id una sola vez; tanto el análisis como la emisión lo
        // necesitan y reconstruirlo dos veces sería trabajo (y una fuente de
        // inconsistencias) de más.
        let nodos: HashMap<u32, &Node> = graph.nodes.iter().map(|n| (n.id, n)).collect();

        let analisis = analisis::analizar(graph, &nodos)?;
        let wgsl = emitir::emitir(graph, &analisis, &nodos);
        let permutation_key = clave::calcular(&nodos, &analisis);

        Ok(GeneratedShader { wgsl, permutation_key, nodes_used: analisis.orden.len() })
    }
}
