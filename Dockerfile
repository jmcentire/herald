FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY herald-server/ herald-server/
COPY herald-cli/ herald-cli/

RUN cargo build --release -p herald-server

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/herald-server /usr/local/bin/herald-server

EXPOSE 8080

CMD ["herald-server"]
