# Inventaire vanyline ↔ lymaille (2026-11-07, lecture seule, à confirmer)

Établi par tailles de fichiers et imports (`grep`), **sans lecture fichier par fichier** : à confirmer lors de l'écriture
de chaque Solution. Aucun code ni spec modifié.

## À retirer (duplique lymaille) — environ 18 000 lignes Rust

| Où | Lignes | Remplacé par |
|---|---|---|
| `lib/src/session.rs`, `event.rs`, `prefixed_mcp.rs`, `model.rs`, `types.rs` (le reste de `lib` demeure) | ~3 500 | `TurnRunner`, `Event`, `mcp_module` de lymaille |
| `cfgstore/` (crate entière) | 9 061 | traits de stockage de lymaille implémentés dans `cli` (`cli/hote`, `cli#DEC-3`) |
| `app/src/system_agents.rs` | 1 326 | agents/contextes de lymaille (`app#DEC-1`, DSD `app`) |
| `app/src/config_store.rs` | 1 305 | stores de lymaille sur la base (ou `llm-config` de miryad-core) |
| `app/src/ws/chat.rs`, `api/conversations.rs` | 1 643 + 1 443 | sessions lymaille hébergées par `app` |
| `app/src/api/local_tools.rs` | 222 | accès code par le MCP de la sandbox (`app#DEC-1`) |
| `app/src/workflow/steps/agent.rs` | ? | nœud `agent` sur lymaille (`run_batch`) |

## À conserver tel quel

`sandbox` (39 k), `controller` (11 k), `crds`, `health`, `tools` (la sandbox en dépend : `ws/fs`, `mcp`, `tools_impl`),
`app` : API `owners`/`projects`/`sandboxes` (dont 1 331 lignes pour les sandboxes), `workflows` (3 105, hors nœud
`agent`), `mcp_domain.rs` (2 419 : **n'est pas un doublon**, c'est le serveur MCP de domaine que les sessions lymaille
consomment), auth, `db` pour projets/sandboxes/owner_links/workflows/runs.

## `vanyline-lib` reste, allégée : la librairie d'interface avec les sandboxes (décision du développeur, 2026-11-07)

`lib/src/k8s.rs` (851 lignes : `VnyK8sClient`, `owner_token`, TokenRequest) n'est pas du LLM. `lib` ne disparaît donc pas :
elle devient la **librairie d'interface avec le cluster et les sandboxes**, fonctionnalité principale de vanyline. Elle perd
seulement ses parties LLM (`session.rs`, `event.rs`, `prefixed_mcp.rs`, `model.rs`, `types.rs`, et ce qu'elle tire de
`cfgstore`) ; `k8s.rs` reste, et `error.rs` est à trier.

Piste à valider : y regrouper aussi le **client de la sandbox**, aujourd'hui dupliqué par nature entre `cli` (ticket
`/ws/ticket`, `/ws/fs` brut pour `sync`, `/git/status`, connexion MCP avec jeton d'Owner) et les relais d'`app` (ticket, git,
santé du code). Une seule implémentation du client plutôt que deux. Impose de réécrire l'identité de `lib` dans une spec/DSD
(le contenu actuel de `lib.sdd` décrit encore les sessions d'agent).

## À réaligner (pas de suppression aveugle)

- `app/src/db/entities` : 10 entités sur 17 sont liées au LLM (`agents`, `app_settings`, `chat_contexts`, `conversations`,
  `llm_providers`, `mcp_servers`, `messages`, `model_profiles`, `skills`, `toolsets`). Elles deviennent l'implémentation
  des traits de stockage de lymaille côté `app` (ou sont fournies par miryad-core) : dépend de la session miryad-core.
- `packages/protocol` : `generated/chat-event.ts` (ts-rs depuis `lib::event`) et `rpc.ts`. `Event` de lymaille est
  `Serialize` mais sans ts-rs : choisir entre générer les types TS ou les écrire à la main.
- `packages/ui` : `chat/` (968 lignes, dont `chatEventsToUIStream.ts` qui convertit `ChatEvent`) et `config/` (3 887
  lignes d'écrans de configuration des agents/providers/toolsets, dont le modèle de données change).
- `frontend/src/api/chatTransport.ts`, `chatWs.ts` : transport du chat vers `ws/chat`.
- `cli/` : voir les Solutions `cli/*` (TUI conservé, backend remplacé).

## Ordre proposé pour la tranche mince (étape 4 de la note de réorientation)

1. Redéfinir `lib` (spec/DSD : interface cluster et sandboxes) et en retirer la partie LLM ; `k8s.rs` reste.
2. `cli` d'abord : sa dépendance à lymaille est entière, sans miryad-core, et `cfgstore` n'a plus qu'elle comme client une
   fois `app` migré. La CLI est la tranche verticale la plus indépendante.
3. `app` après la session miryad-core (stores, sessions, endpoint) ; le front en dernier.
