//! Iluminación basada en imagen: exposición medida y empaquetado de los
//! armónicos esféricos para el uniforme de GPU.
//!
//! El BRDF y la evaluación de los propios SH viven en WGSL (`shaders/pbr.wgsl`)
//! porque corren por fragmento en la GPU; lo que queda aquí, en Rust puro, es
//! exactamente lo que **no** depende de la GPU y por tanto sí se puede
//! verificar en este entorno sin adaptador: la irradiancia media de un entorno
//! y la exposición que se deriva de ella. Es la misma fórmula que
//! `forge-render-cpu::sombreado`, reproducida en vez de importada por la regla
//! de que los pilares no se conocen entre sí (ADR-0006).

use forge_render_api::{Ibl, Light, SceneView};

/// `C4 * Y00`, la constante que convierte el coeficiente `l=0` de los SH de
/// radiancia en irradiancia media sobre la esfera. Ver la derivación en
/// `forge-render-cpu::sombreado::irradiancia_media`: los términos `l >= 1`
/// promedian cero sobre la esfera, así que la media es un número exacto y no
/// una integración numérica.
const C4: f32 = 0.886_227;

/// Irradiancia media sobre todas las direcciones, en W/m² por canal.
pub fn irradiancia_media(ibl: &Ibl) -> [f32; 3] {
    [
        (C4 * ibl.sh[0][0] * ibl.intensity).max(0.0),
        (C4 * ibl.sh[0][1] * ibl.intensity).max(0.0),
        (C4 * ibl.sh[0][2] * ibl.intensity).max(0.0),
    ]
}

#[inline]
fn luminancia(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Exposición **medida** de la escena: `π / irradiancia_media`, para que una
/// superficie difusa blanca bajo el entorno salga a 1.0. Con eso, cambiar las
/// luces no invalida en silencio ningún número sintonizado a ojo en el shader.
pub fn exposicion_medida(view: &SceneView<'_>) -> f32 {
    if let Some(e) = view.exposure {
        return e;
    }
    let mut e_media = 0.0f32;
    if let Some(ibl) = view.environment {
        e_media += luminancia(irradiancia_media(ibl));
    }
    for l in view.lights {
        e_media += match *l {
            Light::Directional {
                color, intensity, ..
            } => luminancia(color) * intensity,
            Light::Point {
                color, intensity, ..
            } => luminancia(color) * intensity,
        };
    }
    if e_media > 1e-6 {
        std::f32::consts::PI / e_media
    } else {
        // Escena sin iluminación: cualquier exposición da negro. 1.0 evita
        // infinitos y deja el resto del pipeline verificable.
        1.0
    }
}

/// Los 9 coeficientes RGB de `Ibl::sh`, empaquetados como `[f32; 4]` (canal
/// sin usar en 0.0) para que cada entrada quede en un múltiplo de 16 bytes:
/// es exactamente el layout que WGSL exige para `array<vec4<f32>, 9>` dentro
/// de un uniforme, así que no hace falta relleno manual en el shader.
pub fn sh_para_gpu(ibl: &Ibl) -> [[f32; 4]; 9] {
    let mut out = [[0.0f32; 4]; 9];
    for i in 0..9 {
        out[i] = [ibl.sh[i][0], ibl.sh[i][1], ibl.sh[i][2], 0.0];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_math::DVec3;

    fn ibl_constante(l: f32) -> Ibl {
        // Y00 = 0.282095: L00 = l / Y00 reproduce una radiancia constante l en
        // toda dirección (ver forge-render-cpu::sombreado::sh_constante).
        const Y00: f32 = 0.282_095;
        let mut sh = [[0.0f32; 3]; 9];
        sh[0] = [l / Y00, l / Y00, l / Y00];
        Ibl {
            sh,
            prefiltered: None,
            intensity: 1.0,
            rotation_rad: 0.0,
        }
    }

    /// Un entorno de radiancia constante `l` tiene que dar irradiancia media
    /// `π · l`: es la relación exacta entre radiancia y densidad de flujo para
    /// un hemisferio uniforme, y es la respuesta conocida que hace de este
    /// test algo más que "el número cambió".
    #[test]
    fn irradiancia_media_de_un_entorno_constante_es_pi_por_l() {
        let ibl = ibl_constante(0.5);
        let e = irradiancia_media(&ibl);
        for c in e {
            assert!(
                (c - std::f32::consts::PI * 0.5).abs() < 1e-4,
                "obtenido {c}"
            );
        }
    }

    #[test]
    fn exposicion_explicita_de_la_vista_gana_a_la_medida() {
        let view = SceneView {
            camera: forge_render_api::Camera::default(),
            instances: &[],
            lights: &[],
            environment: None,
            exposure: Some(3.5),
        };
        assert_eq!(exposicion_medida(&view), 3.5);
    }

    #[test]
    fn escena_sin_luz_ni_entorno_da_exposicion_uno_y_no_infinito() {
        let view = SceneView {
            camera: forge_render_api::Camera::default(),
            instances: &[],
            lights: &[],
            environment: None,
            exposure: None,
        };
        assert_eq!(exposicion_medida(&view), 1.0);
    }

    #[test]
    fn una_luz_direccional_sube_la_irradiancia_media_medida() {
        let luces = [Light::Directional {
            direction: DVec3::NEG_Z,
            color: [1.0, 1.0, 1.0],
            intensity: 2.0,
        }];
        let view = SceneView {
            camera: forge_render_api::Camera::default(),
            instances: &[],
            lights: &luces,
            environment: None,
            exposure: None,
        };
        // e_media = luminancia([1,1,1]) * 2.0 = 2.0; exposicion = pi / 2.0
        let e = exposicion_medida(&view);
        assert!((e - std::f32::consts::PI / 2.0).abs() < 1e-6);
    }

    #[test]
    fn el_empaquetado_para_gpu_conserva_rgb_y_pone_w_a_cero() {
        let ibl = ibl_constante(1.0);
        let packed = sh_para_gpu(&ibl);
        assert_eq!(packed[0][3], 0.0);
        assert_eq!(packed[0][0], ibl.sh[0][0]);
    }
}
