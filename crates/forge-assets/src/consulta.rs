//! La consulta y su traducción a SQL.
//!
//! `construir` es deliberadamente una función **pura**: recibe la consulta y
//! devuelve el SQL y sus parámetros, sin tocar la base. Así se puede probar que
//! un filtro apagado no añade cláusula y que uno encendido añade exactamente la
//! suya, sin montar un almacén entero para averiguarlo.

use std::collections::BTreeSet;

use rusqlite::types::Value;

use crate::AssetType;

/// Filtros de búsqueda. Todos son opcionales y **se combinan con Y lógico**: una
/// consulta con texto y tipo devuelve lo que cumple las dos cosas.
///
/// El valor por defecto no filtra nada, así que `search(&AssetQuery::default())`
/// lista el almacén entero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetQuery {
    /// Subcadena buscada en el nombre **y** en las notas. Sin distinguir
    /// mayúsculas ni acentos: "valvula" encuentra "Válvula".
    pub text: Option<String>,
    /// El activo tiene **todas** estas etiquetas (Y).
    pub all_tags: Vec<String>,
    /// El activo tiene **alguna** de estas etiquetas (O).
    pub any_tags: Vec<String>,
    /// El activo es de alguno de estos tipos (O).
    pub types: Vec<AssetType>,
    /// Rango inclusivo de fecha de importación, en milisegundos epoch.
    pub imported: Option<(i64, i64)>,
    /// Rango inclusivo de fecha de modificación, en milisegundos epoch.
    pub modified: Option<(i64, i64)>,
    /// Rango inclusivo de tamaño del contenido vigente, en bytes.
    pub size: Option<(u64, u64)>,
    /// Tope de resultados. Sin él se devuelven todos.
    pub limit: Option<u32>,
}

impl AssetQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(mut self, t: impl Into<String>) -> Self {
        self.text = Some(t.into());
        self
    }

    pub fn with_all_tags<S: Into<String>>(mut self, tags: impl IntoIterator<Item = S>) -> Self {
        self.all_tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_any_tags<S: Into<String>>(mut self, tags: impl IntoIterator<Item = S>) -> Self {
        self.any_tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_types(mut self, tipos: impl IntoIterator<Item = AssetType>) -> Self {
        self.types = tipos.into_iter().collect();
        self
    }

    pub fn imported_between(mut self, desde: i64, hasta: i64) -> Self {
        self.imported = Some((desde, hasta));
        self
    }

    pub fn modified_between(mut self, desde: i64, hasta: i64) -> Self {
        self.modified = Some((desde, hasta));
        self
    }

    pub fn size_between(mut self, desde: u64, hasta: u64) -> Self {
        self.size = Some((desde, hasta));
        self
    }

    pub fn with_limit(mut self, n: u32) -> Self {
        self.limit = Some(n);
        self
    }
}

/// Minúsculas y sin acentos.
///
/// No se usa una tabla Unicode completa a propósito: el alcance real es el
/// nombre de un activo escrito por una persona, y las cinco vocales acentuadas
/// más `ñ` y `ç` cubren castellano, catalán, portugués y francés. Una
/// dependencia de normalización Unicode entera costaría más de lo que aporta
/// aquí, y `to_lowercase` ya hace el resto del trabajo.
pub(crate) fn normalizar(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        for l in c.to_lowercase() {
            out.push(match l {
                'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
                'é' | 'è' | 'ë' | 'ê' => 'e',
                'í' | 'ì' | 'ï' | 'î' => 'i',
                'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
                'ú' | 'ù' | 'ü' | 'û' => 'u',
                'ñ' => 'n',
                'ç' => 'c',
                otro => otro,
            });
        }
    }
    out
}

/// Envuelve el texto en comodines y escapa los que traiga el usuario. Sin esto,
/// buscar "50%" devolvería medio almacén.
fn patron_like(t: &str) -> String {
    let mut p = String::with_capacity(t.len() + 4);
    p.push('%');
    for c in t.chars() {
        if c == '%' || c == '_' || c == '\\' {
            p.push('\\');
        }
        p.push(c);
    }
    p.push('%');
    p
}

fn huecos(n: usize) -> String {
    let mut s = String::with_capacity(n * 2);
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}

/// Traduce la consulta a SQL con parámetros ligados.
///
/// El orden del resultado es por `id`, que al ser un ULID **es** el orden de
/// importación: una búsqueda repetida devuelve siempre la misma lista, que es
/// lo que permite que los tests cuenten resultados exactos.
pub(crate) fn construir(q: &AssetQuery) -> (String, Vec<Value>) {
    let mut sql = String::from("SELECT a.id FROM activos a WHERE 1=1");
    let mut p: Vec<Value> = Vec::new();

    if let Some(t) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        sql.push_str(" AND (a.nombre_norm LIKE ? ESCAPE '\\' OR a.notas_norm LIKE ? ESCAPE '\\')");
        let pat = patron_like(&normalizar(t));
        p.push(Value::Text(pat.clone()));
        p.push(Value::Text(pat));
    }

    if !q.types.is_empty() {
        sql.push_str(&format!(" AND a.tipo IN ({})", huecos(q.types.len())));
        for t in &q.types {
            p.push(Value::Integer(i64::from(t.code())));
        }
    }

    if let Some((d, h)) = q.imported {
        sql.push_str(" AND a.importado BETWEEN ? AND ?");
        p.push(Value::Integer(d));
        p.push(Value::Integer(h));
    }

    if let Some((d, h)) = q.modified {
        sql.push_str(" AND a.modificado BETWEEN ? AND ?");
        p.push(Value::Integer(d));
        p.push(Value::Integer(h));
    }

    if let Some((d, h)) = q.size {
        sql.push_str(" AND a.tam BETWEEN ? AND ?");
        p.push(Value::Integer(a_i64_saturado(d)));
        p.push(Value::Integer(a_i64_saturado(h)));
    }

    // Repetidas se cuentan una vez: si no, el `= n` de abajo pediría más
    // etiquetas de las que el usuario nombró y no encontraría nada.
    let todas: BTreeSet<String> = q.all_tags.iter().map(|t| normalizar(t)).collect();
    if !todas.is_empty() {
        sql.push_str(&format!(
            " AND (SELECT COUNT(*) FROM etiquetas e WHERE e.id = a.id AND e.etiqueta IN ({})) = ?",
            huecos(todas.len())
        ));
        for t in &todas {
            p.push(Value::Text(t.clone()));
        }
        p.push(Value::Integer(todas.len() as i64));
    }

    let alguna: BTreeSet<String> = q.any_tags.iter().map(|t| normalizar(t)).collect();
    if !alguna.is_empty() {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM etiquetas e WHERE e.id = a.id AND e.etiqueta IN ({}))",
            huecos(alguna.len())
        ));
        for t in &alguna {
            p.push(Value::Text(t.clone()));
        }
    }

    sql.push_str(" ORDER BY a.id");

    if let Some(n) = q.limit {
        sql.push_str(" LIMIT ?");
        p.push(Value::Integer(i64::from(n)));
    }

    (sql, p)
}

/// `u64` a `i64` **saturando**, no truncando.
///
/// Los enteros de SQLite son con signo de 64 bits, así que un `u64` por encima
/// de `i64::MAX` no se puede representar. Un `as i64` a secas envuelve:
/// `u64::MAX` se convierte en `-1`, y una consulta de «cualquier tamaño»
/// —`size_between(0, u64::MAX)`— se transforma en `BETWEEN 0 AND -1`, que no
/// encuentra nada. En silencio, además: no hay error, solo cero resultados.
///
/// Saturar es correcto aquí porque ningún archivo puede medir más de
/// `i64::MAX` bytes: el límite recortado está fuera del dominio real.
fn a_i64_saturado(v: u64) -> i64 {
    v.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::a_i64_saturado;

    /// Control del desbordamiento: sin saturación, `u64::MAX` da `-1` y toda
    /// consulta abierta por arriba devuelve vacío.
    #[test]
    fn saturar_en_vez_de_envolver() {
        assert_eq!(a_i64_saturado(0), 0);
        assert_eq!(a_i64_saturado(1_000), 1_000);
        assert_eq!(a_i64_saturado(u64::MAX), i64::MAX);
        assert_eq!(a_i64_saturado(i64::MAX as u64), i64::MAX);
        assert_eq!(a_i64_saturado(i64::MAX as u64 + 1), i64::MAX);
        // el bug que esto evita, hecho explicito
        assert_eq!(u64::MAX as i64, -1, "asi envolveria un `as` a secas");
        assert!(
            a_i64_saturado(u64::MAX) > a_i64_saturado(0),
            "un rango abierto debe seguir siendo un rango"
        );
    }
}
