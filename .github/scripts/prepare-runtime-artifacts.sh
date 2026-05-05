#!/usr/bin/env bash
set -euo pipefail

target_dir="$1"
destination_dir="$2"
shift 2

rm -rf crates/core/prebuilt "$destination_dir" "$target_dir"
COMPONENTIZE_QJS_RUNTIME_AUDITABLE=1 cargo build --release -p componentize-qjs --target-dir "$target_dir"
mkdir -p "$destination_dir"

for mapping in "$@"; do
  source_name="${mapping%%=*}"
  destination_name="${mapping#*=}"
  source_path=$(find "$target_dir" -path "*/out/$source_name" -type f | sort | tail -n 1)
  destination_path="$destination_dir/$destination_name"

  test -n "$source_path" || { echo "ERROR: $source_name not found"; exit 1; }
  cp "$source_path" "$destination_path"
  test -f "$destination_path" || { echo "ERROR: $destination_name not created"; exit 1; }

  sha256sum "$destination_path" > "$destination_path.sha256"
  auditable2cdx "$destination_path" > "$destination_path.cdx.json"
  test -s "$destination_path.cdx.json" || { echo "ERROR: $destination_name SBOM is empty"; exit 1; }

  echo "$destination_name ready ($(wc -c < "$destination_path") bytes)"
done
