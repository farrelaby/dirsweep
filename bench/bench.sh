#!/usr/bin/env bash
set -euo pipefail

DIRSWEEP="${DIRSWEEP:-target/release/dirsweep}"
NPKILL="${NPKILL:-npkill}"
RUNS="${RUNS:-3}"
SCENARIOS="${SCENARIOS:-1 2 3 4 5}"
BENCH_DIR="${BENCH_DIR:-bench/fixtures}"

usage() {
    cat <<EOF
Usage: $0 [--prepare]

Benchmark dirsweep vs npkill on synthetic project directories.

  --prepare    Only create fixtures, skip benchmarking
  -h, --help   Show this help

Environment variables:
  DIRSWEEP    Path to dirsweep binary (default: target/release/dirsweep)
  NPKILL      npkill command (default: npkill)
  RUNS        Number of runs per scenario (default: 3)
  SCENARIOS   Space-separated scenario numbers (default: 1 2 3 4 5)
  BENCH_DIR   Directory for fixtures (default: bench/fixtures)
EOF
    exit 0
}

# ── Helpers ────────────────────────────────────────────────────────

mkproject() {
    local dir="$1" lock="$2"; mkdir -p "$dir"
    case "$lock" in
        js)     touch "$dir/package-lock.json" ;;
        js-yarn) touch "$dir/yarn.lock" ;;
        rust)   touch "$dir/Cargo.lock" "$dir/Cargo.toml" ;;
        python) touch "$dir/pyproject.toml" ;;
    esac
}

# Create a junk file at the given size (KB). Uses truncate for sparse
# files so no actual disk I/O — only directory structure matters for
# scan performance.
mkjunk() {
    local dir="$1" name="$2" size_kb="$3"
    mkdir -p "$(dirname "$dir/$name")"
    truncate -s "${size_kb}K" "$dir/$name"
}

mknode_modules() { local d="$1" s="$2"; mkjunk "$d/node_modules" pkg.js "$s"; }
mktarget()      { local d="$1" n="$2" s="$3"; mkjunk "$d" "$n.bin" "$s"; }

# ── Scenarios (sizes in KB) ────────────────────────────────────────

setup_1() {
    local r="$BENCH_DIR/1-node-monorepo"
    mkdir -p "$r" && mkproject "$r" js && mknode_modules "$r" 50
    for i in $(seq -w 1 20); do
        mkproject "$r/packages/pkg-$i" js-yarn
        mknode_modules "$r/packages/pkg-$i" $((RANDOM % 30 + 10))
    done
    mktarget "$r/.next" build 15
    echo "$r"
}

setup_2() {
    local r="$BENCH_DIR/2-multi-lang"
    mkdir -p "$r"
    for i in $(seq 1 5); do
        mkproject "$r/js-proj-$i" js
        mknode_modules "$r/js-proj-$i" $((RANDOM % 20 + 5))
    done
    for i in $(seq 1 5); do
        mkproject "$r/rust-proj-$i" rust
        mktarget "$r/rust-proj-$i/target" app $((RANDOM % 50 + 20))
    done
    for i in $(seq 1 5); do
        mkproject "$r/py-proj-$i" python
        mktarget "$r/py-proj-$i/__pycache__" module 2
        mktarget "$r/py-proj-$i/.venv" lib 15
    done
    echo "$r"
}

setup_3() {
    local r="$BENCH_DIR/3-many-tiny"
    mkdir -p "$r"
    for i in $(seq -w 1 500); do
        mkproject "$r/project-$i" js
        mknode_modules "$r/project-$i" 1
    done
    echo "$r"
}

setup_4() {
    local r="$BENCH_DIR/4-deep-nesting"
    mkdir -p "$r"
    for i in $(seq 1 10); do
        d=""
        for j in $(seq 1 "$i"); do d="$d/d$j"; done
        mkproject "$r$d/proj-$i" js
        mknode_modules "$r$d/proj-$i" 5
    done
    for i in $(seq -w 1 100); do
        depth=$((RANDOM % 5 + 1))
        p="$r"
        for j in $(seq 1 "$depth"); do p="$p/level$j"; done
        mkproject "$p/tiny-$i" js
        mknode_modules "$p/tiny-$i" 2
    done
    echo "$r"
}

setup_5() {
    local r="$BENCH_DIR/5-large-targets"
    mkdir -p "$r"
    mkproject "$r/proj-a" js && mknode_modules "$r/proj-a" 100
    mkproject "$r/proj-b" js && mknode_modules "$r/proj-b" 200
    mkproject "$r/proj-c" js && mknode_modules "$r/proj-c" 300
    echo "$r"
}

# Count target directories in a scenario fixture.
count_targets() {
    find "$1" -type d \( \
        -name node_modules -o -name target -o \
        -name __pycache__ -o -name .venv -o -name .next \
    \) 2>/dev/null | wc -l
}

# ── PTY runner (shared Python) ─────────────────────────────────────
# Each call spawns a tool in a PTY, auto-quits, and prints
# "wall_sec|mem_kb" on stdout.

run_in_pty() {
    local dir="$1" cmd="$2" mode="$3"  # mode: dirsweep | npkill
    local dir_e cmd_e
    dir_e=$(echo "$dir" | sed "s/'/'\\\\''/g")
    cmd_e=$(echo "$cmd" | sed "s/'/'\\\\''/g")

    python3 /dev/stdin "$dir_e" "$cmd_e" "$mode" 2>/dev/null << 'PYEOF'
import os, pty, fcntl, termios, struct, select, time, re, signal, sys, resource

def reap(pid, pgid, wait_secs=15):
    deadline = time.time() + wait_secs
    while time.time() < deadline:
        try:
            p, _ = os.waitpid(pid, os.WNOHANG)
            if p != 0: return
        except ChildProcessError:
            return
        time.sleep(0.1)
    try:
        os.killpg(pgid, signal.SIGTERM)
        for _ in range(20):
            try:
                p, _ = os.waitpid(pid, os.WNOHANG)
                if p != 0: return
            except ChildProcessError:
                return
            time.sleep(0.1)
        os.killpg(pgid, signal.SIGKILL)
        os.waitpid(pid, 0)
    except (OSError, ChildProcessError):
        pass

def strip_ansi(s):
    return re.sub(r'\x1b\[[0-9;]*[a-zA-Z]', '', s)

dir_path = sys.argv[1]
tool_cmd = sys.argv[2]
mode = sys.argv[3]

mf, sf = pty.openpty()
fcntl.ioctl(mf, termios.TIOCSWINSZ, struct.pack('HHHH', 50, 200, 0, 0))
pid = os.fork()
if pid == 0:
    os.setpgid(0, 0)
    os.close(mf)
    for fd in [0, 1, 2]: os.dup2(sf, fd)
    os.close(sf)
    if mode == 'npkill':
        os.execvp(tool_cmd, [tool_cmd, '-d', dir_path, '-nu'])
    else:
        os.execvp(tool_cmd, [tool_cmd, '--dir', dir_path])
    os._exit(1)
try: os.setpgid(pid, pid)
except: pass
os.close(sf)

output = b''
start = time.time()
result = None
phase = 0

try:
    while True:
        r, _, _ = select.select([mf], [], [], 0.05)
        if r:
            try:
                c = os.read(mf, 65536)
                if c: output += c
            except OSError:
                pass
        try:
            p_out, _ = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            p_out = pid
        if p_out != 0:
            if result is None:
                result = time.time() - start
            break

        elapsed = time.time() - start

        if mode == 'npkill':
            clean = strip_ansi(output.decode('utf-8', errors='replace'))
            if phase == 0 and 'Search completed' in clean:
                m = re.search(r'Search completed\s+([\d.]+)s', clean)
                if m:
                    result = float(m.group(1))
                os.write(mf, b'q')
                phase = 1
        else:
            if phase == 0 and len(output) > 0:
                os.write(mf, b'q')
                phase = 1
            if phase == 1 and elapsed > 0.05:
                os.write(mf, b'\r')
                phase = 2

        if elapsed > 15:
            result = result or elapsed
            if phase == 0:
                os.write(mf, b'q')
            break
finally:
    os.close(mf)
    reap(pid, pid)

rusage = resource.getrusage(resource.RUSAGE_CHILDREN)
print(f'{result:.6f}|{rusage.ru_maxrss}' if result is not None else '0.000000|0')
PYEOF

}

# ── Table ──────────────────────────────────────────────────────────

print_header() {
    printf "%-8s %-20s %10s %10s %8s %8s %6s\n" \
        "Scenario" "Name" "ds(s)" "np(s)" "ds(MB)" "np(MB)" "targets"
    printf -- "-------- -------------------- ---------- ---------- -------- -------- ------\n"
}
print_row() { printf "%-8s %-20s %10.6f %10.6f %8d %8d %6d\n" "$@"; }

# ── Main ───────────────────────────────────────────────────────────

main() {
    if [[ "${1:-}" == --prepare ]]; then
        echo "Creating fixtures in $BENCH_DIR ..."
        for sn in $SCENARIOS; do
            setup_$sn >/dev/null
            echo "  Scenario $sn done"
        done
        echo "Done."
        exit 0
    fi
    [[ "${1:-}" == -h || "${1:-}" == --help ]] && usage

    [ -f "$DIRSWEEP" ] || { echo "dirsweep not found at $DIRSWEEP — run: cargo build --release"; exit 1; }
    command -v "$NPKILL" &>/dev/null || { echo "npkill not found. Install: npm install -g npkill"; exit 1; }

    echo "dirsweep: $DIRSWEEP"
    echo "npkill:   $NPKILL"
    echo "runs:     $RUNS"
    echo ""

    print_header

    for sn in $SCENARIOS; do
        case "$sn" in
            1) name="Node monorepo"    ;; 2) name="Multi-language" ;;
            3) name="500 tiny projs"   ;; 4) name="Deep nesting" ;;
            5) name="Large targets"    ;; *) echo "Unknown scenario $sn" >&2; continue ;;
        esac

        printf "  Setting up scenario %s (%s)..." "$sn" "$name" >&2
        dir=$(setup_$sn)
        echo " done" >&2

        targets=$(count_targets "$dir")
        ds_w_sum=0; ds_m_sum=0; np_w_sum=0; np_m_sum=0

        for run in $(seq 1 "$RUNS"); do
            ds_out=$(run_in_pty "$dir" "$DIRSWEEP" dirsweep)
            ds_w=$(echo "$ds_out" | cut -d'|' -f1)
            ds_m=$(echo "$ds_out" | cut -d'|' -f2)
            ds_w_sum=$(awk "BEGIN { print $ds_w_sum + $ds_w }")
            ds_m_sum=$(awk "BEGIN { print $ds_m_sum + $ds_m }")

            np_out=$(run_in_pty "$dir" "$NPKILL" npkill)
            np_w=$(echo "$np_out" | cut -d'|' -f1)
            np_m=$(echo "$np_out" | cut -d'|' -f2)
            np_w_sum=$(awk "BEGIN { print $np_w_sum + $np_w }")
            np_m_sum=$(awk "BEGIN { print $np_m_sum + $np_m }")
        done

        ds_w_avg=$(awk "BEGIN { printf \"%.6f\", $ds_w_sum / $RUNS }")
        np_w_avg=$(awk "BEGIN { printf \"%.6f\", $np_w_sum / $RUNS }")
        ds_m_avg=$(awk "BEGIN { printf \"%d\", $ds_m_sum / $RUNS / 1024 }")
        np_m_avg=$(awk "BEGIN { printf \"%d\", $np_m_sum / $RUNS / 1024 }")

        print_row "$sn" "$name" "$ds_w_avg" "$np_w_avg" "$ds_m_avg" "$np_m_avg" "$targets"
    done

    echo ""
    echo "Columns: ds = dirsweep TUI, np = npkill TUI"
    echo "Fixtures left in $BENCH_DIR — remove with: rm -rf $BENCH_DIR"
}

main "$@"
