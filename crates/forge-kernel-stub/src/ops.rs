//! Operaciones de construcción y modificación.

use std::collections::{BTreeMap, HashMap};

use forge_doc::{FeatureId, StableId, TopoClass};
use forge_kernel_api::{BoolOp, KernelError, KernelResult, RevolveOpts, TopoProvenance};
use forge_math::{tol, Aabb, DVec2, DVec3};

use crate::poly::{marca, Cara, Poly};

/// Detección de auto-intersección en un perfil, O(n²).
///
/// Barato para un sketch (decenas de segmentos) y evita el fallo más
/// desconcertante del modelado: un sólido con volumen negativo o con caras que
/// se cruzan, que se ve raro sin que nada haya dado error.
pub fn se_autointersecta(pts: &[DVec2]) -> bool {
    let n = pts.len();
    let cruz = |o: DVec2, a: DVec2, b: DVec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    for i in 0..n {
        let (a1, a2) = (pts[i], pts[(i + 1) % n]);
        for j in (i + 1)..n {
            // Los segmentos contiguos comparten un extremo: no cuenta.
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            let (b1, b2) = (pts[j], pts[(j + 1) % n]);
            let (d1, d2) = (cruz(a1, a2, b1), cruz(a1, a2, b2));
            let (d3, d4) = (cruz(b1, b2, a1), cruz(b1, b2, a2));
            if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
                return true;
            }
        }
    }
    false
}

/// Caja alineada a ejes.
///
/// Índice de cara, fijo y parte del contrato del stub:
/// `0` = −Z, `1` = +Z, `2` = −Y, `3` = +Y, `4` = +X, `5` = −X.
pub fn caja(
    min: DVec3,
    max: DVec3,
    owner: FeatureId,
    id: impl Fn(u32) -> (u64, TopoProvenance),
) -> KernelResult<Poly> {
    if (max - min).min_element() <= tol::CONFUSION_MM {
        return Err(KernelError::Degenerate {
            hint: format!("caja degenerada: min {min:?} max {max:?}"),
        });
    }
    let v = vec![
        DVec3::new(min.x, min.y, min.z),
        DVec3::new(max.x, min.y, min.z),
        DVec3::new(max.x, max.y, min.z),
        DVec3::new(min.x, max.y, min.z),
        DVec3::new(min.x, min.y, max.z),
        DVec3::new(max.x, min.y, max.z),
        DVec3::new(max.x, max.y, max.z),
        DVec3::new(min.x, max.y, max.z),
    ];
    let bucles: [[u32; 4]; 6] = [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [1, 2, 6, 5],
        [0, 4, 7, 3],
    ];
    let caras = bucles
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (mark, prov) = id(i as u32);
            Cara {
                id: StableId {
                    origin: owner,
                    class: TopoClass::Face,
                    mark,
                },
                prov,
                bucle: b.to_vec(),
            }
        })
        .collect();
    Ok(Poly {
        verts: v,
        caras,
        solido: true,
    })
}

/// Revolución facetada.
///
/// **Todas las facetas que salen de la misma arista del perfil comparten
/// `StableId`**: son una sola superficie lógica partida en trozos por el
/// teselado, y tratarlas como caras distintas rompería la selección en cuanto
/// cambiara el número de segmentos.
pub fn revolve(perfil: &Poly, opts: RevolveOpts, owner: FeatureId) -> KernelResult<Poly> {
    let eje = opts.axis_dir.normalize_or_zero();
    if eje == DVec3::ZERO {
        return Err(KernelError::InvalidInput {
            detail: "eje de revolucion nulo".into(),
        });
    }
    if opts.angle_rad.abs() < 1e-9 {
        return Err(KernelError::Degenerate {
            hint: "angulo de revolucion nulo".into(),
        });
    }
    let completo = opts.angle_rad.abs() >= std::f64::consts::TAU - 1e-9;
    let n = ((opts.angle_rad.abs().to_degrees() / 15.0).ceil() as u32).clamp(3, 720);

    let bucle = &perfil.caras[0].bucle;
    let m = bucle.len();
    let anillos = if completo { n } else { n + 1 };

    let girar = |p: DVec3, ang: f64| {
        let d = p - opts.axis_origin;
        let paralelo = eje * d.dot(eje);
        let perp = d - paralelo;
        let orto = eje.cross(perp);
        opts.axis_origin + paralelo + perp * ang.cos() + orto * ang.sin()
    };

    let mut verts = Vec::with_capacity(m * anillos as usize);
    for k in 0..anillos {
        let ang = opts.angle_rad * k as f64 / n as f64;
        for &i in bucle {
            verts.push(girar(perfil.verts[i as usize], ang));
        }
    }

    let idx = |k: u32, i: usize| (k % anillos) * m as u32 + i as u32;
    let mut caras = Vec::new();
    for i in 0..m {
        let j = (i + 1) % m;
        let mark = marca("rev_side", i as u32);
        for k in 0..n {
            caras.push(Cara {
                id: StableId {
                    origin: owner,
                    class: TopoClass::Face,
                    mark,
                },
                prov: TopoProvenance::SweptFromProfileEdge {
                    edge_index: i as u32,
                },
                bucle: vec![idx(k, i), idx(k, j), idx(k + 1, j), idx(k + 1, i)],
            });
        }
    }
    if !completo {
        caras.push(Cara {
            id: StableId {
                origin: owner,
                class: TopoClass::Face,
                mark: marca("cap_start", 0),
            },
            prov: TopoProvenance::Cap { start: true },
            bucle: (0..m as u32).rev().collect(),
        });
        let base = n * m as u32;
        caras.push(Cara {
            id: StableId {
                origin: owner,
                class: TopoClass::Face,
                mark: marca("cap_end", 0),
            },
            prov: TopoProvenance::Cap { start: false },
            bucle: (base..base + m as u32).collect(),
        });
    }

    let mut p = Poly {
        verts,
        caras,
        solido: true,
    };
    p.soldar();
    if p.volumen_con_signo() < 0.0 {
        p.invertir();
    }
    Ok(p)
}

/// Chaflán, y también la topología del redondeo.
///
/// Cada arista seleccionada se sustituye por una cara nueva con procedencia
/// `Blend`; las caras adyacentes quedan `Inherited`. Es lo que el nombrado
/// persistente necesita poder probar: un cambio de topología aguas arriba con
/// referencias que tienen que sobrevivirlo.
///
/// **Límite del stub, explícito**: solo aristas cuyos extremos tengan
/// exactamente tres caras incidentes —una esquina normal— y que no compartan
/// vértice entre sí. Fuera de eso devuelve error en vez de producir un sólido
/// con agujeros.
pub fn biselar(
    p: &Poly,
    prev_owner: FeatureId,
    seleccion: &[StableId],
    dist: f64,
    owner: FeatureId,
    es_fillet: bool,
) -> KernelResult<Poly> {
    if dist <= 0.0 {
        return Err(KernelError::InvalidInput {
            detail: format!("distancia {dist} no positiva"),
        });
    }
    if seleccion.is_empty() {
        return Err(KernelError::InvalidInput {
            detail: "sin aristas seleccionadas".into(),
        });
    }
    let aristas = p.aristas(prev_owner)?;
    let mut elegidas = Vec::new();
    for s in seleccion {
        let e = aristas
            .iter()
            .find(|e| e.id == *s)
            .ok_or(KernelError::UnresolvedReference(*s))?;
        if e.caras.len() != 2 {
            return Err(KernelError::Degenerate {
                hint: "no se puede biselar una arista de borde libre".into(),
            });
        }
        elegidas.push(e.clone());
    }

    // Vértices tocados: cada uno como mucho por una arista seleccionada.
    let mut tocado: HashMap<u32, usize> = HashMap::new();
    for (k, e) in elegidas.iter().enumerate() {
        for v in [e.a, e.b] {
            if tocado.insert(v, k).is_some() {
                return Err(KernelError::Unsupported(
                    "este kernel no bisela aristas que comparten vertice; seleccionalas por separado",
                ));
            }
        }
    }

    // Caras incidentes a cada vértice, y la cara vecina por cada arista.
    let mut caras_de_vert: HashMap<u32, Vec<u32>> = HashMap::new();
    for (fi, c) in p.caras.iter().enumerate() {
        for &v in &c.bucle {
            caras_de_vert.entry(v).or_default().push(fi as u32);
        }
    }
    let mut caras_de_arista: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
    for (fi, c) in p.caras.iter().enumerate() {
        let m = c.bucle.len();
        for i in 0..m {
            let (a, b) = (c.bucle[i], c.bucle[(i + 1) % m]);
            caras_de_arista
                .entry((a.min(b), a.max(b)))
                .or_default()
                .push(fi as u32);
        }
    }
    for v in tocado.keys() {
        let n = caras_de_vert.get(v).map(|c| c.len()).unwrap_or(0);
        if n != 3 {
            return Err(KernelError::Unsupported(
                "este kernel solo bisela esquinas de tres caras",
            ));
        }
    }

    let mut verts = p.verts.clone();
    // (vertice, cara) -> copia desplazada
    let mut copia: HashMap<(u32, u32), u32> = HashMap::new();
    for e in &elegidas {
        let d = (p.verts[e.b as usize] - p.verts[e.a as usize]).normalize();
        for &fi in &e.caras {
            let cara = &p.caras[fi as usize];
            let n = p.normal_de(cara);
            let mut hacia = n.cross(d).normalize();
            let centro = p.centroide_de(cara);
            if hacia.dot(centro - p.verts[e.a as usize]) < 0.0 {
                hacia = -hacia;
            }
            for v in [e.a, e.b] {
                let nuevo = verts.len() as u32;
                verts.push(p.verts[v as usize] + hacia * dist);
                copia.insert((v, fi), nuevo);
            }
        }
    }

    let mut caras: Vec<Cara> = Vec::with_capacity(p.caras.len() + elegidas.len());
    for (fi, c) in p.caras.iter().enumerate() {
        let fi = fi as u32;
        let mut bucle = Vec::with_capacity(c.bucle.len() + 2);
        let m = c.bucle.len();
        for i in 0..m {
            let v = c.bucle[i];
            let Some(&k) = tocado.get(&v) else {
                bucle.push(v);
                continue;
            };
            let e = &elegidas[k];
            if e.caras.contains(&fi) {
                // Cara adyacente a la arista biselada: un solo reemplazo.
                bucle.push(copia[&(v, fi)]);
            } else {
                // Tercera cara de la esquina: hay que meter las DOS copias, y
                // el orden importa -- al reves el bucle se cruza sobre si mismo
                // y sale un solido con volumen erroneo que ademas parece valido.
                //
                // El orden se decide **geometricamente**, no por topologia. La
                // version anterior miraba con cual de las dos caras comparte la
                // arista anterior del bucle, y eso presupone una relacion de
                // enrollado entre las tres caras de la esquina que **no es
                // uniforme**: en un cubo la cara de arriba esta enrollada al
                // reves que la de abajo respecto de las laterales, porque asi
                // tienen que estar para mirar ambas hacia fuera.
                //
                // Medido: 6 de las 12 aristas de un cubo daban volumen
                // incorrecto -- 935, 868.33, y una 1001.67, que *aumentaba* el
                // volumen -- y `is_valid` no lo detectaba en ninguna.
                //
                // Las dos copias caen en el plano de esta cara y ambas cuelgan
                // de `v`, asi que basta ordenarlas por proximidad angular al
                // vertice anterior del bucle: la que apunta mas hacia `prev` va
                // primero. Eso no depende del enrollado de ninguna cara.
                let pv = p.verts[v as usize];
                let hacia_prev = (p.verts[c.bucle[(i + m - 1) % m] as usize] - pv)
                    .normalize_or_zero();
                let (f0, f1) = (e.caras[0], e.caras[1]);
                let alineacion = |f: u32| {
                    (verts[copia[&(v, f)] as usize] - pv)
                        .normalize_or_zero()
                        .dot(hacia_prev)
                };
                let (primera, segunda) = if alineacion(f0) >= alineacion(f1) {
                    (f0, f1)
                } else {
                    (f1, f0)
                };
                bucle.push(copia[&(v, primera)]);
                bucle.push(copia[&(v, segunda)]);
            }
        }
        caras.push(Cara {
            id: StableId {
                origin: owner,
                class: TopoClass::Face,
                mark: crate::poly::fnv(&[c.id.mark, 0xA1]),
            },
            prov: TopoProvenance::Inherited { from: c.id },
            bucle,
        });
    }

    let etiqueta = if es_fillet { "fillet" } else { "chamfer" };
    for (k, e) in elegidas.iter().enumerate() {
        let (f0, f1) = (e.caras[0], e.caras[1]);
        let mut bucle = vec![
            copia[&(e.a, f0)],
            copia[&(e.b, f0)],
            copia[&(e.b, f1)],
            copia[&(e.a, f1)],
        ];

        // **Orientar la cara nueva explicitamente.**
        //
        // El orden de arriba depende de cual de las dos caras adyacentes salga
        // como `e.caras[0]`, y eso lo decide la iteracion de un BTreeMap: es
        // arbitrario. Si salen al reves, el cuadrilatero queda enrollado hacia
        // dentro, y el `invertir()` del final no lo arregla porque ese solo da
        // la vuelta al solido entero, no a una cara suelta.
        //
        // El resultado medido era un solido que parecia valido y tenia el
        // volumen mal: de las 12 aristas de un cubo, 6 daban 935, 868.33 o
        // incluso 1001.67 -- esta ultima *aumentando* el volumen, que es la
        // firma inconfundible de una cara del reves.
        //
        // La normal del chaflan tiene que apuntar hacia fuera, y hacia fuera en
        // una arista convexa es la suma de las normales de las dos caras que
        // sustituye.
        let hacia_fuera = (p.normal_de(&p.caras[f0 as usize])
            + p.normal_de(&p.caras[f1 as usize]))
        .normalize_or_zero();
        let (a, b, c) = (
            verts[bucle[0] as usize],
            verts[bucle[1] as usize],
            verts[bucle[2] as usize],
        );
        if (b - a).cross(c - a).dot(hacia_fuera) < 0.0 {
            bucle.reverse();
        }

        caras.push(Cara {
            id: StableId {
                origin: owner,
                class: TopoClass::Face,
                mark: marca(etiqueta, k as u32),
            },
            prov: TopoProvenance::Blend { of: e.id },
            bucle,
        });
    }

    let mut r = Poly {
        verts,
        caras,
        solido: true,
    };
    r.soldar();
    if r.volumen_con_signo() < 0.0 {
        r.invertir();
    }
    Ok(r)
}

// ---------------------------------------------------------------------------
// Booleanos
// ---------------------------------------------------------------------------

/// ¿Es este poliedro exactamente su caja envolvente?
fn es_caja(p: &Poly) -> Option<Aabb> {
    if p.caras.len() != 6 || p.verts.len() != 8 {
        return None;
    }
    let b = p.bbox();
    let t = b.size();
    let vol_caja = t.x * t.y * t.z;
    if (p.volumen_con_signo().abs() - vol_caja).abs() > vol_caja * 1e-9 {
        return None;
    }
    Some(b)
}

fn interseccion(a: Aabb, b: Aabb) -> Option<Aabb> {
    let min = a.min.max(b.min);
    let max = a.max.min(b.max);
    if (max - min).min_element() <= tol::CONFUSION_MM {
        None
    } else {
        Some(Aabb::new(min, max))
    }
}

/// `a` menos `c`, como hasta seis cajas disjuntas.
fn restar(a: Aabb, c: Aabb) -> Vec<Aabb> {
    let mut out = Vec::new();
    let mut push = |min: DVec3, max: DVec3| {
        if (max - min).min_element() > tol::CONFUSION_MM {
            out.push(Aabb::new(min, max));
        }
    };
    push(a.min, DVec3::new(c.min.x, a.max.y, a.max.z));
    push(DVec3::new(c.max.x, a.min.y, a.min.z), a.max);
    let (x0, x1) = (c.min.x.max(a.min.x), c.max.x.min(a.max.x));
    push(
        DVec3::new(x0, a.min.y, a.min.z),
        DVec3::new(x1, c.min.y, a.max.z),
    );
    push(
        DVec3::new(x0, c.max.y, a.min.z),
        DVec3::new(x1, a.max.y, a.max.z),
    );
    let (y0, y1) = (c.min.y.max(a.min.y), c.max.y.min(a.max.y));
    push(DVec3::new(x0, y0, a.min.z), DVec3::new(x1, y1, c.min.z));
    push(DVec3::new(x0, y0, c.max.z), DVec3::new(x1, y1, a.max.z));
    out
}

/// Booleano exacto, **solo** caja contra caja alineada a ejes.
///
/// Un booleano general robusto es donde mueren los kernels: hacerlo a medias
/// aquí produciría resultados plausibles y equivocados. Fuera de este caso se
/// devuelve `Unsupported`, que es información útil.
pub fn booleano(
    op: BoolOp,
    a: &Poly,
    _oa: FeatureId,
    b: &Poly,
    _ob: FeatureId,
    owner: FeatureId,
) -> KernelResult<Poly> {
    let (ca, cb) = match (es_caja(a), es_caja(b)) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(KernelError::Unsupported(
                "este kernel solo hace booleanos entre cajas alineadas a ejes",
            ))
        }
    };
    let comun = interseccion(ca, cb);

    let piezas: Vec<(Aabb, StableId)> = match op {
        BoolOp::Intersection => match comun {
            Some(c) => vec![(c, a.caras[0].id)],
            None => {
                return Err(KernelError::Degenerate {
                    hint: "la interseccion es vacia".into(),
                })
            }
        },
        BoolOp::Difference => {
            let trozos = match comun {
                Some(c) => restar(ca, c),
                None => vec![ca],
            };
            if trozos.is_empty() {
                return Err(KernelError::Degenerate {
                    hint: "la diferencia elimina el solido entero".into(),
                });
            }
            trozos.into_iter().map(|t| (t, a.caras[0].id)).collect()
        }
        BoolOp::Union => {
            let mut v = vec![(ca, a.caras[0].id)];
            let resto = match comun {
                Some(c) => restar(cb, c),
                None => vec![cb],
            };
            v.extend(resto.into_iter().map(|t| (t, b.caras[0].id)));
            v
        }
    };

    frontera_de_la_union(&piezas, owner)
}

/// Construye la frontera de una unión de cajas disjuntas alineadas a ejes.
///
/// El camino obvio -- emitir cada pieza como una caja cerrada y concatenarlas --
/// da el **volumen correcto** y el **área equivocada**. El volumen sale bien
/// porque las caras de contacto entre dos piezas tienen normales opuestas y sus
/// contribuciones al teorema de la divergencia se cancelan; el área no se
/// cancela, se suma. Medido: un cubo de 10 menos una esquina de 4 daba 816 mm²
/// cuando el valor real es 600, un 36 % de más, y `is_valid()` no veía nada raro
/// porque cada caja por separado sí es una malla cerrada y correcta.
///
/// Tampoco vale con borrar los pares de caras coincidentes: las piezas que
/// devuelve `restar` se tocan en **trozos** de cara, no en caras enteras. La
/// pieza `x < c.min.x` es una losa de altura completa, y contra ella apoyan
/// cuatro piezas más pequeñas. Buscar pares idénticos no encontraría ninguno.
///
/// Lo que sí funciona: comprimir coordenadas. Se toman todos los valores
/// distintos de x, y, z que aparecen en los límites de las piezas y se forma una
/// rejilla. Cada celda de esa rejilla está entera dentro o entera fuera del
/// sólido -- por construcción, porque ninguna cara de ninguna pieza la
/// atraviesa. Entonces la frontera es exactamente el conjunto de caras de celda
/// llena que dan a celda vacía (o al exterior), y eso se enumera sin ambigüedad.
///
/// Sale una malla soldada (los vértices se comparten por índice de rejilla, sin
/// tolerancias de por medio), cerrada y sin caras interiores. El precio es que
/// una cara grande queda partida en las celdas que la componen: más caras de las
/// mínimas. Se prefiere así a fusionarlas, porque fusionar coplanares vuelve a
/// meter uniones en T -- una cara larga contra varias cortas -- y una unión en T
/// deja aristas con una sola cara, o sea un sólido abierto disfrazado.
fn frontera_de_la_union(piezas: &[(Aabb, StableId)], owner: FeatureId) -> KernelResult<Poly> {
    // Coordenadas distintas por eje. Vienen de límites de cajas, así que los
    // duplicados son bit a bit iguales; el `dedup` con tolerancia es por si
    // acaso.
    let eje = |f: fn(&Aabb) -> (f64, f64)| {
        let mut v: Vec<f64> = piezas
            .iter()
            .flat_map(|(c, _)| {
                let (a, b) = f(c);
                [a, b]
            })
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup_by(|a, b| (*a - *b).abs() <= tol::CONFUSION_MM);
        v
    };
    let xs = eje(|c| (c.min.x, c.max.x));
    let ys = eje(|c| (c.min.y, c.max.y));
    let zs = eje(|c| (c.min.z, c.max.z));
    let (nx, ny, nz) = (xs.len() - 1, ys.len() - 1, zs.len() - 1);

    // Este kernel solo hace caja contra caja, así que la rejilla es diminuta
    // (a lo sumo 6 piezas). El tope es para que un cambio futuro reviente aquí
    // en vez de asignar gigabytes en silencio.
    if nx * ny * nz > 100_000 {
        return Err(KernelError::Degenerate {
            hint: format!("rejilla {nx}x{ny}x{nz} demasiado grande"),
        });
    }

    // Qué pieza ocupa cada celda, por el centro. Las piezas son disjuntas, así
    // que a lo sumo una.
    let idx = |i: usize, j: usize, k: usize| (i * ny + j) * nz + k;
    let mut ocupa = vec![None; nx * ny * nz];
    for i in 0..nx {
        for j in 0..ny {
            for k in 0..nz {
                let c = DVec3::new(
                    (xs[i] + xs[i + 1]) * 0.5,
                    (ys[j] + ys[j + 1]) * 0.5,
                    (zs[k] + zs[k + 1]) * 0.5,
                );
                ocupa[idx(i, j, k)] = piezas.iter().position(|(caja_p, _)| {
                    c.cmpge(caja_p.min).all() && c.cmple(caja_p.max).all()
                });
            }
        }
    }
    let lleno = |i: i64, j: i64, k: i64| -> bool {
        i >= 0
            && j >= 0
            && k >= 0
            && (i as usize) < nx
            && (j as usize) < ny
            && (k as usize) < nz
            && ocupa[idx(i as usize, j as usize, k as usize)].is_some()
    };

    // Vértices bajo demanda, indexados por nodo de rejilla: dos celdas vecinas
    // comparten los suyos por construcción, sin comparar coordenadas.
    let mut verts: Vec<DVec3> = Vec::new();
    let nodo_a_vert = &mut vec![u32::MAX; (nx + 1) * (ny + 1) * (nz + 1)];
    let mut nodo = |verts: &mut Vec<DVec3>, i: usize, j: usize, k: usize| -> u32 {
        let n = (i * (ny + 1) + j) * (nz + 1) + k;
        if nodo_a_vert[n] == u32::MAX {
            nodo_a_vert[n] = verts.len() as u32;
            verts.push(DVec3::new(xs[i], ys[j], zs[k]));
        }
        nodo_a_vert[n]
    };

    // Los mismos seis bucles que `caja`, en los mismos índices locales, para no
    // volver a razonar el sentido de giro: 0..7 son las esquinas de la celda con
    // el bit 0 en x, el 1 en y y el 2 en z según el orden de `caja`.
    const ESQUINAS: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [1, 1, 1],
        [0, 1, 1],
    ];
    // (desplazamiento al vecino, bucle local). Si el vecino está lleno, la cara
    // es interior y no se emite.
    const CARAS: [([i64; 3], [usize; 4]); 6] = [
        ([0, 0, -1], [0, 3, 2, 1]),
        ([0, 0, 1], [4, 5, 6, 7]),
        ([0, -1, 0], [0, 1, 5, 4]),
        ([0, 1, 0], [2, 3, 7, 6]),
        ([1, 0, 0], [1, 2, 6, 5]),
        ([-1, 0, 0], [0, 4, 7, 3]),
    ];

    let mut caras = Vec::new();
    for i in 0..nx {
        for j in 0..ny {
            for k in 0..nz {
                let Some(pieza) = ocupa[idx(i, j, k)] else {
                    continue;
                };
                let origen = piezas[pieza].1;
                for (cara, (d, bucle)) in CARAS.iter().enumerate() {
                    if lleno(i as i64 + d[0], j as i64 + d[1], k as i64 + d[2]) {
                        continue;
                    }
                    let bucle = bucle
                        .iter()
                        .map(|&e| {
                            let o = ESQUINAS[e];
                            nodo(&mut verts, i + o[0], j + o[1], k + o[2])
                        })
                        .collect();
                    caras.push(Cara {
                        id: StableId {
                            origin: owner,
                            class: TopoClass::Face,
                            mark: crate::poly::fnv(&[
                                origen.mark,
                                pieza as u64,
                                cara as u64,
                                i as u64,
                                j as u64,
                                k as u64,
                            ]),
                        },
                        prov: TopoProvenance::SplitFrom {
                            original: origen,
                            piece: pieza as u32,
                        },
                        bucle,
                    });
                }
            }
        }
    }
    if caras.is_empty() {
        return Err(KernelError::Degenerate {
            hint: "el resultado del booleano es vacio".into(),
        });
    }
    Ok(Poly {
        verts,
        caras,
        solido: true,
    })
}
