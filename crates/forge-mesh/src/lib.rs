//! Pilar 2 — malla poligonal, y con ella **la frontera de dominio**.
//!
//! Este crate es donde vive la decisión central del proyecto (ADR-0002). Las
//! tres reglas que su código hace cumplir:
//!
//! - **R1** — un teselado nunca es editable. La [`Tessellation`] que entra por
//!   [`to_mesh`] no se modifica jamás: se lee y se produce una [`Mesh`] nueva.
//!   El teselado es caché derivada; la malla es lo que el usuario edita.
//! - **R2** — el cruce es un nodo del grafo, no un comando destructivo. Aguas
//!   arriba se sigue editando paramétricamente para siempre; aguas abajo no se
//!   vuelve.
//! - **R3** — las referencias cruzan **en un solo sentido**, y llevan
//!   procedencia. Una operación de malla puede referenciar entidades del
//!   dominio exacto; nunca al revés. Por eso este crate depende de
//!   `forge-kernel-api` y el kernel no depende de este.
//!
//! # Lo que hace que la frontera funcione de verdad
//!
//! El mapa de procedencia. Sin él, `to_mesh` sería una conversión a sopa de
//! triángulos y una selección moriría en el primer cambio de cota. Mantenerlo a
//! través de **cada** modificador es la deuda técnica más cara de este pilar, y
//! por eso [`ProvenanceMap::cobertura`] existe: la comprobación tiene que ser
//! numérica, porque un modificador que devuelve un mapa vacío compila igual de
//! bien que uno correcto.

use forge_doc::{Binding, StableId};
use forge_kernel_api::Tessellation;
use forge_math::{tol, DVec3};

pub mod mesh;
pub mod modifiers;

pub use mesh::{Adjacency, Face, Mesh, ProvenanceMap};
pub use modifiers::{Array, Mirror, Modifier, ModifierStack, Subdivide, Triangulate, Weld};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeshError {
    #[error("malla corrupta: {0}")]
    Corrupta(String),
    #[error("el modificador `{modificador}` fallo: {detalle}")]
    Modificador { modificador: &'static str, detalle: String },
    #[error("parametro invalido en `{modificador}`: {detalle}")]
    Parametro { modificador: &'static str, detalle: String },
    #[error(
        "el modificador `{0}` perdio procedencia en {1} de {2} caras. \
         Todo modificador debe propagarla a la geometria que crea: sin eso, una \
         seleccion no sobrevive a una edicion aguas arriba."
    )]
    ProcedenciaPerdida(&'static str, usize, usize),
}

pub type Result<T> = std::result::Result<T, MeshError>;

// ---------------------------------------------------------------------------
// La puerta de un solo sentido
// ---------------------------------------------------------------------------

/// Cruza del dominio exacto al discreto.
///
/// **Es unidireccional y no hay inversa.** No existe `to_brep`, y no por falta
/// de tiempo: la reingeniería de una malla a superficies es un producto entero
/// (Geomagic, QuickSurface), no una función. Prometerla a medias es peor que no
/// ofrecerla, porque falla después de que el usuario ya invirtió días.
///
/// Lo que sí cruza intacto es la **identidad**: cada triángulo llega etiquetado
/// con el `StableId` de la cara que lo originó.
pub fn to_mesh(t: &Tessellation) -> Result<Mesh> {
    t.validate().map_err(|e| MeshError::Corrupta(e.to_string()))?;

    let mut m = Mesh {
        positions: t.positions.clone(),
        normals: t.normals.clone(),
        uvs: Vec::new(),
        faces: Vec::with_capacity(t.triangle_count()),
        prov: ProvenanceMap::con_capacidad(0),
    };
    for (i, tri) in t.indices.chunks_exact(3).enumerate() {
        m.faces.push(Face { verts: vec![tri[0], tri[1], tri[2]] });
        m.prov.face_origin.push(t.face_of(i));
    }
    // El teselado trae vértices duplicados por cara —para no promediar normales
    // a través de un canto vivo—, pero una malla editable necesita que las
    // caras compartan vértices o no hay adyacencia y ningún modificador
    // funciona.
    let m = soldar_conservando_procedencia(&m, tol::CONFUSION_MM * 10.0)?;
    m.validate()?;
    Ok(m)
}

/// Suelda vértices coincidentes sin tocar la procedencia de las caras.
pub(crate) fn soldar_conservando_procedencia(m: &Mesh, epsilon: f64) -> Result<Mesh> {
    if epsilon <= 0.0 {
        return Err(MeshError::Parametro {
            modificador: "weld",
            detalle: "epsilon debe ser positivo".into(),
        });
    }
    let q = |v: f64| (v / epsilon).round() as i64;
    let mut mapa: std::collections::HashMap<(i64, i64, i64), u32> = Default::default();
    let mut posiciones = Vec::with_capacity(m.positions.len());
    let mut normales = Vec::new();
    let mut remap = vec![0u32; m.positions.len()];

    for (i, p) in m.positions.iter().enumerate() {
        let clave = (q(p.x), q(p.y), q(p.z));
        let idx = *mapa.entry(clave).or_insert_with(|| {
            posiciones.push(*p);
            if !m.normals.is_empty() {
                normales.push(m.normals[i]);
            }
            posiciones.len() as u32 - 1
        });
        remap[i] = idx;
    }

    let mut faces = Vec::with_capacity(m.faces.len());
    let mut prov = ProvenanceMap::default();
    for (fi, f) in m.faces.iter().enumerate() {
        let mut vs: Vec<u32> = f.verts.iter().map(|&v| remap[v as usize]).collect();
        vs.dedup();
        if vs.len() > 1 && vs[0] == *vs.last().unwrap() {
            vs.pop();
        }
        // Una cara que colapsa a menos de 3 vértices desaparece: mantenerla
        // produciría una malla que `validate` rechaza y un volumen erróneo.
        if vs.len() < 3 {
            continue;
        }
        faces.push(Face { verts: vs });
        prov.face_origin.push(m.prov.face_origin.get(fi).copied().flatten());
    }

    Ok(Mesh { positions: posiciones, normals: normales, uvs: Vec::new(), faces, prov })
}

// ---------------------------------------------------------------------------
// Re-vinculación
// ---------------------------------------------------------------------------

/// Resultado de re-vincular una selección tras un cambio aguas arriba.
#[derive(Clone, Debug, PartialEq)]
pub struct Rebind {
    pub id: StableId,
    pub binding: Binding<u32>,
    /// Caras que llevan esa procedencia en la malla nueva.
    pub faces: Vec<u32>,
}

/// Re-vincula una selección hecha sobre una malla anterior.
///
/// **Lo que no resuelve sale `Broken` y se muestra.** Nunca se re-vincula en
/// silencio a la candidata más parecida: una selección mal re-vinculada produce
/// un modelo plausible pero incorrecto, que es peor que un error visible porque
/// el usuario no tiene forma de enterarse.
pub fn rebind(seleccion: &[StableId], nueva: &Mesh) -> Vec<Rebind> {
    seleccion
        .iter()
        .map(|&id| {
            let faces = nueva.prov.caras_de(id);
            let binding = match faces.first() {
                Some(&f) => Binding::Bound(f),
                None => Binding::Broken,
            };
            Rebind { id, binding, faces }
        })
        .collect()
}

/// Cuántas referencias de una selección sobreviven, de 0 a 1.
///
/// El criterio de aceptación de la Fase 3 es **≥0.95** ante un cambio de cota
/// típico. Es una métrica, no una impresión.
pub fn tasa_de_revinculacion(seleccion: &[StableId], nueva: &Mesh) -> f64 {
    if seleccion.is_empty() {
        return 1.0;
    }
    let vivos = rebind(seleccion, nueva).iter().filter(|r| !r.binding.is_broken()).count();
    vivos as f64 / seleccion.len() as f64
}

// ---------------------------------------------------------------------------
// Primitivas para tests y para construir a mano
// ---------------------------------------------------------------------------

/// Cubo de lado `l` centrado en el origen, con procedencia por cara.
pub fn cubo(l: f64, origen: forge_doc::FeatureId) -> Mesh {
    let h = l * 0.5;
    let p = |x: f64, y: f64, z: f64| DVec3::new(x * h, y * h, z * h);
    let positions = vec![
        p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0),
        p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0),
    ];
    let bucles: [[u32; 4]; 6] = [
        [0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3],
    ];
    let faces: Vec<Face> = bucles.iter().map(|b| Face { verts: b.to_vec() }).collect();
    let prov = ProvenanceMap {
        face_origin: (0..6)
            .map(|i| {
                Some(StableId {
                    origin: origen,
                    class: forge_doc::TopoClass::Face,
                    mark: 1000 + i as u64,
                })
            })
            .collect(),
    };
    Mesh { positions, normals: Vec::new(), uvs: Vec::new(), faces, prov }
}
