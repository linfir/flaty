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

# Delete every ghcr.io container version except the current latest and the
# per-arch child manifests its multi-arch index references
prune-images:
    #!/usr/bin/env bash
    set -euo pipefail
    package=flaty
    image=ghcr.io/linfir/flaty

    # Keep the latest index plus every manifest digest it references.
    raw=$(docker buildx imagetools inspect --raw "$image:latest")
    keep=$(printf '%s' "$raw" | jq -r '.manifests[].digest')
    keep="$keep sha256:$(printf '%s' "$raw" | sha256sum | cut -d' ' -f1)"
    keep_json=$(printf '%s\n' $keep | jq -R . | jq -s .)

    gh api --paginate --slurp "user/packages/container/$package/versions" \
        | jq -r --argjson keep "$keep_json" \
            'add | .[] | select(.name as $d | $keep | index($d) | not) | .id' \
    | while read -r id; do
        echo "Deleting version $id"
        gh api -X DELETE "user/packages/container/$package/versions/$id"
    done
