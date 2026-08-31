# vanyline — Contexte architectural

## Nature du projet

Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes.
Monorepo. Langages : Rust (app, sandbox, controller) + TypeScript/Vue 3 (frontend +
packages partagés `@vanyline/protocol` et `@vanyline/ui`).
Licence : BSD-3.

## Architecture

```
[ vanyline frontend ]          [ kydah-code (dans code-server K8s) ]
        │                                      │
   HTTP REST (app)                    MCP (K8s service interne)
   WS via ticket (sandbox)            + NetworkPolicy (SA TokenReview
        │                               jamais implémenté, cf. note)
        ▼                                      │
     [ app ]◄──────ticket WS (JWT)─────────────┤
  auth · config · K8s client                   ▼
  relais de ticket                    [ sandbox pod ]
                                       WS (ticket) · MCP
                                       Rust server
                                       + toolchains OCI image volumes
                                       + base : cc/ld · libc-dev · make
                                       + git · curl · pkg-config · vim

         [ controller ]
         kube-rs · CRDs : Application, Owner, Project, Sandbox
```

**Note (2026-08-12)** : ce diagramme a longtemps documenté un mécanisme SA TokenReview
pour kydah-code/l'app qui n'a jamais été implémenté (seul JWT/JWKS existe côté
sandbox, cf. `docs/architecture.md` section "Serveur MCP"). Corrigé ici ; kydah-code
ne consomme toujours pas la sandbox en pratique (pas démarré).

**L'app n'est pas sur le chemin chaud éditeur.** Le frontend et kydah-code se connectent
directement à la sandbox. L'app gère l'auth, le LLM, la config.

## Composants

### frontend/
Shell IDE web : coquille dockable (Explorer/Editor/Terminal/Workflow/Chat) + vue
Configuration. TypeScript, Vue 3, `vue-router`, dockview-vue, CodeMirror 6, xterm.js,
Element Plus, Reka UI. Build : Vite. Tests : Vitest. Détails complets :
`docs/architecture.md` section "Frontend — shell IDE Vue".
Se connecte directement à la sandbox en WebSocket, authentifié par un ticket court-vécu
à usage unique miné via `app` (`POST /api/sandboxes/{name}/ws-ticket`) — le navigateur
ne voit jamais le JWT OIDC brut.

### app/
Backend du frontend. Authentification OIDC native, stockage PostgreSQL.
Focus initial : interaction humain/LLM, gestion utilisateurs, API de configuration.
Rôle MCP : client — orchestre les appels LLM et les tools exposés par la sandbox.
**Ne proxifie pas** le WebSocket éditeur ni le MCP kydah-code.

### sandbox/
Serveur Rust embarqué dans un pod Kubernetes.
Expose deux interfaces :
- **WebSocket** : accès éditeur (`/ws/fs` filesystem, `/ws/terminal` PTY réel) — auth
  par ticket court-vécu à usage unique (`POST /ws/ticket`, cf. `docs/architecture.md`
  section "Serveur MCP" pour le détail : l'API WebSocket du navigateur ne permet pas
  de header `Authorization` sur le handshake)
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

**Un seul mode d'authentification réellement implémenté : JWT/JWKS OIDC**
(`sandbox/src/auth.rs`, `require_auth`). Ce fichier a longtemps documenté un second
mode, SA TokenReview + NetworkPolicy, pour kydah-code → sandbox — **jamais construit**,
corrigé le 2026-08-12 (découverte pendant `sandbox-ingress-wiring`). En pratique :
- **frontend → sandbox** (`/ws/fs`, `/ws/terminal`) : ticket court-vécu à usage
  unique, miné par `app` en présentant le JWT OIDC (`id_token`) de l'utilisateur
  authentifié — cf. section `frontend/` ci-dessus.
- **kydah-code → sandbox** : toujours pas câblé (pas démarré) ; NetworkPolicy par
  sandbox déjà en place (restreint aux pods du même namespace/owner + désormais aussi
  au pod `app` et au controller Ingress réel, cf. `docs/architecture.md`), mais aucun
  mécanisme d'auth applicatif pour ce client précis n'existe encore.

### controller/
Opérateur Kubernetes (kube-rs). Gère 4 CRDs namespacées (`vanyline.solidite.fr/v1alpha1`) :
- **Owner** : identité cluster d'un utilisateur — crée/référence un PVC (vanyline ou existant,
  ex: PVC du pod code-server pour kydah-code) + crée un ServiceAccount + attributs quota +
  référence optionnelle vers une Application (`application_ref`)
- **Project** : dépôt git d'un Owner — PVC workspace, repo bare, caches, Jobs/CronJob de
  maintenance (clone initial, fetch périodique)
- **Sandbox** : pod de travail — référence un Project + une branche + liste de toolchains ;
  Ingress public si son Owner référence une Application
- **Application** (`controller-application-crd`) : instance déployée d'`app` — Deployment
  + Service + Ingress, secrets OIDC/base de données/cookie référencés (ou cookie
  auto-généré) via `secretRef`, jamais en clair dans la CR

**Règle — maintenance des projets** : toute action de maintenance du controller sur les
projets (clone, fetch, purge, worktrees, détection de langages) s'exécute dans un pod
portant **l'image sandbox**, via l'utilitaire `vanyline-maint` de l'image — jamais un
script shell assemblé par le controller. Les arguments passent en **argv**
(`command: ["vanyline-maint", ...]`) : aucun champ de CRD n'est interpolé dans une
commande shell. Conséquences : une seule image à maintenir, et l'outillage git/langages
disponible au même endroit pour la maintenance ET pour les sessions LLM.

**Statut : implémenté et déployé** (sorti du statut déféré depuis WS-4/`controller-bootstrap`,
2026-07-11) — reconcilers Owner/Project/Sandbox/Application, NetworkPolicies egress
trois niveaux, suspension manuelle, endpoints git de la sandbox (WS-11/WS-13), Ingress
Application + Ingress par Sandbox (`controller-application-crd`/
`sandbox-ingress-wiring`, 2026-08-12). Détails : `docs/architecture.md`
section "Opérateur Kubernetes — `vanyline-controller`".

## Clients de la sandbox

| Client | Accès | Auth | Usage |
|--------|-------|------|-------|
| vanyline frontend | Ingress par sandbox (`{name}.sandboxes.{host}`) | Ticket court-vécu à usage unique (miné par `app`) | Éditeur web, terminal |
| kydah-code (dans code-server) | Service K8s interne, même namespace | Pas encore câblé (NetworkPolicy en place, aucun mécanisme d'auth applicatif) | MCP tools pour Qwen — pas démarré |
| app | Service K8s interne | JWT OIDC (`id_token` de l'utilisateur, pas un compte de service) | Relais de ticket WS (`/ws/ticket`) — pas d'orchestration MCP par `app` à ce jour |

## Interfaces inter-composants

| Source | Destination | Protocole | Auth |
|--------|-------------|-----------|------|
| frontend | app | HTTP REST | Cookie OIDC (`HttpOnly`, stateless) |
| frontend | app | WebSocket (chat, priorité basse — pas branché au shell IDE) | Cookie OIDC |
| frontend | app | WebSocket (`/api/ws/sandbox-state`, push des phases de sandbox) | Cookie OIDC |
| frontend | sandbox | WebSocket (`/ws/fs`, `/ws/terminal`) | Ticket court-vécu à usage unique |
| app | sandbox | HTTP (`POST /ws/ticket`, relais de ticket) | JWT OIDC (`id_token` de l'utilisateur) |
| kydah-code | sandbox | MCP HTTP streaming | Pas encore câblé — pas démarré |
| controller | K8s API | K8s API | Service account |

## Logging

TBD — à définir lors des premières features app et sandbox.
Convention : jamais `println!`, `dbg!`, `console.log` dans les sources.

## Stack technique

| Composant | Langage | Dépendances clés |
|-----------|---------|-----------------|
| frontend | TypeScript | Vue 3, `vue-router`, dockview-vue, CodeMirror 6, xterm.js, Element Plus, Reka UI, `@vanyline/ui`, `@vanyline/protocol` |
| `packages/protocol` (`@vanyline/protocol`) | TypeScript pur | types Rust↔TS (`ChatEvent` ts-rs, `config-domain.ts` miroir de `lib/src/domain.rs`), enveloppes RPC, `RpcConnection` — zéro dépendance UI |
| `packages/ui` (`@vanyline/ui`) | TypeScript | Vue 3, `@nuxt/ui`, `reka-ui`, `@ai-sdk/vue` — composants chat + 6 écrans config + `ConfigShell`, agnostiques du backend (ports `ChatTransport`/`ChatBackend`/`ConfigRepo` injectés) |
| app | Rust | axum, sqlx/PostgreSQL, `openidconnect`, `vanyline-lib` (+ feature `k8s`) |
| sandbox | Rust | axum, `portable-pty`, `vanyline-tools` |
| controller | Rust | kube-rs, `vanyline-crds` |

## Structure des répertoires

```
vanyline/
├── Cargo.toml          # workspace Cargo racine
├── package.json        # workspace npm racine (workspaces: frontend, packages/*)
├── frontend/           # shell IDE Vue 3 (dépend de @vanyline/ui + @vanyline/protocol)
│   └── src/
├── packages/
│   ├── protocol/       # @vanyline/protocol — types Rust↔TS, RPC, RpcConnection
│   │   └── src/
│   └── ui/             # @vanyline/ui — composants chat + config, agnostiques du backend
│       └── src/
├── app/                # backend Rust
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── sandbox/            # sandbox Rust
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── controller/         # opérateur K8s
│   ├── Cargo.toml      # sous-workspace
│   └── src/
├── docs/
│   ├── architecture.md
│   ├── release-runbook.md  # procédure release + redéploiement sur un cluster de test
│   └── features/       # design docs en cours
└── .tasks/             # tâches Qwen (jamais commité)
```

## Release et déploiement sur un cluster de test

Procédure complète (validation → bump de version → tag → suivi CI →
redéploiement, pièges connus inclus) : `docs/release-runbook.md`.

## Commandes de validation

### Rust (app, sandbox, controller)

```bash
cargo check --workspace        # vérification rapide
cargo test --workspace         # tests
cargo build --workspace        # build complet
cargo clippy --workspace       # linter
cargo fmt --all -- --check     # formatage — obligatoire avant de considérer une tâche terminée
```

`cargo fmt --all -- --check` fait partie des commandes de validation au même titre que les
autres : absent ici avant 2026-08-22, ce qui a laissé passer du code non formaté malgré la
permission déjà accordée à Cadence (`.opencode/agents/cadence.md`) — l'instruction manquait,
pas la permission.

### Frontend + packages

```bash
# packages partagés d'abord (le frontend en dépend via alias source)
npm run check --workspace=@vanyline/protocol && npm run test --workspace=@vanyline/protocol
npm run check --workspace=@vanyline/ui       && npm run test --workspace=@vanyline/ui

npm run build --workspace=frontend   # vue-tsc -b && vite build
npm run test  --workspace=frontend   # vitest run
npm run check --workspace=frontend   # vue-tsc --noEmit — vérification TypeScript/Vue
```

Job CI `tsrs` (types ts-rs à jour) : `cargo test -p vanyline-lib --features ts-rs`
puis `git diff --exit-code -- packages/protocol/src/generated/`.

## Conventions

- Pas de `println!`, `dbg!`, `eprintln!` dans les sources — utiliser le logger projet
- Pas de `console.log` dans le frontend — utiliser le logger projet
- Messages d'erreur avec identifiant unique (format TBD)
- TDD : définir les tests avant l'implémentation
- `.tasks/` jamais commité
- Modifications atomiques : une tâche = un périmètre limité de fichiers
