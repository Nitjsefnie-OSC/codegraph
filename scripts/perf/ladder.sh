#!/usr/bin/env bash
# Synthetic consumer-scale workload ladder (issue #470, epic #469).
#
# Runs the ignored `ladder_step` test from
# crates/codegraph/tests/perf_model_ladder.rs at a series of (packages x
# entities) sizes, collecting the `perf_ladder` TSV lines the test prints
# (wall-clock Instant timings — hyperfine/samply are NOT required; if
# installed they can be used separately around individual rungs).
#
# Usage:
#   scripts/perf/ladder.sh [PxN ...]      # default: 3x8 6x20 9x38
#
# Environment:
#   CODEGRAPH_PERF_PROPS   scalar features per entity (default 6, in-test)
#
# Output: target/perf/ladder.tsv (appended per rung) + an aligned summary.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

SIZES=("$@")
if [ "${#SIZES[@]}" -eq 0 ]; then
    SIZES=(3x8 6x20 9x38)
fi

PROPS="${CODEGRAPH_PERF_PROPS:-6}"

echo "=== building release binary (target/release/codegraph) ==="
cargo build --release -p codegraph
if [ ! -x target/release/codegraph ]; then
    echo "error: target/release/codegraph not found after build" >&2
    exit 1
fi

TSV=target/perf/ladder.tsv
mkdir -p target/perf
if [ ! -f "$TSV" ]; then
    printf 'perf_ladder\tsize\tphase\tseconds\tfiles\n' > "$TSV"
fi

for tool in hyperfine samply; do
    if command -v "$tool" >/dev/null 2>&1; then
        echo "info: $tool found (optional; ladder timings are Instant-based in-test)"
    else
        echo "info: $tool not installed (optional — not required)"
    fi
done

for size in "${SIZES[@]}"; do
    pkgs="${size%%x*}"
    ents="${size##*x}"
    if ! [[ "$pkgs" =~ ^[0-9]+$ && "$ents" =~ ^[0-9]+$ ]]; then
        echo "error: size '$size' is not PxN (e.g. 3x8)" >&2
        exit 1
    fi
    echo "=== ladder rung ${pkgs}x${ents} (props=${PROPS}) ==="
    out="$(mktemp)"
    if ! CODEGRAPH_PERF_PKGS="$pkgs" CODEGRAPH_PERF_ENTITIES="$ents" CODEGRAPH_PERF_PROPS="$PROPS" \
        cargo test --release -p codegraph --test perf_model_ladder ladder_step \
        -- --ignored --nocapture 2>&1 | tee "$out"; then
        echo "error: ladder rung ${pkgs}x${ents} failed" >&2
        rm -f "$out"
        exit 1
    fi
    grep '^perf_ladder' "$out" >> "$TSV"
    rm -f "$out"
done

echo
echo "=== ladder results (${TSV}) ==="
awk -F'\t' '
    { rows[NR] = $0; if (NF > nf) nf = NF; for (i = 1; i <= NF; i++) if (length($i) > w[i]) w[i] = length($i) }
    END {
        for (r = 1; r <= NR; r++) {
            n = split(rows[r], f, "\t")
            line = ""
            for (i = 1; i <= n; i++) {
                line = line ((i < n) ? sprintf("%-*s  ", w[i], f[i]) : f[i])
            }
            print line
        }
    }' "$TSV"
