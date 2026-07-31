# Feature — vscode-ext-bootstrap (WS-6)

## Ce que la feature fait

Bootstrap de l'extension VS Code : un front-end graphique **minimal mais correct** du
CLI en JSON-RPC stdio — de quoi tester le mode `serve --stdio` en conditions réelles.
Même stack UI que le frontend web (Svelte 5, Tailwind).

Fin de sprint — dépend de WS-1b (`cli-rpc-stdio`).

## Ce qu'elle ne fait pas

- Pas d'intégration éditeur (diffs inline, code lens, tree views élaborées) — c'est un
  chat panel, point
- Pas de publication marketplace (packaging vsix local uniquement)
- Pas de mutualisation des composants UI avec le frontend web en v1 (extraction d'un
  `packages/ui` notée pour plus tard, quand les deux fronts auront convergé visuellement)

## Emplacement — extension du workspace npm

```
vanyline/
├── package.json             # workspaces: [frontend, ext, packages/*]
├── frontend/                # web app (existant)
├── ext/                     # extension VS Code
│   ├── src/extension.ts     # host : lifecycle, spawn du CLI, webview
│   └── webview/             # UI Svelte 5 + Tailwind (build vite séparé)
└── packages/
    └── protocol/            # @vanyline/protocol — types ChatEvent + RPC, client ndjson
```

`packages/protocol` est le point important : les types TypeScript de `ChatEvent` et du
protocole RPC y vivent **une seule fois**, consommés par l'extension (stdio) et par le
frontend web (WebSocket — même schéma d'événement, cf. harness-core). **Décidé
(2026-07-06)** : génération depuis les types Rust via `ts-rs` (feature-gated dans
vanyline-lib, fichier généré commité + vérifié en CI) pour éliminer la dérive Rust↔TS.
Repli si ts-rs déçoit sur les enums serde taggées : types manuels + tests de conformité
sur fixtures JSON produites par la lib.

## Architecture de l'extension

**Host (`extension.ts`)** :
- Spawn de `vanyline serve --stdio` (chemin binaire via setting `vanyline.serverPath`,
  défaut `vanyline` dans le PATH) ; `initialize` avec le workspace folder courant.
- Client ndjson (de `@vanyline/protocol`) : corrélation id → promesse, dispatch des
  notifications `chat/event` vers la webview.
- stderr du CLI → OutputChannel « vanyline » ; redémarrage sur crash (avec backoff),
  état visible dans la status bar.

**Webview (panel « vanyline »)** :
- Sélecteur d'agent (depuis `config/agents`), liste/reprise des conversations.
- Fil de conversation streamé : tokens, tool calls avec résultat repliable, usage,
  événements subagent indentés — le rendu de référence des `ChatEvent`.
- Zone de saisie multi-ligne. C'est tout.

Communication host ↔ webview : `postMessage` transparent (les `ChatEvent` passent tels
quels — le host ne les interprète pas).

## Patterns à reprendre de kydah-code

kydah-code (co-développé, sources : `/home/coder/projets/kydah/kydah-code`) a déjà
résolu : webview Svelte buildée par vite (CSP, URIs de ressources), packaging vsce,
structure de settings. S'en inspirer plutôt que redécouvrir — c'est le même duo
d'auteurs.

## Risques et questions ouvertes

- **CSP webview + assets vite** : connu mais toujours pénible ; la tâche 1 (squelette)
  doit inclure un « hello webview » buildé pour purger le sujet tôt.
- **Cycle de vie du process CLI** : un serveur par fenêtre VS Code (workspace ≠ global).
  Fermeture propre via `shutdown` sur deactivate.
- **ts-rs** : à valider sur les enums serde taggées (`#[serde(tag = "type")]`) — si le
  mapping est mauvais, repli sur types manuels + fixtures.

## Découpage en tâches candidates

1. `protocol-package` — @vanyline/protocol : types ChatEvent/RPC (ts-rs ou manuel + fixtures), client ndjson avec tests vitest sur transport mocké.
2. `ext-skeleton` — activation, spawn/initialize/shutdown, OutputChannel, hello webview buildée. (Le spike CSP/vite.)
3. `ext-chat` — fil de conversation streamé complet + saisie.
4. `ext-pickers` — sélecteur d'agent + liste/reprise de conversations + status bar.
5. `ext-package` — build vsix, README d'installation, test manuel de bout en bout documenté.
