//! Frozen scalar rasterizer used only as a pixel oracle for the computed-tip fast path.
use super::*;
use crate::tile::GrayTile;
use photocraft_geom::Point;

impl BrushContext {
    fn rasterize_reference(&self, d: &Dab, dual: bool, rect: Rect, out: &mut Vec<f32>, mut depth: Option<&mut Vec<f32>>) {
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        out.clear();
        out.resize(w * h, 0.0);
        if let Some(dp) = depth.as_deref_mut() {
            dp.clear();
            dp.resize(w * h, 0.0);
        }
        let b = &self.brush;
        let (hardness, mips) = if dual { (b.dual_brush.hardness, self.dual_tip.as_deref()) } else { (b.hardness, self.tip.as_deref()) };
        let aliased = b.aliased && !dual;
        let wet = b.wet_edges && !dual && depth.is_none();
        let noise = b.noise && !dual;
        let tex_tip = !dual && b.texture.enabled && b.texture.each_tip && self.texture.is_some();
        let (cx, cy) = if aliased { grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
        let (sn, cs) = d.angle.sin_cos();
        let (fx, fy) = (if d.flip_x { -1.0 } else { 1.0 }, if d.flip_y { -1.0 } else { 1.0 });
        // Brush Projection: stretch the sampling coordinates along the tilt direction, which
        // foreshortens the tip there by `proj_scale`.
        let proj = (!dual && d.proj_scale < 0.999).then(|| {
            let (ps, pc) = d.proj_angle.sin_cos();
            (pc, ps, 1.0 / d.proj_scale.max(0.05) - 1.0)
        });
        let r = d.radius;
        let ro = d.roundness.max(0.5 / r).min(1.0);
        let rm = (r * ro).max(0.5);
        let reach = if aliased || mips.is_some() { rm } else { crate::tip_reach(rm, hardness).max(rm) };
        let reach2 = (reach + 0.5) * (reach + 0.5);
        // Sampled-tip mapping.
        let (level, inv_scale, tw, th) = match mips {
            Some(m) => {
                let (w0, h0, _) = &m.levels[0];
                let big = *w0.max(h0) as f32;
                (m.level_for(2.0 * r * ro.max(0.25)), big / (2.0 * r), *w0 as f32, *h0 as f32)
            }
            None => (0, 1.0, 1.0, 1.0),
        };
        // A pixel offset from the dab centre in tip space: to y-up, project, rotate by -angle, flip.
        let to_tip = |dx: f32, dy: f32| {
            let (mut ux, mut uy) = (dx, -dy);
            if let Some((ax, ay, k)) = proj {
                let t = (ux * ax + uy * ay) * k;
                ux += t * ax;
                uy += t * ay;
            }
            ((ux * cs + uy * sn) * fx, (-ux * sn + uy * cs) * fy)
        };
        // A tip a few pixels across takes each pixel's covered area (a 4×4 grid inside it):
        // sampling only the pixel centre made a tiny dab's total coverage swing with its sub-pixel
        // position, and thin lines beaded.
        let small = mips.is_none() && !aliased && rm < SMALL_TIP_RADIUS;
        let small_reach2 = (reach + 0.75) * (reach + 0.75);
        for yy in 0..h {
            let y = rect.y0 + yy as i32;
            let dy = y as f32 + 0.5 - cy;
            for xx in 0..w {
                let x = rect.x0 + xx as i32;
                let dx = x as f32 + 0.5 - cx;
                let (u, v) = to_tip(dx, dy);
                let (mut val, rn) = match mips {
                    // Block: the whole-pixel square, hard (the Eraser's Block mode).
                    _ if aliased && b.square => {
                        if dx.abs() >= r || dy.abs() >= r {
                            continue;
                        }
                        (1.0, 0.0)
                    }
                    None if small => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= small_reach2 {
                            continue;
                        }
                        let mut sum = 0.0;
                        for sy in SUBPIXEL {
                            for sx in SUBPIXEL {
                                let (su, sv) = to_tip(dx + sx, dy + sy);
                                let sd = ((su * ro).powi(2) + sv * sv).sqrt();
                                sum += tip_falloff(sd, rm, hardness);
                            }
                        }
                        (sum / (SUBPIXEL.len() * SUBPIXEL.len()) as f32, d2.sqrt() / rm)
                    }
                    None => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= reach2 {
                            continue;
                        }
                        let dd = d2.sqrt();
                        let val = if aliased { if dd <= rm { 1.0 } else { 0.0 } } else { dab_coverage(dd, rm, hardness) };
                        (val, dd / rm)
                    }
                    Some(m) => {
                        let (px, py) = (u * inv_scale + tw / 2.0, -(v / ro) * inv_scale + th / 2.0);
                        if px < -1.0 || py < -1.0 || px > tw + 1.0 || py > th + 1.0 {
                            continue;
                        }
                        let s = m.sample_clamped(level, px / tw, py / th);
                        let val = if aliased { if s >= 0.5 { 1.0 } else { 0.0 } } else { s };
                        (val, 1.0 - s)
                    }
                };
                if val <= 0.0 {
                    continue;
                }
                if noise && val < 1.0 {
                    let n = hash2(x, y, NOISE_SALT);
                    let hard = if n < val { 1.0 } else { 0.0 };
                    val += (hard - val) * 0.7;
                }
                if tex_tip {
                    val = mask_combine(b.texture.mode, val, self.texture_at(x, y), d.depth);
                }
                if wet {
                    val *= wet_factor(rn);
                }
                if let Some(slot) = depth.as_deref_mut().and_then(|dp| dp.get_mut(yy * w + xx)) {
                    *slot = (1.0 - rn).clamp(0.0, 1.0);
                }
                out[yy * w + xx] = val * d.alpha;
            }
        }
    }
}

fn compare(ctx: &BrushContext, dab: &Dab, dual: bool, rect: Rect, with_depth: bool) {
    // Reused, dirty buffers also check clearing and resizing.
    let (mut actual, mut expected) = (vec![0.75; 19], vec![0.75; 19]);
    let (mut actual_depth, mut expected_depth) = (vec![0.25; 23], vec![0.25; 23]);
    ctx.rasterize_into(dab, dual, rect, &mut actual, with_depth.then_some(&mut actual_depth));
    ctx.rasterize_reference(dab, dual, rect, &mut expected, with_depth.then_some(&mut expected_depth));
    for (label, a, b) in [("coverage", actual, expected), ("depth", actual_depth, expected_depth)] {
        assert_eq!(a.len(), b.len());
        let mismatch = a.iter().zip(&b).enumerate().find(|(_, (a, b))| a.to_bits() != b.to_bits());
        assert!(mismatch.is_none(), "{label} mismatch {mismatch:?}; {dab:?}; dual={dual}, rect={rect:?}");
    }
}

#[test]
fn computed_tips_match_scalar_bits() {
    for aliased in [false, true] {
        for hardness in [-0.5, 0.0, 0.1, 0.5, 0.9999, 1.0, 1.5] {
            let ctx = BrushContext::new(&BrushSettings { aliased, hardness, ..Default::default() });
            for radius in [0.5, 1.0, 2.99, 3.0, 3.01, 15.0, 31.5] {
                for roundness in [0.0, 0.1, 0.4, 1.0] {
                    for alpha in [-0.5, -0.0, 0.0, 0.37, 1.0] {
                        let mut d = Dab::round(Point::new(-31.25, 63.75), radius, alpha);
                        d.roundness = roundness;
                        d.flip_x = true;
                        d.flip_y = true;
                        compare(&ctx, &d, false, ctx.dab_rect(&d, false), false);
                    }
                }
            }
        }
    }
}

#[test]
fn large_corner_dabs_keep_off_canvas_coverage() {
    for (hardness, aliased) in [(0.0, false), (1.0, false), (1.0, true)] {
        let ctx = BrushContext::new(&BrushSettings { hardness, aliased, ..Default::default() });
        let d = Dab::round(Point::new(-1050.25, -1050.75), 1500.0, 0.73);
        let rect = ctx.dab_rect(&d, false);
        compare(&ctx, &d, false, rect, false);
        let mut coverage = Vec::new();
        ctx.rasterize(&d, false, rect, &mut coverage);
        // The layer is an infinite plane: paint outside the visible document must survive.
        assert!(coverage.iter().enumerate().any(|(i, v)| *v > 0.0 && rect.x0 + ((i % rect.width() as usize) as i32) < 0));
        compare(&ctx, &d, false, Rect::new(0, 0, 32, 32), false);
        compare(&ctx, &d, false, Rect::new(0, 0, 0, 0), false);
    }
}

#[test]
fn feature_and_exceptional_dabs_match_scalar_bits() {
    let tip = GrayTile::from_fn(8, 5, |x, y| ((x + y) % 7) as f32 / 6.0);
    for feature in 0..7 {
        let mut b = BrushSettings { hardness: 0.25, ..Default::default() };
        match feature {
            0 => b.noise = true,
            1 => b.wet_edges = true,
            2 => {
                b.aliased = true;
                b.square = true;
            }
            3 => b.tip = TipShape::Sampled(tip.clone()),
            4 => {
                b.texture.enabled = true;
                b.texture.each_tip = true;
                b.texture.pattern = Pattern::Tile(tip.clone());
            }
            5 => b.dual_brush.tip = TipShape::Sampled(tip.clone()),
            _ => {}
        }
        let ctx = BrushContext::new(&b);
        for dual in [false, true] {
            for with_depth in [false, true] {
                for (angle, proj_scale) in [(0.0, 1.0), (0.7, 1.0), (0.0, 0.6)] {
                    let mut d = Dab::round(Point::new(63.5, -0.25), 15.0, 0.47);
                    d.angle = angle;
                    d.roundness = 0.4;
                    d.proj_scale = proj_scale;
                    compare(&ctx, &d, dual, ctx.dab_rect(&d, dual), with_depth);
                }
            }
        }
    }
    let ctx = BrushContext::new(&BrushSettings::default());
    for value in [f32::NAN, f32::from_bits(0x7f80_0123), f32::INFINITY, f32::NEG_INFINITY] {
        for field in 0..4 {
            let mut d = Dab::round(Point::new(0.0, 0.0), 15.0, 1.0);
            match field {
                0 => d.alpha = value,
                1 => d.angle = value,
                2 => d.radius = value,
                _ => d.roundness = value,
            }
            compare(&ctx, &d, false, Rect::new(-4, -4, 4, 4), false);
        }
        let ctx = BrushContext::new(&BrushSettings { hardness: value, ..Default::default() });
        compare(&ctx, &Dab::round(Point::new(0.0, 0.0), 15.0, 1.0), false, Rect::new(-16, -16, 16, 16), false);
    }
}

#[test]
fn computed_tips_match_at_pixel_and_float_boundaries() {
    for aliased in [false, true] {
        for center in [Point::new(0.0, -0.0), Point::new(0.5, 0.5), Point::new(255.9999, -256.0001), Point::new(16_777_216.25, -16_777_216.75)] {
            for radius in [3.0, 3.5, 15.25, 113.75] {
                for hardness in [0.0, 0.6, 1.0] {
                    let ctx = BrushContext::new(&BrushSettings { hardness, aliased, ..Default::default() });
                    let mut d = Dab::round(center, radius, 0.137);
                    d.roundness = 0.375;
                    compare(&ctx, &d, false, ctx.dab_rect(&d, false), false);
                }
            }
        }
    }
}

impl StrokeRenderer {
    fn composite_reference(&mut self, pre: &Surface, target: &mut Surface, selection: Option<&Surface>, lock_transparency: bool, all: bool) -> Rect {
        let keys: Vec<(i32, i32)> = if all {
            self.cov.dirty.clear();
            let mut k: Vec<_> = self.cov.tiles.keys().copied().collect();
            k.sort_unstable();
            k
        } else {
            self.cov.take_dirty()
        };
        if let Some(d) = self.dual.as_mut() {
            d.dirty.clear();
        }
        let bounds = self.bounds();
        let fmt = target.format();
        let n = fmt.channels();
        let a_idx = alpha_index(&fmt);
        let nc = fmt.mode.color_channels();
        let b = &self.ctx.brush;
        let opacity = b.opacity.clamp(0.0, 1.0);
        let mut src = [0.0f32; 8];
        from_rgba_into(&fmt, b.color, &mut src);
        // A colour picked as CMYK paints its inks as they are in a CMYK target (#4344).
        if let (ColorMode::Cmyk, Some((rgb, inks))) = (fmt.mode, b.color_cmyk)
            && rgb == [b.color[0], b.color[1], b.color[2]]
        {
            src[..4].copy_from_slice(&inks);
        }
        if let Some(a) = a_idx {
            src[a] = b.color[3];
        }
        // 8-bit targets: the pixels the stroke changes are dithered as they are quantised, so
        // soft, low-flow strokes don't band (see `dither8`). Aliased strokes (the Pencil, the
        // Eraser's Pencil and Block modes) paint every pixel by the same amount, so they have no
        // gradient to band and are not dithered: a Pencil line at partial opacity is one flat
        // colour, as in Photoshop, not grain that a tolerance-0 Magic Wand or Paint Bucket trips on.
        let dither = fmt.sample == SampleType::U8 && !b.aliased;
        let mut dmg = Rect::EMPTY;
        let mut region = Vec::new();
        for (tx, ty) in keys {
            let Some(tile) = self.cov.tiles.get(&(tx, ty)) else { continue };
            let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&bounds);
            if tr.is_empty() {
                continue;
            }
            pre.read_region_into(tr, &mut region);
            let sel = selection.map(|s| (s.channels(), s.read_region(tr)));
            let w = tr.width() as usize;
            for y in tr.y0..tr.y1 {
                for x in tr.x0..tr.x1 {
                    let ti = ((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize;
                    let c = tile.cov[ti];
                    if c <= 0.0 {
                        continue;
                    }
                    let i = (y - tr.y0) as usize * w + (x - tr.x0) as usize;
                    let m = self.ctx.stroke_mask_wet(c, tile.depth.get(ti).copied(), self.dual.as_ref().map(|d| d.get(x, y)), x, y);
                    let s = sel.as_ref().map_or(1.0, |(sc, v)| v[i * sc]);
                    let mut k = (m * opacity * s).min(1.0);
                    if k <= 0.0 {
                        continue;
                    }
                    if b.mode == BlendMode::Dissolve {
                        k = if hash2(x, y, b.seed) < k { 1.0 } else { 0.0 };
                        if k == 0.0 {
                            continue;
                        }
                    }
                    let px = &mut region[i * n..(i + 1) * n];
                    if b.erase {
                        if let (Some(a), false) = (a_idx, lock_transparency) {
                            px[a] *= 1.0 - k;
                        }
                        if dither {
                            dither8(px, x, y);
                        }
                        continue;
                    }
                    if lock_transparency && a_idx.is_some_and(|a| px[a] <= 0.0) {
                        continue;
                    }
                    if self.per_dab_color && !tile.col.is_empty() {
                        let p = &tile.col[ti * nc..(ti + 1) * nc];
                        for ch in 0..nc {
                            src[ch] = p[ch] / c;
                        }
                    }
                    if matches!(b.mode, BlendMode::Normal | BlendMode::Dissolve) {
                        over_native(&fmt, px, &src[..n], k, lock_transparency);
                    } else {
                        let d = to_rgba(&fmt, px);
                        let sr = to_rgba(&fmt, &src[..n]);
                        let mut o = photocraft_color::blend::composite(b.mode, d, sr, k);
                        if lock_transparency {
                            o[3] = d[3];
                        }
                        let mut enc = [0.0f32; 8];
                        from_rgba_into(&fmt, o, &mut enc);
                        px.copy_from_slice(&enc[..n]);
                    }
                    if dither {
                        dither8(px, x, y);
                    }
                }
            }
            target.write_region(tr, &region);
            dmg = dmg.union(&tr);
        }
        if all { bounds } else { dmg }
    }
}

#[test]
fn tiled_dabs_match_dense_rasterization_and_bound_temporary_storage() {
    let brush = BrushSettings { size: 512.0, hardness: 0.3, wet_edges: true, ..Default::default() };
    let ctx = BrushContext::new(&brush);
    let dab = Dab::round(Point::new(-17.25, 29.75), 256.0, 0.37);
    let rect = ctx.dab_rect(&dab, false);
    let (mut dense, mut depths) = (Vec::new(), Vec::new());
    ctx.rasterize_reference(&dab, false, rect, &mut dense, Some(&mut depths));
    let mut expected = CoverageMap::new(0);
    expected.accumulate(rect, &dense, dab.opacity, None, Some(&depths));
    let mut actual = CoverageMap::new(0);
    let (mut scratch, mut depth) = (Vec::new(), Vec::new());
    actual.rasterize_dab(&ctx, &dab, false, &mut scratch, &mut depth, None);
    assert!(scratch.capacity() <= (COV_TILE * COV_TILE) as usize);
    assert!(depth.capacity() <= (COV_TILE * COV_TILE) as usize);
    assert_eq!(actual.bounds(), expected.bounds());
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            assert_eq!(actual.get(x, y), expected.get(x, y), "coverage {x},{y}");
            assert_eq!(actual.depth(x, y), expected.depth(x, y), "depth {x},{y}");
        }
    }
}

#[test]
fn coverage_snapshots_share_tiles_until_their_pixels_change() {
    let mut coverage = CoverageMap::new(0);
    let rect = Rect::new(0, 0, 128, 64);
    coverage.accumulate(rect, &vec![0.2; 128 * 64], 1.0, None, None);
    let mut copy = coverage.finished_subset(None, &HashSet::from([(0, 0), (1, 0)]));
    for key in [(0, 0), (1, 0)] {
        assert!(Arc::ptr_eq(&coverage.tiles[&key], &copy.tiles[&key]));
    }
    copy.accumulate(Rect::new(0, 0, 1, 1), &[0.5], 1.0, None, None);
    assert_eq!(coverage.get(0, 0), 0.2);
    assert!(copy.get(0, 0) > coverage.get(0, 0));
    assert!(!Arc::ptr_eq(&coverage.tiles[&(0, 0)], &copy.tiles[&(0, 0)]));
    assert!(Arc::ptr_eq(&coverage.tiles[&(1, 0)], &copy.tiles[&(1, 0)]));
}

#[test]
fn byte_tile_compositing_matches_the_scalar_oracle() {
    let base = BrushSettings { size: 37.0, hardness: 0.5, flow: 0.4, opacity: 0.6, pressure_size: false, color: [0.3, 0.7, 0.2, 0.45], ..Default::default() };
    let mut brushes = vec![
        base.clone(),
        BrushSettings { erase: true, ..base.clone() },
        BrushSettings { aliased: true, ..base.clone() },
        BrushSettings { opacity: 0.0, ..base.clone() },
        BrushSettings { wet_edges: true, ..base.clone() },
    ];
    let mut dual = base.clone();
    dual.dual_brush.enabled = true;
    dual.dual_brush.size = 19.0;
    brushes.push(dual);
    let mut textured = base.clone();
    textured.texture.enabled = true;
    brushes.push(textured);
    let mut colored = base.clone();
    colored.color_dynamics.enabled = true;
    colored.color_dynamics.hue_jitter = 0.5;
    brushes.push(colored);
    brushes.push(BrushSettings { color: [0.0, 0.0, 0.0, 1.0], ..base.clone() });
    brushes.push(BrushSettings { color: [1.0, 1.0, 1.0, 1.0], ..base.clone() });
    brushes.push(BrushSettings { color: [0.0, 1.0, 0.0, 0.5], ..base.clone() });
    let area = Rect::new(-65, -65, 135, 135);
    let mut selection = Surface::with_default(PixelFormat::GRAY8, &[0.45]);
    selection.fill_rect(Rect::new(-5, -10, 23, 41), &[0.0]);
    for mode in [ColorMode::Rgb, ColorMode::Grayscale, ColorMode::Cmyk, ColorMode::Lab] {
        for sample in SampleType::ALL {
            let fmt = PixelFormat::new(mode, sample, true);
            for default in [[0.0; 4], [0.2, 0.1, 0.4, 0.7]] {
                let mut pre = Surface::with_default(fmt, &photocraft_raster::from_rgba(&fmt, default));
                for y in -7i32..33 {
                    for x in -12i32..29 {
                        pre.write_pixel(x, y, &photocraft_raster::from_rgba(&fmt, [0.4, 0.6, 0.2, (x + y).rem_euclid(11) as f32 / 10.0]));
                    }
                }
                for brush in &brushes {
                    for lock in [false, true] {
                        for sel in [None, Some(&selection)] {
                            let mut renderer = StrokeRenderer::new(brush, Some(fmt), 1.0);
                            let mut reference = renderer.clone();
                            let (mut actual, mut expected) = (pre.clone(), pre.clone());
                            // Incremental calls must overwrite the previous preview from pre,
                            // including zero coverage, selection holes and tile boundaries.
                            for point in [StrokePoint::new(-1.25, 0.5, 1.0), StrokePoint::new(67.0, 61.0, 0.5)] {
                                renderer.push(&[point]);
                                reference.push(&[point]);
                                assert_eq!(
                                    renderer.composite(&pre, &mut actual, sel, lock, false),
                                    reference.composite_reference(&pre, &mut expected, sel, lock, false)
                                );
                                if sample == SampleType::U8 && !renderer.per_dab_color {
                                    // Force the serial byte path below the production threshold
                                    // to check optional masks, selection holes and tile edges.
                                    let keys: Vec<_> = renderer.cov.tiles.keys().copied().collect();
                                    let mut src = [0.0; 8];
                                    from_rgba_into(&fmt, brush.color, &mut src);
                                    match fmt.channels() {
                                        2 => {
                                            renderer.composite_u8_serial::<2>(&keys, &pre, &mut actual, sel, lock, &src, false);
                                        }
                                        4 => {
                                            renderer.composite_u8_serial::<4>(&keys, &pre, &mut actual, sel, lock, &src, false);
                                        }
                                        5 => {
                                            renderer.composite_u8_serial::<5>(&keys, &pre, &mut actual, sel, lock, &src, false);
                                        }
                                        _ => {}
                                    }
                                }
                                assert_eq!(actual.read_region(area), expected.read_region(area), "{mode:?}/{sample:?}, erase {}, lock {lock}", brush.erase);
                            }
                            renderer.finish();
                            reference.finish();
                            renderer.composite(&pre, &mut actual, sel, lock, true);
                            reference.composite_reference(&pre, &mut expected, sel, lock, true);
                            assert_eq!(actual.read_region(area), expected.read_region(area), "finished {mode:?}/{sample:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn large_byte_tile_compositor_matches_the_oracle_on_worker_tiles() {
    for fmt in [PixelFormat::RGBA8, PixelFormat::GRAYA8, PixelFormat::CMYKA8] {
        for erase in [false, true] {
            for lock in [false, true] {
                let brush = BrushSettings { size: 1500.0, hardness: 0.0, erase, pressure_size: false, color: [0.3, 0.6, 0.9, 0.7], ..Default::default() };
                let pre = Surface::with_default(fmt, &photocraft_raster::from_rgba(&fmt, [0.2, 0.3, 0.4, 0.8]));
                let mut renderer = StrokeRenderer::new(&brush, Some(fmt), 1.0);
                renderer.push(&[StrokePoint::new(-1.5, 62.25, 1.0)]);
                renderer.finish();
                assert!(renderer.cov.tiles.len() >= 1024, "exercises the parallel threshold");
                let mut reference = renderer.clone();
                let (mut actual, mut expected) = (pre.clone(), pre.clone());
                let area = renderer.bounds();
                assert_eq!(renderer.composite(&pre, &mut actual, None, lock, true), reference.composite_reference(&pre, &mut expected, None, lock, true));
                assert_eq!(actual.read_region(area), expected.read_region(area), "{fmt:?}, erase {erase}, lock {lock}");
            }
        }
    }
}

#[test]
fn preview_restore_preserves_edges_and_shares_whole_original_tiles() {
    use photocraft_geom::TileCoord;
    let region = Rect::new(-17, -7, 513, 529);
    let area = region.inflate(30);
    for fmt in [PixelFormat::RGBA8, PixelFormat::RGBA16, PixelFormat::RGBA32F, PixelFormat::CMYKA8, PixelFormat::GRAY8] {
        let mut pre = Surface::with_default(fmt, &photocraft_raster::from_rgba(&fmt, [0.2, 0.3, 0.4, 0.1]));
        pre.fill_rect(Rect::new(-13, -9, 411, 499), &photocraft_raster::from_rgba(&fmt, [0.7, 0.1, 0.6, 0.8]));
        let mut actual = pre.clone();
        actual.fill_rect(area, &photocraft_raster::from_rgba(&fmt, [0.9, 0.8, 0.7, 0.6]));
        let mut expected = actual.clone();
        expected.write_region(region, &pre.read_region(region));
        restore_region(&pre, &mut actual, region);
        assert_eq!(actual.read_region(area), expected.read_region(area), "{fmt:?}");
        let coord = TileCoord::new(0, 0);
        assert!(Arc::ptr_eq(actual.tile(coord).unwrap(), pre.tile(coord).unwrap()));
        // Restoring an absent original tile removes its preview allocation entirely.
        let missing = TileCoord::new(10, 10);
        actual.fill_rect(missing.rect(), &vec![1.0; fmt.channels()]);
        restore_region(&pre, &mut actual, missing.rect());
        assert!(actual.tile(missing).is_none());
        assert_eq!(actual.pixel(2560, 2560), pre.default_pixel());
    }
}
