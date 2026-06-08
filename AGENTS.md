# vanyline — Contexte architectural

## Nature du projet

Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes.
Monorepo. Langages : Rust (app, sandbox, controller) + TypeScript/Svelte 5 (frontend).
Licence : BSD-3.

## Architecture

```
┌──────────────────────────────────────┐
│              frontend                │
│   TypeScript · Svelte 5 · CM6        │
└──────────────────┬───────────────────┘
                   │ HTTP / WebSocket
┌──────────────────▼───────────────────┐
│                 app                  │
│   Rust · OIDC · Redis · PGVector     │
└───────────┬──────────────────────────┘
            │ WebSocket (editor)
            │ MCP HTTP streaming (LLM)
┌───────────▼──────────────────────────┐
│           sandbox (pod K8s)          │
│   Rust server · git · curl · make    │
│   + toolchains via volumes OCI       │
└──────────────────────────────────────┘

         controller (déféré)
         kube-rs · CRDs : Application, Owner, Sandbox
```

## Composants

### frontend/
Interface utilisateur : éditeur de code web + conversation LLM.
TypeScript, Svelte 5, CodeMirror 6.
Framework de build : TBD (SvelteKit vs Vite+Svelte).

### app/
Backend du frontend. Authentification OIDC native, cache Redis, stockage PostgreSQL+PGVector.
Focus initial : interaction humain/LLM, gestion utilisateurs, API de configuration.
Rôle MCP : client — orchestre les appels LLM et les tools exposés par la sandbox.

### sandbox/
Serveur Rust embarqué dans un pod Kubernetes.
Expose deux interfaces :
- **WebSocket** : accès éditeur (commandes, filesystem, terminal)
- **MCP HTTP streaming** : tools pour les LLM

Image de base : Debian slim + binaire serveur + git, curl, make, vim.
Toolchains : montées en volumes OCI à la création du pod, PATH/LD_LIBRARY_PATH injectés.

### controller/
Opérateur Kubernetes (kube-rs). Gère 3 CRDs namespacés :
- **Application** : instance deployée de vanyline
- **Owner** : espace de stockage utilisateur (wrape un PVC, aura des attributs quota)
- **Sandbox** : pod de travail — référence un Owner (sous-répertoire PVC) + liste de toolchains

**Statut : déféré.** Pour le dev/test de la sandbox, un script shell/Python crée les pods directement.

## Interfaces inter-composants

| Source | Destination | Protocole |
|--------|-------------|-----------|
| frontend | app | HTTP REST + WebSocket |
| app | sandbox | WebSocket (editor) + MCP HTTP streaming (LLM) |
| controller | sandbox pod | K8s API (création pod, montage volumes OCI) |

## Logging

TBD — à définir lors des premières features app et sandbox.
Convention : jamais `println!`, `dbg!`, `console.log` dans les sources.

## Stack technique

| Composant | Langage | Dépendances clés |
|-----------|---------|-----------------|
| frontend | TypeScript | Svelte 5, CodeMirror 6 |
| app | Rust | TBD (axum probable), sqlx, redis |
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
# [TBD — à compléter une fois le framework de build choisi]
npm run check                  # vérification TypeScript
npm run test                   # tests
npm run build                  # build
```

## Conventions

- Pas de `println!`, `dbg!`, `eprintln!` dans les sources — utiliser le logger projet
- Pas de `console.log` dans le frontend — utiliser le logger projet
- Messages d'erreur avec identifiant unique (format TBD)
- TDD : définir les tests avant l'implémentation
- `.tasks/` jamais commité
- Modifications atomiques : une tâche = un périmètre limité de fichiers
