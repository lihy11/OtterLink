
FROM rust:1.83-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY docs ./docs
COPY README.md ./README.md

RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/otterlink /usr/local/bin/otterlink

ENV CORE_BIND=0.0.0.0:7211
EXPOSE 7211

CMD ["otterlink"]
