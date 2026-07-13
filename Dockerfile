FROM rust:alpine3.24 AS builder
RUN apk add musl-dev
WORKDIR /usr/src/flaty
COPY . ./
RUN cargo install --path . && strip /usr/local/cargo/bin/flaty

FROM alpine:3.24
EXPOSE 80
COPY --from=builder /usr/local/cargo/bin/flaty /usr/local/bin/flaty
CMD ["flaty", "--bind", "0.0.0.0", "--port", "80", "--directory", "/data"]
