# Extension VS Code `vanyline` — séquence de 5 features

Reprise de l'ancien `ws06-vscode-ext-bootstrap` (supprimé), redéfini le 2026-08-29 avec
le développeur en session de design. L'objectif n'est plus « bootstraper » mais faire de
l'extension un **composant utilisable** : chat + configuration + sandboxes, en
réutilisant au maximum le frontend web via des packages partagés.

## Décisions structurantes (prises en session, ne pas re-litiguer)

- **`packages/` partagés** : `@vanyline/protocol` (types TS générés ts-rs depuis
  `vanyline-lib`) + `@vanyline/ui` (composants Vue chat + config, **agnostiques du
  backend** via `provide`/`inject` d'un `ChatTransport` et d'un `ConfigRepo`).
- **`ConfigRepo` name-keyed dans la couche UI uniquement.** Le modèle de données de
  `app` garde ses PK `i32` (deux utilisateurs peuvent avoir un agent de même nom —
  `UNIQUE(owner_id, name)`). L'impl HTTP traduit `name ↔ id` en interne ; la CLI est
  name-keyed nativement.
- **Placement dans VS Code** : chat = webview `WebviewView` dans la secondary sidebar
  droite (comme kydah-code) ; config = webview `WebviewPanel` façon onglet éditeur.
  Câblage (webview provider, CSP/nonce/base href, relais postMessage, QuickPick) repris
  de kydah-code (`~/projets/kydah/kydah-code/src/extension/`).
- **L'extension pilote une CLI `vanyline serve --stdio` locale** — chat 100 % local
  (LLM + MCP/tools configurés dans le YAML), aucun lien avec `app` / les sandboxes K8s
  côté chat. La CLI est téléchargée/mise à jour par l'extension dans `~/.local/bin`
  (release GitHub publique `sebt3/vanyline`, déjà produite par le job `upload-cli` du
  `release.yml`).
- **Cible** : VS Code + code-server, **Linux x86_64 + aarch64 uniquement** (cibles des
  builds CLI). Publisher `sebt3`, nom `vanyline`. Pas de marketplace — `.vsix` attaché à
  la release + `install:local`.
- **On ne reprend pas** le gestionnaire de panneaux dockview ni l'éditeur/terminal web.

## Les 5 features (design docs = `docs/features/F<n>-vscode-ext-*.md`)

Fichiers, branches (`feat/<nom>`), répertoires de tâches (`.tasks/<nom>/`) et fichiers
mémoire de clôture (`.claude/memory/<nom>.md`) portent tous le même nom `F<n>-vscode-ext-…`.

| # | nom | résumé | dépend de |
|---|---|---|---|
| F1 | `F1-vscode-ext-foundations` | **close 2026-08-31.** `packages/protocol` + `packages/ui` extraits du frontend, `ConfigRepo` + impl HTTP, ts-rs. Comportement frontend inchangé. | — |
| F2 | `F2-vscode-ext-cli-rpc` | **close 2026-09-02, mergée+poussée.** Crate feuille `vanyline-cfgstore` (config extraite de `lib`+`cli`, partageable sandbox) + `ConfigStore` lecture→lecture/écriture + RPC `config/<domain>/{create,update,delete}` + actions test/localTools. | alignement noms avec F1 |
| F3 | `F3-vscode-ext-chat` | L'extension elle-même : host, provisioning CLI (download+SHA256), sidebar chat, **packaging + release CI**. | F1 |
| F4 | `F4-vscode-ext-config-ui` | Onglets éditeur pour éditer la config via les écrans `@vanyline/ui` + impl RPC de `ConfigRepo`. | F2, F3 |
| F5 | `F5-vscode-ext-sandboxes` | `TreeView` native Owners/Projects/Sandboxes via les méthodes RPC K8s existantes. | F3 |

Ordre d'exécution recommandé : **F1 → F2 → F3 → (F4 ‖ F5)**. F4 et F5 sont
parallélisables une fois F3 fait. F1 et F2 apportent de la valeur au frontend web même
si l'extension ne sortait jamais.

Chaque feature suit le workflow `mode feature` de `.claude/config.md` : Phase 1 (design
doc, fait) → Phase 2 (tâches just-in-time Qwen/Cadence) → Phase 3 (review Claude +
migration vers `docs/architecture.md` + suppression du design doc).

## État d'avancement

<!-- Une ligne par transition, datée. Clôture Phase 3 / démarrage de feature. -->

- **2026-08-29** — Phase 1 des 5 features écrite (les 5 design docs).
- **2026-08-31** — **F1 close (Phase 3 faite).** Branche `feat/F1-vscode-ext-foundations`
  (pas encore poussée/mergée). `@vanyline/protocol` + `@vanyline/ui` extraits :
  `ChatEvent` (ts-rs) + enveloppes RPC + `RpcConnection` + `config-domain.ts` ;
  composants chat + les 6 écrans config + `ConfigShell` agnostiques du backend via 3
  ports injectés (`ChatTransport`, `ChatBackend`, `ConfigRepo`). `frontend/` implémente
  les ports (`httpConfigRepo` bidirectionnel REST↔canonique, `httpChatBackend`,
  `VanylineChatTransport`). Design doc migré dans `docs/architecture.md` (§ Workspace
  TypeScript), `docs/features/F1-vscode-ext-foundations.md` supprimé. Bilan complet
  (décisions, 2 blocages Phase 2, délégation Qwen ratée sur task 06) :
  `.claude/memory/F1-vscode-ext-foundations.md`.
- **2026-09-02** — **F2 close (Phase 3 faite), branche `feat/F2-vscode-ext-cli-rpc`
  mergée dans `main` et poussée.** Nouveau crate feuille **`vanyline-cfgstore`**
  (`domain` + `store::ConfigStore` déplacés de `lib`, `Layers` + `FsConfigStore`
  déplacés de `cli` ; `lib` re-exporte `domain` + `store`). `ConfigStore` gagne
  `create_*`/`update_*`/`delete_*` + `set_default_agent` (défaut `ReadOnly`), cible de
  couche explicite. RPC : `config/<domain>/{create,update,delete}` pour les 6 domaines,
  actions `config/{providers,mcpServers}/test` + `config/localTools`, codes
  `VNL-RPC-011..015`. Design doc migré dans `docs/architecture.md` (§ Vue d'ensemble,
  Configuration CLI, RPC stdio, Backend web, Workspace TypeScript),
  `docs/features/F2-vscode-ext-cli-rpc.md` supprimé. Bilan complet (décisions, bascule
  modèle Cadence en cours de feature, historique squatté, rouge CI clippy, **3 bugs
  corrigés en review Phase 3**) : `.claude/memory/F2-vscode-ext-cli-rpc.md`.
  **Suite : F3** (`F3-vscode-ext-chat`).

## Comment reprendre dans une session neuve

- **« Démarre F1 / la feature suivante »** (nom = `F<n>-vscode-ext-<slug>`) : lire
  `docs/features/<nom>.md`, créer la branche `feat/<nom>` depuis `main` à jour, commiter
  le design doc dessus, puis Phase 2 — définir la première tâche just-in-time
  (`.tasks/<nom>/task-01-*.md`) d'après l'état réel du code. Une ou deux tâches d'avance
  max.
- **« Fais la Phase 3 de F<n> »** : review Claude du code produit sur la branche, contre
  le design doc. Vérifier les « risques et questions ouvertes » du doc un par un.
  Lancer les 4 commandes de validation **dont `cargo fmt --all -- --check`**
  (`AGENTS.md`). Puis : migrer les parties durables du design vers
  `docs/architecture.md` (section « Workspace TypeScript » ligne ~1726 pour F1, section
  RPC ligne ~251 pour F2, nouvelle section « Extension VS Code » pour F3-F5), supprimer
  `docs/features/<nom>.md`, mettre à jour ce fichier + `.claude/MEMORY.md`, et
  créer/mettre à jour `.claude/memory/<nom>.md` avec le bilan (décisions, pièges, bilan
  de délégation).
- **Véto partagé développeur + Claude** sur le passage Phase 1 → Phase 2 de chaque
  feature : « c'est prêt » requiert l'accord explicite des deux.

## Rappels techniques transverses

- ts-rs sur `ChatEvent` (`#[serde(tag = "type")]`) : **a marché** (F1 tâche 1), pas de
  repli. `ts-rs` v12, `TS_RS_LARGE_INT="number"` dans `.cargo/config.toml`. Job CI `tsrs`
  régénère + `git diff --exit-code`.
- **`config-domain.ts`** : miroir **manuel** de `domain.rs` (déplacé de `lib/` vers
  `cfgstore/` en F2 ; `vanyline_lib::domain` le re-exporte). Forme = serde à la lettre,
  name-keyed. Traduction wire REST `app` ↔ canonique dans `httpConfigRepo` (F1) ; l'impl
  RPC (F4) = pass-through sur `config/<domain>/{create,update,delete}` (F2). `Sse` est
  dans `McpTransport` des deux côtés (ajouté avant F2, `d5aaa54`).
- Validation anti-traversal des `name` de config (deviennent des noms de fichiers) :
  livrée en F2 dans **`vanyline-cfgstore::fs_store::validate_name`**, appliquée avant
  toute opération disque dans chaque chemin d'écriture (pas dans le handler RPC).
- **F2 : `cadence` + `implement` sur le même modèle (qwen3.8-flash-next) → validation
  croisée faible.** La review Claude Phase 3 a trouvé 2 bugs bloquants + 1 rouge CI que
  Cadence avait validés. Si ce couplage persiste, Phase 3 est le seul vrai filet.
- Download binaire (F3) : SHA256 obligatoire contre l'asset `.sha256` de la release
  (refus si absent), HTTPS + allowlist d'hôtes après redirects, install atomique.
  Prérequis CI : ajouter `checksum: sha256` au job `upload-cli` du `release.yml`.
- Nom de domaine : `profiles` (UI) = `models` (CLI `config.yaml`) = `model-profiles`
  (app REST). Un seul point de traduction par impl de `ConfigRepo`.
