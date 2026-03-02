FROM docker.io/library/rust:1.88-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 1000 appuser && useradd -u 1000 -g appuser -d /app appuser \
    && mkdir -p /app/templates
WORKDIR /app
COPY --from=builder /build/target/release/shortlink /app/shortlink
COPY --chown=appuser:appuser templates/ /app/templates/
USER appuser
EXPOSE 3000
ENV TEMPLATE_DIR=/app/templates
CMD ["/app/shortlink"]
