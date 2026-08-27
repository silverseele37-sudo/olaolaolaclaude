//! Kernel geométrico sobre OpenCASCADE.
//!
//! Es la otra mitad del patrón de ABI doble que valida `forge-kernel-stub`
//! (ver el doc de ese crate y ADR-0001/ADR-0007): la misma interfaz
//! [`GeometryKernel`], dos implementaciones. Esta es la que habla con C++ de
//! verdad; la regla de oro de la frontera (ADR-0001 regla 2, y el doc de
//! `forge-kernel-api`) es la misma para las dos: **por aquí cruzan
//! triángulos, polilíneas e identificadores. Nada más.** Ningún tipo de OCCT
//! aparece del lado de Rust — ver `src/shim.hpp` y `src/ffi.rs`.
//!
//! # Compila con y sin OCCT
//!
//! `build.rs` busca OpenCASCADE en disco (ver ese archivo y
//! `docs/construir-occt.md`) y, si no lo encuentra, define el cfg
//! `sin_occt`. Con ese cfg, este crate compila igual, pero **todas** las
//! funciones de [`GeometryKernel`] devuelven
//! `KernelError::Unsupported("compilado sin OpenCASCADE")` sin tocar nada de
//! C++. Es lo que mantiene el workspace verde en cualquier máquina, incluida
//! la que escribió esto: aquí OCCT nunca estuvo instalado, así que todo el
//! código de `src/shim.cpp` y de las ramas `not(sin_occt)` de este archivo
//! **nunca se ha ejecutado**. Está escrito con cuidado, no verificado —
//! `docs/construir-occt.md` es explícito sobre esa distinción.
//!
//! # Qué hace de verdad (cuando hay OCCT)
//!
//! Solo el lado *consumidor* (ADR-0007, el más acotado): [`import_step`],
//! [`tessellate`] con procedencia por cara, `bbox`, `topology`,
//! `mass_properties`, `serialize`/`deserialize`, `release`. El lado
//! *constructor* — `extrude`, `revolve`, `boolean`, `fillet`, `chamfer`,
//! `transform`, `box_solid`, `cylinder`, `profile_from_polygon`, `is_valid` —
//! está declarado (el trait lo exige) pero devuelve `Unsupported` con un TODO
//! explícito: ahí está la dificultad real de ADR-0001 (semanas, no días), y
//! fingir que está resuelto sería peor que decir que no lo está.
//! `export_step` tampoco tiene contraparte aquí: hereda el `Unsupported` por
//! defecto de [`GeometryKernel`].
//!
//! [`import_step`]: GeometryKernel::import_step
//! [`tessellate`]: GeometryKernel::tessellate
//!
//! # Concurrencia: OCCT no es uniformemente seguro
//!
//! ADR-0001 regla 3 lo dice explícitamente y ADR-0007 lo repite: OpenCASCADE
//! no es seguro para llamar desde varios hilos a la vez de forma uniforme
//! (algunas estructuras internas son estáticos globales mutables, como
//! `Interface_Static`, que ademas usa este puente para fijar
//! `xstep.cascade.unit`). Pero [`GeometryKernel`] exige `Send + Sync` y todos
//! sus métodos toman `&self`: cualquier llamante puede, en principio, invocar
//! el kernel desde varios hilos.
//!
//! **Confinamiento de esta versión: un `Mutex<()>` de instancia serializa
//! toda llamada al shim.** Cada método que cruza a C++ toma ese candado antes
//! de llamar y lo suelta al volver. Es grueso — dos hilos con llamadas
//! simultáneas a `&self` esperan en fila en vez de correr en paralelo — pero
//! es correcto, y correcto es lo que hace falta antes que rápido. ADR-0001
//! prevé mover esto a un *pool* de hilos dedicado con propiedad por forma
//! cuando el paralelismo real lo justifique (ADR-0007, "Dónde hay que
//! divergir", punto 2); la regla 2 de ADR-0001 — toda llamada es un comando
//! serializable — ya lo permite sin rediseñar esta interfaz.
//!
//! El propio `shim.cpp` además guarda su almacén de formas tras su propio
//! mutex (necesario de todos modos: llamadas desde dos instancias de
//! `OcctKernel`, si alguna vez las hay, comparten ese estado global de C++).
//! Es candado doble a propósito: el de Rust es el que importa para el
//! contrato de esta API; el de C++ es la red de seguridad de la memoria
//! compartida por debajo.

use forge_doc::{FeatureId, StableId};
use forge_kernel_api::*;
use forge_math::{Aabb, DAffine3, DVec2, DVec3};

#[cfg(not(sin_occt))]
mod ffi;

#[cfg(not(sin_occt))]
use std::collections::HashMap;
#[cfg(not(sin_occt))]
use std::os::raw::c_char;
#[cfg(not(sin_occt))]
use std::sync::{Mutex, RwLock};

/// Kernel geométrico que habla con OpenCASCADE a través de `src/shim.cpp`.
///
/// No guarda ningún dato de OCCT: las formas viven dentro del shim, indexadas
/// por el mismo `u64` que envuelve [`ShapeId`]. Lo único que este struct
/// mantiene del lado de Rust es la tabla `handle -> FeatureId` propietario,
/// porque [`FeatureId`] es un tipo de `forge-doc` y no puede cruzar la
/// frontera (regla de oro) — hace falta recordarlo aquí para poder construir
/// los [`StableId`] que salen de `topology` y `tessellate`.
pub struct OcctKernel {
    /// Confinamiento de las llamadas a OCCT. Ver el doc del módulo.
    #[cfg(not(sin_occt))]
    lock: Mutex<()>,
    #[cfg(not(sin_occt))]
    owners: RwLock<HashMap<ShapeId, FeatureId>>,
}

impl OcctKernel {
    pub fn new() -> Self {
        #[cfg(not(sin_occt))]
        {
            OcctKernel {
                lock: Mutex::new(()),
                owners: RwLock::new(HashMap::new()),
            }
        }
        #[cfg(sin_occt)]
        {
            OcctKernel {}
        }
    }
}

impl Default for OcctKernel {
    fn default() -> Self {
        OcctKernel::new()
    }
}

// ---------------------------------------------------------------------------
// Utilidades del lado `not(sin_occt)`: conversión de la salida plana del shim
// a los tipos de `forge-kernel-api`, y manejo de errores sin `unwrap`.
// ---------------------------------------------------------------------------

#[cfg(not(sin_occt))]
fn envenenado(que: &'static str) -> KernelError {
    KernelError::KernelPanic {
        backtrace: format!("{que} de forge-kernel-occt quedo envenenado tras un panico previo"),
    }
}

#[cfg(not(sin_occt))]
fn corrupto(op: &'static str, detail: impl Into<String>) -> KernelError {
    KernelError::OperationFailed {
        op,
        detail: detail.into(),
    }
}

/// Convierte el `char*` de error del shim en un [`KernelError`] y libera el
/// buffer. Nunca hace `unwrap`: un mensaje que no fuera UTF-8 válido se
/// degrada con reemplazo de caracteres en vez de entrar en pánico.
///
/// # Safety
/// `err` debe ser nulo o un puntero devuelto por el shim en su parámetro
/// `err_out`, no liberado todavía.
#[cfg(not(sin_occt))]
unsafe fn error_de(op: &'static str, err: *mut c_char) -> KernelError {
    if err.is_null() {
        return corrupto(op, "el shim informo un fallo sin mensaje de error");
    }
    let detail = std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned();
    ffi::forge_occt_free_string(err);
    corrupto(op, detail)
}

/// # Safety
/// `ptr` debe ser nulo, o válido para lecturas de `len` elementos `T`.
#[cfg(not(sin_occt))]
unsafe fn slice_or_empty<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

#[cfg(not(sin_occt))]
fn edge_kind_de(v: u8) -> KernelResult<EdgeKind> {
    match v {
        0 => Ok(EdgeKind::Sharp),
        1 => Ok(EdgeKind::Boundary),
        2 => Ok(EdgeKind::Smooth),
        3 => Ok(EdgeKind::Seam),
        4 => Ok(EdgeKind::Degenerate),
        otro => Err(corrupto(
            "tessellate",
            format!("el shim devolvio una clase de arista desconocida: {otro}"),
        )),
    }
}

#[cfg(not(sin_occt))]
fn topo_class_de(v: u8) -> KernelResult<TopoClass> {
    match v {
        0 => Ok(TopoClass::Face),
        1 => Ok(TopoClass::Edge),
        2 => Ok(TopoClass::Vertex),
        otro => Err(corrupto(
            "topology",
            format!("el shim devolvio una clase topologica desconocida: {otro}"),
        )),
    }
}

#[cfg(not(sin_occt))]
fn entidad_desde_ffi(
    e: &ffi::ForgeTopoEntidad,
    owner: FeatureId,
    esperada: TopoClass,
) -> KernelResult<TopoEntity> {
    let clase = topo_class_de(e.clase)?;
    if clase != esperada {
        // No debería poder pasar: es el shim clasificando mal su propia
        // salida. Se trata como corrupción de datos, no se adivina.
        return Err(corrupto(
            "topology",
            "el shim puso una entidad en el arreglo de clase equivocada",
        ));
    }
    let centroid = DVec3::new(e.centroide[0], e.centroide[1], e.centroide[2]);
    let normal = DVec3::new(e.normal[0], e.normal[1], e.normal[2]);
    Ok(TopoEntity {
        id: StableId {
            origin: owner,
            class: clase,
            mark: e.mark,
        },
        // Toda entidad importada de STEP nace de la misma operacion de
        // carga: no hay un StableId anterior del que "heredar" ni una arista
        // de perfil de la que "barrer". `Primitive` es la variante que mejor
        // describe "creada directamente por esta operacion".
        provenance: TopoProvenance::Primitive {
            index: e.mark as u32,
        },
        signature: GeometrySignature::new(centroid, normal, e.medida, clase),
    })
}

#[cfg(not(sin_occt))]
fn topology_desde_ffi(
    out: &ffi::ForgeTopologia,
    owner: FeatureId,
) -> KernelResult<TopologySummary> {
    // Safety: `out` lo acaba de llenar `forge_occt_topology`, que devolvio
    // `true`; sus punteros son validos para sus `_count` hasta el
    // `forge_occt_free_topology` que hace el llamante despues de esta funcion.
    let caras = unsafe { slice_or_empty(out.caras, out.caras_count) };
    let aristas = unsafe { slice_or_empty(out.aristas, out.aristas_count) };
    let vertices = unsafe { slice_or_empty(out.vertices, out.vertices_count) };

    let mut r = TopologySummary {
        is_solid: out.es_solido != 0,
        is_closed: out.es_cerrado != 0,
        ..Default::default()
    };
    for e in caras {
        r.faces.push(entidad_desde_ffi(e, owner, TopoClass::Face)?);
    }
    for e in aristas {
        r.edges.push(entidad_desde_ffi(e, owner, TopoClass::Edge)?);
    }
    for e in vertices {
        r.vertices
            .push(entidad_desde_ffi(e, owner, TopoClass::Vertex)?);
    }
    Ok(r)
}

#[cfg(not(sin_occt))]
fn tessellation_desde_ffi(
    out: &ffi::ForgeTeselado,
    owner: FeatureId,
) -> KernelResult<Tessellation> {
    // Safety: mismo razonamiento que en `topology_desde_ffi`, sobre la salida
    // de `forge_occt_tessellate`.
    let posiciones_flat = unsafe { slice_or_empty(out.posiciones, out.vertex_count * 3) };
    let normales_flat = unsafe { slice_or_empty(out.normales, out.vertex_count * 3) };
    let positions: Vec<DVec3> = posiciones_flat
        .chunks_exact(3)
        .map(|c| DVec3::new(c[0], c[1], c[2]))
        .collect();
    let normals: Vec<DVec3> = normales_flat
        .chunks_exact(3)
        .map(|c| DVec3::new(c[0], c[1], c[2]))
        .collect();

    let indices: Vec<u32> = unsafe { slice_or_empty(out.indices, out.index_count) }.to_vec();
    let triangle_count = out.index_count / 3;
    let face_marks = unsafe { slice_or_empty(out.face_marks, triangle_count) };
    let face_of_triangle: Vec<StableId> = face_marks
        .iter()
        .map(|&mark| StableId {
            origin: owner,
            class: TopoClass::Face,
            mark,
        })
        .collect();

    let edge_marks = unsafe { slice_or_empty(out.edge_marks, out.edge_count) };
    let edge_kinds_raw = unsafe { slice_or_empty(out.edge_kinds, out.edge_count) };
    let edge_offsets = unsafe { slice_or_empty(out.edge_point_offsets, out.edge_count + 1) };
    let total_edge_points = edge_offsets.last().copied().unwrap_or(0) as usize;
    let edge_points_flat = unsafe { slice_or_empty(out.edge_points, total_edge_points * 3) };

    let mut edges = Vec::with_capacity(out.edge_count);
    for i in 0..out.edge_count {
        let mark = *edge_marks
            .get(i)
            .ok_or_else(|| corrupto("tessellate", "edge_marks mas corto que edge_count"))?;
        let kind = edge_kind_de(
            *edge_kinds_raw
                .get(i)
                .ok_or_else(|| corrupto("tessellate", "edge_kinds mas corto que edge_count"))?,
        )?;
        let inicio = *edge_offsets
            .get(i)
            .ok_or_else(|| corrupto("tessellate", "edge_point_offsets incompleto"))?
            as usize;
        let fin = *edge_offsets
            .get(i + 1)
            .ok_or_else(|| corrupto("tessellate", "edge_point_offsets incompleto"))?
            as usize;
        if fin < inicio || fin * 3 > edge_points_flat.len() {
            return Err(corrupto(
                "tessellate",
                format!("offsets de arista fuera de rango: [{inicio}, {fin})"),
            ));
        }
        let points = edge_points_flat[inicio * 3..fin * 3]
            .chunks_exact(3)
            .map(|c| DVec3::new(c[0], c[1], c[2]))
            .collect();
        edges.push(EdgePolyline {
            id: StableId {
                origin: owner,
                class: TopoClass::Edge,
                mark,
            },
            kind,
            points,
        });
    }

    Ok(Tessellation {
        positions,
        normals,
        indices,
        face_of_triangle,
        edges,
        bbox: Aabb::new(
            DVec3::new(out.bbox_min[0], out.bbox_min[1], out.bbox_min[2]),
            DVec3::new(out.bbox_max[0], out.bbox_max[1], out.bbox_max[2]),
        ),
    })
}

/// Métodos que de verdad hablan con OCCT. Separados del `impl GeometryKernel`
/// para que ese `impl` quede legible: una linea por metodo, delegando aqui.
#[cfg(not(sin_occt))]
impl OcctKernel {
    /// FeatureId propietario de `s`, o `UnknownShape` si el handle no se
    /// registró aquí — sin tocar el shim: es más barato y da el error exacto
    /// que promete el contrato en vez de un `OperationFailed` genérico.
    fn owner_de(&self, s: ShapeId) -> KernelResult<FeatureId> {
        let owners = self
            .owners
            .read()
            .map_err(|_| envenenado("el mapa de propietarios"))?;
        owners.get(&s).copied().ok_or(KernelError::UnknownShape(s))
    }

    fn con_occt_import_step(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<Vec<ShapeId>> {
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut ids_ptr: *mut u64 = std::ptr::null_mut();
        let mut count: usize = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // Safety: los cuatro punteros de salida son locales validos; el shim
        // atrapa sus propias excepciones y nunca deja `ids_ptr` apuntando a
        // memoria parcial si devuelve `false`.
        let ok = unsafe {
            ffi::forge_occt_load_step(
                bytes.as_ptr(),
                bytes.len(),
                &mut ids_ptr,
                &mut count,
                &mut err,
            )
        };
        if !ok {
            return Err(unsafe { error_de("import_step", err) });
        }
        let ids: Vec<u64> = unsafe { slice_or_empty(ids_ptr, count) }.to_vec();
        unsafe { ffi::forge_occt_free_ids(ids_ptr, count) };

        let mut owners = self
            .owners
            .write()
            .map_err(|_| envenenado("el mapa de propietarios"))?;
        let salida: Vec<ShapeId> = ids
            .into_iter()
            .map(|id| {
                let sid = ShapeId(id);
                owners.insert(sid, owner);
                sid
            })
            .collect();
        Ok(salida)
    }

    fn con_occt_bbox(&self, s: ShapeId) -> KernelResult<Aabb> {
        self.owner_de(s)?;
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut min = [0.0f64; 3];
        let mut max = [0.0f64; 3];
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe { ffi::forge_occt_bbox(s.0, min.as_mut_ptr(), max.as_mut_ptr(), &mut err) };
        if !ok {
            return Err(unsafe { error_de("bbox", err) });
        }
        Ok(Aabb::new(
            DVec3::new(min[0], min[1], min[2]),
            DVec3::new(max[0], max[1], max[2]),
        ))
    }

    fn con_occt_mass_properties(&self, s: ShapeId) -> KernelResult<MassProperties> {
        self.owner_de(s)?;
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut volumen = 0.0f64;
        let mut area = 0.0f64;
        let mut centroide = [0.0f64; 3];
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe {
            ffi::forge_occt_mass_properties(
                s.0,
                &mut volumen,
                &mut area,
                centroide.as_mut_ptr(),
                &mut err,
            )
        };
        if !ok {
            return Err(unsafe { error_de("mass_properties", err) });
        }
        Ok(MassProperties {
            volume_mm3: volumen,
            area_mm2: area,
            centroid: DVec3::new(centroide[0], centroide[1], centroide[2]),
        })
    }

    fn con_occt_topology(&self, s: ShapeId) -> KernelResult<TopologySummary> {
        let owner = self.owner_de(s)?;
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut out = ffi::ForgeTopologia::default();
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe { ffi::forge_occt_topology(s.0, &mut out, &mut err) };
        if !ok {
            return Err(unsafe { error_de("topology", err) });
        }
        let resultado = topology_desde_ffi(&out, owner);
        unsafe { ffi::forge_occt_free_topology(&mut out) };
        resultado
    }

    fn con_occt_tessellate(
        &self,
        s: ShapeId,
        p: &TessellationParams,
    ) -> KernelResult<Tessellation> {
        let owner = self.owner_de(s)?;
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut out = ffi::ForgeTeselado::default();
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe {
            ffi::forge_occt_tessellate(s.0, p.chord_mm, p.angular_deg, &mut out, &mut err)
        };
        if !ok {
            return Err(unsafe { error_de("tessellate", err) });
        }
        let resultado = tessellation_desde_ffi(&out, owner);
        unsafe { ffi::forge_occt_free_tessellation(&mut out) };
        let t = resultado?;
        t.validate()?;
        Ok(t)
    }

    fn con_occt_serialize(&self, s: ShapeId) -> KernelResult<Vec<u8>> {
        self.owner_de(s)?;
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut datos: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe { ffi::forge_occt_serialize(s.0, &mut datos, &mut len, &mut err) };
        if !ok {
            return Err(unsafe { error_de("serialize", err) });
        }
        let bytes = unsafe { slice_or_empty(datos, len) }.to_vec();
        unsafe { ffi::forge_occt_free_bytes(datos, len) };
        Ok(bytes)
    }

    fn con_occt_deserialize(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<ShapeId> {
        let _guardia = self
            .lock
            .lock()
            .map_err(|_| envenenado("el candado del puente"))?;
        let mut handle: u64 = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        let ok = unsafe {
            ffi::forge_occt_deserialize(bytes.as_ptr(), bytes.len(), &mut handle, &mut err)
        };
        if !ok {
            return Err(unsafe { error_de("deserialize", err) });
        }
        let sid = ShapeId(handle);
        let mut owners = self
            .owners
            .write()
            .map_err(|_| envenenado("el mapa de propietarios"))?;
        owners.insert(sid, owner);
        Ok(sid)
    }
}

/// Cuerpo compartido de los métodos del lado constructor: no implementados
/// todavía. Bajo `sin_occt` el mensaje es siempre el mismo y genérico (no hay
/// nada que distinguir: nada funciona sin OCCT). Con OCCT presente, cada
/// operación tiene su propio mensaje de TODO — es honesto decir *cuál*
/// operación falta, no solo que el kernel en general está incompleto.
macro_rules! lado_constructor_pendiente {
    ($todo:literal) => {{
        #[cfg(sin_occt)]
        {
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            Err(KernelError::Unsupported($todo))
        }
    }};
}

impl GeometryKernel for OcctKernel {
    fn name(&self) -> &'static str {
        "occt"
    }

    // --- construcción: TODO, ver el doc del modulo ---

    fn profile_from_polygon(&self, pts: &[DVec2], owner: FeatureId) -> KernelResult<ShapeId> {
        let _ = (pts, owner);
        lado_constructor_pendiente!(
            "profile_from_polygon: lado constructor pendiente en forge-kernel-occt"
        )
    }

    fn extrude(
        &self,
        profile: ShapeId,
        opts: ExtrudeOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (profile, opts, owner);
        lado_constructor_pendiente!("extrude: lado constructor pendiente en forge-kernel-occt")
    }

    fn revolve(
        &self,
        profile: ShapeId,
        opts: RevolveOpts,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (profile, opts, owner);
        lado_constructor_pendiente!("revolve: lado constructor pendiente en forge-kernel-occt")
    }

    fn box_solid(&self, min: DVec3, max: DVec3, owner: FeatureId) -> KernelResult<ShapeId> {
        let _ = (min, max, owner);
        lado_constructor_pendiente!("box_solid: lado constructor pendiente en forge-kernel-occt")
    }

    fn cylinder(
        &self,
        base: DVec3,
        axis: DVec3,
        radius_mm: f64,
        height_mm: f64,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (base, axis, radius_mm, height_mm, owner);
        lado_constructor_pendiente!("cylinder: lado constructor pendiente en forge-kernel-occt")
    }

    fn fillet(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: FilletSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (solid, edges, spec, owner);
        lado_constructor_pendiente!("fillet: lado constructor pendiente en forge-kernel-occt")
    }

    fn chamfer(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: ChamferSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (solid, edges, spec, owner);
        lado_constructor_pendiente!("chamfer: lado constructor pendiente en forge-kernel-occt")
    }

    fn boolean(
        &self,
        op: BoolOp,
        a: ShapeId,
        b: ShapeId,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let _ = (op, a, b, owner);
        lado_constructor_pendiente!("boolean: lado constructor pendiente en forge-kernel-occt")
    }

    fn transform(&self, s: ShapeId, m: &DAffine3, owner: FeatureId) -> KernelResult<ShapeId> {
        let _ = (s, m, owner);
        lado_constructor_pendiente!("transform: lado constructor pendiente en forge-kernel-occt")
    }

    fn is_valid(&self, s: ShapeId) -> KernelResult<ValidationReport> {
        let _ = s;
        lado_constructor_pendiente!("is_valid: pendiente en forge-kernel-occt")
    }

    // --- consulta: implementado sobre OCCT, ver el doc del modulo ---

    fn topology(&self, s: ShapeId) -> KernelResult<TopologySummary> {
        #[cfg(sin_occt)]
        {
            let _ = s;
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_topology(s)
        }
    }

    fn mass_properties(&self, s: ShapeId) -> KernelResult<MassProperties> {
        #[cfg(sin_occt)]
        {
            let _ = s;
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_mass_properties(s)
        }
    }

    fn bbox(&self, s: ShapeId) -> KernelResult<Aabb> {
        #[cfg(sin_occt)]
        {
            let _ = s;
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_bbox(s)
        }
    }

    // --- cruce de dominio: la unica salida ---

    fn tessellate(&self, s: ShapeId, p: &TessellationParams) -> KernelResult<Tessellation> {
        #[cfg(sin_occt)]
        {
            let _ = (s, p);
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_tessellate(s, p)
        }
    }

    // --- persistencia ---

    fn serialize(&self, s: ShapeId) -> KernelResult<Vec<u8>> {
        #[cfg(sin_occt)]
        {
            let _ = s;
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_serialize(s)
        }
    }

    fn deserialize(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<ShapeId> {
        #[cfg(sin_occt)]
        {
            let _ = (bytes, owner);
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_deserialize(bytes, owner)
        }
    }

    // --- intercambio ---

    fn import_step(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<Vec<ShapeId>> {
        #[cfg(sin_occt)]
        {
            let _ = (bytes, owner);
            Err(KernelError::Unsupported("compilado sin OpenCASCADE"))
        }
        #[cfg(not(sin_occt))]
        {
            self.con_occt_import_step(bytes, owner)
        }
    }

    // `export_step` no se sobreescribe: hereda el `Unsupported("export_step")`
    // por defecto del trait. No es parte del lado consumidor (ver el doc del
    // modulo) y no se fingio una implementacion para no dejar esta lista de
    // metodos "completa" a costa de ser deshonesta.

    fn release(&self, s: ShapeId) {
        #[cfg(sin_occt)]
        {
            let _ = s; // nunca se creo ninguna forma: nada que liberar.
        }
        #[cfg(not(sin_occt))]
        {
            if let Ok(mut owners) = self.owners.write() {
                owners.remove(&s);
            }
            // Si el candado del puente esta envenenado, no hay forma segura
            // de invocar mas C++: se prefiere perder este `release` (una fuga
            // de una forma dentro del shim) a arriesgar comportamiento
            // indefinido. `release` no es fallible en el contrato del trait,
            // asi que tampoco hay donde reportarlo.
            if let Ok(_guardia) = self.lock.lock() {
                unsafe { ffi::forge_occt_release(s.0) };
            }
        }
    }
}
