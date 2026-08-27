// Pase principal: fondo (radiancia del entorno) + geometría PBR metallic-roughness.
//
// Escribe en un color target HDR (Rgba16Float) en radiancia lineal Rec.709,
// SIN mapeo de tono: AgX se aplica después, en un pase de pantalla completa
// separado (ver agx.wgsl). Mezclar el tonemap aquí, por objeto, no compondría
// bien con la mezcla alfa ni con el resultado del blending de MSAA — es una
// transformada de formación de imagen sobre el frame entero, no un BRDF.
//
// Coordenadas: todo lo que llega aquí en `posicion_local` y en las matrices de
// instancia ya está resuelto a `f32` **relativo a la cámara** en el lado Rust
// (ver `crate::camara`), así que este shader nunca ve una coordenada de
// documento absoluta.

const MAX_LUCES: u32 = 16u;

const TIPO_LUZ_DIRECCIONAL: u32 = 0u;
const TIPO_LUZ_PUNTUAL: u32 = 1u;

struct Luz {
    // xyz: direccion de viaje (direccional) o posicion relativa a la camara
    // (puntual). w: tipo (0 = direccional, 1 = puntual), reinterpretado con
    // bitcast porque un uniform no mezcla f32 y u32 en el mismo componente
    // sin romper el layout de 16 bytes.
    direccion_o_posicion: vec4<f32>,
    color: vec3<f32>,
    intensity: f32,
    // x = tipo (bitcast de u32), y = radio_mm (solo puntual), z,w sin usar.
    extra: vec4<f32>,
}

struct Globales {
    vista_proyeccion: mat4x4<f32>,
    exposicion: f32,
    ibl_intensidad: f32,
    ibl_rotacion: f32,
    tiene_ibl: u32,
    numero_luces: u32,
    _relleno0: vec3<f32>,
    luces: array<Luz, MAX_LUCES>,
    sh: array<vec4<f32>, 9>,
}

@group(0) @binding(0)
var<uniform> globales: Globales;

struct EntradaVertice {
    @location(0) posicion_local: vec3<f32>,
    @location(1) normal_local: vec3<f32>,
    // Columnas de la transformada de instancia (relativa a cámara, ver
    // `crate::camara::Camara::transformada_relativa`).
    @location(2) modelo_c0: vec4<f32>,
    @location(3) modelo_c1: vec4<f32>,
    @location(4) modelo_c2: vec4<f32>,
    @location(5) modelo_c3: vec4<f32>,
    // Columnas de la inversa-transpuesta del bloque 3x3, para normales.
    @location(6) normal_c0: vec4<f32>,
    @location(7) normal_c1: vec4<f32>,
    @location(8) normal_c2: vec4<f32>,
    @location(9) normal_c3: vec4<f32>,
    @location(10) base_color: vec4<f32>,
    // x = metallic, y = roughness.
    @location(11) material: vec4<f32>,
}

struct SalidaVertice {
    @builtin(position) clip: vec4<f32>,
    // Posición relativa a la cámara (mundo, sin proyectar): el ojo es siempre
    // el origen en este espacio, así que el vector hacia la cámara en el
    // fragment shader es simplemente `normalize(-posicion_rel)`.
    @location(0) posicion_rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) base_color: vec3<f32>,
    @location(3) metallic_roughness: vec2<f32>,
}

@vertex
fn vs_main(in: EntradaVertice) -> SalidaVertice {
    let modelo = mat4x4<f32>(in.modelo_c0, in.modelo_c1, in.modelo_c2, in.modelo_c3);
    let matriz_normal = mat4x4<f32>(in.normal_c0, in.normal_c1, in.normal_c2, in.normal_c3);

    let mundo_rel = modelo * vec4<f32>(in.posicion_local, 1.0);
    let normal_mundo = (matriz_normal * vec4<f32>(in.normal_local, 0.0)).xyz;

    var out: SalidaVertice;
    out.clip = globales.vista_proyeccion * mundo_rel;
    out.posicion_rel = mundo_rel.xyz;
    out.normal = normal_mundo;
    out.base_color = in.base_color.xyz;
    out.metallic_roughness = in.material.xy;
    return out;
}

// ---------------------------------------------------------------------------
// Armónicos esféricos: radiancia e irradiancia (Ramamoorthi-Hanrahan).
// Réplica de forge-render-cpu::sombreado, en WGSL porque aquí corre por
// fragmento en GPU y no hay forma de compartir el código Rust entre pilares.
// ---------------------------------------------------------------------------

const SH_C1: f32 = 0.429043;
const SH_C2: f32 = 0.511664;
const SH_C3: f32 = 0.743125;
const SH_C4: f32 = 0.886227;
const SH_C5: f32 = 0.247708;

fn desrotar(d: vec3<f32>, rot: f32) -> vec3<f32> {
    let s = sin(-rot);
    let c = cos(-rot);
    return vec3<f32>(c * d.x - s * d.y, s * d.x + c * d.y, d.z);
}

fn base_sh(d: vec3<f32>) -> array<f32, 9> {
    let x = d.x;
    let y = d.y;
    let z = d.z;
    var b: array<f32, 9>;
    b[0] = 0.282095;
    b[1] = 0.488603 * y;
    b[2] = 0.488603 * z;
    b[3] = 0.488603 * x;
    b[4] = 1.092548 * x * y;
    b[5] = 1.092548 * y * z;
    b[6] = 0.315392 * (3.0 * z * z - 1.0);
    b[7] = 1.092548 * x * z;
    b[8] = 0.546274 * (x * x - y * y);
    return b;
}

// Radiancia del entorno en una dirección, con max(., 0): el ringing de SH da
// valores negativos en entornos de mucho contraste.
fn radiancia_sh(dir: vec3<f32>) -> vec3<f32> {
    let d = normalize(desrotar(dir, globales.ibl_rotacion));
    let b = base_sh(d);
    var out = vec3<f32>(0.0);
    for (var i = 0u; i < 9u; i = i + 1u) {
        out = out + globales.sh[i].rgb * b[i];
    }
    return max(out * globales.ibl_intensidad, vec3<f32>(0.0));
}

// Irradiancia difusa E(n), forma cerrada de Ramamoorthi-Hanrahan.
fn irradiancia_sh(n: vec3<f32>) -> vec3<f32> {
    let d = normalize(desrotar(n, globales.ibl_rotacion));
    let x = d.x;
    let y = d.y;
    let z = d.z;
    let l00 = globales.sh[0].rgb;
    let l1m1 = globales.sh[1].rgb;
    let l10 = globales.sh[2].rgb;
    let l11 = globales.sh[3].rgb;
    let l2m2 = globales.sh[4].rgb;
    let l2m1 = globales.sh[5].rgb;
    let l20 = globales.sh[6].rgb;
    let l21 = globales.sh[7].rgb;
    let l22 = globales.sh[8].rgb;

    let e = SH_C1 * l22 * (x * x - y * y)
        + SH_C3 * l20 * z * z
        + SH_C4 * l00
        - SH_C5 * l20
        + 2.0 * SH_C1 * (l2m2 * x * y + l21 * x * z + l2m1 * y * z)
        + 2.0 * SH_C2 * (l11 * x + l1m1 * y + l10 * z);
    return max(e * globales.ibl_intensidad, vec3<f32>(0.0));
}

// ---------------------------------------------------------------------------
// BRDF metallic-roughness. Réplica de forge-render-cpu::sombreado: Lambert +
// Blinn-Phong para luces analíticas, Fresnel de Schlick con rugosidad
// (Lagarde) y la aproximación DFG de Karis para el entorno.
// ---------------------------------------------------------------------------

fn f0_de(base_color: vec3<f32>, metallic: f32) -> vec3<f32> {
    return mix(vec3<f32>(0.04), base_color, metallic);
}

fn fresnel_rugoso(cos_theta: f32, f0: vec3<f32>, rugosidad: f32) -> vec3<f32> {
    let f = pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
    let techo = max(vec3<f32>(1.0 - rugosidad), f0);
    return f0 + (techo - f0) * f;
}

// Devuelve (A, B) tales que la reflectancia especular del entorno es F0*A + B.
fn env_brdf(n_dot_v: f32, rugosidad: f32) -> vec2<f32> {
    let nv = clamp(n_dot_v, 0.0, 1.0);
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rugosidad * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    return vec2<f32>(-1.04 * a004 + r.z, 1.04 * a004 + r.w);
}

fn brillo_de(rugosidad: f32) -> f32 {
    let a = rugosidad * rugosidad;
    return clamp(2.0 / (a * a) - 2.0, 1.0, 1.0e6);
}

fn sombrear(
    posicion_rel: vec3<f32>,
    n_in: vec3<f32>,
    base_color: vec3<f32>,
    metallic: f32,
    roughness_in: f32,
) -> vec3<f32> {
    let n = normalize(n_in);
    // El ojo es el origen en espacio relativo a cámara (ver la nota del
    // módulo): el vector hacia la cámara es -posicion_rel normalizado.
    let v = normalize(-posicion_rel);

    let rugosidad = clamp(roughness_in, 0.02, 1.0);
    let f0 = f0_de(base_color, clamp(metallic, 0.0, 1.0));
    let kd_metal = 1.0 - clamp(metallic, 0.0, 1.0);
    let n_dot_v = max(dot(n, v), 0.0);

    var out = vec3<f32>(0.0);

    let esp = brillo_de(rugosidad);
    let norm_esp = (esp + 8.0) / (8.0 * 3.14159265);
    for (var i = 0u; i < globales.numero_luces; i = i + 1u) {
        let luz = globales.luces[i];
        let tipo = bitcast<u32>(luz.extra.x);
        var dir_l: vec3<f32>;
        var radiancia: vec3<f32>;
        if (tipo == TIPO_LUZ_DIRECCIONAL) {
            dir_l = normalize(-luz.direccion_o_posicion.xyz);
            radiancia = luz.color * luz.intensity;
        } else {
            let delta = luz.direccion_o_posicion.xyz - posicion_rel;
            let dist = length(delta);
            if (dist < 1e-6) {
                continue;
            }
            let radio_mm = luz.extra.y;
            let d_m = max(dist, max(radio_mm, 1e-3)) / 1000.0;
            let atenuacion = 1.0 / (d_m * d_m);
            dir_l = delta / dist;
            radiancia = luz.color * luz.intensity * atenuacion;
        }

        let n_dot_l = max(dot(n, dir_l), 0.0);
        if (n_dot_l <= 0.0) {
            continue;
        }
        let h = normalize(dir_l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let f = fresnel_rugoso(max(dot(dir_l, h), 0.0), f0, rugosidad);
        let s = norm_esp * pow(n_dot_h, esp);
        let difusa = kd_metal * (vec3<f32>(1.0) - f) * base_color / 3.14159265;
        out = out + (difusa + f * s) * radiancia * n_dot_l;
    }

    if (globales.tiene_ibl != 0u) {
        let e = irradiancia_sh(n);
        let f = fresnel_rugoso(n_dot_v, f0, rugosidad);
        let ab = env_brdf(n_dot_v, rugosidad);
        let r = normalize(2.0 * dot(n, v) * n - v);
        let l_esp = radiancia_sh(r);
        out = out + kd_metal * (vec3<f32>(1.0) - f) * base_color * e / 3.14159265;
        out = out + l_esp * (f0 * ab.x + ab.y);
    }

    return out;
}

@fragment
fn fs_main(in: SalidaVertice) -> @location(0) vec4<f32> {
    let color = sombrear(in.posicion_rel, in.normal, in.base_color, in.metallic_roughness.x, in.metallic_roughness.y);
    return vec4<f32>(color * globales.exposicion, 1.0);
}

// ---------------------------------------------------------------------------
// Fondo: un triángulo de pantalla completa que pinta la radiancia del entorno
// a lo largo del rayo de cámara, antes de la geometría (ver
// `crate::renderer`: se dibuja sin test de profundidad para que la geometría
// lo sobrescriba con el z-buffer normal, igual que en forge-render-cpu).
// ---------------------------------------------------------------------------

struct SalidaFondo {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_fondo(@builtin(vertex_index) indice: u32) -> SalidaFondo {
    // Triángulo que cubre de sobra [-1,1]x[-1,1] en NDC con 3 vértices.
    var posiciones = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: SalidaFondo;
    out.clip = vec4<f32>(posiciones[indice], 0.0, 1.0);
    out.ndc = posiciones[indice];
    return out;
}

@fragment
fn fs_fondo(in: SalidaFondo) -> @location(0) vec4<f32> {
    if (globales.tiene_ibl == 0u) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // Reconstruye el rayo de cámara igual que forge-render-cpu::raster::rayo,
    // pero en espacio relativo (el ojo es el origen aquí, así que no hace
    // falta restarlo de los puntos reconstruidos).
    let inv = globales.inversa_vista_proyeccion;
    let cerca = inv * vec4<f32>(in.ndc, 0.0, 1.0);
    let lejos = inv * vec4<f32>(in.ndc, 1.0, 1.0);
    let a = cerca.xyz / cerca.w;
    let b = lejos.xyz / lejos.w;
    let dir = normalize(b - a);
    let color = radiancia_sh(dir);
    return vec4<f32>(color * globales.exposicion, 1.0);
}
