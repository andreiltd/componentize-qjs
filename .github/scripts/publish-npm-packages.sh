#!/usr/bin/env bash
# Publish the main npm package and every per-platform package to the public
# registry. Publishing is idempotent: a package whose exact name@version is
# already on the registry is skipped, so the script can be re-run to finish a
# partially completed release.
set -euo pipefail

max_attempts="${NPM_PUBLISH_ATTEMPTS:-3}"

published_version() {
  npm view "$1" version 2>/dev/null || true
}

publish_dir() {
  local package_dir="$1"
  (
    cd "$package_dir"

    local name version spec attempt delay
    name="$(node -p "require('./package.json').name")"
    version="$(node -p "require('./package.json').version")"
    spec="${name}@${version}"

    if [ -n "$(published_version "$spec")" ]; then
      echo "Skipping ${spec}: already published"
      exit 0
    fi

    attempt=1
    while true; do
      if npm publish --access public; then
        echo "Published ${spec}"
        exit 0
      fi

      if [ -n "$(published_version "$spec")" ]; then
        echo "${spec} became available after a transient failure; continuing"
        exit 0
      fi

      if [ "$attempt" -ge "$max_attempts" ]; then
        echo "ERROR: failed to publish ${spec} after ${max_attempts} attempts" >&2
        exit 1
      fi

      delay=$((attempt * 15))
      echo "Publish of ${spec} failed (attempt ${attempt}/${max_attempts}); retrying in ${delay}s" >&2
      sleep "$delay"
      attempt=$((attempt + 1))
    done
  )
}

for package_json in npm/*/package.json; do
  publish_dir "$(dirname "$package_json")"
done

publish_dir "."
