# app/Dockerfile
# Build depuis la racine du repo :
#   podman build -f app/Dockerfile -t vanyline-app:dev .

# ── Stage 1 : frontend ──────────────────────────────────────────────────────
FROM node:22-alpine AS frontend
WORKDIR /build
COPY package*.json ./
COPY frontend/package*.json frontend/
RUN npm ci
COPY frontend/ frontend/
RUN npm run build

# ── Stage 2 : chef ──────────────────────────────────────────────────────────
# ca-certificates + curl : utoipa-swagger-ui (feature swagger-ui de miryad-core) télécharge les
# assets statiques de Swagger UI via curl à la compilation (son build.rs) — absent de l'image
# rust de base, jamais eu besoin jusqu'ici.
FROM rust:1-slim-trixie AS chef
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Stage 3 : planner ──────────────────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 4 : builder ──────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p vanyline-app
COPY . .
RUN cargo build --release --locked -p vanyline-app

# ── Stage 5 : runtime ──────────────────────────────────────────────────────
# Pas de libssl ici : app est 100% rustls (reqwest rustls-tls, sqlx
# runtime-tokio-rustls) — ca-certificates fournit le trust store système que
# rustls-native-certs lit au démarrage (même choix que controller/Dockerfile).
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/vanyline-app ./vanyline-app
COPY --from=frontend /build/frontend/dist ./static
EXPOSE 8080
ENTRYPOINT ["/app/vanyline-app"]