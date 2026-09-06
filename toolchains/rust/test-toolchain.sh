#!/usr/bin/env bash
# toolchains/rust/test-toolchain.sh
# Vérification post-build de l'alias stable-<host> de l'image toolchain rust.
#
# Le pod sandbox monte cette image en volume `volumes[].image` READ-ONLY, portés
# par un process NON-ROOT. Ce script reproduit exactement ces conditions pour
# vérifier qu'un projet épinglé `rust-toolchain.toml` (channel = "stable") y
# résout bien son toolchain, sans qu'aucune écriture rustup ne soit tentée :
#
#   --read-only    EROFS sur /usr/local/rustup, comme le volume K8s. Aucun RUN
#                  de build ne peut simuler ça : EROFS est une propriété de
#                  mount (les faux verts chmod/nobody des rounds 1-2 de la
#                  tâche — root contourne les DAC, et la base expose
#                  /usr/local/rustup en 777 amont — restent dans l'histoire,
#                  pas dans l'image).
#   -u 65534       nobody, comme le process sandbox.
#   --tmpfs /tmp   seul point d'écriture : projet jouet + CARGO_HOME.
#
# Rouge démontré sur une image sans l'alias : `test -e` échoue, et sans lui le
# `cargo metadata` suivant échouerait en "could not create temp file
# …/rustup/tmp/…: Read-only file system" (verbatim cluster). Succès = LINK_OK
# puis METADATA_OK imprimés, exit 0.
#
# Usage : ./test-toolchain.sh [image]   (défaut : vanyline-toolchains-rust:fix-stable)
set -euo pipefail

image="${1:-vanyline-toolchains-rust:fix-stable}"

# Programme exécuté dans le conteneur par `sh -euc` (dash côté Debian : pas de
# pipefail, d'où des étapes simples et ordonnées, LINK_OK avant METADATA_OK).
# Heredoc QUOTÉ ('SH') : le programme embarque lui-même des single quotes
# (printf, awk) — transmis littéralement, zéro échappement imbriqué awk/sh/bash.
container_script="$(cat <<'SH'
mkdir -p /tmp/proj/src /tmp/cargo
printf '[toolchain]\nchannel = "stable"\n' > /tmp/proj/rust-toolchain.toml
printf '[package]\nname = "p"\nversion = "0.0.0"\nedition = "2021"\n' > /tmp/proj/Cargo.toml
: > /tmp/proj/src/main.rs
host="$(rustc -Vv | awk -F': ' '/^host:/{print $2}')"
test -e "/usr/local/rustup/toolchains/stable-$host/bin/cargo"
echo LINK_OK
(cd /tmp/proj && CARGO_HOME=/tmp/cargo HOME=/tmp/cargo cargo metadata --format-version 1 >/dev/null)
echo METADATA_OK
SH
)"

podman run --rm --read-only --tmpfs /tmp:rw,mode=1777 -u 65534 "$image" \
    sh -euc "$container_script"
