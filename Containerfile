# Stage 1 — frontend
FROM node:22-alpine AS frontend
WORKDIR /build
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# Stage 2 — app binary
FROM rust:1.82-slim-bookworm AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build

# Cache dependencies: workspace root + all member manifests
COPY Cargo.toml Cargo.lock ./
COPY app/Cargo.toml app/
COPY sandbox/Cargo.toml sandbox/
COPY controller/Cargo.toml controller/
RUN mkdir -p app/src sandbox/src controller/src \
    && echo 'fn main(){}' > app/src/main.rs \
    && echo 'fn main(){}' > sandbox/src/main.rs \
    && echo 'fn main(){}' > controller/src/main.rs \
    && cargo build --release -p vanyline-app \
    && rm -rf app/src sandbox/src controller/src

# Build the real app
COPY app/src app/src
RUN touch app/src/main.rs && cargo build --release -p vanyline-app

# Stage 3 — runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/vanyline-app ./vanyline-app
COPY --from=frontend /build/dist ./static
EXPOSE 8080
ENTRYPOINT ["/app/vanyline-app"]
