# Protocole RPC de `vanyline serve --stdio`

Spec autonome pour le développeur d'un client programmatique (extension
VS Code notamment) — pas besoin de lire le code Rust pour l'implémenter.
Reflète l'implémentation réelle (`cli/src/rpc/{mod,protocol,handlers}.rs`).

## Transport

`vanyline serve --stdio` lance un serveur JSON-RPC 2.0 sur stdio :

- **ndjson** — une trame JSON compacte par ligne, UTF-8, terminée par `\n`.
  Pas de framing `Content-Length` (LSP).
- **stdout est réservé au protocole** — n'écrivez jamais rien d'autre
  dessus. Les logs du serveur vont sur **stderr** (tracing), jamais sur
  stdout.
- Le process sort après avoir répondu à `shutdown`, ou à EOF sur stdin.

## Cycle de vie

`initialize` doit être la **première** requête. Toute autre méthode avant
ça reçoit `VNL-RPC-001`. Appeler `initialize` une seconde fois est
autorisé (réinitialise l'état — voir "Concurrence" plus bas pour ce que ça
implique).

## Enveloppe des messages

Trois formes de trames, toutes se terminant par `\n` :

### Requête (client → serveur)

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}
```

`id` peut être un nombre ou une chaîne. `params` est optionnel selon la
méthode.

### Réponse (serveur → client)

Toujours **soit** `result` **soit** `error`, jamais les deux, jamais
aucun des deux :

```json
{"jsonrpc":"2.0","id":1,"result":{"...":"..."}}
```
```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"...","data":{"code":"VNL-RPC-001"}}}
```

`error.code` est un code JSON-RPC 2.0 standard (`-32700` parse error,
`-32601` method not found, `-32000` server error — plage réservée à
l'usage du serveur). **`error.data.code` est l'identifiant `VNL-RPC-*`
faisant foi** — c'est CELUI-LÀ qu'un client doit tester dans son code,
pas `error.code` (qui ne fait que catégoriser en gros).

### Notification (serveur → client, PAS de réponse attendue)

```json
{"jsonrpc":"2.0","method":"chat/event","params":{"conversationId":"...","seq":0,"event":{"type":"token","content":"Bonjour"}}}
```

Pas de champ `id` — c'est la seule façon fiable de distinguer une
notification d'une réponse si vous démultipléxez les deux sur le même flux
de lecture.

### ⚠️ camelCase vs snake_case — piège à connaître

Les enveloppes RPC (`InitializeResult`, `ConversationSummary`,
`ChatSendParams`/`ChatSendResult`, `ChatEventNotificationParams`) sont en
**camelCase** (`protocolVersion`, `conversationId`, `messageCount`...).

MAIS `config/*` et `conversations/get` retournent des objets du **domaine
vanyline_lib tels quels** (`Agent`, `ModelProfile`, `Toolset`, `SkillMeta`,
`Conversation`) — ceux-là sont en **snake_case natif** de la lib
(`system_prompt`, `max_tokens`, `local_tools`, `tool_calls`...), PAS
convertis en camelCase. Un client qui s'attend à du camelCase partout va
se planter sur `config/agents`/`config/models`/`config/toolsets`/
`conversations/get`. Voir les exemples ci-dessous pour la forme exacte.

Les enveloppes d'**écriture** `config/<domain>/*` sont elles aussi en
**camelCase** : `layer`, `name`, `patch`, `item`, `body` (params de
`create`/`update`/`delete`, cf. « Écriture de configuration » plus bas).
MAIS le **contenu** de `item` et de `patch` est le type de domaine en
**snake_case natif** (`system_prompt`, `local_tools`, `max_tokens`,
`api_key`...) — la conversion ne s'applique jamais aux objets de domaine
portés par l'enveloppe, seulement à l'enveloppe elle-même.

Dernier piège à figer : le domaine RPC s'appelle **`models`** (la map
`models:` du `config.yaml`, nom historique de la CLI) alors que
`@vanyline/ui` l'appelle `profiles`. Les deux noms ne désignent qu'une
seule et même chose — c'est le pont RPC de la feature extension (F4) qui
traduit l'un en l'autre.

## Codes d'erreur `VNL-RPC-*`

| Code | Signification |
|------|---------------|
| `VNL-RPC-000` | Requête malformée (JSON invalide, champ requis manquant/mal typé, UUID invalide) |
| `VNL-RPC-001` | Méthode appelée avant `initialize` |
| `VNL-RPC-002` | `chat/send` sur une conversation déjà occupée (tour en cours) |
| `VNL-RPC-003` | `protocolVersion` inconnu dans `initialize` |
| `VNL-RPC-004` | Méthode inconnue |
| `VNL-RPC-005` | `conversationId` référence une conversation inexistante |
| `VNL-RPC-006` | Erreur de config (`config/*`) non typée : lecture en erreur, nom inconnu sur une action `test`, cible de test injoignable, ou erreur du `store` hors des cas 011–015 |
| `VNL-RPC-007` | Erreur de stockage des conversations (I/O disque) |
| `VNL-RPC-008` | Aucun agent résolvable pour `chat/send` (ni param, ni conversation, ni défaut) |
| `VNL-RPC-009` | Le tour LLM a échoué (`run_agent_turn`) |
| `VNL-RPC-010` | Erreur K8s (client injoignable ou appel API échoué, `owners/projects/sandboxes`) |
| `VNL-RPC-011` | `CONFIG_WRITE_ERROR` — échec d'écriture disque ou de sérialisation (`WriteError`/`Io`) |
| `VNL-RPC-012` | `CONFIG_NOT_FOUND` — `update`/`delete` sur un `name` absent de la **couche ciblée** |
| `VNL-RPC-013` | `CONFIG_NAME_CONFLICT` — `create` sur un `name` déjà présent dans la **couche ciblée** |
| `VNL-RPC-014` | `CONFIG_INVALID_NAME` — `name` violant la contrainte anti-traversal |
| `VNL-RPC-015` | `CONFIG_VALIDATION` — valeur énumérée invalide (`type` de provider/MCP, `mode` d'agent) ou `item` non désérialisable |

## Méthodes

### `initialize`

Première requête obligatoire. `workspace` fixe la racine du layering de
config (deux couches, globale + workspace) — c'est le client qui la
connaît (dossier ouvert dans l'éditeur), pas le cwd du process serveur.
Absent -> fallback sur le cwd du process.

```json
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"workspace":"/home/dev/monprojet"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"serverVersion":"0.0.1-alpha.1","workspaceRoot":"/home/dev/monprojet","defaultAgent":"build"}}
```

`workspaceRoot`/`defaultAgent` sont **absents** (pas `null`) si aucun
marqueur `.vanyline/`/`.git` n'est trouvé, ou aucun agent par défaut
configuré — champs optionnels au sens JSON usuel (absence), pas
`null` explicite.

Version de protocole inconnue :
```json
→ {"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":99}}
← {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Unknown protocol version: got 99, expected 1","data":{"code":"VNL-RPC-003"}}}
```

### `shutdown`

Pas de params. Le process sort **après** avoir envoyé cette réponse.

```json
→ {"jsonrpc":"2.0","id":3,"method":"shutdown"}
← {"jsonrpc":"2.0","id":3,"result":null}
```

### `config/agents`

Liste les agents résolus (deux couches fusionnées). Objets `Agent` en
snake_case natif — voir l'avertissement camelCase/snake_case plus haut.

```json
→ {"jsonrpc":"2.0","id":4,"method":"config/agents"}
← {"jsonrpc":"2.0","id":4,"result":[
    {"name":"build","mode":"primary","model":"qwen-local","toolsets":["shell"],"skills":"auto","system_prompt":"Tu es un assistant de développement."}
  ]}
```

### `config/models`

```json
→ {"jsonrpc":"2.0","id":5,"method":"config/models"}
← {"jsonrpc":"2.0","id":5,"result":[
    {"name":"qwen-local","provider":"ollama-local","model":"qwen2.5","max_tokens":4096}
  ]}
```

### `config/providers`

Liste les providers résolus (deux couches fusionnées). Objets `Provider` en
snake_case natif — `type` est l'enum `"ollama" | "openai-compatible"`, et
`api_key` est **absent** (pas `null`) quand non configuré.

```json
→ {"jsonrpc":"2.0","id":8,"method":"config/providers"}
← {"jsonrpc":"2.0","id":8,"result":[
    {"name":"ollama-local","type":"ollama","endpoint":"http://localhost:11434"}
  ]}
```

### `config/mcpServers`

Liste les serveurs MCP résolus (deux couches fusionnées). Objets `McpServer`
en snake_case natif — `type` est l'enum `"http-streamable" | "sse"`, et
`headers` est toujours présent (`{}` si aucun header).

`sse` est **stockable** mais le transport est **non implémenté** : un `test`
sur un serveur `sse` (comme son usage par un toolset) échoue avec
`VNL-MCP-004`, replié en `VNL-RPC-006`.

```json
→ {"jsonrpc":"2.0","id":9,"method":"config/mcpServers"}
← {"jsonrpc":"2.0","id":9,"result":[
    {"name":"grafana","type":"http-streamable","url":"http://mcp:3000","headers":{"X-Token":"secret"}}
  ]}
```

### `config/toolsets`

```json
→ {"jsonrpc":"2.0","id":6,"method":"config/toolsets"}
← {"jsonrpc":"2.0","id":6,"result":[
    {"name":"shell","local_tools":["read_file","write_file"],"mcp":[]}
  ]}
```

### `config/skills`

Index léger (name + description) — pas le corps du skill.

```json
→ {"jsonrpc":"2.0","id":7,"method":"config/skills"}
← {"jsonrpc":"2.0","id":7,"result":[
    {"name":"pdf","description":"Traitement de fichiers PDF"}
  ]}
```

Erreur de lecture de config (fichier YAML invalide, etc.) sur n'importe
laquelle des 6 méthodes ci-dessus :
```json
← {"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"VNL-CFG-001: Configuration error: ...","data":{"code":"VNL-RPC-006"}}}
```
(Le message embarque le code `VNL-CFG-*`/`VNL-LLM-*` interne à
`vanyline_lib` — utile pour le débogage, le code de premier niveau côté
client reste `VNL-RPC-006`.)

### Écriture de configuration

18 méthodes — `config/<domain>/create`, `config/<domain>/update`,
`config/<domain>/delete` avec `domain ∈ {providers, models, mcpServers,
toolsets, agents, skills}`. L'enveloppe des params est en camelCase
(`layer`, `name`, `patch`, `item`, `body`), son contenu (`item`, `patch`)
en **snake_case natif** de domaine. Une enveloppe `params` non
désérialisable répond `VNL-RPC-000` (comme partout). Tout succès d'écriture
répond `result: null` — rien à relire dans la réponse : seule la méthode
`config/<domain>` de lecture correspondante renvoie l'entrée (fusion des
deux couches, pas la couche ciblée).

**Cible de couche (`layer?`)** — le param `layer` vaut `"global"` ou
`"workspace"` (minuscules ; toute autre valeur = enveloppe malformée,
`VNL-RPC-000`) :

- absent — `workspace` si `initialize` a résolu un workspace (marqueur
  `.vanyline/` ou `.git`), sinon `global` ;
- `"global"` — force la couche globale **même en workspace résolu** ;
- `"workspace"` explicite sans workspace résolu — `VNL-RPC-006` ;
- une écriture ne touche **que** la couche ciblée : le fichier de l'autre
  couche reste inchangé octet pour octet. La résolution 2-couches à la
  lecture n'est pas affectée ;
- conflits et absences (`VNL-RPC-012`, `VNL-RPC-013`) jugés **dans la
  couche ciblée uniquement** — un nom présent dans l'autre couche ne
  bloque pas un `create` dans la couche ciblée.

**Les 3 formes**, pour les 6 domaines :

- `config/<domain>/create` — params `{layer?, item}` ; `item` = l'entité
  snake_case complète (avec son `name`). Succès -> `result: null`.
- `config/<domain>/update` — params `{layer?, name, patch}` ; `patch` est
  un objet partiel : clé absente = inchangée, clé présente = **remplacée**,
  clé à `null` = efface un champ optionnel (ou vide une liste). Les clés
  inconnues sont ignorées. Succès -> `result: null`.
- `config/<domain>/delete` — params `{layer?, name}`. Succès ->
  `result: null`. **Non idempotent** (contrairement à
  `conversations/delete`) : un `name` absent de la couche ciblée répond
  `VNL-RPC-012`.

Un `item` non désérialisable dans le type de domaine (`type` hors enum de
provider/MCP, `mode` d'agent invalide, champ mal typé...) répond
`VNL-RPC-015`, avant toute atteinte du store.

Un patch dont le résultat rendrait l'entrée **non relisible** est refusé
en entier par `VNL-RPC-015`, **rien n'est écrit** : `null` sur un champ
requis (`type`/`endpoint` pour providers, `provider`/`model` pour models,
`type`/`url` pour MCP, `mode`/`model`/`system_prompt` pour agents,
`description`/`body` pour skills), **ou** une valeur mal typée sur
n'importe quel champ (`endpoint: 123`, `temperature: "hot"`,
`headers: "x"`…). L'entrée relue après un patch rejeté est l'originale
intacte. `null` sur un champ optionnel (ou une liste) l'efface/vide
normalement.

**Exception de forme pour skills** — `config/skills/create` prend
`{layer?, item, body}` : `item` = le `SkillMeta` (`{name, description}`),
`body` = le corps du `SKILL.md` (hors frontmatter), séparé de l'item.
`body` est **requis** : absent -> `VNL-RPC-000` (c'est l'enveloppe qui
échoue, pas l'`item`). `config/skills/update` patche les clés
`description` et/ou `body`. Le corps n'est jamais exposé en lecture —
`config/skills` ne renvoie que l'index léger.

Exemple complet (providers, écriture en couche globale puis conflit de nom) :

```json
→ {"jsonrpc":"2.0","id":10,"method":"config/providers/create","params":{"layer":"global","item":{"name":"ollama-local","type":"ollama","endpoint":"http://localhost:11434"}}}
← {"jsonrpc":"2.0","id":10,"result":null}
→ {"jsonrpc":"2.0","id":11,"method":"config/providers/create","params":{"layer":"global","item":{"name":"ollama-local","type":"ollama","endpoint":"http://localhost:11434"}}}
← {"jsonrpc":"2.0","id":11,"error":{"code":-32000,"message":"VNL-CFG-007: provider 'ollama-local' already exists in Global layer","data":{"code":"VNL-RPC-013"}}}
```

**Validation des noms (anti-traversal)** — contrat de sécurité. `name`
devient une clé de map et/ou un nom de fichier et de répertoire : il doit
matcher `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` et faire au plus 64 caractères.
`..`, `/`, `\`, chemins absolus sont rejetés par `VNL-RPC-014`, **sans
aucune écriture** (rien n'est créé sur disque, pas même un répertoire). La
validation vit dans la couche de store (`vanyline-cfgstore`), pas dans le
handler RPC : la contrainte est donc identique pour le CLI et pour toute
surface future (sandbox).

**Effet de bord assumé** — réécrire une entrée réécrit le `config.yaml` de
la couche concernée via `yaml_serde` : les **données** des autres entrées
et des autres maps sont préservées, pas le **formatting** — commentaires
et ordre d'origine perdus. Dette assumée, documentée ici.

**Fichiers annexes** — les `create` des domaines fichiers écrivent
respectivement `toolsets/<name>.yaml`, `agents/<name>.md` et
`skills/<name>/SKILL.md` (relatifs au répertoire de la couche) ; les
`update` réécrivent ce même fichier, les `delete` le suppriment (et le
répertoire du skill). Rien d'autre ne vit dans le répertoire d'un skill —
il n'existe aucune surface RPC pour éditer des fichiers annexes.

### Actions

**`config/localTools`** — sans params. Registre **statique** des 8 tools
intégrés du CLI, en descripteurs MCP `{name, description, inputSchema}`
passés verbatim (schéma MCP en camelCase — pas des entités de domaine).
Filesystem (5) : `read_file`, `write_file`, `edit_file`, `delete_file`,
`list_directory` ; search (2) : `find_files`, `search` ; command (1) :
`execute_command`. Un toolset peut référencer ces noms dans sa liste
`local_tools`.

```json
→ {"jsonrpc":"2.0","id":14,"method":"config/localTools"}
← {"jsonrpc":"2.0","id":14,"result":[
    {"name":"read_file","description":"Read a file as numbered lines. …","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"]}},
    {"name":"write_file","description":"…","inputSchema":{"…":"…"}}
  ]}
```
(tronqué à 2 entrées — 8 tools au total)

**`config/providers/test`** — params `{name}` (**requis**, sinon
`VNL-RPC-000`). Pas de `layer` : l'entrée est résolue par nom dans le store
**fusionné**, exactement comme la verrait une lecture. Sonde le provider :
`ollama` -> `GET {endpoint}/api/tags` -> les `models[].name` ;
`openai-compatible` -> `GET {endpoint}/v1/models` (+ header
`Authorization: Bearer` si `api_key` présente) -> les `data[].id`.
Timeout 10 s.

```json
→ {"jsonrpc":"2.0","id":12,"method":"config/providers/test","params":{"name":"ollama-local"}}
← {"jsonrpc":"2.0","id":12,"result":{"models":["llama3:latest","qwen2.5"]}}
```

Nom inconnu, cible injoignable ou réponse non-JSON -> `VNL-RPC-006` (le
message porte le détail interne `VNL-LLM-003`/`VNL-LLM-004`).

**`config/mcpServers/test`** — params `{name}` (même résolution que
ci-dessus). Se connecte au serveur MCP et liste les noms de ses outils.

```json
→ {"jsonrpc":"2.0","id":13,"method":"config/mcpServers/test","params":{"name":"grafana"}}
← {"jsonrpc":"2.0","id":13,"result":{"tools":["list_alerts","query_metrics"]}}
```

Échec de connexion — ou transport `sse`, non implémenté (`VNL-MCP-004`) —
-> `VNL-RPC-006`. Timeout 10 s (une cible qui accepte la connexion sans
répondre -> `VNL-RPC-006` au bout de 10 s, pas de blocage du serveur).

**Sécurité — note SSRF (assumée, non mitigée)** — les actions `test`
requêtent les URLs de provider/MCP **stockées dans la config** : la cible
est donc contrôlée par qui a écrit cette config. Le serveur RPC tourne
**en local, sous l'utilisateur**, seul maître de sa configuration — la
surface d'attaque est la sienne, cohérent avec « Sécurité workspace
assumée ». **Cette hypothèse ne vaut plus dès lors que la même surface de
config est exposée par un serveur multi-tenant (sandbox)** — à retraiter
dans la feature sandbox. Le crate de config lui-même ne fait aucune
requête réseau : seules les actions `test` en font.

### `conversations/list`

Vue allégée (`ConversationSummary`, camelCase — PAS les messages).

```json
→ {"jsonrpc":"2.0","id":8,"method":"conversations/list"}
← {"jsonrpc":"2.0","id":8,"result":[
    {"id":"3fa85f64-5717-4562-b3fc-2c963f66afa6","agent":"build","title":"Refactor auth","messageCount":4}
  ]}
```

### `conversations/create`

```json
→ {"jsonrpc":"2.0","id":9,"method":"conversations/create","params":{"agent":"build","title":"Nouvelle session"}}
← {"jsonrpc":"2.0","id":9,"result":{"id":"c1a2b3c4-...","agent":"build","title":"Nouvelle session","messageCount":0}}
```

`agent`/`title` optionnels — aucune validation que `agent` référence un
agent existant (même comportement que la commande CLI `conversations
new`).

### `conversations/get`

Retourne la `Conversation` **complète** de `vanyline_lib` (snake_case
natif), messages inclus.

```json
→ {"jsonrpc":"2.0","id":10,"method":"conversations/get","params":{"id":"c1a2b3c4-..."}}
← {"jsonrpc":"2.0","id":10,"result":{
    "id":"c1a2b3c4-...","agent":"build","title":"Nouvelle session",
    "messages":[
      {"role":"user","content":"Bonjour"},
      {"role":"assistant","content":"Salut !","tool_calls":[{"name":"read_file","arguments":{"path":"README.md"},"result":"..."}]}
    ]
  }}
```

Conversation introuvable :
```json
← {"jsonrpc":"2.0","id":10,"error":{"code":-32000,"message":"Conversation not found: ...","data":{"code":"VNL-RPC-005"}}}
```

### `conversations/delete`

Idempotent — un `id` inconnu réussit silencieusement (même comportement
que la commande CLI).

```json
→ {"jsonrpc":"2.0","id":11,"method":"conversations/delete","params":{"id":"c1a2b3c4-..."}}
← {"jsonrpc":"2.0","id":11,"result":null}
```

### `chat/send`

**La seule méthode dont la réponse arrive de façon ASYNCHRONE.** Le
serveur valide et réserve la conversation (voir "Concurrence"), spawn le
tour LLM, et retourne — la vraie réponse arrive **plus tard**, avec le
**même `id`**, potentiellement précédée d'une ou plusieurs notifications
`chat/event` sur la même conversation.

```json
→ {"jsonrpc":"2.0","id":12,"method":"chat/send","params":{"conversationId":"c1a2b3c4-...","message":"Explique-moi ce fichier","agent":"build"}}
```

… puis, en attendant la fin du tour, des notifications interfoliées :
```json
← {"jsonrpc":"2.0","method":"chat/event","params":{"conversationId":"c1a2b3c4-...","seq":0,"event":{"type":"token","content":"Ce "}}}
← {"jsonrpc":"2.0","method":"chat/event","params":{"conversationId":"c1a2b3c4-...","seq":1,"event":{"type":"token","content":"fichier "}}}
← {"jsonrpc":"2.0","method":"chat/event","params":{"conversationId":"c1a2b3c4-...","seq":2,"event":{"type":"done"}}}
```

… puis, avec le `id":12` d'origine, la réponse finale :
```json
← {"jsonrpc":"2.0","id":12,"result":{"text":"Ce fichier ...","toolCalls":[]}}
```

`agent` optionnel : priorité `params.agent` -> `conversation.agent` ->
agent par défaut du workspace. Si aucun n'est résolvable :
```json
← {"jsonrpc":"2.0","id":12,"error":{"code":-32000,"message":"No agent specified, ...","data":{"code":"VNL-RPC-008"}}}
```
(celle-ci, comme les erreurs `VNL-RPC-000`/`VNL-RPC-002`/`VNL-RPC-005`/
`VNL-RPC-007` de `chat/send`, arrive **immédiatement**, sans notification
`chat/event`, PAS de tour spawné.)

Conversation déjà occupée (un tour est déjà en cours dessus) :
```json
← {"jsonrpc":"2.0","id":13,"error":{"code":-32000,"message":"Conversation busy: a turn is already in progress","data":{"code":"VNL-RPC-002"}}}
```

Échec du tour (provider injoignable, agent/modèle mal configuré...) —
celle-là arrive de façon asynchrone, généralement précédée d'une
notification `chat/event` de type `error` :
```json
← {"jsonrpc":"2.0","method":"chat/event","params":{"conversationId":"c1a2b3c4-...","seq":0,"event":{"type":"error","code":"VNL-LLM-001","message":"..."}}}
← {"jsonrpc":"2.0","id":12,"error":{"code":-32000,"message":"VNL-LLM-001: LLM provider error: ...","data":{"code":"VNL-RPC-009"}}}
```

### `chat/cancel`

**No-op en v1** — l'annulation réelle dépend du support d'annulation de
`vanyline_lib` (prévu M3). Cette méthode existe déjà dans le protocole
pour ne pas casser les clients qui l'appellent : elle valide juste que
`conversationId` est un UUID bien formé, accepte, et ne fait RIEN d'autre
(n'exige même pas que la conversation existe ou qu'un tour soit en cours).

```json
→ {"jsonrpc":"2.0","id":14,"method":"chat/cancel","params":{"conversationId":"c1a2b3c4-..."}}
← {"jsonrpc":"2.0","id":14,"result":null}
```

## Notification `chat/event`

`event` est la sérialisation serde **directe** du `ChatEvent` de
`vanyline_lib` — le même schéma que le WebSocket de l'app web, un seul
format d'événement dans tout le projet. Tag `"type"`, snake_case. `seq`
est un compteur croissant **par conversation** (démarre à 0 à chaque
`chat/send`), pour détecter perte/désordre côté client.

Variantes de `event` :

| `type` | Champs |
|--------|--------|
| `token` | `content: string` |
| `tool_call` | `id, name: string, args: object` |
| `tool_result` | `id, name, result: string, is_error: boolean` |
| `skill_loaded` | `name: string` |
| `subagent_start` | `id, agent, task: string` |
| `subagent_event` | `id: string, event: ChatEvent` (récursif) |
| `subagent_end` | `id, result: string` |
| `usage` | `input_tokens, output_tokens: number` |
| `done` | — |
| `error` | `code, message: string` |

## Ressources K8s (owners/projects/sandboxes)

Les méthodes `owners/*` (tâche 04a), `projects/*` (4b), `sandboxes/*` (4c)
interagissent avec un cluster Kubernetes via un client `VnlK8sClient` construit
paresseusement au premier appel. Le namespace est résolu **une seule fois**,
après `initialize`, en lisant `defaults.namespace` du `config.yaml` fusionné
(des deux couches initialisées par `initialize`), puis en fallback sur le
namespace du contexte kubeconfig courant.

**Limitation v1 — namespace par session** : le namespace est résolu une seule
fois et appliqué à toutes les méthodes `owners/`/`projects/`/`sandboxes/` de la
session. Il n'y a **pas de param `namespace` par appel**. La configuration CLI/RPC
est considérée comme stable en cours de session : changer `config.yaml` entre deux
appels a un effet immédiat sur la config (`config/*`) mais le namespace K8s n'est
réévalué qu'après un nouvel `initialize` (qui remet le client K8s à `None`). Ceci
est cohérent avec le modèle du serveur stdio long-vivant, mais diffère du CLI où
chaque invocation peut recevoir un `--namespace`.

### owners/list

Retourne la liste des `Owner` du namespace. L'objet retourné est la
**sérialisation directe du CRD K8s** en camelCase (le même format qu'utilise
`kubectl get owners -o json`).

```json
→ {"jsonrpc":"2.0","id":100,"method":"owners/list"}
← {"jsonrpc":"2.0","id":100,"result":[
    {
      "apiVersion":"vanyline.solidite.fr/v1alpha1",
      "kind":"Owner",
      "metadata":{
        "name":"alice",
        "namespace":"dev"
      },
      "spec":{"existingPvc":null,"homeSize":"1Gi"},
      "status":{"pvcName":"owner-alice-home","serviceAccount":"alice"}
    }
  ]}
```

### owners/get

Retourne un `Owner` par nom.

```json
→ {"jsonrpc":"2.0","id":101,"method":"owners/get","params":{"name":"alice"}}
← {"jsonrpc":"2.0","id":101,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Owner",
    "metadata":{"name":"alice","namespace":"dev"},
    "spec":{"existingPvc":null,"homeSize":"1Gi"},
    "status":{"pvcName":"owner-alice-home","serviceAccount":"alice"}
  }}
```

`name` requis. Si params malformé (pas de `name`) :
```json
← {"jsonrpc":"2.0","id":101,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

### owners/create

Crée un `Owner` dans le namespace. `name` + champs de `OwnerSpec` aplati en
camelCase (pas d'objet `spec` imbriqué).

```json
→ {"jsonrpc":"2.0","id":102,"method":"owners/create","params":{"name":"alice","homeSize":"2Gi"}}
← {"jsonrpc":"2.0","id":102,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Owner",
    "metadata":{"name":"alice","namespace":"dev","uid":"..."},
    "spec":{"existingPvc":null,"homeSize":"2Gi"},
    "status":{"pvcName":"owner-alice-home","serviceAccount":"alice"}
  }}
```

`name` requis. `homeSize`, `existingPvc`, `projectDefaults` optionnels (valeurs
par défaut appliquées par le controller).

### owners/delete

Supprime un `Owner` par nom. Succès -> `result: null`. **Pas idempotent** —
contrairement à `conversations/delete` : un nom inexistant remonte l'erreur
404 de l'API K8s telle quelle (`VNL-RPC-010`), même comportement que la
commande CLI `vanyline owner delete`.

```json
→ {"jsonrpc":"2.0","id":103,"method":"owners/delete","params":{"name":"alice"}}
← {"jsonrpc":"2.0","id":103,"result":null}
```

Nom malformé :
```json
← {"jsonrpc":"2.0","id":103,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

Échec du client K8s ou de l'appel API :
```json
← {"jsonrpc":"2.0","id":103,"error":{"code":-32000,"message":"...","data":{"code":"VNL-RPC-010"}}}
```

### projects/list

Retourne la liste des `Project` du namespace. L'objet retourné est la
**sérialisation directe du CRD K8s** en camelCase (le même format qu'utilise
`kubectl get projects -o json`).

```json
→ {"jsonrpc":"2.0","id":104,"method":"projects/list"}
← {"jsonrpc":"2.0","id":104,"result":[
    {
      "apiVersion":"vanyline.solidite.fr/v1alpha1",
      "kind":"Project",
      "metadata":{
        "name":"demo-project",
        "namespace":"dev"
      },
      "spec":{
        "owner":"alice",
        "repoUrl":"https://github.com/alice/demo.git",
        "defaultBranch":"main",
        "existingPvc":null,
        "storageSize":"10Gi",
        "storageClass":"standard",
        "gitSecret":null,
        "caches":["cargo","pnpm"],
        "fetchInterval":"1h"
      },
      "status":{"pvcName":"project-demo-project","cloned":true,"lastFetch":null,"worktrees":[] }
    }
  ]}
```

### projects/get

Retourne un `Project` par nom.

```json
→ {"jsonrpc":"2.0","id":105,"method":"projects/get","params":{"name":"demo-project"}}
← {"jsonrpc":"2.0","id":105,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Project",
    "metadata":{"name":"demo-project","namespace":"dev"},
    "spec":{"owner":"alice","repoUrl":"https://github.com/alice/demo.git","defaultBranch":"main","existingPvc":null,"storageSize":"10Gi","storageClass":"standard","gitSecret":null,"caches":["cargo","pnpm"],"fetchInterval":"1h"},
    "status":{"pvcName":"project-demo-project","cloned":true,"lastFetch":null,"worktrees":[]}
  }}
```

`name` requis. Si params malformé (pas de `name`) :
```json
← {"jsonrpc":"2.0","id":105,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

### projects/create

Crée un `Project` dans le namespace. `name` + champs de `ProjectSpec` aplati
en camelCase (pas d'objet `spec` imbriqué).

```json
→ {"jsonrpc":"2.0","id":106,"method":"projects/create","params":{"name":"demo-project","owner":"alice","repoUrl":"https://github.com/alice/demo.git","defaultBranch":"main"}}
← {"jsonrpc":"2.0","id":106,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Project",
    "metadata":{"name":"demo-project","namespace":"dev","uid":"..."},
    "spec":{"owner":"alice","repoUrl":"https://github.com/alice/demo.git","defaultBranch":"main","existingPvc":null,"storageSize":null,"storageClass":null,"gitSecret":null,"caches":null,"fetchInterval":null},
    "status":{"pvcName":"project-demo-project","cloned":false,"lastFetch":null,"worktrees":[]}
  }}
```

`name` requis. `owner` et `repoUrl` requis, les autres champs sont optionnels
(valeurs par défaut appliquées par le controller).

### projects/delete

Supprime un `Project` par nom. Succès -> `result: null`. **Pas idempotent** —
contrairement à `conversations/delete` : un nom inexistant remonte l'erreur
404 de l'API K8s telle quelle (`VNL-RPC-010`), même comportement que la
commande CLI `vanyline project delete`.

```json
→ {"jsonrpc":"2.0","id":107,"method":"projects/delete","params":{"name":"demo-project"}}
← {"jsonrpc":"2.0","id":107,"result":null}
```

Nom malformé :
```json
← {"jsonrpc":"2.0","id":107,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

Échec du client K8s ou de l'appel API :
```json
← {"jsonrpc":"2.0","id":107,"error":{"code":-32000,"message":"...","data":{"code":"VNL-RPC-010"}}}
```

### sandboxes/list

Retourne la liste des `Sandbox` du namespace. L'objet retourné est la
**sérialisation directe du CRD K8s** en camelCase (le même format qu'utilise
`kubectl get sandboxes -o json`).

```json
→ {"jsonrpc":"2.0","id":108,"method":"sandboxes/list"}
← {"jsonrpc":"2.0","id":108,"result":[
    {
      "apiVersion":"vanyline.solidite.fr/v1alpha1",
      "kind":"Sandbox",
      "metadata":{
        "name":"demo-sandbox",
        "namespace":"dev"
      },
      "spec":{
        "project":"demo-project",
        "branch":"main",
        "toolchains":[
          {"ociImage":"rust:slim-trixie","env":{"PATH":"…/usr/local/bin","RUSTUP_HOME":"…/.rustup"}}
        ],
        "image":null,
        "resources":null
      },
      "status":{"podName":"sandbox-demo-sandbox"}
    }
  ]}
```

### sandboxes/get

Retourne un `Sandbox` par nom.

```json
→ {"jsonrpc":"2.0","id":109,"method":"sandboxes/get","params":{"name":"demo-sandbox"}}
← {"jsonrpc":"2.0","id":109,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Sandbox",
    "metadata":{"name":"demo-sandbox","namespace":"dev"},
    "spec":{"project":"demo-project","branch":"main","toolchains":[],"image":null,"resources":null},
    "status":{"podName":"sandbox-demo-sandbox"}
  }}
```

`name` requis. Si params malformé (pas de `name`) :
```json
← {"jsonrpc":"2.0","id":109,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

### sandboxes/create

Crée un `Sandbox` dans le namespace. `name` + champs de `SandboxSpec` aplati
en camelCase (pas d'objet `spec` imbriqué). `toolchains` et `image` sont
optionnels (valeurs par défaut appliquées par le controller).

```json
→ {"jsonrpc":"2.0","id":110,"method":"sandboxes/create","params":{"name":"demo-sandbox","project":"demo-project","branch":"main"}}
← {"jsonrpc":"2.0","id":110,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Sandbox",
    "metadata":{"name":"demo-sandbox","namespace":"dev","uid":"..."},
    "spec":{"project":"demo-project","branch":"main","toolchains":[],"image":null,"resources":null},
    "status":{"podName":"sandbox-demo-sandbox"}
  }}
```

`name` requis. `project` et `branch` requis, les autres champs sont optionnels
(valeurs par défaut appliquées par le controller).

### sandboxes/delete

Supprime un `Sandbox` par nom. Succès -> `result: null`. **Pas idempotent** —
contrairement à `conversations/delete` : un nom inexistant remonte l'erreur
404 de l'API K8s telle quelle (`VNL-RPC-010`), même comportement que la
commande CLI `vanyline sandbox delete`.

```json
→ {"jsonrpc":"2.0","id":111,"method":"sandboxes/delete","params":{"name":"demo-sandbox"}}
← {"jsonrpc":"2.0","id":111,"result":null}
```

Nom malformé :
```json
← {"jsonrpc":"2.0","id":111,"error":{"code":-32700,"message":"Malformed request: ...","data":{"code":"VNL-RPC-000"}}}
```

Échec du client K8s ou de l'appel API :
```json
← {"jsonrpc":"2.0","id":111,"error":{"code":-32000,"message":"...","data":{"code":"VNL-RPC-010"}}}
```

### `sandboxes/stop` / `sandboxes/start`

Patch `spec.suspended` (merge patch JSON, pas de remplacement complet de la
spec) — `true` pour `stop`, `false` pour `start`. Le controller interprète ce
champ pour suspendre/redémarrer le pod sans supprimer la ressource (voir
`docs/architecture.md`, section "Opérateur Kubernetes"). Retourne le `Sandbox`
patché (même format que `sandboxes/get`), pas `null` — contrairement à
`sandboxes/delete`, la transition d'état est ce que l'appelant veut voir.

```json
→ {"jsonrpc":"2.0","id":112,"method":"sandboxes/stop","params":{"name":"demo-sandbox"}}
← {"jsonrpc":"2.0","id":112,"result":{
    "apiVersion":"vanyline.solidite.fr/v1alpha1",
    "kind":"Sandbox",
    "metadata":{"name":"demo-sandbox","namespace":"dev"},
    "spec":{"project":"demo-project","branch":"main","toolchains":[],"image":null,"resources":null,"egress":[],"suspended":true},
    "status":{"podName":"sandbox-demo-sandbox"}
  }}
```

```json
→ {"jsonrpc":"2.0","id":113,"method":"sandboxes/start","params":{"name":"demo-sandbox"}}
← {"jsonrpc":"2.0","id":113,"result":{"...":"...","spec":{"...":"...","suspended":false},"...":"..."}}
```

`name` requis. Nom malformé ou échec K8s : mêmes codes que `sandboxes/get`
(`VNL-RPC-000` / `VNL-RPC-010`).

## Concurrence

Un seul tour actif **par conversation** — un `chat/send` sur une
conversation déjà occupée répond `VNL-RPC-002` immédiatement, sans rien
spawner. **Plusieurs conversations différentes tournent en vrai
parallèle** : chaque `chat/send` valide est exécuté dans sa propre tâche
tokio, la boucle de lecture stdin continue à traiter d'autres requêtes
(y compris d'autres `chat/send` sur d'autres conversations) sans attendre
la fin du tour en cours. Les notifications `chat/event` de plusieurs
conversations peuvent donc être interfoliées sur le flux stdout —
démultiplexez-les par `conversationId`.

## Rechargement de config

Pas de watcher : la config (`config/*`, agents/modèles/toolsets/skills
utilisés par `chat/send`) est relue depuis le disque à **chaque appel**
(le store CLI n'a pas de cache). Éditer `.vanyline/` pendant que le
serveur tourne prend effet à la requête suivante, sans redémarrer le
serveur ni rappeler `initialize`.
