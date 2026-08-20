# toolchains/node/Dockerfile
# Image toolchain "node" (js/ts) — même base que l'ancien défaut de
# TOOLCHAIN_IMAGE_NODE (docker.io/library/node:trixie-slim), avec
# typescript-language-server ajouté au build (théia-ide, actif — vérifié
# 2026-08-20). `npm install -g` pose ses binaires dans /usr/local/bin sur
# cette image (préfixe npm par défaut) — pas de symlink nécessaire,
# contrairement à rust-analyzer/rustup.
#
# Note : mcr.microsoft.com/devcontainers/typescript-node a été envisagée puis
# écartée — vérifié en lisant son Dockerfile réel, elle ne contient PAS le
# LSP (seulement typescript/eslint/tslint-to-eslint-config).
#
# Build depuis la racine du repo :
#   podman build -f toolchains/node/Dockerfile -t vanyline-toolchains-node:dev .
#
# Publiée avec le même tag que app/sandbox/controller (cf.
# .github/workflows/release.yml).
FROM docker.io/library/node:trixie-slim

RUN npm install -g typescript-language-server typescript
