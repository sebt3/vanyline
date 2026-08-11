# vanyline — Contexte architectural

## Nature du projet

Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes.
Monorepo. Langages : Rust (app, sandbox, controller) + TypeScript/Svelte 5 (frontend).
Licence : BSD-3.

## Architecture

```
[ vanyline frontend ]          [ kydah-code (dans code-server K8s) ]
        │                                      │
   HTTP REST                         MCP (K8s service interne)
   WS direct (JWT)                   + NetworkPolicy + SA TokenReview
        │                                      │
        ▼                                      ▼
     [ app ]◄──────MCP HTTP streaming──── [ sandbox pod ]
  auth · config                          WS (JWT) · MCP
  LLM orchestration                      Rust server
                                         + toolchains OCI image volumes
                                         + base : cc/ld · libc-dev · make
                                         + git · curl · pkg-config · vim

         [ controller ] (déféré)
         kube-rs · CRDs : Application, Owner, Sandbox
```

**L'app n'est pas sur le chemin chaud éditeur.** Le frontend et kydah-code se connectent
directement à la sandbox. L'app gère l'auth, le LLM, la config.

## Composants

### frontend/
Interface utilisateur : éditeur de code web + conversation LLM.
TypeScript, Svelte 5, CodeMirror 6, svelte-spa-router, Tailwind CSS 4.
Build : Vite. Tests : Vitest. Composants : Storybook.
Se connecte directement à la sandbox en WebSocket (JWT validé par la sandbox).

### app/
Backend du frontend. Authentification OIDC native, cache Redis, stockage PostgreSQL+PGVector.
Focus initial : interaction humain/LLM, gestion utilisateurs, API de configuration.
Rôle MCP : client — orchestre les appels LLM et les tools exposés par la sandbox.
**Ne proxifie pas** le WebSocket éditeur ni le MCP kydah-code.

### sandbox/
Serveur Rust embarqué dans un pod Kubernetes.
Expose deux interfaces :
- **WebSocket** : accès éditeur (commandes, filesystem, terminal) — auth JWT
- **MCP HTTP streaming** : tools pour les LLM et pour kydah-code

Image de base : Debian slim + binaire serveur + **substrat natif commun** (compilateur C
`cc`/`ld` + binutils, `libc-dev`, make, pkg-config) + git, curl, vim. Le linker C est
obligatoire dans le base : sans lui aucune compilation native ne lie (rust, node-gyp, cgo…).

L'image embarque un second binaire : **`vanyline-maint`** (`sandbox/src/bin/maint.rs`),
l'utilitaire de maintenance des workspaces invoqué par les Jobs du controller
(`init`/`fetch`/`purge`/`checkout`/`remove`/`detect`) — cf. la règle de maintenance
dans la section controller et `docs/architecture.md` (section `vanyline-maint`).

Toolchains : images OCI standard (ex: `rust:slim-trixie`, `node:trixie-slim`) montées via
`volumes[].image` (feature K8s native, GA depuis v1.36, prérequis : v1.31+). Une toolchain
devient utilisable par **injection d'env au démarrage**, jamais par magie :
- `PATH` → `…/usr/local/bin` (ou `cargo/bin`) du volume
- `LD_LIBRARY_PATH` → `…/usr/lib/<arch>-linux-gnu` du volume (le loader du base ne trouve pas
  les libs du volume sinon — ex: `libatomic.so.1` pour node)
- **env spécifiques par toolchain** (ex: rust → `RUSTUP_HOME` sur le volume ;
  `CARGO_HOME` writable **hors volume**, dans le PVC du Owner car le volume est read-only)

**Contrainte** : base et images toolchain doivent être sur la **même famille de distro**
(trixie) — le loader du base résout la glibc ; un mismatch de distro est du hasard, pas du design.

**Deux modes d'authentification :**
- **JWT** (frontend → sandbox via ingress) : token OIDC émis par l'app, validé par la sandbox
- **SA TokenReview + NetworkPolicy** (kydah-code → sandbox via service K8s interne) :
  la sandbox valide le service account du pod appelant via l'API K8s TokenReview ;
  une NetworkPolicy par sandbox restreint l'accès aux pods du même namespace avec les bons labels

### controller/
Opérateur Kubernetes (kube-rs). Gère 3 CRDs namespacés :
- **Application** : instance deployée de vanyline
- **Owner** : identité cluster d'un utilisateur — crée/référence un PVC (vanyline ou existant,
  ex: PVC du pod code-server pour kydah-code) + crée un ServiceAccount + attributs quota
- **Sandbox** : pod de travail — référence un Owner (sous-répertoire PVC) + liste de toolchains

**Règle — maintenance des projets** : toute action de maintenance du controller sur les
projets (clone, fetch, purge, worktrees, détection de langages) s'exécute dans un pod
portant **l'image sandbox**, via l'utilitaire `vanyline-maint` de l'image — jamais un
script shell assemblé par le controller. Les arguments passent en **argv**
(`command: ["vanyline-maint", ...]`) : aucun champ de CRD n'est interpolé dans une
commande shell. Conséquences : une seule image à maintenir, et l'outillage git/langages
disponible au même endroit pour la maintenance ET pour les sessions LLM.

**Statut : implémenté et déployé** (sorti du statut déféré depuis WS-4/`controller-bootstrap`,
2026-07-11) — reconcilers Owner/Project/Sandbox, NetworkPolicies egress trois niveaux,
suspension manuelle, endpoints git de la sandbox (WS-11/WS-13). Détails : `docs/architecture.md`
section "Opérateur Kubernetes — `vanyline-controller`".

## Clients de la sandbox

| Client | Accès | Auth | Usage |
|--------|-------|------|-------|
| vanyline frontend | Ingress K8s | JWT (OIDC via app) | Éditeur web, terminal |
| kydah-code (dans code-server) | Service K8s interne, même namespace | SA TokenReview + NetworkPolicy | MCP tools pour Qwen |
| app (LLM orchestration) | Service K8s interne | SA TokenReview (SA du Owner concerné) | MCP HTTP streaming pour les tool calls LLM |

## Interfaces inter-composants

| Source | Destination | Protocole | Auth |
|--------|-------------|-----------|------|
| frontend | app | HTTP REST | OIDC token |
| frontend | sandbox | WebSocket | JWT |
| kydah-code | sandbox | MCP HTTP streaming | SA TokenReview + NetPol |
| app | sandbox | MCP HTTP streaming | SA TokenReview (SA du Owner concerné) |
| controller | K8s API | K8s API | Service account |

## Logging

TBD — à définir lors des premières features app et sandbox.
Convention : jamais `println!`, `dbg!`, `console.log` dans les sources.

## Stack technique

| Composant | Langage | Dépendances clés |
|-----------|---------|-----------------|
| frontend | TypeScript | Svelte 5, CodeMirror 6, svelte-spa-router, Tailwind CSS 4 |
| app | Rust | TBD (axum probable), sqlx, redis, client API Ollama-compatible |
| sandbox | Rust | TBD |
| controller | Rust | kube-rs |

## Structure des répertoires

```
vanyline/
├── Cargo.toml          # workspace Cargo racine
├── package.json        # workspace npm racine
├── frontend/           # frontend Svelte 5
│   └── src/
├── app/                # backend Rust
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── sandbox/            # sandbox Rust
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── controller/         # opérateur K8s (déféré)
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── docs/
│   ├── architecture.md
│   └── features/       # design docs en cours
└── .tasks/             # tâches Qwen (jamais commité)
```

## Commandes de validation

### Rust (app, sandbox, controller)

```bash
cargo check --workspace        # vérification rapide
cargo test --workspace         # tests
cargo build --workspace        # build complet
cargo clippy --workspace       # linter
```

### Frontend

```bash
npm run build                  # vue-tsc -b && vite build
npm run test                   # vitest run
npm run check                  # vue-tsc --noEmit — vérification TypeScript/Vue
```

## Conventions

- Pas de `println!`, `dbg!`, `eprintln!` dans les sources — utiliser le logger projet
- Pas de `console.log` dans le frontend — utiliser le logger projet
- Messages d'erreur avec identifiant unique (format TBD)
- TDD : définir les tests avant l'implémentation
- `.tasks/` jamais commité
- Modifications atomiques : une tâche = un périmètre limité de fichiers
