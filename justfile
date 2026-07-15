# Rebuild and rerun on changes, serving example_site with trace logging
dev:
    RUST_LOG=flaty=trace cargo watch -c -i example_site -x 'run -- -d example_site'

# Build in release mode and install the binary to ~/.local/bin
install:
    cargo build --release
    cp target/release/flaty ~/.local/bin/
