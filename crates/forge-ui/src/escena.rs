//! La frontera de extracción: `Snapshot` de `forge-doc` → `DrawInstance` de
//! `forge-render-api`.
//!
//! Es la función que impide que el renderer conozca el documento (el propio
//! contrato de `forge-render-api` lo exige) y, de paso, la que permite que un
//! futuro `forge-runtime` comparta exactamente este mismo camino: si el
//! runtime también sabe producir un `Snapshot`, ya sabe alimentar cualquier
//! `Renderer`.
//!
//! # Qué entidades se dibujan
//!
//! Solo las que tienen [`Geometry`] con payload de **dominio discreto**
//! (`Mesh`). Una entidad con un B-Rep o un sketch (dominio exacto) no tiene
//! triángulos que subir a un `DrawInstance` — hace falta tesela antes, y eso es
//! trabajo de `forge-mesh` / `forge-kernel-*`, no de esta capa. En la práctica
//! un documento suele tener la entidad exacta y, colgando de ella, una entidad
//! hija con la malla ya teselada (así lo hace `examples/cadena_completa.rs`):
//! esta función dibuja la segunda y ​omite la primera calladamente, que es
//! exactamente lo que se espera de una vista "aplanada".
//!
//! # Cajas de mundo
//!
//! `DrawInstance::bounds` debería ser la caja del contenido real de la malla,
//! pero esta capa no resuelve mallas (no le corresponde: el hash es una
//! referencia opaca al almacén de blobs). Por eso la caja local se pide a un
//! [`ResolutorDeMallas`] inyectado por quien llama. Sin resolutor —o si el
//! resolutor no conoce ese hash todavía— la caja es [`Aabb::EMPTY`], el
//! elemento neutro: la instancia se dibuja iguallosmente, solo que no participa
//! del encuadre automático hasta que su malla esté disponible.

use forge_doc::{EntityId, Geometry, GeometryPayload, Snapshot, Visible};
use forge_math::Aabb;
use forge_render_api::{DrawInstance, MaterialId};
use forge_store::BlobHash;

/// Traduce el hash de una malla a su caja local (sin transformar).
///
/// El paralelo es deliberado con `forge_render_cpu::MeshProvider`: la política
/// de residencia (todo en memoria, caché LRU, teselado bajo demanda...) es de
/// quien implementa el trait, nunca de esta función de extracción.
pub trait ResolutorDeMallas {
    /// `None` cuando la malla todavía no está disponible. No es un error.
    fn caja_local(&self, hash: BlobHash) -> Option<Aabb>;
}

/// Resolutor por defecto: no conoce ninguna malla.
///
/// Con este resolutor todas las instancias se dibujan (si el renderer sabe
/// interpretar su hash) pero ninguna aporta caja al encuadre automático.
/// Documentado en el README de arranque: la resolución de geometría real
/// (hash → posiciones/índices) queda fuera del alcance de esta tarea.
#[derive(Clone, Copy, Debug, Default)]
pub struct SinResolucion;

impl ResolutorDeMallas for SinResolucion {
    fn caja_local(&self, _hash: BlobHash) -> Option<Aabb> {
        None
    }
}

/// Extrae las instancias dibujables del estado actual del documento.
///
/// `seleccion` marca qué entidades aparecen con `DrawInstance::selected`. No
/// hace falta que sea eficiente para selecciones enormes: en un documento CAD
/// la selección son unidades o decenas de entidades, no miles.
pub fn extraer_instancias(
    snapshot: &Snapshot,
    seleccion: &[EntityId],
    resolutor: &dyn ResolutorDeMallas,
) -> Vec<DrawInstance> {
    snapshot
        .iter::<Geometry>()
        .filter_map(|(e, g)| {
            let GeometryPayload::Mesh(hash) = g.0 else {
                return None;
            };
            let visible = snapshot.get::<Visible>(e).map(|v| v.0).unwrap_or(true);
            if !visible {
                return None;
            }
            let mundo = snapshot.world_transform(e);
            let local = resolutor.caja_local(hash).unwrap_or(Aabb::EMPTY);
            Some(DrawInstance {
                entity: e,
                mesh: hash,
                material: MaterialId::DEFAULT,
                transform: mundo,
                bounds: local.transformed(&mundo),
                visible: true,
                selected: seleccion.contains(&e),
            })
        })
        .collect()
}

/// Caja que engloba todas las instancias dadas, en mundo.
///
/// Es lo que consume "encuadrar todo" (`F` sin selección). Si no hay ninguna
/// instancia con caja conocida, devuelve [`Aabb::EMPTY`] y quien llama decide
/// qué hacer (normalmente: no mover la cámara).
pub fn caja_de_conjunto(instancias: &[DrawInstance]) -> Aabb {
    instancias
        .iter()
        .fold(Aabb::EMPTY, |acc, i| acc.union(i.bounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::{Document, Name, Parent, Transform};
    use forge_math::DVec3;
    use forge_store::BlobHash;

    fn hash_de(bytes: &[u8]) -> BlobHash {
        BlobHash::of(bytes)
    }

    /// Resolutor de prueba: responde con una caja conocida para un hash
    /// conocido, para poder verificar `bounds` sin decodificar ninguna malla
    /// real.
    struct ResolutorFijo(BlobHash, Aabb);
    impl ResolutorDeMallas for ResolutorFijo {
        fn caja_local(&self, hash: BlobHash) -> Option<Aabb> {
            (hash == self.0).then_some(self.1)
        }
    }

    #[test]
    fn cuenta_solo_las_entidades_con_malla_discreta() {
        let mut doc = Document::new();
        let h = hash_de(b"malla-1");
        doc.edit("preparar", |tx| {
            // con malla: cuenta
            let e1 = tx.spawn();
            tx.set(e1, Geometry(GeometryPayload::Mesh(h)));
            tx.set(e1, Visible(true));

            // solo B-Rep, dominio exacto: no cuenta (no hay triángulos aún)
            let e2 = tx.spawn();
            tx.set(e2, Geometry(GeometryPayload::Brep(hash_de(b"brep"))));
            tx.set(e2, Visible(true));

            // sin componente Geometry en absoluto: no cuenta
            let e3 = tx.spawn();
            tx.set(e3, Name("sin geometria".into()));
            let _ = e3;
        });

        let snap = doc.snapshot();
        let out = extraer_instancias(&snap, &[], &SinResolucion);
        assert_eq!(out.len(), 1, "solo la entidad con Geometry::Mesh cuenta");
        assert_eq!(out[0].mesh, h);
    }

    #[test]
    fn excluye_las_invisibles() {
        let mut doc = Document::new();
        let h = hash_de(b"malla");
        doc.edit("preparar", |tx| {
            let visible = tx.spawn();
            tx.set(visible, Geometry(GeometryPayload::Mesh(h)));
            tx.set(visible, Visible(true));

            let oculta = tx.spawn();
            tx.set(oculta, Geometry(GeometryPayload::Mesh(h)));
            tx.set(oculta, Visible(false));

            // sin componente Visible: por defecto se considera visible
            let por_defecto = tx.spawn();
            tx.set(por_defecto, Geometry(GeometryPayload::Mesh(h)));
        });

        let snap = doc.snapshot();
        let out = extraer_instancias(&snap, &[], &SinResolucion);
        assert_eq!(out.len(), 2, "la oculta explícitamente no debe aparecer");
        assert!(out.iter().all(|i| i.visible));
    }

    #[test]
    fn la_transformada_de_mundo_compone_padre_e_hijo() {
        let mut doc = Document::new();
        let h = hash_de(b"malla");
        let hijo = doc.edit("preparar", |tx| {
            let padre = tx.spawn();
            tx.set(
                padre,
                Transform::from_translation(DVec3::new(10.0, 0.0, 0.0)),
            );

            let hijo = tx.spawn();
            tx.set(hijo, Parent(padre));
            tx.set(hijo, Transform::from_translation(DVec3::new(0.0, 5.0, 0.0)));
            tx.set(hijo, Geometry(GeometryPayload::Mesh(h)));
            hijo
        });

        let snap = doc.snapshot();
        let out = extraer_instancias(&snap, &[], &SinResolucion);
        assert_eq!(out.len(), 1);
        let mundo = out[0].transform;
        let origen = mundo.transform_point3(DVec3::ZERO);
        // conocido a mano: traslación del padre (10,0,0) + la del hijo (0,5,0)
        assert!(
            (origen - DVec3::new(10.0, 5.0, 0.0)).length() < forge_math::tol::CONFUSION_MM,
            "esperaba (10,5,0), fue {origen:?}"
        );
        assert_eq!(out[0].entity, hijo);
    }

    #[test]
    fn marca_la_seleccion() {
        let mut doc = Document::new();
        let h = hash_de(b"malla");
        let (e1, e2) = doc.edit("preparar", |tx| {
            let e1 = tx.spawn();
            tx.set(e1, Geometry(GeometryPayload::Mesh(h)));
            let e2 = tx.spawn();
            tx.set(e2, Geometry(GeometryPayload::Mesh(h)));
            (e1, e2)
        });

        let snap = doc.snapshot();
        let out = extraer_instancias(&snap, &[e1], &SinResolucion);
        let sel: std::collections::HashMap<_, _> =
            out.iter().map(|i| (i.entity, i.selected)).collect();
        assert!(sel[&e1]);
        assert!(!sel[&e2]);
    }

    #[test]
    fn la_caja_de_mundo_usa_el_resolutor_y_se_transforma() {
        let mut doc = Document::new();
        let h = hash_de(b"malla");
        // caja local: cubo unidad centrado en el origen
        let local = Aabb::new(DVec3::splat(-0.5), DVec3::splat(0.5));
        doc.edit("preparar", |tx| {
            let e = tx.spawn();
            tx.set(e, Transform::from_translation(DVec3::new(100.0, 0.0, 0.0)));
            tx.set(e, Geometry(GeometryPayload::Mesh(h)));
        });

        let snap = doc.snapshot();
        let resolutor = ResolutorFijo(h, local);
        let out = extraer_instancias(&snap, &[], &resolutor);
        assert_eq!(out.len(), 1);
        // conocido a mano: el cubo unidad trasladado 100 en X
        let esperado = Aabb::new(DVec3::new(99.5, -0.5, -0.5), DVec3::new(100.5, 0.5, 0.5));
        assert!((out[0].bounds.min - esperado.min).length() < 1e-9);
        assert!((out[0].bounds.max - esperado.max).length() < 1e-9);
    }

    #[test]
    fn sin_resolutor_la_caja_es_vacia_y_no_participa_del_encuadre() {
        let mut doc = Document::new();
        let h = hash_de(b"malla-sin-resolver");
        doc.edit("preparar", |tx| {
            let e = tx.spawn();
            tx.set(e, Geometry(GeometryPayload::Mesh(h)));
        });
        let snap = doc.snapshot();
        let out = extraer_instancias(&snap, &[], &SinResolucion);
        assert_eq!(out.len(), 1);
        assert!(out[0].bounds.is_empty());
        assert!(caja_de_conjunto(&out).is_empty());
    }
}
