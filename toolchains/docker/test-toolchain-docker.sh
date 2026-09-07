#!/usr/bin/env bash
# toolchains/docker/test-toolchain-docker.sh
# Vérification post-build de l'image toolchain docker (feature docker-lsp).
#
# Le pod sandbox monte cette image en volumes `volumes[].image` séparés
# (/toolchains/docker et /toolchains/docker-lsp) et le composite du controller
# lance docker-langserver + vnl-hadolint-lsp — ce dernier spawn `hadolint` —
# depuis /usr/local/bin de l'image. Ce script vérifie l'IMAGE construite
# (exécuté sur l'hôte via podman, jamais la machine hôte) :
#
#   1. LAYOUT_OK           : les trois binaires présents et exécutables.
#   2. HADOLINT_OK         : hadolint --version → 2.15.1 (épinglé au build).
#   3. DOCKERLS_OK         : docker-langserver s'exécute sur la base node.
#   4. LSP_SMOKE_OK        : vnl-hadolint-lsp en bout-en-bouche avec le VRAI
#                            hadolint de l'image (handshake, didOpen,
#                            publishDiagnostics, code DL3007 réel, source
#                            "hadolint" — conversion de la tâche 03). Le stdin
#                            du wrapper est maintenu ouvert ~4 s (printf +
#                            sleep dans le pipe) : le wrapper ne publie qu'après
#                            le lint et rend la main à EOF — un printf seul
#                            fermerait le pipe trop tôt.
#   5. HADOLINT_CONFIG_OK  : le .hadolint.yaml du cwd est respecté SANS
#                            interpolation (question ouverte du design) : même
#                            scène depuis /work contenant ignored: ["DL3007"]
#                            ⟹ publishDiagnostics présente, vidée de DL3007.
#
# Chaque étape dans `podman run --rm -w <dir> -i image sh -euc '…'` (le `-i`
# branche l'entrée stdin pour l'étape 4). Heredocs QUOTÉS ('SH') : les
# programmes embarquent eux-mêmes des single quotes (printf des trames LSP) —
# transmis littéralement, zéro échappement imbriqué (même motif que
# toolchains/rust/test-toolchain.sh ; dash côté Debian, pas de pipefail —
# étapes simples et ordonnées).
#
# Usage : ./test-toolchain-docker.sh [image]   (défaut : vanyline-toolchains-docker:dev)
set -euo pipefail

image="${1:-vanyline-toolchains-docker:dev}"

# ── 1. LAYOUT_OK ─────────────────────────────────────────────────────────────
layout_script="$(cat <<'SH'
test -x /usr/local/bin/hadolint
test -x /usr/local/bin/docker-langserver
test -x /usr/local/bin/vnl-hadolint-lsp
echo LAYOUT_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$layout_script"

# ── 2. HADOLINT_OK ───────────────────────────────────────────────────────────
hadolint_script="$(cat <<'SH'
hadolint --version | grep -q '2\.15\.1'
echo HADOLINT_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$hadolint_script"

# ── 3. DOCKERLS_OK ───────────────────────────────────────────────────────────
# L'essentiel (fichier de tâche) : le binaire s'exécute sur la base node. Le
# drapeau --version n'EXISTE PAS sur ce paquet — vérifié empiriquement
# 2026-09-07 sur l'image construite : `--version` comme `--help` retournent
# exit 1 (« Connection input stream is not set » levé par vscode-languageserver,
# le CLI ne connaît que les flags de transport) — le fallback « --help exit 0 »
# du fichier de tâche supposait une implémentation absente. Preuve retenue : le
# vrai contrat client, handshake LSP complet sur stdio (initialize → réponse
# capabilities → shutdown → exit 0) — exactement la façon dont le composite du
# controller lance ce serveur, donc plus fort qu'un flag de version.
# Params initialize COMPLETS : avec params:{} le serveur ne répond pas
# (validation interne silencieuse — sonde 2026-09-07) ; capabilities:{} suffit.
dockerls_script="$(cat <<'SH'
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
shutdown='{"jsonrpc":"2.0","id":2,"method":"shutdown"}'
exitn='{"jsonrpc":"2.0","method":"exit"}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  sleep 3
  printf 'Content-Length: %d\r\n\r\n%s' "${#shutdown}" "$shutdown"
  printf 'Content-Length: %d\r\n\r\n%s' "${#exitn}" "$exitn"
  sleep 3
} | timeout 20 docker-langserver --stdio > /tmp/dl.out 2>/tmp/dl.err
grep -q '"capabilities"' /tmp/dl.out
grep -q '"id":2,"result":null' /tmp/dl.out
echo DOCKERLS_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$dockerls_script"

# ── 4. LSP_SMOKE_OK ──────────────────────────────────────────────────────────
# Scène LSP : initialize (requête → capabilities), initialized, didOpen d'un
# Dockerfile à problème (FROM …:latest ⟹ DL3007). Le groupe { printf…; sleep 4; }
# EST l'entrée stdin du wrapper : le pipe ne se ferme qu'après le sleep, le lint
# (didOpen → hadolint → publishDiagnostics) a le temps de publier.
lsp_smoke_script="$(cat <<'SH'
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
doc='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p/Dockerfile","languageId":"dockerfile","version":1,"text":"FROM ubuntu:latest\nRUN echo x\n"}}}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  printf 'Content-Length: %d\r\n\r\n%s' "${#doc}" "$doc"
  sleep 4
} | timeout 20 /usr/local/bin/vnl-hadolint-lsp > /tmp/lsp.out 2>/tmp/lsp.err
grep -q '"capabilities"' /tmp/lsp.out
grep -q publishDiagnostics /tmp/lsp.out
grep -q DL3007 /tmp/lsp.out
grep -q '"source":"hadolint"' /tmp/lsp.out
echo LSP_SMOKE_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$lsp_smoke_script"

# ── 5. HADOLINT_CONFIG_OK ────────────────────────────────────────────────────
# Même scène avec le cwd posé à /work, où un .hadolint.yaml ignore DL3007 :
# hadolint lit le config du cwd HÉRITÉ du wrapper (aucune interpolation —
# l'URI file:///p/Dockerfile est fictif, c'est bien le cwd qui décide). Le
# wrapper publie alors diagnostics:[] : preuve croisée que la notification
# vient du lint réel et non d'un code en dur.
# Négation en `if grep … exit 1` et non `! grep` : sous dash, errexit est
# ignoré pour un pipeline commençant par `!` (POSIX, vérifié : `sh -euc
# '! true; echo REACHED'` rend REACHED/0) — la forme if est la seule qui rende
# l'absence de DL3007 réellement obligatoire.
# Pas de `-w /work` ici, contrairement aux autres étapes : podman valide le
# workdir AU DÉMARRAGE du conteneur (« workdir "/work" does not exist »,
# vérifié podman 5.4.2) alors que /work est créé par le programme lui-même ;
# le `cd /work` ci-dessous pose le cwd du wrapper — c'est CE cwd que hadolint
# herite et scanne, sémantique identique.
hadolint_config_script="$(cat <<'SH'
mkdir -p /work
printf 'ignored: ["DL3007"]\n' > /work/.hadolint.yaml
cd /work
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
doc='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p/Dockerfile","languageId":"dockerfile","version":1,"text":"FROM ubuntu:latest\nRUN echo x\n"}}}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  printf 'Content-Length: %d\r\n\r\n%s' "${#doc}" "$doc"
  sleep 4
} | timeout 20 /usr/local/bin/vnl-hadolint-lsp > /tmp/lsp.out 2>/tmp/lsp.err
grep -q '"capabilities"' /tmp/lsp.out
grep -q publishDiagnostics /tmp/lsp.out
grep -q '"diagnostics":\[\]' /tmp/lsp.out
if grep -q DL3007 /tmp/lsp.out; then
  echo "HADOLINT_CONFIG_FAIL : DL3007 publié malgré /work/.hadolint.yaml" >&2
  cat /tmp/lsp.out >&2
  exit 1
fi
echo HADOLINT_CONFIG_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$hadolint_config_script"
