#!/usr/bin/env bash
# Type-checks the PHP corpora in corpus/ (git submodules, never executed) and
# writes one summary per project to .corpus-results/<label>/, for comparing
# diagnostic counts and timing between revisions.
#   git submodule update --init --depth 1
#   [LEVEL=n] scripts/check-corpus.sh <label> [project...]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
label="${1:?usage: check-corpus.sh <label> [project...]}"
shift
projects=("$@")
if [ "${#projects[@]}" -eq 0 ]; then
  projects=(framework phpstan-src symfony laravel-io)
fi

(cd "$root" && cargo build -q --release -p stir_cli)
bin="$root/target/release/stirphp"
out="$root/.corpus-results/$label"
mkdir -p "$out"

for name in "${projects[@]}"; do
  dir="$root/corpus/$name"
  if [ ! -d "$dir/vendor" ]; then
    # Scripts and plugins stay off, so installing runs none of the packages' code.
    (cd "$dir" && composer install -q --no-interaction --no-scripts --no-plugins \
      --no-progress --ignore-platform-reqs)
  fi

  start=$(date +%s.%N)
  status=0
  (cd "$dir" && "$bin" check --format json ${LEVEL:+--level "$LEVEL"}) > "$out/$name.json" 2> "$out/$name.stderr" || status=$?
  end=$(date +%s.%N)

  {
    echo "project: $name"
    echo "commit: $(git -C "$dir" rev-parse --short HEAD)"
    echo "exit: $status"
    printf 'seconds: %.1f\n' "$(echo "$end - $start" | bc)"
    tail -n 1 "$out/$name.stderr"
    echo "by code:"
    jq -r '.[] | "\(.severity) \(.code)"' "$out/$name.json" | sort | uniq -c | sort -rn
  } | tee "$out/$name.summary"
done
