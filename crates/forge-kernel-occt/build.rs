//! Descubrimiento de OpenCASCADE, no rutas fijas.
//!
//! Este script no asume que OCCT está instalado. Busca en varios sitios
//! razonables y, si no encuentra nada, **no falla el build**: emite
//! `cfg(sin_occt)` y compila el crate entero con las funciones devolviendo
//! `KernelError::Unsupported("compilado sin OpenCASCADE")`. Es lo que mantiene
//! el workspace verde en cualquier máquina sin C++ instalado (ver
//! `docs/construir.md` §3 y `docs/construir-occt.md`).
//!
//! ## Por qué los toolkits se enumeran del disco
//!
//! Los nombres de los toolkits de OCCT cambian entre versiones: la 7.8 fusionó
//! `TKSTEP`, `TKSTEPBase` y otros en `TKDESTEP`. Una lista fija de nombres se
//! rompe en la siguiente versión sin avisar (falla el *link*, no la compilación,
//! que es más difícil de diagnosticar). En vez de eso, este script lee el
//! directorio de librerías y enlaza lo que encuentra con prefijo `TK`.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // Registra el cfg para que `rustc` no lo trate como "unexpected" bajo
    // `-D warnings` (CI compila con eso). Sin esta línea, `#[cfg(sin_occt)]`
    // produciría un warning -> error en cuanto CI lo viera.
    println!("cargo:rustc-check-cfg=cfg(sin_occt)");
    println!("cargo:rerun-if-env-changed=OCCT_ROOT");
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=src/shim.hpp");

    match descubrir() {
        Some(occt) => compilar(&occt),
        None => {
            println!("cargo:rustc-cfg=sin_occt");
            println!(
                "cargo:warning=forge-kernel-occt: no se encontro OpenCASCADE. \
                 El crate compila con todas las funciones devolviendo \
                 KernelError::Unsupported(\"compilado sin OpenCASCADE\"). \
                 Para compilar el kernel de verdad: compila OCCT (ver \
                 docs/construir-occt.md) y exportá OCCT_ROOT apuntando a su \
                 directorio de build o de instalacion, por ejemplo: \
                 OCCT_ROOT=~/dev/occt-build cargo build --workspace"
            );
        }
    }
}

/// Una instalación (o árbol de build) de OCCT ya localizada en disco.
struct Occt {
    /// Uno o más directorios `-I`. El árbol de build de OCCT expone las
    /// cabeceras en `inc/`; una instalación empaquetada suele usar
    /// `include/opencascade/`. Se prueban ambos porque no hay forma de saber
    /// cuál es sin mirar.
    include_dirs: Vec<PathBuf>,
    lib_dir: PathBuf,
    /// Nombres de toolkit sin prefijo `lib` ni extensión, p.ej. `TKernel`,
    /// `TKDESTEP`. Lo que exista en el disco, no una lista fija.
    toolkits: Vec<String>,
}

/// Prueba, en orden, las rutas que la documentación (`docs/construir.md` §3)
/// promete que funcionan. La primera que contenga cabeceras y al menos un
/// toolkit `TK*` gana.
fn descubrir() -> Option<Occt> {
    let mut raices: Vec<PathBuf> = Vec::new();
    if let Ok(v) = env::var("OCCT_ROOT") {
        if !v.is_empty() {
            raices.push(PathBuf::from(v));
        }
    }
    // Homebrew en macOS instala así; se prueba igual en otras plataformas por
    // si alguien replicó la disposición a mano.
    raices.push(PathBuf::from("/usr/local/opt/occt"));
    // Rutas de Windows sugeridas por docs/construir.md, elegidas justamente
    // para no colisionar con `C:/dev/OCCT` (el árbol fuente) — ver la nota
    // sobre mayúsculas/minúsculas ahí.
    raices.push(PathBuf::from("C:/dev/occt-install"));
    raices.push(PathBuf::from("C:/dev/occt-build"));

    for raiz in &raices {
        if let Some(o) = probar_raiz(raiz) {
            return Some(o);
        }
    }

    // Último recurso: pkg-config. OCCT vainilla no publica un `.pc`, pero
    // algunas distribuciones (y algunos `cmake --install` con parches locales)
    // sí, bajo el nombre `opencascade`. `probe()` solo hace metadata (no
    // enlaza nada por sí mismo), así que igual hace falta enumerar el
    // directorio de librerías para tener la lista de toolkits reales.
    if let Ok(lib) = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("opencascade")
    {
        let lib_dir = lib.link_paths.into_iter().next()?;
        let toolkits = enumerar_toolkits(&lib_dir);
        if !toolkits.is_empty() {
            return Some(Occt {
                include_dirs: lib.include_paths,
                lib_dir,
                toolkits,
            });
        }
    }

    None
}

fn probar_raiz(raiz: &Path) -> Option<Occt> {
    if !raiz.is_dir() {
        return None;
    }

    let candidatos_lib = [
        "lib",
        "lib64",
        // Layout típico de un `cmake --install` de OCCT en Windows con MSVC.
        "win64/vc14/lib",
        "win64/vc14/libd",
    ];
    let lib_dir = candidatos_lib
        .iter()
        .map(|c| raiz.join(c))
        .find(|p| p.is_dir())?;

    let toolkits = enumerar_toolkits(&lib_dir);
    if toolkits.is_empty() {
        return None;
    }

    let candidatos_inc = ["include/opencascade", "inc", "include"];
    let include_dirs: Vec<PathBuf> = candidatos_inc
        .iter()
        .map(|c| raiz.join(c))
        .filter(|p| p.is_dir())
        .collect();
    if include_dirs.is_empty() {
        return None;
    }

    Some(Occt {
        include_dirs,
        lib_dir,
        toolkits,
    })
}

/// Lee `dir` y devuelve los nombres de toolkit `TK*` que encuentra,
/// sin duplicados y sin prefijo/extensión de plataforma.
///
/// Unix: `libTKernel.so`, `libTKernel.so.7.9.0`, `libTKernel.a` -> `TKernel`.
/// Windows: `TKernel.lib` -> `TKernel` (el import lib no lleva prefijo `lib`).
fn enumerar_toolkits(dir: &Path) -> Vec<String> {
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut vistos = std::collections::BTreeSet::new();
    for e in entradas.flatten() {
        let Some(archivo) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let sin_prefijo = archivo.strip_prefix("lib").unwrap_or(&archivo);
        // Corta en el primer punto: cubre `.so`, `.so.7`, `.dylib`, `.a`, `.lib`.
        let nombre = match sin_prefijo.split_once('.') {
            Some((n, _)) => n,
            None => continue,
        };
        if nombre.starts_with("TK") && !nombre.is_empty() {
            vistos.insert(nombre.to_string());
        }
    }
    vistos.into_iter().collect()
}

fn compilar(occt: &Occt) {
    for tk in &occt.toolkits {
        println!("cargo:rustc-link-lib=dylib={tk}");
    }
    println!("cargo:rustc-link-search=native={}", occt.lib_dir.display());

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // El objeto que compila `cc` contiene símbolos de C++ (manejo de
    // excepciones, `new`/`delete`); en Unix hace falta pedir la biblioteca de
    // runtime explícitamente. MSVC la enlaza sola.
    match target_os.as_str() {
        "macos" => println!("cargo:rustc-link-lib=c++"),
        "windows" => {}
        _ => println!("cargo:rustc-link-lib=stdc++"),
    }
    // `TKernel` y compañía usan hilos y carga dinámica en Linux/BSD.
    if target_os != "windows" && target_os != "macos" {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=dl");
    }

    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file("src/shim.cpp");
    for inc in &occt.include_dirs {
        build.include(inc);
    }
    // OCCT expone macros que dependen de la plataforma para exportar símbolos
    // de las DLL en Windows; sin definir esto, algunas cabeceras de TKernel no
    // resuelven en MSVC al consumirse desde fuera del propio build de OCCT.
    if target_os == "windows" {
        build.define("WNT", None);
    }
    build.warnings(true).compile("forge_occt_shim");
}
