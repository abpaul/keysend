FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 --home /app keysend \
    && mkdir -p /data/db /data/cache && chown -R keysend:keysend /data
WORKDIR /app
# Built with `cargo build --release --locked --bin keysend-cli` in rust:1.99.0-bookworm, the same
# Debian release as this image, so that glibc matches.
COPY --chmod=0755 dist/keysend-cli /usr/local/bin/keysend-cli
COPY assets/static ./assets/static
COPY assets/views ./assets/views
COPY config ./config
COPY content/seo/catalog.json ./content/seo/catalog.json
USER keysend
EXPOSE 5150
CMD ["keysend-cli", "start", "--server-and-worker", "--environment", "production"]
