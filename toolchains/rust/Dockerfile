# toolchains/rust/Dockerfile
# Image toolchain "rust" — même base que l'ancien défaut de TOOLCHAIN_IMAGE_RUST
# (docker.io/library/rust:slim-trixie), avec rust-analyzer ajouté au build.
#
# rust-analyzer ne peut pas s'installer au runtime dans le pod sandbox : le
# volume `volumes[].image` qui monte cette image y est toujours en lecture
# seule (propriété K8s, pas une contrainte vanyline) — `rustup component add`
# y échoue avec "Read-only file system" (vérifié empiriquement, cf.
# .claude/memory/lsp-integration.md). D'où l'ajout au build de l'image plutôt
# qu'au démarrage du pod.
#
# Build depuis la racine du repo :
#   podman build -f toolchains/rust/Dockerfile -t vanyline-toolchains-rust:dev .
#
# Publiée avec le même tag que app/sandbox/controller (cf.
# .github/workflows/release.yml) — même schéma de versionnement, pas un
# pin séparé sur une version upstream de rust-analyzer.
FROM docker.io/library/rust:slim-trixie

RUN rustup component add rust-analyzer \
    && ln -s "$(rustup which rust-analyzer)" /usr/local/bin/rust-analyzer
