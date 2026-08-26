# ADR-0006 — Módulos, plugins y scripting

**Estado:** aceptada
**Fecha:** 2026-08-26

## Escepticismo sobre "plugins desde el día uno"

El requisito dice: cada pilar debe ser un módulo desacoplado que se comunique por
interfaces estables, no por acceso directo al estado global. **Estoy de acuerdo con
la intención y en desacuerdo con la implementación que se suele inferir de ella.**

Hay que separar dos cosas que la palabra "plugin" mezcla:

1. **Fronteras de módulo** — que los pilares no se toquen entre sí ni compartan
   estado mutable. **Esto sí, desde el día uno, y es innegociable.** Cuesta poco al
   principio y es irrecuperable después.
2. **Carga dinámica de código de terceros** — ABI estable, versionado, sandbox,
   compatibilidad hacia atrás. **Esto no, en v1.** Congela interfaces antes de saber
   si son las correctas, y un equipo pequeño acaba manteniendo compatibilidad con
   una API que todavía está equivocada.

Rust, además, no tiene ABI estable, así que "cargar un .dll de plugin" implica o
bien envoltorios `abi_stable`/C sobre toda la superficie de API, o bien exigir que
todos compilen con la versión exacta del compilador. Ninguna es buena a estas
alturas.

## Decisión

**Fronteras estrictas ya; carga dinámica después.**

- Cada pilar es un crate independiente que **no depende de los otros tres**. Solo
  dependen de `forge-core` (documento, comandos, eventos) y de crates de contratos
  (`forge-kernel-api`, `forge-render-api`). Esto lo verifica CI con un test de grafo
  de dependencias que falla el build si alguien añade una arista prohibida — la
  regla se hace cumplir mecánicamente, no por buena voluntad.
- La comunicación entre pilares es por **comandos tipados y consultas de solo
  lectura sobre un snapshot** del documento. Ninguna referencia mutable compartida.
- La API de extensión pública en v1 es el **bus de comandos**, no las estructuras
  internas. Es la misma superficie que usarán después los plugins nativos; si
  resulta insuficiente para escribir herramientas con ella, también lo habría sido
  para plugins — y lo descubrimos antes de haber prometido compatibilidad.
- **v2: plugins como componentes WebAssembly** (interfaz descrita en WIT, ejecutada
  con Wasmtime). Es la única vía razonable de ABI estable y aislada para Rust hoy.
  Limitación conocida y aceptada: los plugins WASM orquestan comandos y hacen
  cálculo acotado, no acceden a la GPU ni mueven mallas de millones de vértices con
  buen rendimiento.

## Scripting: Lua embebido, Python como cliente externo

**Decisión: `mlua` (LuaJIT) dentro de la aplicación; Python fuera, hablando el mismo
protocolo de comandos.**

Rationale:

- **Lua embebido** cuesta ~200 KB, arranca en microsegundos, se puede aislar por
  script (límites de instrucciones y de memoria), y se recarga en caliente. Es el
  lenguaje adecuado para herramientas interactivas, macros y generadores de
  geometría dentro del bucle del editor.
- **Python** es lo que los usuarios de CAD esperan y donde vive el ecosistema
  (NumPy, pandas, `usd-core`, notebooks). Pero embeber CPython trae el GIL, el
  empaquetado de dependencias nativas y los conflictos de versión al interior de la
  aplicación — precisamente los problemas que hacen frágiles a FreeCAD y Blender
  como plataformas.
- La salida: **el protocolo de comandos es la API**. Lua lo llama en proceso, Python
  lo llama por IPC (socket local, comandos en CBOR). Un mismo script conceptual
  funciona en ambos. Como efecto secundario, se obtiene modo batch, automatización
  desde CI y un banco de pruebas: si algo no se puede hacer por el bus de comandos,
  es un hueco de la API, y se ve enseguida.

## Consecuencias

- Un usuario que quiera un generador de engranajes en Python lo ejecuta contra el
  editor abierto, no dentro de él. Es un cambio de expectativa que hay que
  documentar; a cambio, un script que entra en bucle infinito no cuelga el editor.
- El protocolo de comandos se convierte en la interfaz más importante del proyecto y
  hay que diseñarlo con ese peso. Está en
  [`01-contratos.md`](../01-contratos.md).
- Si el proyecto se equivoca aquí, se equivoca barato: el bus se puede versionar
  desde dentro. Congelar un ABI nativo en v1, no.
