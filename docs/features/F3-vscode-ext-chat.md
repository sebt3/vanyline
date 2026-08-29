# Feature — F3-vscode-ext-chat

Troisième des cinq features « extension VS Code `vanyline` ». Séquence et état :
`.claude/memory/vscode-ext-sequence.md`. Dépend de **F1** (packages `@vanyline/ui` +
`@vanyline/protocol`). N'a pas besoin de F2.

## Ce que la feature fait

L'extension VS Code `vanyline` elle-même : une sidebar droite avec le chat (composants
`@vanyline/ui`), pilotant une CLI `vanyline serve --stdio` locale que l'extension
**télécharge et met à jour automatiquement** dans `~/.local/bin`. Inclut le packaging
`.vsix` et le job de release CI.

## Ce qu'elle ne fait pas

- **Pas d'UI de configuration** (F4) — la commande « Paramètres » ouvre pour l'instant
  les settings VS Code natifs de l'extension.
- **Pas de gestion sandbox / K8s** (F5).
- Pas de FIM / complétion inline, pas de LSP client, pas de highlighting sémantique —
  kydah-code les a, `vanyline` n'en veut pas ici (le LSP vit dans la sandbox).
- Pas de publication marketplace : `.vsix` attaché à la release GitHub +
  `npm run install:local` pour code-server.
- **Linux x86_64 + aarch64 uniquement** (cibles des builds `upload-cli` du
  `release.yml`). Pas de macOS/Windows.
- Pas d'annulation réelle de tour (`chat/cancel` reste un no-op côté CLI, dette assumée).
- Pas de réutilisation du shell dockview / de l'éditeur web — VS Code a son éditeur.

## Architecture

```
ext/
├── package.json            manifeste VS Code (main: ./dist/extension/index.js)
├── esbuild.mjs             bundling host (repris de kydah-code, allégé : cible "extension" seule)
├── src/
│   ├── extension.ts        activate/deactivate, spawn CLI, OutputChannel, status bar, backoff
│   ├── cli-provisioning.ts résolution + download + vérif SHA256 du binaire vanyline
│   ├── rpc.ts              RpcConnection (@vanyline/protocol) branché sur child.stdin/stdout
│   └── panels/chat.ts      WebviewViewProvider (sidebar), relais postMessage ↔ RPC, QuickPick
└── webview/                Vite + Vue, build séparé → ext/dist/webview/
    └── src/
        ├── main.ts         monte ChatWindow de @vanyline/ui
        └── postMessageChatTransport.ts   ChatTransport sur postMessage
```

### Host — `extension.ts`

- `activate` : résout le binaire (voir provisioning) → `spawn(bin, ['serve', '--stdio'],
  { shell: false, cwd: workspaceFolders[0], env: … })` → `RpcConnection` sur
  stdin/stdout → `initialize` avec `{ protocolVersion, workspace:
  workspaceFolders[0].uri.fsPath }`.
- stderr du CLI → OutputChannel « vanyline ».
- Status bar : `démarrage` / `prêt` / `erreur` / `redémarrage (n)`.
- Crash du process → redémarrage avec backoff exponentiel plafonné ; au-delà, message
  actionnable + commande `vanyline.restartServer`, on ne martèle pas.
- `deactivate` : `shutdown` RPC puis kill.

### Provisioning CLI — `cli-provisioning.ts`

- **Version attendue bakée au build** : `EXPECTED_CLI_VERSION`, injectée par esbuild
  `define` depuis un fichier `ext/cli-version.txt` versionné (bump explicite, découplé de
  la version de l'extension).
- Check : si `vanyline.serverPath` est défini → on l'utilise tel quel, **auto-update
  désactivée**, log clair du binaire utilisé. Sinon : `~/.local/bin/vanyline` existe et
  `--version` == `EXPECTED_CLI_VERSION` ? oui → rien ; non → download.
- Download :
  `https://github.com/sebt3/vanyline/releases/download/v<ver>/vanyline-<target>.tar.gz`
  avec `target ∈ {x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu}` résolu depuis
  `process.arch` / `process.platform`.
- **Intégrité (obligatoire)** :
  1. télécharger `vanyline-<target>.tar.gz.sha256` de la **même release**, vérifier le
     hash de l'archive. **Si le `.sha256` est absent → refus du download**, pas de
     fallback « on fait confiance ». (Prérequis CI : `checksum: sha256` sur le job
     `upload-cli` — cf. plus bas.)
  2. HTTPS strict ; suivre les redirects mais **valider l'hôte final** contre une
     allowlist (`github.com`, `objects.githubusercontent.com`, `*.githubusercontent.com`)
     — refus sinon.
- Install atomique : download → fichier temporaire → vérif → `chmod +x` → `rename` vers
  `~/.local/bin/vanyline`. **Jamais d'exécution avant vérif.**
- Erreurs identifiées : `VNL-EXT-001` pas de réseau, `-002` checksum invalide, `-003`
  target non supporté, `-004` `~/.local/bin` non inscriptible, `-005` `.sha256` absent.

### Webview — sidebar chat

- `WebviewViewProvider` sur `vanyline.chatView` dans `viewsContainers.secondarySidebar`
  (`vanyline`, icône `$(hubot)`), `retainContextWhenHidden: true`.
- `buildHtml` : CSP + nonce + `<base href>` — repris de kydah-code
  (`src/extension/panels/main.ts`), `localResourceRoots` sur `dist/webview`.
- La webview monte `ChatWindow` de `@vanyline/ui` et fournit un `ChatTransport`
  implémenté sur `postMessage` :
  - `sendMessages` → `postMessage({ type: 'chat/send', conversationId, message, agent })`
  - le host relaie en `chat/send` RPC ; les notifications `chat/event` (avec `seq`)
    reviennent en `postMessage({ type: 'chat/event', ... })`
  - la webview reconstruit le `ReadableStream<UIMessageChunk>` via
    `chatEventsToUIStream` de `@vanyline/ui` (partagé avec le frontend web).
- Sélecteur d'agent (`config/agents` RPC via host), liste/reprise de conversation
  (`conversations/list|create|get` RPC), + `QuickPick` natif côté host pour le picker de
  session (repris de kydah-code `openSessionPicker`).

### `contributes` (extrait)

- `viewsContainers.secondarySidebar` : `vanyline`
- `views` : webview `vanyline.chatView`
- `commands` : `vanyline.openPanel`, `.newSession`, `.sessionPicker`, `.openSettings`,
  `.restartServer`
- `menus/view/title` sur `vanyline.chatView` : nouvelle session, picker, paramètres
- `configuration.properties` : `vanyline.serverPath` (string, override), 
  `vanyline.autoUpdateCli` (bool, défaut `true`), `vanyline.defaultLogLevel`

### CI

- `.github/workflows/release.yml` :
  - job `upload-cli` : **ajouter `checksum: sha256`** à
    `taiki-e/upload-rust-binary-action` (prérequis de la vérif d'intégrité).
  - nouveau job `ext` : `npm ci` racine → build `packages/*` → build `ext` (host +
    webview) → `vsce package --no-dependencies` → attache `vanyline-<ver>.vsix` à la
    release.
- `.github/workflows/test.yml` : job `ext` (build + `vitest run`).
- `npm run install:local` racine (repris de kydah-code) : `package` + `code-server
  --install-extension`.
- `docs/ext-install.md` : install manuelle + procédure de test e2e documentée.

## Sécurité (argv / URL / chemin) — cœur de F3

- **Téléchargement + exécution d'un binaire** : contrainte complète ci-dessus (section
  provisioning) — version pinnée bakée, SHA256 obligatoire contre l'asset `.sha256`
  (refus si absent), HTTPS + allowlist d'hôtes sur l'URL **finale** après redirects,
  install atomique, `~/.local/bin` uniquement, jamais exécuter avant vérif.
- `vanyline.serverPath` (override) : exécuté tel quel (l'utilisateur assume) mais
  **auto-update sautée** et binaire utilisé loggué explicitement.
- `spawn` : `shell: false`, argv `['serve', '--stdio']`, `cwd` = workspace folder réel,
  env contrôlé. Jamais de chaîne de commande.
- `workspace` passé à `initialize` = `workspaceFolders[0].uri.fsPath` (chemin local
  réel), pas une entrée arbitraire.

## Risques et questions ouvertes

- **Spike CSP + Vite + stack Vue complète** (Element Plus / Nuxt UI / Tailwind) dans une
  webview — risque n°1. À purger en tâche 1 : « hello ChatWindow » buildé et affiché.
- **Poids du `.vsix`** : si `@vanyline/ui` tire trop (Element Plus + Nuxt UI), basculer
  la webview chat sur l'entrypoint `@vanyline/ui/chat` (exports conditionnels préparés en
  F1). Mesurer en tâche 1.
- **Version attendue CLI vs version extension** : découplées (`cli-version.txt`). Le vrai
  contrat est `PROTOCOL_VERSION` (RPC) — `initialize` échoue proprement sur mismatch de
  protocole, message actionnable (« mettez à jour l'extension / la CLI »).
- **`upload-rust-binary-action` + `checksum: sha256`** : confirmer le nom exact de
  l'asset checksum produit (`vanyline-<target>.tar.gz.sha256` attendu) en tâche CI.
- **code-server vs VS Code desktop** : `os.homedir()`, `context.secrets`, `~/.local/bin`
  — OK sur les deux en Linux. À vérifier sur code-server réel en test e2e.
- **Offline au premier lancement** : pas de binaire, pas de réseau → l'extension doit
  rester utilisable en dégradé (message clair, commande retry), pas planter l'activation.

## Découpage en tâches candidates

1. Scaffold `ext/` + esbuild host + webview Vite « hello ChatWindow » buildé + CSP (le spike, + mesure du poids).
2. `RpcConnection` sur stdio + `initialize`/`shutdown` + OutputChannel + status bar + backoff + `restartServer`.
3. `cli-provisioning.ts` (download + SHA256 + install atomique) + `checksum: sha256` sur le workflow + tests (HTTP mocké).
4. Pont `postMessageChatTransport` + sélecteur d'agent + conversations + `QuickPick`.
5. Packaging : job release `.github`, job test, `install:local`, `docs/ext-install.md`, test e2e.
