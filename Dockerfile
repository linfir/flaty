FROM rust:alpine3.24 AS builder
RUN apk add musl-dev
WORKDIR /usr/src/flaty
COPY . ./
RUN cargo install --path . && strip /usr/local/cargo/bin/flaty

FROM alpine:3.24
RUN adduser -D -H flaty
EXPOSE 8080
COPY --from=builder /usr/local/cargo/bin/flaty /usr/local/bin/flaty
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh
USER flaty
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
