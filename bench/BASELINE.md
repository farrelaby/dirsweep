# Benchmark baseline — pre-rewrite

Recorded **2026-10-14** from commit `cffe51b` + the `core-bench` branch work
(instrumentation and a `Clone` derive only; no scan logic changed).

This is the reference measurement for the planned core update. Re-run the same
benchmarks afterwards and compare to answer one question: *did the update make
it faster?*

Produced by `benches/scan.rs` — see "Benchmarking the core" in the README for
the harness itself.

> **Read "Machine drift" below before comparing anything.** Two runs of
> identical code on this machine differed by **1.87×**. A baseline saved in a
> different power state will report a false speedup across the board.

---

## Environment

Benchmarks are only comparable on the same machine under similar conditions.
Anything below changing means the numbers are no longer directly comparable.

| | |
|---|---|
| **CPU** | AMD Ryzen 5 PRO 4650U — 6 cores / 12 threads, boost 4.0 GHz |
| **RAM** | 14 GB |
| **OS** | Fedora Linux 44 (kernel 7.2.9-201.fc44.x86_64) |
| **Filesystem** | btrfs on `/dev/nvme0n1p3` |
| **CPU governor** | `schedutil` |
| **Toolchain** | rustc / cargo 1.95.0 |

### Dependency versions

| Crate | Version |
|---|---|
| criterion | 0.8.2 |
| walkdir | 2.5.0 |
| ratatui | 0.30.2 |
| chrono | 0.4.45 |

### Build configuration

```toml
[profile.bench]
opt-level = 3
# inherits from [profile.release]: lto = "thin", codegen-units = 1
```

The shipped binary is `opt-level = "z"`; benches are **not**. Both sides of any
comparison use the bench profile, so results isolate algorithmic change from
optimizer behaviour — but they are not a measurement of the binary you ship.

---

## Machine drift

**This is a laptop with a `schedutil` governor, and it dominates error bars.**
Two full runs of byte-identical code and byte-identical fixtures, roughly an
hour apart:

| benchmark | run 1 | run 2 | ratio |
|---|---|---|---|
| `pipeline/many-tiny` | 64.917 ms | 34.758 ms | 1.867× |
| `pipeline/deep-nesting` | 8.551 ms | 4.505 ms | 1.898× |
| `pipeline/node-monorepo` | 1.572 ms | 829.03 us | 1.896× |
| `scan/many-tiny` | 25.172 ms | 13.593 ms | 1.852× |
| `size/many-tiny` | 42.134 ms | 22.462 ms | 1.876× |
| `render/many-tiny` | 710.42 us | 371.96 us | 1.910× |
| `render/dense-targets` | 216.59 us | 112.60 us | 1.924× |

Two things to take from this:

1. **The scaling is uniform** (~1.85–1.92× across every phase and every
   scenario). A uniform factor is clock frequency and thermal state, not
   algorithmic change. That is the signature to look for when validating a
   future comparison — if *everything* moved by the same amount, nothing did.
2. **Criterion's confidence intervals are misleadingly tight** (±0.3% within a
   run, while runs differ by 87%). They measure repeatability *within* one
   power state, not reproducibility *across* them.

**The numbers recorded below are from run 2**, which is the one that includes
the `build_tree` group.

### Correct comparison procedure

Save the baseline and run the comparison **back to back in the same session**,
without a reboot or a long idle in between:

```bash
git checkout main            && cargo bench --bench scan -- --save-baseline before
git checkout <update-branch> && cargo bench --bench scan -- --baseline before
```

Do not reuse a `before` baseline saved hours or days earlier — you will measure
the machine, not the change.

---

## How these numbers were produced

```bash
cargo bench --bench scan -- --save-baseline before
```

- Fixtures are **deterministic** (seeded PRNG), so every run rebuilds the
  identical trees. Confirmed: the work counters were byte-identical across both
  runs above.
- Default criterion settings: 100 samples, 3 s warm-up, 5 s measurement per
  benchmark.
- Times are the **mean** with a 95% confidence interval.
- Fixture trees under `bench/fixtures/core-bench/` are rebuilt from scratch on
  every run.

> **Keep `target/criterion/` until the comparison is done.** That directory
> holds the machine-readable `before` baseline that `--baseline before` reads,
> and `cargo clean` deletes it. Either avoid cleaning, or preserve it now:
>
> ```bash
> cp -r target/criterion /tmp/criterion-before
> ```
>
> `bench/BASELINE.md` and `bench/baseline.json` are the durable human-readable
> copies; the JSON under `target/criterion/` is what criterion actually
> compares against.

---

## Results

### pipeline — scan → build_tree → first rendered frame

The headline number: roughly what the user waits for before the TUI responds.

| scenario | mean | 95% CI |
|---|---|---|
| `dense-targets` | 204.44 us | 204.06 us – 204.87 us |
| `multi-lang` | 819.49 us | 818.06 us – 821.00 us |
| `node-monorepo` | 829.03 us | 824.77 us – 834.78 us |
| `deep-nesting` | 4.505 ms | 4.493 ms – 4.520 ms |
| `many-tiny` | **34.758 ms** | 34.677 ms – 34.846 ms |

### scan — discovery + target assignment

| scenario | mean | 95% CI |
|---|---|---|
| `dense-targets` | 85.11 us | 84.80 us – 85.45 us |
| `node-monorepo` | 567.16 us | 566.21 us – 568.19 us |
| `multi-lang` | 600.92 us | 599.31 us – 602.76 us |
| `deep-nesting` | 3.188 ms | 3.178 ms – 3.201 ms |
| `many-tiny` | **13.593 ms** | 13.539 ms – 13.649 ms |

### build_tree — flattening scan output into the render tree

| scenario | mean | 95% CI |
|---|---|---|
| `dense-targets` | 1.52 us | 1.51 us – 1.52 us |
| `multi-lang` | 19.30 us | 19.25 us – 19.35 us |
| `node-monorepo` | 45.66 us | 45.53 us – 45.86 us |
| `deep-nesting` | 979.78 us | 977.96 us – 981.90 us |
| `many-tiny` | **20.110 ms** | 20.054 ms – 20.171 ms |

### size — `scan_target_size` across every target

| scenario | mean | 95% CI |
|---|---|---|
| `multi-lang` | 918.55 us | 917.28 us – 919.88 us |
| `node-monorepo` | 2.736 ms | 2.728 ms – 2.744 ms |
| `deep-nesting` | 5.247 ms | 5.231 ms – 5.264 ms |
| `dense-targets` | 11.027 ms | 11.011 ms – 11.048 ms |
| `many-tiny` | **22.462 ms** | 22.426 ms – 22.505 ms |

### render — `ui::render` into a 120x40 `TestBackend`, tree pre-built

| scenario | mean | 95% CI |
|---|---|---|
| `dense-targets` | 112.60 us | 112.30 us – 112.93 us |
| `multi-lang` | 187.63 us | 187.21 us – 188.07 us |
| `node-monorepo` | 207.25 us | 206.84 us – 207.71 us |
| `deep-nesting` | 259.25 us | 258.41 us – 260.21 us |
| `many-tiny` | 371.96 us | 367.54 us – 379.17 us |

Rendering is sub-millisecond everywhere. It is not worth optimising.

---

## Where the time actually goes

Share of `pipeline/many-tiny` (34.758 ms), the worst case:

| phase | time | share |
|---|---|---|
| `scan` | 13.593 ms | 39% |
| **`build_tree`** | **20.110 ms** | **58%** |
| `render` | 0.372 ms | 1% |

### The bottleneck is `build_tree`, not the scanner

`build_tree` does a linear scan of `target_dirs` for every project, so its cost
is quadratic in projects × targets:

```rust
let children: Vec<&ScannedDir> = output.target_dirs.iter()
    .filter(|d| project.children.contains(&d.path))   // O(targets), per project
    .collect();
```

| scenario | projects × targets | comparisons | `build_tree` |
|---|---|---|---|
| `dense-targets` | 3 × 3 | ~9 | 1.52 us |
| `multi-lang` | 15 × 20 | ~300 | 19.30 us |
| `node-monorepo` | 21 × 22 | ~462 | 45.66 us |
| `deep-nesting` | 110 × 110 | ~12,100 | 979.78 us |
| `many-tiny` | 500 × 500 | ~250,000 | 20.110 ms |

Work grows **27,000×** from `dense-targets` to `many-tiny`; time grows
**13,230×**. `node-monorepo` → `many-tiny` is 541× the comparisons and 440× the
time. Quadratic, and confirmed directly rather than by subtraction — an earlier
run measured it as 39.035 ms by subtraction against a 1.87× faster clock, which
rescales to 20.9 ms. The two methods agree.

Fixing this is likely the single largest available win, and it does not require
touching the walker at all — a `HashMap<PathBuf, _>` over `project.children`
turns it linear.

Note `size` is a *separate* phase from `pipeline`: the TUI computes sizes on a
background worker after the first frame, so `size` costs the user nothing
upfront. It is still the largest single cost overall, and the one a parallelised
worker would attack.

### How `scan` divides between its passes

`ScanStats` gained `pass1_us` / `project_build_us` / `pass2_us` to answer one
question: a parallel walker replaces **pass 1 only** — pass 2 (`find_target_dirs`)
is hand-rolled `fs::read_dir` recursion that no walker crate touches. Choosing a
walker without this split would risk adding a dependency that speeds up the
minority of the work.

Mean of 20 scans per fixture:

| scenario | pass1 | build | pass2 | sum | pass1 % of `scan` | pass1 % of `pipeline` |
|---|---|---|---|---|---|---|
| node-monorepo | 330 us | 12 us | 221 us | 563 us | **58.6%** | 40.4% |
| multi-lang | 339 us | 4 us | 254 us | 597 us | **56.8%** | 41.1% |
| many-tiny | 7,858 us | 167 us | 5,401 us | 13,426 us | **58.5%** | 22.9% |
| deep-nesting | 1,918 us | 132 us | 1,166 us | 3,216 us | **59.6%** | 43.0% |
| dense-targets | 53 us | 0 us | 28 us | 81 us | **65.4%** | 26.2% |

**Pass 1 is 57–65% of `scan` in every scenario** — consistent, not a quirk of
one fixture. The gate for a parallel walker passes.

**The instrumentation is accurate.** `sum` covers 95–100% of the independently
measured `scan` (e.g. `many-tiny` 13,426 us against `scan` at 13,574 us), so the
three marks bracket nearly all the work and are not double-counting.

**The instrumentation is free.** Re-running every benchmark with the marks added
came within **±2%** of the recorded baseline (`pipeline/many-tiny` 34.335 ms vs
34.758 ms, `scan/many-tiny` 13.574 vs 13.593). That also confirms the baseline
is reproducible in a back-to-back session — unlike the 1.87× drift seen between
sessions an hour apart.

**Ceiling:** even a walker that made pass 1 cost nothing could only take
`pipeline` down by 23–43%, depending on scenario. On `many-tiny` the
`build_tree` fix (58%) is worth more than twice as much as the walker; on the
other four, the walker is the larger prize. They are complementary, not
competing.

---

## Work counters

Wall-clock time on a laptop is noisy; work done is not. These counters change
only if the algorithm changes, so they are the most reliable before/after signal.
They were **byte-identical across both recorded runs**.

| scenario | targets | files | disc_d | disc_f | pruned | detect | tgt_d |
|---|---|---|---|---|---|---|---|
| node-monorepo | 22 | 194 | 22 | 21 | 22 | 21 | 21 |
| multi-lang | 20 | 100 | 26 | 30 | 15 | 20 | 25 |
| many-tiny | 500 | 1500 | 501 | 500 | 500 | 500 | 500 |
| deep-nesting | 110 | 340 | 126 | 110 | 110 | 110 | 110 |
| dense-targets | 3 | 753 | 4 | 3 | 3 | 3 | 3 |

- `disc_d` / `disc_f` — directories and files entered during discovery
- `pruned` — subtrees skipped by `filter_entry` (VCS dirs, known targets)
- `detect` — detection files matched (lock files, manifests)
- `tgt_d` — directories opened while looking for targets

`dense-targets` makes the pruning strategy visible: 753 files exist on disk, but
discovery enters only 4 directories and sees 3 files, because `node_modules` is
pruned rather than walked. Sizing still visits all 753 — hence `size` at
11.0 ms against `scan` at 85 us for the same scenario.

---

## Comparing after the update

```bash
git checkout <update-branch>
cargo bench --bench scan -- --baseline-lenient before
```

Criterion prints a significance test per benchmark — `regressed`, `improved` or
`no change` — rather than a bare percentage. Also diff the work counters table
printed at the start of the run against the one above.

Things to watch for:

- **Validate the comparison first.** If every benchmark moved by roughly the
  same factor, you measured the machine, not the change. Trust only results
  where some benchmarks moved and others did not.
- **`build_tree/many-tiny` is the number that matters most** — 58% of pipeline
  time in the worst case, and quadratic. A scanner-only rewrite will barely move
  `pipeline/many-tiny`.
- **`build_tree/many-tiny` should scale quadratically with the fixture.** If the
  rewrite makes it linear, expect a large drop here and almost nothing elsewhere.
- **Counters are the tiebreaker.** If `disc_d` drops but time does not, the win
  is being spent elsewhere; if time drops with no counter change, suspect the
  machine.
- **`size/*` is unaffected by a `build_tree` fix** — it is a different phase on
  a different thread. Use it as a control: if `size` also "improves", that part
  is drift.
- **`size/*` is sequential by design; `size_par/*` is the parallel fan-out.**
  They run in the same session, so `size / size_par` is a drift-immune measure
  of the rayon speedup — far more trustworthy than either against a baseline
  recorded in another session.

---

## Rewrite progress

Measured against the baseline above, each phase run back-to-back in one session
with `--baseline-lenient before`.

### Phase A — per-phase timing (`pass1_us` / `project_build_us` / `pass2_us`)

Not a performance change; instrumentation. `scan` divides into:

| scenario | pass 1 (walker) | build | pass 2 (read_dir) | **pass 1 % of `scan`** |
|---|---|---|---|---|
| node-monorepo | 330 us | 12 us | 221 us | 58.6% |
| multi-lang | 339 us | 4 us | 254 us | 56.8% |
| many-tiny | 7,858 us | 167 us | 5,401 us | 58.5% |
| deep-nesting | 1,918 us | 132 us | 1,166 us | 59.6% |
| dense-targets | 53 us | 0 us | 28 us | 65.4% |

Pass 1 is 57–65% of `scan` everywhere, so a parallel walker targets the
majority of the scan. The three marks sum to 95–100% of `scan`, and adding them
moved every benchmark by at most 2%.

### Phase B — `build_tree` indexed lookup

`HashMap<PathBuf, &ScannedDir>` built once, children looked up instead of
rescanning `target_dirs` per project: O(projects × targets) → O(projects + targets).

| benchmark | before | after | change |
|---|---|---|---|
| `build_tree/many-tiny` | 20.110 ms | 0.440 ms | **−97.8%** |
| `build_tree/deep-nesting` | 0.980 ms | 0.101 ms | −89.7% |
| `build_tree/node-monorepo` | 0.0457 ms | 0.0188 ms | −58.8% |
| `build_tree/multi-lang` | 0.0193 ms | 0.0133 ms | −31.1% |
| `build_tree/dense-targets` | 0.0015 ms | 0.0019 ms | +25.4% |
| **`pipeline/many-tiny`** | **34.758 ms** | **14.745 ms** | **−57.6%** |
| `pipeline/deep-nesting` | 4.505 ms | 3.624 ms | −19.6% |

`dense-targets` regresses because hashing beats scanning nine elements by less
than building the map costs — 1.9 us, irrelevant.

Attribution is exact: `deep-nesting` pipeline fell 889 us while its `build_tree`
fell 870 us. `scan`, `size` and `render` all stayed within 3% and the work
counters were byte-identical.

### Phase C — rayon size worker

`target_paths.into_par_iter()` replacing the sequential loop in `main.rs`, plus
a `size_par` benchmark group so the harness actually exercises the parallel path.

| scenario | sequential | parallel | speedup |
|---|---|---|---|
| many-tiny (500 targets) | 22.712 ms | 3.793 ms | **5.99×** |
| deep-nesting (110) | 5.214 ms | 0.969 ms | **5.38×** |
| node-monorepo (22) | 2.726 ms | 0.650 ms | **4.19×** |
| multi-lang (20) | 0.920 ms | 0.267 ms | **3.44×** |
| dense-targets (3) | 10.842 ms | 7.669 ms | 1.41× |

These are **within a single run**, so drift cannot explain them. Speedup tracks
target count up to ~6×, which is this machine's physical core count — the
ceiling is task availability, not the thread pool. `dense-targets` has only
three targets, so three-way is the most it can reach and load imbalance on the
largest tree pulls it to 1.41×.

The sequential control held at −1.7% to +1.1% against the recorded baseline,
confirming the gain is rayon rather than the clock.

### Summary

| phase | moves | controls held |
|---|---|---|
| A instrumentation | nothing (by design) | all benchmarks within 2% |
| B `build_tree` index | `pipeline/many-tiny` −57.6% | `scan`, `size`, `render` within 3% |
| C rayon size | `size` 3.4–6.0× faster | `pipeline`, `scan`, `build_tree` unchanged |
