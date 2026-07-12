# Feature — cli-rpc-stdio (WS-1b)

## Ce que la feature fait

Ajoute `vanyline serve --stdio` : un serveur **JSON-RPC 2.0 sur stdio** (une trame JSON
par ligne, ndjson) exposant tout le harness — pour l'extension VS Code dédiée (même
stack UI que le frontend web) et tout autre client programmatique.

## Ce qu'elle ne fait pas

- Pas l'extension VS Code elle-même (→ `vscode-ext-bootstrap.md` ; les types TS du
  protocole vivent dans `packages/protocol`)
- Pas de transport HTTP/socket (stdio uniquement en v1)
- Pas de flow de permission — yolo assumé ; si un jour il existe (phase d'adoption),
  il passera par de nouvelles notifications, sans casser la v1

## Choix de framing

**ndjson** (un message JSON-RPC 2.0 compact par ligne, UTF-8, `\n`) plutôt que le framing
LSP `Content-Length` : trivial à implémenter des deux côtés, suffisant pour du texte.
stdout est réservé au protocole ; les logs vont sur stderr (tracing).

## Protocole — v1

Toutes les réponses d'erreur utilisent `error.data.code` = identifiant `VNL-*`.

### Requêtes (client → serveur)

| Méthode | Params | Résultat |
|---------|--------|----------|
| `initialize` | `{ protocolVersion: 1, workspace?: string }` | `{ protocolVersion: 1, serverVersion, workspaceRoot?, defaultAgent? }` |
| `shutdown` | — | `null` (le process sort après la réponse) |
| `config/agents` | — | `Agent[]` (résolus, couche source incluse) |
| `config/models` | — | `ModelProfile[]` |
| `config/toolsets` | — | `Toolset[]` |
| `config/skills` | — | `SkillMeta[]` |
| `conversations/list` | — | `ConversationSummary[]` |
| `conversations/get` | `{ id }` | `Conversation` (messages inclus) |
| `conversations/create` | `{ agent?, title? }` | `ConversationSummary` |
| `conversations/delete` | `{ id }` | `null` |
| `chat/send` | `{ conversationId, message, agent? }` | `{ text, toolCalls }` — répond à la FIN du tour |
| `chat/cancel` | `{ conversationId }` | `null` (M3 — accepté et no-op en v1, documenté) |

### Notifications (serveur → client)

| Méthode | Params |
|---------|--------|
| `chat/event` | `{ conversationId, seq: number, event: ChatEvent }` |

`event` est la sérialisation serde **directe** du `ChatEvent` de vanyline-lib — le même
schéma que le WebSocket de l'app web : un seul format d'événement dans tout le projet.
`seq` est un compteur croissant par conversation (détection de perte/désordre côté client).

### Concurrence

Un seul tour actif **par conversation** (un `chat/send` sur une conversation occupée
répond `VNL-RPC-002 busy`). Plusieurs conversations peuvent streamer en parallèle —
les notifications sont multiplexées par `conversationId`.

**Mécanisme (décidé tâche rpc-chat, 2026-07-12)** : la boucle stdio lit et dispatche
les lignes en séquence, mais `chat/send` est **spawné** (tâche tokio indépendante) —
sinon un tour en cours bloquerait la lecture de toute nouvelle requête, y compris sur
une AUTRE conversation, ce que ce paragraphe interdit explicitement. Pour que ça soit
sûr, les champs de `ServerState` qui doivent être visibles/mutables depuis une tâche
spawnée (le suivi des conversations occupées, le compteur `seq` par conversation, le
sender du canal d'écriture) sont chacun `Arc<std::sync::Mutex<...>>` (ou juste `Clone`
pour le sender mpsc, déjà `Clone` nativement) — PAS tout `ServerState` derrière un
`Arc<Mutex<ServerState>>` global : `store` (config, en lecture seule après
`initialize`) est un simple `Arc<FsConfigStore>` partagé sans verrou, et le reste de
l'état (`initialized`, etc.) reste local à la boucle séquentielle, jamais touché par
une tâche spawnée. Un `chat/send` concurrent sur la MÊME conversation reste refusé
(`VNL-RPC-002`) via le set `busy` ; deux conversations différentes tournent en vrai
parallèle, chacune dans sa propre tâche tokio.

### Cycle de vie

`initialize` obligatoire avant toute autre méthode (`VNL-RPC-001` sinon). Le paramètre
`workspace` fixe la racine pour le layering de config — c'est l'extension qui la connaît
(workspace folder VS Code), pas le cwd du process.

## Modules

Nouveau `cli/src/rpc/` : `mod.rs` (boucle read stdin / write stdout via canal mpsc,
un writer unique), `protocol.rs` (types serde requêtes/réponses), `handlers.rs`.
Réutilise `FsConfigStore` + `run_agent_turn` avec un `EventSink` qui pousse des
notifications dans le canal d'écriture. Pas de crate jsonrpc lourde : les types
JSON-RPC 2.0 de base en serde font l'affaire (~50 lignes).

## Risques et questions ouvertes

- **Annulation réelle** : dépend du support d'annulation de la lib (M3). L'API est en
  place dès la v1 pour ne pas casser le protocole.
- **Rechargement de config** : si l'utilisateur édite `.vanyline/` pendant que le serveur
  tourne — v1 : relecture à chaque `chat/send` (FsConfigStore sans cache, les fichiers
  sont petits). Pas de watcher.
- **Versionnage** : `protocolVersion` entier strict ; le serveur refuse une version
  inconnue (`VNL-RPC-003`).

## Découpage en tâches candidates

1. `rpc-skeleton` — boucle stdio + dispatch + initialize/shutdown + erreurs RPC. Tests : process piloté par stdin scripté.
2. `rpc-config` — méthodes config/* et conversations/*. Tests sur fixtures.
3. `rpc-chat` — chat/send + notifications chat/event + seq + verrou par conversation. Tests avec provider mock si possible, sinon test manuel scripté documenté.
4. `rpc-doc` — `docs/rpc-protocol.md` : spec autonome pour le développeur de l'extension (exemples de trames complets).
