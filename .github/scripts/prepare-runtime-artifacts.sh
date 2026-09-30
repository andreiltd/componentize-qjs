#!/usr/bin/env bash
set -euo pipefail

target_dir="$1"
destination_dir="$2"
shift 2

artifact_dir="$target_dir/runtime"
cargo run --locked -p xtask -- --release --auditable \
  --output "$artifact_dir" --build-dir "$target_dir/build"
mkdir -p "$destination_dir"

for mapping in "$@"; do
  source_name="${mapping%%=*}"
  destination_name="${mapping#*=}"
  source_path="$artifact_dir/$source_name"
  destination_path="$destination_dir/$destination_name"

  test -f "$source_path" || { echo "ERROR: $source_name not found"; exit 1; }
  cp "$source_path" "$destination_path"
  test -f "$destination_path" || { echo "ERROR: $destination_name not created"; exit 1; }

  sha256sum "$destination_path" > "$destination_path.sha256"
  auditable2cdx "$destination_path" > "$destination_path.cdx.json"
  test -s "$destination_path.cdx.json" || { echo "ERROR: $destination_name SBOM is empty"; exit 1; }

  echo "$destination_name ready ($(wc -c < "$destination_path") bytes)"
done
