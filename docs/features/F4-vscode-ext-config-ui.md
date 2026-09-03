# Feature — F4-vscode-ext-config-ui

Quatrième des cinq features « extension VS Code `vanyline` ». Séquence et état :
`.claude/memory/vscode-ext-sequence.md`. Dépend de **F2** (RPC write-side) et **F3**
(host + `RpcConnection` + webview).

## Ce que la feature fait

Ajoute à l'extension des onglets **éditeur** (webview `WebviewPanel`) pour éditer les
cinq domaines de config + skills via les écrans `@vanyline/ui`, branchés sur la CLI par
RPC. Le chat reste dans la sidebar (F3), la config s'ouvre comme un document.

## Ce qu'elle ne fait pas

- Pas de nouvelle capacité RPC **de comportement** — à l'exception de deux ajouts
  CLI/RPC tranchés au démarrage de F4 (2026-09-03, développeur) :
  - **`config/skills/get`** : lecture `{name, description, body, source}` d'un skill.
    Découverte à la lecture du code : `SkillsScreen.vue` (F1, figé) appelle
    `repo.get('skills', name)` pour charger le `body` avant édition, et F2 n'expose
    aucune lecture du body → l'édition d'un skill existant l'écraserait avec un body
    vide. Le store lit déjà le body (`cfgstore fs_store.rs`, `load_skill`) ; l'ajout
    est un handler RPC + smoke test + rpc-protocol.md.
  - **Champ `source` en lecture** : les 6 lectures `config/<domain>` (et
    `config/skills/get`) ajoutent `"source": "workspace" | "global"` par entrée
    (additif — les clients existants ignorent la clé), résolu par
    `config_entry_source`/`file_entry_source`/`skill_entry_source` (cfgstore, déjà
    utilisé par `config check`). Sert au badge de couche (ci-dessous).
- **Pas de synchronisation avec les settings VS Code natifs** — la config vit dans le
  YAML deux-couches de la CLI, pas dans `settings.json`. Divergence assumée avec
  kydah-code (qui, lui, stocke tout dans `configuration.properties`).
- Pas d'éditeur texte brut du `config.yaml` en secours.
- Pas de sélecteur de couche (workspace/global) actif en v1 — hérite du défaut F2. Un
  **badge lecture seule** « workspace » / « global » par entrée est en revanche souhaité
  dès v1 (voir questions ouvertes).

## Architecture

```
ext/
├── src/panels/config.ts   commande vanyline.openSettings → WebviewPanel (colonne éditeur),
│                          panel unique réutilisé, relais postMessage ↔ RPC F2
└── webview/src/
    ├── main.ts            router interne : ?view=chat | ?view=config (même bundle)
    └── rpcConfigRepo.ts   impl ConfigRepo (@vanyline/ui) sur postMessage → host → RPC
```

### Host — `panels/config.ts`

- `vanyline.openSettings` → `window.createWebviewPanel('vanyline.config', 'vanyline —
  Configuration', ViewColumn.Active, { retainContextWhenHidden: true })`. Si déjà
  ouvert : `panel.reveal()`.
- Même `buildHtml` (CSP + nonce + base href) que le chat, même `dist/webview`.
- Relaie `config/*` (list/get/create/update/delete) et les actions (`config/providers/
  test`, `config/mcpServers/test`, `config/localTools`) entre la webview et
  `RpcConnection`.
- Après **tout write réussi**, émet `postMessage({ type: 'config/changed', domain })` à
  **toutes** les webviews (sidebar chat incluse) → le sélecteur d'agent du chat refetch.

### Webview — mode config

- Même bundle que le chat, `main.ts` choisit le composant racine selon `?view=`.
- Monte `ConfigShell` de `@vanyline/ui` (nav gauche + groupes, extraite en F1) avec les
  6 écrans.
- Fournit (`provide`, clé `vanyline.configRepo`) une instance de `rpcConfigRepo` :
  - `ConfigRepo` est **name-keyed nativement côté CLI** → pas de résolution `name→id`,
    plus simple que l'impl HTTP du frontend.
  - traduit le domaine `'profiles'` (UI) ↔ `config/models/*` (RPC).

## Modules touchés

| Module | Changement |
|---|---|
| `ext/src/panels/config.ts` | **nouveau** — WebviewPanel + relais `config/*` |
| `ext/src/extension.ts` | `vanyline.openSettings` ouvre le vrai panel (remplace le fallback F3) |
| `ext/webview/src/main.ts` | routeur interne `?view=` |
| `ext/webview/src/rpcConfigRepo.ts` | **nouveau** — impl `ConfigRepo` sur postMessage |
| `ext/package.json` | rien de neuf côté `contributes` (commande déjà déclarée en F3) |
| `cli/src/rpc/handlers.rs` + `cli/tests/rpc_stdio_smoke.rs` + `docs/rpc-protocol.md` | les 2 ajouts RPC tranchés (`config/skills/get`, `source` en lecture) |
| `@vanyline/protocol` (`config-domain.ts`) | champ optionnel `source?: 'workspace' \| 'global'` sur les 6 types de domaine (même précédent que `available_models`/`is_default` : « augmenté, lecture seule ») |
| `@vanyline/ui` | écrans : badge « workspace »/« global » par entrée quand `source` présent (les écrans et `ConfigShell` viennent de F1, seule cette addition les touche) |

## Sécurité (argv / URL / chemin)

- Les `name` saisis dans les formulaires transitent vers `config/*/create` → la
  **validation anti-traversal est côté CLI (F2, `VNL-RPC-014`)**. La webview ne fait que
  présenter l'erreur. Aucune logique de chemin côté extension.
- Aucun nouveau canal d'exécution : postMessage → RPC → écriture de fichiers `std::fs`.

## Tests

- `ext/webview` (Vitest) : `rpcConfigRepo` — chaque méthode émet le bon `postMessage`,
  résout sur la réponse `<type>/response`, propage l'erreur `{ error }`. Mapping
  `profiles` ↔ `models` vérifié.
- `ext` host (Vitest) : `panels/config.ts` relaie correctement, émet `config/changed`
  après un write, réutilise le panel existant.
- Test e2e manuel documenté (`docs/ext-install.md`, section config) : créer un agent
  depuis l'extension → visible dans `vanyline config check` en ligne de commande →
  visible dans le sélecteur du chat sans rechargement.

## Risques et questions ouvertes

- **`WebviewPanel` (éditeur) et `WebviewView` (sidebar) ne partagent pas d'état** — deux
  instances Vue. Résolu par `config/changed` host→webviews + refetch. Vérifier qu'aucun
  écran ne garde de cache silencieux qui survivrait à l'événement.
- **Badge de couche** : **tranché 2026-09-03 — inclus en F4.** La source par entrée est
  rendue par le champ additif `source` des lectures RPC (voir « Ce qu'elle ne fait pas »),
  miroitée par `source?` optionnel dans `config-domain.ts`, et affichée par les écrans
  `@vanyline/ui` quand présente (jamais présente côté web/`app` → pas de badge sur le
  frontend web, inchangé).
- **Mapping `profiles`/`models`** : un seul point de traduction (`rpcConfigRepo`), testé
  explicitement — un bug ici est silencieux (mauvais domaine écrit).
- **`ConfigShell` extraite en F1** : si F1 ne l'a finalement pas fait (dette), la
  première tâche de F4 la remonte — ne pas réimplémenter une nav dans `ext/`.
- **Deux modes dans un bundle** vs deux bundles : retenu un bundle + `?view=` pour ne
  pas dupliquer la chaîne Vite / le poids. Si le tree-shaking ne sépare pas bien
  chat/config, repli sur deux entrypoints Vite.

## Découpage en tâches (figé au démarrage de F4, 2026-09-03)

`ConfigShell` ayant été extrait dès F1, la tâche 1 candidate est tombée ; les deux
décisions (badge, `config/skills/get`) sont tranchées ci-dessus. Ordre réel :

1. CLI : `config/skills/get` (+ smoke tests + rpc-protocol.md).
2. CLI : champ additif `source` sur les 6 lectures `config/<domain>` (+ smoke + doc).
3. `@vanyline/protocol` : `source?` optionnel sur les 6 types de `config-domain.ts`.
4. `@vanyline/ui` : badge « workspace »/« global » par entrée quand `source` présent.
5. `panels/config.ts` : WebviewPanel + CSP (`buildHtml` + paramètre de vue) + routeur
   `?view=` webview + « hello ConfigShell » (repo stub) + `openSettings` réel.
6. `rpcConfigRepo` + pont host `config/*` (whitelist) + mapping `profiles↔models` + tests.
7. Invalidation `config/changed` cross-webview + refetch du sélecteur d'agent du chat.
8. Écrans bout-à-bout (vérification) + section e2e manuel dans `docs/ext-install.md`.
