//! Cámara: matrices en `f32` **relativas a la cámara**, y culling de frustum.
//!
//! # Por qué `f32` relativo y no `f32` absoluto
//!
//! El documento vive en `f64` y milímetros; una pieza a 50 m del origen tiene
//! coordenadas del orden de `5×10⁴`. El épsilon de `f32` en ese rango ya es del
//! orden de `0.004`, así que restar dos vértices cercanos para obtener una
//! normal o una arista pierde precisión visible. La técnica estándar —y la que
//! pide la disciplina del proyecto— es hacer la resta cara (`posición - ojo`)
//! en `f64`, **antes** de bajar a `f32`: lo que llega a la GPU nunca es mayor
//! que el tamaño de la escena alrededor de la cámara, sin importar a qué
//! distancia del origen esté esa escena.
//!
//! Concretamente: la vista se descompone en una matriz de vista **sin
//! traslación** (`R`, pura rotación) y la traslación por el ojo se resta a
//! mano de cada transformada de instancia, en `f64`, en
//! [`transformada_relativa`]. La igualdad que lo sostiene:
//!
//! ```text
//! view(p) = R · (p - ojo)
//! clip    = proyeccion · R · (transform(p_local) - ojo)
//!         = proyeccion · R · (T(-ojo) · transform)(p_local)
//! ```
//!
//! así que `proyeccion · R` (calculado una vez por frame, [`Camara::vista_proyeccion_relativa`])
//! y `T(-ojo) · transform` (calculado una vez por instancia) multiplicados dan
//! exactamente el mismo resultado que `proyeccion · vista · transform`, sin que
//! ningún término intermedio en `f32` sea mayor que la escena misma.
//!
//! # Frustum culling
//!
//! El culling, en cambio, se hace en `f64` y en espacio absoluto (igual que
//! `forge-render-cpu::camara`): `DrawInstance::bounds` ya viene en mundo, y
//! comparar contra los planos absolutos no tiene el problema de precisión de
//! arriba porque no hay resta de vértices, solo un producto punto por caja.
//!
//! # El handedness (idéntico a `forge-render-cpu`)
//!
//! Z arriba, diestro; `perspective_rh` con profundidad `[0, 1]`, que es
//! exactamente la convención de NDC de wgpu (a diferencia de OpenGL, que usa
//! `[-1, 1]`). A diferencia del rasterizador por software, aquí **no** hay que
//! invertir Y a mano en ningún sitio: wgpu asigna `NDC.y = +1` a la fila
//! superior de la textura de color en las tres API nativas (Vulkan, DX12,
//! Metal) de la misma forma, así que ese paso lo hace el hardware y no el
//! código de esta cámara.

use forge_math::{DMat3, DMat4, DVec3};
use forge_render_api::{Camera, DrawInstance};

/// Matriz 4×4 en `f32`, columna a columna — el layout exacto que espera
/// `mat4x4<f32>` en WGSL (cuatro columnas de 16 bytes, sin relleno).
pub type Mat4Gpu = [[f32; 4]; 4];

const IDENTIDAD_GPU: Mat4Gpu = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[inline]
fn columnas_f32(m: DMat4) -> Mat4Gpu {
    [
        [
            m.x_axis.x as f32,
            m.x_axis.y as f32,
            m.x_axis.z as f32,
            m.x_axis.w as f32,
        ],
        [
            m.y_axis.x as f32,
            m.y_axis.y as f32,
            m.y_axis.z as f32,
            m.y_axis.w as f32,
        ],
        [
            m.z_axis.x as f32,
            m.z_axis.y as f32,
            m.z_axis.z as f32,
            m.z_axis.w as f32,
        ],
        [
            m.w_axis.x as f32,
            m.w_axis.y as f32,
            m.w_axis.z as f32,
            m.w_axis.w as f32,
        ],
    ]
}

/// Un plano `normal · p + d >= 0` para «dentro». Idéntico en forma al de
/// `forge-render-cpu::camara`, reproducido aquí porque los pilares no se
/// conocen entre sí (ADR-0006) y este es el mismo cálculo, no una dependencia.
#[derive(Clone, Copy, Debug)]
pub struct Plano {
    pub normal: DVec3,
    pub d: f64,
}

impl Plano {
    #[inline]
    pub fn distancia(&self, p: DVec3) -> f64 {
        self.normal.dot(p) + self.d
    }
}

/// Matrices y datos derivados de una [`Camera`], listos para un frame.
#[derive(Clone, Copy, Debug)]
pub struct Camara {
    /// `proyeccion · R`, en `f32`, sin la traslación del ojo. Ver la nota del
    /// módulo: es la matriz que multiplica a cada transformada ya hecha
    /// relativa a la cámara.
    pub vista_proyeccion_relativa: Mat4Gpu,
    /// Inversa de la anterior. La usa el pase de fondo para reconstruir el
    /// rayo de cámara por píxel a partir de NDC, igual que
    /// `forge-render-cpu::raster::rayo` pero en espacio relativo (el ojo
    /// siempre es el origen en ese espacio, así que no hace falta restarlo).
    pub inversa_vista_proyeccion_relativa: Mat4Gpu,
    /// Posición absoluta del ojo, en `f64`. Solo para restarla a las
    /// transformadas de instancia; nunca viaja a la GPU.
    pub ojo: DVec3,
    /// Los 6 planos del frustum, en espacio absoluto de mundo.
    planos: [Plano; 6],
}

impl Camara {
    /// Construye las matrices. **No entra en pánico** con cámaras
    /// degeneradas (`eye == target`, `up` paralelo a la vista): son un estado
    /// transitorio normal mientras el usuario orbita, no un bug que deba
    /// abortar el frame. La guarda es la misma que en
    /// `forge-render-cpu::camara::Camara::nueva`.
    pub fn nueva(c: &Camera, aspecto: f64) -> Camara {
        let mut adelante = c.target - c.eye;
        if adelante.length_squared() < 1e-18 {
            adelante = -DVec3::Y;
        }
        let adelante = adelante.normalize();

        let mut arriba = if c.up.length_squared() < 1e-18 {
            DVec3::Z
        } else {
            c.up.normalize()
        };
        if arriba.cross(adelante).length_squared() < 1e-12 {
            arriba = if adelante.z.abs() > 0.9 {
                DVec3::Y
            } else {
                DVec3::Z
            };
        }

        let near = c.near_mm.max(1e-6);
        let far = c.far_mm.max(near * (1.0 + 1e-6));
        let fov = c.fov_y_rad.clamp(1e-4, std::f64::consts::PI - 1e-4);
        let aspecto = if aspecto.is_finite() && aspecto > 1e-9 {
            aspecto
        } else {
            1.0
        };

        let vista_absoluta = DMat4::look_at_rh(c.eye, c.eye + adelante, arriba);
        // Vista "solo rotación": la misma orientación, ojo en el origen. Con
        // eye=ZERO, look_at_rh no tiene traslación que restar, así que el
        // resultado es exactamente la R de la nota del módulo.
        let rotacion = DMat4::look_at_rh(DVec3::ZERO, adelante, arriba);
        let proyeccion = DMat4::perspective_rh(fov, aspecto, near, far);

        let vp_absoluta = proyeccion * vista_absoluta;
        let vp_relativa = proyeccion * rotacion;
        let inv_vp_relativa = vp_relativa.inverse();

        Camara {
            vista_proyeccion_relativa: columnas_f32(vp_relativa),
            inversa_vista_proyeccion_relativa: if inv_vp_relativa.is_finite() {
                columnas_f32(inv_vp_relativa)
            } else {
                // Proyección no invertible (fov o aspecto degenerados tras el
                // clamping de arriba, que en la práctica no debería ocurrir):
                // el pase de fondo se queda sin rayo válido y no pinta nada,
                // en vez de propagar NaN a la pantalla entera.
                IDENTIDAD_GPU
            },
            ojo: c.eye,
            planos: planos_de(&vp_absoluta),
        }
    }

    /// ¿La instancia queda completamente fuera del frustum? Delegación directa
    /// a [`fuera_del_frustum`] sobre sus planos absolutos.
    pub fn instancia_fuera_del_frustum(&self, instancia: &DrawInstance) -> bool {
        fuera_del_frustum(&self.planos, &instancia.bounds.corners())
    }

    /// La transformada de una instancia, hecha relativa a esta cámara:
    /// `T(-ojo) · transform`. Ver la nota del módulo para la derivación.
    ///
    /// Se calcula por completo en `f64` y solo se pasa a `f32` en el último
    /// paso, que es lo que mantiene acotado el error de redondeo sin importar
    /// dónde esté la pieza en el documento.
    pub fn transformada_relativa(&self, instancia: &DrawInstance) -> Mat4Gpu {
        let t = instancia.transform;
        let traduccion_rel = t.translation - self.ojo;
        let m = t.matrix3;
        columnas_f32(DMat4::from_cols(
            m.x_axis.extend(0.0),
            m.y_axis.extend(0.0),
            m.z_axis.extend(0.0),
            traduccion_rel.extend(1.0),
        ))
    }

    /// Matriz para transformar normales: inversa-transpuesta del bloque 3×3
    /// lineal de la transformada (rotación + escala). No depende del ojo —una
    /// dirección no tiene posición— así que no hace falta la variante
    /// relativa aquí, solo la conversión a `f32`.
    ///
    /// Se empaqueta como `mat4x4<f32>` con la traslación a cero en vez de
    /// como `mat3x3<f32>`: WGSL alinea `mat3x3<f32>` a 16 bytes por columna
    /// igual que un `mat4x4`, así que usar directamente cuatro columnas evita
    /// una conversión de layout que no ahorra nada.
    pub fn matriz_normal(instancia: &DrawInstance) -> Mat4Gpu {
        let m = instancia.transform.matrix3;
        let inv_t = inversa_transpuesta_segura(m);
        columnas_f32(DMat4::from_cols(
            inv_t.x_axis.extend(0.0),
            inv_t.y_axis.extend(0.0),
            inv_t.z_axis.extend(0.0),
            DVec3::ZERO.extend(1.0),
        ))
    }
}

/// Inversa-transpuesta de un bloque 3×3, con fallback a la identidad cuando la
/// matriz es singular (escala cero en algún eje). Sin la guarda, `inverse()`
/// sobre una matriz singular da `NaN`/`Inf` que contaminarían el sombreado
/// entero del objeto en vez de, en el peor caso, dar una normal ligeramente
/// incorrecta en un objeto ya degenerado.
#[inline]
fn inversa_transpuesta_segura(m: DMat3) -> DMat3 {
    if m.determinant().abs() < 1e-18 {
        DMat3::IDENTITY
    } else {
        m.inverse().transpose()
    }
}

/// Extracción de Gribb–Hartmann de los 6 planos del frustum, sobre la matriz
/// combinada **absoluta**. Idéntica a `forge-render-cpu::camara::Camara::planos`:
/// la fila del plano cercano es `r2` sin sumar `r3` porque la profundidad aquí
/// es `[0, 1]`, no `[-1, 1]`.
fn planos_de(vista_proyeccion: &DMat4) -> [Plano; 6] {
    let m = *vista_proyeccion;
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    let filas = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
    let mut out = [Plano {
        normal: DVec3::ZERO,
        d: 0.0,
    }; 6];
    for (i, q) in filas.into_iter().enumerate() {
        let normal = DVec3::new(q.x, q.y, q.z);
        let n = normal.length();
        out[i] = if n > 1e-12 {
            Plano {
                normal: normal / n,
                d: q.w / n,
            }
        } else {
            Plano { normal, d: q.w }
        };
    }
    out
}

/// ¿La caja queda completamente fuera de algún plano? Conservador: puede
/// aceptar cajas que en realidad no se ven, nunca rechaza una que sí.
pub fn fuera_del_frustum(planos: &[Plano; 6], esquinas: &[DVec3; 8]) -> bool {
    planos
        .iter()
        .any(|p| esquinas.iter().all(|&c| p.distancia(c) < 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_math::{Aabb, DAffine3};
    use forge_render_api::MaterialId;

    fn instancia(bounds: Aabb, transform: DAffine3) -> DrawInstance {
        DrawInstance {
            entity: forge_doc::EntityId::from_u128(1),
            mesh: forge_store::BlobHash::of(b"x"),
            material: MaterialId::DEFAULT,
            transform,
            bounds,
            visible: true,
            selected: false,
        }
    }

    /// Una caja justo delante de la cámara sobrevive al culling; una detrás
    /// del observador, no. Es el control positivo mínimo: un verificador que
    /// siempre dice "dentro" pasaría un test que solo comprobase el caso
    /// positivo.
    #[test]
    fn culling_descarta_lo_que_queda_detras_de_la_camara() {
        let cam = Camera {
            eye: DVec3::new(0.0, -10.0, 0.0),
            target: DVec3::ZERO,
            up: DVec3::Z,
            fov_y_rad: 60f64.to_radians(),
            near_mm: 0.1,
            far_mm: 1000.0,
        };
        let c = Camara::nueva(&cam, 1.0);

        let delante = Aabb::new(DVec3::new(-1.0, -1.0, -1.0), DVec3::new(1.0, 1.0, 1.0));
        assert!(!c.instancia_fuera_del_frustum(&instancia(delante, DAffine3::IDENTITY)));

        let detras = Aabb::new(DVec3::new(-1.0, -20.0, -1.0), DVec3::new(1.0, -18.0, 1.0));
        assert!(c.instancia_fuera_del_frustum(&instancia(detras, DAffine3::IDENTITY)));
    }

    /// La cámara relativa reproduce exactamente la proyección absoluta: para
    /// cualquier punto de mundo, `vp_relativa · (p - ojo)` tiene que dar el
    /// mismo clip space que `vp_absoluta · p`. Esta es la identidad algebraica
    /// de la que depende toda la técnica de coordenadas relativas a cámara.
    #[test]
    fn la_vista_relativa_reproduce_la_proyeccion_absoluta() {
        let cam = Camera {
            eye: DVec3::new(1000.0, -500.0, 300.0),
            target: DVec3::new(1000.0, 0.0, 0.0),
            up: DVec3::Z,
            fov_y_rad: 50f64.to_radians(),
            near_mm: 1.0,
            far_mm: 10_000.0,
        };
        let aspecto = 16.0 / 9.0;
        let c = Camara::nueva(&cam, aspecto);

        // Referencia independiente: la misma construcción que
        // forge-render-cpu, pero calculada aquí a mano en f64 para no
        // depender de ese crate (los pilares no se conocen entre sí).
        let adelante = (cam.target - cam.eye).normalize();
        let vista_absoluta = DMat4::look_at_rh(cam.eye, cam.eye + adelante, cam.up.normalize());
        let proyeccion = DMat4::perspective_rh(cam.fov_y_rad, aspecto, cam.near_mm, cam.far_mm);
        let vp_absoluta = proyeccion * vista_absoluta;

        for p in [
            DVec3::new(1000.0, 50.0, 300.0),
            DVec3::new(950.0, 20.0, 280.0),
            DVec3::new(1200.0, -100.0, 500.0),
        ] {
            let esperado = vp_absoluta * p.extend(1.0);

            let rel = p - cam.eye;
            let m = c.vista_proyeccion_relativa;
            let obtenido = [
                m[0][0] as f64 * rel.x
                    + m[1][0] as f64 * rel.y
                    + m[2][0] as f64 * rel.z
                    + m[3][0] as f64,
                m[0][1] as f64 * rel.x
                    + m[1][1] as f64 * rel.y
                    + m[2][1] as f64 * rel.z
                    + m[3][1] as f64,
                m[0][2] as f64 * rel.x
                    + m[1][2] as f64 * rel.y
                    + m[2][2] as f64 * rel.z
                    + m[3][2] as f64,
                m[0][3] as f64 * rel.x
                    + m[1][3] as f64 * rel.y
                    + m[2][3] as f64 * rel.z
                    + m[3][3] as f64,
            ];

            // Tolerancia amplia: `m` ya pasó por f32, así que el error de
            // redondeo de ese paso es esperable y no lo que se está probando.
            for i in 0..4 {
                assert!(
                    (obtenido[i] - esperado[i]).abs() < 1e-2,
                    "componente {i}: obtenido {obtenido:?} vs esperado {esperado:?}"
                );
            }
        }
    }

    #[test]
    fn matriz_normal_es_identidad_sin_escala() {
        let inst = instancia(Aabb::EMPTY, DAffine3::IDENTITY);
        let n = Camara::matriz_normal(&inst);
        assert_eq!(n[0][0], 1.0);
        assert_eq!(n[1][1], 1.0);
        assert_eq!(n[2][2], 1.0);
    }

    #[test]
    fn matriz_normal_no_da_nan_con_escala_degenerada() {
        let t = DAffine3::from_scale(DVec3::new(1.0, 1.0, 0.0));
        let inst = instancia(Aabb::EMPTY, t);
        let n = Camara::matriz_normal(&inst);
        for fila in n {
            for v in fila {
                assert!(
                    v.is_finite(),
                    "matriz normal con NaN/Inf ante escala degenerada: {n:?}"
                );
            }
        }
    }
}
