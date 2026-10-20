//! Large brush/eraser and fast symmetry previews, including smoothing-tail restoration.
//! cargo run --release -p photocraft-paint --example bench_interactive_brush -- [--quick] [--reps N] [--json out.json]
//! RAYON_NUM_THREADS=1 exercises the serial path used by WebAssembly (not browser timing).
use std::time::Instant;

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::{BrushSettings, Smoothing, StrokePoint, StrokeRenderer, UnionLive};
use photocraft_raster::Surface;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quick = args.iter().any(|arg| arg == "--quick");
    let value = |name: &str| args.iter().position(|arg| arg == name).and_then(|i| args.get(i + 1));
    let reps = value("--reps").map(|s| s.parse::<usize>()).transpose()?.unwrap_or(if quick { 1 } else { 3 });
    if !(1..=20).contains(&reps) || (args.iter().any(|a| a == "--json") && value("--json").is_none()) {
        return Err("expected --reps 1..20 and a path after --json".into());
    }
    let (width, height) = if quick { (1920, 1080) } else { (6000, 4000) };
    let mut rows = Vec::new();
    for (name, size, erase, symmetry) in [("large brush", 5000.0, false, false), ("large eraser", 5000.0, true, false), ("rapid symmetry", 200.0, false, true)]
    {
        let mut samples = Vec::new();
        let (mut push_ms, mut composite_ms) = (0.0, 0.0);
        for _ in 0..reps {
            let brush = BrushSettings {
                size,
                erase,
                hardness: 0.0,
                pressure_size: false,
                spacing: 0.25,
                smoothing: Smoothing { amount: 0.1, ..Default::default() },
                ..Default::default()
            };
            let mut target = Surface::new(PixelFormat::RGBA8);
            target.fill_rect(Rect::new(0, 0, width, height), &[0.5, 0.6, 0.7, 1.0]);
            let pre = target.clone();
            let mut first = StrokeRenderer::new(&brush, Some(PixelFormat::RGBA8), 1.0);
            let mut mirror = StrokeRenderer::new(&brush, Some(PixelFormat::RGBA8), 1.0);
            let mut live = UnionLive::default();
            let count = if symmetry { 120 } else { 3 };
            for i in 0..count {
                let x = f64::from(width) * (0.2 + 0.6 * f64::from(i) / f64::from(count - 1));
                let y = f64::from(height) * (0.5 + 0.2 * (f64::from(i) * 0.2).sin());
                let point = StrokePoint { time: f64::from(i) * 8.0, ..StrokePoint::new(x, y, 1.0) };
                let start = Instant::now();
                first.push(&[point]);
                if symmetry {
                    mirror.push(&[StrokePoint { x: f64::from(width) - x, ..point }]);
                }
                let pushed = start.elapsed().as_secs_f64() * 1000.0;
                if symmetry {
                    StrokeRenderer::composite_union_live(&mut [&mut first, &mut mirror], &mut live, i == 0, &pre, &mut target, None, false);
                } else {
                    StrokeRenderer::composite_union_live(&mut [&mut first], &mut live, i == 0, &pre, &mut target, None, false);
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                push_ms += pushed;
                composite_ms += elapsed - pushed;
                samples.push(elapsed);
            }
        }
        let worst = samples.iter().copied().fold(0.0, f64::max);
        println!("{name}: {width}x{height}, {size}px, {} events, push={push_ms:.2}ms composite={composite_ms:.2}ms worst={worst:.2}ms", samples.len());
        rows.push(json!({"name": name, "samples_ms": samples, "push_ms": push_ms, "composite_ms": composite_ms}));
    }
    if let Some(path) = value("--json") {
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"bench": "bench_interactive_brush", "context": {"width": width, "height": height, "quick": quick, "reps": reps}, "rows": rows}),
            )?,
        )?;
    }
    Ok(())
}
