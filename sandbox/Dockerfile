# sandbox/Dockerfile
# Build depuis la racine du repo :
#   podman build -f sandbox/Dockerfile -t vanyline-sandbox:dev .

# ── Stage 1 : chef ──────────────────────────────────────────────────────────
FROM rust:1-slim-trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Stage 2 : planner ───────────────────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3 : builder ──────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p vanyline-sandbox

# ── Stage 4 : runtime ──────────────────────────────────────────────────────
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        gcc libc6-dev binutils make pkg-config \
        git curl vim ca-certificates \
        ripgrep fd-find jq procps less file tree patch diffutils \
        unzip openssh-client python3 \
    && rm -rf /var/lib/apt/lists/*
RUN ln -s /usr/bin/fdfind /usr/local/bin/fd
COPY --from=builder /app/target/release/vanyline-sandbox /usr/local/bin/vanyline-sandbox
COPY --from=builder /app/target/release/vanyline-maint /usr/local/bin/vanyline-maint

EXPOSE 3000 9090
ENTRYPOINT ["/usr/local/bin/vanyline-sandbox"]
