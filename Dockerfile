FROM rust:alpine3.17 AS builder
WORKDIR /usr/src/flaty
COPY . ./
RUN apk add --no-cache musl-dev && cargo install --path .

FROM alpine:3.17
EXPOSE 8080
COPY --from=builder /usr/local/cargo/bin/flaty /usr/local/bin/flaty
WORKDIR /data
CMD ["flaty", "0.0.0.0:8080", "."]
