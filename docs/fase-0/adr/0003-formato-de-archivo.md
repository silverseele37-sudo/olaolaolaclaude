# ADR-0003 — Formato de archivo: contenedor ZIP + almacén direccionado por contenido

**Estado:** aceptada
**Fecha:** 2026-08-26

## Decisión

Un documento `.forge` es un **archivo ZIP** (sin compresión para los blobs ya
comprimidos, escritura determinista) con esta disposición:

```
documento.forge
├── manifest.json          versión de formato, unidades, tolerancias, generador
├── document.cbor          el grafo del documento: entidades, componentes,
│                          árbol de features, grafos de nodos. Todo por ULID.
├── document.json          (opcional) el mismo grafo en JSON legible, para diff
├── refs/
│   ├── head               hash de la versión actual
│   └── history            lista de versiones (hash, timestamp, etiqueta)
└── blobs/
    └── <aa>/<hash>        payloads inmutables direccionados por contenido:
                           B-Rep, mallas, texturas, teselados cacheados
```

La misma disposición, sin comprimir, es la **forma explotada** — un directorio con
esos mismos archivos. Es el formato recomendado para control de versiones: los
blobs son inmutables (git los deduplica bien) y `document.json` produce diffs
legibles.

**El almacén de blobs direccionado por contenido no es un detalle del formato: es la
columna vertebral del sistema.** El mismo mecanismo sirve a cuatro cosas distintas
—undo/redo, versiones del documento, caché de evaluación y deduplicación del almacén
de activos— y esa unificación es lo que hace que los cuatro pilares compartan núcleo
de verdad y no solo de nombre. Ver [ADR-0004](0004-undo-unificado.md).

## Reglas

- **Blobs inmutables, con nombre = hash del contenido** (BLAKE3). Nunca se
  sobreescribe un blob. Escribir el mismo contenido dos veces es un no-op.
- **Escritura atómica**: se escribe a temporal en el mismo volumen y se hace
  `rename`. Un fallo de energía deja el archivo anterior intacto, nunca uno a medias.
- **`manifest.json` primero y sin comprimir**, para poder identificar un archivo
  leyendo sus primeros kilobytes.
- **Los teselados cacheados son opcionales.** Un `.forge` sin ellos abre igual,
  solo más lento. `guardar --sin-cache` produce archivos pequeños para adjuntar.
- **Versión de formato explícita con migraciones dirigidas**: cada bump aporta una
  función de migración con test. Nunca se lee un documento de versión desconocida
  "a ver si va".
- **Documentado en `docs/formato/`** con especificación normativa y un lector de
  referencia mínimo (~300 líneas de Python) que sirve de test de la propia
  especificación. Esto es lo que hace que "formato abierto" signifique algo.

## Alternativas descartadas

- **SQLite como contenedor del documento.** Muy tentador: transaccional,
  escritura incremental, sin riesgo de archivo a medias, consultable. Se descarta
  para el *documento* porque hace inservible el control de versiones (un blob
  binario opaco por documento) y porque la atomicidad ya la da el patrón
  temporal+rename. **Sí se usa para el índice del almacén de activos**, donde las
  consultas —etiquetas, búsqueda, miniaturas, dependencias— son el caso de uso
  central; ese índice es reconstruible en su totalidad a partir de los blobs, así
  que no es fuente de verdad.
- **Formato binario propio monolítico.** Más rápido de abrir, imposible de
  inspeccionar, y cada herramienta externa exige escribir un parser. Contradice el
  requisito de formato abierto.
- **USD como formato nativo.** Se evaluó seriamente. Se descarta: USD no tiene
  modelo para B-Rep exacto ni para árboles de features paramétricos, así que habría
  que meterlo todo en datos personalizados — con lo cual la interoperabilidad
  prometida es ficticia, y a cambio se hereda la complejidad de composición de USD
  (capas, variantes, referencias) en el núcleo. USD se queda donde debe: en la capa
  de exportación.

## Interoperabilidad

| Formato | Dirección | Vía | Límites, dichos en voz alta |
|---|---|---|---|
| **STEP** AP203/AP214/AP242 | leer / escribir | OCCT | B-Rep exacto sin historia paramétrica. Exportar desde dominio poligonal **no se ofrece**: STEP facetado es un antipatrón que decepciona a quien recibe el archivo. |
| **glTF 2.0** | leer / escribir | crate `gltf` | Formato de referencia del pilar runtime. Escritura completa: mallas, PBR, jerarquía, texturas. Sin B-Rep (glTF no lo tiene). |
| **OBJ** | leer / escribir | propio (trivial) | Intercambio rápido. Sin PBR ni jerarquía. |
| **USD** | escribir (v1) | escritor propio de `.usda`/`.usdc` acotado | Solo geometría estática y materiales básicos. Composición (capas, variantes, payloads) **fuera de alcance en v1**. Ver [`03-dependencias.md`](../03-dependencias.md). |
| **FBX** | leer | `ufbx` | Solo si aparece demanda real. No es un requisito. |
