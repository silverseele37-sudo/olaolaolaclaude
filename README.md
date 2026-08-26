# FORGE

Entorno unificado de modelado: CAD paramétrico exacto, edición poligonal tipo DCC,
motor de render en tiempo real y almacén de activos versionado, sobre un único
núcleo de datos.

> **Estado: Fase 1 en curso — núcleo de datos completo.** El núcleo headless
> —matemáticas, almacén de blobs, documento con undo unificado y contenedor
> `.forge`— está implementado, probado y con demo. Falta el visor 3D, que
> necesita GPU. La Fase 0 (arquitectura y decisiones) está cerrada.

## Empezar

```bash
cargo test --workspace              # 39 tests
cargo run -p forge-io --example fase1_nucleo
```

El demo construye una escena, muestra la deduplicación, guarda un `.forge`, lo
recarga en un almacén vacío, comprueba igualdad estructural, deshace y rehace, e
inyecta un fallo de escritura para enseñar que el archivo anterior sobrevive.

El archivo que produce se lee sin FORGE:

```bash
unzip -l target/demo/soporte.forge
python3 docs/formato/lector_referencia.py target/demo/soporte.forge
```

## Estado por crate

| Crate | Qué hace | Estado |
|---|---|---|
| `forge-math` | f64, milímetros, **Z arriba**, tolerancias, deflexión adaptativa | ✅ |
| `forge-store` | blobs por contenido (BLAKE3), dedup, memoria y disco | ✅ |
| `forge-doc` | documento inmutable, transacciones, **undo unificado**, componentes | ✅ |
| `forge-io` | contenedor `.forge`, escritura atómica, migraciones | ✅ |
| `forge-render` | viewport wgpu | pendiente (necesita GPU) |
| `forge-ui` | navegación, selección | pendiente |
| `forge-kernel-api` · `forge-kernel-occt` | puente a OpenCASCADE | Fase 2 |
| `forge-param` · `forge-mesh` · `forge-assets` | los otros tres pilares | Fases 2–5 |

Las fronteras entre crates las hace cumplir `tests/arquitectura.rs`, que falla el
build si alguien añade una arista prohibida — con control positivo, para que no
sea un verificador que siempre dice que sí.

## Qué leer y en qué orden

| Documento | Para qué |
|---|---|
| [`docs/fase-0/00-arquitectura.md`](docs/fase-0/00-arquitectura.md) | Documento maestro: módulos, flujo de datos, formato de archivo, plan de fases. **Empieza aquí.** |
| [`docs/fase-0/01-contratos.md`](docs/fase-0/01-contratos.md) | Las interfaces entre los cuatro pilares. Es la parte que más cuesta cambiar después. |
| [`docs/fase-0/02-alcance-y-recortes.md`](docs/fase-0/02-alcance-y-recortes.md) | Qué del alcance original es irrealizable y qué se recorta para llegar a un producto usable. |
| [`docs/fase-0/03-dependencias.md`](docs/fase-0/03-dependencias.md) | Qué integrar (OpenCASCADE, OpenSubdiv, MaterialX, xatlas…) y qué escribir. |
| [`docs/fase-0/adr/`](docs/fase-0/adr/) | Decisiones de arquitectura, una por archivo, con alternativas descartadas y por qué. |
| [`docs/formato/`](docs/formato/) | Especificación normativa del formato `.forge`, con lector de referencia en Python que está en el suite de tests. |

## La decisión central, en una frase

El B-Rep es la **única fuente de verdad** en el dominio exacto; su teselado es un
**artefacto derivado y cacheado**, nunca editable; y el paso al dominio poligonal
ocurre en un **nodo explícito y unidireccional del árbol de historia** (`ToMesh`)
que conserva un mapa de procedencia cara↔triángulo. Detalle y justificación en
[ADR-0002](docs/fase-0/adr/0002-representacion-dual.md).

## Recorte propuesto para v1

FORGE v1 no es "Fusion + Blender + Unreal + un DAM". Es **un CAD paramétrico cuya
salida son mallas listas para producción**, con un almacén de activos serio detrás.
Escultura, rigging, animación y el runtime independiente quedan fuera de v1; el
razonamiento está en [`02-alcance-y-recortes.md`](docs/fase-0/02-alcance-y-recortes.md).

## Licencia

Sin definir todavía. La elección está condicionada por las dependencias del kernel
(OpenCASCADE es LGPL con excepción); ver
[`03-dependencias.md`](docs/fase-0/03-dependencias.md#5-licencias).
