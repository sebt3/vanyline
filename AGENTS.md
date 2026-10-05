# vanyline — SpecDD + TTD

Before working on this project, read `.specdd/bootstrap.md`, then `.specdd/bootstrap.project.md`.

Assume the role, rules, workflow, and implementation constraints described in SpecDD. Treat
SpecDD specs as source-adjacent development contracts, not optional documentation. Adhere to
SpecDD rules unless explicitly instructed otherwise.

## Mémoire projet

`.claude/MEMORY.md` est l'index synthétique de la mémoire du projet (direction, décisions,
leçons). Le détail vit sous `.claude/memory/`. À lire en entrée de session.

## Flux de travail : SpecDD + test-first (TTD)

`Spec approuvée → tests depuis les Scenario → implémentation minimale → validation → [x]`.
Détail complet dans `.specdd/bootstrap.project.md`. Règles clés :

- Pas de code sans spec `.sdd` approuvée par Sébastien ; jamais d'implémentation avant les
  tests dérivés des `Scenario` de la spec (tests écrits d'abord, compilés, exécutés, vus échouer).
- Une spec `.sdd` par fichier `*.rs` du workspace (groupe 2–3 fichiers seulement si un seul
  contrat), décrivant **tout** le comportement observable du fichier. TS/Vue : même boucle,
  règle un-fichier-une-spec appliquée au fil de l'eau.
- CI et harnais outil ont leurs propres specs : `.github/workflows/workflows.sdd`,
  `tooling.sdd`, spec racine `vanyline.sdd`.
- Jamais de tâche `[x]` sans synthèse du `validator` au vert.

## Agents (`.opencode/agents/`)

| Agent | Rôle |
|---|---|
| `spec-dd` (primary) | Rédige les specs avec Sébastien, orchestre les sous-agents, ne code jamais directement |
| `spec-reverse` | Reverse-engineering : rédige la spec la plus complète possible d'un fichier source existant, sans jamais toucher le code |
| `implementer` | Réalise une spec précise : tests d'abord depuis les `Scenario`, puis implémentation minimale |
| `validator` | Vérifie implémentation ↔ spec + batterie tests/clippy/fmt, remonte une synthèse si ce n'est pas bon |
| `release` | Exécute `docs/release-runbook.md` (hors SpecDD : ops) |

## Harnais clippy

La table `[workspace.lints]` du `Cargo.toml` racine (héritée par chaque membre via
`lints.workspace = true`) porte le harnais (contractualisé par `tooling.sdd`) :
`unsafe_code` deny (exemptions ciblées justifiées uniquement), `missing_docs` warn (deny
après purge), `pedantic` + `cargo` et la
famille stricte (`unwrap_used`, `expect_used`, `panic`, `unreachable`, `dbg_macro`, `todo`,
`unimplemented`, `print_stdout`, `print_stderr`, `arithmetic_side_effects`) en `deny`. Seul
`multiple_crate_versions` est `allow`. En production : aucun
`unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!`/`println!`/`eprintln!` —
propager l'erreur du crate, logger via `tracing`. Exemptions uniquement sous `cfg(test)`
ou `#[allow]` justifié d'une ligne citant la spec. La dette préexistante est tracée dans
`tooling.sdd`, ne doit pas grossir, et se purge crate par crate.

## Commandes de validation

Batterie complète — identique à la CI (`test.yml`, voir sa spec `.github/workflows/workflows.sdd`),
lancée par le `validator`. Une tâche n'est terminée que si tout passe.

### Rust (workspace cargo UNIQUE : lib, app, cli, sandbox, tools, controller, crds, cfgstore, health)

```bash
cargo check --workspace
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings   # harnais — les flags ne sont pas optionnels
cargo fmt --all -- --check
```

`--all-targets` couvre le code de test (`-D warnings` comme la CI — vu sur F2/`vanyline-cfgstore`
2026-09-02 : unwraps de test verts en local, rouges en CI). `cargo fmt --all -- --check` est
obligatoire avant de considérer une tâche terminée.

### Specs

```bash
specdd lint        # doit rendre « 0 errors, 0 warnings » (binaire npm global : ~/.npm-global/bin/specdd)
```

Une spec créée ou modifiée doit passer `specdd lint` avant de servir de base à une implémentation.

### Frontend + packages

```bash
# packages partagés d'abord (le frontend en dépend via alias source)
npm run check --workspace=@vanyline/protocol && npm run test --workspace=@vanyline/protocol
npm run check --workspace=@vanyline/ui       && npm run test --workspace=@vanyline/ui

npm run build --workspace=frontend   # vue-tsc -b && vite build
npm run test  --workspace=frontend   # vitest run
npm run check --workspace=frontend   # vue-tsc --noEmit
```

Job CI `tsrs` (types ts-rs à jour) : `cargo test -p vanyline-lib --features ts-rs`
puis `cargo test -p vanyline-health --features ts-rs`,
puis `git diff --exit-code -- packages/protocol/src/generated/`.

## Git

- Pas de `Co-Authored-By` dans les messages de commit.
- Spec, code et tests dans la même PR ; tâches `[x]` uniquement après vérifications vertes.
- Messages d'erreur avec identifiant unique (format à contractualiser dans la spec concernée).

---

# Contexte architectural

> **État cible.** Cette section décrit l'architecture visée, telle que les specs `.sdd` la
> contractent (décisions du 2026-10-04). Là où le code diffère encore, la migration est tracée
> dans les `Tasks` de `vanyline.sdd` et des specs concernées ; en cas de divergence, la spec fait
> foi et le code est à aligner.

## Nature du projet

Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour
Kubernetes. Monorepo. Langages : Rust (un seul workspace cargo racine) + TypeScript/Vue 3
(frontend + packages partagés `@vanyline/protocol` et `@vanyline/ui`). L'extension VS Code
(`ext/`) est supprimée. Licence : BSD-3.

Deux piliers au-delà de l'IDE web : **la santé du code** (même vert en local et au central,
`vanyline-health`) et **les workflows** (moteur de miryad-core sur Restate, nœuds agent / rhai /
shell / gate de santé). Les agents (configuration, projet, sandbox) agissent par MCP avec les
droits de l'utilisateur.

## Architecture

```
[ navigateur : frontend ]                       [ kydah-code · cli · opérateur ]
    │ REST + WS chat (cookie OIDC)                        │ jeton du SA de l'Owner
    ▼                                                     │ (TokenRequest / kubeconfig)
[ app ] — namespace d'application <app-ns>                │
  auth OIDC · config LLM · agents · workflows             │
  endpoint Restate embarqué · MCP (config + domaine)      │
  relais sandbox : ticket WS, git, code-health            │
    │ jeton du SA de l'Owner (TokenRequest)               │
    ▼                                                     ▼
[ sandbox pod ] — namespace de l'Owner <app-ns>-<owner>
  port principal : /mcp · /git/* · /code-health · /jobs · POST /ws/ticket · /ws/* (ticket)
  port d'exploitation : /health · /metrics (sans auth, jamais publié)
  toolchains OCI image volumes · base cc/ld · libc-dev · make · git · curl · pkg-config · vim

[ controller ]  kube-rs · CRDs Owner / Project / Sandbox / Application
                crée le namespace de chaque Owner, ses RoleBindings, le Restate de l'Application
[ Restate ]     StatefulSet, sous-ressource de l'Application (moteur de workflow)
```

**Authentification de la sandbox : un seul mécanisme (2026-10-04).** La sandbox appartient à un
Owner ; l'accès est tout ou rien, sans niveaux ni groupes. Seul le jeton JWT émis par
l'apiserver pour le ServiceAccount de l'Owner (`owner-<nom>`, API TokenRequest, audience
`vanyline-sandbox`) est accepté. Qui peut obtenir ce jeton est décidé en amont : `app` vérifie le
lien utilisateur↔Owner avant de le demander ; un opérateur le demande avec son contexte kubeconfig.
Le SA de l'Owner n'a **aucun droit RBAC** : c'est une pure identité. Détail : `sandbox/src/auth.sdd`.

**Un Owner = un namespace.** La CR `Owner` vit dans le namespace de l'application ; le
controller crée `<app-ns>-<owner>` qui contient le SA, le PVC home, les Projects, les
Sandboxes et tout ce qu'ils produisent. Supprimer l'Owner supprime son namespace (finalizer).
Détail : `controller/src/namespace.sdd`.

**L'app n'est pas sur le chemin chaud éditeur** : le frontend ouvre les WebSocket éditeur
directement sur la sandbox avec un ticket court-vécu à usage unique (miné par `app`). En
revanche le navigateur ne voit **jamais** le jeton de l'Owner : l'HTTP vers la sandbox (git,
santé du code) passe par des relais de l'app.

## Composants

### frontend/
Shell web : coquille IDE dockable (Explorer/Editor/Terminal/Git/Chat/Workflows) + écrans
projet, accueil, paramètres, vue de sandbox responsive. TypeScript, Vue 3, `vue-router`,
dockview-vue, CodeMirror 6, xterm.js, `@nuxt/ui`/Tailwind, Reka UI. Build : Vite. Tests : Vitest.
**Tous les écrans sont responsives sauf l'IDE** (ordinateur seulement, largeur ≥ 1024 px) ; la
vue de sandbox responsive (`/overview`) affiche santé du code, workflows, runs et un chat.
Chaque écran (accueil, projet, sandbox, paramètres) offre un chat avec un agent. Contrat :
`frontend/src/frontend.sdd`. Détails d'architecture : `docs/architecture.md`.

### app/
Backend du frontend. Authentification OIDC (miryad-core), PostgreSQL (SeaORM).
- **Config LLM et agents** : providers, profils, toolsets, skills, serveurs MCP, agents
  (`vanyline-lib` pour l'exécution). Des **agents système** préconfigurés (config, accueil,
  projet, sandbox) : seul leur modèle est configurable. Réglages globaux (`app-settings`,
  modèle par défaut) réservés aux administrateurs.
- **Workflows** (`app/src/workflow/`) : endpoint Restate embarqué (port interne), kinds
  `agent`, `rhai`, `shell`, `gate` ; un projet définit des workflows de portée *projet* ou
  *sandbox* (shell et gate seulement en portée sandbox) ; runs indexés en base, statut lu dans
  Restate.
- **MCP** : générique de miryad-core (CRUD de la configuration) et de domaine (`/mcp/domain` :
  projets, sandboxes, santé du code, workflows, runs), tous deux avec l'identité de l'utilisateur.
- **Relais sandbox** : ticket WS, git, santé du code, avec le jeton de l'Owner.
**Ne proxifie pas** les WebSocket éditeur.

### sandbox/
Serveur Rust embarqué dans un pod Kubernetes, dans le namespace de l'Owner. Deux ports :
- **Principal** (authentifié par le jeton de l'Owner) : MCP HTTP streaming (`/mcp`, dont l'outil
  `code_health`), git (`/git/*`), **santé du code** (`/code-health`, état toujours disponible,
  recalcul après un délai de calme), **jobs** (`/jobs`, scripts shell durables pour les workflows),
  WebSocket éditeur (`/ws/fs`, `/ws/terminal`, `/ws/lsp/*`) par ticket.
- **Exploitation** (sans auth, jamais publié) : `/health`, `/metrics`.

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

Les outils de vérification de la santé du code (`cargo-llvm-cov`, `ruff`, `hadolint`, …) doivent
être présents dans les images toolchain, à versions épinglées.

**Contrainte** : base et images toolchain doivent être sur la **même famille de distro**
(trixie) — le loader du base résout la glibc ; un mismatch de distro est du hasard, pas du design.

### controller/
Opérateur Kubernetes (kube-rs). Gère 4 CRDs namespacées (`vanyline.solidite.fr/v1alpha1`) :
- **Owner** (namespace d'application) : identité cluster d'un utilisateur — crée son namespace
  `<app-ns>-<owner>`, son ServiceAccount (sans droit RBAC), son PVC home, et le RoleBinding qui
  délègue à `app` ses droits dans ce namespace ; référence optionnelle vers une Application
  (`application_ref`). Pas de PVC préexistant (`existing_pvc` supprimé).
- **Project** (namespace de l'Owner) : dépôt git — PVC workspace, repo bare, caches,
  Jobs/CronJob de maintenance (clone initial, fetch périodique)
- **Sandbox** (namespace de l'Owner) : pod de travail — référence un Project + une branche +
  liste de toolchains ; Ingress public si son Owner référence une Application
- **Application** (namespace d'application) : instance déployée d'`app` — Deployment + Service +
  Ingress, secrets OIDC/base de données/cookie référencés via `secretRef`, et, si `spec.restate`
  est présent, le StatefulSet + Service Restate (moteur de workflow)

**Règle — maintenance des projets** : toute action de maintenance du controller sur les
projets (clone, fetch, purge, worktrees, détection de langages) s'exécute dans un pod
portant **l'image sandbox**, via l'utilitaire `vanyline-maint` de l'image — jamais un
script shell assemblé par le controller. Les arguments passent en **argv**
(`command: ["vanyline-maint", ...]`) : aucun champ de CRD n'est interpolé dans une
commande shell. Conséquences : une seule image à maintenir, et l'outillage git/langages
disponible au même endroit pour la maintenance ET pour les sessions LLM.

Détails : `docs/architecture.md`, section "Opérateur Kubernetes — `vanyline-controller`".

### health/ — santé du code
Crate feuille `vanyline-health` : presets de vérifications par langage (rust, js-ts/vue, python,
dockerfile), parseurs (lcov, clippy, tsc, ruff, hadolint), restriction au code modifié, exécution
et rapport typé (exporté en TypeScript). Source unique des *invocations* de vérification : même
résultat en local (sandbox) et au central. Les règles restent celles du projet analysé.

### cli/
Binaire `vanyline` : chat interactif en **TUI ratatui** (sans sous-commande ; exige un terminal),
`run` (one-shot texte/JSONL), gestion de la configuration et des objets K8s, et serveur RPC
JSON-RPC `serve --stdio` (conservé comme interface d'intégration).

## Clients de la sandbox

| Client | Accès | Auth | Usage |
|--------|-------|------|-------|
| frontend (WebSocket éditeur) | Ingress par sandbox (`{name}.sandboxes.{host}`) | Ticket court-vécu à usage unique (miné par `app`) | Éditeur web, terminal, LSP |
| app | Service K8s interne | Jeton du SA de l'Owner (TokenRequest), après contrôle utilisateur↔Owner | Relais de ticket, git, santé du code, workflows (jobs, gate), agents (MCP) |
| kydah-code, cli (`--toolbox`), opérateur | Service K8s interne / Ingress | Jeton du SA de l'Owner (kubeconfig de l'utilisateur) | MCP pour les LLM |
| kubelet, supervision | Port d'exploitation | Aucune (non publié ; NetworkPolicy dédiée) | `/health`, `/metrics` |

## Interfaces inter-composants

| Source | Destination | Protocole | Auth |
|--------|-------------|-----------|------|
| frontend | app | HTTP REST (dont workflows, runs, relais santé/git) | Cookie OIDC (`HttpOnly`, stateless) |
| frontend | app | WebSocket (chat contextuel ; `/api/ws/sandbox-state`) | Cookie OIDC |
| frontend | sandbox | WebSocket (`/ws/fs`, `/ws/terminal`, `/ws/lsp/*`) | Ticket court-vécu à usage unique |
| app | sandbox | HTTP (ticket, `/git/*`, `/code-health`, `/jobs`, MCP) | Jeton du SA de l'Owner |
| app | app (loopback) | MCP (`/mcp`, `/mcp/domain`) pour les agents | Jeton d'API jetable de l'utilisateur |
| app | Restate | admin (enregistrement, statut) et ingress (lancement) | Réseau interne |
| Restate | app | endpoint de services (`DagInterpreter`, `StepDispatcher`) | Réseau interne, port dédié |
| controller | K8s API | K8s API | Service account |

## Logging

Jamais `println!`, `dbg!`, `eprintln!` côté Rust ni `console.log` côté frontend —
logger du crate (`tracing` côté Rust, logger du projet côté frontend ; le harnais clippy
`print_stdout`/`print_stderr` le fait respecter). Le TUI du cli écrit via le backend ratatui.

## Stack technique

| Composant | Langage | Dépendances clés |
|-----------|---------|-----------------|
| frontend | TypeScript | Vue 3, `vue-router`, dockview-vue, CodeMirror 6, xterm.js, `@nuxt/ui`, Tailwind, Reka UI, `@vanyline/ui`, `@vanyline/protocol` |
| `packages/protocol` (`@vanyline/protocol`) | TypeScript pur | types Rust↔TS (`ChatEvent`, santé du code générés par ts-rs ; `config-domain.ts`, `workflow-domain.ts` miroirs), enveloppes RPC, `RpcConnection` — zéro dépendance UI |
| `packages/ui` (`@vanyline/ui`) | TypeScript | Vue 3, `@nuxt/ui`, `reka-ui`, `@ai-sdk/vue` — composants chat + écrans config + `ConfigShell`, agnostiques du backend (ports `ChatTransport`/`ChatBackend`/`ConfigRepo` injectés) |
| `lib` (`vanyline-lib`) | Rust | types/événements, session d'agent, tâches autonomes, feature `k8s` (client, `owner_token`), ts-rs |
| `crds` (`vanyline-crds`) | Rust | définitions des CRDs du controller, nommage du namespace d'Owner |
| `tools` (`vanyline-tools`) | Rust | tools MCP partagés |
| `cfgstore` (`vanyline-cfgstore`) | Rust | stockage de configuration |
| `health` (`vanyline-health`) | Rust | santé du code : presets, parseurs, runner, rapport (ts-rs) |
| `llm-config` (`vanyline-llm-config`, à créer) | Rust | persistance et administration de la config LLM, partageable entre applications miryad-core |
| app | Rust | axum, SeaORM/PostgreSQL, `miryad-core` (features `mcp`, `workflow`), `vanyline-lib` (+ `k8s`), restate-sdk |
| sandbox | Rust | axum, `portable-pty`, `vanyline-tools`, `vanyline-health` |
| controller | Rust | kube-rs, `vanyline-crds` |
| cli | Rust | ratatui, crossterm, harness/CLI du projet |

## Structure des répertoires

```
vanyline/
├── Cargo.toml          # workspace Cargo UNIQUE (membres lib/app/cli/sandbox/tools/controller/crds/cfgstore/health, + llm-config à venir) + [workspace.lints] (harnais)
├── package.json        # workspace npm racine (workspaces: frontend, packages/*)
├── vanyline.sdd        # spec racine — contraintes transverses + feuille de route de migration
├── tooling.sdd         # spec du harnais clippy/fmt + dette tracée
├── .specdd/            # bootstrap SpecDD (bootstrap.md immutable, bootstrap.project.md = règles du projet)
├── .github/workflows/  # CI + sa spec workflows.sdd
├── frontend/           # shell web Vue 3 (dépend de @vanyline/ui + @vanyline/protocol)
├── packages/
│   ├── protocol/       # @vanyline/protocol — types Rust↔TS, RPC, RpcConnection
│   └── ui/             # @vanyline/ui — composants chat + config, agnostiques du backend
├── lib/  app/  cli/  sandbox/  tools/  controller/  crds/  cfgstore/  health/   # membres du workspace cargo
├── deploy/             # manifests / déploiement
├── toolchains/         # images OCI des toolchains sandbox
└── docs/
    ├── architecture.md
    └── release-runbook.md  # procédure release + redéploiement sur un cluster de test
```

## Release et déploiement sur un cluster de test

Procédure complète (validation → bump de version → tag → suivi CI →
redéploiement, pièges connus inclus) : `docs/release-runbook.md`.
