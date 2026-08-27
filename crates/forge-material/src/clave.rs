//! Clave de caché de permutación (`GeneratedShader::permutation_key`).
//!
//! Tiene que cumplir tres cosas a la vez:
//! - el mismo grafo produce siempre la misma clave;
//! - reordenar `graph.nodes` (o `graph.links`) sin tocar qué se conecta con
//!   qué produce la misma clave, porque el orden de almacenamiento en el
//!   `Vec` no es semántica del grafo;
//! - cambiar un literal (una constante, un valor por defecto) cambia la
//!   clave, porque es lo único que distinguiría a dos permutaciones de shader
//!   que necesitan compilarse por separado.
//!
//! La clave no se calcula sobre `graph.nodes` en su orden de almacenamiento
//! ni sobre los `id` que el usuario les puso: se calcula sobre
//! `analisis.orden` (el orden topológico, que depende solo de qué está
//! conectado con qué) usando como identidad de cada nodo su **posición** en
//! ese orden, no su `id`. Así, dos grafos con la misma topología pero ids
//! distintos —o el mismo grafo con `nodes` barajado— hashean igual.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use forge_material_api::{Node, NodeKind, Value};

use crate::analisis::{Analisis, Entrada};

pub(crate) fn calcular(nodos: &HashMap<u32, &Node>, analisis: &Analisis) -> u64 {
    // `DefaultHasher::new()` usa una semilla fija (a diferencia de
    // `RandomState`, que la sortea por proceso): es justo lo que hace falta
    // para que la clave sea estable entre ejecuciones, no solo dentro de una.
    let mut h = std::collections::hash_map::DefaultHasher::new();

    let indice: HashMap<u32, usize> =
        analisis.orden.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    for &id in &analisis.orden {
        let nodo = nodos[&id];
        nodo.kind.nombre().hash(&mut h);
        if let NodeKind::Constant = nodo.kind {
            if let Some(v) = nodo.constant {
                hash_valor(v, &mut h);
            }
        }
        for entrada in &analisis.entradas[&id] {
            match entrada {
                // El índice topológico, no el `id` de almacenamiento: dos
                // grafos isomorfos con ids distintos deben coincidir.
                Entrada::Nodo(origen) => {
                    0u8.hash(&mut h);
                    indice[origen].hash(&mut h);
                }
                Entrada::Literal(v) => {
                    1u8.hash(&mut h);
                    hash_valor(*v, &mut h);
                }
            }
        }
    }

    h.finish()
}

/// Hashea un valor por sus bits crudos (`to_bits`), no por su `Display`: dos
/// flotantes que se imprimen igual pero difieren en el último bit deben dar
/// claves distintas, y comparar bits evita además el caso patológico de
/// `NaN != NaN` bajo `PartialEq`.
fn hash_valor(v: Value, h: &mut impl Hasher) {
    match v {
        Value::Float(x) => {
            0u8.hash(h);
            x.to_bits().hash(h);
        }
        Value::Vec2([a, b]) => {
            1u8.hash(h);
            a.to_bits().hash(h);
            b.to_bits().hash(h);
        }
        Value::Vec3([a, b, c]) => {
            2u8.hash(h);
            a.to_bits().hash(h);
            b.to_bits().hash(h);
            c.to_bits().hash(h);
        }
        Value::Color([a, b, c]) => {
            3u8.hash(h);
            a.to_bits().hash(h);
            b.to_bits().hash(h);
            c.to_bits().hash(h);
        }
    }
}
