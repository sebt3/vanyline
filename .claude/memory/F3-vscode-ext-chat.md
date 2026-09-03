# F3 — `F3-vscode-ext-chat` (close 2026-09-03)

Troisième des 5 features « extension VS Code `vanyline` » (séquence :
`vscode-ext-sequence.md`). **L'extension elle-même** : sidebar de chat pilotant une CLI
`vanyline serve --stdio` locale que l'extension télécharge + met à jour seule. Inclut le
packaging `.vsix` et les jobs CI. Dépend de F1 (`@vanyline/{protocol,ui}`), pas de F2.

Branche `feat/F3-vscode-ext-chat` — **mergée dans `main` et poussée**.

## Ce qui a été livré

- **`ext/`** ajouté au workspace npm racine (`package.json` racine renommé
  `vanyline` → `vanyline-workspace` ; l'extension prend le nom `vanyline`).
- **Host** (esbuild → `dist/extension/index.js`, CJS Node) : `extension.ts` (activate,
  status bar, OutputChannel, commandes), `cli-provisioning.ts` (`ensureCli`),
  `rpc.ts` (`startServer` : spawn `shell:false` + transport ndjson + `initialize`),
  `supervisor.ts` (machine à états pure : backoff expo plafonné, fenêtre de stabilité,
  `restart`/`stop`, `generation` contre les await en vol), `panels/{chat,bridge,html}.ts`.
- **Webview** (Vite+Vue → `dist/webview/`) : monte `ChatWindow` de `@vanyline/ui`, ports
  `vanyline.chatBackend` / `vanyline.chatTransport` (`PostMessageChatTransport`), pont
  `postMessage` avec `getBridgeClient()` mémoïsé (`acquireVsCodeApi` appelable une fois).
- **Sécurité provisioning** (cœur de F3) : SHA256 obligatoire contre
  `vanyline-<target>.tar.gz.sha256` (refus `VNL-EXT-005` si 404, aucun fallback), HTTPS +
  allowlist d'hôte sur l'URL finale après redirects (`VNL-EXT-006`), install atomique
  (tmpfile → vérif → `chmod` → `copyFile` → `rename`), `~/.local/bin` uniquement, jamais
  d'exécution avant vérif. `execFile`/`spawn` argv tableaux `shell:false` partout, URLs
  construites depuis constantes de build. Codes `VNL-EXT-001..006`, `-010/-011` (serveur),
  `-020/-021` (pont).
- **Packaging** : job `ext` dans `release.yml` (`vsce package --no-dependencies`, attache
  le `.vsix`) + `checksum: sha256` sur `upload-cli` ; job `ext` dans `test.yml` ;
  `npm run install:local` racine ; `docs/ext-install.md` (install + checklist e2e).

Design migré dans `docs/architecture.md` § « Extension VS Code — `ext/` (F3) » ;
`docs/features/F3-vscode-ext-chat.md` supprimé.

## Bilan de délégation — Cadence, sans escalade, propre

Livré par **Cadence** (5 tâches). **Contraste net avec git-integration / miryad-core** :
review Phase 3 = **0 bug bloquant**, tous les tests verts avant et après. Niveau
équivalent à `lsp-integration`. Le `fmt`… sans objet : F3 ne touche aucun fichier Rust.

Écarts auto-déclarés par Cadence, tous jugés corrects en review :
- `createdAt` absent de la surface RPC F2 → repli `title = title ?? 'Session <id8>'` +
  `createdAt: ''` côté webview (le repli date de `ChatWindow` n'est jamais atteint).
  Backlog hors F3 : **le store CLI ne persiste aucune date de conversation**.
- Codes `-006/-020/-021` hors de la liste `-001..-005` du design (catégories que le
  design ne nommait pas). Acceptés.
- `ext/LICENSE` posé (BSD-3, `Copyright (c) 2026, Sébastien Huss`) — **confirmé par le
  développeur : son nom sur sa licence, comme ses autres projets OSS.** LICENSE racine du
  dépôt toujours absent (gouvernance repo, hors F3).
- Course connue héritée de F2/`packages/protocol/src/rpc.ts` (pending enregistré après
  write) — inatteignable sur stdio réel, laissée telle quelle.

## Review Phase 3 — 6 findings mineurs corrigés par Claude avant merge

Aucun bloquant. Corrigés directement (accord développeur « corriges tes trouvailles ») :

1. **`html.ts:generateNonce` — `Math.random()` → `node:crypto` `randomInt`.** Un nonce
   CSP n'a de valeur que s'il est imprévisible. Non exploitable ici (`buildHtml`
   n'interpole que des valeurs de confiance) mais defense-in-depth. Version kydah-code
   s'appuyait sur `Math.random`.
2. **`validateFinalUrl` s'exécute après que `fetch` a suivi les redirects** — la requête
   a déjà atteint l'hôte final. Conforme au design ; commenté dans le code + noté dans
   `architecture.md` : l'allowlist est un garde-fou intégrité/exfil, pas une garantie
   « on ne parle jamais à un hôte non-GitHub » (l'archive reste SHA256-gatée).
3. **`PostMessageChatTransport` — fuite d'abonnement.** Sur rejet du `chatSend` (comme
   sur `done`/`error`), `controller.error()` était appelé sans `unsubscribe()` ; le
   listener `abortSignal` 'abort' n'était jamais retiré non plus. Ajout d'un `cleanup()`
   appelé sur chaque issue terminale. +2 tests (`cas 11b/11c`).
4. **`RELAY_WHITELIST` : `conversations/delete` retiré.** Aucune affordance de
   suppression côté webview en F3 (`ChatBackend` n'a pas la méthode). Surface morte —
   ré-ajout possible en F4 quand `ChatWindow` exposera le geste.
5. **`docs/release-runbook.md` §1/§2/§4** : `ext/cli-version.txt` et `ext/package.json`
   `version` sont des **bumps manuels découplés** du workspace Rust — documenté (sinon
   l'extension va chercher une CLI d'une release antérieure sans assets `.sha256`).
6. **esbuild `sourcemap`** : `dist/extension/index.js.map` (80 Ko) était exclu du `.vsix`
   → réf sourcemap cassée. `.vscodeignore` : `**/*.map` → `webview/**/*.map`, la map host
   est embarquée (traces d'erreur lisibles pour une extension publiée).

### Fix adjacent — `chatEventsToUIStream` (`@vanyline/ui`, partagé avec le frontend web)

Surfacé par les nouveaux tests du finding 3. Le listener `abort` interne de
`chatEventsToUIStream` (F1, jamais retiré) appelle `ctrl.enqueue` **sans garde** — un
`abort` postérieur à la fermeture du controller lève `ERR_INVALID_STATE`. `enqueue`
devient un no-op si `controllerClosed`. Frontend : 306 tests verts après.

## e2e réel — testé sur code-server, un défaut trouvé + corrigé

Le développeur a installé le `.vsix` : l'extension s'active, mais le download du binaire
échouait en **`VNL-EXT-005` (asset `.sha256` absent, HTTP 404)**. Cause : la release
`v0.0.11-alpha.5` (pointée par `cli-version.txt`) est **antérieure** à l'ajout de
`checksum: sha256` sur `upload-cli` — elle n'a que les `.tar.gz`, pas les sidecars.
Le code de provisioning est correct (nom d'asset `vanyline-<target>.tar.gz.sha256`,
format GNU `<hash>␣␣<basename>` — vérifié contre la source de
`taiki-e/upload-rust-binary-action`).

**Résolution** : les deux `.sha256` ont été calculés (`sha256sum` sur les tarballs
publics) et **attachés manuellement à la release `v0.0.11-alpha.5`** via `gh release
upload`. `cli-version.txt` inchangé — le download fonctionne désormais. À partir de la
prochaine release coupée depuis `main` (qui porte `checksum: sha256`), les sidecars
seront produits automatiquement.

Reste à re-dérouler par le développeur : la checklist complète de `docs/ext-install.md`
(flux chat, backoff, QuickPick, sélecteur d'agent) sur code-server réel.

## Points à connaître pour F4 / F5

- **F4** (`F4-vscode-ext-config-ui`) : impl RPC de `ConfigRepo` (pass-through sur
  `config/<domain>/{create,update,delete}` de F2) + onglets éditeur pour les 6 écrans
  `@vanyline/ui`. La commande `vanyline.openSettings` ouvre pour l'instant les settings
  VS Code natifs — à remplacer.
- **F5** (`F5-vscode-ext-sandboxes`) : `TreeView` native via les méthodes RPC K8s
  existantes. Dépend de F3 (host + superviseur), parallélisable avec F4.
- Le `RELAY_WHITELIST` du pont host est le contrat de sécurité webview→CLI — toute
  nouvelle méthode relayée depuis la webview s'y ajoute explicitement.
- `chat/cancel` reste un no-op CLI (dette assumée, cf. `architecture.md` § Limites).
