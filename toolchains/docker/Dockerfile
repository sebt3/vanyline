# toolchains/docker/Dockerfile
# Image toolchain "docker" (feature docker-lsp) — dockerfile-language-server
# + hadolint épinglé + wrapper vnl-hadolint-lsp. Le controller pointe cette
# image par défaut (ghcr.io/…/vanyline-toolchains-docker:v<version>, tâche 04)
# et la monte en DEUX volumes séparés côté pod : /toolchains/docker et
# /toolchains/docker-lsp (la toolchain docker NE monte PAS le volume
# toolchain-node — base node fraîche ici, montages distincts déjà câblés).
#
# Build depuis la racine du repo :
#   podman build -f toolchains/docker/Dockerfile -t vanyline-toolchains-docker:dev .
#
# Publiée avec le même tag que app/sandbox/controller (cf.
# .github/workflows/release.yml) — amd64-only comme les autres (arm64 commenté
# dans le workflow).
#
# Vérification post-build : ./test-toolchain-docker.sh

# ── Stage 1 : hadolint (fetch) ───────────────────────────────────────────────
# Binaire statique (~55 Mo, vérifié empiriquement : s'exécute sans aucune
# dépendance installée, `--version` → "Haskell Dockerfile Linter 2.15.1").
# Épinglage URL + checksum SHA256 vérifié au build = la contrainte de validation
# du design docker-lsp (même exigence que VNL-EXT-005) : un checksum qui bouge
# doit CASSER le build — jamais de `|| true`, jamais de désépinglage.
# amd64 : ce que build la CI (release.yml, linux/amd64 uniquement) ; l'asset
# arm64 existe en amont le jour où la matrice deviendra multi-arch.
FROM docker.io/library/debian:trixie-slim AS hadolint
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /hadolint \
      https://github.com/hadolint/hadolint/releases/download/v2.15.1/hadolint-linux-x86_64 \
    && echo "c7187db94eeeeca956519a6af171adc31453941a1e777961f6e680f697c8c507  /hadolint" | sha256sum --check \
    && chmod 0755 /hadolint

# ── Stage 2 : wrapper (build cargo) ──────────────────────────────────────────
# vnl-hadolint-lsp : binaire du crate sandbox (src/bin/hadolint_lsp.rs), baké
# dans l'image toolchain docker par copie multi-stage (§Wrapper du design).
# Pas de cargo-chef ici, contrairement à sandbox/Dockerfile : build unique par
# release, la simplicité prime (pattern écarté à dessein).
# Contrainte de cohérence glibc : compilé sur rust:1-slim-trixie, exécuté dans
# le pod sandbox (debian:trixie-slim) via le volume — même famille trixie,
# même loader (règle AGENTS.md).
FROM docker.io/library/rust:1-slim-trixie AS wrapper
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p vanyline-sandbox --bin vnl-hadolint-lsp

# ── Stage 3 : final ──────────────────────────────────────────────────────────
# dockerfile-language-server-nodejs = le paquet npm flottant dont le binaire
# s'appelle `docker-langserver` (posé dans /usr/local/bin par npm -g, préfixe
# npm par défaut de la base node — même convention flottante que
# typescript-language-server dans toolchains/node ; nom vérifié par le test 1
# de test-toolchain-docker.sh).
#
# Côté pod : l'image est montée en volumes read-only séparés /toolchains/docker
# et /toolchains/docker-lsp ⟹ /usr/local/bin de CETTE image se retrouve sous
# /toolchains/<x>/usr/local/bin/… — exactement les chemins que le composite du
# controller (tâche 04) met dans VNL_LSP_TOOLCHAINS.
#
# Pas de USER : les toolchains node/rust n'en mettent pas — le pod pose l'UID,
# et le contenu du volume est read-only de toute façon.
FROM docker.io/library/node:trixie-slim

RUN npm install -g dockerfile-language-server-nodejs
COPY --from=hadolint /hadolint /usr/local/bin/hadolint
COPY --from=wrapper /app/target/release/vnl-hadolint-lsp /usr/local/bin/vnl-hadolint-lsp
