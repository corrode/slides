# syntax=docker/dockerfile:1.7

FROM lukemathwalker/cargo-chef:latest-rust-1.98-slim-trixie AS chef
WORKDIR /build

FROM chef AS planner
RUN --mount=type=bind,source=.,target=/workspace,ro \
    cd /workspace \
    && cargo chef prepare --recipe-path /build/recipe.json
RUN touch -t 197001010000 recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --locked --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY assets/catppuccin-mocha.tmTheme ./assets/catppuccin-mocha.tmTheme
RUN cargo build --locked --release

FROM debian:trixie-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 slides \
    && useradd --system --uid 10001 --gid slides --home-dir /app --shell /usr/sbin/nologin slides \
    && mkdir -p /app/data \
    && chown slides:slides /app/data

WORKDIR /app

COPY --link --from=builder /build/target/release/slides /usr/local/bin/slides
COPY --link assets ./assets
COPY --link examples ./examples

ENV SLIDES_BIND=0.0.0.0:3000 \
    SLIDES_DATABASE_URL=sqlite://data/slides.db \
    SLIDES_HEALTHCHECK_URL=http://127.0.0.1:3000/healthz \
    RUST_LOG=slides=info,tower_http=info

USER slides

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl --fail --silent "${SLIDES_HEALTHCHECK_URL}" || exit 1

CMD ["slides"]
