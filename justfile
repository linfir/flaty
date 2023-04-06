dev:
    cargo watch -x 'run -- 127.0.0.1:8080 example_site'

build:
    podman build -t flaty .

run:
    podman run --rm -it --name flaty -v ./example_site:/data -p 8080:8080 flaty

stop:
    podman stop flaty
