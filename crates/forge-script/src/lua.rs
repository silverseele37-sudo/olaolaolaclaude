//! Host de Lua embebido.
//!
//! # Por qué Lua y no Python dentro del proceso
//!
//! ADR-0006 lo decide y la mitad de la razón está en este archivo: un intérprete
//! embebido se puede **acotar**. Un script que entre en bucle infinito se corta
//! por número de instrucciones; uno que intente reservar un gigabyte se corta por
//! memoria. CPython embebido no ofrece ninguna de las dos cosas de forma
//! razonable, y por eso el cliente Python vive fuera del proceso hablando el
//! mismo protocolo de comandos.
//!
//! # Cómo se ve desde el script
//!
//! Una tabla global `forge` con **una función por comando**. No hay ninguna otra
//! puerta: el script no ve el documento, no ve la transacción y no ve el
//! registro de componentes. Si algo no se puede hacer llamando a `forge.*`, es
//! un hueco del bus — que es exactamente el banco de pruebas que pide ADR-0006.
//!
//! ```lua
//! forge.begin_group("tres cubos")
//! for i = 1, 3 do
//!   local e = forge.spawn("cubo " .. i)
//!   forge.set_transform(e, { tx = i * 10.0 })
//! end
//! forge.end_group()   -- un solo Ctrl+Z los borra los tres
//! ```
//!
//! Los identificadores viajan a Lua como cadenas hexadecimales de 32 caracteres
//! y no como números: un `EntityId` es un ULID de 128 bits y el `number` de Lua
//! es un `f64`, que perdería bits en silencio. Perder identidad en silencio es
//! peor que un error.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use forge_doc::{Document, EntityId, GeometryPayload, Transform};
use forge_math::{DQuat, DVec3};
use forge_store::BlobHash;
use mlua::{HookTriggers, Lua, Table, Value, VmState};

use crate::command::{Command, CommandBus, CommandError, CommandOutcome};

/// Cada cuántas instrucciones de la VM se comprueba el presupuesto.
///
/// Ni 1 (el hook dominaría el coste de ejecución) ni 10^6 (un bucle vacío
/// tardaría demasiado en notarse). Con 1000 el corte es imperceptible para un
/// humano y el sobrecoste no se mide.
const PASO_DEL_HOOK: u32 = 1_000;

/// Presupuesto de un script.
///
/// Los valores por defecto son generosos para un generador de geometría e
/// insuficientes para un bucle infinito, que es justo lo que se quiere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Instrucciones de la VM antes de abortar.
    pub max_instrucciones: u64,
    /// Bytes que el estado de Lua puede tener reservados a la vez.
    pub max_memoria_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_instrucciones: 20_000_000,
            max_memoria_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error(
        "el script superó el límite de {limite} instrucciones y se abortó. No se aplicó nada \
         de lo que quedaba: revisa el bucle, o sube `Limits::max_instrucciones` si el script \
         es legítimamente largo."
    )]
    LimiteDeInstrucciones { limite: u64 },

    #[error(
        "el script superó el límite de {limite} bytes de memoria y se abortó. Suele ser una \
         tabla que crece dentro de un bucle: acumula en el documento con comandos, no en Lua."
    )]
    LimiteDeMemoria { limite: usize },

    #[error("error de Lua: {0}")]
    Lua(String),

    #[error(transparent)]
    Comando(#[from] CommandError),
}

/// Resultado de ejecutar un script. Nunca un pánico: un script de terceros no
/// puede tumbar el editor.
pub type ScriptResult<T> = std::result::Result<T, ScriptError>;

/// Intérprete de Lua con la tabla `forge` conectada al bus.
pub struct LuaHost {
    lua: Lua,
    limites: Limits,
    /// Instrucciones consumidas por la ejecución en curso. Vive fuera del hook
    /// porque el hook tiene que ser `'static` y el host no.
    contador: Arc<AtomicU64>,
}

impl LuaHost {
    pub fn new(limites: Limits) -> ScriptResult<Self> {
        let lua = Lua::new();
        lua.set_memory_limit(limites.max_memoria_bytes)
            .map_err(err_lua)?;
        Ok(LuaHost {
            lua,
            limites,
            contador: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn limits(&self) -> Limits {
        self.limites
    }

    /// Instrucciones consumidas por la última ejecución.
    pub fn instrucciones_usadas(&self) -> u64 {
        self.contador.load(Ordering::Relaxed)
    }

    /// Ejecuta `src` contra `doc` a través de `bus`.
    ///
    /// El `bus` es el mismo que usa la interfaz: un script no tiene una vía
    /// privilegiada. Si el script deja un grupo abierto, el error sale de
    /// [`CommandBus::finish`] y sus comandos no se aplicaron.
    pub fn run(&self, doc: &mut Document, bus: &mut CommandBus, src: &str) -> ScriptResult<()> {
        self.armar_hook()?;
        let estado = RefCell::new(Puente {
            doc,
            bus,
            fallo: None,
        });

        let r = self.lua.scope(|scope| {
            let t = self.lua.create_table()?;

            macro_rules! fn_forge {
                ($nombre:literal, |$e:ident, $args:tt : $ty:ty| $cuerpo:expr) => {
                    t.set(
                        $nombre,
                        scope.create_function(|_, $args: $ty| {
                            let mut $e = estado.borrow_mut();
                            $cuerpo
                        })?,
                    )?;
                };
            }

            fn_forge!("spawn", |p, name: Option<String>| {
                let out = p.enviar(Command::Spawn { id: None, name })?;
                Ok(out.entity().map(id_a_lua))
            });
            fn_forge!("despawn", |p, id: String| {
                p.enviar(Command::Despawn {
                    entity: id_de_lua(&id)?,
                })?;
                Ok(())
            });
            fn_forge!("set_name", |p, (id, name): (String, String)| {
                p.enviar(Command::SetName {
                    entity: id_de_lua(&id)?,
                    name,
                })?;
                Ok(())
            });
            fn_forge!("set_visible", |p, (id, v): (String, bool)| {
                p.enviar(Command::SetVisible {
                    entity: id_de_lua(&id)?,
                    visible: v,
                })?;
                Ok(())
            });
            fn_forge!("set_transform", |p, (id, t): (String, Table)| {
                let transform = transform_de_lua(&t)?;
                p.enviar(Command::SetTransform {
                    entity: id_de_lua(&id)?,
                    transform,
                })?;
                Ok(())
            });
            fn_forge!(
                "set_parent",
                |p, (hijo, padre): (String, Option<String>)| {
                    let parent = match padre {
                        Some(s) => Some(id_de_lua(&s)?),
                        None => None,
                    };
                    p.enviar(Command::SetParent {
                        child: id_de_lua(&hijo)?,
                        parent,
                    })?;
                    Ok(())
                }
            );
            fn_forge!("set_geometry", |p,
                                       (id, kind, hex): (
                String,
                String,
                String
            )| {
                let payload = geometria_de_lua(&kind, &hex)?;
                p.enviar(Command::SetGeometry {
                    entity: id_de_lua(&id)?,
                    payload,
                })?;
                Ok(())
            });
            fn_forge!("clear_geometry", |p, id: String| {
                p.enviar(Command::ClearGeometry {
                    entity: id_de_lua(&id)?,
                })?;
                Ok(())
            });
            fn_forge!("undo", |p, (): ()| {
                p.enviar(Command::Undo)?;
                Ok(())
            });
            fn_forge!("redo", |p, (): ()| {
                p.enviar(Command::Redo)?;
                Ok(())
            });
            fn_forge!("begin_group", |p, label: String| {
                p.enviar(Command::BeginGroup { label })?;
                Ok(())
            });
            fn_forge!("end_group", |p, (): ()| {
                p.enviar(Command::EndGroup)?;
                Ok(())
            });

            self.lua.globals().set("forge", t)?;
            self.lua.load(src).exec()
        });

        self.lua.remove_hook();
        // La tabla se retira siempre: sus funciones mueren con el `scope` y
        // dejarla accesible convertiría una llamada posterior en un error
        // críptico en vez de en "forge is nil".
        let _ = self.lua.globals().set("forge", Value::Nil);

        let fallo = estado.into_inner().fallo;
        match r {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(c) = fallo {
                    return Err(ScriptError::Comando(c));
                }
                Err(self.clasificar(e))
            }
        }
    }

    fn armar_hook(&self) -> ScriptResult<()> {
        self.contador.store(0, Ordering::Relaxed);
        let contador = Arc::clone(&self.contador);
        let limite = self.limites.max_instrucciones;
        self.lua
            .set_hook(
                HookTriggers::default().every_nth_instruction(PASO_DEL_HOOK),
                move |_, _| {
                    let usadas = contador.fetch_add(PASO_DEL_HOOK as u64, Ordering::Relaxed)
                        + PASO_DEL_HOOK as u64;
                    if usadas > limite {
                        // Devolver `Err` desde el hook aborta la VM: es la única vía
                        // de detener un bucle sin `longjmp` ni matar el hilo.
                        return Err(mlua::Error::RuntimeError(MARCA_INSTRUCCIONES.into()));
                    }
                    Ok(VmState::Continue)
                },
            )
            .map_err(err_lua)?;
        Ok(())
    }

    fn clasificar(&self, e: mlua::Error) -> ScriptError {
        if matches!(e, mlua::Error::MemoryError(_)) {
            return ScriptError::LimiteDeMemoria {
                limite: self.limites.max_memoria_bytes,
            };
        }
        let txt = e.to_string();
        if txt.contains(MARCA_INSTRUCCIONES) {
            return ScriptError::LimiteDeInstrucciones {
                limite: self.limites.max_instrucciones,
            };
        }
        if txt.contains("not enough memory") {
            return ScriptError::LimiteDeMemoria {
                limite: self.limites.max_memoria_bytes,
            };
        }
        ScriptError::Lua(txt)
    }
}

/// Marca del abort por instrucciones. Va en el texto del error de Lua porque es
/// lo único que sobrevive intacto al desenrollado de la VM.
const MARCA_INSTRUCCIONES: &str = "forge:limite-de-instrucciones";

/// Lo que las funciones de `forge` tienen prestado durante la ejecución.
struct Puente<'a> {
    doc: &'a mut Document,
    bus: &'a mut CommandBus,
    /// El error del bus se guarda aquí además de propagarse como error de Lua:
    /// el texto de un error de Lua ya lleva la traza del script pegada y
    /// perdería la variante tipada, que es lo que el llamante quiere ver.
    fallo: Option<CommandError>,
}

impl Puente<'_> {
    fn enviar(&mut self, cmd: Command) -> mlua::Result<CommandOutcome> {
        match self.bus.apply(self.doc, cmd) {
            Ok(o) => Ok(o),
            Err(e) => {
                let texto = e.to_string();
                self.fallo = Some(e);
                Err(mlua::Error::RuntimeError(texto))
            }
        }
    }
}

fn err_lua(e: mlua::Error) -> ScriptError {
    ScriptError::Lua(e.to_string())
}

fn id_a_lua(e: EntityId) -> String {
    format!("{:032x}", e.0 .0)
}

fn id_de_lua(s: &str) -> mlua::Result<EntityId> {
    u128::from_str_radix(s, 16)
        .map(EntityId::from_u128)
        .map_err(|_| {
            mlua::Error::RuntimeError(format!(
                "«{s}» no es un id de entidad. Usa el valor que devolvió forge.spawn(), \
             una cadena hexadecimal de 32 caracteres."
            ))
        })
}

fn geometria_de_lua(kind: &str, hex: &str) -> mlua::Result<GeometryPayload> {
    let h = BlobHash::from_hex(hex).map_err(|e| {
        mlua::Error::RuntimeError(format!(
            "hash de blob invalido: {e}. Debe ser el hex de 64 caracteres que devuelve el \
             almacén de blobs."
        ))
    })?;
    match kind {
        "sketch" => Ok(GeometryPayload::Sketch(h)),
        "curve" => Ok(GeometryPayload::Curve(h)),
        "brep" => Ok(GeometryPayload::Brep(h)),
        "mesh" => Ok(GeometryPayload::Mesh(h)),
        "pointcloud" => Ok(GeometryPayload::PointCloud(h)),
        otro => Err(mlua::Error::RuntimeError(format!(
            "dominio de geometría desconocido «{otro}»: usa sketch, curve, brep, mesh o \
             pointcloud."
        ))),
    }
}

/// Tabla `{ tx, ty, tz, rx, ry, rz, rw, sx, sy, sz }`, todos opcionales.
/// Lo que falte queda en la identidad: escribir una traslación no debería
/// obligar a repetir el cuaternión.
fn transform_de_lua(t: &Table) -> mlua::Result<Transform> {
    let num = |k: &str, def: f64| -> mlua::Result<f64> {
        match t.get::<Value>(k)? {
            Value::Nil => Ok(def),
            v => f64::from_lua(v, k),
        }
    };
    Ok(Transform {
        translation: DVec3::new(num("tx", 0.0)?, num("ty", 0.0)?, num("tz", 0.0)?),
        rotation: DQuat::from_xyzw(
            num("rx", 0.0)?,
            num("ry", 0.0)?,
            num("rz", 0.0)?,
            num("rw", 1.0)?,
        )
        .normalize(),
        scale: DVec3::new(num("sx", 1.0)?, num("sy", 1.0)?, num("sz", 1.0)?),
    })
}

/// Conversión con mensaje propio: el de `mlua` no dice qué campo falló.
trait NumeroDeLua: Sized {
    fn from_lua(v: Value, campo: &str) -> mlua::Result<Self>;
}

impl NumeroDeLua for f64 {
    fn from_lua(v: Value, campo: &str) -> mlua::Result<Self> {
        v.as_f64().ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "el campo `{campo}` de la transformada no es un número"
            ))
        })
    }
}
