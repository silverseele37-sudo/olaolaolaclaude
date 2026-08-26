# ADR-0005 — Render sobre wgpu; MaterialX como modelo, WGSL generado en casa

**Estado:** aceptada
**Fecha:** 2026-08-26

## API gráfica: wgpu

**Decisión: wgpu.**

Da Vulkan, Metal, D3D12 y WebGPU desde una sola base de código. Para un equipo
pequeño, la alternativa —Vulkan directo— significa escribir y mantener a mano
gestión de memoria, sincronización, descriptor sets, swapchain y las rutas
específicas de cada plataforma, además de un backend Metal aparte para macOS
(MoltenVK es una opción, no un regalo). Ese trabajo no diferencia el producto en
absoluto.

Lo que se cede: las capacidades más recientes de GPU (ray tracing, mesh shaders,
bindless agresivo) llegan a wgpu más tarde o tras banderas experimentales; conviene
verificar su estado actual antes de planificar cualquier función que dependa de
ellas. Ninguna es necesaria para un editor con PBR, sombras, IBL y post-proceso, y
son exactamente el tipo de característica vistosa que no hay que construir en v1.

El premio adicional: WebGPU significa que un visor en navegador es un objetivo de
compilación, no un proyecto nuevo. Eso vale mucho para revisión de assets y para el
almacén de activos.

## Materiales: adoptar MaterialX como modelo de datos, generar WGSL propio

Este punto tiene una trampa que conviene ver ahora y no en la Fase 4.

MaterialX resuelve muy bien la parte difícil y aburrida: un modelo de grafo de
materiales bien especificado, una biblioteca estándar de nodos, y un formato de
intercambio que otras herramientas leen. Su generador de código, en cambio, emite
GLSL, OSL, MDL y MSL — **no WGSL** (conviene confirmar el estado actual del
generador antes de fijar el plan). Traducir GLSL a WGSL con la pasarela de `naga`
es una vía posible pero frágil para código generado.

**Decisión:** adoptar MaterialX como **modelo de documento e intercambio** de los
grafos de material (y su biblioteca de definiciones de nodos), y escribir un
**generador de WGSL propio para un subconjunto acotado** de esos nodos: el
suficiente para el shading model de superficie estándar, con permutaciones
cacheadas y compiladas de forma perezosa.

Un subconjunto acotado y bien definido de nodos que funcionan de verdad vale más
que soporte nominal completo con casos rotos. La lista de nodos soportados es parte
del contrato público del producto.

## Arquitectura de render

- **Grafo de render explícito**: pases declarados con sus recursos y dependencias,
  transiciones y alias de memoria resueltos automáticamente. Evita el enredo de
  estado que produce llamar a la API en línea.
- **Un solo camino de render para editor y runtime.** El editor es el runtime más
  gizmos y overlays. Si se bifurcan, divergen, y lo que se ve al editar deja de ser
  lo que se obtiene al exportar.
- **Precisión**: el documento es f64 en milímetros; la GPU es f32. Se renderiza en
  coordenadas relativas a la cámara, con origen local por objeto. Sin esto, una
  escena de más de unos pocos cientos de metros tiembla de forma visible.
- **PBR**: metallic-roughness (compatible con glTF), IBL con prefiltrado de
  specular y armónicos esféricos para la difusa, sombras con cascadas, y post
  mínimo — tonemapping ACES, exposición, FXAA/TAA. Nada más en v1.

## El "runtime independiente"

El requisito original pide exportar escenas como aplicación ejecutable autónoma.
Eso es un producto aparte: empaquetado de assets, mapeo de entrada, bucle de juego,
scripting en tiempo de ejecución, firma y distribución por plataforma.

**Recorte propuesto para v1:** demostrar el *camino de datos*, no el ejecutable.
`forge-runtime` es un binario headless que carga un `.forge` (o un glTF exportado),
renderiza y saca imágenes o una ventana — sin editor, compartiendo el mismo grafo de
render. Con eso se prueba que la escena es portable y que el render no depende del
editor, que es el 80% del valor arquitectónico. El empaquetado por plataforma se
pospone a v2. Razonamiento completo en
[`02-alcance-y-recortes.md`](../02-alcance-y-recortes.md).
