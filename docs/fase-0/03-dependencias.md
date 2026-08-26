# Dependencias: qué integrar y qué escribir

Regla general: **escribir solo aquello que diferencia el producto.** Un booleano
B-Rep robusto no diferencia — o funciona o el producto no existe. La frontera de
dominio con procedencia sí diferencia, y no la vende nadie.

Todas las afirmaciones sobre licencias y capacidades de terceros son de partida y
deben verificarse contra el estado actual de cada proyecto antes de comprometerse.

---

## 1. Integrar sin dudarlo

### OpenCASCADE (OCCT) — kernel B-Rep
**Escribir esto desde cero es un proyecto de década.** Booleanos robustos con
tolerancias, fillets que cierran en esquinas de tres aristas, y sobre todo un
lector/escritor STEP que aguante los archivos raros que manda el mundo real: es
trabajo de treinta años que se obtiene gratis.

- Se envuelve **nuestra** interfaz de 40–80 funciones, no la API de OCCT
  ([ADR-0001](adr/0001-lenguaje-del-nucleo.md)).
- `opencascade-rs` sirve de punto de partida; cubre una fracción y habrá que
  extenderlo.
- Licencia: LGPL 2.1 con excepción adicional. **Requiere revisión legal** contra
  el modelo de distribución previsto; condiciona si se enlaza dinámicamente.
- Riesgo asumido: se hereda su comportamiento con tolerancias y sus límites de
  escala (ver [`00-arquitectura.md §4`](00-arquitectura.md#4-unidades-precisión-y-tolerancias)).

### PlaneGCS — solver de restricciones 2D
El solver de FreeCAD, extraíble. Escribir uno propio es viable en lo básico
(Newton / Levenberg-Marquardt sobre los residuos de las restricciones), pero la
parte difícil no es converger: es **diagnosticar** sub y sobre-restricción,
identificar el conjunto de restricciones en conflicto y elegir configuraciones
testigo que no salten a otra rama de la solución. Eso es donde se van los meses.

- Licencia LGPL, compatible con la elección de OCCT.
- **Evitar `libslvs` de SolveSpace: es GPL** y contaminaría el proyecto entero.
  Es una trampa fácil de pisar porque técnicamente es una opción excelente.

### xatlas — desenvolvimiento UV automático
Genera cartas y las empaqueta. MIT, C++, autocontenido. Escribir un unwrapper
—LSCM/ABF más packing— es una tesis. Integrar es un día.

### ufbx — lectura de FBX
Si hace falta FBX: C, MIT, un solo archivo, sin dependencias. **Preferible a
Assimp** para este caso concreto (ver abajo).

### BLAKE3, SQLite, wgpu, egui, rayon, serde
Infraestructura estándar. Sin discusión.

---

## 2. Integrar con matices

### MaterialX — modelo de material, **no** generador de shaders
Resuelve bien la parte aburrida: modelo de grafo especificado, biblioteca estándar
de nodos, formato de intercambio que otras herramientas leen.

**La trampa:** su generador de código emite GLSL, OSL, MDL y MSL — hasta donde
alcanza este análisis, **no WGSL**. Verificar el estado actual antes de planificar.
Traducir GLSL→WGSL con la pasarela de `naga` es posible pero frágil para código
generado.

**Decisión:** adoptar MaterialX como modelo de documento e intercambio; escribir un
generador de WGSL propio para un subconjunto acotado y públicamente documentado de
nodos. Ver [ADR-0005](adr/0005-render-y-materiales.md).

### OpenSubdiv — subdivisión
Es el estándar, hace superficies límite, creases semi-agudas y evaluación en GPU.
Pero es una dependencia C++ importante y **Catmull-Clark básica con creases es de
las pocas cosas de este proyecto que sí son razonables de escribir** (cientos de
líneas, bien documentada, fácil de testear).

**Decisión:** implementación propia en CPU en v1; integrar OpenSubdiv cuando el
rendimiento o las superficies límite lo exijan de verdad. Diseñar el modificador de
subdivisión tras una interfaz que permita sustituir el motor sin tocar a los
llamantes.

### OpenUSD — solo si el mercado lo exige
Dependencia C++ enorme, sin implementación Rust completa. La composición de USD es
un modelo conceptual del tamaño de un pilar.

**Decisión v1:** escritor propio de `.usda`/`.usdc` acotado a geometría estática y
materiales básicos; alternativamente, un puente fuera de proceso vía `usd-core` en
Python, que encaja con la arquitectura de scripting ya decidida
([ADR-0006](adr/0006-plugins-y-scripting.md)). USD completo se reevalúa en v2 según
demanda real de estudios.

---

## 3. No integrar

### Assimp
Soporta muchísimos formatos, y ese es el problema: se paga una dependencia grande,
un modelo de escena intermedio que hay que traducir igualmente, y calidad desigual
por formato. Para los formatos que importan hay opciones mejores y más pequeñas:
crate `gltf` para glTF, un lector propio trivial para OBJ, `ufbx` para FBX.
**Integrar Assimp solo si aparece demanda real de la cola larga de formatos.**

### Un motor de física / ECS de terceros
No hace falta física en v1. Para consultas espaciales —picking, BVH, proximidad—
basta `parry3d`, que es acotado. Un ECS completo de terceros (`bevy_ecs`, `hecs`)
impone un modelo de datos mutable que choca con el documento inmutable de
[ADR-0004](adr/0004-undo-unificado.md): el almacén entidad-componente de
`forge-doc` es a medida, y es poco código.

### Un kernel B-Rep alternativo en Rust (`truck`)
Prometedor y muy lejos de OCCT en booleanos, fillets e importación STEP. Se
reevalúa en v3. La interfaz `GeometryKernel` deja la puerta abierta.

---

## 4. Escribir en casa

Lo que sí diferencia el producto y no se compra:

1. **La frontera de dominio con procedencia** — el nodo `ToMesh`, el mapa
   cara↔triángulo y la re-vinculación. Es el corazón del proyecto.
2. **El naming persistente por genealogía** — OCAF/`TNaming` existe y no cubre las
   referencias cruzadas al dominio poligonal.
3. **El documento inmutable, las transacciones y el undo unificado** — es lo que
   hace que los cuatro pilares compartan núcleo de verdad.
4. **El almacén direccionado por contenido** — pocas líneas, y sirve a la vez a
   undo, versiones, caché y deduplicación de assets.
5. **El grafo de render y la extracción de escena** — a medida, y compartido entre
   editor y runtime.
6. **El generador de WGSL** para el subconjunto de MaterialX.
7. **El formato `.forge`** y su lector de referencia.

---

## 5. Licencias

Auditoría necesaria **antes** de escribir código, no después:

| Dependencia | Licencia (verificar) | Nota |
|---|---|---|
| OpenCASCADE | LGPL 2.1 + excepción | Condiciona enlazado y distribución. Revisión legal. |
| PlaneGCS (FreeCAD) | LGPL 2 o posterior | Compatible con la anterior. |
| **libslvs (SolveSpace)** | **GPL 3** | **Evitar.** Contaminaría todo el proyecto. |
| OpenSubdiv | Apache 2.0 modificada (Pixar) | Permisiva. |
| MaterialX | Apache 2.0 | Permisiva. |
| xatlas | MIT | Permisiva. |
| ufbx | MIT | Permisiva. |
| wgpu, egui, serde, rayon | MIT / Apache 2.0 | Permisivas. |
| SQLite | Dominio público | Sin restricciones. |

El riesgo R6 —descubrir una incompatibilidad de licencia tarde— es de los pocos que
se eliminan por completo con una tarde de trabajo al principio.
