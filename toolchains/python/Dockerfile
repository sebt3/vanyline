# toolchains/python/Dockerfile
# Image toolchain "python" (feature python-support) — pyright (LSP primaire)
# + ruff épinglé + wrapper vnl-ruff-lsp. Le controller pointe cette image par
# défaut (ghcr.io/…/vanyline-toolchains-python:v<version>, tâche 03) et la
# monte en DEUX volumes séparés côté pod : /toolchains/python et
# /toolchains/python-lsp (montages distincts déjà câblés, même schéma que la
# toolchain docker).
#
# Build depuis la racine du repo :
#   podman build -f toolchains/python/Dockerfile -t vanyline-toolchains-python:dev .
#
# Publiée avec le même tag que app/sandbox/controller (cf.
# .github/workflows/release.yml) — amd64-only comme les autres (arm64 commenté
# dans le workflow).
#
# Vérification post-build : ./test-toolchain-python.sh

# ── Stage 1 : pyright (fetch npm) ────────────────────────────────────────────
# pyright = le paquet npm flottant dont le binaire LSP s'appelle
# `pyright-langserver` (posé dans /usr/local/bin par npm -g, préfixe npm par
# défaut de la base node, paquet dans /usr/local/lib/node_modules/pyright —
# même convention flottante que typescript-language-server/docker-langserver ;
# bump = modifier cette ligne).
FROM docker.io/library/node:trixie-slim AS pyright
RUN npm install -g pyright

# ── Stage 2 : ruff (fetch binaire épinglé) ───────────────────────────────────
# Épinglage URL + checksum SHA256 DU TARBALL vérifié au build = la contrainte
# de validation du design (même exigence que hadolint/VNL-EXT-005) : le
# checksum vérifié est celui de l'asset d'origine publié en amont (le `.sha256`
# officiel de la release GitHub, vérifié empiriquement) — un checksum qui
# bouge doit CASSER le build, jamais de `|| true`, jamais de désépinglage.
# (Alternative « sha du binaire extrait » écartée : jamais publiée par
# l'éditeur ; le binaire extrait vaut sha256
# 03b22709bb8f62028a0732423eaac6f7792235bbcb05b3feaddaac3d992318d8 — simple
# référence, pas le checksum vérifié.)
# amd64 : ce que build la CI (release.yml, linux/amd64 uniquement) ; l'asset
# arm64 existe en amont le jour où la matrice deviendra multi-arch.
# Contenu du tarball : exactement ruff-x86_64-unknown-linux-gnu/ruff (un seul
# fichier, pas de LICENSE — d'où le mv sans accompagnement).
FROM docker.io/library/debian:trixie-slim AS ruff
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /ruff.tar.gz \
      https://github.com/astral-sh/ruff/releases/download/0.16.6/ruff-x86_64-unknown-linux-gnu.tar.gz \
    && echo "0696335ef16615d8c7445ad438750eb0f55b3da6f153df21265a7c6d5750254f  /ruff.tar.gz" | sha256sum --check \
    && tar -xzf /ruff.tar.gz -C / \
    && mv /ruff-x86_64-unknown-linux-gnu/ruff /ruff \
    && chmod 0755 /ruff

# ── Stage 3 : wrapper (build cargo) ──────────────────────────────────────────
# vnl-ruff-lsp : binaire du crate sandbox (src/bin/ruff_lsp.rs, tâche 02),
# baké dans l'image toolchain python par copie multi-stage (même pattern que
# vnl-hadolint-lsp ; pas de cargo-chef, build unique par release).
# Contrainte de cohérence glibc : compilé sur rust:1-slim-trixie, exécuté dans
# le pod sandbox via le volume — même famille trixie, même loader (règle
# AGENTS.md).
FROM docker.io/library/rust:1-slim-trixie AS wrapper
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p vanyline-sandbox --bin vnl-ruff-lsp

# ── Stage 4 : final ──────────────────────────────────────────────────────────
# python:3.13-slim-trixie : version figée (cohérent rust/node, design périmètre).
#
# libatomic1 : le binaire node copié ci-dessous la lie (ldd vérifié sur
# node:trixie-slim : libatomic/libdl/libm/libstdc++/libgcc/libc) et la base
# python NE L'EMBARQUE PAS (vérifié empiriquement 2026-09-07 — sans elle, node
# meurt « error while loading shared libraries: libatomic.so.1 » dans l'image
# et dans le pod ; même leçon libatomic que les volumes toolchain AGENTS.md,
# risque n° 1 du design). libstdc++6 et libgcc-s1 sont déjà là (Python en
# dépend).
#
# node dans /usr/local/bin : le binaire node est copié comme fichier régulier
# (seul moyen fiable — il n'est pas symlinké dans la base), et le stage final
# l'embarque dans /usr/local/bin pile (monté en /toolchains/python ⟹ sur le
# PATH du preset toolchain via /toolchains/python/usr/local/bin, design §2/
# risque 1) : pyright-langserver a un shebang `#!/usr/bin/env node`.
#
# pyright-langserver en SYMLINK RELATIF et non en COPY (déviation du pattern
# « COPY déréférence les symlinks npm », vérifié empiriquement 2026-09-07 sur
# l'image construite) : le bin npm est un symlink vers le `langserver.index.js`
# du paquet, dont le contenu résout SON répertoire (`global.__rootDirectory =
# __dirname + '/dist/'` + `require('./dist/pyright-langserver')`). Copié en
# fichier régulier dans /usr/local/bin, il meurt « MODULE_NOT_FOUND
# ./dist/pyright-langserver » (et typeshed-fallback serait hors de portée
# aussi) — en image comme monté sous /toolchains/python-lsp. Le symlink
# relatif `../lib/node_modules/…` (convention des npm/npx de la base node)
# résout contre sa propre position : valide dans l'image ET sous le montage
# volume du pod ; le +x et le shebang sont portés par la cible du paquet.
#
# Côté pod : l'image est montée en volumes read-only séparés /toolchains/python
# et /toolchains/python-lsp ⟹ /usr/local/bin de CETTE image se retrouve sous
# /toolchains/<x>/usr/local/bin/… — exactement les chemins que référence le
# composite du controller (tâche 03).
#
# Pas de USER : les toolchains node/rust/docker n'en mettent pas — le pod pose
# l'UID, et le contenu du volume est read-only de toute façon.
FROM docker.io/library/python:3.13-slim-trixie
RUN apt-get update && apt-get install -y --no-install-recommends libatomic1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=pyright /usr/local/lib/node_modules/pyright /usr/local/lib/node_modules/pyright
COPY --from=pyright /usr/local/bin/node /usr/local/bin/node
RUN ln -s ../lib/node_modules/pyright/langserver.index.js /usr/local/bin/pyright-langserver
COPY --from=ruff /ruff /usr/local/bin/ruff
COPY --from=wrapper /app/target/release/vnl-ruff-lsp /usr/local/bin/vnl-ruff-lsp
