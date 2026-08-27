//! El bus de comandos y el host de scripting.
//!
//! # Por qué este crate es el que más importa
//!
//! ADR-0006 decide que la API de extensión de v1 **es el bus de comandos**, no
//! las estructuras internas. La interfaz, los scripts Lua, el cliente Python por
//! IPC y —en v2— los plugins WebAssembly hablan todos por aquí. La consecuencia
//! práctica es la que dice el ADR: *si algo no se puede hacer por el bus, es un
//! hueco de la API*. Este crate es el banco de pruebas de esa afirmación, y lo
//! que sigue es lo que se encontró al construirlo.
//!
//! # Huecos encontrados en §9 de `01-contratos.md`
//!
//! 1. **§9 no puede crear una entidad.** Todas sus variantes de creación
//!    (`CreateSketch`, `AddFeature`, `ImportAsset`) fabrican geometría de un
//!    pilar. No hay forma de crear una entidad vacía, renombrarla, moverla,
//!    ocultarla, emparentarla ni borrarla — y son exactamente las operaciones
//!    que el árbol de escena de la interfaz necesita en su primer día. Sin
//!    ellas, la interfaz tendría que tocar `Transaction` directamente y el bus
//!    dejaría de ser *la* API. Aquí se añadieron como [`Command::Spawn`],
//!    [`Command::Despawn`], [`Command::SetName`], [`Command::SetTransform`],
//!    [`Command::SetVisible`], [`Command::SetParent`],
//!    [`Command::SetGeometry`] y [`Command::ClearGeometry`].
//! 2. **§9 no dice de dónde sale un `EntityId`.** Los comandos reciben ids pero
//!    ninguno los produce, y un `CreateSketch` que genera el id por dentro hace
//!    que el log **no sea reproducible**: cada reproducción crearía entidades
//!    distintas y la huella final no coincidiría. El bus lo resuelve grabando
//!    siempre el id ya resuelto (ver [`Command::Spawn`]), pero la regla debería
//!    estar en el contrato: *todo comando que cree algo lleva su identidad en el
//!    propio comando*.
//! 3. **§9 no tiene consultas.** El bus solo escribe. Un script que quiera
//!    "duplicar la entidad seleccionada" o "poner a todas las visibles la
//!    escala 2" necesita leer, y hoy tiene que recibir un `Snapshot` por un
//!    canal que no es el bus. Para Lua en proceso se disimula; para el cliente
//!    Python por IPC **no hay forma**: el protocolo de comandos por sí solo no
//!    permite escribir un script no trivial.
//! 4. **§9 no puede meter bytes en el almacén de blobs.** `ImportAsset { path }`
//!    supone que el proceso que ejecuta el comando ve el mismo sistema de
//!    archivos — falso para el cliente Python remoto y falso para un plugin WASM
//!    aislado. Falta un comando que transporte contenido (`PutBlob { bytes }` →
//!    `BlobHash`), y sin él [`Command::SetGeometry`] solo puede apuntar a blobs
//!    que ya existan.
//! 5. **§9 no define el error.** Es la mitad del contrato de una API pública y
//!    no aparece. Ver [`CommandError`].
//! 6. **§9 no dice qué devuelve un comando.** Sin resultado tipado, `Spawn` no
//!    puede decir qué creó. Ver [`CommandOutcome`].
//! 7. **Los grupos no se anidan y §9 no lo dice.** Dos macros que se llamen
//!    entre sí abrirían grupos anidados y la entrada de undo sería ambigua.
//!    Aquí es un error explícito ([`CommandError::GrupoAnidado`]); el contrato
//!    debería decidirlo, no dejarlo al implementador.
//! 8. **`Undo` como comando del mismo enum es una trampa.** Deshacer no es una
//!    edición: no puede formar parte de un grupo y no puede ir dentro de una
//!    transacción. Van juntos en `Command` porque §9 los pone juntos, pero el
//!    bus tiene que rechazarlos por dentro ([`CommandError::NoAgrupable`]).
//!
//! # Comandos de §9 que este crate deja fuera, y por qué
//!
//! La tabla de fronteras de `tests/arquitectura.rs` permite a `forge-script`
//! depender de `forge-math`, `forge-doc` y `forge-store`. **De ahí sale el
//! hallazgo más incómodo:** casi todos los comandos de §9 llevan un payload
//! definido en un crate que el bus no puede nombrar.
//!
//! | Comando | Payload | Vive en | Estado |
//! |---|---|---|---|
//! | `CreateSketch` / `AddConstraint` / `SetDimension` | `PlaneRef`, `Constraint`, `DimId` | `forge-kernel-api` | fuera: frontera |
//! | `AddFeature` / `SuppressFeature` / `ReorderFeature` | `FeatureSpec` | `forge-param` (no existe) | fuera: sin pilar |
//! | `ConvertToMesh` | `TessellationParams` | `forge-kernel-api` | fuera: frontera |
//! | `EditMesh` / `PushModifier` | `MeshOp`, `ModifierSpec` | `forge-mesh` | fuera: frontera |
//! | `SetMaterial` / `EditMaterialGraph` | `MaterialId`, `GraphEdit` | `forge-material-api` | fuera: frontera |
//! | `ImportAsset` / `TagAsset` | `AssetMeta`, `AssetId` | `forge-assets` (no existe) | fuera: sin pilar |
//! | `Undo` / `Redo` / `BeginGroup` / `EndGroup` | — | `forge-doc` | **implementados** |
//!
//! El problema no es que falten pilares: es que **el bus, tal como está escrito
//! en §9, obliga a que quien lo declara dependa de los cuatro pilares a la vez**
//! — justo lo que ADR-0006 prohíbe. Hay dos salidas y el proyecto tiene que
//! elegir una antes de que existan los pilares:
//!
//! - **(a) Los payloads bajan a crates de contratos.** `Constraint` y
//!   `TessellationParams` ya viven en `forge-kernel-api`; harían falta
//!   `forge-mesh-api` y `forge-assets-api`, y `forge-script` dependería de los
//!   contratos, nunca de las implementaciones. Es la coherente con ADR-0006.
//! - **(b) El bus se parte en dos niveles**: un `Command` de núcleo (esto) más
//!   un sobre opaco `Pillar { pilar: &str, payload: Vec<u8> }` que cada pilar
//!   registra y decodifica. Mantiene el crate del bus sin dependencias, pero
//!   pierde el tipado en la frontera, que es lo que hoy hace que un comando mal
//!   formado no compile.
//!
//! Duplicar los tipos dentro de `forge-script` —tercera opción tentadora— no se
//! hizo a propósito: dos definiciones de `TessellationParams` divergen, y lo
//! harían justo en el formato serializado que usan el IPC y los logs.
//!
//! # Límites de ejecución de Lua
//!
//! Ver [`lua`]. Están porque son media razón de que ADR-0006 eligiera Lua
//! embebido en vez de CPython: un script en bucle infinito se corta, no cuelga
//! el editor.

mod command;

pub use command::{Command, CommandBus, CommandError, CommandLog, CommandOutcome, Result};

#[cfg(feature = "lua")]
pub mod lua;

#[cfg(feature = "lua")]
pub use lua::{Limits, LuaHost, ScriptError};
