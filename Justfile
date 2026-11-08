default:
    @just --list

# Build the language server for the browser into `editors/web/pkg`, then test it in Node
web:
    wasm-pack build crates/stir_wasm --profile wasm --target web --out-dir ../../editors/web/pkg --no-pack -- --locked
    cd editors/web && node --test 'test/*.test.mjs'

# Bump every version, then commit and tag the release: `just bump minor` or `just bump patch`
bump part:
    #!/usr/bin/env bash
    set -euo pipefail

    if [ -n "$(git status --porcelain)" ]; then
        echo "the working tree isn't clean: commit or stash first" >&2
        exit 1
    fi

    current=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
    IFS=. read -r major minor patch <<< "$current"
    case "{{ part }}" in
        minor) next="$major.$((minor + 1)).0" ;;
        patch) next="$major.$minor.$((patch + 1))" ;;
        *)
            echo "expected minor or patch, got '{{ part }}'" >&2
            exit 1
            ;;
    esac

    sed -i "0,/^version = .*/s//version = \"$next\"/" Cargo.toml
    cargo update --workspace --quiet
    npm version "$next" --prefix editors/vscode --no-git-tag-version > /dev/null
    npm version "$next" --prefix editors/web --no-git-tag-version > /dev/null
    sed -Ei "s/stirphp-[0-9]+\.[0-9]+\.[0-9]+\.vsix/stirphp-$next.vsix/" editors/vscode/README.md

    git add Cargo.toml Cargo.lock editors/vscode/package.json editors/vscode/package-lock.json editors/vscode/README.md editors/web/package.json
    git commit --quiet -m "Release $next"
    git tag "v$next"

    echo "$current -> $next"
    echo "publish it with: git push origin HEAD v$next"
