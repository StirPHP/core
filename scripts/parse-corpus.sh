#!/usr/bin/env bash
# Runs the parser over the PHP corpora in corpus/ (git submodules; parse-only,
# never executed), reporting files with syntax errors.
#   git submodule update --init --depth 1
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
for dir in "$root"/corpus/*/; do
  cargo run -q --release -p stir_syntax --example corpus -- "$dir" "${1:-20}"
done
