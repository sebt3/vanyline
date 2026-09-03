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
| F3 | `F3-vscode-ext-chat` | **close 2026-09-03, mergée+poussée.** L'extension elle-même : host, provisioning CLI (download+SHA256), sidebar chat, **packaging + release CI**. | F1 |
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
- **2026-09-03** — **F3 close (Phase 3 faite), branche `feat/F3-vscode-ext-chat` mergée
  dans `main` et poussée.** L'extension `vanyline` elle-même : `ext/` ajouté au workspace
  npm (racine renommée `vanyline-workspace`), host esbuild (extension/provisioning/rpc/
  superviseur/pont) + webview Vite-Vue montant `ChatWindow` de `@vanyline/ui`,
  provisioning CLI SHA256-vérifié + install atomique, jobs CI `ext` (`test.yml` +
  `release.yml` avec `checksum: sha256`), `install:local`, `docs/ext-install.md`. Livré
  par **Cadence sans escalade, 0 bug bloquant en review** (contraste net avec
  git-integration/miryad-core). Review Phase 3 : 6 findings mineurs corrigés par Claude
  (nonce crypto, fuite d'abonnement du transport, whitelist réduite, runbook, sourcemap
  du vsix) + 1 fix adjacent dans `chatEventsToUIStream` (`@vanyline/ui`, partagé). e2e
  réel sur code-server : `VNL-EXT-005` (release `v0.0.11-alpha.5` antérieure à
  `checksum: sha256`) → `.sha256` attachés manuellement à la release, download OK.
  Design migré dans `docs/architecture.md` § « Extension VS Code — `ext/` (F3) »,
  `docs/features/F3-vscode-ext-chat.md` supprimé. Bilan complet :
  `.claude/memory/F3-vscode-ext-chat.md`.
  **Suite : F4 ‖ F5** (parallélisables).

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
- Download binaire (F3, **livré**) : SHA256 obligatoire contre `vanyline-<target>.tar.gz.sha256`
  de la release (refus `VNL-EXT-005` si absent), HTTPS + allowlist d'hôtes sur l'URL finale
  après redirects, install atomique. `checksum: sha256` sur `upload-cli` — **une release
  d'avant ce changement n'a pas les sidecars** (les attacher à la main via `gh release
  upload` si `cli-version.txt` la pointe). `ext/cli-version.txt` + `ext/package.json`
  `version` = bumps manuels, cf. `docs/release-runbook.md` §2.
- Le pont host (`ext/src/panels/bridge.ts`) a un `RELAY_WHITELIST` = contrat de sécurité
  webview→CLI. F3 : `conversations/list|get|create` + `config/agents`. Toute méthode
  relayée depuis la webview en F4/F5 s'y ajoute explicitement.
- Nom de domaine : `profiles` (UI) = `models` (CLI `config.yaml`) = `model-profiles`
  (app REST). Un seul point de traduction par impl de `ConfigRepo`.
