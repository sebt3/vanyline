# Roadmap — vanyline harness & bootstrap composants

Ce document est le plan programme des prochaines semaines. Il ordonne les chantiers
(workstreams), fixe leurs dépendances et pointe vers les design docs de feature.
Il est maintenu par les développeurs et Claude ; les fichiers de tâches Qwen en dérivent.

## Objectif

Transformer le CLI actuel en un **enabler moderne** (niveau opencode) à configuration
simple, et **bootstrapper** les composants restants (sandbox, controller, extension
VS Code) en parallèle.

**Philosophie assumée** : les agents travaillent en mode yolo — pas de système de
permission avant la phase d'adoption externe. Le tooling est là pour accélérer les
modèles de confiance, pas pour les brider ; l'isolation vient de l'infrastructure
(pod sandbox, branche git dédiée), pas de garde-fous dans la boucle.

## Nouveaux concepts (transverses CLI + app web)

| Concept | Résumé | Design doc |
|---------|--------|------------|
| **ModelProfile** | Un modèle *paramétré* (provider + modèle brut + paramètres client). Les agents ne référencent jamais un modèle brut. | `features/harness-core.md` |
| **Toolset** | Groupe cohérent d'outils (sélection fine par serveur MCP + outils locaux) + fragment de prompt système. | `features/harness-core.md` |
| **Skill** | Connaissance procédurale chargée à la demande (lazy). Format SKILL.md compatible écosystème. | `features/harness-core.md` |
| **Agent v2** | `primary` ou `subagent`. Agrège 1 ModelProfile + toolset(s) + prompt. Tool builtin d'invocation des subagents. | `features/harness-core.md` |
| **Config nommée** | Tout est identifié par **nom** (les UUID restent internes à l'app/PG). YAML + markdown côté CLI, PG côté app. | `features/cli-harness.md`, `features/app-harness-parity.md` |
| **ChatEvent** | Événement de session unique (token, tool call/result, usage, subagent…) sérialisable — partagé par REPL, WS app, JSON-RPC et webview VS Code. Types TS dans `packages/protocol`. | `features/harness-core.md`, `features/vscode-ext-bootstrap.md` |

## Workstreams et dépendances

```
WS-0  harness-core (lib)            ─── socle Rust, bloque WS-1 et WS-2
WS-1  cli-harness  (cli)            ─── dépend de WS-0
WS-1b cli-rpc-stdio (cli)           ─── dépend de WS-1
WS-2  app-harness-parity (app+front)─── dépend de WS-0, parallèle à WS-1
WS-3  sandbox-bootstrap (sandbox)   ─── indépendant (tools + kydah-mcp-template) → jour 1
WS-4  controller-bootstrap          ─── isolé (kube-rs) → jour 1
WS-5  tools-v2 (tools)              ─── indépendant (crate feuille pure) → jour 1
WS-6  vscode-ext-bootstrap (ext/)   ─── dépend de WS-1b → fin de sprint
```

Quatre fronts avancent en parallèle dès le jour 1 : **WS-0**, **WS-3**, **WS-4**, **WS-5**.
WS-1 et WS-2 s'ouvrent dès que les interfaces de WS-0 sont figées (le gel des types et
traits suffit, pas besoin que tout WS-0 soit terminé). WS-3 consomme la surface d'outils
de WS-5 au fil de l'eau (schémas partagés dans `tools/src/mcp.rs`).

## Jalons

### M0 — Socles (WS-0 + démarrages parallèles)
- WS-0 : types v2 name-keyed, `ChatEvent`/`EventSink`, `ConfigStore`, `ModelProfile`
  appliqué au build de modèle, résolution de toolsets, tool results + usage remontés.
- WS-5 : surface d'outils v2 (edit_file, search, find_files, sorties bornées).
- WS-3 : fork du template, interop client rmcp validée.
- WS-4 : CRDs + reconciler Owner.
**Critère de sortie WS-0** : `cargo test --workspace` vert, cli et app compilent sur
le nouveau cœur (adaptation minimale, sans nouvelles features).

### M1 — Harness CLI + parité app
- WS-1 : config YAML layered (globale + workspace), commandes par nom, skills,
  tool subagent, injection contexte workspace (AGENTS.md).
- WS-2 : migrations PG, `PgConfigStore`, API CRUD v2, WS sur `ChatEvent`, UI minimale.
- WS-3/WS-4 : sandbox conteneurisée ; reconcilers Project et Sandbox.
**Critère de sortie** : le CLI exécute un agent avec toolsets + skills + subagents
définis dans `.vanyline/` d'un workspace ; l'app expose les mêmes concepts via API.

### M2 — Frontends et composants cluster
- WS-1b : `vanyline serve --stdio` (JSON-RPC) + `packages/protocol` (types TS).
- WS-6 : extension VS Code minimale (chat panel Svelte sur le RPC stdio).
- WS-3 : sandbox déployée, MCP répondant depuis le cluster.
- WS-4 : Owner + Project + Sandbox réconciliés de bout en bout (worktree + caches).
**Critère de sortie** : l'extension VS Code dialogue avec le CLI en stdio ; un pod
sandbox créé par le controller sur une branche d'un Project répond au MCP.

### M3 — Consolidation
Annulation de tour en cours (RPC + lib), fidélité de l'historique (tool calls rejoués),
taxonomie d'erreurs complétée, docs. Contenu ajusté selon les découvertes de M1/M2.

## Hors scope (rappel)

- Intégration app ↔ sandbox (attend la convergence via controller)
- Multi-utilisateur complet, quotas
- Permissions/approbation des tools : n'arrivera qu'en phase d'adoption externe (yolo assumé)
- Compaction/gestion automatique du contexte (important pour les petits modèles — plus tard)
- CRD Application, merge/push automatique des branches sandbox

## Ordre de travail suggéré pour l'exécution

1. WS-0 en séquence stricte (c'est le socle — pas de parallélisme intra-WS-0 avant le gel des types).
2. WS-3, WS-4 et WS-5 en parallèle de WS-0 dès le premier jour.
3. À la fin du gel WS-0 : ouvrir WS-1 et WS-2 en parallèle.
4. WS-1b après les commandes de base de WS-1 ; WS-6 après WS-1b.

Chaque design doc contient son découpage en tâches candidates (à transformer en
`.tasks/<feature>/task-XX-*.md` juste-à-temps, une ou deux d'avance maximum).
