dev:
    cargo watch -x 'run -- 127.0.0.1:8080 example_site'

docker-build:
    docker buildx build -t flaty .

docker-run:
    docker run --rm -it -v ./example_site:/data -p 8080:8080 flaty
