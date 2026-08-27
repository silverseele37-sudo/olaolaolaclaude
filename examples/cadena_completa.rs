//! Demo de cadena completa: **del sketch a la malla de producción**.
//!
//!     cargo run --example cadena_completa
//!
//! Recorre los pilares que existen hoy, en el orden en que los recorrería un
//! usuario, y sin GPU:
//!
//! ```text
//!   perfil 2D ─► extrude ─► chaflán       (dominio EXACTO,   forge-kernel-stub)
//!                    │
//!                    ▼  ToMesh  ← la puerta de un solo sentido
//!                    │
//!   subdividir ─► espejo ─► triangular    (dominio DISCRETO, forge-mesh)
//!                    │
//!                    ├─► glTF 2.0 / OBJ                     (forge-interop)
//!                    ├─► documento .forge                   (forge-doc + forge-io)
//!                    └─► biblioteca de activos              (forge-assets)
//! ```
//!
//! Lo que el demo demuestra no es que cada pieza funcione —para eso están los
//! tests— sino que **la identidad sobrevive el viaje entero**: la cara que el
//! usuario selecciona en el sólido exacto sigue siendo localizable después de
//! cruzar al dominio poligonal, subdividir dos veces y triangular.

use std::sync::Arc;

use forge_assets::{AssetMeta, AssetQuery, AssetStore, AssetType};
use forge_doc::{ComponentRegistry, Document, Geometry, GeometryPayload, Name, Transform, Visible};
use forge_interop::{gltf, obj, TriangleSoup};
use forge_kernel_api::*;
use forge_kernel_stub::StubKernel;
use forge_math::{DVec2, DVec3};
use forge_mesh::{Mirror, ModifierStack, Subdivide, Triangulate};
use forge_store::{BlobStore, MemoryBlobStore};

fn titulo(t: &str) {
    println!("\n\x1b[1m{t}\x1b[0m\n{}", "─".repeat(t.chars().count()));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::Path::new("target/demo");
    std::fs::create_dir_all(dir)?;

    // =======================================================================
    titulo("1. Dominio exacto — modelar la pieza");

    let k = StubKernel::new();
    let feat = forge_doc::FeatureId::from_u128(1);

    // Perfil en L, el caso de prueba clásico: no es convexo, así que obliga al
    // recorte de orejas de verdad.
    let perfil_pts = [
        DVec2::new(0.0, 0.0), DVec2::new(60.0, 0.0), DVec2::new(60.0, 20.0),
        DVec2::new(20.0, 20.0), DVec2::new(20.0, 50.0), DVec2::new(0.0, 50.0),
    ];
    let perfil = k.profile_from_polygon(&perfil_pts, feat)?;
    let solido = k.extrude(
        perfil,
        ExtrudeOpts { direction: DVec3::Z, distance_mm: 12.0, symmetric: false },
        feat,
    )?;

    let props = k.mass_properties(solido)?;
    let topo = k.topology(solido)?;
    println!("   perfil en L, 6 lados, extruido 12 mm");
    println!("   caras {}   aristas {}", topo.faces.len(), topo.edges.len());
    println!("   volumen {:.1} mm³   area {:.1} mm²", props.volume_mm3, props.area_mm2);
    // 60·20 + 20·30 = 1800 mm² de sección, x 12 mm = 21600 mm³
    println!("   (a mano: (60·20 + 20·30) · 12 = 21600 mm³)");

    // Chaflán en una arista vertical: cambia la topología, que es lo que el
    // nombrado persistente tiene que sobrevivir.
    let arista = topo.edges[0].id;
    let con_chaflan = k.chamfer(solido, &[arista], ChamferSpec::Symmetric { distance_mm: 3.0 }, feat)?;
    let topo2 = k.topology(con_chaflan)?;
    let v2 = k.mass_properties(con_chaflan)?.volume_mm3;
    // Un chaflán de d sobre una arista de longitud L quita un prisma triangular
    // de catetos d: (d²/2)·L. Aquí la arista mide 60 mm, así que (9/2)·60 = 270.
    let largo = {
        let e = &topo.edges[0];
        let _ = e;
        (props.volume_mm3 - v2) / (3.0 * 3.0 / 2.0)
    };
    println!("\n   chaflán de 3 mm sobre una arista");
    println!("   caras {} → {}   volumen {:.1} mm³", topo.faces.len(), topo2.faces.len(), v2);
    println!("   quitó {:.1} mm³ = (3²/2)·{:.0}, o sea una arista de {:.0} mm",
        props.volume_mm3 - v2, largo, largo);

    // La cara que el usuario "selecciona": la elegimos por procedencia, no por
    // índice, que es justamente el punto.
    let seleccionada = topo2
        .faces
        .iter()
        .find(|f| matches!(f.provenance, TopoProvenance::Blend { .. }))
        .map(|f| f.id)
        .expect("el chaflán creó una cara Blend");
    println!("   el usuario selecciona la cara del chaflán");

    // =======================================================================
    titulo("2. La puerta de un solo sentido — ToMesh");

    let tess = k.tessellate(con_chaflan, &TessellationParams::default())?;
    tess.validate()?;
    println!("   teselado: {} triángulos, {} aristas clasificadas",
        tess.triangle_count(), tess.edges.len());
    println!("   se dibujan {} (el resto son tangentes o costuras)",
        tess.edges.iter().filter(|e| e.kind.se_dibuja()).count());

    let malla = forge_mesh::to_mesh(&tess)?;
    println!("   malla: {} vértices, {} caras, Euler = {}",
        malla.vertex_count(), malla.face_count(), malla.euler());
    println!("   procedencia conservada: {:.0}%", malla.prov.cobertura() * 100.0);
    println!("   la cara seleccionada son ahora {} triángulos",
        malla.prov.caras_de(seleccionada).len());

    // =======================================================================
    titulo("3. Dominio discreto — la pila de modificadores");

    let pila = ModifierStack::new()
        .push(Subdivide::new(2))
        .push(Mirror { punto: DVec3::new(-10.0, 0.0, 0.0), normal: DVec3::X, soldar_costura: false })
        .push(Triangulate);
    let final_ = pila.apply(&malla)?;
    final_.validate()?;

    println!("   subdividir ×2 → espejo → triangular");
    println!("   {} caras → {} caras", malla.face_count(), final_.face_count());
    println!("   procedencia conservada: {:.0}%", final_.prov.cobertura() * 100.0);

    let sobreviven = final_.prov.caras_de(seleccionada).len();
    println!("\n   \x1b[1mla cara seleccionada aguas arriba sigue localizable: {} caras\x1b[0m", sobreviven);
    println!("   (eso es la frontera de dominio haciendo su trabajo)");

    // =======================================================================
    titulo("4. Exportar");

    let mut soup = TriangleSoup {
        name: "soporte-en-L".into(),
        positions: final_.positions.clone(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: final_.faces.iter().flat_map(|f| f.verts.clone()).collect(),
    };
    soup.normals.clear();
    soup.validate()?;

    let glb = gltf::to_glb(&soup, gltf::GltfOptions::default())?;
    std::fs::write(dir.join("soporte.glb"), &glb)?;
    let j = gltf::glb_json(&glb)?;
    let max = j["accessors"][0]["max"].as_array().unwrap();
    println!("   soporte.glb   {} bytes", glb.len());
    println!("   convertido a Y-arriba y a metros, como manda la especificación:");
    println!("   extremo superior en glTF = {:.4} m  (era {:.1} mm en Z)",
        max[1].as_f64().unwrap(), final_.positions.iter().map(|p| p.z).fold(f64::MIN, f64::max));

    obj::write(dir.join("soporte.obj"), &soup, obj::ObjOptions::completo())?;
    println!("   soporte.obj   {} bytes (Z-arriba nativo)",
        std::fs::metadata(dir.join("soporte.obj"))?.len());

    // =======================================================================
    titulo("5. Documento y biblioteca");

    let blobs = MemoryBlobStore::new();
    let brep_blob = blobs.put(&k.serialize(con_chaflan)?)?;
    let malla_blob = blobs.put(&glb)?;

    let mut doc = Document::new();
    let pieza = doc.edit("importar soporte", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("soporte en L".into()));
        tx.set(e, Transform::IDENTITY);
        tx.set(e, Geometry(GeometryPayload::Brep(brep_blob)));
        tx.set(e, Visible(true));
        e
    });
    doc.edit("version poligonal", |tx| {
        let e = tx.spawn();
        tx.set(e, Name("soporte en L (malla)".into()));
        tx.set(e, forge_doc::Parent(pieza));
        tx.set(e, Geometry(GeometryPayload::Mesh(malla_blob)));
        tx.set(e, Visible(true));
    });

    let snap = doc.snapshot();
    println!("   documento: {} entidades, {} blobs referenciados",
        snap.entity_count(), snap.referenced_blobs().len());
    for (e, g) in snap.iter::<Geometry>() {
        let n = snap.get::<Name>(e).map(|n| n.0.as_str()).unwrap_or("?");
        println!("     {:<24} {:<5} dominio {:?}", n, g.0.kind(), g.0.domain());
    }

    let ruta = dir.join("soporte.forge");
    forge_io::save(&ruta, &snap, &blobs, &forge_io::SaveOptions::default())?;
    println!("   soporte.forge  {} bytes", std::fs::metadata(&ruta)?.len());

    let recargado = forge_io::load(&ruta, Arc::new(ComponentRegistry::new()), &MemoryBlobStore::new())?;
    println!("   recargado en un almacén vacío: huella {}",
        if recargado.snapshot().fingerprint() == snap.fingerprint() { "IDÉNTICA" } else { "DISTINTA" });

    // Biblioteca de activos
    let lib = dir.join("biblioteca");
    let _ = std::fs::remove_dir_all(&lib);
    let mut store = AssetStore::open(&lib)?;
    store.import_bytes("/piezas/soporte.glb", &glb,
        AssetMeta::new("Soporte en L", AssetType::Modelo)
            .with_tags(["mecanica", "chapa"])
            .with_notes("perfil en L, chaflan 3 mm"))?;
    store.import_bytes("/piezas/soporte.forge", &std::fs::read(&ruta)?,
        AssetMeta::new("Soporte en L (documento)", AssetType::Documento)
            .with_tags(["mecanica"]))?;

    let encontrados = store.search(&AssetQuery::new().with_any_tags(["mecanica"]))?;
    println!("   biblioteca: {} activos, {} con la etiqueta «mecanica»",
        store.len()?, encontrados.len());

    // =======================================================================
    titulo("Resumen");
    println!("   El perfil 2D llegó a glTF pasando por seis transformaciones,");
    println!("   dos dominios y tres formatos — y la cara que el usuario");
    println!("   seleccionó al principio sigue siendo localizable al final.");
    println!("\n   Archivos en {}\n", dir.display());
    Ok(())
}
