# ADR-0002 — Convivencia de B-Rep exacto y malla poligonal

**Estado:** aceptada (bloqueante para toda la Fase 1)
**Fecha:** 2026-08-26
**Decide:** el problema central del proyecto. Todo lo demás depende de esto.

---

## 1. El problema, planteado con precisión

Es tentador enunciarlo como "¿cómo guardo dos geometrías en el mismo archivo?".
Eso es lo fácil. Un `enum { Brep, Mesh }` en el payload de una entidad lo resuelve
en una tarde.

El problema real son **tres** problemas distintos que suelen confundirse:

**P1 — Visualización.** El B-Rep no se puede rasterizar. Hay que teselarlo. Esto
es un problema resuelto: teselado por desviación de cuerda, cacheado, con LOD.
Nadie ha fracasado nunca por esto.

**P2 — Autoría cruzada.** El usuario quiere biselar una arista de un sólido
paramétrico usando las herramientas de malla, o hacer un booleano entre un STEP
importado y una malla esculpida. Aquí "tener las dos representaciones" no basta:
hay que decidir **cuál manda** y **qué le pasa a la otra** cuando la primera cambia.

**P3 — Estabilidad de referencias (topological naming).** Si el usuario selecciona
la cara 47 de un sólido teselado, aplica una operación, y luego edita una cota del
sketch original 15 pasos más arriba, la topología se regenera y "la cara 47" puede
no existir, o existir siendo otra. Este es el problema que ha consumido dos décadas
de FreeCAD y sigue causando fallos en productos comerciales maduros. Cruzar el
dominio exacto↔poligonal **multiplica** este problema, porque ahora hay referencias
que apuntan desde el mundo poligonal a entidades del mundo exacto.

Cualquier estrategia que solo responda a P1 es una no-respuesta.

---

## 2. Las tres opciones sobre la mesa

### Opción A — Representación dual con teselado bajo demanda

Cada objeto mantiene su B-Rep y una malla derivada, regenerada cuando hace falta.

**Consecuencia para el usuario:** ve un solo objeto, siempre. No hay conversión ni
paso de no retorno. La ilusión de unificación es total… hasta que intenta mover un
vértice de la malla. Entonces la aplicación tiene que responder a una pregunta que
la arquitectura no ha respondido: ¿ese movimiento modifica el B-Rep (imposible en
general), se descarta al regenerar (frustrante y silencioso), o convierte el objeto
en malla sin avisar (destructivo y sorpresivo)?

**Veredicto:** resuelve P1, ignora P2 y empeora P3. No es una estrategia, es una
decisión aplazada. Rechazada como estrategia completa; **retenida** como mecanismo
para el caso puramente visual.

### Opción B — Conversión destructiva con punto de no retorno marcado

Existe un comando "Convertir a malla". Al ejecutarlo se pierde el B-Rep. El árbol
de historia marca claramente dónde ocurrió.

**Consecuencia para el usuario:** el modelo es honesto y comprensible — nadie se
sorprende. Pero el árbol paramétrico se vuelve inútil justo cuando más valdría:
si a las tres semanas hay que cambiar un diámetro, hay que rehacer a mano todo el
trabajo poligonal posterior. En la práctica el usuario aprende a no convertir hasta
el final, y con ello los dos pilares nunca se usan juntos. **El producto se
convierte en dos aplicaciones que comparten instalador.**

**Veredicto:** honesta y barata, pero mata el valor de la integración, que es la
única razón de existir del proyecto. Rechazada como estrategia completa;
**retenida** su idea nuclear: la conversión debe ser explícita y visible.

### Opción C — Capas separadas que solo se unifican al renderizar

Dos escenas paralelas, dos conjuntos de herramientas, un solo viewport. La
unificación es puramente visual.

**Consecuencia para el usuario:** cero sorpresas y cero integración. No puede
hacer un booleano entre las capas, ni referenciar geometría exacta desde la malla.
Es el comportamiento por defecto de casi todos los motores de render que aceptan
importar CAD, y es exactamente el fracaso que el usuario describe como "cuatro
mitades".

**Veredicto:** rechazada. Es la opción que resulta de no decidir.

---

## 3. Decisión

> **B-Rep autoritativo, teselado derivado, y una puerta de un solo sentido.**

Cuatro reglas, en orden de importancia:

### R1 — El B-Rep es la única fuente de verdad de su dominio; el teselado es caché

Un teselado **nunca** es un objeto editable. Es un artefacto derivado, indexado por
`hash(brep) + parámetros_de_teselado`, con exactamente el mismo estatus que un
binario compilado: reproducible, desechable, y opcionalmente persistido en el
archivo solo para acelerar la apertura. Ninguna operación de usuario escribe jamás
sobre un teselado.

Esto elimina de raíz la clase de bug más cara de este tipo de aplicaciones: dos
representaciones del mismo objeto que divergen y nadie sabe cuál es la buena.

### R2 — El cruce de dominio es un nodo del grafo, no un comando destructivo

Existe un nodo `ToMesh { fuente, tolerancia_cuerda, tolerancia_angular, ... }`.
Todo lo que está **aguas arriba** sigue siendo paramétrico y editable para siempre.
Todo lo que está **aguas abajo** es poligonal y no puede volver.

La distinción con la Opción B es la clave y no es cosmética: **la conversión es
semánticamente destructiva (se pierde exactitud) pero documentalmente reversible
(borras el nodo y recuperas tu B-Rep)**. El usuario puede cambiar el diámetro del
sketch tres semanas después: el árbol re-evalúa, `ToMesh` produce un teselado nuevo,
y las ediciones poligonales posteriores se re-aplican sobre él.

En la interfaz esto se dibuja explícitamente: el árbol muestra una línea horizontal
—la *frontera de dominio*— con un candado. Arriba, cotas exactas. Abajo, vértices.

### R3 — Las referencias cruzan en un solo sentido, y llevan procedencia

El teselado no es una sopa de triángulos anónima. Cada triángulo conserva el
identificador de la cara B-Rep que lo originó; cada arista de malla que cae sobre
una arista exacta lleva su identificador; cada vértice que coincide con un vértice
exacto, el suyo. Ese **mapa de procedencia** es lo que permite que "selecciona esta
cara del sólido y biséllala con la herramienta de malla" funcione, y que la
selección se re-vincule tras una edición paramétrica aguas arriba.

Las operaciones poligonales **pueden** referenciar entidades derivadas del B-Rep.
Las operaciones paramétricas **no pueden** referenciar geometría de malla. Sin
excepciones en v1.

Esto no es purismo: es lo que mantiene el grafo de evaluación acíclico, con un
orden topológico trivial y una "marca de agua" de dominio que se puede verificar
estáticamente. En el momento en que se permite la referencia inversa aparecen
ciclos, y la evaluación deja de ser un DAG y pasa a ser un punto fijo — con eso se
va la reproducibilidad y la mitad del rendimiento.

### R4 — La dirección malla → B-Rep no existe en v1

No hay "Convertir malla a sólido" salvo un caso trivial y explícitamente marcado
como aproximado: mallas cerradas, manifold, cuyas caras ya son planas y coplanares
por regiones (cajas, piezas facetadas). Todo lo demás —ajuste de superficies,
reconocimiento de features, reingeniería de escaneos— **está fuera de alcance**.

Esto hay que decirlo en la documentación del producto, no esconderlo: *"si esculpes,
no recuperas una pieza mecanizable"*. Es una promesa que sí se puede cumplir. La
alternativa —insinuar que la ida y vuelta funciona— genera exactamente la clase de
decepción que hunde productos, porque el 80% de los casos funciona y el 20% falla
después de que el usuario ya ha invertido tres días.

---

## 4. El mapa de procedencia y el naming persistente

Sin esto, R3 es una promesa vacía. La implementación concreta:

```
StableId {
    origen: FeatureId,      // el nodo del árbol que creó la entidad
    clase:  Face|Edge|Vertex,
    marca:  u64,            // discriminador semántico del propio nodo
}
```

La `marca` la genera el nodo que crea la geometría, con su propia semántica: para
un extrude, "cara lateral generada por la arista E3 del sketch"; para un fillet,
"cara de redondeo de la arista referenciada R7". No es un índice de la lista de
caras que devuelve el kernel — ese índice cambia con cualquier cosa.

Cuando la topología cambia lo bastante como para que un `StableId` no se pueda
resolver, se recurre a una **firma geométrica** (centroide, normal, área,
cuantizados a tolerancia gruesa) para intentar re-vincular. Si tampoco resuelve, la
referencia pasa a estado `Rota` y **se muestra**. Nunca se re-vincula silenciosamente
a la candidata "más parecida": una selección mal re-vinculada produce un modelo
plausible pero incorrecto, que es peor que un error visible.

Sobre esto conviene ser humilde: OpenCASCADE trae `TNaming`/OCAF para el problema,
pero su modelo es incómodo de usar desde fuera y no cubre las referencias cruzadas
al dominio poligonal. La recomendación es un esquema propio de nombrado por
genealogía, apoyado en OCAF donde ayude, y **una batería de tests de regresión
topológica desde el primer día**: cada test edita un parámetro aguas arriba y
verifica que las referencias aguas abajo siguen apuntando a lo correcto. Si esos
tests no existen desde la Fase 2, el problema se descubre en la Fase 4 y ya es
demasiado tarde.

---

## 5. Qué ve el usuario, en concreto

| Quiere hacer | Qué pasa |
|---|---|
| Modelar con cotas, cambiarlas más tarde | Funciona indefinidamente. Es el camino feliz. |
| Ver su pieza CAD con PBR y sombras | Automático. El teselado es transparente. |
| Esculpir / desenvolver UV / riggear una pieza CAD | Cruza la frontera `ToMesh`. La UI lo dice antes, con un diálogo, una vez. |
| Cambiar una cota después de haber cruzado | Funciona. El trabajo poligonal se re-aplica. Lo que no se pueda re-vincular sale marcado en rojo, no se pierde en silencio. |
| Filetear una malla importada de glTF | **No se puede.** Está en dominio poligonal desde el origen. |
| Recuperar un sólido exacto de algo esculpido | **No se puede.** Y se dice desde el principio. |
| Booleano entre sólido paramétrico y malla | Se resuelve en dominio poligonal: el sólido baja por `ToMesh` implícito, con aviso. |
| Importar un STEP y trabajarlo | Entra como B-Rep sin historia (feature raíz "sólido importado"), con caras y aristas seleccionables. Modelado directo sí; historia paramétrica no. |

---

## 6. Coste que aceptamos

Esta decisión no es gratis. Lo que cuesta:

- **El mapa de procedencia hay que mantenerlo a través de cada operación de malla.**
  Un bevel que subdivide una cara tiene que propagar la procedencia a los triángulos
  nuevos. Esto es trabajo real y recurrente en cada modificador que se añade.
- **La re-aplicación de ediciones poligonales tras un cambio aguas arriba no siempre
  converge.** Habrá casos marcados como rotos. Es aceptable si son visibles y
  minoritarios; es un fracaso si son frecuentes. Métrica de aceptación para la
  Fase 3: en el conjunto de modelos de prueba, un cambio de cota típico debe
  re-vincular ≥95% de las referencias.
- **Renunciamos a la ida y vuelta.** Es la funcionalidad más pedida y la menos
  realizable. Prefiero perder esa venta que perder la confianza del usuario.

---

## 7. Alternativas revisadas y descartadas

- **SubD como puente universal** (estilo Rhino/SubD): la superficie límite de
  Catmull–Clark se puede convertir a NURBS con exactitud razonable en regiones
  regulares, lo que daría una vuelta parcial malla→exacto. Es una idea buena y
  **está explícitamente aplazada a v2**: exige un tipo de geometría más (tres
  dominios en lugar de dos), y triplica la superficie del problema de naming antes
  de que el problema con dos dominios esté resuelto.
- **Kernel unificado tipo "todo son celdas"** (representación por complejos
  celulares con geometría intercambiable): elegante en el papel, sin implementación
  industrial que lo respalde, y significaría escribir el kernel desde cero. Ver
  [ADR-0001](0001-lenguaje-del-nucleo.md) sobre por qué eso no se hace.
