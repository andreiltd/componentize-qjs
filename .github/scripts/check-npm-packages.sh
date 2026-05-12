#!/usr/bin/env bash
# Run `npm pack --dry-run` for the main npm package and every per-platform
# package, and assert every tarball contains README.md and LICENSE. Must be
# run from the `npm/` package directory.
set -euo pipefail

required_assets=("README.md" "LICENSE")

assert_assets() {
  local label="$1"
  local files
  files=$(npm pack --dry-run --json | jq -r '.[0].files[].path')
  for asset in "${required_assets[@]}"; do
    if ! grep -qxF "$asset" <<< "$files"; then
      echo "ERROR: ${label} is missing ${asset}" >&2
      echo "$files" >&2
      exit 1
    fi
  done
}

for package_json in npm/*/package.json; do
  package_dir="$(dirname "$package_json")"
  (
    cd "$package_dir"
    npm pack --dry-run
    assert_assets "$(basename "$package_dir") platform package"
  )
done

npm pack --dry-run
assert_assets "main package"
