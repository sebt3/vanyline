# controller/Dockerfile
# Build depuis la racine du repo :
#   podman build -f controller/Dockerfile -t vanyline-controller:dev .

# ── Stage 1 : chef ──────────────────────────────────────────────────────────
# ca-certificates + curl : `cargo chef cook` (étape 3) construit les dépendances de tout le
# workspace (pas juste vanyline-controller, même Cargo.lock partagé) — utoipa-swagger-ui
# (feature swagger-ui de miryad-core, activée côté app) télécharge les assets Swagger UI via
# curl à la compilation (son build.rs), absent de l'image rust de base.
FROM rust:1-slim-trixie AS chef
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Stage 2 : planner ───────────────────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3 : builder ───────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p vanyline-controller

# ── Stage 4 : runtime ───────────────────────────────────────────────────────
# Pas de git/gcc ici : le controller ne fait qu'appeler l'API Kubernetes
# (rustls-tls — cf. kube = { features = ["runtime", "derive"] }, TLS pur Rust,
# pas de libssl). ca-certificates fournit le trust store système que
# rustls-native-certs lit au démarrage.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/vanyline-controller /usr/local/bin/vanyline-controller

ENTRYPOINT ["/usr/local/bin/vanyline-controller"]
