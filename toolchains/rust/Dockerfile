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

# `ln -sr` (symlink RELATIF) est indispensable ici : `rustup which` rend un
# chemin absolu (/usr/local/rustup/toolchains/<triple>/bin/rust-analyzer), qui
# ne résout plus une fois l'image montée en volume K8s à un chemin autre que
# `/` (ex. /toolchains/rust-lsp/ côté pod sandbox) — un lien absolu pointe
# alors dans le rootfs du pod, pas dans le volume. Vérifié en le cassant
# d'abord sur un cluster réel : `ln -s` (absolu) donnait un lien mort une fois
# monté, `ln -sr` (relatif) résout correctement quel que soit le mount path.
# `rust-src` : la base rust:slim-trixie fournit rustc/cargo/la std compilée,
# pas la SOURCE de la std — sans elle rust-analyzer ne peut pas résoudre le
# sysroot ("can't load standard library from sysroot ... try installing
# rust-src the same way you installed rustc", trouvé en usage réel).
RUN rustup component add rust-analyzer rust-src \
    && ln -srf "$(rustup which rust-analyzer)" /usr/local/bin/rust-analyzer
