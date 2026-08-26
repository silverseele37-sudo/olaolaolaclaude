# ADR-0007 — Relación con cadviz: proyectos separados, estructura compartida

**Estado:** aceptada
**Fecha:** 2026-08-26
**Contexto:** existe `cadviz`, un núcleo de visualización CAD del mismo autor, en
estado M4, con puente a OpenCASCADE funcionando y medido.

---

## Decisión

**FORGE y cadviz son proyectos separados, sin acoplamiento de código.** FORGE toma
de cadviz *estructura* —patrones, decisiones y hallazgos medidos— pero no lo
consume como dependencia ni comparte su árbol de fuentes.

cadviz sigue siendo lo que es y es bueno: un visor estrecho, verificado, con una
identidad deliberada («no es una interfaz de autoría»). FORGE es la versión
multidisciplinaria: autoría, historia paramétrica, dominio poligonal, activos.

---

## Por qué esto tiene un coste, y cuál es

Hay que decirlo antes de que aparezca solo: **se mantienen dos puentes a OCCT y
dos renderers.** El bug del bounding box —`BRepBndLib::Add` usando la
triangulación por defecto, lo que hace saltar la cámara en cada re-teselado— se
descubrió y corrigió en cadviz. Con esta decisión, no llega a FORGE por sí solo.

Mitigación, y es barata pero no gratis:

- **Un registro de hallazgos por escrito**, no un `git subtree`. Cada hallazgo
  medido de un proyecto se anota en el otro con su número y su test. Es lo que
  esta ADR y la sección de tolerancias del documento maestro empiezan a hacer.
- **Firmas compatibles donde la ABI se solapa.** No es un requisito, es una
  cortesía barata: si algún día se decide unificar, no habrá que reescribir.
  Concretamente: por la frontera cruzan triángulos, polilíneas e IDs, y nada más,
  en ambos proyectos.

---

## Qué toma FORGE, en concreto

### Patrones de arquitectura

1. **La regla de oro de la frontera.** «Por la frontera cruzan triángulos,
   polilíneas e IDs. Nada más. Ningún tipo de OCCT aparece del lado de Rust.»
   Es la regla 2 de [ADR-0001](0001-lenguaje-del-nucleo.md), ahora con evidencia:
   en cadviz eso son ~330 líneas de C++ para 16 funciones.

2. **Dos implementaciones intercambiables de la misma ABI.** cadviz tiene
   `shim_occt.cpp` y `shim_stub.cpp` (geometría procedural). FORGE adopta esto
   **desde el primer commit**, y es más valioso aquí que allá: permite desarrollar
   y testear `forge-doc`, `forge-store`, `forge-mesh` y `forge-render` sin OCCT
   compilado, y deja el CI sin un build de 20 minutos en el camino crítico.
   Además es la prueba de que la costura del kernel es real y no decorativa.

3. **El hilo dueño de la forma.** `Shape` es `!Send`; en vez de mentir con un
   `unsafe impl Send`, un hilo es dueño y nadie más la toca, con *coalescing* de
   peticiones y un umbral de re-teselado de 1,6×. **FORGE debe generalizarlo, no
   copiarlo tal cual** — ver «Dónde hay que divergir».

4. **Render offscreen determinista como primitiva, no como función.** En cadviz
   `--png` existe desde M1 y la superficie de ventana es no-sRGB *a propósito*,
   para que lo que se ve sea idéntico byte a byte a lo que se escribe. Sin esa
   propiedad, un test por imagen no significa nada. FORGE lo necesita desde la
   Fase 1: es lo que hace barato el criterio de aceptación de la Fase 4 («editor
   y runtime producen píxeles idénticos») y lo que abarata `forge-runtime`.

5. **Descubrir los toolkits de OCCT del disco**, no hardcodearlos: los nombres
   cambian entre versiones (7.8 fusionó `TKSTEP`/`TKSTEPBase` en `TKDESTEP`).
   Y compilar OCCT sin el módulo de visualización — es justo lo que reemplaza wgpu.

### Disciplina de verificación

Es lo más valioso de cadviz y lo más fácil de perder. Cada capacidad llegó con un
test de respuesta conocida: `π·L` para la irradiancia, `4π` para la integral
esférica, `1.0000` para el horno blanco, `0%`/`100%` para orientación,
`delta 39`/`delta 0` para el picking. Tres bugs los encontró la medición, no la
vista.

Dos prácticas concretas se adoptan como norma en FORGE:

- **Control negativo obligatorio.** El test de picking no prueba nada leyendo el
  ID: prueba seleccionando la cara devuelta, re-renderizando, verificando que ese
  píxel cambió, **y verificando que seleccionar otra cara no lo cambia**. Sin la
  segunda mitad, un resaltado global pasaría el test.
- **Constantes medidas, no sintonizadas a ojo.** En cadviz la exposición es
  `π / irradiancia_media` y la profundidad de sombra sale de integrar el entorno
  dos veces. La consecuencia: cambiar las luces no invalida en silencio tres
  números repartidos por el código.

### Hallazgos numéricos

Incorporados a [`00-arquitectura.md §4`](../00-arquitectura.md#4-unidades-precisión-y-tolerancias)
y a [ADR-0002](0002-representacion-dual.md): factor 1,75 de la deflexión de OCCT,
cuantización de `f32`, `BRepBndLib` sin triangulación, `Backends::PRIMARY`,
`xstep.cascade.unit`, y la fórmula de deflexión por píxel.

### La convención de verificación en la documentación

De la referencia técnica de M2 de `cadviz`: lo no marcado está verificado contra
la fuente primaria; lo marcado **⚠ NO VERIFICADO** no se pudo confirmar y no debe
copiarse sin chequear. FORGE la adopta para todo documento con constantes,
matrices o fórmulas de terceros. Un documento que dice *dónde* gastar la
desconfianza vale varias veces uno que no lo hace.

Dos prácticas asociadas, también adoptadas:

- **Acotar el error por los dos lados.** El test del polinomio de AgX en `cadviz`
  no comprueba `err < 0.007` sino `0.004 < err < 0.007`. Un límite de un solo
  lado detecta que alguien empeoró el valor; uno de dos lados detecta además que
  alguien lo «mejoró» y descalibró en silencio lo que venía después. La norma
  general: **afirmar el valor que se tiene, no solo el que se tolera.**
- **Corregir dentro del documento, con la razón.** La referencia de M2 lleva tres
  correcciones fechadas y explicadas en el sitio donde estaba el error, no en un
  changelog aparte. Eso hace que el lector siguiente aprenda del fallo en vez de
  repetirlo.

---

## Dónde hay que divergir

Copiar cadviz sin pensar rompe FORGE en cuatro sitios:

1. **La ABI de cadviz es de solo lectura.** Sus 16 funciones cubren cargar,
   teselar, aristas, bbox, propiedades y picking: el lado *consumidor* del kernel.
   FORGE necesita además el lado *constructor* —extrude, revolve, sweep, loft,
   fillet, chamfer, shell, boolean, escritura STEP—, que es donde vive la
   dificultad. La estimación de 40–80 funciones de ADR-0001 sigue vigente: las 16
   de cadviz son aproximadamente la mitad barata.

2. **Un hilo dueño de *una* forma no escala a un grafo de features.** En un visor
   hay un modelo; en FORGE hay un DAG con decenas de nodos que deben evaluarse en
   paralelo. El patrón correcto es un **pool de actores de kernel con propiedad por
   forma**, no un hilo único. Copiar literalmente el modelo de cadviz serializaría
   toda la evaluación de FORGE — sería un error de rendimiento estructural y
   difícil de revertir después.

3. **La filosofía de interfaz no se traslada.** cadviz elige deliberadamente la
   clase de referencia de los visores —eDrawings, 3D PDF, KeyShot—, con el viewport
   como aplicación y ~15 comandos. FORGE es una herramienta de autoría con
   cientos de operaciones, árbol de features y estados modales. La densidad de
   paneles y los idiomas de navegación (ViewCube, `F` para encuadrar) sí se
   heredan; la forma general de la interfaz, no.

4. **La frontera de crates de `cadviz` tiene una fuga que FORGE no puede
   permitirse.** En su `Cargo.lock`, el crate `render` depende de `kernel`, que
   depende de `occt-sys`, que depende de `cc`. Funcionalmente es inocuo —solo
   cruzan tipos de datos planos—, pero significa que el renderer **no se puede
   compilar ni testear sin la cadena de C++ en el grafo de build**. En un visor es
   un detalle; en FORGE rompe la regla de que ningún pilar depende de otro. La
   solución es la que ya está en el diagrama: un crate de contratos con los tipos
   de datos (`MeshData`, `EdgeData`, `StableId`) del que dependan ambos, y cero
   aristas entre `forge-render` y `forge-kernel-*`.

5. **cadviz no tiene núcleo de documento.** Sin documento inmutable, sin
   transacciones, sin undo, sin almacén direccionado por contenido, sin dominio
   poligonal, sin frontera `ToMesh`. Eso es la Fase 1 y la Fase 3 de FORGE
   completas, y no hay nada que tomar prestado.

---

## Validación cruzada

Vale la pena registrarlo porque es el argumento más fuerte a favor de la
arquitectura: cadviz llegó de forma independiente a la regla R1 de
[ADR-0002](0002-representacion-dual.md).

> «el B-rep es la fuente de verdad, el teselado es una *vista* descartable y
> adaptativa, y el renderer nunca ve B-rep»

Dos análisis independientes, la misma decisión. Y cadviz aporta la parte que
faltaba: la deflexión no como constante sino derivada del tamaño de un píxel en el
mundo, que convierte «el teselado es adaptativo» en un número verificable.
