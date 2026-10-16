# The build and runtime stages use the same Debian release, so that glibc matches.
FROM rust:1.99.0-bookworm AS build
# The toolchain of the image, not the components that rust-toolchain.toml would download.
ENV RUSTUP_TOOLCHAIN=1.99.0 CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
ARG GIT_SHA
WORKDIR /src
COPY . .
# The binaries leave the cache mount through /out, because the mount is not in the image.
RUN --mount=type=cache,id=keysend-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=keysend-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=keysend-target,target=/src/target,sharing=locked \
    printf '%s\n' "$GIT_SHA" | grep -Ex '[0-9a-f]{40}' \
    && grep -Fx "channel = \"$RUSTUP_TOOLCHAIN\"" rust-toolchain.toml \
    && cargo build --release --locked --bin keysend-cli --bin keysend-credentials \
    && mkdir /out \
    && cp target/release/keysend-cli target/release/keysend-credentials /out/

FROM debian:bookworm-slim
ARG GIT_SHA
LABEL org.opencontainers.image.revision=$GIT_SHA
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 --home /app keysend \
    && mkdir -p /data/db /data/cache && chown -R keysend:keysend /data
WORKDIR /app
COPY --from=build --chmod=0755 /out/keysend-cli /usr/local/bin/keysend-cli
COPY --from=build --chmod=0755 /out/keysend-credentials /usr/local/bin/keysend-credentials
RUN keysend-cli --version && keysend-credentials --help >/dev/null
COPY assets/static ./assets/static
COPY assets/views ./assets/views
COPY config ./config
COPY content/seo/catalog.json ./content/seo/catalog.json
USER keysend
EXPOSE 5150
CMD ["keysend-cli", "start", "--server-and-worker", "--environment", "production"]
