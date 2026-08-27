// Contrato C de la frontera con OpenCASCADE.
//
// Regla de oro (ver crates/forge-kernel-api/src/lib.rs y ADR-0001 regla 2):
// por aqui cruzan triangulos, polilineas e identificadores. Nada mas. Ningun
// tipo de OCCT (TopoDS_Shape, gp_Pnt, Handle<...>, ...) aparece en esta
// cabecera: solo enteros, `double`, punteros a arreglos planos y `bool`.
//
// Este archivo es la fuente de verdad del *layout* de las structs; el lado
// Rust (`src/ffi.rs`) las declara con `#[repr(C)]` en el mismo orden de
// campos. Si se cambia una struct aqui, hay que cambiar la espejada alli: no
// hay generador automatico (cbindgen/bindgen) de por medio todavia.
//
// Convencion de errores: toda funcion que puede fallar devuelve `bool`
// (true = exito) y recibe `char** err_out`. En exito deja `*err_out = nullptr`
// y no hay nada que liberar. En fallo escribe un mensaje propio (con `strdup`
// o equivalente) en `*err_out`, que el llamante debe liberar con
// `forge_occt_free_string`. El shim nunca deja escapar una excepcion de C++:
// cada funcion la atrapa en la frontera (ver shim.cpp) y la traduce a este
// mecanismo.
//
// Duenio de la memoria: el shim es duenio de las formas (indexadas por el
// `uint64_t` que el mismo asigna) y de cualquier buffer que devuelva por
// puntero de salida. Cada buffer tiene su `forge_occt_free_*` correspondiente;
// liberar con `delete`/`free` del lado de Rust seria comportamiento
// indefinido.

#pragma once

#include <cstddef>
#include <cstdint>

extern "C" {

// --- carga -----------------------------------------------------------------

// Lee un archivo STEP desde memoria y devuelve un handle por cada forma raiz
// transferida. `*ids_out` se libera con `forge_occt_free_ids`.
bool forge_occt_load_step(const uint8_t* datos, size_t len, uint64_t** ids_out,
                           size_t* count_out, char** err_out);

// Libera una forma. Sin efecto si `handle` no existe: `release` en la
// frontera de Rust (forge-kernel-api) no es fallible, asi que este lado
// tampoco lo es.
void forge_occt_release(uint64_t handle);

// --- consultas ---------------------------------------------------------------

bool forge_occt_bbox(uint64_t handle, double min_out[3], double max_out[3],
                      char** err_out);

bool forge_occt_mass_properties(uint64_t handle, double* volumen_mm3_out,
                                 double* area_mm2_out, double centroide_out[3],
                                 char** err_out);

// Una entidad topologica (cara, arista o vertice) aplanada.
//
// `clase`: 0 = cara, 1 = arista, 2 = vertice (espeja `forge_doc::TopoClass`).
// `mark`: indice estable *dentro de esta llamada*, derivado de recorrer la
// forma con `TopExp::MapShapes` (ver nota de estabilidad en shim.cpp). El
// lado Rust lo combina con el `FeatureId` que posee la forma para construir
// el `StableId` completo -- ningun `FeatureId` cruza esta frontera.
struct ForgeTopoEntidad {
    uint64_t mark;
    uint8_t clase;
    double centroide[3];
    // Para caras: normal de superficie en un punto representativo. Para
    // aristas: tangente en el punto medio, como sustituto razonable. Para
    // vertices: sin significado, siempre {0,0,0}.
    double normal[3];
    // Area para caras, longitud para aristas, 0 para vertices.
    double medida;
};

struct ForgeTopologia {
    ForgeTopoEntidad* caras;
    size_t caras_count;
    ForgeTopoEntidad* aristas;
    size_t aristas_count;
    ForgeTopoEntidad* vertices;
    size_t vertices_count;
    uint8_t es_solido;
    uint8_t es_cerrado;
};

bool forge_occt_topology(uint64_t handle, ForgeTopologia* out, char** err_out);
void forge_occt_free_topology(ForgeTopologia* t);

// --- teselado ----------------------------------------------------------------

// Clasificacion de arista para dibujado; espeja `forge_kernel_api::EdgeKind`.
// 0 = Sharp, 1 = Boundary, 2 = Smooth, 3 = Seam, 4 = Degenerate.
struct ForgeTeselado {
    // Vertices, sin compartir entre caras (una cara = sus propios vertices):
    // dos caras que comparten una arista tienen normales distintas, y
    // compartir el vertice promediaria la normal.
    const double* posiciones; // xyz * vertex_count
    const double* normales;   // xyz * vertex_count, plana por triangulo
    size_t vertex_count;

    const uint32_t* indices; // tripletes, index_count % 3 == 0
    size_t index_count;
    // Un mark de cara por triangulo: face_marks[i] para el triangulo i.
    const uint64_t* face_marks; // index_count / 3 entradas

    // Polilineas de arista, muestreadas de la curva analitica (no de la
    // malla): ver el doc de `Tessellation` en forge-kernel-api sobre por que.
    const uint64_t* edge_marks;         // edge_count
    const uint8_t* edge_kinds;          // edge_count
    const uint32_t* edge_point_offsets; // edge_count + 1, prefijos en edge_points
    const double* edge_points;          // xyz * edge_point_offsets[edge_count]
    size_t edge_count;

    double bbox_min[3];
    double bbox_max[3];
};

bool forge_occt_tessellate(uint64_t handle, double chord_mm, double angular_deg,
                            ForgeTeselado* out, char** err_out);
void forge_occt_free_tessellation(ForgeTeselado* t);

// --- persistencia --------------------------------------------------------------

bool forge_occt_serialize(uint64_t handle, uint8_t** datos_out, size_t* len_out,
                           char** err_out);
bool forge_occt_deserialize(const uint8_t* datos, size_t len,
                             uint64_t* handle_out, char** err_out);

// --- liberacion de buffers -----------------------------------------------------

void forge_occt_free_string(char* s);
void forge_occt_free_ids(uint64_t* p, size_t len);
void forge_occt_free_bytes(uint8_t* p, size_t len);

} // extern "C"
