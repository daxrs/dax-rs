# ── Stage 1: cargo-chef installer ────────────────────────────────────────────
# RUST_CHANNEL=stable (default) or nightly — nightly is required for the
# `nightly` polars feature (SIMD + specialization); see POLARS_FEATURES below.
ARG RUST_CHANNEL=stable
FROM rust:1-slim-bookworm AS chef
ARG RUST_CHANNEL
RUN if [ "$RUST_CHANNEL" != "stable" ]; then rustup toolchain install "$RUST_CHANNEL"; fi
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Stage 2: dependency planner ──────────────────────────────────────────────
FROM chef AS planner
ARG RUST_CHANNEL
COPY . .
RUN cargo +${RUST_CHANNEL} chef prepare --recipe-path recipe.json

# ── Stage 3: builder ─────────────────────────────────────────────────────────
FROM chef AS builder
ARG RUST_CHANNEL
# Cargo features to pass to both the dependency-cook step and the real build —
# must match so the cached dependency layer isn't invalidated. Independent
# flags: pass e.g. "performant,nightly" to combine them for a nightly image.
ARG POLARS_FEATURES=performant
COPY --from=planner /app/recipe.json recipe.json
# Compile dependencies — this layer is cached as long as Cargo.toml/Cargo.lock
# and recipe.json are unchanged, even when application source changes.
RUN cargo +${RUST_CHANNEL} chef cook --release --recipe-path recipe.json --features "${POLARS_FEATURES}"
COPY . .
RUN cargo +${RUST_CHANNEL} build --release --bin dax-rs --bin generate-demodata --features "${POLARS_FEATURES}"

# ── Stage 4: runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      wget \
      tzdata \
 && rm -rf /var/lib/apt/lists/*

# Standard Docker timezone convention: TZ env var, overridable at `docker run`
# with -e TZ=Region/City. This only affects NOW()/TODAY() when the app-level
# `timezone` config (server.yaml / DAX_TIMEZONE / --timezone) is left unset —
# that config takes precedence and doesn't depend on tzdata at all (chrono-tz
# bundles its own IANA database in the binary).
ENV TZ=Etc/UTC

# Fixed, non-root UID/GID: the image doesn't depend on the host's user
# database, and named/anonymous volumes inherit this ownership on first use.
# Bind-mounting a host directory instead reflects the host's own ownership —
# either chown that directory to 10001:10001 first, or run with
# `--user $(id -u):$(id -g)` to match your host user.
RUN groupadd -g 10001 dax && useradd -u 10001 -g dax -M -s /usr/sbin/nologin dax

COPY --from=builder /app/target/release/dax-rs /usr/local/bin/dax-rs
COPY --from=builder /app/target/release/generate-demodata /usr/local/bin/generate-demodata
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /data

# Default server.yaml location — mount a config file here to have it picked up
# automatically with no extra flags:
#   docker run -v ./server.yaml:/config/server.yaml:ro ...
# (overrides DAX_CONFIG or --config still take priority if you need a
# different path).
RUN mkdir -p /config && chown -R dax:dax /data /config
ENV DAX_CONFIG=/config/server.yaml

USER dax

EXPOSE 3000

# GET /health returns {"status":"ok"} once the HTTP server is accepting requests.
# start-period gives the server time to load and warm up models before failures count.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD wget -qO- http://localhost:3000/health > /dev/null || exit 1

ENTRYPOINT ["/entrypoint.sh"]
CMD []
