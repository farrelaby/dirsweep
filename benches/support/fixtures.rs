//! Deterministic on-disk fixtures for the scan benchmarks.
//!
//! `bench/bench.sh` seeds its fixtures with `$RANDOM`, so two runs measure
//! different trees and any before/after delta is partly noise. These
//! generators use a fixed seed instead: the same scenario always produces the
//! same tree, which is what makes a core-rewrite comparison meaningful.
//!
//! Fixtures are built once per process and reused across every benchmark
//! iteration. `scan` never mutates them, so repeated runs are safe.
//!
//! Files are created sparse — they report their full size through
//! `metadata.len()` but cost no disk I/O, matching `truncate -s` in
//! `bench/bench.sh`. Note the consequence: the size walk's cost scales with
//! file *count*, not bytes, so the scenarios that stress it create many small
//! files rather than few large ones.

use std::fs;
use std::path::{Path, PathBuf};

/// xorshift64*. Deliberately not the `rand` crate — pulling a PRNG dependency
/// in just to pick a handful of directory sizes would dwarf the code it
/// serves.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Uniform in `[lo, hi]`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

const KB: u64 = 1024;

/// A generated scenario on disk.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Human-readable scenario name, used as the benchmark ID.
    pub name: &'static str,
    pub dir: PathBuf,
    /// Target directories the scan is expected to find.
    pub expected_targets: usize,
    /// Files created anywhere under `dir`. Drives the size-walk benchmark.
    pub expected_files: usize,
}

/// Create a sparse file of `bytes`, returning 1 as a file count.
fn sparse(path: &Path, bytes: u64) -> usize {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    let file = fs::File::create(path).expect("create file");
    file.set_len(bytes).expect("set file length");
    1
}

fn empty_file(path: &Path) -> usize {
    sparse(path, 0)
}

/// A JS project marker. `package-lock.json` is a `js` detection file.
fn js_project(project: &Path) -> usize {
    empty_file(&project.join("package-lock.json"))
}

/// A Rust project marker. Both `Cargo.toml` and `Cargo.lock` detect as rust.
fn rust_project(project: &Path) -> usize {
    empty_file(&project.join("Cargo.toml")) + empty_file(&project.join("Cargo.lock"))
}

/// A Python project marker.
fn python_project(project: &Path) -> usize {
    empty_file(&project.join("pyproject.toml"))
}

/// A `node_modules` tree holding `count` sparse files across nested packages.
fn node_modules(project: &Path, rng: &mut Rng, count: u64) -> usize {
    let mut files = 0;
    for i in 0..count {
        let kb = rng.range(1, 8);
        files += sparse(
            &project.join(format!("node_modules/pkg-{i:03}/index.js")),
            kb * KB,
        );
    }
    files
}

/// A Rust `target` tree.
fn rust_target(project: &Path, rng: &mut Rng, count: u64) -> usize {
    let mut files = 0;
    for i in 0..count {
        let kb = rng.range(2, 16);
        files += sparse(
            &project.join(format!("target/debug/deps/mod-{i:03}.rlib")),
            kb * KB,
        );
    }
    files
}

/// Python junk dirs. Both `__pycache__` and `.venv` are *deep* targets, so
/// they are found recursively rather than only at the project root.
fn python_junk(project: &Path, rng: &mut Rng) -> usize {
    let mut files = 0;
    for i in 0..2 {
        files += sparse(
            &project.join(format!("src/pkg/mod-{i}.cpython-312.pyc")),
            2 * KB,
        );
    }
    for i in 0..4 {
        files += sparse(
            &project.join(format!(".venv/lib/python3.12/site-packages/pkg-{i}.py")),
            rng.range(1, 4) * KB,
        );
    }
    files
}

/// Root JS project with a workspace of packages, plus a `.next` build dir.
///
/// Mirrors `setup_1` in `bench/bench.sh`. Targets: root `node_modules`,
/// root `.next`, and one `node_modules` per package — 22 in total.
fn scenario_node_monorepo(root: &Path) -> Fixture {
    let mut rng = Rng::new(1_000_001);
    let mut files = 0;

    files += js_project(root);
    files += node_modules(root, &mut rng, 8);
    for i in 0..6 {
        files += sparse(&root.join(format!(".next/cache/chunk-{i}.js")), 15 * KB);
    }

    for i in 0..20 {
        let pkg = root.join(format!("packages/pkg-{i:02}"));
        files += empty_file(&pkg.join("yarn.lock"));
        let nm_files = rng.range(3, 12);
        files += node_modules(&pkg, &mut rng, nm_files);
    }

    Fixture {
        name: "node-monorepo",
        dir: root.to_path_buf(),
        expected_targets: 22,
        expected_files: files,
    }
}

/// Five JS, five Rust and five Python projects, each with their own junk dirs.
///
/// Mirrors `setup_2`. Targets: 5 + 5 + (2 deep dirs x 5) = 20.
fn scenario_multi_lang(root: &Path) -> Fixture {
    let mut rng = Rng::new(1_000_002);
    let mut files = 0;

    for i in 0..5 {
        let p = root.join(format!("js-proj-{i}"));
        files += js_project(&p);
        files += node_modules(&p, &mut rng, 4);
    }
    for i in 0..5 {
        let p = root.join(format!("rust-proj-{i}"));
        files += rust_project(&p);
        files += rust_target(&p, &mut rng, 6);
    }
    for i in 0..5 {
        let p = root.join(format!("py-proj-{i}"));
        files += python_project(&p);
        files += python_junk(&p, &mut rng);
    }

    Fixture {
        name: "multi-lang",
        dir: root.to_path_buf(),
        expected_targets: 20,
        expected_files: files,
    }
}

/// 500 minimal JS projects. Mirrors `setup_3`.
///
/// This is the scenario that punishes per-directory overhead: 500 projects
/// means 500 detection-file hits and 500 target lookups, but almost no files.
fn scenario_many_tiny(root: &Path) -> Fixture {
    let mut rng = Rng::new(1_000_003);
    let mut files = 0;

    for i in 0..500 {
        let p = root.join(format!("project-{i:03}"));
        files += js_project(&p);
        files += node_modules(&p, &mut rng, 2);
    }

    Fixture {
        name: "many-tiny",
        dir: root.to_path_buf(),
        expected_targets: 500,
        expected_files: files,
    }
}

/// `prefix1/prefix2/.../prefixN`, deliberately with no leading separator.
///
/// `Path::join` silently discards its base when the argument is absolute, so a
/// stray `/` here would write to the filesystem root instead of the fixture.
fn nested_prefix(prefix: &str, depth: u64) -> String {
    (1..=depth)
        .map(|j| format!("{prefix}{j}"))
        .collect::<Vec<_>>()
        .join("/")
}

/// Nested projects at varying depths. Mirrors `setup_4`.
///
/// Ten projects each nested `i` levels deep, plus a hundred at a random depth.
fn scenario_deep_nesting(root: &Path) -> Fixture {
    let mut rng = Rng::new(1_000_004);
    let mut files = 0;

    for i in 1..=10 {
        // Note: `join` discards the base when handed an absolute path, so the
        // relative prefix must not start with a separator.
        let rel = nested_prefix("d", i);
        let p = root.join(format!("{rel}/proj-{i}"));
        files += js_project(&p);
        files += node_modules(&p, &mut rng, 3);
    }

    for i in 0..100 {
        let depth = rng.range(1, 5);
        let rel = nested_prefix("level", depth);
        let p = root.join(format!("{rel}/tiny-{i:03}"));
        files += js_project(&p);
        files += node_modules(&p, &mut rng, 2);
    }

    Fixture {
        name: "deep-nesting",
        dir: root.to_path_buf(),
        expected_targets: 110,
        expected_files: files,
    }
}

/// Three JS projects with densely populated `node_modules`.
///
/// The complement of `many-tiny`: few targets, many files. This is the
/// scenario where `scan_target_size` dominates, so it is the one that will
/// reveal whether a rewrite actually parallelises the size walk.
fn scenario_dense_targets(root: &Path) -> Fixture {
    let mut rng = Rng::new(1_000_005);
    let mut files = 0;

    for (name, count) in [("proj-a", 250), ("proj-b", 250), ("proj-c", 250)] {
        let p = root.join(name);
        files += js_project(&p);
        files += node_modules(&p, &mut rng, count);
    }

    Fixture {
        name: "dense-targets",
        dir: root.to_path_buf(),
        expected_targets: 3,
        expected_files: files,
    }
}

/// Scenario generator: a name plus the function that builds it.
type ScenarioBuilder = (&'static str, fn(&Path) -> Fixture);

/// Build every scenario under `root`, one subdirectory per scenario.
///
/// Called once per benchmark process, before any timing begins.
pub fn build_all(root: &Path) -> Vec<Fixture> {
    let builders: [ScenarioBuilder; 5] = [
        ("node-monorepo", scenario_node_monorepo),
        ("multi-lang", scenario_multi_lang),
        ("many-tiny", scenario_many_tiny),
        ("deep-nesting", scenario_deep_nesting),
        ("dense-targets", scenario_dense_targets),
    ];

    builders
        .iter()
        .map(|(name, build)| {
            let dir = root.join(name);
            if dir.exists() {
                fs::remove_dir_all(&dir).expect("clear stale fixture");
            }
            fs::create_dir_all(&dir).expect("create fixture dir");
            let fixture = build(&dir);
            // Belt and braces: a `join` fed an absolute path drops its base and
            // writes to the filesystem root. Catch that here rather than
            // benchmarking a tree that was never created.
            assert!(
                fixture.dir.starts_with(root),
                "fixture {} escaped its root: {:?}",
                fixture.name,
                fixture.dir
            );
            fixture
        })
        .collect()
}
