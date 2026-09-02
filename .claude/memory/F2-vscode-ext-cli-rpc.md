# F2 — vscode-ext-cli-rpc (close 2026-09-02)

Deuxième des 5 features « extension VS Code `vanyline` » (cf. [[vscode-ext-sequence]]).
**Aucune ligne d'extension** ici : F2 (a) extrait la couche config dans un crate feuille
partageable et (b) ajoute le CRUD de config au RPC `vanyline serve --stdio`.

Branche `feat/F2-vscode-ext-cli-rpc` — mergée dans `main` et poussée à la clôture.

## Ce qui a été livré

### Crate `vanyline-cfgstore` (feuille, sans harness)

Nouveau 8ᵉ crate. Deps : `serde`/`serde_json`/`yaml_serde`/`async-trait`/`thiserror`
uniquement — **pas** de rig/rmcp/tokio-full, pour que la **sandbox** (qui ne dépend pas
de `vanyline-lib`) puisse le consommer plus tard.

- `domain` — déplacé tel quel de `lib/src/domain.rs`. `vanyline_lib::domain` le
  **re-exporte** → aucun churn sur les `use vanyline_lib::domain::…` existants
  (app/cli/`packages/protocol`). Les tests `*_wire_shape` sont partis avec.
- `store` — `ConfigStore` (trait) + `InMemoryConfigStore`, déplacés de `lib/src/store.rs`.
- `layers` — `Layers`, `RawConfigFile`, fusion deux couches, `*_entry_source` — déplacés
  de `cli/src/config.rs`.
- `fs_store` — `FsConfigStore` + `validate_name`, déplacé de `cli/src/fs_store.rs`
  (fichier supprimé côté cli).
- `error` — `CfgStoreError`, codes `VNL-CFG-001..010` (001-004 lecture/IO repris de
  `VnyError` ; 005-010 écriture : InvalidName, NotFound, NameConflict, WriteError,
  ReadOnly, Validation).

`cli/src/config.rs` réduit au **câblage hôte** : `config_dir`/`data_dir` (`dirs`),
`discover_workspace_root` (remontée cwd), `discover_layers` (assemble un `Layers`).

### `ConfigStore` — un seul trait, lecture + écriture

Décision développeur (« ConfigStore doit avoir toutes les méthodes ») : **pas** de trait
`ConfigWrite` séparé. Le trait porte `list_*`/`get_*` **et** `create_*`/`update_*`/
`delete_*` par domaine + `set_default_agent`, avec **cible de couche explicite**
(`Layer::Global` / `Workspace`). Les méthodes d'écriture ont un **défaut
`Err(CfgStoreError::ReadOnly)`** → `PgConfigStore` (app) et les doubles de test compilent
sans stub ; seuls `FsConfigStore` et `InMemoryConfigStore` les implémentent pour de vrai.

Ripple d'erreur (le trait quitte `lib`, ne peut plus renvoyer `VnyError`) : `CfgStoreError`
+ `From<CfgStoreError>` pour `VnyError` (lib) et `AppError` (app). Un seul point qui
*matche* le type au lieu de `?` : `cli/src/config_check.rs`.

### Écriture — sécurité et robustesse

- `validate_name` (`^[a-zA-Z0-9][a-zA-Z0-9._-]*$`, ≤ 64, rejet `..`/`/`/`\`/absolu)
  appliquée **avant toute opération disque** dans chaque chemin d'écriture — la
  contrainte vit dans le crate, pas dans le handler RPC (bénéficie à la future surface
  sandbox sans réécriture). `VNL-CFG-005` → `VNL-RPC-014`.
- `update` de `config.yaml` (providers/models/mcp) : merge JSON brut **puis revalidation
  typée** de l'entrée via son `Raw*Entry` avant écriture (fix Phase 3, cf. plus bas).
- `update` fichiers (toolsets/agents/skills) : `parse_*_file` → patch typé champ par
  champ → réécriture. Pas de rename via patch (clé `name` ignorée).
- Écritures **non atomiques** (`std::fs::write` direct) — dette assumée, à revoir avant
  que la sandbox consomme le crate.

### RPC — `cli/src/rpc/handlers.rs`

- Lectures ajoutées : `config/providers`, `config/mcpServers`.
- `config/<domain>/{create,update,delete}` pour les 6 domaines. `layer?` optionnel
  (`resolve_layer` : workspace si résolu à `initialize`, sinon global). `item`/`patch`
  passthrough snake_case (types `domain` tels quels).
- Actions : `config/providers/test` + `config/mcpServers/test` (sondage réseau de la
  cible **stockée dans la config** — SSRF assumée, serveur local), `config/localTools`
  (registre statique des 8 tools intégrés, lecture seule).
- Codes `VNL-RPC-011..015` (`protocol.rs::vnl_code`), mapping depuis `CfgStoreError` dans
  `config_write_response`.

## Décisions structurantes (prises en session, ne pas re-litiguer)

1. **Crate feuille, pas un split de `lib` par features.** La sandbox ne dépend pas de
   `vanyline-lib` (serveur de pod léger — lib traîne rig/rmcp). « Partageable à la
   sandbox » = crate feuille dédié, pas `lib` avec `default-features = false`.
2. **Un seul `ConfigStore`, toutes les méthodes, défaut `ReadOnly` sur l'écriture.**
   Alternative écartée (trait dans `lib` + méthodes inhérentes sur `FsConfigStore`) :
   rejetée car la sandbox doit pouvoir utiliser le trait sans voir `lib`.
3. **`domain.rs` déplacé, `lib` re-exporte.** Zéro churn sur les imports existants.
4. **`config.yaml` : ordre et commentaires perdus à la réécriture** (`yaml_serde` ne les
   garde pas) — accepté, le round-trip préserve les *données*.
5. **`models` (RPC/CLI) = `profiles` (`@vanyline/ui`) = `model-profiles` (app REST)** —
   figé dans `docs/rpc-protocol.md`, F4 traduit.
6. **Couche globale côté sandbox : question laissée ouverte.** `Layers.global_dir` reste
   non-optionnel ; la sandbox pointera sur un chemin inexistant (`load_config_layer`
   renvoie `default()` sur `NotFound`) ou un jeu de defaults baké en image. À trancher
   dans la feature sandbox.

## Bilan de délégation — Cadence, puis Cadence/qwen3.8-flash-next

Feature pilotée par **Cadence**. Bascule de modèle **en cours de feature** : tâches
0a/0b/1/2 (2026-08-31) sous `smart/deepseek-v4-flash` ; tâches 3/4 (2026-09-02) après
bascule de `cadence` **et** `implement` sur `dgx/qwen3.8-flash-next` (reasoningEffort
xhigh, temp 1.0). Constat du développeur : **cadence et implement sur le même modèle →
la validation croisée ne vaut plus grand-chose** (« cadence a toujours été content »).
La review Phase 3 l'a confirmé — 2 bugs réels + 1 commit mal étiqueté + 1 rouge CI ont
passé le verdict « content » de Cadence :

- **Historique squatté** : tâche 1a (commit `b7bcbc0`, écriture `config.yaml`) +
  un commit parasite `33b3c1f` « X-Commit-Payload-X » `reset` deux fois pendant 1b, le
  contenu 1a+1b re-committé sous le seul libellé 1b (`a08c727`). Aucun contenu perdu,
  granularité d'historique seule. Pas de chirurgie git (reset --hard interdit).
- **Rouge CI clippy** : les `mod tests` de `cfgstore/src/{fs_store,layers}.rs` sans
  `#![allow(clippy::unwrap_used, clippy::expect_used)]` (que `domain.rs`/`store.rs`
  avaient) → ~390 lints, **verts en local sans `--all-targets`**, rouges en CI. Corrigé
  (`9821060`) + `AGENTS.md`/`docs/architecture.md` alignés sur la commande CI exacte
  (`cargo clippy --workspace --all-targets -- -D warnings`) — même classe de trou que le
  `cargo fmt` récurrent.
- `InMemoryConfigStore` : champs passés en `Mutex<Vec<…>>` (le trait prend `&self`), 27
  sites de construction de test adaptés. `lock().unwrap()` d'abord posé, corrigé en
  `unwrap_or_else(|e| e.into_inner())` (pattern projet, `16cabba`).

## Review Phase 3 (Claude) — 3 bugs corrigés, minors reportés

Commit `f6d4c96` (`fix: review Phase 3 F2 …`).

1. **`update` mal typé empoisonnait tout le domaine.** `config/providers/update` avec
   `{"patch": {"endpoint": 12345}}` (type invalide, **pas** `null`) → `Ok()`, écrivait la
   valeur, et **toute relecture du domaine** (`config/providers`, `chat/send`) tombait en
   `VNL-CFG-001`. Même classe que le null-sur-requis de `74dded9`, qui n'avait patché que
   le cas `null`. Les `update` de `config.yaml` faisaient un merge JSON brut sans
   revalidation typée — contrairement à `InMemoryConfigStore::update_*` (typé) et
   `FsConfigStore::update_{toolset,agent,skill}` (typé). Fix : revalidation via
   `Raw*Entry` avant écriture. Vérifié empiriquement (test jetable).
2. **`config/mcpServers/test` pouvait geler tout le serveur RPC.** Dispatch série
   (`handle_line` await en boucle) + client MCP streamable-http **sans timeout** → une
   URL qui accepte la connexion sans répondre bloquait `chat/send`, `config/*`,
   `shutdown`. `config/providers/test` avait un timeout reqwest 10 s, pas l'autre. Fix :
   `tokio::time::timeout(10s)` autour de `list_mcp_server_tools`.
3. **Les 2 actions réseau avaient zéro test.** Ajout : nom inconnu → `VNL-RPC-006`,
   endpoint injoignable → `VNL-RPC-006`, **cible black-hole** (listener TCP qui accepte
   sans répondre) → réponse en ~10 s **et serveur toujours vivant** (valide le fix #2).

**Minors reportés (non bloquants)** :
- `set_default_agent` implémenté dans le trait mais **non exposé en RPC** — F4 (UI
  config) en aura besoin.
- `resolve_layer(None) → Global` quand le workspace n'est pas résolu (pas de
  `.git`/`.vanyline` au-dessus du dossier ouvert). Conforme au design, mais footgun F4 :
  toujours envoyer `layer` explicite depuis l'extension.
- `delete_skill` : `remove_file(SKILL.md)` puis `remove_dir(parent)` — échoue si le
  répertoire a d'autres fichiers → skill à moitié supprimé.
- Écritures non atomiques (cf. plus haut).
- `From<CfgStoreError> for AppError` mappe tout → `InternalError` 500 ; inoffensif (app
  n'appelle jamais `PgConfigStore` en écriture via le trait).

Validation finale : `cargo test --workspace` (cfgstore 175, rpc_stdio_smoke 18, tout le
reste vert), `fmt`, `clippy --workspace --all-targets -- -D warnings`, job `tsrs`
(`generated/` propre malgré le déplacement de `domain.rs`), `@vanyline/protocol` check+test.

## Pour la suite

- **F3** (`F3-vscode-ext-chat`) : l'extension elle-même. Le RPC config est prêt à être
  consommé ; F4 fera le pont `ConfigRepo` RPC (pass-through, cf. [[F1-vscode-ext-foundations]]).
- **Feature sandbox config** (pas encore planifiée) : brancher `vanyline-cfgstore` dans
  `sandbox/` pour éditer le `.vanyline/` d'un workspace. Prérequis : trancher la couche
  globale (point 6) ; passer les écritures en atomique (tmp + rename) ; la note SSRF de
  `docs/rpc-protocol.md` ne tiendra plus (serveur multi-tenant, cible réseau interne).
- Modèle Cadence : si `cadence` et `implement` restent sur le même modèle, la review
  Claude Phase 3 est le seul vrai filet — en tenir compte dans la vigilance.
