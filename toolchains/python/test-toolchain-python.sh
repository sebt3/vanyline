#!/usr/bin/env bash
# toolchains/python/test-toolchain-python.sh
# Vérification post-build de l'image toolchain python (feature python-support).
#
# Le pod sandbox monte cette image en volumes `volumes[].image` séparés
# (/toolchains/python et /toolchains/python-lsp) et le composite du controller
# lance pyright-langserver + vnl-ruff-lsp — ce dernier spawn `ruff` — depuis
# /usr/local/bin de l'image. Ce script vérifie l'IMAGE construite (exécuté sur
# l'hôte via podman, jamais la machine hôte) :
#
#   1. LAYOUT_OK              : les quatre binaires présents et exécutables
#                               (node y compris — pyright-langserver est un
#                               script node au shebang `#!/usr/bin/env node`).
#   2. VERSIONS_RO_NOROOT_OK  : le montage read-only non-root du pod rejoué
#                               (gabarit toolchains/rust/test-toolchain.sh) —
#                               python 3.13.x, ruff 0.16.6 (épinglé au build),
#                               node DÉMARRE avec libatomic1 (risque 1 du
#                               design : la base python ne l'embarque pas).
#   3. PYRIGHT_SMOKE_OK       : handshake LSP complet sur pyright-langserver
#                               --stdio (initialize → capabilities → shutdown
#                               → exit) — exactement la façon dont le
#                               composite du controller lance ce serveur.
#   4. RUFF_WRAPPER_SMOKE_OK  : vnl-ruff-lsp en bout-en-bouche avec le VRAI
#                               ruff de l'image (handshake, didOpen,
#                               publishDiagnostics, code F401 réel, source
#                               "ruff", severity 2, codeDescription —
#                               conversion de la tâche 02). Le stdin du
#                               wrapper est maintenu ouvert ~4 s (printf +
#                               sleep dans le pipe) : le wrapper ne publie
#                               qu'après le lint et rend la main à EOF — un
#                               printf seul fermerait le pipe trop tôt.
#   5. RUFF_CONFIG_OK         : le pyproject.toml du cwd est respecté SANS
#                               interpolation : même scène depuis /work avec
#                               extend-ignore = ["F401", "I001"] (les DEUX
#                               règles que ruff émet réellement sur ce
#                               document de smoke, trace 0.16.6 vérifiée)
#                               ⟹ publishDiagnostics présente, diagnostics
#                               vidés.
#
# Chaque étape dans `podman run --rm [-w /] [-i] image sh -euc '…'` (le `-i`
# branche l'entrée stdin pour les étapes LSP). Heredocs QUOTÉS ('SH') : les
# programmes embarquent eux-mêmes des single quotes (printf des trames LSP) —
# transmis littéralement, zéro échappement imbriqué (même motif que
# toolchains/docker/test-toolchain-docker.sh ; dash côté Debian, pas de
# pipefail — étapes simples et ordonnées).
#
# Usage : ./test-toolchain-python.sh [image]   (défaut : vanyline-toolchains-python:dev)
set -euo pipefail

image="${1:-vanyline-toolchains-python:dev}"

# ── 1. LAYOUT_OK ─────────────────────────────────────────────────────────────
layout_script="$(cat <<'SH'
test -x /usr/local/bin/node
test -x /usr/local/bin/pyright-langserver
test -x /usr/local/bin/ruff
test -x /usr/local/bin/vnl-ruff-lsp
echo LAYOUT_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$layout_script"

# ── 2. VERSIONS_RO_NOROOT_OK ─────────────────────────────────────────────────
# Rejoue les conditions réelles du pod : --read-only (EROFS sur les volumes
# comme en K8s), -u 65534 (nobody, comme le process sandbox), --tmpfs /tmp
# seul point d'écriture (gabarit toolchains/rust/test-toolchain.sh:46).
# node --version sans grep volontaire : c'est la preuve que le binaire DÉMARRE
# (libatomic1 résolu) — un ldd ne prouve pas l'exécution, et la version de
# node est flottante (base node:trixie-slim).
# Pas d'assert `pyright-langserver --version` : le drapeau --version
# n'EXISTE PAS sur ce paquet — vérifié empiriquement 2026-09-07 sur l'image
# construite : exit 1, « Connection input stream is not set. Use arguments of
# createConnection or set command line parameters: '--node-ipc', '--stdio' or
# '--socket={number}' » levé par vscode-languageserver, le CLI ne connaît que
# les flags de transport (classe exacte du bug docker-langserver documenté
# dans toolchains/docker/test-toolchain-docker.sh). Repli documenté : la
# preuve du vrai contrat client est PYRIGHT_SMOKE_OK (étape 3, handshake LSP
# complet), exactement la façon dont le composite du controller lance ce
# serveur — plus fort qu'un flag de version.
versions_script="$(cat <<'SH'
python3 --version | grep -q '3\.13\.'
ruff --version | grep -q '0\.16\.6'
node --version
echo VERSIONS_RO_NOROOT_OK
SH
)"
podman run --rm --read-only --tmpfs /tmp:rw,mode=1777 -u 65534 "$image" \
    sh -euc "$versions_script"

# ── 3. PYRIGHT_SMOKE_OK ──────────────────────────────────────────────────────
# Le vrai contrat client, handshake LSP complet sur stdio (initialize →
# réponse capabilities → initialized → shutdown → exit) — exactement la façon
# dont le composite du controller lance ce serveur.
# Params initialize COMPLETS (même constat que docker-langserver : avec
# params:{} le serveur peut ne pas répondre — capabilities:{} suffit mais on
# envoie le minimum complet). Root/rw ici, contrairement à l'étape 2 : pyright
# peut toucher son cache ; en pod c'est l'espace writable du container.
# sleep 3 AUSSI entre shutdown et exit (gabarit docker : les deux d'un bloc) :
# vérifié empiriquement 2026-09-07 sur l'image construite, pyright rend la
# main sur exit sans flusher la réponse à shutdown quand les deux trames
# partent d'un bloc — la réponse "id":2,"result":null manque alors. Le bon
# rythme client LSP est précisément celui-ci : attendre la réponse de shutdown
# avant d'envoyer exit (même cadence que le composite du controller).
pyright_script="$(cat <<'SH'
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
shutdown='{"jsonrpc":"2.0","id":2,"method":"shutdown"}'
exitn='{"jsonrpc":"2.0","method":"exit"}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  sleep 3
  printf 'Content-Length: %d\r\n\r\n%s' "${#shutdown}" "$shutdown"
  sleep 3
  printf 'Content-Length: %d\r\n\r\n%s' "${#exitn}" "$exitn"
  sleep 3
} | timeout 30 pyright-langserver --stdio > /tmp/pyright.out 2>/tmp/pyright.err
grep -q '"capabilities"' /tmp/pyright.out
grep -q '"id":2,"result":null' /tmp/pyright.out
echo PYRIGHT_SMOKE_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$pyright_script"

# ── 4. RUFF_WRAPPER_SMOKE_OK ─────────────────────────────────────────────────
# Scène LSP : initialize (requête → capacités minimales du wrapper),
# initialized, didOpen d'un document python à problème
# (`import os\nprint(1)\n` ⟹ F401, trace ruff 0.16.6 vérifiée — la présence
# attendue de I001 en plus n'est pas assertée ici, l'étape 5 l'ignore par
# config). Le groupe { printf…; sleep 4; } EST l'entrée stdin du wrapper : le
# pipe ne se ferme qu'après le sleep, le lint (didOpen → ruff →
# publishDiagnostics) a le temps de publier.
# `codeDescription":{"href":"…unused-import"}` : l'URL réelle du F401 rendue
# par ruff 0.16.6, relayée verbatim par la conversion (tâche 02) ; severity 2
# = décision design §3 (tout en Warning, severity ruff ignorée).
ruff_smoke_script="$(cat <<'SH'
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
doc='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p/main.py","languageId":"python","version":1,"text":"import os\nprint(1)\n"}}}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  printf 'Content-Length: %d\r\n\r\n%s' "${#doc}" "$doc"
  sleep 4
} | timeout 20 /usr/local/bin/vnl-ruff-lsp > /tmp/lsp.out 2>/tmp/lsp.err
grep -q publishDiagnostics /tmp/lsp.out
grep -q F401 /tmp/lsp.out
grep -q '"source":"ruff"' /tmp/lsp.out
grep -q '"severity":2' /tmp/lsp.out
grep -q '"codeDescription":{"href":"https://docs.astral.sh/ruff/rules/unused-import"}' /tmp/lsp.out
echo RUFF_WRAPPER_SMOKE_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$ruff_smoke_script"

# ── 5. RUFF_CONFIG_OK ────────────────────────────────────────────────────────
# Même scène avec le cwd posé à /work, où un pyproject.toml étend l'ignore
# aux DEUX règles que la trace réelle de ruff 0.16.6 émet sur ce document
# (F401 + I001 — vérifiée, aucune autre ne fire) : ruff lit le config du cwd
# HÉRITÉ du wrapper (aucune interpolation — l'URI file:///p/main.py est
# fictif, c'est bien le cwd qui décide). Le wrapper publie alors
# diagnostics:[] : preuve croisée que la notification vient du lint réel et
# non d'un code en dur.
# Négation en `if grep … exit 1` et non `! grep` : sous dash, errexit est
# ignoré pour un pipeline commençant par `!` (POSIX, vérifié : `sh -euc
# '! true; echo REACHED'` rend REACHED/0) — la forme if est la seule qui rende
# l'absence de l'item réellement obligatoire. Un item résiduel ici = à
# diagnostiquer (quelle règle, pourquoi), PAS à absorber en élargissant
# extend-ignore.
# Pas de `-w /work` ici, contrairement aux autres étapes : podman valide le
# workdir AU DÉMARRAGE du conteneur (« workdir "/work" does not exist »,
# vérifié podman 5.4.2) alors que /work est créé par le programme lui-même ;
# le `cd /work` ci-dessous pose le cwd du wrapper — c'est CE cwd que ruff
# herite et scanne, sémantique identique.
ruff_config_script="$(cat <<'SH'
mkdir -p /work
printf '[tool.ruff.lint]\nextend-ignore = ["F401", "I001"]\n' > /work/pyproject.toml
cd /work
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
opened='{"jsonrpc":"2.0","method":"initialized","params":{}}'
doc='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p/main.py","languageId":"python","version":1,"text":"import os\nprint(1)\n"}}}'
{ printf 'Content-Length: %d\r\n\r\n%s' "${#init}" "$init"
  printf 'Content-Length: %d\r\n\r\n%s' "${#opened}" "$opened"
  printf 'Content-Length: %d\r\n\r\n%s' "${#doc}" "$doc"
  sleep 4
} | timeout 20 /usr/local/bin/vnl-ruff-lsp > /tmp/lsp.out 2>/tmp/lsp.err
grep -q '"capabilities"' /tmp/lsp.out
grep -q publishDiagnostics /tmp/lsp.out
grep -q '"diagnostics":\[\]' /tmp/lsp.out
if grep -qE 'F401|I001' /tmp/lsp.out; then
  echo "RUFF_CONFIG_FAIL : item résiduel publié malgré /work/pyproject.toml" >&2
  cat /tmp/lsp.out >&2
  exit 1
fi
echo RUFF_CONFIG_OK
SH
)"
podman run --rm -w / -i "$image" sh -euc "$ruff_config_script"
