FROM rust:1.93-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY apps/api/Cargo.toml apps/api/Cargo.toml
COPY apps/api/src apps/api/src
COPY apps/api/migrations apps/api/migrations
RUN cargo build --locked --release -p restaurant-api

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/restaurant-api /usr/local/bin/restaurant-api
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["restaurant-api"]
