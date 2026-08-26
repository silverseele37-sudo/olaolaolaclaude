# Alcance: qué es irrealizable y qué recortaría

Me pediste escepticismo con tu propio pedido. Aquí está, sin suavizar.

---

## 1. La magnitud del alcance original

Los cuatro pilares que describes existen. Cada uno es un producto maduro con
décadas detrás:

| Pilar pedido | Referente | Orden de magnitud del esfuerzo histórico |
|---|---|---|
| CAD paramétrico con kernel B-Rep | FreeCAD (2002–), Fusion 360, Onshape | Décadas · decenas de ingenieros · el kernel (Parasolid, ACIS, OCCT) es a su vez un producto de 30 años |
| Modelado poligonal DCC completo | Blender (1994–), Modo, Maya | Décadas · decenas de ingenieros |
| Motor de render con runtime exportable | Godot, Unity, Unreal | Décadas · decenas a cientos |
| DAM versionado | Perforce Helix, ftrack, Kitsu | Años · equipos dedicados |

Un equipo pequeño —digamos 2 a 5 personas durante 12 a 18 meses— dispone de entre
3 y 8 años-ingeniero. Frente a un alcance cuyos referentes suman varios cientos.

**No es una cuestión de trabajar más deprisa o de que las herramientas modernas
ayuden.** El problema es que estos sistemas no son grandes por acumulación de
funcionalidades sino por acumulación de **casos límite resueltos**: el booleano que
falla con caras tangentes, el fillet que no cierra en una esquina de tres aristas,
el solver que oscila con una restricción redundante. Esos casos no se pueden
saltar, no se pueden diseñar por adelantado y no se aceleran con IA ni con
herramientas mejores. Se descubren uno a uno, con usuarios reales, durante años.

Tu propia frase —"prefiero un núcleo excelente a cuatro mitades"— es exactamente
el criterio correcto. Lo que sigue es aplicarlo.

---

## 2. Lo que recortaría, por orden de contundencia

### Recorte 1 — Escultura. **Fuera.**

Un sistema de escultura utilizable necesita multiresolución o topología dinámica,
un motor de brochas con caída y presión, stroke smoothing, remallado, máscaras y
un pipeline de rendimiento que sostenga millones de polígonos a 60 fps. ZBrush
lleva 25 años en esto y sigue siendo su producto entero.

Una escultura mediocre no aporta nada: quien esculpe ya tiene ZBrush o Blender, y
no cambiará por una versión peor integrada con un CAD. **El coste de oportunidad
es enorme y el retorno, cero.**

### Recorte 2 — Rigging, skinning y animación. **Fuera.**

Solo paga si además tienes render de personajes, exportación a motores,
herramientas de retargeting y un pipeline de animación. Es el pilar más grande de
los cuatro disfrazado de línea en una lista. Y no tiene ninguna relación con el
CAD paramétrico, que es lo que hace único a este proyecto.

Nota: **glTF exporta skinning y animación**. Si el usuario riggea en Blender y la
malla viene de FORGE, el flujo funciona sin que FORGE riggee nada.

### Recorte 3 — Runtime empaquetado como aplicación autónoma. **Recortado a la mitad.**

El requisito dice "exportar/ejecutar escenas como aplicación runtime independiente,
no solo previsualización de editor". La intención arquitectónica es buena: obliga a
que el render no dependa del editor. Pero el *entregable* pedido —un ejecutable
distribuible— implica empaquetado de assets, mapeo de entrada, bucle de juego,
scripting en tiempo de ejecución, firma de código y distribución por plataforma.

**Contrapropuesta:** `forge-runtime`, un binario headless que carga un `.forge` o
un glTF y renderiza, compartiendo el mismo grafo de pases que el editor. Se prueba
el 80% del valor arquitectónico (la escena es portable, el render es independiente)
con el 10% del trabajo. El empaquetado por plataforma es v2.

### Recorte 4 — Malla → B-Rep. **Fuera, y anunciado.**

Ya está decidido en [ADR-0002](adr/0002-representacion-dual.md#r4--la-dirección-malla--b-rep-no-existe-en-v1),
pero merece repetirse aquí porque será la funcionalidad más pedida: la reingeniería
automática de mallas a superficies es un producto entero (Geomagic, QuickSurface),
no una función. Prometerla a medias es peor que no ofrecerla, porque falla después
de que el usuario ya invirtió días.

### Recorte 5 — USD completo. **Reducido a exportación estática.**

USD como formato de intercambio de geometría y materiales: razonable. USD como
sistema de composición —capas, variantes, referencias, payloads, value resolution—
es un modelo conceptual del tamaño de un pilar. En v1, escritura de un subconjunto
estático. Ver [`03-dependencias.md`](03-dependencias.md).

### Recorte 6 — UV interactivo. **Reducido a automático.**

El unwrap automático se resuelve integrando **xatlas**. La edición interactiva de
costuras, el pinning, el packing manual y el visor de distorsión son otro proyecto.
Compromiso: automático en v1, con costuras marcables manualmente como única
intervención del usuario.

### Recorte 7 — Superficies NURBS como disciplina de autoría. **Fuera.**

`sweep` y `loft` a través de OpenCASCADE, sí. Parcheado de superficies, continuidad
G2, herramientas de superficie clase A: eso es lo que Alias y Rhino hacen, y es
donde la sofisticación de la interfaz importa más que el kernel.

### Recorte 8 — Plugins nativos cargados dinámicamente. **Aplazado, boundaries no.**

Detallado en [ADR-0006](adr/0006-plugins-y-scripting.md). Las fronteras entre
módulos, desde el día uno e impuestas por CI. La carga dinámica, cuando las
interfaces hayan demostrado ser las correctas.

---

## 3. Lo que quedaría: la propuesta de v1

> **FORGE v1: un CAD paramétrico cuya salida son mallas listas para producción,
> con un almacén de activos serio detrás.**

Concretamente:

1. **CAD paramétrico sólido** — sketch con restricciones, extrude/revolve/sweep/loft,
   fillet/chamfer/shell, booleanos, patterns, árbol de historia editable, STEP in/out.
2. **La frontera de dominio hecha bien** — `ToMesh` con procedencia y re-vinculación.
   Es el diferenciador real; nadie lo hace bien.
3. **Edición poligonal acotada** — operaciones de vértice/arista/cara, subdivisión,
   y una pila corta de modificadores no destructivos. Lo suficiente para limpiar y
   preparar geometría venida del CAD, no para modelar personajes desde cero.
4. **Viewport PBR de calidad** — IBL, sombras, materiales por nodos. Que la pieza se
   vea bien es lo que hace que el usuario enseñe capturas, y eso es marketing gratis.
5. **Almacén de activos versionado** — es el pilar más barato de los cuatro (semanas,
   no años, porque el mecanismo de blobs ya existe para el undo) y el que menos
   competencia tiene integrado en una herramienta de modelado.

### Por qué este recorte es coherente y no arbitrario

Los cuatro pilares originales no tienen un usuario común: quien hace CAD mecánico
no esculpe, y quien esculpe no necesita cotas exactas. Un producto que intenta
servir a los dos no sirve a ninguno.

En cambio, **"del CAD a la malla de producción" sí es un flujo con dolor real y
usuarios que ya pagan por resolverlo**: visualización de producto, animación
técnica, impresión 3D, videojuegos con assets industriales, arquitectura. Hoy ese
flujo es *exportar STEP → importar en Blender → arreglar la malla a mano → volver
a empezar cuando cambia una cota*. Ese último punto —el retrabajo tras un cambio de
diseño— es exactamente lo que resuelve la frontera de dominio con procedencia.

Es un nicho pequeño, y por eso mismo abordable. Es el mismo territorio donde MOI3D
y Plasticity han encontrado usuarios siendo productos de una o dos personas.

---

## 4. Si el recorte no se acepta

Es una decisión legítima, y en ese caso conviene ser explícito sobre las
consecuencias, no discutirlas después:

- El calendario se mide en **años, no en meses**, y hace falta un equipo mayor.
- Los cuatro pilares llegarán simultáneamente a un estado de "funciona en la demo,
  falla con trabajo real", que es el peor lugar posible: demasiado inacabado para
  usarse, demasiado grande para arreglarse.
- La arquitectura de este documento **sigue siendo la correcta** en cualquiera de
  los dos casos. Lo que cambia es cuántas fases se completan y en qué plazo. Nada
  de lo decidido en los ADR habría que rehacerlo.

---

## 5. Recomendación

Aceptar los recortes 1, 2, 4, 6 y 7 sin reservas. Negociar el 3 (runtime) y el 5
(USD) según a quién se quiera vender. Mantener el 8 como está.

Y fijar un criterio de parada para cada fase antes de empezarla, porque el riesgo
R3 —"no se recorta y salen cuatro mitades"— no se materializa por una decisión
grande, sino por veinte pequeñas del tipo "y ya que estamos, añadimos…".
