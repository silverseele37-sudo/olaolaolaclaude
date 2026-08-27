# FORGE

Entorno unificado de modelado: CAD paramétrico exacto, edición poligonal tipo DCC,
motor de render en tiempo real y almacén de activos versionado, sobre un único
núcleo de datos.

> **Estado: cuatro pilares en pie, sin GPU todavía.** El núcleo de datos, el
> kernel geométrico, la frontera de dominio, la interoperabilidad y el almacén de
> activos están implementados y probados. El visor con GPU y el puente a
> OpenCASCADE están escritos pero **no verificados**: el contenedor de desarrollo
> no tiene ni GPU ni OCCT. Ver [`docs/construir.md`](docs/construir.md) §4.

## Empezar

```bash
cargo test --workspace
cargo run --example cadena_completa
```

El demo recorre la cadena entera y es la mejor forma de ver qué hace el proyecto:

```
perfil en L → extrude → chaflán           dominio EXACTO
        ↓ ToMesh                          la puerta de un solo sentido
subdividir ×2 → espejo → triangular       dominio DISCRETO
        ↓
glTF 2.0 · OBJ · documento .forge · biblioteca de activos
```

Y el pilar de render, de punta a punta y sin GPU:

```bash
cargo run -p forge-runtime --example escena_demo
cargo run -p forge-runtime -- /tmp/forge-demo/escena.forge --ppm salida.ppm --size 800x600
```

Escribe un `.forge` con tres cajas, lo vuelve a leer del disco con un almacén de
blobs limpio —el mismo camino que recorrería otra máquina— y lo dibuja con el
rasterizador por software.

Lo que demuestra no es que cada pieza funcione —para eso están los tests— sino
que **la identidad sobrevive el viaje**: la cara del chaflán que se selecciona en
el sólido exacto sigue siendo localizable después de cruzar al dominio poligonal,
subdividir dos veces, espejar y triangular.

Lo que produce se lee sin FORGE:

```bash
unzip -l target/demo/soporte.forge
python3 docs/formato/lector_referencia.py target/demo/soporte.forge
```

Guía completa, incluidas las trampas de los gráficos híbridos y cómo compilar
OpenCASCADE: [`docs/construir.md`](docs/construir.md).

## Estado por crate

| Crate | Qué hace | Estado |
|---|---|---|
| `forge-math` | f64, milímetros, **Z arriba**, tolerancias, deflexión adaptativa | ✅ |
| `forge-store` | blobs por contenido (BLAKE3), dedup, memoria y disco | ✅ |
| `forge-doc` | documento inmutable, transacciones, **undo unificado**, componentes | ✅ |
| `forge-io` | contenedor `.forge`, escritura atómica, migraciones | ✅ |
| `forge-kernel-api` | contrato del kernel: teselado con procedencia, `StableId` | ✅ |
| `forge-kernel-stub` | kernel analítico sin C++ — la otra mitad de la ABI doble | ✅ |
| `forge-mesh` | **la frontera de dominio**, `ToMesh`, modificadores | ✅ |
| `forge-interop` | glTF 2.0, OBJ, y la conversión de ejes acotada | ✅ |
| `forge-assets` | almacén versionado, índice SQLite reconstruible | ✅ |
| `forge-param` | árbol de features, nombrado persistente, solver 2D | en pruebas |
| `forge-script` | bus de comandos y host de Lua | en pruebas |
| `forge-render-cpu` | rasterizador por software — la referencia sin GPU | en pruebas |
| `forge-escena` | `Snapshot` → `DrawInstance`: la regla de qué se dibuja, una sola vez | ✅ |
| `forge-runtime` | reproductor sin editor: lee un `.forge` del disco y lo dibuja | ✅ |
| `forge-render` · `forge-ui` · `forge-kernel-occt` | wgpu, visor, OpenCASCADE | **sin verificar** |

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
