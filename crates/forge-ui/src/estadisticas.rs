//! Recuento de un documento, para la barra de estado y para `forge --stats`.

use forge_doc::{Geometry, Snapshot, Visible};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Estadisticas {
    pub entidades: usize,
    pub con_geometria: usize,
    pub visibles: usize,
    pub ocultas: usize,
    pub blobs_referenciados: usize,
    pub dominio_exacto: usize,
    pub dominio_discreto: usize,
}

/// Calcula el recuento del snapshot dado. Pura: no toca disco ni GPU, así que
/// es exactamente lo que corre `forge --stats` y exactamente lo que se
/// verifica aquí.
pub fn calcular(snapshot: &Snapshot) -> Estadisticas {
    let mut e = Estadisticas {
        entidades: snapshot.entity_count(),
        blobs_referenciados: snapshot.referenced_blobs().len(),
        ..Estadisticas::default()
    };

    for (entidad, g) in snapshot.iter::<Geometry>() {
        e.con_geometria += 1;
        match g.0.domain() {
            forge_doc::Domain::Exact => e.dominio_exacto += 1,
            forge_doc::Domain::Discrete => e.dominio_discreto += 1,
        }
        let visible = snapshot
            .get::<Visible>(entidad)
            .map(|v| v.0)
            .unwrap_or(true);
        if visible {
            e.visibles += 1;
        } else {
            e.ocultas += 1;
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::{Document, GeometryPayload};
    use forge_store::BlobHash;

    #[test]
    fn cuenta_entidades_geometria_visibilidad_y_blobs() {
        let mut doc = Document::new();
        let h1 = BlobHash::of(b"a");
        let h2 = BlobHash::of(b"b");
        doc.edit("preparar", |tx| {
            let e1 = tx.spawn();
            tx.set(e1, Geometry(GeometryPayload::Mesh(h1)));
            tx.set(e1, Visible(true));

            let e2 = tx.spawn();
            tx.set(e2, Geometry(GeometryPayload::Brep(h2)));
            tx.set(e2, Visible(false));

            // reutiliza h1: el blob no debe contarse dos veces
            let e3 = tx.spawn();
            tx.set(e3, Geometry(GeometryPayload::Mesh(h1)));

            // sin geometria
            let _sin_geo = tx.spawn();
        });

        let snap = doc.snapshot();
        let stats = calcular(&snap);
        assert_eq!(stats.entidades, 4);
        assert_eq!(stats.con_geometria, 3);
        assert_eq!(stats.visibles, 2); // e1 explicita, e3 por defecto
        assert_eq!(stats.ocultas, 1);
        assert_eq!(stats.dominio_exacto, 1);
        assert_eq!(stats.dominio_discreto, 2);
        assert_eq!(stats.blobs_referenciados, 2, "h1 y h2, sin duplicar h1");
    }

    #[test]
    fn documento_vacio_da_estadisticas_en_cero() {
        let doc = Document::new();
        let stats = calcular(&doc.snapshot());
        assert_eq!(stats, Estadisticas::default());
    }
}
