# Large brush and symmetry latency

Measured 2026-10-20 against `872748fb` on Linux x86_64, with a four-CPU quota,
Rust 1.99.1 and the workspace release profile. These are CPU preview updates,
not end-to-end display latency or measurements on Windows or in a browser.

## What changed

- Large dabs rasterize into coverage tiles instead of a full tip-sized float scratch
  image. A 5000 px soft tip previously needed about 285 MiB for that scratch image.
- Coverage snapshots share immutable tiles and copy only modified tiles. Symmetry
  merges no longer deep-copy every unchanged coverage array in a preview.
- Single-pass painting uses the same incremental merged-tail preview as symmetry.
  It redraws affected tiles once rather than restoring a large bounding rectangle,
  compositing the base stroke, then compositing its temporary smoothing tail again.
  The old restore could allocate about 1.11 GiB of RGBA floats for a 5000 px soft tip.
- Native builds rasterize large dabs and composite large byte-tile batches in parallel.
  WebAssembly retains the serial path. Small updates avoid Rayon scheduling overhead.
- The common alpha-bearing 8-bit Normal path blends directly into native byte tiles;
  other depths, modes and per-dab color keep the generic compositor. The arithmetic,
  dither, off-canvas content, selection and transparency-lock behavior are preserved.
- Preview restoration shares whole original surface tiles and copies edge bytes,
  without a dense float intermediate.

## Measurements

The benchmark starts with an opaque solid-colored RGBA8 layer (shared filled tiles),
uses a 5000 px soft brush/eraser,
10% smoothing and three input events. Mirroring uses a 200 px soft brush and
120 events, timestamped 8 ms apart. The default canvas runs twice (six large-stroke
samples); the 24 MP canvas runs once. With only three or six large-stroke samples,
the reported maximum matters more than an inferred latency distribution.

Times below are milliseconds. The p50 uses nearest-rank, as `cargo xtask perf` does.

| Scenario | Canvas | p50 before → after | Worst before → after |
|---|---|---:|---:|
| large brush | 1920x1080 | 2187.62 → 591.84 | 3426.67 → 1872.57 |
| large eraser | 1920x1080 | 2366.55 → 290.37 | 3045.46 → 363.66 |
| rapid symmetry | 1920x1080 | 10.47 → 8.40 | 47.75 → 16.63 |
| large brush | 6000x4000 | 11284.52 → 1017.79 | 18181.42 → 1151.66 |
| large eraser | 6000x4000 | 7950.43 → 614.62 | 10713.64 → 709.35 |
| rapid symmetry | 6000x4000 | 23.63 → 15.16 | 66.32 → 28.23 |

On the default canvas, symmetry p95 changes from 17.41 to 13.81 ms. A separate
single-thread native run measures p50 1198.48 ms for the brush, 738.78 ms for the
eraser and 7.53 ms for symmetry. This exercises the serial algorithm used by WebAssembly;
actual browser and Windows latency still require their respective runtimes.

These changes reduce the reproduced stalls; they do not establish a 16 ms budget
for a 5000 px brush. Large strokes can still take hundreds of milliseconds or more,
and layer effects, canvas composition and display upload are outside this harness.
Brush latency therefore stays **partial**, and P62–P64 remain unenforced targets.
The existing performance baseline and parity percentages are unchanged.

Raw samples and context: [brush-performance-2026-10-20.json](../perf/brush-performance-2026-10-20.json).

## Reproduce and monitor

```sh
cargo run --release --locked -p photocraft-paint --example bench_interactive_brush -- --quick --json target/brush-default.json
cargo run --release --locked -p photocraft-paint --example bench_interactive_brush -- --json target/brush-24mp.json
RAYON_NUM_THREADS=1 cargo run --release --locked -p photocraft-paint --example bench_interactive_brush -- --quick --json target/brush-serial.json
cargo xtask perf --quick --bench bench_interactive_brush
```

The checked-in benchmark follows the current `LiveStroke` preview path. Before
measurements used the original rasterizer/compositor and the original single-pass
restore/base/tail sequence with the same input trace and release profile. Runs
were sequential, without concurrent Cargo builds; normal runner noise remains.

`brush-performance.yml` runs paint pixel/allocation regressions, live-stroke tests,
Clippy, WebAssembly compilation and native/serial release samples on Linux and
Windows. Measurements are uploaded as artifacts. Portable correctness and storage
invariants gate CI; noisy wall-clock samples are reported rather than used as a
universal hardware threshold. The existing main CI and release workflows continue
to provide their broader checks and release process.
