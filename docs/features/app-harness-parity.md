# Feature — app-harness-parity (WS-2)

## Statut

Backend terminé et clos dans `docs/architecture.md` (section "Backend web —
`vanyline-app`") : migrations PG (`model_profiles`/`toolsets`/`skills`/`agents` v2),
`PgConfigStore`, API REST CRUD par nom, WebSocket chat sur `ChatEvent` avec streaming
réel (`ChannelSink`). Ce fichier ne couvre plus que ce qui reste — la partie
frontend, jamais commencée.

## Ce qui reste à faire

Le frontend (`frontend/src/`) est resté au niveau MVP (`initial-app-frontend`, clos) :
deux pages (`Login.svelte`, `Chat.svelte`), `ChatMessage.svelte` ne rend que le texte
et les tool calls à plat. Il manque :

1. **`front-crud`** — écrans de gestion CRUD : model profiles, toolsets, skills,
   agents (formulaires simples, un client API par ressource sur le modèle de
   `frontend/src/lib/api/agents.ts` — il manque son équivalent pour
   `model-profiles`/`toolsets`/`skills`). 2-3 tâches probables (une par groupe de
   ressources similaires).
2. **`front-chat`** — enrichir `ChatMessage.svelte` pour les événements `ChatEvent`
   non encore rendus : tool result repliable/dépliable, badge d'usage (tokens),
   sous-fil visuellement distinct pour les événements de subagent
   (`ChatEvent::SubagentEvent`).

## Ce qu'elle ne fait pas

- Pas d'exécution d'outils locaux dans l'app (règle intacte : l'app est sur le chemin
  froid, ses seuls outils viennent des MCP — plus tard de la sandbox)
- Pas de partage de config entre utilisateurs (chaque user a son espace de config)
- Pas d'import/export YAML ↔ PG (idée notée, hors scope)

## Risques et questions ouvertes

- Aucun risque backend restant (résolu). Le seul risque frontend : le schéma
  `ChatEvent` (`vanyline_lib::event`) doit être lu directement côté TS tant que
  `packages/protocol` (génération `ts-rs`, cf. `docs/architecture.md` section
  "Workspace TypeScript") n'existe pas — pas de double maintenance de types prévue
  pour ces deux tâches, le typage restera manuel côté `frontend/src/lib/types.ts`.
