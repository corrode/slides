FROM rust:1.98-trixie AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY assets ./assets
COPY examples ./examples

RUN cargo build --locked --release

FROM debian:trixie-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 slides \
    && useradd --system --uid 10001 --gid slides --home-dir /app --shell /usr/sbin/nologin slides \
    && mkdir -p /app/data \
    && chown -R slides:slides /app

WORKDIR /app

COPY --from=builder --chown=slides:slides /build/target/release/slides /usr/local/bin/slides
COPY --from=builder --chown=slides:slides /build/assets ./assets

ENV SLIDES_BIND=0.0.0.0:3000 \
    SLIDES_DATABASE_URL=sqlite://data/slides.db \
    SLIDES_HEALTHCHECK_URL=http://127.0.0.1:3000/healthz \
    RUST_LOG=slides=info,tower_http=info

USER slides

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl --fail --silent "${SLIDES_HEALTHCHECK_URL}" || exit 1

CMD ["slides"]
