# ADR-0001 — Lenguaje del núcleo: Rust, con el kernel geométrico aislado tras un puente C++

**Estado:** aceptada
**Fecha:** 2026-08-26

## Contexto

Hay que elegir entre Rust y C++ para el núcleo. La elección no es ideológica: está
determinada por una asimetría concreta del proyecto.

**Toda la geometría exacta seria está en C++.** OpenCASCADE, OpenSubdiv, MaterialX,
OpenUSD, ufbx, xatlas — el ecosistema entero del modelado geométrico. Elegir Rust
significa pagar un peaje de FFI en la parte más difícil del sistema.

**Todo lo demás se beneficia enormemente de Rust.** La capa de aplicación —documento,
undo, serialización, grafo de evaluación, bus de comandos, UI, render— es donde un
equipo pequeño acumula bugs de vida útil larga: punteros colgantes tras una
invalidación de caché, carreras entre el hilo de render y el de documento, corrupción
al deshacer. Rust elimina categorías enteras de esos fallos en tiempo de compilación,
y `wgpu` —la mejor opción de render multiplataforma disponible (ver
[ADR-0005](0005-render-y-materiales.md))— es nativo de Rust.

Además: **un editor que no se cae es una característica de producto**. Los kernels
geométricos lanzan excepciones y a veces abortan ante entradas degeneradas. Aislar
esa inestabilidad es más fácil desde un lenguaje con fronteras explícitas.

## Decisión

**Rust para el núcleo.** OpenCASCADE queda envuelto tras una interfaz estrecha,
diseñada desde el principio para poder moverse fuera de proceso.

Tres reglas que hacen que esto funcione:

1. **No se envuelve OpenCASCADE.** Se envuelve *nuestra* interfaz de kernel: entre
   40 y 80 funciones (`extrude`, `revolve`, `boolean`, `fillet`, `tessellate`,
   `import_step`…), no las miles de clases de OCCT. El puente se escribe con `cxx`
   —tipos comprobados en ambos lados, sin `unsafe` disperso— y vive en un único
   crate, `forge-kernel-occt`. Si mañana hay que cambiar a Parasolid o a un kernel
   propio, se reimplementa un trait, no se reescribe la aplicación.

2. **Toda llamada al kernel es un comando serializable.** Entra una estructura de
   parámetros y referencias, sale un handle de geometría más un teselado. Nada de
   punteros a objetos OCCT cruzando la frontera. Esto es lo que permite mover el
   kernel a otro proceso sin tocar a los llamantes.

3. **v1 ejecuta el kernel en proceso**, en un pool de hilos dedicado (OCCT no es
   seguro en concurrencia de forma uniforme), con las excepciones de C++ capturadas
   y traducidas a `Result` en la frontera `cxx`. Fuera de proceso es la salida de
   emergencia si la estabilidad lo exige, y no requiere rediseño porque la regla 2
   ya lo contempla.

## Consecuencias

**A favor**
- La superficie de código insegura queda confinada a un crate auditable.
- Un único gestor de dependencias y de build para el 90% del proyecto.
- `wgpu`, `egui`, `rayon`, `serde` sin fricción.
- La frontera del kernel obliga a un contrato explícito — que es justamente lo que
  el proyecto necesita priorizar.

**En contra, y hay que asumirlo**
- El puente a OCCT es trabajo real: semanas, no días, y hay que rehacerlo
  parcialmente cada vez que hace falta una función nueva del kernel. Presupuestar
  entre 3 y 5 semanas de un ingeniero solo para llegar a "extrude + boolean + fillet
  + STEP in/out" estables.
- `opencascade-rs` existe y sirve de referencia, pero cubre una fracción pequeña;
  contar con extenderlo, no con adoptarlo tal cual.
- Los bindings de MaterialX, OpenSubdiv y USD tienen el mismo peaje. La respuesta
  a eso está en [`03-dependencias.md`](../03-dependencias.md): en varios casos la
  recomendación es no depender de la librería C++ sino de su *formato*.
- Build multiplataforma mixto Rust+C++: hay que resolver el toolchain de OCCT en
  Windows, macOS y Linux desde la Fase 0. No dejarlo para después; es de las cosas
  que aparentan ser triviales y consumen una semana.

## Alternativas descartadas

- **C++ para todo.** Elimina el peaje de FFI y da acceso directo al ecosistema.
  Se descarta porque el ahorro se concentra en el 10% del código donde el trabajo
  es difícil pero acotado, mientras el coste —bugs de memoria y concurrencia en el
  99% restante, que un equipo pequeño mantiene durante años— se paga a perpetuidad.
  Si el equipo tuviera ya un experto en OCCT y ninguno en Rust, esta decisión
  debería invertirse: la experiencia del equipo pesa más que este análisis.
- **Rust puro con `truck` como kernel.** Elimina C++ por completo. Se descarta:
  `truck` no está cerca de OCCT en booleanos robustos, fillets y —sobre todo—
  importación/exportación STEP. Escribir un kernel B-Rep industrial es un proyecto
  de década, no un módulo. Reevaluable en v3.
- **Zig / C.** Sin masa crítica de librerías ni de contratación para este dominio.
