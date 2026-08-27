//! Declaraciones `extern "C"` de la frontera con el shim de OpenCASCADE.
//!
//! Este módulo solo se compila con `not(sin_occt))` — es decir, solo cuando
//! `build.rs` encontró OCCT y compiló `shim.cpp`. Bajo `sin_occt` no existe:
//! no hay nada que enlazar ni declarar.
//!
//! **Ningún tipo de OCCT aparece aquí.** Estas structs son el espejo en Rust
//! de las de `src/shim.hpp` — mismo orden de campos, mismos anchos. Si se
//! cambia una struct de un lado hay que cambiar la otra: no hay generador
//! automático (`bindgen`/`cbindgen`) de por medio todavía.
//!
//! Todo lo que hay aquí es `unsafe`: son punteros crudos que el shim
//! garantiza válidos hasta la llamada a `forge_occt_free_*` correspondiente.
//! `src/lib.rs` es la única llamante y la que mantiene esa disciplina; nada
//! fuera de este crate ve estas firmas.

#![allow(dead_code)] // el lado constructor (extrude, boolean...) no llama a nada de aquí todavía

use std::os::raw::{c_char, c_uchar};

/// Espejo de `ForgeTopoEntidad` (shim.hpp). `clase`: 0 cara, 1 arista, 2 vértice.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ForgeTopoEntidad {
    pub mark: u64,
    pub clase: u8,
    pub centroide: [f64; 3],
    pub normal: [f64; 3],
    pub medida: f64,
}

/// Espejo de `ForgeTopologia` (shim.hpp).
#[repr(C)]
pub struct ForgeTopologia {
    pub caras: *mut ForgeTopoEntidad,
    pub caras_count: usize,
    pub aristas: *mut ForgeTopoEntidad,
    pub aristas_count: usize,
    pub vertices: *mut ForgeTopoEntidad,
    pub vertices_count: usize,
    pub es_solido: u8,
    pub es_cerrado: u8,
}

impl Default for ForgeTopologia {
    fn default() -> Self {
        // Todo en cero/nulo: es el estado que `forge_occt_topology` deja en
        // `*out` antes de intentar nada, para que un `false` con campos a
        // medio llenar nunca sea observable del lado Rust.
        ForgeTopologia {
            caras: std::ptr::null_mut(),
            caras_count: 0,
            aristas: std::ptr::null_mut(),
            aristas_count: 0,
            vertices: std::ptr::null_mut(),
            vertices_count: 0,
            es_solido: 0,
            es_cerrado: 0,
        }
    }
}

/// Espejo de `ForgeTeselado` (shim.hpp). `edge_kinds`: 0 Sharp, 1 Boundary,
/// 2 Smooth, 3 Seam, 4 Degenerate — mismo orden que `EdgeKind` en
/// `forge_kernel_api` (`src/lib.rs` lo verifica con un `match` exhaustivo al
/// convertir, no con una tabla que pueda desincronizarse en silencio).
#[repr(C)]
pub struct ForgeTeselado {
    pub posiciones: *const f64,
    pub normales: *const f64,
    pub vertex_count: usize,

    pub indices: *const u32,
    pub index_count: usize,
    pub face_marks: *const u64,

    pub edge_marks: *const u64,
    pub edge_kinds: *const c_uchar,
    pub edge_point_offsets: *const u32,
    pub edge_points: *const f64,
    pub edge_count: usize,

    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
}

impl Default for ForgeTeselado {
    fn default() -> Self {
        ForgeTeselado {
            posiciones: std::ptr::null(),
            normales: std::ptr::null(),
            vertex_count: 0,
            indices: std::ptr::null(),
            index_count: 0,
            face_marks: std::ptr::null(),
            edge_marks: std::ptr::null(),
            edge_kinds: std::ptr::null(),
            edge_point_offsets: std::ptr::null(),
            edge_points: std::ptr::null(),
            edge_count: 0,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
        }
    }
}

extern "C" {
    // --- carga ---
    pub fn forge_occt_load_step(
        datos: *const c_uchar,
        len: usize,
        ids_out: *mut *mut u64,
        count_out: *mut usize,
        err_out: *mut *mut c_char,
    ) -> bool;

    pub fn forge_occt_release(handle: u64);

    // --- consultas ---
    pub fn forge_occt_bbox(
        handle: u64,
        min_out: *mut f64, // arreglo de 3
        max_out: *mut f64, // arreglo de 3
        err_out: *mut *mut c_char,
    ) -> bool;

    pub fn forge_occt_mass_properties(
        handle: u64,
        volumen_mm3_out: *mut f64,
        area_mm2_out: *mut f64,
        centroide_out: *mut f64, // arreglo de 3
        err_out: *mut *mut c_char,
    ) -> bool;

    pub fn forge_occt_topology(
        handle: u64,
        out: *mut ForgeTopologia,
        err_out: *mut *mut c_char,
    ) -> bool;
    pub fn forge_occt_free_topology(t: *mut ForgeTopologia);

    // --- teselado ---
    pub fn forge_occt_tessellate(
        handle: u64,
        chord_mm: f64,
        angular_deg: f64,
        out: *mut ForgeTeselado,
        err_out: *mut *mut c_char,
    ) -> bool;
    pub fn forge_occt_free_tessellation(t: *mut ForgeTeselado);

    // --- persistencia ---
    pub fn forge_occt_serialize(
        handle: u64,
        datos_out: *mut *mut c_uchar,
        len_out: *mut usize,
        err_out: *mut *mut c_char,
    ) -> bool;
    pub fn forge_occt_deserialize(
        datos: *const c_uchar,
        len: usize,
        handle_out: *mut u64,
        err_out: *mut *mut c_char,
    ) -> bool;

    // --- liberación de buffers ---
    pub fn forge_occt_free_string(s: *mut c_char);
    pub fn forge_occt_free_ids(p: *mut u64, len: usize);
    pub fn forge_occt_free_bytes(p: *mut c_uchar, len: usize);
}
