//! Phase-level benchmarks for the scan core.
//!
//! The pre-existing `bench/bench.sh` measures dirsweep end-to-end through a
//! PTY against npkill. That is the right instrument for the README comparison
//! but the wrong one for a core rewrite: it collapses startup, discovery,
//! target assignment, tree assembly and the first render into a single number,
//! so a regression in one phase is invisible behind a win in another.
//!
//! These benchmarks split the pipeline into its parts and add the one phase
//! `bench.sh` never measures at all — size computation, which is fire-and-forget
//! in the TUI and therefore discarded before it completes.
//!
//! ## Workflow
//!
//! ```text
//! git checkout main        && cargo bench -- --save-baseline before
//! git checkout <rewrite>   && cargo bench -- --baseline-lenient before
//! ```
//!
//! `--baseline-lenient` rather than `--baseline` on purpose: the strict form
//! panics the whole run if *any* benchmark is missing from the named baseline,
//! so adding a group (as `size_par` was added) would break every subsequent
//! comparison. Lenient compares what exists and skips only what does not.
//!
//! Criterion prints a per-benchmark significance test (regressed / improved /
//! no change) rather than a raw percentage.
//!
//! ## Profiles
//!
//! `[profile.bench]` sets `opt-level = 3` while the shipped binary is built at
//! `opt-level = "z"`. Both the baseline and the rewrite are measured under the
//! bench profile, so the comparison is internally consistent — it isolates
//! algorithmic change from optimizer behaviour. See `Cargo.toml`.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use ratatui::{Terminal, backend::TestBackend};
use rayon::prelude::*;

use dirsweep::app::{AppPhase, AppState};
use dirsweep::scanner;

mod support;
use support::fixtures::{self, Fixture};

/// Terminal size used for the render and pipeline benchmarks. Wide enough that
/// `ui::render_browsing` takes its column-aligned branch.
const TERM_W: u16 = 120;
const TERM_H: u16 = 40;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench/fixtures/core-bench")
}

/// Discovery walk + target assignment + tree assembly + first frame.
///
/// The headline number: this is roughly what the user waits for before the TUI
/// becomes interactive.
fn bench_pipeline(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("pipeline");

    for f in fixtures {
        let mut terminal = Terminal::new(TestBackend::new(TERM_W, TERM_H)).expect("terminal");
        group.throughput(Throughput::Elements(f.expected_files as u64));
        group.bench_with_input(BenchmarkId::from_parameter(f.name), f, |b, f| {
            b.iter(|| {
                let mut state = AppState::new(f.dir.clone());
                state.build_tree(scanner::scan(black_box(&f.dir)));
                state.phase = AppPhase::Browsing;
                terminal
                    .draw(|frame| dirsweep::ui::render(&mut state, frame))
                    .expect("draw");
                black_box(state);
            });
        });
    }

    group.finish();
}

/// `scanner::scan` on its own — discovery and target assignment, no sizing.
fn bench_scan(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("scan");

    for f in fixtures {
        group.throughput(Throughput::Elements(f.expected_files as u64));
        group.bench_with_input(BenchmarkId::from_parameter(f.name), f, |b, f| {
            b.iter(|| black_box(scanner::scan(black_box(&f.dir))));
        });
    }

    group.finish();
}

/// `AppState::build_tree` — flattening scan output into the render tree.
///
/// Isolated because `pipeline` hides it and it is the phase that dominates at
/// scale. `build_tree` does a linear scan of `target_dirs` for every project,
/// so its cost is quadratic in projects x targets: ~87 us at 21x22 but ~39 ms
/// at 500x500.
///
/// The scan runs once, outside the timed region; each iteration clones the
/// `ScanOutput` (setup, excluded by `iter_batched`) and builds a fresh tree.
fn bench_build_tree(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("build_tree");

    for f in fixtures {
        let output = scanner::scan(&f.dir);
        group.throughput(Throughput::Elements(output.target_dirs.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(f.name), &output, |b, output| {
            b.iter_batched(
                || (AppState::new(f.dir.clone()), output.clone()),
                |(mut state, out)| {
                    state.build_tree(out);
                    state
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Total sequential work to size every target.
///
/// The size worker in `main.rs` fans this out across rayon, so this is no
/// longer what the user waits for — it is the *control*. Kept sequential on
/// purpose: `scan_target_size` itself did not change, so this should stay flat
/// across the rewrite and expose machine drift, while `size_par` moves.
///
/// Private to `main.rs`, so it cannot be called directly; this mirrors it.
fn bench_size(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("size");

    for f in fixtures {
        let targets: Vec<PathBuf> = scanner::scan(&f.dir)
            .target_dirs
            .iter()
            .map(|d| d.path.clone())
            .collect();

        group.throughput(Throughput::Elements(targets.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(f.name),
            &targets,
            |b, targets| {
                b.iter(|| {
                    let mut total = 0u64;
                    for path in targets {
                        if let Ok((size, _)) = scanner::scan_target_size(black_box(path)) {
                            total += size;
                        }
                    }
                    black_box(total)
                });
            },
        );
    }

    group.finish();
}

/// Time to compute *every* size, fanned out across rayon — the number the user
/// actually waits on, since the TUI fills in sizes as they arrive.
///
/// Deliberately measured alongside the sequential `size` group rather than
/// replacing it: running both in the same session gives the parallel speedup
/// directly, immune to the ~1.9x clock drift this machine shows *between*
/// sessions. Rayon's pool is process-wide and persists across criterion's
/// iterations, so per-iteration cost is stable.
///
/// The per-target channel send in `main.rs` is not mirrored — it needs a
/// draining receiver to model, and the cost is a single unbounded mpsc send per
/// target, which is not what this group is trying to measure.
fn bench_size_parallel(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("size_par");

    for f in fixtures {
        let targets: Vec<PathBuf> = scanner::scan(&f.dir)
            .target_dirs
            .iter()
            .map(|d| d.path.clone())
            .collect();

        group.throughput(Throughput::Elements(targets.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(f.name),
            &targets,
            |b, targets| {
                b.iter(|| {
                    let total: u64 = targets
                        .par_iter()
                        .map(|path| match scanner::scan_target_size(black_box(path)) {
                            Ok((size, _)) => size,
                            Err(_) => 0,
                        })
                        .sum();
                    black_box(total)
                });
            },
        );
    }

    group.finish();
}

/// First frame rendered into a fixed-size `TestBackend`.
///
/// The tree is built once, outside the timed region, so this measures the
/// rendering layer alone.
fn bench_render(c: &mut Criterion, fixtures: &[Fixture]) {
    let mut group = c.benchmark_group("render");
    group.throughput(Throughput::Elements(u64::from(TERM_W) * u64::from(TERM_H)));

    for f in fixtures {
        group.bench_with_input(BenchmarkId::from_parameter(f.name), f, |b, f| {
            let mut state = AppState::new(f.dir.clone());
            state.build_tree(scanner::scan(&f.dir));
            state.phase = AppPhase::Browsing;

            let mut terminal = Terminal::new(TestBackend::new(TERM_W, TERM_H)).expect("terminal");

            b.iter(|| {
                terminal
                    .draw(|frame| dirsweep::ui::render(&mut state, frame))
                    .expect("draw");
            });
        });
    }

    group.finish();
}

/// Print the work counters alongside the timings.
///
/// Wall-clock time on a loaded machine is noisy; work done is not. If the
/// rewrite cuts `discovery_dirs` by a third, that is a real result even when
/// the timer cannot resolve it.
fn report_stats(fixtures: &[Fixture]) {
    println!();
    println!("── scan work counters ─────────────────────────────────────────────");
    println!(
        "{:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "scenario", "targets", "files", "disc_d", "disc_f", "pruned", "detect", "tgt_d"
    );

    for f in fixtures {
        let s = scanner::scan(&f.dir).stats;
        println!(
            "{:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            f.name,
            f.expected_targets,
            f.expected_files,
            s.discovery_dirs,
            s.discovery_files,
            s.pruned_subtrees,
            s.detection_hits,
            s.target_walk_dirs,
        );
    }

    println!();
    println!("columns: disc_d/disc_f = directories/files entered during discovery,");
    println!("         pruned   = subtrees skipped, detect = detection files matched,");
    println!("         tgt_d    = directories opened while looking for targets");
    println!();

    report_pass_split(fixtures);
}

/// Print how `scan`'s wall time divides between its three phases.
///
/// This is the input to choosing a parallelism strategy: a parallel walker
/// only replaces pass 1, while pass 2 is hand-rolled recursion that a walker
/// never touches. Getting that backwards would add a dependency for no gain.
///
/// A single scan is too short to time reliably — `dense-targets` scans in
/// ~85µs — so this averages repeated scans. Unlike the counters above, these
/// numbers are wall-clock and will differ between runs.
fn report_pass_split(fixtures: &[Fixture]) {
    const SAMPLES: u64 = 20;

    println!(
        "── scan pass split (mean of {} scans, µs) ──────────────────────────",
        SAMPLES
    );
    println!(
        "{:<16} {:>10} {:>10} {:>10} {:>10} {:>7} {:>7}",
        "scenario", "pass1_us", "build_us", "pass2_us", "sum_us", "p1%", "p2%"
    );

    for f in fixtures {
        let mut p1 = 0u64;
        let mut build = 0u64;
        let mut p2 = 0u64;

        for _ in 0..SAMPLES {
            let s = scanner::scan(&f.dir).stats;
            p1 += s.pass1_us;
            build += s.project_build_us;
            p2 += s.pass2_us;
        }

        let p1 = p1 / SAMPLES;
        let build = build / SAMPLES;
        let p2 = p2 / SAMPLES;
        let total = p1 + build + p2;
        let pct = |v: u64| {
            if total > 0 {
                v as f64 * 100.0 / total as f64
            } else {
                0.0
            }
        };

        println!(
            "{:<16} {:>10} {:>10} {:>10} {:>10} {:>6.1}% {:>6.1}%",
            f.name,
            p1,
            build,
            p2,
            total,
            pct(p1),
            pct(p2),
        );
    }

    println!();
    println!("pass1 = discovery walk (walkdir), build = detection hits -> projects,");
    println!("        pass2 = target claiming (hand-rolled read_dir recursion)");
    println!();
}

fn main() {
    let root = fixture_root();
    println!("building fixtures under {}", root.display());
    let built = fixtures::build_all(&root);
    println!("  {} scenarios ready", built.len());

    report_stats(&built);

    let mut criterion = Criterion::default().configure_from_args();

    bench_pipeline(&mut criterion, &built);
    bench_scan(&mut criterion, &built);
    bench_build_tree(&mut criterion, &built);
    bench_size(&mut criterion, &built);
    bench_size_parallel(&mut criterion, &built);
    bench_render(&mut criterion, &built);

    criterion.final_summary();
}
