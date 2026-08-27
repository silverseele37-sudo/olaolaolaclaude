//! Wavefront OBJ.
//!
//! Formato mínimo y universal: sin PBR, sin jerarquía, sin animación. Sirve para
//! intercambio rápido y como banco de pruebas de ida y vuelta, porque es lo
//! bastante simple como para que un fallo señale al código y no al formato.
//!
//! **Sobre los ejes:** OBJ no especifica ninguna convención. En la práctica las
//! herramientas de DCC asumen Y arriba y las de CAD, Z arriba. Como no hay
//! verdad que respetar, exportamos en Z-up —el nativo, sin pérdida— y dejamos
//! [`ObjOptions::y_up`] para quien lo necesite del otro modo. Suponer en silencio
//! sería peor que ofrecer la opción.

use std::fmt::Write as _;
use std::path::Path;

use forge_math::{DVec2, DVec3};

use crate::{y_up_to_z_up, z_up_to_y_up, InteropError, Result, TriangleSoup};

#[derive(Clone, Copy, Debug, Default)]
pub struct ObjOptions {
    /// Convertir a Y arriba al escribir (y desde Y arriba al leer).
    pub y_up: bool,
    pub write_normals: bool,
    pub write_uvs: bool,
}

impl ObjOptions {
    pub fn completo() -> Self {
        ObjOptions {
            y_up: false,
            write_normals: true,
            write_uvs: true,
        }
    }
}

pub fn to_string(soup: &TriangleSoup, opts: ObjOptions) -> Result<String> {
    soup.validate()?;
    let mut s = String::with_capacity(soup.positions.len() * 32);
    let _ = writeln!(s, "# generado por FORGE");
    let _ = writeln!(
        s,
        "# unidades: milimetros · ejes: {}",
        if opts.y_up { "Y arriba" } else { "Z arriba" }
    );
    if !soup.name.is_empty() {
        let _ = writeln!(s, "o {}", soup.name);
    }

    for p in &soup.positions {
        let p = if opts.y_up { z_up_to_y_up(*p) } else { *p };
        // 9 decimales: por debajo de la tolerancia de confusion (1e-7 mm) sin
        // llenar el archivo de ruido de punto flotante.
        let _ = writeln!(s, "v {:.9} {:.9} {:.9}", p.x, p.y, p.z);
    }
    let con_uv = opts.write_uvs && !soup.uvs.is_empty();
    if con_uv {
        for t in &soup.uvs {
            let _ = writeln!(s, "vt {:.9} {:.9}", t.x, t.y);
        }
    }
    let con_n = opts.write_normals && !soup.normals.is_empty();
    if con_n {
        for n in &soup.normals {
            let n = if opts.y_up { z_up_to_y_up(*n) } else { *n };
            let _ = writeln!(s, "vn {:.9} {:.9} {:.9}", n.x, n.y, n.z);
        }
    }

    for tri in soup.indices.chunks_exact(3) {
        // OBJ indexa desde 1.
        let f = |i: u32| i + 1;
        match (con_uv, con_n) {
            (true, true) => {
                let _ = writeln!(
                    s,
                    "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}",
                    a = f(tri[0]),
                    b = f(tri[1]),
                    c = f(tri[2])
                );
            }
            (false, true) => {
                let _ = writeln!(
                    s,
                    "f {a}//{a} {b}//{b} {c}//{c}",
                    a = f(tri[0]),
                    b = f(tri[1]),
                    c = f(tri[2])
                );
            }
            (true, false) => {
                let _ = writeln!(
                    s,
                    "f {a}/{a} {b}/{b} {c}/{c}",
                    a = f(tri[0]),
                    b = f(tri[1]),
                    c = f(tri[2])
                );
            }
            (false, false) => {
                let _ = writeln!(s, "f {} {} {}", f(tri[0]), f(tri[1]), f(tri[2]));
            }
        }
    }
    Ok(s)
}

pub fn write(path: impl AsRef<Path>, soup: &TriangleSoup, opts: ObjOptions) -> Result<()> {
    let path = path.as_ref();
    let s = to_string(soup, opts)?;
    std::fs::write(path, s).map_err(|e| InteropError::Io {
        path: path.into(),
        source: e,
    })
}

pub fn from_str(txt: &str, opts: ObjOptions) -> Result<TriangleSoup> {
    let mut soup = TriangleSoup::default();
    let mut normales_indexadas: Vec<DVec3> = Vec::new();
    let mut uvs_indexadas: Vec<DVec2> = Vec::new();
    // OBJ permite que un vertice use indices distintos para posicion, UV y
    // normal. Nosotros necesitamos un indice unico, asi que de-duplicamos por
    // la terna: es lo que hace que la ida y vuelta sea fiel en vez de
    // aproximada.
    let mut mapa: std::collections::HashMap<(u32, u32, u32), u32> = Default::default();
    let mut pos_crudas: Vec<DVec3> = Vec::new();

    for (n, linea) in txt.lines().enumerate() {
        let linea = linea.split('#').next().unwrap_or("").trim();
        if linea.is_empty() {
            continue;
        }
        let mut it = linea.split_whitespace();
        let clave = it.next().unwrap_or("");
        let num = |x: Option<&str>, campo: &str| -> Result<f64> {
            x.ok_or_else(|| InteropError::Malformed {
                line: n + 1,
                detail: format!("falta el componente {campo}"),
            })?
            .parse::<f64>()
            .map_err(|e| InteropError::Malformed {
                line: n + 1,
                detail: e.to_string(),
            })
        };

        match clave {
            "o" | "g" => {
                if soup.name.is_empty() {
                    soup.name = it.collect::<Vec<_>>().join(" ");
                }
            }
            "v" => {
                let p = DVec3::new(
                    num(it.next(), "x")?,
                    num(it.next(), "y")?,
                    num(it.next(), "z")?,
                );
                pos_crudas.push(if opts.y_up { y_up_to_z_up(p) } else { p });
            }
            "vn" => {
                let v = DVec3::new(
                    num(it.next(), "x")?,
                    num(it.next(), "y")?,
                    num(it.next(), "z")?,
                );
                normales_indexadas.push(if opts.y_up { y_up_to_z_up(v) } else { v });
            }
            "vt" => {
                uvs_indexadas.push(DVec2::new(num(it.next(), "u")?, num(it.next(), "v")?));
            }
            "f" => {
                let vertices: Vec<&str> = it.collect();
                if vertices.len() < 3 {
                    return Err(InteropError::Malformed {
                        line: n + 1,
                        detail: format!("cara con {} vertices", vertices.len()),
                    });
                }
                let mut idx = Vec::with_capacity(vertices.len());
                for v in &vertices {
                    let mut partes = v.split('/');
                    let leer = |p: Option<&str>, total: usize| -> Result<u32> {
                        let raw: i64 = match p {
                            Some(s) if !s.is_empty() => {
                                s.parse().map_err(|_| InteropError::Malformed {
                                    line: n + 1,
                                    detail: format!("indice `{s}`"),
                                })?
                            }
                            _ => return Ok(u32::MAX), // ausente
                        };
                        // OBJ admite indices negativos, relativos al final.
                        let i = if raw < 0 { total as i64 + raw } else { raw - 1 };
                        if i < 0 || i as usize >= total {
                            return Err(InteropError::Malformed {
                                line: n + 1,
                                detail: format!("indice {raw} fuera de rango ({total} elementos)"),
                            });
                        }
                        Ok(i as u32)
                    };
                    let ip = leer(partes.next(), pos_crudas.len())?;
                    let it_ = leer(partes.next(), uvs_indexadas.len().max(1))?;
                    let in_ = leer(partes.next(), normales_indexadas.len().max(1))?;
                    let terna = (ip, it_, in_);
                    let nuevo = *mapa.entry(terna).or_insert_with(|| {
                        soup.positions.push(pos_crudas[ip as usize]);
                        if in_ != u32::MAX && (in_ as usize) < normales_indexadas.len() {
                            soup.normals.push(normales_indexadas[in_ as usize]);
                        }
                        if it_ != u32::MAX && (it_ as usize) < uvs_indexadas.len() {
                            soup.uvs.push(uvs_indexadas[it_ as usize]);
                        }
                        soup.positions.len() as u32 - 1
                    });
                    idx.push(nuevo);
                }
                // Abanico: correcto para poligonos convexos, que es lo que
                // produce cualquier exportador sensato.
                for k in 1..idx.len() - 1 {
                    soup.indices
                        .extend_from_slice(&[idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {} // mtllib, usemtl, s, ... se ignoran a proposito
        }
    }

    // Si unos vertices tenian normal y otros no, la lista queda descuadrada:
    // mejor descartarla entera que exportar normales que no corresponden.
    if soup.normals.len() != soup.positions.len() {
        soup.normals.clear();
    }
    if soup.uvs.len() != soup.positions.len() {
        soup.uvs.clear();
    }
    soup.validate()?;
    Ok(soup)
}

pub fn read(path: impl AsRef<Path>, opts: ObjOptions) -> Result<TriangleSoup> {
    let path = path.as_ref();
    let txt = std::fs::read_to_string(path).map_err(|e| InteropError::Io {
        path: path.into(),
        source: e,
    })?;
    from_str(&txt, opts)
}
