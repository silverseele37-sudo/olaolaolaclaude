# ADR-0004 — Undo/redo unificado: documento inmutable con compartición estructural

**Estado:** aceptada
**Fecha:** 2026-08-26

## La confusión que hay que evitar primero

**El árbol de historia paramétrico y la pila de undo son cosas distintas.** Es el
error de diseño más común en este tipo de aplicaciones y contamina todo si se
comete.

- El **árbol de features** es *datos*: una descripción del modelo que el usuario
  edita, reordena y suprime. Vive en el documento y se guarda en el archivo.
- El **undo** es *meta*: deshace cambios al documento, incluidos los cambios al
  árbol de features. No se guarda en el archivo (más allá del historial de
  versiones explícito).

Deshacer "añadir un fillet" no es lo mismo que suprimir el nodo fillet: lo primero
devuelve el documento a un estado anterior, lo segundo *es* una edición que a su
vez es deshacible.

## Decisión

**El documento es una estructura de datos persistente (inmutable con compartición
estructural). Una versión del documento es un puntero raíz. Deshacer es mover el
puntero.**

```
DocumentVersion = { root: Arc<DocNode>, parent: Option<VersionId>, label: String }
```

- Las colecciones del documento (entidades, componentes, aristas del grafo) usan
  mapas persistentes (HAMT). Una edición que toca 3 entidades de 100 000 comparte
  el 99,99% de la estructura con la versión anterior.
- Los payloads pesados —B-Rep, mallas, texturas, teselados— **no viven en el árbol
  del documento**: viven en el almacén direccionado por contenido
  ([ADR-0003](0003-formato-de-archivo.md)) y el documento guarda solo su hash. Dos
  versiones que comparten una malla comparten el blob, sin copiar.
- Toda mutación pasa por una `Transaction`: se abre, se acumulan cambios, se cierra
  con una etiqueta legible ("Extrusión 12 mm"). Una transacción = una entrada de
  undo, atravesando los pilares que haga falta.

## Por qué esto resuelve "unificado a través de los cuatro pilares"

Porque **ningún pilar tiene su propio undo**. Un pilar no puede deshacer: solo
produce comandos que el documento aplica. Editar un vértice, cambiar una cota,
recablear un nodo de material y renombrar una etiqueta de un asset son, todos, la
misma operación a nivel de documento: producir una versión nueva. Un `Ctrl+Z`
después de una operación mixta las revierte todas juntas, sin coordinación entre
módulos, porque no hay nada que coordinar.

Es también el motivo por el que el requisito "los pilares se comunican por
interfaces estables, no por estado global" es realizable: el estado *no es* global
mutable, es un valor inmutable que se pasa. Un pilar recibe un snapshot de solo
lectura y devuelve comandos.

## Cachés derivadas

Teselados, BVH de picking, buffers de GPU, miniaturas: **no forman parte del
documento y no participan en el undo**. Se indexan por hash del contenido de
entrada. Deshacer vuelve a un hash anterior, que casi siempre sigue en la caché,
así que el undo tras una operación pesada es instantáneo — un efecto secundario
agradable de haber separado bien las cosas.

## Coste

- Las estructuras persistentes son entre 2× y 5× más lentas que un `Vec` en acceso
  bruto. Irrelevante para el grafo del documento (miles de nodos); inaceptable para
  datos de malla (millones de vértices) — por eso las mallas son blobs opacos
  fuera del árbol, con copia sobre escritura a nivel de blob completo.
- Una operación que toca de verdad una malla de 5 M de vértices genera un blob
  nuevo. Mitigación: las ediciones interactivas (arrastrar un vértice) acumulan en
  un buffer mutable y sellan **una sola** versión al soltar el ratón. Los blobs de
  malla se guardan como base + deltas cuando el delta es pequeño.
- Hay que fijar un presupuesto de memoria para el historial y podar versiones
  antiguas (por número, por bytes, o ambos), sin dejar nunca huérfanos alcanzables.

## Alternativas descartadas

- **Comando + comando inverso.** Barato en memoria, y frágil: cada operación nueva
  exige escribir su inversa correctamente, y un solo inverso mal escrito corrompe
  el documento de forma silenciosa y diferida. Con cuatro pilares y plugins de
  terceros escribiendo comandos, es cuestión de tiempo.
- **Snapshot completo por operación.** Correcto y trivial, pero inviable con
  documentos grandes.
- **Log de operaciones tipo CRDT.** Resolvería además la edición colaborativa. Se
  descarta por ahora: la colaboración está fuera de alcance en v1 y el coste de
  diseño es alto. La decisión actual no la impide en v2 — un documento inmutable
  con versiones enlazadas es un punto de partida razonable.
