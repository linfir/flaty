# Rebuild and rerun on changes, serving example_site with trace logging
dev:
    RUST_LOG=flaty=trace cargo watch -c -i example_site -x 'run -- -d example_site'

# Build in release mode and install the binary to ~/.local/bin
install:
    cargo build --release
    cp target/release/flaty ~/.local/bin/

# Grant gh the package scopes needed by prune-images (run once, interactive)
gh-auth:
    gh auth refresh -s read:packages,delete:packages

# Delete all ghcr.io container images except the most recent one
prune-images:
    #!/usr/bin/env bash
    set -euo pipefail
    package=flaty
    gh api --paginate --slurp "user/packages/container/$package/versions" \
        | jq -r 'add | sort_by(.created_at) | reverse | .[1:] | .[].id' \
    | while read -r id; do
        echo "Deleting version $id"
        gh api -X DELETE "user/packages/container/$package/versions/$id"
    done
