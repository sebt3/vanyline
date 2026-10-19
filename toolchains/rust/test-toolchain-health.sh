#!/usr/bin/env bash
# toolchains/rust/test-toolchain-health.sh
# Vérification post-build de l'exigence de santé du code de l'image toolchain rust
# (Scenarios de toolchains/toolchains.sdd, check `rust-coverage` de health/src/checks.sdd) :
#
#   - `cargo llvm-cov --version` réussit et imprime la version épinglée dans le
#     Dockerfile (lue ici par grep, jamais dupliquée en dur dans ce script) ;
#   - le composant rustup `llvm-tools` est présent dans `rustup component list --installed`.
#
# Mêmes conditions que test-toolchain.sh, celles du pod sandbox :
#   --read-only    /usr/local/cargo/bin et /usr/local/rustup en lecture seule (volume K8s) :
#                  rien ne peut être installé au runtime, la vérif porte donc sur le build.
#   -u 65534       nobody, comme le process sandbox.
#   --tmpfs /tmp   seul point d'écriture.
#
# Succès = LLVM_COV_OK puis LLVM_TOOLS_OK imprimés, exit 0.
#
# Usage : ./test-toolchain-health.sh [image]   (défaut : vanyline-toolchains-rust:health)
set -euo pipefail

image="${1:-vanyline-toolchains-rust:health}"
dockerfile="$(dirname "$0")/Dockerfile"

# Version épinglée : ligne `cargo install --locked --version X.Y.Z cargo-llvm-cov`.
pin="$(awk '/cargo install/ && /--locked/ && /cargo-llvm-cov/ {
    for (i = 1; i < NF; i++) if ($i == "--version") { print $(i + 1); exit }
}' "$dockerfile")"
if [ -z "$pin" ]; then
    echo "PIN_MISSING: aucune version épinglée de cargo-llvm-cov dans $dockerfile" >&2
    exit 1
fi

# Programme exécuté dans le conteneur par `sh -euc` (dash : pas de pipefail, étapes simples).
# Heredoc quoté : aucun échappement à faire.
container_script="$(cat <<'SH'
v="$(cargo llvm-cov --version)"
echo "LLVM_COV: $v"
echo "RUSTUP_LIST_BEGIN"
rustup component list --installed
echo "RUSTUP_LIST_END"
SH
)"

out="$(podman run --rm --read-only --tmpfs /tmp:rw,mode=1777 -u 65534 "$image" \
    sh -euc "$container_script")"
printf '%s\n' "$out"

llvm_line="$(grep -F 'LLVM_COV:' <<<"$out" || true)"
case "$llvm_line" in
    *"$pin"*) echo LLVM_COV_OK ;;
    *)
        echo "LLVM_COV_FAIL: version épinglée $pin absente de la sortie de cargo llvm-cov --version" >&2
        exit 1
        ;;
esac

rustup_list="$(sed -n '/^RUSTUP_LIST_BEGIN$/,/^RUSTUP_LIST_END$/p' <<<"$out")"
case "$rustup_list" in
    *llvm-tools*) echo LLVM_TOOLS_OK ;;
    *)
        echo "LLVM_TOOLS_FAIL: llvm-tools absent de rustup component list --installed" >&2
        exit 1
        ;;
esac
