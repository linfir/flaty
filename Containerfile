FROM docker.io/rust:alpine3.17 AS builder
RUN apk add musl-dev
WORKDIR /usr/src/flaty
COPY . ./
RUN cargo install --path . && strip /usr/local/cargo/bin/flaty

FROM docker.io/alpine:3.17
EXPOSE 8080
COPY --from=builder /usr/local/cargo/bin/flaty /usr/local/bin/flaty
CMD ["flaty", "--bind", "0.0.0.0", "--port", "8080", "--directory", "/data"]
