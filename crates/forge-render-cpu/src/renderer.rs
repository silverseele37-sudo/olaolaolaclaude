//! Cablea `agx`, `camara`, `raster`, `malla` y `sombreado` detrás del trait
//! [`Renderer`] de `forge-render-api`.
//!
//! El pipeline, en orden:
//!
//! 1. Fondo: se pinta cada píxel con la radiancia del entorno a lo largo del
//!    rayo de cámara, **antes** de dibujar ninguna geometría (así lo que no
//!    cubre ningún triángulo se queda con esa radiancia).
//! 2. Por instancia: culling de frustum contra `bounds` (ya en mundo — es
//!    la razón de que `DrawInstance` la lleve aparte de `transform`, para
//!    poder descartar sin resolver la malla) y resolución de malla vía
//!    [`MeshProvider`].
//! 3. Por triángulo: paso a mundo, recorte contra el plano cercano, paso a
//!    píxeles y rasterizado con z-buffer.
//! 4. Por fragmento: sombreado en radiancia lineal (`f32`, ver
//!    [`crate::sombreado`]).
//! 5. Al final del frame: exposición medida, resolución del sobremuestreo y
//!    AgX + OETF sRGB para producir los bytes finales.

use crate::agx;
use crate::camara::Camara;
use crate::malla::MeshProvider;
use crate::malla::TablaDeMateriales;
use crate::raster::{self, Cobertura, Lienzo, VerticeClip};
use crate::sombreado::{exposicion_medida, sombrear};
use forge_math::DVec3;
use forge_render_api::{RenderStats, RenderTarget, Renderer, SceneView};

/// Renderiza con `forge-render-cpu`. Genérico sobre [`MeshProvider`]: ver la
/// nota de `crate::malla` sobre por qué la resolución de malla es un trait y
/// no un mapa fijo.
pub struct SoftwareRenderer<M: MeshProvider> {
    pub mallas: M,
    pub materiales: TablaDeMateriales,
    /// Modo de depuración: cualquier píxel cuya cobertura resuelta sea cara
    /// trasera se sustituye por magenta puro `(255, 0, 255, 255)`, **después**
    /// de resolver el sobremuestreo y **sin pasar por AgX**.
    ///
    /// No es un hack de test escondido en producción: es la utilidad de
    /// visualización de orientación que cualquier motor de verificación
    /// necesita, y que hace posible medir la fracción de píxeles con caras
    /// traseras visibles sin adivinar a partir del color sombreado (que AgX
    /// desaturaría y haría imposible de contar con exactitud de byte).
    pub modo_orientacion: bool,
    ultimas_stats: RenderStats,
}

/// Resultado interno de un frame: color lineal (post sobremuestreo y
/// exposición, pre AgX) y cobertura, ambos ya a resolución de destino.
struct Resultado {
    ancho: u32,
    alto: u32,
    color: Vec<[f32; 3]>,
    cobertura: Vec<Cobertura>,
}

impl<M: MeshProvider> SoftwareRenderer<M> {
    pub fn nueva(mallas: M, materiales: TablaDeMateriales) -> Self {
        SoftwareRenderer {
            mallas,
            materiales,
            modo_orientacion: false,
            ultimas_stats: RenderStats::default(),
        }
    }

    /// Últimas estadísticas calculadas, sin volver a renderizar.
    pub fn stats(&self) -> RenderStats {
        self.ultimas_stats
    }

    fn ejecutar(&mut self, view: &SceneView<'_>, target: RenderTarget) -> (Resultado, RenderStats) {
        let inicio = std::time::Instant::now();

        // `samples` como factor de sobremuestreo (SSAA: se sombrea entero
        // cada sub-píxel, no solo la cobertura como haría MSAA real). Es más
        // caro que MSAA pero determinista con el mismo pipeline de sombreado
        // que el resto del frame, y acotado a 4 para que un valor disparatado
        // no agote memoria en vez de fallar limpio.
        let factor = target.samples.clamp(1, 4);
        let ancho_ss = target.width.saturating_mul(factor);
        let alto_ss = target.height.saturating_mul(factor);
        let mut lienzo = Lienzo::nuevo(ancho_ss, alto_ss, factor);

        let aspecto = target.width as f64 / (target.height as f64);
        let camara = Camara::nueva(&view.camera, aspecto);
        let planos = camara.planos();
        let exposicion = exposicion_medida(view);

        let mut stats = RenderStats::default();

        // Paso 1: fondo. Se saca `color` de `lienzo` porque el cierre que
        // rasteriza más abajo necesita escribir en él mientras `rasterizar`
        // tiene prestado `&mut lienzo` para profundidad y cobertura; son dos
        // variables distintas y el prestamista queda contento.
        let mut color = std::mem::take(&mut lienzo.color);
        for y in 0..lienzo.alto {
            for x in 0..lienzo.ancho {
                let i = lienzo.idx(x, y);
                color[i] = match view.environment {
                    Some(ibl) => {
                        let dir = raster::rayo(&camara, lienzo.ancho, lienzo.alto, x, y);
                        crate::sombreado::radiancia_sh(ibl, dir)
                    }
                    None => [0.0, 0.0, 0.0],
                };
            }
        }

        // Paso 2 y 3: instancias y triángulos.
        for instancia in view.instances {
            if !instancia.visible {
                // Oculta explícitamente: ni se intenta ni se cuenta como
                // descartada, porque no es un fallo de render sino una
                // decisión de la escena.
                continue;
            }
            if crate::camara::fuera_del_frustum(&planos, &instancia.bounds.corners()) {
                stats.instances_culled += 1;
                continue;
            }
            let Some(malla) = self.mallas.malla(instancia.mesh) else {
                // Hash sin resolver: se cuenta igual que un descarte por
                // frustum (no hay un contador dedicado en `RenderStats`, y la
                // semántica es la misma — "esta instancia no se dibujó").
                stats.instances_culled += 1;
                continue;
            };
            if !malla.es_valida() {
                stats.instances_culled += 1;
                continue;
            }

            stats.instances_submitted += 1;
            stats.draw_calls += 1;
            stats.triangles += (malla.indices.len() / 3) as u64;

            let material = self.materiales.material(instancia.material);

            for tri in malla.indices.chunks_exact(3) {
                let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                let locales = [
                    malla.positions[i0],
                    malla.positions[i1],
                    malla.positions[i2],
                ];
                let mundo = locales.map(|p| instancia.transform.transform_point3(p));

                let normales = if malla.normals.is_empty() {
                    // Normal geométrica del triángulo ya transformado: evita
                    // el problema de la inversa-transpuesta bajo escala no
                    // uniforme, porque el producto vectorial de dos aristas en
                    // mundo da la normal correcta sin importar cómo se llegó
                    // ahí.
                    let n = (mundo[1] - mundo[0])
                        .cross(mundo[2] - mundo[0])
                        .normalize_or_zero();
                    [n, n, n]
                } else {
                    [
                        instancia
                            .transform
                            .transform_vector3(malla.normals[i0])
                            .normalize_or_zero(),
                        instancia
                            .transform
                            .transform_vector3(malla.normals[i1])
                            .normalize_or_zero(),
                        instancia
                            .transform
                            .transform_vector3(malla.normals[i2])
                            .normalize_or_zero(),
                    ]
                };

                let clip: [VerticeClip; 3] = std::array::from_fn(|k| {
                    let (xyz, w) =
                        crate::camara::proyectar_homogeneo(&camara.vista_proyeccion, mundo[k]);
                    VerticeClip {
                        clip: xyz,
                        clip_w: w,
                        rel: mundo[k] - camara.ojo,
                        normal: normales[k],
                    }
                });

                let poligono = raster::recortar_cercano(clip);
                if poligono.len() < 3 {
                    continue;
                }
                for k in 1..poligono.len() - 1 {
                    let sub = [poligono[0], poligono[k], poligono[k + 1]];
                    let (Some(p0), Some(p1), Some(p2)) = (
                        raster::a_pixel(&sub[0], lienzo.ancho, lienzo.alto),
                        raster::a_pixel(&sub[1], lienzo.ancho, lienzo.alto),
                        raster::a_pixel(&sub[2], lienzo.ancho, lienzo.alto),
                    ) else {
                        continue;
                    };
                    // Nunca se descartan caras traseras: una cámara dentro de
                    // un sólido cerrado depende de verlas (es lo que verifica
                    // el test de orientación con control positivo).
                    raster::rasterizar(&mut lienzo, [p0, p1, p2], true, |i, frag| {
                        let rel =
                            DVec3::new(frag.rel[0] as f64, frag.rel[1] as f64, frag.rel[2] as f64);
                        let p = camara.ojo + rel;
                        let n = DVec3::new(
                            frag.normal[0] as f64,
                            frag.normal[1] as f64,
                            frag.normal[2] as f64,
                        )
                        .normalize_or_zero();
                        let v = (-rel).normalize_or_zero();
                        color[i] = sombrear(p, n, v, &material, view.lights, view.environment);
                    });
                }
            }
        }
        lienzo.color = color;

        stats.gpu_uploads = 0;
        stats.cpu_time_us = inicio.elapsed().as_micros() as u64;

        // Paso 5: sobremuestreo, exposición. AgX queda para el llamante
        // (`render_offscreen`), porque `render` no necesita nunca los bytes.
        let color_resuelto = lienzo.resolver_color();
        let cobertura = lienzo.resolver_cobertura();
        let (ancho, alto) = lienzo.destino();
        let color = color_resuelto
            .into_iter()
            .map(|c| [c[0] * exposicion, c[1] * exposicion, c[2] * exposicion])
            .collect();

        self.ultimas_stats = stats;
        (
            Resultado {
                ancho,
                alto,
                color,
                cobertura,
            },
            stats,
        )
    }
}

impl<M: MeshProvider> Renderer for SoftwareRenderer<M> {
    fn name(&self) -> &'static str {
        "forge-render-cpu"
    }

    /// Sin superficie de ventana en este crate: ejecuta el mismo pipeline que
    /// `render_offscreen` y descarta los píxeles, devolviendo solo las
    /// estadísticas. Un backend con ventana real (wgpu) presentaría el
    /// framebuffer en este método; aquí no hay a qué presentarlo.
    fn render(&mut self, view: &SceneView<'_>, target: RenderTarget) -> RenderStats {
        let (_, stats) = self.ejecutar(view, target);
        stats
    }

    fn render_offscreen(&mut self, view: &SceneView<'_>, target: RenderTarget) -> Vec<u8> {
        let (res, _) = self.ejecutar(view, target);
        let n = res.ancho as usize * res.alto as usize;
        let mut salida = vec![0u8; n * 4];
        for i in 0..n {
            let rgba = if self.modo_orientacion && res.cobertura[i] == Cobertura::Trasera {
                [255u8, 0, 255, 255]
            } else {
                let lineal = agx::agx(res.color[i]);
                [
                    agx::a_byte(lineal[0]),
                    agx::a_byte(lineal[1]),
                    agx::a_byte(lineal[2]),
                    255,
                ]
            };
            salida[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }
        salida
    }
}
