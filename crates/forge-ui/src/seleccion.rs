//! Selección por clic.
//!
//! Un clic es un arrastre de **menos de 3 píxeles**. Sin ese umbral, soltar el
//! botón tras orbitar la cámara selecciona lo que quedó bajo el cursor en ese
//! instante — que casi nunca es lo que el usuario quería mirar de cerca.

use forge_doc::EntityId;

/// Por debajo de este desplazamiento (en píxeles), soltar el botón es un clic.
/// En él y por encima, es el final de un arrastre (órbita, pan) y no
/// selecciona nada.
pub const UMBRAL_CLIC_PX: f32 = 3.0;

/// `true` si el desplazamiento entre el botón bajado y el botón soltado es
/// menor que [`UMBRAL_CLIC_PX`].
pub fn es_clic(dx_px: f32, dy_px: f32) -> bool {
    dx_px.hypot(dy_px) < UMBRAL_CLIC_PX
}

/// Candidato de picking: una entidad con su posición ya proyectada a pantalla
/// y su profundidad de vista (mayor = más lejos de la cámara).
#[derive(Clone, Copy, Debug)]
pub struct CandidatoPicking {
    pub entity: EntityId,
    pub pantalla_px: [f32; 2],
    pub profundidad: f32,
}

/// Entidad más cercana al punto de clic, dentro de `radio_px`.
///
/// Empates de distancia se resuelven por menor profundidad (lo más cercano a
/// la cámara gana), que es el comportamiento esperado al hacer clic donde dos
/// siluetas se superponen en pantalla.
pub fn mas_cercana(
    clic_px: [f32; 2],
    candidatos: &[CandidatoPicking],
    radio_px: f32,
) -> Option<EntityId> {
    candidatos
        .iter()
        .map(|c| {
            let dx = c.pantalla_px[0] - clic_px[0];
            let dy = c.pantalla_px[1] - clic_px[1];
            (dx.hypot(dy), c)
        })
        .filter(|(d, _)| *d <= radio_px)
        .min_by(|(d1, c1), (d2, c2)| {
            d1.partial_cmp(d2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    c1.profundidad
                        .partial_cmp(&c2.profundidad)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .map(|(_, c)| c.entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::EntityId;

    #[test]
    fn sin_desplazamiento_es_clic() {
        assert!(es_clic(0.0, 0.0));
    }

    #[test]
    fn justo_debajo_del_umbral_es_clic() {
        assert!(es_clic(2.99, 0.0));
        assert!(es_clic(0.0, -2.99));
        // 2,2 -> hipotenusa 2.828, sigue por debajo de 3
        assert!(es_clic(2.0, 2.0));
    }

    #[test]
    fn en_el_umbral_o_por_encima_no_es_clic() {
        assert!(!es_clic(3.0, 0.0), "3.0 exacto ya es arrastre, no clic");
        assert!(!es_clic(4.0, 0.0));
        // 3,4 -> hipotenusa exacta 5
        assert!(!es_clic(3.0, 4.0));
    }

    #[test]
    fn picking_elige_la_mas_cercana_dentro_del_radio() {
        let a = EntityId::from_u128(1);
        let b = EntityId::from_u128(2);
        let lejos = EntityId::from_u128(3);
        let candidatos = [
            CandidatoPicking {
                entity: a,
                pantalla_px: [10.0, 10.0],
                profundidad: 5.0,
            },
            CandidatoPicking {
                entity: b,
                pantalla_px: [12.0, 10.0],
                profundidad: 5.0,
            },
            CandidatoPicking {
                entity: lejos,
                pantalla_px: [500.0, 500.0],
                profundidad: 1.0,
            },
        ];
        // el clic cae más cerca de `a` (dist 0) que de `b` (dist 2)
        let elegido = mas_cercana([10.0, 10.0], &candidatos, 20.0);
        assert_eq!(elegido, Some(a));
    }

    #[test]
    fn picking_ignora_candidatos_fuera_de_radio() {
        let a = EntityId::from_u128(1);
        let candidatos = [CandidatoPicking {
            entity: a,
            pantalla_px: [100.0, 100.0],
            profundidad: 1.0,
        }];
        assert_eq!(mas_cercana([0.0, 0.0], &candidatos, 5.0), None);
    }

    #[test]
    fn empate_de_distancia_gana_el_de_menor_profundidad() {
        let cerca = EntityId::from_u128(1);
        let lejos = EntityId::from_u128(2);
        let candidatos = [
            CandidatoPicking {
                entity: lejos,
                pantalla_px: [10.0, 0.0],
                profundidad: 10.0,
            },
            CandidatoPicking {
                entity: cerca,
                pantalla_px: [-10.0, 0.0],
                profundidad: 1.0,
            },
        ];
        // ambos a distancia 10 del origen en pantalla
        assert_eq!(mas_cercana([0.0, 0.0], &candidatos, 50.0), Some(cerca));
    }

    #[test]
    fn sin_candidatos_no_hay_seleccion() {
        assert_eq!(mas_cercana([0.0, 0.0], &[], 100.0), None);
    }
}
