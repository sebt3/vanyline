# Feature — F2-vscode-ext-cli-rpc

Deuxième des cinq features « extension VS Code `vanyline` ». Séquence et état :
`.claude/memory/vscode-ext-sequence.md`. Dépend de F1 uniquement pour l'alignement des
noms de domaines (`profiles` ↔ `models`) — pas de dépendance de code.

## Ce que la feature fait

Ajoute à `vanyline serve --stdio` le CRUD complet des cinq domaines de configuration
(providers, profils de modèle, serveurs MCP, toolsets, agents) plus les skills, pour que
l'extension édite la config locale deux-couches (YAML) **sans passer par l'app**.

**La couche d'écriture est construite dans un crate partagé nouveau, `vanyline-cfgstore`**,
pas dans `cli/`. Raison : la sandbox (`sandbox/`) voudra plus tard manipuler ces mêmes
fichiers de config dans le workspace qu'elle gère (agents/toolsets/skills/MCP scopés au
projet, éditables depuis l'IDE web et par le LLM). La sandbox **ne dépend pas de
`vanyline-lib`** (serveur de pod volontairement léger — `vanyline-lib` traîne
`rig-core`/`rmcp`/`reqwest`, tout le harness LLM). Il faut donc un crate feuille que
`cli` **et** `sandbox` peuvent consommer. Décider le placement maintenant coûte une task
d'extraction ; le décider après que F4 (pont RPC `ConfigRepo`) et la feature sandbox
aient construit dessus coûte beaucoup plus.

Le câblage RPC côté sandbox (endpoints WS/MCP exposant ce CRUD) **n'est pas** dans F2 —
seul le crate est rendu partageable. F2 ne touche que `vanyline serve --stdio`.

## Ce qu'elle ne fait pas

- Aucune UI (F4).
- Aucun endpoint côté `sandbox/` — le crate est extrait et rendu consommable, son
  branchement dans le serveur sandbox est une feature ultérieure.
- Pas de watcher de config — `FsConfigStore` relit le disque à chaque appel (dette
  assumée conservée, cf. `docs/architecture.md` section RPC).
- Pas de validation croisée à l'écriture au-delà de ce que `config check` fait en
  lecture (best-effort, pas de fail-fast, références pendantes autorisées).
- Pas d'édition des fichiers annexes d'un skill : `skills/<name>/` = création d'un
  `SKILL.md` (frontmatter `name`+`description` + corps), rien d'autre dans le répertoire.
- Pas de `config.yaml` brut exposé en édition texte.
- Pas de trait `ConfigWrite` séparé — les méthodes d'écriture sont **sur `ConfigStore`**
  (cf. « Interfaces clés »).

## Contexte — extraction `vanyline-cfgstore` + couche d'écriture nette-neuve

État actuel, la couche config est à cheval sur deux crates et lecture seule :

| Morceau | Où aujourd'hui |
|---|---|
| types domaine (`Provider`, `Agent`, `Toolset`, `McpTransport`…) | `lib/src/domain.rs` |
| trait `ConfigStore` (lecture) + `InMemoryConfigStore` + `resolve_by_name` | `lib/src/store.rs` |
| `Layers`, `RawConfigFile`, merge/résolution 2 couches, `*_entry_source` | `cli/src/config.rs` |
| `FsConfigStore` (impl `ConfigStore` sur les 2 couches YAML) | `cli/src/fs_store.rs` |
| seul chemin d'écriture existant | `cli/src/config.rs::set_default_agent` |

Les sous-commandes CLI (`model`, `mcp`, `toolset`, `agent`…) sont **`List` uniquement**
(`cli/src/*_cmd.rs`, 7 lignes chacune). Le trait `ConfigStore` est **lecture seule**
(`list_*` / `get_*` / `load_skill`).

F2 fait donc, dans l'ordre :

1. **Extraire `vanyline-cfgstore`** (crate feuille, déplacement pur, zéro changement de
   comportement) : y déplacer `domain` + `store` (depuis `lib/`) et `Layers` +
   machinerie + `FsConfigStore` (depuis `cli/`). `vanyline-lib` re-exporte `domain` et
   `store` (aucun churn pour les imports `vanyline_lib::domain::…` / `::store::…`
   existants dans app/cli/`packages/protocol`). Erreur : nouveau `CfgStoreError` local
   au crate (le trait ne peut plus renvoyer `VnyError` — crate feuille), avec
   `impl From<CfgStoreError> for VnyError` côté lib et `for AppError` côté app.
2. **Ajouter les méthodes d'écriture sur `ConfigStore`** : sérialisation retour de
   chaque entité vers son format (`config.yaml` maps, `agents/<name>.md`,
   `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`) en **préservant la séparation des
   couches** (une écriture workspace ne touche pas le fichier global et réciproquement,
   pas de deep-merge détruit), + validation `name` anti-traversal. Impl réelle dans
   `FsConfigStore` et `InMemoryConfigStore` ; défaut `Err(CfgStoreError::ReadOnly)` pour
   les autres backends.
3. **Les méthodes RPC** qui l'exposent dans `vanyline serve --stdio`.

Le crate feuille `vanyline-cfgstore` dépend **uniquement** de `serde`, `serde_json`,
`yaml_serde`, `async-trait`, `thiserror`. Restent **hors** du crate (câblage hôte, restent
dans `cli/src/config.rs`) : `config_dir()` / `data_dir()` (crate `dirs`),
`discover_workspace_root()` (remontée depuis le cwd), `configured_namespace` /
`configured_toolbox` (concepts K8s/CLI). `cli` garde une fonction
`discover_layers(start) -> Layers` qui assemble ces morceaux hôte autour du `Layers` de
cfgstore. L'action `config/*/test` (HTTP sortant, cf. plus bas) reste dans le handler RPC
(besoin d'un client HTTP — hors périmètre d'un crate fs pur).

## Interfaces clés

### Crate `vanyline-cfgstore`

```
cfgstore/
├── Cargo.toml          # leaf : serde, serde_json, yaml_serde, async-trait, thiserror
└── src/
    ├── lib.rs
    ├── domain.rs       # déplacé de lib/src/domain.rs — inchangé
    ├── error.rs        # CfgStoreError (codes VNL-CFG-*)
    ├── layers.rs       # Layers, RawConfigFile, merge/résolution, *_entry_source
    │                   #   — déplacé de cli/src/config.rs
    └── store.rs        # trait ConfigStore (lecture + écriture) + InMemoryConfigStore
    │                   #   + resolve_by_name — déplacé de lib/src/store.rs
    └── fs_store.rs     # FsConfigStore : impl ConfigStore (lecture déplacée de cli/ +
                        #   écriture nouvelle + validation name)
```

Racine `Cargo.toml` : `members += "cfgstore"`. `lib/Cargo.toml` et `cli/Cargo.toml` :
`vanyline-cfgstore = { path = "../cfgstore" }`. `sandbox/` **n'est pas** modifié par F2
(consommera le crate plus tard).

### Trait `ConfigStore` (cfgstore, `store.rs`)

Lecture — inchangée (`list_*`, `get_*` défaut, `load_skill`, `default_agent`).

Écriture — **ajoutées au trait**, pour `domain ∈ {providers, models, mcp_servers,
toolsets, agents, skills}` :

```rust
/// Couche ciblée par une écriture. La résolution "workspace si dispo sinon
/// global" est faite par l'appelant (handler RPC) — le trait prend un Layer
/// explicite. InMemoryConfigStore ignore ce paramètre (jeu unique en mémoire).
pub enum Layer { Global, Workspace }

async fn create_provider(&self, layer: Layer, item: Provider)          -> Result<(), CfgStoreError>;
async fn update_provider(&self, layer: Layer, name: &str, patch: serde_json::Value) -> Result<(), CfgStoreError>;
async fn delete_provider(&self, layer: Layer, name: &str)              -> Result<(), CfgStoreError>;
// … idem model / mcp_server / toolset / agent
async fn create_skill(&self, layer: Layer, meta: SkillMeta, body: String) -> Result<(), CfgStoreError>;
async fn update_skill(&self, layer: Layer, name: &str, patch: serde_json::Value) -> Result<(), CfgStoreError>;
async fn delete_skill(&self, layer: Layer, name: &str)                    -> Result<(), CfgStoreError>;
async fn set_default_agent(&self, layer: Layer, name: &str)               -> Result<(), CfgStoreError>;
```

- **`patch` = `serde_json::Value`** (objet partiel) au niveau du trait — le handler RPC
  passe du JSON de toute façon ; l'impl applique un merge champ à champ (clé absente =
  inchangée, clé présente = remplacée, y compris à `null` → efface un champ optionnel).
- **Défaut de trait** : toutes les méthodes d'écriture renvoient
  `Err(CfgStoreError::ReadOnly)`. Overridées pour de vrai par `FsConfigStore` et
  `InMemoryConfigStore` uniquement. `PgConfigStore` (app) et les doubles de test héritent
  du défaut — aucune modification de leur code.
- `set_default_agent` : plie le `cli/src/config.rs::set_default_agent` existant dans le
  trait (aujourd'hui il n'écrit que la couche globale — le `Layer` explicite lève cette
  limite).

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
  `lib/src/prefixed_mcp.rs` / le client provider). **Reste dans le handler RPC**, pas
  dans cfgstore.
- `config/mcpServers/test` — `{ tools: [...] }`.
- `config/localTools` — registre statique des 8 tools intégrés
  (`vanyline_tools::mcp::{filesystem,search,command}_tools`), lecture seule.

Passthrough camelCase/snake_case : les enveloppes (`layer`, `name`) en camelCase ; les
`item`/`patch` sont les types `vanyline_cfgstore::domain` **tels quels**, snake_case
natif — cohérent avec la règle déjà documentée pour `config/*` en lecture.

### Cible de couche

Param `layer` optionnel côté RPC. Défaut : **workspace si un workspace est résolu à
`initialize`, sinon global** — aligné sur la sémantique du CLI. `layer: "global"` force
la couche globale même en workspace. Le handler RPC traduit ce `layer?` optionnel en
`Layer` explicite avant d'appeler le trait.

### `CfgStoreError` (cfgstore, `error.rs`)

Enum `thiserror`, identifiants stables `VNL-CFG-*`. Reprend les 3 variantes de
`VnyError` que `resolve_by_name` produisait (mêmes codes, sémantique inchangée) et ajoute
le write-side :

| Variante | Code | Sens |
|---|---|---|
| `Config(String)` | `VNL-CFG-001` | parse / forme YAML invalide (remplace `VnyError::ConfigError` côté store) |
| `DuplicateName(&'static str, String)` | `VNL-CFG-002` | >1 entrée du même nom en lecture |
| `UnknownReference(&'static str, String)` | `VNL-CFG-003` | `get_*` / `load_skill` sur nom absent |
| `Io(#[from] std::io::Error)` | `VNL-CFG-004` | I/O disque |
| `InvalidName(String)` | `VNL-CFG-005` | `name` viole la contrainte anti-traversal |
| `NotFound { kind, name, layer }` | `VNL-CFG-006` | `update` / `delete` sur nom absent **dans la couche ciblée** |
| `NameConflict { kind, name, layer }` | `VNL-CFG-007` | `create` sur nom déjà présent **dans la couche ciblée** |
| `WriteError(String)` | `VNL-CFG-008` | échec sérialisation / écriture disque |
| `ReadOnly` | `VNL-CFG-009` | backend sans écriture (défaut de trait) |
| `Validation(String)` | `VNL-CFG-010` | type énuméré invalide (`provider_type` / `transport` / `mode`) |

- `vanyline-lib` : `impl From<CfgStoreError> for VnyError` — 001/002/003 mappent sur les
  variantes `VnyError` de même code (conservées) ; le reste → `VnyError::ConfigError`
  avec le message complet.
- `vanyline-app` : `impl From<CfgStoreError> for AppError` (à côté du
  `From<vanyline_lib::VnyError>` existant, `app/src/error.rs`).

### Codes d'erreur RPC (`cli/src/rpc/protocol.rs::vnl_code`)

Le handler traduit `CfgStoreError` en code protocole RPC :

- `VNL-RPC-011` `CONFIG_WRITE_ERROR` ← `WriteError` / `Io` — échec d'écriture disque / sérialisation
- `VNL-RPC-012` `CONFIG_NOT_FOUND` ← `NotFound` — `update`/`delete` sur un `name` absent dans la couche ciblée
- `VNL-RPC-013` `CONFIG_NAME_CONFLICT` ← `NameConflict` — `create` sur un `name` déjà présent dans la couche ciblée
- `VNL-RPC-014` `CONFIG_INVALID_NAME` ← `InvalidName` — `name` qui ne respecte pas la contrainte ci-dessous
- `VNL-RPC-015` `CONFIG_VALIDATION` ← `Validation` — type énuméré invalide, miroir des anciens `CHECK` restaurés côté app via `before_create`

## Sécurité (argv / URL / chemin)

- **`name` fourni par le client devient un nom de fichier** (`agents/<name>.md`,
  `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`) **et une clé de map dans
  `config.yaml`**. Contrainte, à valider **dans `vanyline-cfgstore` avant toute
  opération disque** (une seule implémentation, partagée cli + future sandbox) : `name`
  doit matcher `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`, longueur bornée (≤ 64), et **rejeter
  explicitement** `..`, `/`, `\`, un `.` ou `..` seul, tout chemin absolu. Sans ça,
  `name = "../../.ssh/authorized_keys"` écrit hors de la config → traversal. C'est
  exactement le trou trouvé sur `git-integration` (2026-08-22, cf. `.claude/config.md`).
  Erreur `CfgStoreError::InvalidName` → `VNL-RPC-014`. **La validation est faite dans le
  crate, pas dans le handler RPC** — c'est le point où la future surface sandbox en
  bénéficie sans la réécrire.
- **URLs provider / MCP** stockées telles quelles puis requêtées par `config/*/test`
  (requête HTTP sortante vers une cible contrôlée par le client → SSRF théorique). Le
  serveur RPC tourne **en local sous l'utilisateur** : la surface d'attaque est celle de
  l'utilisateur lui-même. Acceptable, cohérent avec « Sécurité workspace assumée » de
  `docs/architecture.md`. À documenter dans `rpc-protocol.md`, pas à mitiger. **Note :
  cette hypothèse ne tiendra plus quand la sandbox exposera le même crate** (serveur
  multi-tenant, cible réseau interne au cluster) — à retraiter dans la feature sandbox,
  hors F2. Le crate lui-même ne fait aucune requête réseau : la surface SSRF reste
  entièrement côté appelant.
- **Aucune interpolation shell** — aucune commande construite, uniquement des écritures
  de fichiers via `std::fs` et de la sérialisation `yaml_serde`.

## Modules touchés

| Module | Changement |
|---|---|
| `cfgstore/` (nouveau crate) | `Cargo.toml` (leaf) ; `src/domain.rs` (déplacé de `lib/`) ; `src/store.rs` (trait `ConfigStore` lecture+écriture + `InMemoryConfigStore` + `resolve_by_name`, déplacé de `lib/`) ; `src/layers.rs` (`Layers`, `RawConfigFile`, merge/résolution, `*_entry_source`, déplacé de `cli/src/config.rs`) ; `src/fs_store.rs` (`FsConfigStore` lecture déplacée de `cli/` + **méthodes d'écriture** + validation `name`) ; `src/error.rs` (`CfgStoreError`) |
| `Cargo.toml` (racine) | `members += "cfgstore"` |
| `lib/Cargo.toml` | `vanyline-cfgstore = { path = "../cfgstore" }` |
| `lib/src/lib.rs` | `pub mod domain;` / `pub mod store;` → `pub use vanyline_cfgstore::{domain, store};` |
| `lib/src/error.rs` | `impl From<CfgStoreError> for VnyError` ; le test `error_codes` (aujourd'hui égaré dans `domain.rs`) revient ici / part dans `cfgstore/src/error.rs` |
| `lib/src/{session,builtin/task,builtin/skill}.rs` | `?` sur méthodes `ConfigStore` : conversion via `From<CfgStoreError>` — **propagation seule, aucun `match` sur le type d'erreur** |
| `app/src/config_store.rs` | `PgConfigStore` : signatures `ConfigStore` `VnyError` → `CfgStoreError` ; méthodes d'écriture non implémentées (héritent du défaut `ReadOnly`) |
| `app/src/error.rs` | `impl From<CfgStoreError> for AppError` |
| `cli/src/config.rs` | ne garde que le câblage hôte : `config_dir` / `data_dir` / `discover_workspace_root` / `configured_namespace` / `configured_toolbox` + nouvelle `discover_layers(start) -> Layers` |
| `cli/src/fs_store.rs` | **supprimé** (déplacé) — points d'usage passent à `use vanyline_cfgstore::FsConfigStore` |
| `cli/src/config_check.rs` | `check_config` : `Vec<VnyError>` conservé en signature, map à la frontière `CfgStoreError` → `VnyError` |
| `cli/Cargo.toml` | `vanyline-cfgstore = { path = "../cfgstore" }` ; `yaml_serde` retiré si plus utilisé directement |
| `cli/src/rpc/protocol.rs` | params create/update/delete, codes `VNL-RPC-011..015` |
| `cli/src/rpc/handlers.rs` | dispatch + handlers des nouvelles méthodes ; résolution `layer?` → `Layer` ; mapping `CfgStoreError` → `VNL-RPC-01x` |
| `cli/tests/rpc_stdio_smoke.rs` | round-trips par domaine, conflits, traversal rejeté, cible de couche |
| `docs/rpc-protocol.md` | section « Écriture de configuration » + note SSRF |
| `docs/architecture.md` | maj section RPC + nouveau crate `vanyline-cfgstore` (place dans le workspace, frontière crate) — à la clôture Phase 3 |

## Tests

### `cfgstore` — unitaires (déplacés + nouveaux)

- Tous les tests actuels de `cli/src/fs_store.rs`, `cli/src/config.rs` (module `tests`) et
  `lib/src/store.rs` / `lib/src/domain.rs` **passent inchangés** après déplacement (hormis
  adaptation du chemin d'import et du type d'erreur : `VnyError` → `CfgStoreError`).
- `create` → `list` contient l'entrée → `get` / lecture renvoie les bons champs →
  `update` (patch partiel : un champ modifié, les autres préservés ; `null` efface un
  optionnel) → `delete` → `list` ne la contient plus. Par domaine.
- `create` sur nom existant dans la couche → `NameConflict`. `update` / `delete` sur nom
  absent de la couche → `NotFound`.
- `name` = `"../evil"`, `"a/b"`, `".."`, `"/abs"`, `"a\\b"` → `InvalidName`, **aucun
  fichier créé hors du répertoire de config** (assert sur le FS).
- `provider_type` / `transport` / `mode` inconnu → `Validation`.
- `layer: Workspace` écrit dans `<root>/.vanyline/`, `layer: Global` dans le
  `global_dir` (tmpdir de test) ; l'un n'altère pas l'autre (relire les deux `config.yaml`
  après une écriture workspace : le global est identique octet pour octet sur les clés
  non touchées).
- écriture d'une entrée `config.yaml` : les autres maps (`providers`/`models`/`mcp`/
  `defaults`) et les autres entrées de la même map sont préservées **en contenu** (pas en
  formatage — `yaml_serde` ne garde ni commentaires ni ordre : documenté).
- `InMemoryConfigStore` : les méthodes d'écriture mutent les `Vec` internes, `Layer`
  ignoré ; `ReadOnly` jamais renvoyé.
- backend qui n'override pas l'écriture → `create_*` renvoie `ReadOnly`.

### `cli/tests/rpc_stdio_smoke.rs`

Par domaine :
- `create` → `list` contient l'entrée → `update` (patch partiel) → `delete` → `list` ne
  la contient plus.
- `create` sur nom existant → `VNL-RPC-013`. `update` / `delete` sur nom absent →
  `VNL-RPC-012`.
- `name` = `"../evil"`, `"a/b"`, `".."`, `"/abs"` → `VNL-RPC-014`, **aucun fichier créé
  hors du répertoire de config** (assert sur le FS).
- `provider_type` inconnu → `VNL-RPC-015`.
- `layer: "workspace"` écrit dans `<root>/.vanyline/`, `layer: "global"` dans le
  répertoire global (tmpdir de test), l'un n'altère pas l'autre.
- `config/providers` et `config/mcpServers` (lecture) renvoient les entrées attendues.
- `config/localTools` → 8 entrées attendues.

### CI

- Job `check`/`test` des crates : ajouter `cfgstore` (comme les autres membres du
  workspace).
- Job `tsrs` inchangé (`cargo test -p vanyline-lib --features ts-rs` puis
  `git diff --exit-code -- packages/protocol/src/generated/`) — `domain.rs` n'a aucun
  dérive ts-rs, son déplacement ne change pas la sortie générée. Vérifier quand même que
  `packages/protocol` compile après le déplacement (re-export lib).

## Risques et questions ouvertes

- **Task 0 est la task lourde** (extraction + ripple d'erreur `lib`/`app`/`cli`), plus
  grosse que l'ex-task 1. Découpage proposé en 0a / 0b (cf. section suivante) pour rester
  sous 30-45 min par task. Chaque sous-task doit laisser `cargo test --workspace` vert.
- **Ripple d'erreur** : le trait quitte `lib`, ne peut plus renvoyer `VnyError`. Choix
  retenu : `CfgStoreError` dans cfgstore + `From` vers `VnyError` et `AppError`. Le seul
  endroit qui **matche** sur le type d'erreur (pas juste `?`) est
  `cli/src/config_check.rs` (et un test app) — mappe à la frontière. Alternative écartée :
  garder le trait dans `lib` et donner à `FsConfigStore` des méthodes inhérentes +
  `impl ConfigStore` délégant — rejetée car le trait doit **porter toutes les méthodes**
  (demande explicite) et être utilisable par la sandbox, qui ne voit pas `lib`.
- **Défaut `ReadOnly` sur les méthodes d'écriture du trait** : retenu pour que
  `PgConfigStore` et les doubles de test compilent sans stub. Conséquence : appeler
  `create_agent` sur un `PgConfigStore` échoue au runtime (`ReadOnly`), pas à la
  compilation. Acceptable — `app` n'appelle jamais l'écriture via ce trait.
- **Couche globale côté sandbox** (question ouverte #2, non tranchée) : la sandbox
  n'aura peut-être qu'une couche workspace, ou un jeu de defaults baké en image
  (read-only). `Layers.global_dir` reste non-optionnel — pointer sur un chemin
  inexistant suffit (`load_config_layer` renvoie `RawConfigFile::default()` sur
  `NotFound`). À trancher dans la feature sandbox, pas dans F2.
- **`ConfigWrite` trait séparé** : écarté (demande explicite — tout sur `ConfigStore`).
- **Nom de domaine `models` vs `profiles`** : la CLI garde `config/models/*` (la map
  `models:` du `config.yaml`) ; `@vanyline/ui` dit `profiles` ; le pont RPC de F4
  traduit. À figer dans `rpc-protocol.md` pour ne pas piéger.
- **`providers`/`mcp` locaux et non restreints** côté CLI alors qu'ils sont
  globaux+AdminOnly côté app : cohérent (l'extension est un harness local), pas un bug.
- **Réécriture de `config.yaml`** : préserver l'ordre / les commentaires est hors
  portée (`yaml_serde` ne les garde pas) — accepté, documenté. Le round-trip préserve
  les *données*, pas le formatage.
- **`McpTransport::Sse`** : déjà présent dans `domain.rs` (commit `d5aaa54`, mergé) —
  l'item « F2 doit l'ajouter » de `vscode-ext-sequence.md` est caduc, retiré du scope.

## Découpage en tâches candidates

0a. **Crate `vanyline-cfgstore` + déplacement depuis `lib/`** : créer le crate (leaf),
    déplacer `domain.rs` et `store.rs` (trait + `InMemoryConfigStore` + `resolve_by_name`)
    depuis `lib/`, créer `CfgStoreError` (reprend `VNL-CFG-001/002/003` + `Io`), le trait
    renvoie `CfgStoreError`, `ConfigStore` gagne les **signatures** d'écriture avec défaut
    `ReadOnly` (pas d'impl réelle encore). `lib` re-exporte `domain` + `store`, ajoute
    `From<CfgStoreError> for VnyError` ; `lib/src/{session,builtin/*}` recâblés (`?`).
    `app` : `PgConfigStore` + `From<CfgStoreError> for AppError`. Tests déplacés, verts.
0b. **Déplacement depuis `cli/`** : `Layers` + `RawConfigFile` + machinerie +
    `*_entry_source` (→ `cfgstore/src/layers.rs`), `FsConfigStore` lecture (→
    `cfgstore/src/fs_store.rs`). `cli/src/config.rs` réduit au câblage hôte +
    `discover_layers`. `cli/src/fs_store.rs` supprimé. `config_check` mappe à la
    frontière. Tests déplacés, verts. `cargo test --workspace` vert.
1. **Écriture `config.yaml`** : `create/update/delete` providers / models / mcp_servers
   dans `FsConfigStore` + `InMemoryConfigStore` + validation `name` anti-traversal +
   `CfgStoreError` write-side (`VNL-CFG-005..010`) + `set_default_agent` plié dans le
   trait + tests unitaires cfgstore.
2. **Écriture fichiers / répertoires** : `create/update/delete` toolsets / agents /
   skills (`agents/<name>.md`, `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`) +
   tests.
3. **RPC** : `config/providers` + `config/mcpServers` (lecture manquante) +
   `config/<domain>/{create,update,delete}` + résolution `layer?` → `Layer` + mapping
   `CfgStoreError` → `VNL-RPC-01x` + smoke tests.
4. **Actions** : `test` providers / mcp + `config/localTools` + section
   `docs/rpc-protocol.md` (« Écriture de configuration » + note SSRF).

## Commandes de validation

(Voir `AGENTS.md` — section « Commandes de validation ». `cfgstore` est un membre du
workspace : `cargo check/test/clippy --workspace` et `cargo fmt --all -- --check` le
couvrent. `cargo fmt --all -- --check` **obligatoire** avant de considérer une task
terminée.)
