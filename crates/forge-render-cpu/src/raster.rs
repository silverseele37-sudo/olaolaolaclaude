//! El rasterizador: z-buffer, baricéntricas y recorte contra el plano cercano.

use crate::camara::{facing, Camara, Facing};
use forge_math::{DVec3, DVec4};

/// Qué hay en un píxel. Lo necesita el test de orientación para contar caras
/// traseras sin tener que adivinar colores después del mapeo de tono.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cobertura {
    Fondo,
    Frontal,
    Trasera,
}

/// Buffer de trabajo, en radiancia lineal y a resolución **interna** (es decir,
/// ya multiplicada por el sobremuestreo).
#[derive(Clone, Debug)]
pub struct Lienzo {
    pub ancho: u32,
    pub alto: u32,
    /// Factor de sobremuestreo respecto del destino pedido.
    pub factor: u32,
    pub color: Vec<[f32; 3]>,
    pub profundidad: Vec<f32>,
    pub cobertura: Vec<Cobertura>,
}

impl Lienzo {
    pub fn nuevo(ancho: u32, alto: u32, factor: u32) -> Lienzo {
        let n = (ancho as usize) * (alto as usize);
        Lienzo {
            ancho,
            alto,
            factor,
            color: vec![[0.0; 3]; n],
            profundidad: vec![f32::INFINITY; n],
            cobertura: vec![Cobertura::Fondo; n],
        }
    }

    #[inline]
    pub fn idx(&self, x: u32, y: u32) -> usize {
        (y as usize) * (self.ancho as usize) + (x as usize)
    }

    /// Resolución del destino, una vez deshecho el sobremuestreo.
    pub fn destino(&self) -> (u32, u32) {
        (self.ancho / self.factor, self.alto / self.factor)
    }

    /// Promedia el sobremuestreo **en lineal**, que es donde promediar tiene
    /// sentido físico. Promediar después del mapeo de tono da bordes que se ven
    /// más claros de lo que deberían.
    pub fn resolver_color(&self) -> Vec<[f32; 3]> {
        let (w, h) = self.destino();
        let f = self.factor as usize;
        let inv = 1.0 / (f * f) as f32;
        let mut out = vec![[0.0f32; 3]; (w as usize) * (h as usize)];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let mut acc = [0.0f32; 3];
                for sy in 0..f {
                    for sx in 0..f {
                        let c = self.color[(y * f + sy) * self.ancho as usize + (x * f + sx)];
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                    }
                }
                out[y * w as usize + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv];
            }
        }
        out
    }

    /// Cobertura resuelta por mayoría, con `Trasera` ganando los empates: en el
    /// test de orientación una cara trasera visible es lo que hay que ver, y
    /// esconderla en el promedio anularía el control positivo.
    pub fn resolver_cobertura(&self) -> Vec<Cobertura> {
        let (w, h) = self.destino();
        let f = self.factor as usize;
        let mut out = vec![Cobertura::Fondo; (w as usize) * (h as usize)];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let (mut fr, mut tr) = (0u32, 0u32);
                for sy in 0..f {
                    for sx in 0..f {
                        match self.cobertura[(y * f + sy) * self.ancho as usize + (x * f + sx)] {
                            Cobertura::Frontal => fr += 1,
                            Cobertura::Trasera => tr += 1,
                            Cobertura::Fondo => {}
                        }
                    }
                }
                out[y * w as usize + x] = if tr > 0 && tr >= fr {
                    Cobertura::Trasera
                } else if fr > 0 {
                    Cobertura::Frontal
                } else {
                    Cobertura::Fondo
                };
            }
        }
        out
    }
}

/// Vértice tras la transformación, antes del recorte.
#[derive(Clone, Copy, Debug)]
pub struct VerticeClip {
    pub clip: DVec4,
    /// Posición **relativa al ojo**. Es el motivo por el que este rasterizador
    /// puede usar `f32` sin perder precisión en una escena de 100 m: lo que se
    /// interpola es la diferencia, no la coordenada absoluta.
    pub rel: DVec3,
    pub normal: DVec3,
}

/// Recorte contra el plano cercano en espacio de clip (`z >= 0`).
///
/// Sin esto, una cámara **dentro** de un objeto no dibuja nada: los triángulos
/// que cruzan el plano cercano se descartarían enteros y el control positivo del
/// test de orientación —cámara dentro del cubo, 100 % caras traseras— sería
/// imposible de construir.
pub fn recortar_cercano(tri: [VerticeClip; 3]) -> Vec<VerticeClip> {
    const EPS: f64 = 1e-9;
    let dentro = |v: &VerticeClip| v.clip.z >= EPS;
    if tri.iter().all(dentro) {
        return tri.to_vec();
    }
    if tri.iter().all(|v| v.clip.z < EPS) {
        return Vec::new();
    }
    let mut salida: Vec<VerticeClip> = Vec::with_capacity(4);
    for i in 0..3 {
        let a = tri[i];
        let b = tri[(i + 1) % 3];
        let (da, db) = (a.clip.z - EPS, b.clip.z - EPS);
        if da >= 0.0 {
            salida.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            salida.push(VerticeClip {
                clip: a.clip + (b.clip - a.clip) * t,
                rel: a.rel + (b.rel - a.rel) * t,
                normal: a.normal + (b.normal - a.normal) * t,
            });
        }
    }
    salida
}

/// Vértice ya en píxeles, con los atributos divididos por `w`.
#[derive(Clone, Copy, Debug)]
pub struct VerticePixel {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub inv_w: f32,
    pub rel_sobre_w: [f32; 3],
    pub n_sobre_w: [f32; 3],
}

/// División de perspectiva y paso a píxeles. Ver la nota de handedness en
/// [`crate::camara`]: la Y se invierte **aquí y solo aquí**.
pub fn a_pixel(v: &VerticeClip, ancho: u32, alto: u32) -> Option<VerticePixel> {
    let w = v.clip.w;
    if !(w.is_finite()) || w <= 1e-12 {
        return None;
    }
    let inv_w = 1.0 / w;
    let ndc = DVec3::new(v.clip.x * inv_w, v.clip.y * inv_w, v.clip.z * inv_w);
    let iw = inv_w as f32;
    Some(VerticePixel {
        x: ((ndc.x * 0.5 + 0.5) * ancho as f64) as f32,
        y: ((0.5 - ndc.y * 0.5) * alto as f64) as f32,
        z: ndc.z as f32,
        inv_w: iw,
        rel_sobre_w: [
            (v.rel.x as f32) * iw,
            (v.rel.y as f32) * iw,
            (v.rel.z as f32) * iw,
        ],
        n_sobre_w: [
            (v.normal.x as f32) * iw,
            (v.normal.y as f32) * iw,
            (v.normal.z as f32) * iw,
        ],
    })
}

#[inline]
fn orient(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (bx - ax) * (cy - ay) - (cx - ax) * (by - ay)
}

/// Atributos interpolados en un fragmento.
#[derive(Clone, Copy, Debug)]
pub struct Fragmento {
    pub z: f32,
    pub rel: [f32; 3],
    pub normal: [f32; 3],
    pub cara: Facing,
}

/// Rasteriza un triángulo llamando a `emitir` por cada fragmento que pasa el
/// test de profundidad.
///
/// El test es estricto (`<`): en un empate exacto gana el primero dibujado. Es
/// lo que hace que invertir el orden de dos quads a distinta profundidad dé una
/// imagen idéntica byte a byte, y que la palabra «determinista» signifique algo.
pub fn rasterizar<F: FnMut(usize, Fragmento)>(
    lienzo: &mut Lienzo,
    t: [VerticePixel; 3],
    dibujar_traseras: bool,
    mut emitir: F,
) {
    let area = orient(t[0].x, t[0].y, t[1].x, t[1].y, t[2].x, t[2].y);
    let cara = facing(area);
    match cara {
        Facing::Nula => return,
        Facing::Trasera if !dibujar_traseras => return,
        _ => {}
    }

    let (w, h) = (lienzo.ancho as i64, lienzo.alto as i64);
    let min_x = t.iter().fold(f32::INFINITY, |a, v| a.min(v.x)).floor() as i64;
    let max_x = t.iter().fold(f32::NEG_INFINITY, |a, v| a.max(v.x)).ceil() as i64;
    let min_y = t.iter().fold(f32::INFINITY, |a, v| a.min(v.y)).floor() as i64;
    let max_y = t.iter().fold(f32::NEG_INFINITY, |a, v| a.max(v.y)).ceil() as i64;
    let (x0, x1) = (min_x.max(0), max_x.min(w - 1));
    let (y0, y1) = (min_y.max(0), max_y.min(h - 1));
    if x0 > x1 || y0 > y1 {
        return;
    }

    let inv_area = 1.0 / area;
    for py in y0..=y1 {
        for px in x0..=x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let e0 = orient(t[1].x, t[1].y, t[2].x, t[2].y, cx, cy);
            let e1 = orient(t[2].x, t[2].y, t[0].x, t[0].y, cx, cy);
            let e2 = orient(t[0].x, t[0].y, t[1].x, t[1].y, cx, cy);
            // Mismo signo que el área: sirve para las dos orientaciones sin
            // duplicar el bucle.
            let dentro = if area > 0.0 {
                e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0
            } else {
                e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0
            };
            if !dentro {
                continue;
            }
            let (b0, b1, b2) = (e0 * inv_area, e1 * inv_area, e2 * inv_area);
            let z = b0 * t[0].z + b1 * t[1].z + b2 * t[2].z;
            if !(z.is_finite()) || !(0.0..=1.0).contains(&z) {
                continue;
            }
            let i = lienzo.idx(px as u32, py as u32);
            if z >= lienzo.profundidad[i] {
                continue;
            }
            let inv_w = b0 * t[0].inv_w + b1 * t[1].inv_w + b2 * t[2].inv_w;
            if inv_w.abs() < 1e-20 {
                continue;
            }
            let w_frag = 1.0 / inv_w;
            let mut rel = [0.0f32; 3];
            let mut nrm = [0.0f32; 3];
            for k in 0..3 {
                rel[k] = (b0 * t[0].rel_sobre_w[k]
                    + b1 * t[1].rel_sobre_w[k]
                    + b2 * t[2].rel_sobre_w[k])
                    * w_frag;
                nrm[k] = (b0 * t[0].n_sobre_w[k] + b1 * t[1].n_sobre_w[k] + b2 * t[2].n_sobre_w[k])
                    * w_frag;
            }
            lienzo.profundidad[i] = z;
            lienzo.cobertura[i] =
                if cara == Facing::Trasera { Cobertura::Trasera } else { Cobertura::Frontal };
            emitir(i, Fragmento { z, rel, normal: nrm, cara });
        }
    }
}

/// Dirección del rayo de cámara que pasa por el centro del píxel `(x, y)`.
/// Se usa para pintar el fondo con la radiancia del entorno.
pub fn rayo(cam: &Camara, ancho: u32, alto: u32, x: u32, y: u32) -> DVec3 {
    let ndc_x = ((x as f64 + 0.5) / ancho as f64) * 2.0 - 1.0;
    let ndc_y = 1.0 - ((y as f64 + 0.5) / alto as f64) * 2.0;
    let inv = cam.vista_proyeccion.inverse();
    let cerca = inv * DVec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let lejos = inv * DVec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if cerca.w.abs() < 1e-12 || lejos.w.abs() < 1e-12 {
        return DVec3::NEG_Y;
    }
    let a = DVec3::new(cerca.x, cerca.y, cerca.z) / cerca.w;
    let b = DVec3::new(lejos.x, lejos.y, lejos.z) / lejos.w;
    (b - a).normalize_or_zero()
}
