//! Validación del grafo y resolución de sus enlaces, antes de generar nada.
//!
//! Todo lo que puede fallar en un grafo de materiales falla aquí, de una vez:
//! ciclos, tipos incompatibles, entradas sin resolver, ausencia de salida. El
//! generador de WGSL (`crate::emitir`) recibe ya un [`Analisis`] resuelto y no
//! vuelve a comprobar nada — así el "análisis" y la "escritura de texto" no se
//! entrelazan, y un cambio en la sintaxis WGSL no puede esconder un bug de
//! validación ni al revés.

use std::collections::HashMap;

use forge_material_api::{MaterialError, MaterialGraph, Node, NodeKind, Result, SocketType, Value};

/// De dónde sale el valor que alimenta una entrada ya resuelta.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Entrada {
    /// Enlazada a la salida de otro nodo, por id.
    Nodo(u32),
    /// Sin enlace: se usa el valor por defecto declarado en el nodo.
    Literal(Value),
}

/// El grafo, ya comprobado y listo para imprimirse como WGSL.
pub(crate) struct Analisis {
    /// Orden topológico (dependencias antes que dependientes), restringido a
    /// los nodos alcanzables desde la salida. Los inalcanzables simplemente no
    /// aparecen aquí — es como se evita generar código muerto.
    pub orden: Vec<u32>,
    /// Para cada nodo usado, el origen resuelto de cada una de sus entradas,
    /// en el mismo orden que `NodeKind::inputs()`.
    pub entradas: HashMap<u32, Vec<Entrada>>,
}

/// Estado de un nodo durante el recorrido en profundidad, para detectar
/// ciclos sin colgarse: si se vuelve a entrar a un nodo que sigue
/// `Visitando` (todavía en la pila de llamadas), hay un ciclo.
enum Estado {
    Visitando,
    Hecho,
}

/// Tipo de salida real de un nodo.
///
/// Para todo salvo [`NodeKind::Constant`] coincide con
/// [`NodeKind::output_type`], que es estructural (depende solo de la clase de
/// nodo). Una constante es la excepción: su tipo de salida lo decide el
/// literal que guarda (`Float`, `Vec2`, `Vec3` o `Color`), no una convención
/// fija — así una constante de tipo `Color` puede alimentar `base_color` y una
/// de tipo `Float` puede alimentar `metallic`, y una mezcla incompatible (por
/// ejemplo, un `Float` hacia una entrada `Vec3`) se detecta como
/// `TypeMismatch` en vez de colarse como un `vec3<f32>` cualquiera.
pub(crate) fn tipo_salida(nodo: &Node) -> SocketType {
    match nodo.kind {
        // Si `constant` es `None` el nodo es inválido de todos modos (se
        // reporta como `MissingInput` en `recorrer`); el `Vec3` de aquí es
        // solo un valor de repliegue para no tener que propagar un `Option`.
        NodeKind::Constant => nodo
            .constant
            .map(Value::socket_type)
            .unwrap_or(SocketType::Vec3),
        otro => otro.output_type(),
    }
}

/// Valida y resuelve el grafo completo a partir de su nodo de salida.
pub(crate) fn analizar(graph: &MaterialGraph, nodos: &HashMap<u32, &Node>) -> Result<Analisis> {
    let salida_id = graph.output.ok_or(MaterialError::NoOutput)?;
    let salida = nodos
        .get(&salida_id)
        .ok_or(MaterialError::UnknownNode(salida_id))?;
    if salida.kind != NodeKind::StandardSurface {
        // Un grafo cuya salida no es una superficie estándar no tiene, en la
        // práctica, una salida válida: se reporta igual que "sin salida" en
        // vez de inventar una variante de error que el contrato no declara.
        return Err(MaterialError::NoOutput);
    }

    let mut estado = HashMap::new();
    let mut orden = Vec::new();
    let mut entradas = HashMap::new();
    recorrer(
        salida_id,
        graph,
        nodos,
        &mut estado,
        &mut orden,
        &mut entradas,
    )?;
    Ok(Analisis { orden, entradas })
}

/// Recorrido en profundidad desde `id` hacia sus dependencias, que además:
/// - detecta ciclos (estado `Visitando` revisitado),
/// - resuelve cada entrada a un enlace o a un valor por defecto,
/// - comprueba tipos en cada resolución,
/// - y deja en `orden` un orden topológico válido para generar código.
///
/// Solo visita nodos alcanzables desde la salida: un nodo sin conexión a la
/// salida nunca se llama a través de esta función, así que sus posibles
/// problemas (entradas sin resolver, tipos incompatibles) no bloquean la
/// generación. Es la lectura literal de "un nodo inalcanzable no es un
/// error".
fn recorrer(
    id: u32,
    graph: &MaterialGraph,
    nodos: &HashMap<u32, &Node>,
    estado: &mut HashMap<u32, Estado>,
    orden: &mut Vec<u32>,
    entradas: &mut HashMap<u32, Vec<Entrada>>,
) -> Result<()> {
    match estado.get(&id) {
        Some(Estado::Hecho) => return Ok(()),
        Some(Estado::Visitando) => return Err(MaterialError::Cycle(id)),
        None => {}
    }
    let nodo = *nodos.get(&id).ok_or(MaterialError::UnknownNode(id))?;
    estado.insert(id, Estado::Visitando);

    if let NodeKind::Constant = nodo.kind {
        if nodo.constant.is_none() {
            return Err(MaterialError::MissingInput {
                node: id,
                input: "constant",
            });
        }
    }

    let mut resueltas = Vec::with_capacity(nodo.kind.inputs().len());
    for &(nombre, tipo) in nodo.kind.inputs() {
        let entrada = resolver_entrada(graph, nodo, nombre, tipo, nodos)?;
        if let Entrada::Nodo(origen_id) = entrada {
            recorrer(origen_id, graph, nodos, estado, orden, entradas)?;
        }
        resueltas.push(entrada);
    }

    entradas.insert(id, resueltas);
    estado.insert(id, Estado::Hecho);
    orden.push(id);
    Ok(())
}

/// Resuelve una entrada nombrada de `nodo`: primero busca un enlace, luego un
/// valor por defecto, y si no encuentra ninguno de los dos es `MissingInput`.
fn resolver_entrada(
    graph: &MaterialGraph,
    nodo: &Node,
    nombre: &'static str,
    tipo_esperado: SocketType,
    nodos: &HashMap<u32, &Node>,
) -> Result<Entrada> {
    if let Some(enlace) = graph
        .links
        .iter()
        .find(|l| l.to == nodo.id && l.to_input == nombre)
    {
        let origen = *nodos
            .get(&enlace.from)
            .ok_or(MaterialError::UnknownNode(enlace.from))?;
        let tipo_origen = tipo_salida(origen);
        if !tipo_origen.compatible_with(tipo_esperado) {
            return Err(MaterialError::TypeMismatch {
                from: tipo_origen,
                to: tipo_esperado,
            });
        }
        return Ok(Entrada::Nodo(origen.id));
    }

    if let Some((_, valor)) = nodo.defaults.iter().find(|(n, _)| n == nombre) {
        let tipo_valor = valor.socket_type();
        if !tipo_valor.compatible_with(tipo_esperado) {
            return Err(MaterialError::TypeMismatch {
                from: tipo_valor,
                to: tipo_esperado,
            });
        }
        return Ok(Entrada::Literal(*valor));
    }

    Err(MaterialError::MissingInput {
        node: nodo.id,
        input: nombre,
    })
}
