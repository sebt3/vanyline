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

# Volar v3 (mode hybride, feature vue-lsp) : @vue/typescript-plugin = plugin
# chargé par typescript-language-server (aux tsserver-forward du composite
# node+vue) ; @vue/language-server = serveur .vue (primaire du composite).
# Épinglés au patch (décision 2026-09-06 — churn Volar) ; les faire avancer =
# modifier ces lignes. typescript/typescript-language-server restent flottants
# (hors périmètre vue-lsp).
#
# --install-links est OBLIGATOIRE sur @vue/language-server : sans lui npm
# hoiste @vue/typescript-plugin au top-level global (version de dépendance
# exactement égale) et le sous-répertoire
# /usr/local/lib/node_modules/@vue/language-server/node_modules/@vue/typescript-plugin
# n'existe pas — or le preset composite du controller référence le plugin par
# `location: /toolchains/node-lsp/usr/local/lib/node_modules/@vue/language-server`
# (montage de la MÊME image en volume `/toolchains/node-lsp`) et la résolution
# tsserver d'un plugin passe par `<location>/node_modules/<name>` — layout
# vérifié sur l'image construite (risque n° 3 du design, itérage 2).
RUN npm install -g @vue/typescript-plugin@3.3.11
RUN npm install -g --install-links @vue/language-server@3.3.11
