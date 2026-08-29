# Feature — F2-vscode-ext-cli-rpc

Deuxième des cinq features « extension VS Code `vanyline` ». Séquence et état :
`.claude/memory/vscode-ext-sequence.md`. Dépend de F1 uniquement pour l'alignement des
noms de domaines (`profiles` ↔ `models`) — pas de dépendance de code.

## Ce que la feature fait

Ajoute à `vanyline serve --stdio` le CRUD complet des cinq domaines de configuration
(providers, profils de modèle, serveurs MCP, toolsets, agents) plus les skills, pour que
l'extension édite la config locale deux-couches (YAML) **sans passer par l'app**.

## Ce qu'elle ne fait pas

- Aucune UI (F4).
- Pas de watcher de config — `FsConfigStore` relit le disque à chaque appel (dette
  assumée conservée, cf. `docs/architecture.md` section RPC).
- Pas de validation croisée à l'écriture au-delà de ce que `config check` fait en
  lecture (best-effort, pas de fail-fast, références pendantes autorisées).
- Pas d'édition des fichiers annexes d'un skill : `skills/<name>/` = création d'un
  `SKILL.md` (frontmatter `name`+`description` + corps), rien d'autre dans le répertoire.
- Pas de `config.yaml` brut exposé en édition texte.

## Contexte — la couche d'écriture est nette-neuve

Les sous-commandes CLI actuelles (`model`, `mcp`, `toolset`, `agent`…) sont **`List`
uniquement** (`cli/src/*_cmd.rs`, 7 lignes chacune). Le trait `vanyline_lib::ConfigStore`
est **lecture seule** (`list_*` / `get_*` / `load_skill`). Le seul chemin d'écriture
existant est `cli/src/config.rs::set_default_agent`. F2 construit donc :

1. la sérialisation retour de chaque entité vers son format
   (`config.yaml` maps, `agents/<name>.md`, `toolsets/<name>.yaml`,
   `skills/<name>/SKILL.md`) en **préservant la séparation des couches** (une écriture
   workspace ne touche pas le fichier global et réciproquement, pas de deep-merge
   détruit) ;
2. les méthodes RPC qui l'exposent.

## Interfaces clés

### Méthodes RPC (`cli/src/rpc/handlers.rs`, dispatch `handle_request`)

Lecture — compléter l'existant (`config/agents|models|toolsets|skills` déjà là) :
- `config/providers` — **manque aujourd'hui**
- `config/mcpServers` — **manque aujourd'hui**

Écriture — pour `domain ∈ {providers, models, mcpServers, toolsets, agents, skills}` :
- `config/<domain>/create` — params `{ layer?: "global" | "workspace", item: <entité snake_case> }`
- `config/<domain>/update` — params `{ layer?, name, patch }`
- `config/<domain>/delete` — params `{ layer?, name }`

Actions :
- `config/providers/test` — interroge le provider, renvoie `{ models: [...] }` (même
  logique que `POST /api/v1/llm-providers/{id}/test` côté app, réutilise le client de
  `lib/src/prefixed_mcp.rs` / le client provider).
- `config/mcpServers/test` — `{ tools: [...] }`.
- `config/localTools` — registre statique des 8 tools intégrés
  (`vanyline_tools::mcp::{filesystem,search,command}_tools`), lecture seule.

Passthrough camelCase/snake_case : les enveloppes (`layer`, `name`) en camelCase ; les
`item`/`patch` sont les types `vanyline_lib` **tels quels**, snake_case natif — cohérent
avec la règle déjà documentée pour `config/*` en lecture.

### Cible de couche

Param `layer` optionnel. Défaut : **workspace si un workspace est résolu à
`initialize`, sinon global** — aligné sur la sémantique du CLI. `layer: "global"`
force la couche globale même en workspace.

### Codes d'erreur (`cli/src/rpc/protocol.rs::vnl_code`)

- `VNL-RPC-011` `CONFIG_WRITE_ERROR` — échec d'écriture disque / sérialisation
- `VNL-RPC-012` `CONFIG_NOT_FOUND` — `update`/`delete` sur un `name` absent dans la couche ciblée
- `VNL-RPC-013` `CONFIG_NAME_CONFLICT` — `create` sur un `name` déjà présent dans la couche ciblée
- `VNL-RPC-014` `CONFIG_INVALID_NAME` — `name` qui ne respecte pas la contrainte ci-dessous
- `VNL-RPC-015` `CONFIG_VALIDATION` — type énuméré invalide (`provider_type` / `server_type` / `mode`), miroir des anciens `CHECK` restaurés côté app via `before_create`

## Sécurité (argv / URL / chemin)

- **`name` fourni par le client devient un nom de fichier** (`agents/<name>.md`,
  `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`). Contrainte, à valider **avant toute
  opération disque** : `name` doit matcher `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`, longueur
  bornée, et **rejeter explicitement** `..`, `/`, `\`, un `.` ou `..` seul, tout chemin
  absolu. Sans ça, `name = "../../.ssh/authorized_keys"` écrit hors de la config →
  traversal. C'est exactement le trou trouvé sur `git-integration` (2026-08-22, cf.
  `.claude/config.md`). Erreur `VNL-RPC-014`.
- **URLs provider / MCP** stockées telles quelles puis requêtées par `config/*/test`
  (requête HTTP sortante vers une cible contrôlée par le client → SSRF théorique). Le
  serveur RPC tourne **en local sous l'utilisateur** : la surface d'attaque est celle de
  l'utilisateur lui-même. Acceptable, cohérent avec « Sécurité workspace assumée » de
  `docs/architecture.md`. À documenter dans `rpc-protocol.md`, pas à mitiger.
- **Aucune interpolation shell** — aucune commande construite, uniquement des écritures
  de fichiers via `std::fs` et de la sérialisation `yaml_serde`.

## Modules touchés

| Module | Changement |
|---|---|
| `cli/src/config.rs` / `cli/src/fs_store.rs` | méthodes d'écriture par entité + validation `name` + gestion de couche |
| `cli/src/rpc/protocol.rs` | params create/update/delete, codes `VNL-RPC-011..015` |
| `cli/src/rpc/handlers.rs` | dispatch + handlers des nouvelles méthodes |
| `cli/tests/rpc_stdio_smoke.rs` | round-trips par domaine, conflits, traversal rejeté, cible de couche |
| `docs/rpc-protocol.md` | section « Écriture de configuration » + note SSRF |
| `docs/architecture.md` | maj section RPC (méthodes d'écriture) — à la clôture Phase 3 |

## Tests

`cli/tests/rpc_stdio_smoke.rs`, par domaine :
- `create` → `list` contient l'entrée → `get`/lecture renvoie les bons champs → `update`
  (patch partiel) → `delete` → `list` ne la contient plus.
- `create` sur nom existant → `VNL-RPC-013`. `update`/`delete` sur nom absent → `VNL-RPC-012`.
- `name` = `"../evil"`, `"a/b"`, `".."`, `"/abs"` → `VNL-RPC-014`, **aucun fichier créé
  hors du répertoire de config** (assert sur le FS).
- `provider_type` inconnu → `VNL-RPC-015`.
- `layer: "workspace"` écrit dans `<root>/.vanyline/`, `layer: "global"` dans
  `~/.config/vanyline/` (tmpdir de test), l'un n'altère pas l'autre.
- `config/localTools` → 8 entrées attendues.

## Risques et questions ouvertes

- **Couche cible** : param `layer` explicite (retenu) vs heuristique seule. Retenu
  parce que l'extension F4 voudra offrir le choix « ce workspace / global ».
- **Nom de domaine `models` vs `profiles`** : la CLI garde `config/models/*` (la map
  `models:` du `config.yaml`) ; `@vanyline/ui` dit `profiles` ; le pont RPC de F4
  traduit. À figer dans `rpc-protocol.md` pour ne pas piéger.
- **`providers`/`mcp` locaux et non restreints** côté CLI alors qu'ils sont
  globaux+AdminOnly côté app : cohérent (l'extension est un harness local), pas un bug.
- **Réécriture de `config.yaml`** : préserver l'ordre / les commentaires est hors
  portée (`yaml_serde` ne les garde pas) — accepté, documenté. Le round-trip preserve
  les *données*, pas le formatage.

## Découpage en tâches candidates

1. Couche d'écriture `FsConfigStore` + validation `name` anti-traversal + codes d'erreur + tests unitaires côté `config.rs`.
2. `config/{providers,models,mcpServers}/*` (maps `config.yaml`) + smoke tests.
3. `config/{toolsets,agents,skills}/*` (fichiers / répertoires) + smoke tests.
4. Actions `test` providers/mcp + `config/localTools` + section `docs/rpc-protocol.md`.
