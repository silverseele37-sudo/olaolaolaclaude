use forge_render_cpu::agx::*;

fn escala(xp: f32, yp: f32, s: f32, p: f32) -> f32 {
    ((s * xp).powf(-p) * ((s * (xp / yp)).powf(p) - 1.0)).powf(-1.0 / p)
}
fn hip(x: f32, p: f32) -> f32 { x / (1.0 + x.powf(p)).powf(1.0 / p) }
fn curva(x: f32, pend: f32, pie: f32, hom: f32) -> f32 {
    let px = PIVOTE_X;
    let sp = -escala(px, 0.5, pend, pie);
    let sh = escala(1.0 - px, 0.5, pend, hom);
    let (s, p) = if x < px { (sp, pie) } else { (sh, hom) };
    (s * hip(pend * (x - px) / s, p) + 0.5).clamp(0.0, 1.0)
}

#[test]
fn medir() {
    for (pend, pie, hom) in [(2.0f32,3.0f32,2.9f32),(2.0,3.0,3.0),(2.0,3.0,2.85),(2.05,3.0,2.9),(2.0,2.9,2.9),(2.0,3.1,2.9)] {
        let mut maxe = 0.0f32; let mut at = 0.0;
        for i in 0..=8000 {
            let x = i as f32 / 8000.0;
            let e = (contraste_polinomico(x) - curva(x, pend, pie, hom)).abs();
            if e > maxe { maxe = e; at = x; }
        }
        println!("pend={pend:.2} pie={pie:.2} hom={hom:.2} -> err={maxe:.6} en x={at:.3}");
    }
}
