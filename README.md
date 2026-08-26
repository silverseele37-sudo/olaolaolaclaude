# FORGE

Entorno unificado de modelado: CAD paramétrico exacto, edición poligonal tipo DCC,
motor de render en tiempo real y almacén de activos versionado, sobre un único
núcleo de datos.

> **Estado: Fase 0 — diseño.** Todavía no hay código ejecutable. Este repositorio
> contiene, por ahora, el documento de arquitectura y las decisiones que lo
> sustentan. La Fase 1 no empieza hasta que la estrategia de representación dual
> esté cerrada y aceptada.

## Qué leer y en qué orden

| Documento | Para qué |
|---|---|
| [`docs/fase-0/00-arquitectura.md`](docs/fase-0/00-arquitectura.md) | Documento maestro: módulos, flujo de datos, formato de archivo, plan de fases. **Empieza aquí.** |
| [`docs/fase-0/01-contratos.md`](docs/fase-0/01-contratos.md) | Las interfaces entre los cuatro pilares. Es la parte que más cuesta cambiar después. |
| [`docs/fase-0/02-alcance-y-recortes.md`](docs/fase-0/02-alcance-y-recortes.md) | Qué del alcance original es irrealizable y qué se recorta para llegar a un producto usable. |
| [`docs/fase-0/03-dependencias.md`](docs/fase-0/03-dependencias.md) | Qué integrar (OpenCASCADE, OpenSubdiv, MaterialX, xatlas…) y qué escribir. |
| [`docs/fase-0/adr/`](docs/fase-0/adr/) | Decisiones de arquitectura, una por archivo, con alternativas descartadas y por qué. |

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
