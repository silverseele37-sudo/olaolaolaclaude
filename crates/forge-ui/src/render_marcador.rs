//! Renderizador provisional, sin geometría real.
//!
//! # Por qué existe esto y no un tercer rasterizador
//!
//! El plan original era que `forge-ui` usara `forge-render-cpu` por defecto:
//! ya implementa el mismo trait [`forge_render_api::Renderer`], ya está
//! verificado sin GPU, y es exactamente el «arranca aunque no haya GPU» que
//! pide la tarea. Pero `tests/arquitectura.rs` —que esta tarea pide no
//! tocar— no incluye `forge-render-cpu` en las dependencias permitidas de
//! `forge-ui` ni de `forge-app` (solo en las de `forge-render`, que tampoco
//! existe todavía). Añadir esa arista habría hecho fallar
//! `ningun_crate_cruza_una_frontera_no_declarada`.
//!
//! Con las dos restricciones firmes —no tocar el guardia, no depender de un
//! crate que no está en su tabla— la única manera de que el visor y el modo
//! `--png` funcionen hoy es que `forge-ui` traiga su **propia** implementación
//! mínima del trait. Esta es esa implementación: deliberadamente pequeña,
//! deliberadamente honesta sobre lo que hace.
//!
//! # Lo que dibuja
//!
//! No rasteriza triángulos: no resuelve el hash de una malla a posiciones e
//! índices (ver el módulo [`crate::escena`] sobre por qué). En su lugar
//! proyecta el **centro de la caja de mundo** de cada instancia visible y
//! pinta un disco plano, coloreado según si está seleccionada. Es lo mínimo
//! que permite verificar la extracción de escena y la cámara de punta a
//! punta con una imagen real, y lo mínimo que necesita `forge --png` para
//! producir algo. `RenderStats::triangles` se reporta en `0` a propósito: no
//! dibuja ninguno, y afirmar lo contrario sería mentir en la propia métrica
//! que existe para detectar justo este tipo de sustituto.
//!
//! Cuando `forge-render` (wgpu) exista, `forge-app` debería preferirlo; este
//! renderizador queda como retén cuando no hay GPU o como implementación por
//! defecto de `forge-ui` mientras tanto.

use std::time::Instant;

use forge_render_api::{RenderStats, RenderTarget, Renderer, SceneView};

use crate::proyeccion::{proyectar, vista_proyeccion};

/// Color de fondo y de instancia en RGBA8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletaMarcador {
    pub fondo: [u8; 4],
    pub instancia: [u8; 4],
    pub seleccionada: [u8; 4],
}

impl Default for PaletaMarcador {
    fn default() -> Self {
        PaletaMarcador {
            fondo: [24, 26, 30, 255],
            instancia: [150, 160, 175, 255],
            seleccionada: [255, 149, 0, 255],
        }
    }
}

/// Radio del disco marcador, en píxeles. Fijo: sin geometría real no hay un
/// tamaño aparente que calcular.
const RADIO_MARCADOR_PX: f32 = 6.0;

#[derive(Clone, Debug, Default)]
pub struct RenderizadorMarcador {
    pub paleta: PaletaMarcador,
}

impl RenderizadorMarcador {
    pub fn nuevo(paleta: PaletaMarcador) -> Self {
        RenderizadorMarcador { paleta }
    }

    fn pintar(&self, buf: &mut [u8], ancho: u32, alto: u32, px: f32, py: f32, color: [u8; 4]) {
        let r = RADIO_MARCADOR_PX;
        let x0 = (px - r).floor().max(0.0) as i64;
        let x1 = (px + r).ceil().min(ancho as f32) as i64;
        let y0 = (py - r).floor().max(0.0) as i64;
        let y1 = (py + r).ceil().min(alto as f32) as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - px;
                let dy = y as f32 + 0.5 - py;
                if dx * dx + dy * dy <= r * r {
                    let i = (y as usize * ancho as usize + x as usize) * 4;
                    buf[i..i + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

impl Renderer for RenderizadorMarcador {
    fn name(&self) -> &'static str {
        "forge-ui::marcador (provisional, sin geometría real — ver docs del módulo)"
    }

    fn render(&mut self, view: &SceneView<'_>, target: RenderTarget) -> RenderStats {
        // Sin superficie de ventana propia: se reutiliza el camino sin
        // ventana y se descartan los píxeles. Las estadísticas son idénticas
        // en ambos casos porque es exactamente el mismo cálculo.
        let _ = self.render_offscreen(view, target);
        self.estadisticas(view)
    }

    fn render_offscreen(&mut self, view: &SceneView<'_>, target: RenderTarget) -> Vec<u8> {
        let ancho = target.width.max(1);
        let alto = target.height.max(1);
        let mut buf = vec![0u8; ancho as usize * alto as usize * 4];
        for pixel in buf.chunks_exact_mut(4) {
            pixel.copy_from_slice(&self.paleta.fondo);
        }

        let vp = vista_proyeccion(&view.camera, ancho, alto);
        for inst in view.instances.iter().filter(|i| i.visible) {
            if inst.bounds.is_empty() {
                continue; // sin caja conocida: no hay dónde dibujar el marcador
            }
            let centro = inst.bounds.center();
            if let Some((px, _prof)) = proyectar(&vp, centro, ancho, alto) {
                let color = if inst.selected {
                    self.paleta.seleccionada
                } else {
                    self.paleta.instancia
                };
                self.pintar(&mut buf, ancho, alto, px[0], px[1], color);
            }
        }

        buf
    }
}

impl RenderizadorMarcador {
    fn estadisticas(&self, view: &SceneView<'_>) -> RenderStats {
        let inicio = Instant::now();
        let enviadas = view.instances.len() as u32;
        let visibles = view.instances.iter().filter(|i| i.visible).count() as u32;
        RenderStats {
            instances_submitted: enviadas,
            instances_culled: enviadas - visibles,
            triangles: 0, // honesto: no se rasteriza ningún triángulo real
            draw_calls: visibles,
            gpu_uploads: 0,
            cpu_time_us: inicio.elapsed().as_micros() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::EntityId;
    use forge_math::{Aabb, DAffine3, DVec3};
    use forge_render_api::{Camera, DrawInstance, MaterialId};
    use forge_store::BlobHash;
    use std::f64::consts::FRAC_PI_2;

    fn instancia(centro: DVec3, seleccionada: bool) -> DrawInstance {
        DrawInstance {
            entity: EntityId::from_u128(1),
            mesh: BlobHash::of(b"x"),
            material: MaterialId::DEFAULT,
            transform: DAffine3::IDENTITY,
            bounds: Aabb::new(centro, centro),
            visible: true,
            selected: seleccionada,
        }
    }

    fn pixel(buf: &[u8], ancho: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * ancho as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn escena_vacia_es_todo_fondo() {
        let mut r = RenderizadorMarcador::default();
        let view = SceneView {
            camera: Camera::default(),
            instances: &[],
            lights: &[],
            environment: None,
            exposure: None,
        };
        let target = RenderTarget {
            width: 32,
            height: 32,
            samples: 1,
        };
        let buf = r.render_offscreen(&view, target);
        assert_eq!(buf.len(), 32 * 32 * 4);
        for i in (0..buf.len()).step_by(4) {
            assert_eq!(&buf[i..i + 4], &r.paleta.fondo);
        }
    }

    #[test]
    fn una_instancia_en_el_objetivo_pinta_el_centro_del_lienzo() {
        let mut r = RenderizadorMarcador::default();
        let camara = Camera {
            eye: DVec3::new(10.0, 0.0, 0.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: FRAC_PI_2,
            near_mm: 0.1,
            far_mm: 1000.0,
        };
        let inst = instancia(DVec3::ZERO, false);
        let instancias = [inst];
        let view = SceneView {
            camera: camara,
            instances: &instancias,
            lights: &[],
            environment: None,
            exposure: None,
        };
        let target = RenderTarget {
            width: 64,
            height: 64,
            samples: 1,
        };
        let buf = r.render_offscreen(&view, target);

        // centro del lienzo: color de instancia
        assert_eq!(pixel(&buf, 64, 32, 32), r.paleta.instancia);
        // esquina: sigue siendo fondo
        assert_eq!(pixel(&buf, 64, 0, 0), r.paleta.fondo);
    }

    #[test]
    fn una_instancia_seleccionada_usa_el_color_de_seleccion() {
        let mut r = RenderizadorMarcador::default();
        let camara = Camera {
            eye: DVec3::new(10.0, 0.0, 0.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: FRAC_PI_2,
            near_mm: 0.1,
            far_mm: 1000.0,
        };
        let instancias = [instancia(DVec3::ZERO, true)];
        let view = SceneView {
            camera: camara,
            instances: &instancias,
            lights: &[],
            environment: None,
            exposure: None,
        };
        let target = RenderTarget {
            width: 64,
            height: 64,
            samples: 1,
        };
        let buf = r.render_offscreen(&view, target);
        assert_eq!(pixel(&buf, 64, 32, 32), r.paleta.seleccionada);
    }

    #[test]
    fn una_instancia_invisible_no_se_pinta() {
        let mut r = RenderizadorMarcador::default();
        let camara = Camera {
            eye: DVec3::new(10.0, 0.0, 0.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: FRAC_PI_2,
            near_mm: 0.1,
            far_mm: 1000.0,
        };
        let mut inst = instancia(DVec3::ZERO, false);
        inst.visible = false;
        let instancias = [inst];
        let view = SceneView {
            camera: camara,
            instances: &instancias,
            lights: &[],
            environment: None,
            exposure: None,
        };
        let target = RenderTarget {
            width: 64,
            height: 64,
            samples: 1,
        };
        let buf = r.render_offscreen(&view, target);
        assert_eq!(pixel(&buf, 64, 32, 32), r.paleta.fondo);
    }

    #[test]
    fn las_estadisticas_reportan_cero_triangulos_con_honestidad() {
        let mut r = RenderizadorMarcador::default();
        let instancias = [instancia(DVec3::ZERO, false)];
        let view = SceneView {
            camera: Camera::default(),
            instances: &instancias,
            lights: &[],
            environment: None,
            exposure: None,
        };
        let target = RenderTarget {
            width: 16,
            height: 16,
            samples: 1,
        };
        let stats = r.render(&view, target);
        assert_eq!(stats.triangles, 0);
        assert_eq!(stats.instances_submitted, 1);
    }
}
