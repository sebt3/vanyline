# Fondations d'architecture

Décisions structurantes prises tôt dans le projet, toujours valides.

## Quatre composants

| Composant | Rôle | Décision |
|-----------|------|----------|
| frontend | Éditeur web + UI LLM | Vite + Svelte 5, CodeMirror 6, svelte-spa-router, Tailwind CSS 4, Vitest, Storybook |
| app | Backend, OIDC, Redis, PGVector | Rust — focus initial : interaction LLM, users, config API |
| sandbox | Pod K8s, serveur WS/MCP | Rust — image Debian slim + toolchains OCI image volumes |
| controller | Opérateur K8s | Rust, kube-rs — **DÉFÉRÉ** (sorti du statut déféré depuis, cf. `controller-bootstrap.md`) |

Note (2026-08-09) : le rôle du composant `frontend` tel que décrit ici (éditeur web) est
remis en cause par la réorientation stratégique — cf. `reorientation-2026-08-09.md`.

## L'app n'est pas sur le chemin chaud

Le frontend se connecte **directement** à la sandbox en WebSocket (JWT validé par la sandbox).
kydah-code se connecte directement au MCP de la sandbox via service K8s interne.
L'app ne proxifie rien — elle gère l'auth, le LLM, la config.

## Deux modes d'auth sur la sandbox

- **JWT** (frontend via ingress) : token OIDC émis par l'app, validé par la sandbox
- **SA TokenReview + NetworkPolicy** (kydah-code et app via service interne) : la sandbox appelle
  le K8s TokenReview API pour valider le SA token du pod appelant ; NetworkPolicy par sandbox
  restreint l'accès aux pods du même namespace avec les bons labels

Mécanisme uniforme pour kydah-code ET l'app : tous deux utilisent le ServiceAccount du Owner
concerné. L'app, pour orchestrer le LLM d'un utilisateur donné, utilise le SA de son Owner.

Décision : Option A (JWT émis par l'app pour kydah-code) rejetée car elle crée une dépendance
à l'app avant que l'app existe — incompatible avec le développement des deux axes en parallèle.

## Design sandbox — toolchains via K8s image volumes

Les toolchains utilisent des images Docker standard (ex: `rust:slim-trixie`, `node:trixie-slim`)
montées via `volumes[].image` — feature K8s native GA depuis v1.36, prérequis v1.31+.
Pas de registre propriétaire, pas de build custom.

**Validé en conditions réelles sur cluster 1.36 (2026-07-01, cri-o 1.36.1)** — deux pods de test
(`deploy/sandbox-imagevol-*.yaml`, supprimés depuis par WS-8 une fois la recette absorbée par
`sandbox/Dockerfile` et le controller) ont éprouvé node et rust. Recette d'assemblage confirmée :

- **Répartition base / volume / PVC** :
  - *base* = substrat natif commun installé proprement (apt) : **linker C `cc`/`ld` + binutils,
    `libc-dev`, make, pkg-config**, git, curl, vim. Le linker est obligatoire : `rust:slim`
    ne l'embarque pas → `cargo build` échoue sur `error: linker cc not found`. Vrai pour toute
    compilation native (node-gyp, cgo…).
  - *volumes* = toolchains langage, read-only, utilisables par **injection d'env** :
    - `PATH` → `…/bin` du volume
    - `LD_LIBRARY_PATH` → `…/usr/lib/<arch>-linux-gnu` du volume (sinon le loader du base ne
      trouve pas les libs du volume — ex: `libatomic.so.1` manquant pour node)
    - **env par toolchain** : rust → `RUSTUP_HOME` sur le volume (les binaires de `cargo/bin`
      sont des symlinks vers `rustup`, ça suffit — rustup n'est PAS un obstacle)
  - *PVC du Owner* = homes writable **hors volume read-only** : `CARGO_HOME`, `~/.npm`, etc.
- **Contrainte distro** : base et images toolchain sur la **même famille** (trixie). Le loader
  du base résout la glibc ; un mismatch de distro marche par compat ascendante = hasard, pas design.
- **Correction doc** : l'ancienne formule « PATH/LD_LIBRARY_PATH injectés » sous-estimait le sujet
  (linker manquant dans le base + env par toolchain). AGENTS.md corrigé en conséquence.

## kydah-code est un client de la sandbox

kydah-code (extension VS Code pour code-server) consomme le MCP de la sandbox pour donner
à Qwen l'accès aux vrais outils (builds, filesystem, terminal) sans saturer le pod code-server.
Le Owner dans ce cas référence le PVC existant de code-server — pas de nouveau stockage.
Fonctionne uniquement quand kydah-code tourne dans un code-server K8s (service interne).
