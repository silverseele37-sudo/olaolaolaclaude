// Puente C++ hacia OpenCASCADE. Lado consumidor (ADR-0001, ADR-0007 punto 1):
// cargar STEP, teselar con procedencia por cara, bbox, propiedades de masa,
// topologia y persistencia binaria/textual de la forma. El lado constructor
// (extrude, boolean, fillet...) no tiene contraparte aqui todavia: el trait
// `GeometryKernel` del lado Rust lo declara `Unsupported` con un TODO
// explicito en vez de fingir que esta resuelto.
//
// *** ESTE ARCHIVO NUNCA SE HA COMPILADO. *** build.rs solo lo compila cuando
// encuentra OCCT, y OCCT no estaba instalado en el entorno donde se escribio
// esto. La estructura (que solo crucen tipos planos, que ninguna excepcion
// escape, como se numeran caras/aristas) esta pensada con cuidado; las firmas
// exactas de algunas llamadas de OCCT estan marcadas "VERIFICAR" donde hay
// motivo real de duda entre versiones. Verificarlas contra la version de OCCT
// instalada es el primer paso al compilar esto de verdad, no un detalle.
//
// Reglas no negociables de esta frontera:
//  1. Ningun tipo de OCCT cruza a Rust: todo lo que sale es double/uint64_t/
//     uint32_t/uint8_t o punteros a arreglos de esos tipos.
//  2. Ninguna excepcion de C++ cruza a Rust: cada funcion exportada esta
//     envuelta en try/catch(Standard_Failure)/catch(...).
//  3. El shim es duenio de la memoria de las formas (mapa por handle) y de
//     cualquier buffer que devuelve: cada uno tiene su `forge_occt_free_*`.

#include "shim.hpp"

#include <algorithm>
#include <cstring>
#include <mutex>
#include <new>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

// --- OpenCASCADE -------------------------------------------------------------
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <BRepGProp_Face.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepTools.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GProp_GProps.hxx>
#include <GeomAbs_Shape.hxx>
#include <BRepAdaptor_Curve.hxx>
#include <Geom_Curve.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <Interface_Static.hxx>
#include <Poly_Triangulation.hxx>
#include <STEPControl_Reader.hxx>
#include <Standard_Failure.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_Vec.hxx>

namespace {

// ---------------------------------------------------------------------------
// Almacen de formas. El shim es su duenio; Rust solo ve el `uint64_t`.
// ---------------------------------------------------------------------------

// Un unico mutex para todo el almacen y para las llamadas que tocan estado
// global de OCCT (p.ej. `Interface_Static`). Grueso a proposito: OCCT no es
// uniformemente seguro en concurrencia (ADR-0001 regla 3) y la correccion
// importa mas que el paralelismo en esta primera version. El lado Rust
// (`src/lib.rs`) hace lo mismo con un mutex de instancia, asi que en la
// practica hay dos candados; es redundante pero inofensivo, y este es el que
// de verdad protege memoria compartida de C++.
std::mutex g_mutex;
std::unordered_map<uint64_t, TopoDS_Shape> g_formas;
uint64_t g_siguiente_id = 1;

// Debe llamarse con `g_mutex` ya tomado.
uint64_t guardar_forma_sin_candado(TopoDS_Shape forma) {
    uint64_t id = g_siguiente_id++;
    g_formas.emplace(id, std::move(forma));
    return id;
}

// Debe llamarse con `g_mutex` ya tomado. Devuelve `nullptr` si no existe.
const TopoDS_Shape* buscar_forma_sin_candado(uint64_t handle) {
    auto it = g_formas.find(handle);
    return it == g_formas.end() ? nullptr : &it->second;
}

// ---------------------------------------------------------------------------
// Utilidades de error y de asignacion, para que el resto del archivo no
// repita `strdup`/`new[]` con sus casos borde.
// ---------------------------------------------------------------------------

char* duplicar_error(const std::string& msg) {
    char* buf = new (std::nothrow) char[msg.size() + 1];
    if (!buf) return nullptr; // sin memoria para el mensaje de error mismo;
                               // el llamante ya recibe `false` de todos modos.
    std::memcpy(buf, msg.c_str(), msg.size() + 1);
    return buf;
}

template <typename T>
T* copiar_vector(const std::vector<T>& v) {
    if (v.empty()) return nullptr;
    T* buf = new T[v.size()];
    std::copy(v.begin(), v.end(), buf);
    return buf;
}

// EdgeKind del lado Rust, replicado como enteros (ver shim.hpp).
enum ClaseArista : uint8_t {
    ARISTA_SHARP = 0,
    ARISTA_BOUNDARY = 1,
    ARISTA_SMOOTH = 2,
    ARISTA_SEAM = 3,
    ARISTA_DEGENERATE = 4,
};

enum ClaseTopo : uint8_t {
    TOPO_FACE = 0,
    TOPO_EDGE = 1,
    TOPO_VERTEX = 2,
};

// Constante propia en vez de `M_PI`: no es estandar (MSVC la esconde detras de
// `_USE_MATH_DEFINES`), y no vale la pena el condicional de plataforma por un
// solo numero.
constexpr double PI = 3.14159265358979323846;

// Normal de superficie de una cara en un punto representativo (el centro de
// su rango parametrico). Es una aproximacion deliberada: `GeometrySignature`
// solo necesita distinguir caras entre si con tolerancia gruesa, no una
// normal de shading exacta (esa la calcula el teselado, por triangulo).
//
// VERIFICAR: la firma de `BRepGProp_Face::Normal` y si ya tiene en cuenta
// `face.Orientation()` internamente cambia algo entre versiones de OCCT;
// confirmar contra la version instalada.
gp_Vec normal_representativa(const TopoDS_Face& cara) {
    Standard_Real umin, umax, vmin, vmax;
    BRepTools::UVBounds(cara, umin, umax, vmin, vmax);
    BRepGProp_Face propiedad(cara);
    gp_Pnt p;
    gp_Vec n;
    propiedad.Normal((umin + umax) * 0.5, (vmin + vmax) * 0.5, p, n);
    if (n.Magnitude() > 1e-12) n.Normalize();
    return n;
}

ForgeTopoEntidad entidad_de_cara(const TopoDS_Face& cara, uint64_t mark) {
    GProp_GProps props;
    BRepGProp::SurfaceProperties(cara, props);
    gp_Pnt c = props.CentreOfMass();
    gp_Vec n = normal_representativa(cara);
    ForgeTopoEntidad e{};
    e.mark = mark;
    e.clase = TOPO_FACE;
    e.centroide[0] = c.X();
    e.centroide[1] = c.Y();
    e.centroide[2] = c.Z();
    e.normal[0] = n.X();
    e.normal[1] = n.Y();
    e.normal[2] = n.Z();
    e.medida = props.Mass();
    return e;
}

ForgeTopoEntidad entidad_de_arista(const TopoDS_Edge& arista, uint64_t mark) {
    GProp_GProps props;
    BRepGProp::LinearProperties(arista, props);
    gp_Pnt c = props.CentreOfMass();
    ForgeTopoEntidad e{};
    e.mark = mark;
    e.clase = TOPO_EDGE;
    e.centroide[0] = c.X();
    e.centroide[1] = c.Y();
    e.centroide[2] = c.Z();
    e.normal[0] = e.normal[1] = e.normal[2] = 0.0;
    if (!BRep_Tool::Degenerated(arista)) {
        Standard_Real u0, u1;
        Handle(Geom_Curve) curva = BRep_Tool::Curve(arista, u0, u1);
        if (!curva.IsNull()) {
            gp_Pnt p;
            gp_Vec tangente;
            curva->D1((u0 + u1) * 0.5, p, tangente);
            if (tangente.Magnitude() > 1e-12) {
                tangente.Normalize();
                e.normal[0] = tangente.X();
                e.normal[1] = tangente.Y();
                e.normal[2] = tangente.Z();
            }
        }
    }
    e.medida = props.Mass();
    return e;
}

// Clasifica una arista para dibujado (ver `forge_kernel_api::EdgeKind`).
//
// VERIFICAR: el orden de los valores de `GeomAbs_Shape` (C0 < G1 < C1 < G2 <
// C2 < C3 < CN) se asume estable entre versiones porque es parte de la ABI
// publica de OCCT desde hace mucho, pero conviene confirmarlo una vez.
ClaseArista clasificar_arista(
    const TopoDS_Edge& arista,
    const TopTools_IndexedDataMapOfShapeListOfShape& caras_por_arista) {
    if (BRep_Tool::Degenerated(arista)) return ARISTA_DEGENERATE;

    std::vector<TopoDS_Face> vecinas;
    if (caras_por_arista.Contains(arista)) {
        const TopTools_ListOfShape& lista = caras_por_arista.FindFromKey(arista);
        for (TopTools_ListIteratorOfListOfShape it(lista); it.More(); it.Next()) {
            vecinas.push_back(TopoDS::Face(it.Value()));
        }
    }

    if (vecinas.empty()) return ARISTA_BOUNDARY; // no deberia pasar en un solido
    if (vecinas.size() == 1) {
        // Misma cara, referenciada dos veces por su costura de parametrizacion
        // (el "seam" de un cilindro o una esfera) frente a un borde libre real.
        return BRepTools::IsReallyClosed(arista, vecinas[0]) ? ARISTA_SEAM
                                                              : ARISTA_BOUNDARY;
    }
    GeomAbs_Shape continuidad = BRep_Tool::Continuity(arista, vecinas[0], vecinas[1]);
    return continuidad >= GeomAbs_G1 ? ARISTA_SMOOTH : ARISTA_SHARP;
}

} // namespace

// ---------------------------------------------------------------------------
// API exportada
// ---------------------------------------------------------------------------

extern "C" {

bool forge_occt_load_step(const uint8_t* datos, size_t len, uint64_t** ids_out,
                           size_t* count_out, char** err_out) {
    *err_out = nullptr;
    *ids_out = nullptr;
    *count_out = 0;
    try {
        std::lock_guard<std::mutex> candado(g_mutex);

        // Global de OCCT: fijarlo explicitamente en cada carga evita que una
        // llamada de otro sitio lo deje en otra unidad y las cargas
        // posteriores cambien en silencio (hallazgo de cadviz,
        // docs/fase-0/00-arquitectura.md #4).
        Interface_Static::SetCVal("xstep.cascade.unit", "MM");

        std::string contenido(reinterpret_cast<const char*>(datos), len);
        std::istringstream entrada(contenido);

        STEPControl_Reader lector;
        // VERIFICAR: `ReadStream` para lectura en memoria existe en OCCT
        // moderno (>= 7.6 aprox.); si la version instalada no lo tiene, la
        // alternativa es volcar `datos` a un archivo temporal y usar
        // `lector.ReadFile(ruta)`.
        IFSelect_ReturnStatus estado = lector.ReadStream("forge_step", entrada);
        if (estado != IFSelect_RetDone) {
            *err_out = duplicar_error(
                "STEP: fallo la lectura (codigo " + std::to_string(static_cast<int>(estado)) + ")");
            return false;
        }

        lector.TransferRoots();
        Standard_Integer n = lector.NbShapes();
        if (n <= 0) {
            *err_out = duplicar_error("STEP: el archivo no contiene formas transferibles");
            return false;
        }

        std::vector<uint64_t> ids;
        ids.reserve(static_cast<size_t>(n));
        for (Standard_Integer i = 1; i <= n; ++i) {
            ids.push_back(guardar_forma_sin_candado(lector.Shape(i)));
        }

        *ids_out = copiar_vector(ids);
        *count_out = ids.size();
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (const std::exception& e) {
        *err_out = duplicar_error(std::string("excepcion: ") + e.what());
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en load_step");
        return false;
    }
}

void forge_occt_release(uint64_t handle) {
    // No hay try/catch: `unordered_map::erase` con una clave que puede no
    // existir no lanza. `release` en forge-kernel-api no es fallible; si esto
    // alguna vez pudiera lanzar, tendria que dejar de serlo silenciosamente
    // en vez de propagar un panico a Rust.
    std::lock_guard<std::mutex> candado(g_mutex);
    g_formas.erase(handle);
}

bool forge_occt_bbox(uint64_t handle, double min_out[3], double max_out[3],
                      char** err_out) {
    *err_out = nullptr;
    try {
        std::lock_guard<std::mutex> candado(g_mutex);
        const TopoDS_Shape* forma = buscar_forma_sin_candado(handle);
        if (!forma) {
            *err_out = duplicar_error("handle desconocido");
            return false;
        }

        Bnd_Box caja;
        // useTriangulation = false: si no, la caja crece con el margen de
        // deflexion del ultimo teselado y CAMBIA al re-teselar -- con
        // teselado adaptativo eso hace saltar la camara en cada rueda de
        // raton (hallazgo de cadviz, docs/fase-0/00-arquitectura.md #4).
        BRepBndLib::Add(*forma, caja, /*useTriangulation=*/Standard_False);
        if (caja.IsVoid()) {
            *err_out = duplicar_error("bbox vacia: la forma no tiene geometria");
            return false;
        }
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        caja.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        min_out[0] = xmin;
        min_out[1] = ymin;
        min_out[2] = zmin;
        max_out[0] = xmax;
        max_out[1] = ymax;
        max_out[2] = zmax;
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en bbox");
        return false;
    }
}

bool forge_occt_mass_properties(uint64_t handle, double* volumen_mm3_out,
                                 double* area_mm2_out, double centroide_out[3],
                                 char** err_out) {
    *err_out = nullptr;
    try {
        std::lock_guard<std::mutex> candado(g_mutex);
        const TopoDS_Shape* forma = buscar_forma_sin_candado(handle);
        if (!forma) {
            *err_out = duplicar_error("handle desconocido");
            return false;
        }

        GProp_GProps vol, sup;
        BRepGProp::VolumeProperties(*forma, vol);
        BRepGProp::SurfaceProperties(*forma, sup);
        *volumen_mm3_out = vol.Mass();
        *area_mm2_out = sup.Mass();
        gp_Pnt c = vol.CentreOfMass();
        centroide_out[0] = c.X();
        centroide_out[1] = c.Y();
        centroide_out[2] = c.Z();
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en mass_properties");
        return false;
    }
}

bool forge_occt_topology(uint64_t handle, ForgeTopologia* out, char** err_out) {
    *err_out = nullptr;
    *out = ForgeTopologia{};
    try {
        std::lock_guard<std::mutex> candado(g_mutex);
        const TopoDS_Shape* forma = buscar_forma_sin_candado(handle);
        if (!forma) {
            *err_out = duplicar_error("handle desconocido");
            return false;
        }

        // Mapas indexados: cada subforma unica recibe un indice de 1..N.
        // Es estable *mientras la forma no cambie*, que es la unica garantia
        // que este handle ofrece (no hay edicion en el lado consumidor).
        TopTools_IndexedMapOfShape mapa_caras, mapa_aristas, mapa_vertices;
        TopExp::MapShapes(*forma, TopAbs_FACE, mapa_caras);
        TopExp::MapShapes(*forma, TopAbs_EDGE, mapa_aristas);
        TopExp::MapShapes(*forma, TopAbs_VERTEX, mapa_vertices);

        std::vector<ForgeTopoEntidad> caras, aristas, vertices;
        caras.reserve(static_cast<size_t>(mapa_caras.Extent()));
        for (Standard_Integer i = 1; i <= mapa_caras.Extent(); ++i) {
            caras.push_back(entidad_de_cara(TopoDS::Face(mapa_caras.FindKey(i)),
                                             static_cast<uint64_t>(i)));
        }
        aristas.reserve(static_cast<size_t>(mapa_aristas.Extent()));
        for (Standard_Integer i = 1; i <= mapa_aristas.Extent(); ++i) {
            aristas.push_back(entidad_de_arista(TopoDS::Edge(mapa_aristas.FindKey(i)),
                                                 static_cast<uint64_t>(i)));
        }
        vertices.reserve(static_cast<size_t>(mapa_vertices.Extent()));
        for (Standard_Integer i = 1; i <= mapa_vertices.Extent(); ++i) {
            gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(mapa_vertices.FindKey(i)));
            ForgeTopoEntidad e{};
            e.mark = static_cast<uint64_t>(i);
            e.clase = TOPO_VERTEX;
            e.centroide[0] = p.X();
            e.centroide[1] = p.Y();
            e.centroide[2] = p.Z();
            e.medida = 0.0;
            vertices.push_back(e);
        }

        out->caras = copiar_vector(caras);
        out->caras_count = caras.size();
        out->aristas = copiar_vector(aristas);
        out->aristas_count = aristas.size();
        out->vertices = copiar_vector(vertices);
        out->vertices_count = vertices.size();
        TopAbs_ShapeEnum tipo = forma->ShapeType();
        out->es_solido = (tipo == TopAbs_SOLID || tipo == TopAbs_COMPSOLID) ? 1 : 0;
        out->es_cerrado = forma->Closed() ? 1 : 0;
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en topology");
        return false;
    }
}

void forge_occt_free_topology(ForgeTopologia* t) {
    if (!t) return;
    delete[] t->caras;
    delete[] t->aristas;
    delete[] t->vertices;
    *t = ForgeTopologia{};
}

bool forge_occt_tessellate(uint64_t handle, double chord_mm, double angular_deg,
                            ForgeTeselado* out, char** err_out) {
    *err_out = nullptr;
    *out = ForgeTeselado{};
    try {
        std::lock_guard<std::mutex> candado(g_mutex);
        const TopoDS_Shape* forma_const = buscar_forma_sin_candado(handle);
        if (!forma_const) {
            *err_out = duplicar_error("handle desconocido");
            return false;
        }
        // BRepMesh_IncrementalMesh muta la forma (le anexa la triangulacion
        // como dato auxiliar); se opera sobre una copia superficial para no
        // alterar la forma almacenada mientras otro hilo la lee. `TopoDS_Shape`
        // es un handle liviano (comparticion de la representacion subyacente
        // via `TShape`), asi que copiarla aqui no duplica la geometria.
        TopoDS_Shape forma = *forma_const;

        // Compensacion del hallazgo de cadviz: la deflexion de OCCT acota la
        // desviacion de la curva que aproxima la superficie, no la del
        // interior de los triangulos -- entrega ~1.75x menos precision de la
        // que promete. Se compensa dividiendo entre 2.
        // (docs/fase-0/00-arquitectura.md #4).
        Standard_Real deflexion_lineal = std::max(chord_mm, 1e-6) / 2.0;
        Standard_Real deflexion_angular_rad =
            std::max(angular_deg, 0.1) * PI / 180.0;

        // VERIFICAR: la firma exacta de este constructor (que parametros son
        // posicionales vs. con valor por defecto) varia poco entre versiones
        // recientes, pero es el primer sitio a mirar si esto no compila.
        BRepMesh_IncrementalMesh malla(forma, deflexion_lineal,
                                        /*isRelative=*/Standard_False,
                                        deflexion_angular_rad,
                                        /*isInParallel=*/Standard_False);

        TopTools_IndexedMapOfShape mapa_caras, mapa_aristas;
        TopExp::MapShapes(forma, TopAbs_FACE, mapa_caras);
        TopExp::MapShapes(forma, TopAbs_EDGE, mapa_aristas);
        TopTools_IndexedDataMapOfShapeListOfShape caras_por_arista;
        TopExp::MapShapesAndAncestors(forma, TopAbs_EDGE, TopAbs_FACE, caras_por_arista);

        std::vector<double> posiciones, normales;
        std::vector<uint32_t> indices;
        std::vector<uint64_t> face_marks;

        for (Standard_Integer i = 1; i <= mapa_caras.Extent(); ++i) {
            TopoDS_Face cara = TopoDS::Face(mapa_caras.FindKey(i));
            TopLoc_Location ubicacion;
            Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(cara, ubicacion);
            if (tri.IsNull()) continue; // cara degenerada o sin area; se omite

            const gp_Trsf& transformacion = ubicacion.Transformation();
            bool invertida = (cara.Orientation() == TopAbs_REVERSED);
            uint64_t mark = static_cast<uint64_t>(i);

            // Vertices propios por cara (no compartidos): dos caras que
            // comparten una arista tienen normales distintas, y compartir el
            // vertice promediaria la normal en el canto (mismo razonamiento
            // que `StubKernel::teselar_poly`).
            uint32_t base = static_cast<uint32_t>(posiciones.size() / 3);
            for (Standard_Integer v = 1; v <= tri->NbNodes(); ++v) {
                gp_Pnt p = tri->Node(v).Transformed(transformacion);
                posiciones.push_back(p.X());
                posiciones.push_back(p.Y());
                posiciones.push_back(p.Z());
            }
            // Se reserva ya el hueco de normales de esta cara; se rellena mas
            // abajo con el promedio de los triangulos que tocan cada vertice.
            // Como los vertices no se comparten entre caras (arriba), promediar
            // aqui no suaviza cantos reales entre caras -- solo la curvatura
            // interna de esta cara, que es lo deseable en una superficie curva
            // importada. Evita ademas depender de si `Poly_Triangulation` trae
            // normales de nodo precalculadas (varia segun
            // `IMeshTools_Parameters`).
            normales.resize(posiciones.size(), 0.0);
            std::vector<gp_Vec> acumulado(tri->NbNodes(), gp_Vec(0, 0, 0));

            for (Standard_Integer t = 1; t <= tri->NbTriangles(); ++t) {
                Standard_Integer a, b, c;
                tri->Triangle(t).Get(a, b, c);
                if (invertida) std::swap(b, c);
                indices.push_back(base + static_cast<uint32_t>(a - 1));
                indices.push_back(base + static_cast<uint32_t>(b - 1));
                indices.push_back(base + static_cast<uint32_t>(c - 1));
                face_marks.push_back(mark);

                gp_Pnt pa = tri->Node(a).Transformed(transformacion);
                gp_Pnt pb = tri->Node(b).Transformed(transformacion);
                gp_Pnt pc = tri->Node(c).Transformed(transformacion);
                gp_Vec n = gp_Vec(pa, pb).Crossed(gp_Vec(pa, pc));
                acumulado[a - 1] += n;
                acumulado[b - 1] += n;
                acumulado[c - 1] += n;
            }
            for (Standard_Integer v = 1; v <= tri->NbNodes(); ++v) {
                gp_Vec n = acumulado[v - 1];
                if (n.Magnitude() > 1e-12) n.Normalize();
                size_t idx = (base + static_cast<uint32_t>(v - 1)) * 3;
                normales[idx] = n.X();
                normales[idx + 1] = n.Y();
                normales[idx + 2] = n.Z();
            }
        }

        // --- aristas: polilineas de la curva analitica, con clasificacion ---
        std::vector<uint64_t> edge_marks;
        std::vector<uint8_t> edge_kinds;
        std::vector<uint32_t> edge_offsets;
        std::vector<double> edge_points;
        edge_offsets.push_back(0);

        for (Standard_Integer i = 1; i <= mapa_aristas.Extent(); ++i) {
            TopoDS_Edge arista = TopoDS::Edge(mapa_aristas.FindKey(i));
            ClaseArista clase = clasificar_arista(arista, caras_por_arista);

            if (BRep_Tool::Degenerated(arista)) {
                // Sin longitud parametrica real (p.ej. el apice de un cono):
                // un unico punto basta para que exista la entrada, y nunca se
                // dibuja (EdgeKind::Degenerate::se_dibuja() == false).
                TopoDS_Vertex v0 = TopExp::FirstVertex(arista);
                gp_Pnt p = BRep_Tool::Pnt(v0);
                edge_points.push_back(p.X());
                edge_points.push_back(p.Y());
                edge_points.push_back(p.Z());
            } else {
                BRepAdaptor_Curve adaptador(arista);
                // VERIFICAR: el orden de parametros de
                // `GCPnts_TangentialDeflection` (angular antes que lineal) es
                // el habitual en OCCT, pero es facil de invertir sin querer.
                GCPnts_TangentialDeflection muestreo(
                    adaptador, deflexion_angular_rad, deflexion_lineal);
                Standard_Integer n = muestreo.NbPoints();
                for (Standard_Integer k = 1; k <= n; ++k) {
                    gp_Pnt p = muestreo.Value(k);
                    edge_points.push_back(p.X());
                    edge_points.push_back(p.Y());
                    edge_points.push_back(p.Z());
                }
            }
            edge_marks.push_back(static_cast<uint64_t>(i));
            edge_kinds.push_back(static_cast<uint8_t>(clase));
            edge_offsets.push_back(static_cast<uint32_t>(edge_points.size() / 3));
        }

        out->posiciones = copiar_vector(posiciones);
        out->normales = copiar_vector(normales);
        out->vertex_count = posiciones.size() / 3;
        out->indices = copiar_vector(indices);
        out->index_count = indices.size();
        out->face_marks = copiar_vector(face_marks);
        out->edge_marks = copiar_vector(edge_marks);
        out->edge_kinds = copiar_vector(edge_kinds);
        out->edge_point_offsets = copiar_vector(edge_offsets);
        out->edge_points = copiar_vector(edge_points);
        out->edge_count = edge_marks.size();

        Bnd_Box caja;
        BRepBndLib::Add(forma, caja, /*useTriangulation=*/Standard_False);
        if (!caja.IsVoid()) {
            Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
            caja.Get(xmin, ymin, zmin, xmax, ymax, zmax);
            out->bbox_min[0] = xmin;
            out->bbox_min[1] = ymin;
            out->bbox_min[2] = zmin;
            out->bbox_max[0] = xmax;
            out->bbox_max[1] = ymax;
            out->bbox_max[2] = zmax;
        }
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en tessellate");
        return false;
    }
}

void forge_occt_free_tessellation(ForgeTeselado* t) {
    if (!t) return;
    delete[] t->posiciones;
    delete[] t->normales;
    delete[] t->indices;
    delete[] t->face_marks;
    delete[] t->edge_marks;
    delete[] t->edge_kinds;
    delete[] t->edge_point_offsets;
    delete[] t->edge_points;
    *t = ForgeTeselado{};
}

bool forge_occt_serialize(uint64_t handle, uint8_t** datos_out, size_t* len_out,
                           char** err_out) {
    *err_out = nullptr;
    *datos_out = nullptr;
    *len_out = 0;
    try {
        std::lock_guard<std::mutex> candado(g_mutex);
        const TopoDS_Shape* forma = buscar_forma_sin_candado(handle);
        if (!forma) {
            *err_out = duplicar_error("handle desconocido");
            return false;
        }
        // Formato de texto BREP, no el binario: `BRepTools` esta en el
        // toolkit basico (TKBRep, siempre presente); el binario (`BinTools`)
        // vive en un toolkit aparte que este build.rs podria no haber
        // encontrado. Mas grande en disco, pero sin dependencia adicional.
        // TODO: pasar a `BinTools::Write` cuando el enlazado incluya ese
        // toolkit, por tamano.
        std::ostringstream salida;
        BRepTools::Write(*forma, salida);
        std::string texto = salida.str();

        uint8_t* buf = new uint8_t[texto.size()];
        std::memcpy(buf, texto.data(), texto.size());
        *datos_out = buf;
        *len_out = texto.size();
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en serialize");
        return false;
    }
}

bool forge_occt_deserialize(const uint8_t* datos, size_t len,
                             uint64_t* handle_out, char** err_out) {
    *err_out = nullptr;
    try {
        std::string texto(reinterpret_cast<const char*>(datos), len);
        std::istringstream entrada(texto);
        BRep_Builder builder;
        TopoDS_Shape forma;
        // `BRepTools::Read` con flujo (sin ruta de progreso) para no
        // depender de `Message_ProgressRange` en firmas antiguas.
        if (!BRepTools::Read(forma, entrada, builder)) {
            *err_out = duplicar_error("BREP: el flujo no se pudo interpretar como forma");
            return false;
        }
        std::lock_guard<std::mutex> candado(g_mutex);
        *handle_out = guardar_forma_sin_candado(std::move(forma));
        return true;
    } catch (const Standard_Failure& e) {
        *err_out = duplicar_error(
            std::string("OCCT: ") + (e.GetMessageString() ? e.GetMessageString() : "fallo sin mensaje"));
        return false;
    } catch (...) {
        *err_out = duplicar_error("excepcion desconocida atrapada en deserialize");
        return false;
    }
}

void forge_occt_free_string(char* s) { delete[] s; }

void forge_occt_free_ids(uint64_t* p, size_t) { delete[] p; }

void forge_occt_free_bytes(uint8_t* p, size_t) { delete[] p; }

} // extern "C"
