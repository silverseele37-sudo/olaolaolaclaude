//! Esquema del índice y migraciones **dirigidas**.
//!
//! El índice es una caché reconstruible, así que sería tentador tratar una base
//! de versión desconocida como basura y rehacerla sin preguntar. No se hace, por
//! el mismo motivo que en `forge-io`: una base de versión *futura* la escribió
//! una build más nueva que esta, y borrarla en silencio destruye trabajo que esa
//! build sí entiende (por ejemplo, columnas que aquí ni se leen). Se falla con un
//! mensaje que dice qué pasó, y quien quiera rehacerla lo pide explícitamente con
//! [`crate::AssetStore::open_rebuilding`].

use rusqlite::Connection;

use crate::{AssetError, Result};

/// Versión del esquema del índice. Cada incremento trae su función de migración
/// y su test; nunca se abre una base de versión desconocida "a ver si va".
pub const ESQUEMA_VERSION: u32 = 2;

/// El esquema completo de la versión 1.
///
/// Las columnas `*_norm` guardan el texto en minúsculas y sin acentos. Sin
/// ellas, buscar "valvula" no encontraría "Válvula", y normalizar en tiempo de
/// consulta obligaría a recorrer la tabla en Rust en vez de en SQLite.
const V1: &str = r#"
CREATE TABLE activos (
    id              TEXT PRIMARY KEY,
    nombre          TEXT NOT NULL,
    nombre_norm     TEXT NOT NULL,
    tipo            INTEGER NOT NULL,
    notas           TEXT NOT NULL,
    notas_norm      TEXT NOT NULL,
    tam             INTEGER NOT NULL,
    importado       INTEGER NOT NULL,
    modificado      INTEGER NOT NULL,
    origen          TEXT NOT NULL,
    version_vigente INTEGER NOT NULL,
    hash_vigente    TEXT NOT NULL,
    miniatura       TEXT
);

-- El origen es la identidad de re-importación: volver a importar la misma ruta
-- es una versión nueva del mismo activo, no un activo distinto.
CREATE UNIQUE INDEX idx_activos_origen     ON activos(origen);
CREATE INDEX        idx_activos_tipo       ON activos(tipo);
CREATE INDEX        idx_activos_importado  ON activos(importado);
CREATE INDEX        idx_activos_modificado ON activos(modificado);
CREATE INDEX        idx_activos_tam        ON activos(tam);

CREATE TABLE versiones (
    id      TEXT NOT NULL,
    version INTEGER NOT NULL,
    hash    TEXT NOT NULL,
    tam     INTEGER NOT NULL,
    creada  INTEGER NOT NULL,
    PRIMARY KEY (id, version)
);
-- La recolección pregunta "¿qué versiones referencian este blob?", no al revés.
CREATE INDEX idx_versiones_hash ON versiones(hash);

CREATE TABLE etiquetas (
    id       TEXT NOT NULL,
    etiqueta TEXT NOT NULL,
    PRIMARY KEY (id, etiqueta)
);
CREATE INDEX idx_etiquetas_etiqueta ON etiquetas(etiqueta);

-- Arista dirigida: `de` depende de `a`.
CREATE TABLE dependencias (
    de TEXT NOT NULL,
    a  TEXT NOT NULL,
    PRIMARY KEY (de, a)
);
-- Sin este índice, `dependents` (el sentido inverso) recorre la tabla entera.
CREATE INDEX idx_dependencias_a ON dependencias(a);

CREATE TABLE meta_indice (
    clave TEXT PRIMARY KEY,
    valor TEXT NOT NULL
);
"#;

/// Qué encontró la apertura del índice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Estado {
    /// No había índice: se creó vacío y hay que rellenarlo desde el diario.
    Creado,
    /// El índice ya estaba en la versión de esta build.
    AlDia,
}

pub fn version(c: &Connection) -> Result<u32> {
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v as u32)
}

/// Versión 2 — índices compuestos que terminan en `id`.
///
/// Medido sobre 100 000 activos: las consultas que filtran por tipo, tamaño o
/// fecha tardaban entre 14 y 52 ms **incluso con `LIMIT 100`**, mientras que las
/// que no usaban índice bajaban a decenas de microsegundos. La causa es que
/// todas las consultas terminan en `ORDER BY a.id`: cuando el filtro usa un
/// índice de una sola columna, SQLite obtiene las filas en orden de esa columna
/// y tiene que **ordenar todas las coincidencias** antes de poder aplicar el
/// `LIMIT`. El límite deja de servir justo cuando más falta hace.
///
/// Un índice `(columna, id)` devuelve las filas ya ordenadas por `id` dentro de
/// cada valor, así que el plan puede recorrerlo y cortar al llegar al límite.
///
/// Los índices de una columna se eliminan: `(tipo, id)` sirve igual de bien para
/// filtrar solo por `tipo`, y mantener los dos duplica el coste de escritura sin
/// dar nada a cambio.
const V2: &str = r#"
DROP INDEX IF EXISTS idx_activos_tipo;
DROP INDEX IF EXISTS idx_activos_importado;
DROP INDEX IF EXISTS idx_activos_modificado;
DROP INDEX IF EXISTS idx_activos_tam;

CREATE INDEX idx_activos_tipo_id       ON activos(tipo, id);
CREATE INDEX idx_activos_importado_id  ON activos(importado, id);
CREATE INDEX idx_activos_modificado_id ON activos(modificado, id);
CREATE INDEX idx_activos_tam_id        ON activos(tam, id);
"#;

/// Crea el esquema desde cero y lo marca con su versión.
pub fn crear(c: &Connection) -> Result<()> {
    c.execute_batch(V1)?;
    c.execute_batch(V2)?;
    c.pragma_update(None, "user_version", i64::from(ESQUEMA_VERSION))?;
    Ok(())
}

/// v1 → v2. Solo toca índices, que son derivados: no puede perder datos.
fn a_v2(c: &Connection) -> Result<()> {
    c.execute_batch(V2)?;
    c.pragma_update(None, "user_version", 2i64)?;
    Ok(())
}

/// Migraciones dirigidas. Hoy solo existe la versión 1; el `match` está para que
/// añadir la 2 sea rellenar un brazo —con su test— y no rediseñar la apertura.
pub fn migrar(c: &Connection) -> Result<Estado> {
    match version(c)? {
        // `user_version` vale 0 en una base recién creada: no hay nada que migrar.
        0 => {
            crear(c)?;
            Ok(Estado::Creado)
        }
        1 => {
            a_v2(c)?;
            Ok(Estado::AlDia)
        }
        2 => Ok(Estado::AlDia),
        v => Err(AssetError::EsquemaFuturo { encontrada: v, soportada: ESQUEMA_VERSION }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices(c: &Connection) -> Vec<String> {
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_activos%' ORDER BY name")
            .expect("consulta");
        let v: Vec<String> = st
            .query_map([], |r| r.get::<_, String>(0))
            .expect("filas")
            .map(|r| r.expect("fila"))
            .collect();
        v
    }

    #[test]
    fn una_base_nueva_nace_en_la_version_vigente() {
        let c = Connection::open_in_memory().expect("memoria");
        assert_eq!(migrar(&c).expect("migrar"), Estado::Creado);
        assert_eq!(version(&c).expect("version"), ESQUEMA_VERSION);
        assert_eq!(
            indices(&c),
            vec![
                "idx_activos_importado_id",
                "idx_activos_modificado_id",
                // el unico de la v1 que sobrevive: es UNIQUE y sostiene la
                // identidad de re-importacion, no es un indice de consulta
                "idx_activos_origen",
                "idx_activos_tam_id",
                "idx_activos_tipo_id"
            ]
        );
    }

    /// La migración v1 → v2 sobre una base que de verdad está en la v1.
    ///
    /// Solo toca índices, que son derivados, así que no puede perder datos: el
    /// test comprueba justamente eso, que las filas siguen ahí después.
    #[test]
    fn migrar_de_v1_a_v2_cambia_los_indices_y_conserva_los_datos() {
        let c = Connection::open_in_memory().expect("memoria");
        c.execute_batch(V1).expect("v1");
        c.pragma_update(None, "user_version", 1i64).expect("marcar v1");
        c.execute(
            "INSERT INTO activos (id, nombre, nombre_norm, tipo, notas, notas_norm, tam,
                                  importado, modificado, origen, version_vigente, hash_vigente)
             VALUES ('X', 'pieza', 'pieza', 0, '', '', 10, 0, 0, '/x', 1, 'h')",
            [],
        )
        .expect("fila de prueba");

        // en la v1 estan los indices de una columna
        assert_eq!(
            indices(&c),
            vec![
                "idx_activos_importado",
                "idx_activos_modificado",
                "idx_activos_origen",
                "idx_activos_tam",
                "idx_activos_tipo"
            ]
        );

        assert_eq!(migrar(&c).expect("migrar"), Estado::AlDia);
        assert_eq!(version(&c).expect("version"), 2);
        assert_eq!(
            indices(&c),
            vec![
                "idx_activos_importado_id",
                "idx_activos_modificado_id",
                "idx_activos_origen",
                "idx_activos_tam_id",
                "idx_activos_tipo_id"
            ],
            "la migracion no reemplazo los indices"
        );
        let n: i64 = c.query_row("SELECT COUNT(*) FROM activos", [], |r| r.get(0)).expect("contar");
        assert_eq!(n, 1, "la migracion perdio datos");
    }

    /// Migrar dos veces no rompe: los `DROP ... IF EXISTS` lo hacen idempotente.
    #[test]
    fn migrar_es_idempotente() {
        let c = Connection::open_in_memory().expect("memoria");
        migrar(&c).expect("primera");
        assert_eq!(migrar(&c).expect("segunda"), Estado::AlDia);
        assert_eq!(version(&c).expect("version"), ESQUEMA_VERSION);
    }

    /// Control: una base de versión **futura** no se toca ni se rehace en
    /// silencio. La escribió una build más nueva y borrarla destruiria trabajo
    /// que esa build si entiende.
    #[test]
    fn una_base_del_futuro_se_rechaza_en_vez_de_rehacerse() {
        let c = Connection::open_in_memory().expect("memoria");
        migrar(&c).expect("crear");
        c.pragma_update(None, "user_version", 99i64).expect("marcar futuro");
        match migrar(&c) {
            Err(AssetError::EsquemaFuturo { encontrada, soportada }) => {
                assert_eq!(encontrada, 99);
                assert_eq!(soportada, ESQUEMA_VERSION);
            }
            otro => panic!("se esperaba EsquemaFuturo, salio {otro:?}"),
        }
        // y no la toco
        assert_eq!(version(&c).expect("version"), 99);
    }
}
