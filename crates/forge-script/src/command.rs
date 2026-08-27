//! El bus de comandos.
//!
//! Un [`Command`] es un dato: se serializa, se manda por un socket, se guarda en
//! un archivo y se vuelve a aplicar. Esa es la propiedad de la que dependen el
//! cliente Python por IPC, las macros, el modo batch y el `replay` de los tests
//! (ADR-0006). Todo lo demás de este módulo existe para no romperla.

use serde::{Deserialize, Serialize};

use forge_doc::{
    Document, EntityId, Geometry, GeometryPayload, Name, Parent, Snapshot, Transaction, Transform,
    VersionId, Visible,
};

// ---------------------------------------------------------------------------
// Comandos
// ---------------------------------------------------------------------------

/// La API pública de FORGE.
///
/// # Qué hay aquí y qué no
///
/// Las variantes de §9 de `01-contratos.md` que dependen de un pilar que aún no
/// existe —o que este crate no puede nombrar sin romper la tabla de fronteras de
/// `tests/arquitectura.rs`— **no están**. La lista completa y el porqué está en
/// la documentación del crate. Lo que queda es el núcleo de escena, que §9 no
/// enumera y sin el cual ni la propia interfaz podría funcionar: ese hueco es un
/// hallazgo, no un descuido.
///
/// # Por qué los ids viajan explícitos
///
/// [`Command::Spawn`] admite `id: None` por comodidad del llamante, pero el bus
/// **resuelve el id antes de aplicar y antes de grabar**. Un log con ids
/// implícitos genera entidades distintas en cada reproducción y la huella final
/// no coincide: dejaría de ser un log reproducible, que es justo lo que se le
/// pide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    // --- núcleo de escena ---
    /// Crea una entidad. Con `id: None` el bus asigna uno y lo devuelve.
    Spawn {
        id: Option<EntityId>,
        name: Option<String>,
    },
    /// Borra una entidad y todos sus componentes.
    Despawn { entity: EntityId },
    SetName {
        entity: EntityId,
        name: String,
    },
    SetTransform {
        entity: EntityId,
        transform: Transform,
    },
    SetVisible {
        entity: EntityId,
        visible: bool,
    },
    /// `parent: None` desengancha la entidad y la deja en la raíz.
    SetParent {
        child: EntityId,
        parent: Option<EntityId>,
    },
    SetGeometry {
        entity: EntityId,
        payload: GeometryPayload,
    },
    ClearGeometry { entity: EntityId },

    // --- transversal (§9) ---
    Undo,
    Redo,
    /// Abre una entrada de undo compuesta. Ver [`CommandBus`].
    BeginGroup { label: String },
    EndGroup,
}

impl Command {
    /// Etiqueta con la que aparece en el historial de undo. Es texto de
    /// interfaz: lo lee el usuario en el menú «Deshacer …».
    pub fn label(&self) -> String {
        match self {
            Command::Spawn { name: Some(n), .. } => format!("crear «{n}»"),
            Command::Spawn { name: None, .. } => "crear entidad".into(),
            Command::Despawn { .. } => "borrar entidad".into(),
            Command::SetName { name, .. } => format!("renombrar a «{name}»"),
            Command::SetTransform { .. } => "mover".into(),
            Command::SetVisible { visible: true, .. } => "mostrar".into(),
            Command::SetVisible { visible: false, .. } => "ocultar".into(),
            Command::SetParent { parent: Some(_), .. } => "emparentar".into(),
            Command::SetParent { parent: None, .. } => "desemparentar".into(),
            Command::SetGeometry { .. } => "asignar geometría".into(),
            Command::ClearGeometry { .. } => "quitar geometría".into(),
            Command::Undo => "deshacer".into(),
            Command::Redo => "rehacer".into(),
            Command::BeginGroup { label } => label.clone(),
            Command::EndGroup => "fin de grupo".into(),
        }
    }

    /// Nombre corto y estable de la variante. Se usa en mensajes de error y en
    /// el puente de Lua; no depende del idioma de la etiqueta.
    pub fn kind(&self) -> &'static str {
        match self {
            Command::Spawn { .. } => "Spawn",
            Command::Despawn { .. } => "Despawn",
            Command::SetName { .. } => "SetName",
            Command::SetTransform { .. } => "SetTransform",
            Command::SetVisible { .. } => "SetVisible",
            Command::SetParent { .. } => "SetParent",
            Command::SetGeometry { .. } => "SetGeometry",
            Command::ClearGeometry { .. } => "ClearGeometry",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::BeginGroup { .. } => "BeginGroup",
            Command::EndGroup => "EndGroup",
        }
    }

    /// `true` si la variante muta el documento dentro de una transacción. Undo,
    /// Redo y las marcas de grupo no: operan sobre el historial, que es meta.
    fn es_edicion(&self) -> bool {
        !matches!(
            self,
            Command::Undo | Command::Redo | Command::BeginGroup { .. } | Command::EndGroup
        )
    }
}

// ---------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------

/// Qué salió mal **y qué hacer**.
///
/// Un error que solo dice "no existe" obliga al autor del script a adivinar. El
/// bus es una API pública consumida por gente que no ve el código de FORGE: el
/// mensaje es parte del contrato.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error(
        "la entidad {0} no existe en el documento. Puede que se borrara o que el id venga \
         de otro documento: crea la entidad con `Spawn {{ id: Some(..) }}` antes de tocarla, \
         o vuelve a leer la selección del snapshot actual."
    )]
    EntidadDesconocida(EntityId),

    #[error(
        "la entidad {0} ya existe. `Spawn` con id explícito no pisa una entidad viva a \
         propósito: usa `Spawn {{ id: None }}` para que el bus asigne uno libre, o \
         `Despawn` antes si de verdad querías reemplazarla."
    )]
    EntidadDuplicada(EntityId),

    #[error(
        "`EndGroup` sin `BeginGroup`. Cada `EndGroup` cierra exactamente un grupo abierto: \
         quita este `EndGroup`, o abre el grupo con `BeginGroup {{ label }}` antes."
    )]
    EndGroupSinBeginGroup,

    #[error(
        "el grupo «{0}» quedó abierto al terminar. Sus comandos no se aplicaron: un grupo a \
         medias no puede ser una entrada de undo. Cierra con `EndGroup`, o no abras el grupo."
    )]
    GrupoSinCerrar(String),

    #[error(
        "ya hay un grupo abierto («{0}») y los grupos no se anidan: la entrada de undo sería \
         ambigua. Cierra el grupo con `EndGroup` antes de abrir otro."
    )]
    GrupoAnidado(String),

    #[error(
        "`{0}` no puede ir dentro de un grupo: el grupo aún no se ha aplicado, así que no hay \
         nada que deshacer ni rehacer. Cierra el grupo con `EndGroup` y ejecuta `{0}` después."
    )]
    NoAgrupable(&'static str),

    #[error("no hay nada que deshacer: el documento está en su versión inicial.")]
    NadaQueDeshacer,

    #[error("no hay nada que rehacer: no se ha deshecho nada, o se editó después de deshacer.")]
    NadaQueRehacer,

    #[error(
        "emparentar {child} bajo {parent} crearía un ciclo en el grafo de escena. \
         Desemparenta antes el ancestro con `SetParent {{ parent: None }}`."
    )]
    CicloDeJerarquia { child: EntityId, parent: EntityId },

    #[error(
        "`Spawn` llegó al ejecutor sin id resuelto. Es un fallo interno del bus: los ids se \
         resuelven antes de grabar para que el log sea reproducible."
    )]
    IdSinResolver,

    #[error("no se pudo codificar el log de comandos: {0}")]
    Codificacion(String),

    #[error(
        "no se pudo decodificar el log de comandos: {0}. Suele ser un log grabado por una \
         versión del bus con variantes distintas."
    )]
    Decodificacion(String),
}

pub type Result<T> = std::result::Result<T, CommandError>;

// ---------------------------------------------------------------------------
// Resultado
// ---------------------------------------------------------------------------

/// Resultado tipado de aplicar un comando.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandOutcome {
    /// Se creó una versión nueva del documento.
    Applied {
        version: VersionId,
        entity: Option<EntityId>,
    },
    /// El comando quedó en cola dentro de un grupo abierto. El id, si lo hay, ya
    /// está resuelto: un script puede seguir usándolo sin esperar al `EndGroup`.
    Queued { entity: Option<EntityId> },
    GroupBegan { label: String },
    /// Undo/Redo: el cursor del historial se movió, no hay versión nueva.
    Moved { to: VersionId },
}

impl CommandOutcome {
    /// La entidad creada, si el comando creó alguna.
    pub fn entity(&self) -> Option<EntityId> {
        match self {
            CommandOutcome::Applied { entity, .. } | CommandOutcome::Queued { entity } => *entity,
            _ => None,
        }
    }

    /// La versión resultante, si el comando produjo o alcanzó una.
    pub fn version(&self) -> Option<VersionId> {
        match self {
            CommandOutcome::Applied { version, .. } => Some(*version),
            CommandOutcome::Moved { to } => Some(*to),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// El bus
// ---------------------------------------------------------------------------

struct Grupo {
    label: String,
    pendientes: Vec<Command>,
}

/// Aplica comandos sobre un [`Document`] y graba lo que aplicó.
///
/// # Agrupación
///
/// Entre `BeginGroup` y `EndGroup` los comandos **no se aplican**: se validan y
/// se encolan, y el `EndGroup` los ejecuta todos dentro de **una** transacción.
/// Como una transacción es una entrada de undo (ver `forge-doc`), una macro de
/// veinte comandos se deshace con un `Ctrl+Z`, igual que un comando nativo. Es
/// lo que pide §9 y la razón de que exista la pareja.
///
/// Un grupo que nunca se cierra no aplica nada: sus comandos se descartan y
/// [`CommandBus::finish`] lo denuncia. Es deliberado que olvidarse de cerrar sea
/// inocuo para el documento y ruidoso para el llamante.
pub struct CommandBus {
    grupo: Option<Grupo>,
    log: CommandLog,
    grabando: bool,
}

impl Default for CommandBus {
    fn default() -> Self {
        CommandBus::new()
    }
}

impl CommandBus {
    pub fn new() -> Self {
        CommandBus { grupo: None, log: CommandLog::default(), grabando: true }
    }

    /// Bus que no graba. Lo usa [`CommandLog::replay`]: reproducir un log no
    /// debe volver a grabarlo.
    pub fn sin_grabar() -> Self {
        CommandBus { grupo: None, log: CommandLog::default(), grabando: false }
    }

    pub fn log(&self) -> &CommandLog {
        &self.log
    }

    pub fn take_log(&mut self) -> CommandLog {
        std::mem::take(&mut self.log)
    }

    /// Etiqueta del grupo abierto, si lo hay.
    pub fn grupo_abierto(&self) -> Option<&str> {
        self.grupo.as_deref_label()
    }

    /// Aplica un comando.
    pub fn apply(&mut self, doc: &mut Document, cmd: Command) -> Result<CommandOutcome> {
        let cmd = resolver_ids(cmd);

        match &cmd {
            Command::BeginGroup { label } => {
                if let Some(g) = &self.grupo {
                    return Err(CommandError::GrupoAnidado(g.label.clone()));
                }
                let label = label.clone();
                self.grupo = Some(Grupo { label: label.clone(), pendientes: Vec::new() });
                self.grabar(cmd);
                Ok(CommandOutcome::GroupBegan { label })
            }

            Command::EndGroup => {
                let g = self.grupo.take().ok_or(CommandError::EndGroupSinBeginGroup)?;
                let version = aplicar_lote(doc, &g.pendientes, &g.label)?;
                self.grabar(cmd);
                Ok(CommandOutcome::Applied { version, entity: None })
            }

            Command::Undo | Command::Redo => {
                if self.grupo.is_some() {
                    return Err(CommandError::NoAgrupable(cmd.kind()));
                }
                let to = if matches!(cmd, Command::Undo) {
                    doc.undo().ok_or(CommandError::NadaQueDeshacer)?
                } else {
                    doc.redo().ok_or(CommandError::NadaQueRehacer)?
                };
                self.grabar(cmd);
                Ok(CommandOutcome::Moved { to })
            }

            _ => {
                debug_assert!(cmd.es_edicion());
                if self.grupo.is_some() {
                    // Se valida ahora aunque se aplique luego: un error de la
                    // línea 3 de una macro debe salir en la línea 3.
                    let previstas = self.entidades_previstas(&doc.snapshot());
                    validar(&cmd, |e| previstas.contains(e))?;
                    let entity = entidad_creada(&cmd);
                    if let Some(g) = &mut self.grupo {
                        g.pendientes.push(cmd.clone());
                    }
                    self.grabar(cmd);
                    return Ok(CommandOutcome::Queued { entity });
                }

                let label = cmd.label();
                let mut tx = doc.begin();
                let entity = match aplicar_en_tx(&mut tx, &cmd) {
                    Ok(e) => e,
                    Err(err) => {
                        // Sin commit no hay versión nueva: el documento queda
                        // exactamente como estaba.
                        tx.rollback();
                        return Err(err);
                    }
                };
                let version = tx.commit(label);
                self.grabar(cmd);
                Ok(CommandOutcome::Applied { version, entity })
            }
        }
    }

    /// Aplica una secuencia, parando en el primer error.
    pub fn apply_all(
        &mut self,
        doc: &mut Document,
        cmds: impl IntoIterator<Item = Command>,
    ) -> Result<Vec<CommandOutcome>> {
        cmds.into_iter().map(|c| self.apply(doc, c)).collect()
    }

    /// Cierra la sesión del bus. **Un grupo abierto aquí es un error**: sus
    /// comandos no llegaron al documento y el llamante cree que sí.
    pub fn finish(&mut self) -> Result<()> {
        match self.grupo.take() {
            Some(g) => Err(CommandError::GrupoSinCerrar(g.label)),
            None => Ok(()),
        }
    }

    fn grabar(&mut self, cmd: Command) {
        if self.grabando {
            self.log.commands.push(cmd);
        }
    }

    /// Entidades que existirán cuando el grupo se aplique: las del documento más
    /// las que el propio grupo crea, menos las que borra.
    fn entidades_previstas(&self, snap: &Snapshot) -> Previstas {
        let mut p = Previstas {
            base: snap.entity_ids().into_iter().collect(),
        };
        if let Some(g) = &self.grupo {
            for c in &g.pendientes {
                match c {
                    Command::Spawn { id: Some(e), .. } => {
                        p.base.push(*e);
                    }
                    Command::Despawn { entity } => p.base.retain(|x| x != entity),
                    _ => {}
                }
            }
        }
        p
    }
}

struct Previstas {
    base: Vec<EntityId>,
}

impl Previstas {
    fn contains(&self, e: &EntityId) -> bool {
        self.base.contains(e)
    }
}

/// Truco de legibilidad: evita un `as_ref().map(...)` en el sitio de uso.
trait LabelOpt {
    fn as_deref_label(&self) -> Option<&str>;
}

impl LabelOpt for Option<Grupo> {
    fn as_deref_label(&self) -> Option<&str> {
        self.as_ref().map(|g| g.label.as_str())
    }
}

/// Resuelve el id de `Spawn` **antes** de aplicar y de grabar. Sin esto el log
/// no es reproducible: cada reproducción crearía entidades con ids nuevos.
fn resolver_ids(cmd: Command) -> Command {
    match cmd {
        Command::Spawn { id: None, name } => {
            Command::Spawn { id: Some(EntityId::new()), name }
        }
        otro => otro,
    }
}

fn entidad_creada(cmd: &Command) -> Option<EntityId> {
    match cmd {
        Command::Spawn { id, .. } => *id,
        _ => None,
    }
}

/// Validación que solo necesita saber qué entidades existen. Se usa al encolar
/// dentro de un grupo, donde todavía no hay transacción.
fn validar(cmd: &Command, existe: impl Fn(&EntityId) -> bool) -> Result<()> {
    match cmd {
        Command::Spawn { id: Some(e), .. } => {
            if existe(e) {
                return Err(CommandError::EntidadDuplicada(*e));
            }
        }
        Command::Spawn { id: None, .. } => return Err(CommandError::IdSinResolver),
        Command::Despawn { entity }
        | Command::SetName { entity, .. }
        | Command::SetTransform { entity, .. }
        | Command::SetVisible { entity, .. }
        | Command::SetGeometry { entity, .. }
        | Command::ClearGeometry { entity } => {
            if !existe(entity) {
                return Err(CommandError::EntidadDesconocida(*entity));
            }
        }
        Command::SetParent { child, parent } => {
            if !existe(child) {
                return Err(CommandError::EntidadDesconocida(*child));
            }
            if let Some(p) = parent {
                if !existe(p) {
                    return Err(CommandError::EntidadDesconocida(*p));
                }
                if p == child {
                    return Err(CommandError::CicloDeJerarquia { child: *child, parent: *p });
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Aplica un lote dentro de **una** transacción: es lo que convierte un grupo en
/// una sola entrada de undo.
fn aplicar_lote(doc: &mut Document, cmds: &[Command], label: &str) -> Result<VersionId> {
    let mut tx = doc.begin();
    for c in cmds {
        if let Err(e) = aplicar_en_tx(&mut tx, c) {
            // Un grupo es atómico: si un comando falla, no se confirma nada.
            tx.rollback();
            return Err(e);
        }
    }
    Ok(tx.commit(label.to_string()))
}

fn aplicar_en_tx(tx: &mut Transaction<'_>, cmd: &Command) -> Result<Option<EntityId>> {
    validar(cmd, |e| tx.contains(*e))?;
    match cmd {
        Command::Spawn { id, name } => {
            let e = id.ok_or(CommandError::IdSinResolver)?;
            tx.spawn_with_id(e);
            if let Some(n) = name {
                tx.set(e, Name(n.clone()));
            }
            Ok(Some(e))
        }
        Command::Despawn { entity } => {
            tx.despawn(*entity);
            Ok(None)
        }
        Command::SetName { entity, name } => {
            tx.set(*entity, Name(name.clone()));
            Ok(None)
        }
        Command::SetTransform { entity, transform } => {
            tx.set(*entity, *transform);
            Ok(None)
        }
        Command::SetVisible { entity, visible } => {
            tx.set(*entity, Visible(*visible));
            Ok(None)
        }
        Command::SetParent { child, parent } => {
            match parent {
                Some(p) => {
                    comprobar_ciclo(tx, *child, *p)?;
                    tx.set(*child, Parent(*p));
                }
                None => tx.remove::<Parent>(*child),
            }
            Ok(None)
        }
        Command::SetGeometry { entity, payload } => {
            tx.set(*entity, Geometry(*payload));
            Ok(None)
        }
        Command::ClearGeometry { entity } => {
            tx.remove::<Geometry>(*entity);
            Ok(None)
        }
        // El bus nunca las trae hasta aquí; si llegara, es un fallo del bus y no
        // un error del usuario.
        Command::Undo | Command::Redo | Command::BeginGroup { .. } | Command::EndGroup => {
            Err(CommandError::NoAgrupable(cmd.kind()))
        }
    }
}

/// Un `Parent` cíclico no rompe el snapshot —`world_transform` corta— pero sí
/// rompe cualquier recorrido ingenuo del árbol de escena en la interfaz. Se
/// rechaza en la puerta.
fn comprobar_ciclo(tx: &Transaction<'_>, child: EntityId, parent: EntityId) -> Result<()> {
    let mut actual = Some(parent);
    let mut guarda = 0usize;
    while let Some(a) = actual {
        if a == child {
            return Err(CommandError::CicloDeJerarquia { child, parent });
        }
        guarda += 1;
        if guarda > 4096 {
            break; // jerarquía ya corrupta: no colgarse aquí también
        }
        actual = tx.get::<Parent>(a).map(|p| p.0);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

/// Los comandos que se aplicaron, en orden.
///
/// Aplicarlo sobre un documento vacío reconstruye el mismo estado —misma
/// [`forge_doc::Snapshot::fingerprint`]—. De ahí salen, sin código adicional, el
/// modo batch, la automatización en CI y el banco de pruebas del que habla
/// ADR-0006.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandLog {
    pub commands: Vec<Command>,
}

impl CommandLog {
    pub fn new(commands: Vec<Command>) -> Self {
        CommandLog { commands }
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        ciborium::into_writer(self, &mut out)
            .map_err(|e| CommandError::Codificacion(e.to_string()))?;
        Ok(out)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        ciborium::from_reader(bytes).map_err(|e| CommandError::Decodificacion(e.to_string()))
    }

    /// Reproduce el log sobre `doc`. Un grupo sin cerrar al final del log es un
    /// error: el log estaría mintiendo sobre lo que aplicó.
    pub fn replay(&self, doc: &mut Document) -> Result<()> {
        let mut bus = CommandBus::sin_grabar();
        for c in &self.commands {
            bus.apply(doc, c.clone())?;
        }
        bus.finish()
    }

    /// Reproduce sobre un documento nuevo con el registro de componentes por
    /// defecto.
    pub fn replay_into_new(&self) -> Result<Document> {
        let mut doc = Document::new();
        self.replay(&mut doc)?;
        Ok(doc)
    }
}
