//! Kernel geométrico analítico, sin C++.
//!
//! Es la otra mitad del patrón que `cadviz` valida empíricamente: **dos
//! implementaciones intercambiables de la misma ABI**. La de OpenCASCADE llegará
//! y tendrá que encajar sin que ningún llamante se entere.
//!
//! Esto no es código desechable. Permite desarrollar y testear los otros tres
//! pilares sin OCCT compilado —lo que deja el CI sin un build de veinte minutos
//! en el camino crítico— y es la prueba de que la costura del kernel es real y
//! no decorativa: una interfaz con una sola implementación no está probada como
//! interfaz, solo como nombre.
//!
//! # Alcance, dicho en voz alta
//!
//! Un stub que finge poder con todo es peor que uno honesto. Lo que **no** hace,
//! y devuelve [`KernelError::Unsupported`] en vez de aproximar en silencio:
//!
//! - Booleanos fuera del caso caja-contra-caja alineada a ejes.
//! - `fillet` con geometría de redondeo real: produce la **topología** correcta
//!   (una cara nueva por arista, con procedencia `Blend`) pero la geometría es
//!   un chaflán. Es deliberado: lo que los demás pilares necesitan probar es el
//!   nombrado persistente a través de un cambio de topología, y eso sí es real.
//! - Superficies libres, NURBS, STEP.
//!
//! Los cilindros sí son analíticos: se facetan **en el teselado**, según la
//! tolerancia, no al construirlos. Así el volumen exacto es πr²h y el teselado
//! converge de verdad al refinar.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_kernel_api::*;
use forge_math::{Aabb, DAffine3, DVec2, DVec3};
use serde::{Deserialize, Serialize};

mod ops;
pub mod poly;

use poly::{marca, Cara, Poly};

/// Un cuerpo: o un poliedro de caras planas, o un cilindro analítico.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Body {
    Poly(Poly),
    Cyl(Cilindro),
}

/// Cilindro recto. Analítico a propósito: ver el doc del módulo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cilindro {
    pub base: DVec3,
    /// Unitario.
    pub eje: DVec3,
    pub radio: f64,
    pub altura: f64,
    /// Ids de las tres caras: lateral, tapa inferior, tapa superior.
    pub ids: [StableId; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Shape {
    body: Body,
    owner: FeatureId,
}

/// Kernel analítico. Dueño de la memoria de sus formas, como cualquier kernel.
pub struct StubKernel {
    shapes: RwLock<HashMap<ShapeId, Shape>>,
    siguiente: AtomicU64,
}

impl Default for StubKernel {
    fn default() -> Self {
        StubKernel::new()
    }
}

impl StubKernel {
    pub fn new() -> Self {
        StubKernel { shapes: RwLock::new(HashMap::new()), siguiente: AtomicU64::new(1) }
    }

    /// Formas vivas. Para comprobar en tests que `release` libera de verdad.
    pub fn live_shapes(&self) -> usize {
        self.shapes.read().map(|m| m.len()).unwrap_or(0)
    }

    fn guardar(&self, body: Body, owner: FeatureId) -> KernelResult<ShapeId> {
        let id = ShapeId(self.siguiente.fetch_add(1, Ordering::Relaxed));
        let mut m = self.shapes.write().map_err(|_| KernelError::KernelPanic {
            backtrace: "el almacen de formas quedo envenenado".into(),
        })?;
        m.insert(id, Shape { body, owner });
        Ok(id)
    }

    fn leer(&self, id: ShapeId) -> KernelResult<Shape> {
        let m = self.shapes.read().map_err(|_| KernelError::KernelPanic {
            backtrace: "el almacen de formas quedo envenenado".into(),
        })?;
        m.get(&id).cloned().ok_or(KernelError::UnknownShape(id))
    }

    fn poliedro(&self, id: ShapeId) -> KernelResult<(Poly, FeatureId)> {
        match self.leer(id)? {
            Shape { body: Body::Poly(p), owner } => Ok((p, owner)),
            Shape { body: Body::Cyl(_), .. } => Err(KernelError::Unsupported(
                "la operacion no acepta cilindros analiticos en este kernel",
            )),
        }
    }

    /// Corrige la orientación de todas las caras si el volumen sale negativo.
    ///
    /// Es una red de seguridad general: en vez de razonar el sentido de giro en
    /// cada constructor —donde un signo mal puesto produce un sólido del revés
    /// que solo se nota al sombrear—, se comprueba el resultado y se arregla.
    fn orientar(p: &mut Poly) {
        if p.solido && p.volumen_con_signo() < 0.0 {
            p.invertir();
        }
    }
}

// ---------------------------------------------------------------------------
// Teselado
// ---------------------------------------------------------------------------

/// Segmentos necesarios para que un arco de radio `r` respete la tolerancia.
///
/// La desviación de cuerda de un arco dividido en `n` partes es
/// `r·(1 − cos(π/n))`. Se invierte para despejar `n`, y se respeta además la
/// desviación angular. Esto es lo que hace que el teselado **converja** al
/// refinar en vez de estar facetado de una vez para siempre.
pub fn segmentos_para(radio: f64, p: &TessellationParams) -> u32 {
    let por_cuerda = if p.chord_mm <= 0.0 || p.chord_mm >= radio {
        8.0
    } else {
        let c = (1.0 - p.chord_mm / radio).clamp(-1.0, 1.0);
        std::f64::consts::PI / c.acos()
    };
    let por_angulo = 360.0 / p.angular_deg.max(0.1);
    (por_cuerda.max(por_angulo).ceil() as u32).clamp(8, 4096)
}

impl StubKernel {
    fn teselar_poly(&self, p: &Poly, owner: FeatureId) -> KernelResult<Tessellation> {
        let mut t = Tessellation { bbox: p.bbox(), ..Default::default() };
        for cara in &p.caras {
            let n = p.normal_de(cara);
            // Vértices por cara y no compartidos: dos caras que comparten una
            // arista tienen normales distintas, y compartir el vértice
            // promediaría la normal y redondearía un canto vivo.
            let base = t.positions.len() as u32;
            let mut local: HashMap<u32, u32> = HashMap::new();
            for (k, &v) in cara.bucle.iter().enumerate() {
                local.insert(v, base + k as u32);
                t.positions.push(p.verts[v as usize]);
                t.normals.push(n);
            }
            for tri in p.triangular(cara) {
                for v in tri {
                    t.indices.push(local[&v]);
                }
                t.face_of_triangle.push(cara.id);
            }
        }
        for e in p.aristas(owner)? {
            t.edges.push(EdgePolyline {
                id: e.id,
                kind: e.kind,
                points: vec![p.verts[e.a as usize], p.verts[e.b as usize]],
            });
        }
        t.validate()?;
        Ok(t)
    }

    fn teselar_cyl(&self, c: &Cilindro, p: &TessellationParams) -> KernelResult<Tessellation> {
        let n = segmentos_para(c.radio, p);
        let (u, v) = forge_math::orthonormal_basis(c.eje);
        let punto = |k: u32, arriba: bool| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            c.base + (u * a.cos() + v * a.sin()) * c.radio + c.eje * if arriba { c.altura } else { 0.0 }
        };

        let mut t = Tessellation::default();
        // --- lateral ---
        for k in 0..n {
            let k1 = (k + 1) % n;
            let (b0, b1, t0, t1) = (punto(k, false), punto(k1, false), punto(k, true), punto(k1, true));
            let nb = ((b0 - c.base) - c.eje * (b0 - c.base).dot(c.eje)).normalize();
            let nb1 = ((b1 - c.base) - c.eje * (b1 - c.base).dot(c.eje)).normalize();
            let base = t.positions.len() as u32;
            t.positions.extend_from_slice(&[b0, b1, t1, t0]);
            t.normals.extend_from_slice(&[nb, nb1, nb1, nb]);
            t.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            t.face_of_triangle.push(c.ids[0]);
            t.face_of_triangle.push(c.ids[0]);
        }
        // --- tapas ---
        for (arriba, id, normal) in [(false, c.ids[1], -c.eje), (true, c.ids[2], c.eje)] {
            let centro = t.positions.len() as u32;
            t.positions.push(c.base + c.eje * if arriba { c.altura } else { 0.0 });
            t.normals.push(normal);
            for k in 0..n {
                t.positions.push(punto(k, arriba));
                t.normals.push(normal);
            }
            for k in 0..n {
                let a = centro + 1 + k;
                let b = centro + 1 + (k + 1) % n;
                if arriba {
                    t.indices.extend_from_slice(&[centro, a, b]);
                } else {
                    t.indices.extend_from_slice(&[centro, b, a]);
                }
                t.face_of_triangle.push(id);
            }
        }
        // --- aristas: los dos circulos y la costura ---
        for (arriba, id) in [(false, c.ids[1]), (true, c.ids[2])] {
            let pts: Vec<DVec3> = (0..=n).map(|k| punto(k % n, arriba)).collect();
            t.edges.push(EdgePolyline {
                id: StableId { origin: id.origin, class: TopoClass::Edge, mark: poly::fnv(&[id.mark, 1]) },
                kind: EdgeKind::Sharp,
                points: pts,
            });
        }
        t.edges.push(EdgePolyline {
            id: StableId {
                origin: c.ids[0].origin,
                class: TopoClass::Edge,
                mark: poly::fnv(&[c.ids[0].mark, 7]),
            },
            // La costura de parametrizacion nunca se dibuja: no es un quiebre
            // del solido, es un artefacto de como se describe la superficie.
            kind: EdgeKind::Seam,
            points: vec![punto(0, false), punto(0, true)],
        });

        t.bbox = Aabb::from_points(t.positions.iter().copied());
        t.validate()?;
        Ok(t)
    }
}

// ---------------------------------------------------------------------------
// El contrato
// ---------------------------------------------------------------------------

impl GeometryKernel for StubKernel {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn profile_from_polygon(&self, pts: &[DVec2], owner: FeatureId) -> KernelResult<ShapeId> {
        if pts.len() < 3 {
            return Err(KernelError::InvalidInput {
                detail: format!("un perfil necesita al menos 3 puntos, tiene {}", pts.len()),
            });
        }
        let area: f64 = (0..pts.len())
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
                p.x * q.y - q.x * p.y
            })
            .sum::<f64>()
            * 0.5;
        if area.abs() < 1e-9 {
            return Err(KernelError::Degenerate { hint: "el perfil tiene area nula".into() });
        }
        if ops::se_autointersecta(pts) {
            return Err(KernelError::Degenerate {
                hint: "el perfil se autointersecta; un solido no puede nacer de el".into(),
            });
        }
        // Se normaliza a antihorario: asi el extrude no tiene que adivinar.
        let mut orden: Vec<usize> = (0..pts.len()).collect();
        if area < 0.0 {
            orden.reverse();
        }
        let verts = orden.iter().map(|&i| DVec3::new(pts[i].x, pts[i].y, 0.0)).collect();
        let poly = Poly {
            verts,
            caras: vec![Cara {
                id: StableId { origin: owner, class: TopoClass::Face, mark: marca("profile", 0) },
                prov: TopoProvenance::Primitive { index: 0 },
                bucle: (0..pts.len() as u32).collect(),
            }],
            solido: false,
        };
        self.guardar(Body::Poly(poly), owner)
    }

    fn extrude(&self, profile: ShapeId, opts: ExtrudeOpts, owner: FeatureId) -> KernelResult<ShapeId> {
        let (perfil, _) = self.poliedro(profile)?;
        if perfil.caras.len() != 1 {
            return Err(KernelError::InvalidInput {
                detail: format!("se esperaba un perfil de una cara, tiene {}", perfil.caras.len()),
            });
        }
        if opts.distance_mm.abs() < forge_math::tol::CONFUSION_MM {
            return Err(KernelError::Degenerate { hint: "distancia de extrusion nula".into() });
        }
        let d = opts.direction.normalize_or_zero();
        if d == DVec3::ZERO {
            return Err(KernelError::InvalidInput { detail: "direccion de extrusion nula".into() });
        }
        let (d0, d1) = if opts.symmetric {
            (-d * opts.distance_mm * 0.5, d * opts.distance_mm * 0.5)
        } else {
            (DVec3::ZERO, d * opts.distance_mm)
        };

        let bucle = perfil.caras[0].bucle.clone();
        let m = bucle.len();
        let mut verts = Vec::with_capacity(m * 2);
        for &i in &bucle {
            verts.push(perfil.verts[i as usize] + d0);
        }
        for &i in &bucle {
            verts.push(perfil.verts[i as usize] + d1);
        }

        let mut caras = Vec::with_capacity(m + 2);
        let cara = |mark: u64, prov: TopoProvenance, bucle: Vec<u32>| Cara {
            id: StableId { origin: owner, class: TopoClass::Face, mark },
            prov,
            bucle,
        };
        caras.push(cara(
            marca("cap_start", 0),
            TopoProvenance::Cap { start: true },
            (0..m as u32).rev().collect(),
        ));
        caras.push(cara(
            marca("cap_end", 0),
            TopoProvenance::Cap { start: false },
            (m as u32..2 * m as u32).collect(),
        ));
        for i in 0..m as u32 {
            let j = (i + 1) % m as u32;
            caras.push(cara(
                marca("side", i),
                TopoProvenance::SweptFromProfileEdge { edge_index: i },
                vec![i, j, j + m as u32, i + m as u32],
            ));
        }

        let mut p = Poly { verts, caras, solido: true };
        p.soldar();
        Self::orientar(&mut p);
        self.guardar(Body::Poly(p), owner)
    }

    fn revolve(&self, profile: ShapeId, opts: RevolveOpts, owner: FeatureId) -> KernelResult<ShapeId> {
        let (perfil, _) = self.poliedro(profile)?;
        if perfil.caras.len() != 1 {
            return Err(KernelError::InvalidInput { detail: "se esperaba un perfil de una cara".into() });
        }
        ops::revolve(&perfil, opts, owner).and_then(|p| self.guardar(Body::Poly(p), owner))
    }

    fn box_solid(&self, min: DVec3, max: DVec3, owner: FeatureId) -> KernelResult<ShapeId> {
        let p = ops::caja(min, max, owner, |i| {
            (marca("box_face", i), TopoProvenance::Primitive { index: i })
        })?;
        self.guardar(Body::Poly(p), owner)
    }

    fn cylinder(
        &self,
        base: DVec3,
        axis: DVec3,
        radius_mm: f64,
        height_mm: f64,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        if radius_mm <= 0.0 || height_mm <= 0.0 {
            return Err(KernelError::InvalidInput {
                detail: format!("radio {radius_mm} y altura {height_mm} deben ser positivos"),
            });
        }
        let eje = axis.normalize_or_zero();
        if eje == DVec3::ZERO {
            return Err(KernelError::InvalidInput { detail: "eje nulo".into() });
        }
        let f = |i: u32| StableId { origin: owner, class: TopoClass::Face, mark: marca("cyl", i) };
        self.guardar(
            Body::Cyl(Cilindro { base, eje, radio: radius_mm, altura: height_mm, ids: [f(0), f(1), f(2)] }),
            owner,
        )
    }

    fn fillet(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: FilletSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let FilletSpec::Constant { radius_mm } = spec;
        let (p, prev) = self.poliedro(solid)?;
        // Geometria de chaflan, topologia de redondeo. Documentado arriba.
        let p = ops::biselar(&p, prev, edges, radius_mm, owner, true)?;
        self.guardar(Body::Poly(p), owner)
    }

    fn chamfer(
        &self,
        solid: ShapeId,
        edges: &[StableId],
        spec: ChamferSpec,
        owner: FeatureId,
    ) -> KernelResult<ShapeId> {
        let ChamferSpec::Symmetric { distance_mm } = spec;
        let (p, prev) = self.poliedro(solid)?;
        let p = ops::biselar(&p, prev, edges, distance_mm, owner, false)?;
        self.guardar(Body::Poly(p), owner)
    }

    fn boolean(&self, op: BoolOp, a: ShapeId, b: ShapeId, owner: FeatureId) -> KernelResult<ShapeId> {
        let (pa, oa) = self.poliedro(a)?;
        let (pb, ob) = self.poliedro(b)?;
        let p = ops::booleano(op, &pa, oa, &pb, ob, owner)?;
        self.guardar(Body::Poly(p), owner)
    }

    fn transform(&self, s: ShapeId, m: &DAffine3, owner: FeatureId) -> KernelResult<ShapeId> {
        match self.leer(s)?.body {
            Body::Poly(mut p) => {
                for v in &mut p.verts {
                    *v = m.transform_point3(*v);
                }
                Self::orientar(&mut p);
                self.guardar(Body::Poly(p), owner)
            }
            Body::Cyl(c) => {
                // Solo rigidos: una afin general convierte el cilindro en
                // eliptico, y este kernel no representa eso. Mejor decirlo que
                // devolver algo que no es lo que se pidio.
                let escala = m.matrix3.x_axis.length();
                let uniforme = (m.matrix3.y_axis.length() - escala).abs() < 1e-9
                    && (m.matrix3.z_axis.length() - escala).abs() < 1e-9;
                if !uniforme {
                    return Err(KernelError::Unsupported(
                        "transformar un cilindro con escala no uniforme lo haria eliptico",
                    ));
                }
                self.guardar(
                    Body::Cyl(Cilindro {
                        base: m.transform_point3(c.base),
                        eje: m.transform_vector3(c.eje).normalize(),
                        radio: c.radio * escala,
                        altura: c.altura * escala,
                        ids: c.ids,
                    }),
                    owner,
                )
            }
        }
    }

    fn topology(&self, s: ShapeId) -> KernelResult<TopologySummary> {
        let sh = self.leer(s)?;
        match &sh.body {
            Body::Poly(p) => {
                let mut r = TopologySummary {
                    is_solid: p.solido,
                    is_closed: p.solido,
                    ..Default::default()
                };
                for c in &p.caras {
                    r.faces.push(TopoEntity {
                        id: c.id,
                        provenance: c.prov.clone(),
                        signature: p.firma_de_cara(c),
                    });
                }
                for e in p.aristas(sh.owner)? {
                    r.edges.push(TopoEntity {
                        id: e.id,
                        provenance: e.prov.clone(),
                        signature: p.firma_de_arista(&e),
                    });
                }
                Ok(r)
            }
            Body::Cyl(c) => {
                let mut r =
                    TopologySummary { is_solid: true, is_closed: true, ..Default::default() };
                let centro_lat = c.base + c.eje * (c.altura * 0.5);
                for (i, id) in c.ids.iter().enumerate() {
                    let (centro, normal, medida) = match i {
                        0 => (centro_lat, c.eje, std::f64::consts::TAU * c.radio * c.altura),
                        1 => (c.base, -c.eje, std::f64::consts::PI * c.radio * c.radio),
                        _ => (
                            c.base + c.eje * c.altura,
                            c.eje,
                            std::f64::consts::PI * c.radio * c.radio,
                        ),
                    };
                    r.faces.push(TopoEntity {
                        id: *id,
                        provenance: TopoProvenance::Primitive { index: i as u32 },
                        signature: GeometrySignature::new(centro, normal, medida, TopoClass::Face),
                    });
                }
                Ok(r)
            }
        }
    }

    fn mass_properties(&self, s: ShapeId) -> KernelResult<MassProperties> {
        match self.leer(s)?.body {
            Body::Poly(p) => Ok(p.propiedades()),
            // Exacto, no teselado: es la ventaja de guardarlo analitico.
            Body::Cyl(c) => Ok(MassProperties {
                volume_mm3: std::f64::consts::PI * c.radio * c.radio * c.altura,
                area_mm2: std::f64::consts::TAU * c.radio * (c.radio + c.altura),
                centroid: c.base + c.eje * (c.altura * 0.5),
            }),
        }
    }

    fn bbox(&self, s: ShapeId) -> KernelResult<Aabb> {
        match self.leer(s)?.body {
            Body::Poly(p) => Ok(p.bbox()),
            Body::Cyl(c) => {
                let (u, v) = forge_math::orthonormal_basis(c.eje);
                let r = (u.abs() + v.abs()) * c.radio;
                let a = c.base;
                let b = c.base + c.eje * c.altura;
                Ok(Aabb::new((a - r).min(b - r), (a + r).max(b + r)))
            }
        }
    }

    fn is_valid(&self, s: ShapeId) -> KernelResult<ValidationReport> {
        let sh = self.leer(s)?;
        let mut r = ValidationReport { valid: true, problems: Vec::new() };
        if let Body::Poly(p) = &sh.body {
            if p.solido && p.volumen_con_signo() <= 0.0 {
                r.problems.push("volumen no positivo: caras al reves o solido abierto".into());
            }
            // Dos caras pueden compartir marca si son facetas de la misma
            // superficie logica -- una revolucion produce N cuadrilateros que
            // son *una* cara para el usuario. Lo que no puede pasar es que dos
            // caras con distinta procedencia compartan identidad.
            let mut por_marca: std::collections::HashMap<u64, &TopoProvenance> = HashMap::new();
            for c in &p.caras {
                match por_marca.get(&c.id.mark) {
                    Some(prev) if **prev != c.prov => r
                        .problems
                        .push(format!("marca {} compartida por procedencias distintas", c.id.mark)),
                    _ => {
                        por_marca.insert(c.id.mark, &c.prov);
                    }
                }
            }
            for c in &p.caras {
                if c.bucle.len() < 3 {
                    r.problems.push(format!("cara con {} vertices", c.bucle.len()));
                }
            }
            if let Err(e) = p.aristas(sh.owner) {
                r.problems.push(e.to_string());
            }
        }
        r.valid = r.problems.is_empty();
        Ok(r)
    }

    fn tessellate(&self, s: ShapeId, p: &TessellationParams) -> KernelResult<Tessellation> {
        let sh = self.leer(s)?;
        match &sh.body {
            Body::Poly(poly) => self.teselar_poly(poly, sh.owner),
            Body::Cyl(c) => self.teselar_cyl(c, p),
        }
    }

    fn serialize(&self, s: ShapeId) -> KernelResult<Vec<u8>> {
        let sh = self.leer(s)?;
        let mut out = Vec::new();
        ciborium::into_writer(&sh, &mut out).map_err(|e| KernelError::OperationFailed {
            op: "serialize",
            detail: e.to_string(),
        })?;
        Ok(out)
    }

    fn deserialize(&self, bytes: &[u8], owner: FeatureId) -> KernelResult<ShapeId> {
        let sh: Shape = ciborium::from_reader(bytes).map_err(|e| KernelError::OperationFailed {
            op: "deserialize",
            detail: e.to_string(),
        })?;
        self.guardar(sh.body, owner)
    }

    fn release(&self, s: ShapeId) {
        if let Ok(mut m) = self.shapes.write() {
            m.remove(&s);
        }
    }
}
