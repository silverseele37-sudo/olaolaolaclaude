# Construir y ejecutar FORGE

Guía para pasar del repositorio a algo que corre. Escrita pensando en un portátil
con **gráficos híbridos** (AMD integrada + NVIDIA discreta), que es donde están
las dos trampas que este proyecto ya se ha comido.

---

## 1. Lo mínimo — sin GPU y sin OpenCASCADE

Esto funciona hoy, en cualquier máquina, sin nada instalado más que Rust.

```bash
rustup default stable          # 1.90 o posterior
cargo test --workspace         # el suite completo
cargo run --example cadena_completa
```

El demo recorre la cadena entera —perfil 2D → sólido exacto → chaflán → cruce al
dominio poligonal → subdivisión → espejo → glTF, `.forge` y biblioteca de
activos— y deja los archivos en `target/demo/`.

Lo que produce se puede abrir sin FORGE:

```bash
unzip -l target/demo/soporte.forge
python3 docs/formato/lector_referencia.py target/demo/soporte.forge
# soporte.glb y soporte.obj se abren en Blender, Windows 3D Viewer, etc.
```

**Test de rendimiento del almacén de activos** (tarda, por eso está aparte):

```bash
cargo test -p forge-assets --release -- --ignored --nocapture
```

---

## 2. Con GPU — el visor

### La trampa de los gráficos híbridos

En un portátil con integrada AMD y discreta NVIDIA, **incluir el backend GL
junto a Vulkan y DX12 revienta dentro de `wgpu::Instance::new`**, antes siquiera
de poder enumerar adaptadores. No es un fallo del programa: es la convivencia de
los backends.

FORGE usa `Backends::PRIMARY` por eso, y además porque es lo correcto: sin
Vulkan ni DX12, este programa no tiene nada que hacer.

Si aun así falla al arrancar, comprobá en este orden:

```bash
# ¿ve la GPU el sistema?
vulkaninfo --summary          # Linux
dxdiag                        # Windows
```

- **Drivers recientes.** Una GPU nueva con drivers viejos no expone Vulkan 1.3
  ni DX12 Ultimate, y wgpu no encuentra adaptador.
- **Forzar la discreta.** En Windows: Configuración → Pantalla → Gráficos →
  añadir `forge.exe` → Alto rendimiento. En Linux con PRIME:
  `DRI_PRIME=1 cargo run` o `__NV_PRIME_RENDER_OFFLOAD=1 __GLX_VENDOR_LIBRARY_NAME=nvidia cargo run`.
- **Elegir backend a mano** para aislar el problema:
  ```bash
  WGPU_BACKEND=vulkan cargo run
  WGPU_BACKEND=dx12   cargo run     # Windows
  ```

### La segunda trampa: el formato de la superficie

La ventana usa formato **no-sRGB a propósito**, y el shader aplica su propio
gamma. Con una superficie sRGB, el hardware lo aplicaría otra vez y lo que se ve
en pantalla dejaría de ser idéntico byte a byte a lo que escribe el render sin
ventana. Si divergieran, los tests por imagen de referencia no significarían
nada.

Si alguien "arregla" eso poniendo `Bgra8UnormSrgb`, la imagen sale lavada y los
tests dejan de valer. Está comentado en el código; esto es el recordatorio.

---

## 3. Con OpenCASCADE — el kernel exacto de verdad

FORGE trae **dos implementaciones del mismo kernel**: `forge-kernel-stub`
(analítico, sin C++, ya funciona) y `forge-kernel-occt` (el de verdad). El
workspace compila y pasa los tests **sin OCCT**, así que esto es opcional hasta
que hagan falta STEP, booleanos generales o fillets reales.

Instrucciones detalladas en [`construir-occt.md`](construir-occt.md). Lo esencial:

```bash
git clone --depth 1 --branch V7_9_3 https://github.com/Open-Cascade-SAS/OCCT.git ~/dev/OCCT
```

Configurar **sin el módulo de visualización** —es justo lo que reemplaza wgpu— y
sin dependencias de terceros. Eso baja el build de unos 4000 objetivos a ~1500:

```bash
cmake -S ~/dev/OCCT -B ~/dev/occt-build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_LIBRARY_TYPE=Shared \
  -DBUILD_MODULE_Visualization=OFF -DBUILD_MODULE_Draw=OFF \
  -DBUILD_DOC_Overview=OFF \
  -DUSE_FREETYPE=OFF -DUSE_TK=OFF -DUSE_TCL=OFF -DUSE_TBB=OFF \
  -DUSE_VTK=OFF -DUSE_FREEIMAGE=OFF -DUSE_RAPIDJSON=OFF \
  -DUSE_DRACO=OFF -DUSE_OPENVR=OFF -DUSE_FFMPEG=OFF -DUSE_OPENGL=OFF
```

**Con 16 GB de RAM, limitá los trabajos paralelos.** Compilar C++ pesado a todos
los núcleos a la vez se come la memoria y el sistema empieza a paginar, que es
más lento que compilar con la mitad de hilos:

```bash
cmake --build ~/dev/occt-build -j4
```

Después:

```bash
export OCCT_ROOT=~/dev/occt-build     # el directorio de build ya sirve como raíz
cargo build --workspace
```

`build.rs` **descubre los toolkits del disco** en vez de hardcodearlos: los
nombres cambian entre versiones —la 7.8 fusionó `TKSTEP`, `TKSTEPBase` y otros en
`TKDESTEP`— y una lista fija se rompe en la siguiente.

> **Windows es insensible a mayúsculas.** Un prefijo `C:/dev/occt` colisiona con
> el árbol fuente `C:/dev/OCCT`, y OCCT copia recursos a `<prefix>/src`: acabarían
> mezclados con el código fuente. Usá nombres que no colisionen,
> `C:/dev/occt-build` y `C:/dev/OCCT`.

En Windows hace falta MSVC (Visual Studio con la carga de trabajo de C++), el
toolchain `stable-x86_64-pc-windows-msvc` de Rust, CMake y Ninja. Ninja con MSVC
necesita el entorno de `vcvars64.bat`, así que el build va por `cmd`.

---

## 4. Qué está verificado y qué no

Esta tabla importa más que la guía. **Lo que no se ha podido medir, no se afirma.**

| Parte | Estado |
|---|---|
| Núcleo de datos, undo, `.forge` | Verificado. Ida y vuelta sobre 20 documentos, 10 000 undo/redo contra re-ejecución desde cero, escritura atómica con inyección de fallo. |
| Kernel analítico (`stub`) | Verificado con respuestas conocidas derivadas a mano. |
| Frontera de dominio (`ToMesh`) | Verificado: 100 % de procedencia conservada a través de la pila de modificadores. |
| Interoperabilidad glTF/OBJ | Verificado, incluidas las conversiones de ejes y unidades. |
| Almacén de activos | Verificado, con rendimiento medido sobre 100 000 activos. |
| Reproductor sin editor (`forge-runtime`) | Verificado de punta a punta con el rasterizador por software: escribe un `.forge`, lo relee del disco con un almacén de blobs limpio y lo dibuja. Ver `cargo run -p forge-runtime --example escena_demo`. |
| **Visor con GPU** | **No verificado aquí**: el contenedor de desarrollo no tiene GPU. Compila; que renderice bien está por comprobar en tu máquina. |
| **Puente a OpenCASCADE** | **No verificado**: OCCT no estaba instalado. El andamiaje está; el C++ no se ha ejecutado nunca. |

Si algo de las dos últimas filas falla en tu portátil, no es una sorpresa: es lo
que dice esta tabla.

Una advertencia sobre cómo leerla: «verificado» significa que hay un test con una
respuesta conocida derivada a mano, no que el código parezca correcto. Tres
fallos reales de este proyecto pasaron por delante de tests que existían y
pasaban: un chaflán que daba mal el volumen en 6 de las 12 aristas de un cubo
(los tests solo probaban la arista 0), un booleano que inflaba el área hasta un
72 % (el test solo miraba el volumen, y el volumen sí era correcto), y un
reproductor que abría cualquier documento, dibujaba una imagen vacía y decía que
todo había ido bien (los dos tests usaban un documento vacío). Ninguno daba
error.

---

## 5. El test que vale la pena en cuanto tengas GPU

FORGE tiene **dos renderers que implementan el mismo trait**: uno por software
(`forge-render-cpu`) y uno sobre wgpu (`forge-render`). El de software está
verificado sin GPU; el de wgpu no.

Eso permite una validación cruzada que no requiere confiar en ninguno de los dos
por separado: renderizar **la misma escena con los dos** y comparar las imágenes.

```bash
cargo test --workspace --release -- --ignored --nocapture
```

Si divergen más allá de la tolerancia perceptual, uno de los dos está mal — y
saber cuál es mucho más fácil que descubrir que ambos lo están.
