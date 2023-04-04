FROM rust:buster AS builder
WORKDIR /usr/src/flaty
COPY . ./
RUN cargo install --path .

FROM debian:buster
EXPOSE 8080
COPY --from=builder /usr/local/cargo/bin/flaty /usr/local/bin/flaty
WORKDIR /data
CMD ["flaty", "0.0.0.0:8080", "."]
