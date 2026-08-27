//! Capa de interfaz de FORGE.
//!
//! # Qué está verificado y qué no
//!
//! El contenedor donde se escribió este crate **no tiene GPU ni pantalla**, así
//! que la separación entre lo verificable y lo que no lo es no es un detalle:
//! es la estructura del crate.
//!
//! - **Verificado**: la extracción de escena, la cámara orbital, la proyección,
//!   el umbral de clic y las estadísticas. Todo eso es matemática y lógica
//!   pura, y se prueba con respuestas conocidas sin abrir una ventana.
//! - **No verificado**: nada que abra una ventana o toque la GPU. Está marcado
//!   `#[ignore]` y se ejecuta en una máquina con hardware.
//!
//! # La pieza que sostiene la arquitectura
//!
//! [`escena::extraer_instancias`] es la **frontera de extracción**: convierte un
//! `Snapshot` de `forge-doc` en las `DrawInstance` de `forge-render-api`. Es lo
//! que impide que el renderer conozca el documento, y por tanto lo que permite
//! que `forge-runtime` comparta exactamente el mismo camino de render que el
//! editor sin arrastrar la interfaz.
//!
//! Si algún día el renderer necesita mirar dentro del documento, la separación
//! se rompió y el runtime deja de ser posible.

pub mod camara;
pub mod error;
pub mod escena;
pub mod estadisticas;
pub mod png_io;
pub mod proyeccion;
pub mod render_marcador;
pub mod seleccion;

pub use camara::{mundo_por_pixel, CamaraOrbital};
pub use error::UiError;
pub use escena::{caja_de_conjunto, extraer_instancias, SinResolucion};
pub use estadisticas::{calcular as calcular_estadisticas, Estadisticas};
pub use png_io::escribir_png;
pub use proyeccion::{proyectar, vista_proyeccion};
pub use render_marcador::{PaletaMarcador, RenderizadorMarcador};
pub use seleccion::{es_clic, mas_cercana, CandidatoPicking, UMBRAL_CLIC_PX};

pub type Result<T> = std::result::Result<T, UiError>;
