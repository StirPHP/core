#!/usr/bin/env bash
# Shallow-clones large PHP projects into target/corpus (parse-only, never executed)
# and runs the parser over them, reporting files with syntax errors.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
corpus="$root/target/corpus"
mkdir -p "$corpus"
for repo in laravel/framework symfony/symfony phpstan/phpstan-src; do
  dir="$corpus/$(basename "$repo")"
  [ -d "$dir" ] || git clone -q --depth 1 "https://github.com/$repo" "$dir"
done
for dir in "$corpus"/*/; do
  cargo run -q --release -p stir_syntax --example corpus -- "$dir" "${1:-20}"
done
