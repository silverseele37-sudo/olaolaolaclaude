# Formato `.forge` — especificación v1

**Estado:** normativo para `format_version: 1`.
Implementación de referencia: [`lector_referencia.py`](lector_referencia.py) —
solo biblioteca estándar de Python, ninguna dependencia de FORGE.

Ese lector no es documentación de cortesía: **está en el suite de tests**
(`crates/forge-io/tests/formato_abierto.rs`). Si el formato cambia sin
actualizar la especificación, o si la especificación es incorrecta, el build
falla. Un formato abierto que nadie fuera del proyecto puede leer no es abierto.

---

## 1. Contenedor

Un documento `.forge` es un **archivo ZIP**.

```
documento.forge
├── manifest.json          obligatorio · primero · sin comprimir
├── document.cbor          obligatorio · el grafo del documento
├── document.json          opcional    · el mismo grafo, legible
├── refs/history           opcional    · etiquetas del historial
└── blobs/<aa>/<hash>      opcional    · payloads inmutables
```

La misma disposición **sin comprimir** es la *forma explotada*: un directorio
con esos mismos archivos. Es la recomendada para control de versiones, porque
los blobs son inmutables (git los deduplica bien) y `document.json` produce
diffs legibles.

### Reglas del contenedor

- `manifest.json` es la **primera** entrada y va con método `Stored`. Así se
  puede identificar un archivo leyendo sus primeros kilobytes sin descomprimir.
- Los blobs van con método `Stored`: ya suelen estar comprimidos y volver a
  comprimirlos solo gasta tiempo.
- La escritura es **atómica**: temporal en el mismo directorio y `rename`. Un
  fallo a mitad deja el archivo anterior intacto y no deja temporales.

---

## 2. `manifest.json`

```json
{
  "format": "forge",
  "format_version": 1,
  "units": "mm",
  "up_axis": "Z",
  "tolerance_confusion_mm": 1e-7,
  "generator": "forge 0.1.0"
}
```

| Campo | Obligatorio | Significado |
|---|---|---|
| `format` | sí | Siempre `"forge"`. Otro valor: no es un documento FORGE. |
| `format_version` | sí | Entero. Un lector **debe rechazar** una versión mayor que la suya, con un mensaje que diga qué hacer. Nunca leer "a ver si va". |
| `units` | sí | Unidad interna. `"mm"` en v1. |
| `up_axis` | sí | Eje vertical del documento. `"Z"` en v1 (convención CAD/STEP). Está en el archivo para que ningún lector tenga que suponerlo. |
| `tolerance_confusion_mm` | sí | Distancia por debajo de la cual dos puntos son el mismo punto. |
| `generator` | sí | Informativo. |

---

## 3. `document.cbor`

CBOR (RFC 8949). Estructura:

```
{
  "entities": [ EntityId, ... ],          // ordenado ascendente
  "stores":   [ { "name": str,
                  "data": bytes }, ... ]  // ordenado por name
}
```

- **`EntityId`** se codifica como el ULID en su representación de cadena
  canónica (26 caracteres Crockford base32).
- **`data`** es a su vez CBOR: un array de pares `[EntityId, valor]`, **ordenado
  ascendente por EntityId**.

Ese orden no es un detalle estético: es lo que hace que el archivo sea
reproducible byte a byte para el mismo contenido, y de ahí salen los diffs
pequeños y la comparación de documentos por huella.

### Componentes de v1

| `name` | Forma del valor |
|---|---|
| `forge.name` | `str` |
| `forge.transform` | `{ "translation": [f64;3], "rotation": [f64;4], "scale": [f64;3] }` — cuaternión en orden `x,y,z,w` |
| `forge.visible` | `bool` |
| `forge.parent` | `EntityId` |
| `forge.geometry` | variante etiquetada: `{"Brep": hash}`, `{"Mesh": hash}`, `{"Sketch": hash}`, `{"Curve": hash}`, `{"PointCloud": hash}` |

`hash` es el hexadecimal en minúsculas de 64 caracteres de un BLAKE3.

**Un componente desconocido es un error, no un campo a ignorar.** Ignorarlo
haría que el guardado siguiente perdiera datos del usuario en silencio.

---

## 4. `blobs/`

Ruta: `blobs/<dos primeros dígitos hex del hash>/<hash hex completo>`.

- El nombre **es** el hash BLAKE3 del contenido. Un lector debe verificarlo y
  rechazar el archivo si no coincide.
- El archivo empaqueta **exactamente** los blobs que el documento referencia: ni
  menos (faltaría geometría) ni más (arrastraría descartes del almacén de
  sesión).
- Un `.forge` guardado sin blobs es válido y pequeño, pero solo abre donde el
  almacén ya los tiene.

---

## 5. Lo que el formato **no** guarda

- **La pila de undo.** El undo es *meta*, no datos del documento (ADR-0004). Lo
  que persiste son las etiquetas del historial, para poder mostrarlas.
- **Los teselados cacheados.** Son derivados y reproducibles (ADR-0002, R1). La
  v1 no los persiste; una versión futura podrá hacerlo como caché opcional,
  indexada por `hash(brep) + parámetros`, sin que eso los convierta en datos.

---

## 6. Compatibilidad

- Un lector **debe** rechazar `format_version` mayor que la suya.
- Cada incremento de `format_version` trae una función de migración dirigida y
  su test. No hay migraciones "por si acaso" ni lectura tolerante.
- Añadir un componente nuevo **no** incrementa la versión: los componentes se
  identifican por nombre y el registro dice cuáles conoce esta build.
- Cambiar el `name` de un componente existente **sí** rompe documentos y exige
  migración.
