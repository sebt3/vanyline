# Feature — LSP Vue dans la sandbox

## Ce que la feature fait (une phrase)

Détecte les projets Vue et fournit un LSP `.vue` (diagnostics, complétion, hover,
goto-definition, rename) à l'éditeur web et aux tools `lsp_*`, via la toolchain
`node` — Volar v3 en mode hybride, multiplexé côté sandbox.

## Ce qu'elle ne fait pas (périmètre explicite)

- Pas de sélection de version Volar par projet, pas de config Volar au-delà du preset
  controller.
- Pas de support Vue 2 (Volar v3 = Vue 3 par défaut).
- Pas de fusion détection dérivée × `spec.toolchains` explicite (comportement actuel
  inchangé — si l'utilisateur fixe `toolchains`, il pose lui-même `toolchain.lsp`).
- Pas de changement de signature des tools `lsp_*`.
- Pas de nouvelle capacité / auth pour `app` ou le controller.

## Décision : Volar v3 (multiplexeur), pas v1.8 takeover

Tranché par le développeur le 2026-09-05, en connaissance du coût (cf. SPIKE
ci-dessous). Recommandation Claude était v1.8 takeover pour la v1 ; **écartée** — on
prend v3 et son multiplexeur dès la première version. Compromis accepté :
`semanticTokens` sur `.vue` sera dégradé ou délégué par plage (risque n° 1), et
l'ossature multiplexeur est ~3-4 tâches.

---

## SPIKE — état réel de Volar v3 (fait 2026-09-05)

Sources : nvim-lspconfig `lsp/vue_ls.lua`, discussions vuejs/language-tools
#5456 / #5931 / #3789, PR #5252 / #5248.

### Ce que v3 impose

- **Takeover mode supprimé en v3.0** (PR #5248). Plus aucun mode où un seul
  `vue-language-server` gère `.vue` + `.ts` + `.js`.
- **Mode hybride obligatoire, à deux serveurs** :
  - `vue-language-server --stdio` — attaché aux `.vue` **uniquement**, gère
    template / style / spécifique-Vue.
  - `typescript-language-server` avec `@vue/typescript-plugin` chargé
    (`initializationOptions.plugins = [{ name: "@vue/typescript-plugin", location:
    <dir du paquet @vue/language-server> }]`) — attaché aux `.vue` **et** aux
    `.ts`/`.js`, fournit **toute** l'intelligence des blocs `<script>`
    (complétion / diagnostics / hover / definition).
- **Le client fait la coordination**, deux volets :
  1. **Forwarding** : `vue-language-server` émet des notifications `tsserver/request`
     (`params = [[id, command, payload]]`, commandes préfixées `_vue:` —
     `_vue:projectInfo`, `_vue:quickinfo`, `_vue:documentHighlights-full`,
     `_vue:encodedSemanticClassifications-full`, `_vue:collectExtractProps`, …). Le
     client les exécute contre le serveur TS via
     `workspace/executeCommand "typescript.tsserverRequest"` (`arguments =
     [command, payload]`), puis renvoie `tsserver/response`
     (`params = [[id, response.body]]`). `typescript-language-server` **supporte**
     `typescript.tsserverRequest` (vérifié README, `executeCommandProvider`).
  2. **Merge multi-serveur** : dans nvim/vscode, les deux serveurs sont attachés au
     même buffer et l'éditeur fusionne nativement leurs réponses. `.vue` reçoit ses
     diagnostics `<script>` de `typescript-language-server`, pas de
     `vue-language-server`.

### Pourquoi c'est un multiplexeur côté vanyline

L'éditeur navigateur (`@codemirror/lsp-client` via `/ws/lsp/:toolchain`) et le client
MCP (`lsp_client.rs`) consomment chacun **un seul flux LSP** par toolchain. Pas de
merge multi-serveur natif. Donc **la session LSP de la sandbox devient un vrai
multiplexeur LSP** :

- lancer `vue-language-server` + `typescript-language-server(+plugin)` comme deux
  enfants ;
- fan-out doc-sync (`didOpen`/`didChange`/`didClose`/`didSave`) aux deux ;
- **fusionner par méthode** les réponses aux requêtes du client :
  `publishDiagnostics` (concat par URI), `completion`/`completionItem/resolve`
  (merge de listes + routage du `resolve` vers le serveur d'origine → suivi de
  provenance), `hover` (concat contents), `definition`/`references`/`implementation`
  (concat locations), `rename`/`prepareRename` (merge `WorkspaceEdit`), `codeAction`
  (concat), `formatting` / `documentSymbol` (`vue-language-server`) ;
- **`semanticTokens` : non fusionnable** — déléguer par plage (script → TS,
  template → Vue) ou accepter dégradé. **Risque ouvert n° 1.**
- traiter le forwarding `tsserver/request` → `typescript.tsserverRequest` →
  `tsserver/response` en parallèle du merge.

### Écarté : `@vue/language-server` v1.8 (takeover)

Un seul process gérant tout `.vue`+`.ts`+`.js` — changement de session quasi nul, pas
de `semanticTokens` à résoudre. Écarté : Volar figé ~2023, TS embarqué ~5.3, pas de
Vue 3.4+/vapor tooling. v2.x n'aide pas (déjà deux serveurs + couche « named pipes »
retirée en v3).

---

## Interfaces clés et modules touchés

### Détection — `sandbox/src/maint.rs`
- `detect_languages` : marqueur `vue` = présence d'un `*.vue` dans l'arbre HEAD.
- Ordre de sortie figé étendu (avec `docker-lsp` :
  `["rust","js-ts","vue","dockerfile"]`, filtré).
- `Project.status.languages` : valeur documentée dans `crds/src/lib.rs`. Pas de
  changement de struct. RBAC inchangé (merge patch `status.languages` existant).
- **Le marqueur `vue` implique la toolchain `node`** indépendamment de `js-ts`.

### Controller — `controller/src/sandbox.rs`, `controller/src/main.rs`
- `effective_toolchains` : `"vue"` détecté ⟹ toolchain `node` présente + marquée
  « variante Volar ».
- `resolve_toolchain_lsp` : `node` variante Volar → **spec composite** — primaire
  `vue-language-server --stdio` (+ `--tsdk` si requis) + aux
  `{ role: "tsserver-forward", bin: ".../typescript-language-server",
  args: ["--stdio"], initOptions: { plugins: [{ name: "@vue/typescript-plugin",
  location: "/toolchains/node-lsp/.../node_modules/@vue/language-server" }] } }` —
  sérialisée dans `VNL_LSP_TOOLCHAINS` (champ `aux[]` additif ; absent → session
  mono-process, comportement actuel strict).
- `node` **sans** Vue → inchangé (`typescript-language-server --stdio`, pas d'aux).
- `SandboxPodContext` : pas de nouveau flag (réutilise `lsp_image_node` — l'image
  node LSP embarque désormais Volar + le plugin).

### Format `VNL_LSP_TOOLCHAINS` (env sandbox) — champ additif
```json
{ "name": "node", "bin": "...vue-language-server", "args": ["--stdio"],
  "aux": [ { "role": "tsserver-forward",
             "bin": "...typescript-language-server", "args": ["--stdio"],
             "initOptions": { "plugins": [ { "name": "@vue/typescript-plugin",
                                            "location": "..." } ] } } ] }
```
Rôles `aux` : `tsserver-forward` (ici) et `diagnostics-merge` (feature `docker-lsp`).

### Image — `toolchains/node/Dockerfile`
- `npm install -g @vue/language-server@~3 @vue/typescript-plugin@~3` (en plus de
  `typescript-language-server typescript` déjà présents).
- Publiée avec le même tag que app/sandbox/controller.

### Sandbox — session LSP multiplexeur — `sandbox/src/lsp.rs`
- **API publique de `LspSession` inchangée** (`subscribe`/`send`/`cached_diagnostics`/
  `wait_for_diagnostics`/`is_alive`). Bridge navigateur (`ws/lsp.rs`) et client MCP
  (`lsp_client.rs`) **inchangés**.
- Multiplexeur interne : cf. SPIKE. Découpage prévisionnel :
  1. ossature multi-process : spawn N enfants, fan-out doc-sync, cycle de vie
     (primaire mort → session morte ; aux mort → dégradé signalé pour
     `tsserver-forward`) ;
  2. remapping d'ID par enfant + corrélation des réponses ;
  3. fusion par méthode (requêtes) + fusion `publishDiagnostics` par URI **en amont
     du cache** (navigateur et MCP voient le même résultat — contrainte, pas option) ;
  4. handler `tsserver/request` → `executeCommand typescript.tsserverRequest` sur
     l'aux → `tsserver/response` au primaire ;
  5. stratégie `semanticTokens` (risque n° 1).
- Réutiliser les patrons `Notify` existants (`initialize_notify`, `diagnostics_notify`).

### Sandbox — mapping — `sandbox/src/tools_impl.rs`
- `toolchain_for_path` : `.vue` → `("node", "vue")`.
- Miroir frontend `frontend/src/components/panels/editorLanguage.ts` :
  `lspToolchainForPath` → `.vue` → `{ toolchain: "node", languageId: "vue" }`.

## Contrainte de validation / échappement

- `VNL_LSP_TOOLCHAINS` (dont `aux` / `initOptions`) : valeurs = constantes
  construites par le controller (chemins d'image, bins, args, `location` du plugin).
  **Aucun champ de CRD, aucune entrée utilisateur interpolée.** Spawn en argv, jamais
  de shell.
- `--tsdk` (si requis par `vue-language-server`) : chemin constant dans l'image
  (`/toolchains/node-lsp/.../node_modules/typescript/lib`).
- Forwarding `tsserver/request` : le `command` et le `payload` viennent de
  `vue-language-server` (process de confiance, pas l'utilisateur) et sont passés tels
  quels à `executeCommand` — pas de construction de commande shell, pas
  d'interpolation. Les URIs contenues sont réécrites par `rewrite_uris` du bridge
  comme tout autre message.
- Chemin / URI navigateur → LSP : inchangé (confinement R5 + `rewrite_uris`). `.vue`
  ne crée aucun nouveau chemin d'entrée.
- `toolchain` / `languageId` : littéraux dérivés de l'extension.

## Risques identifiés

1. **`semanticTokens` non fusionnable** — deux jeux de tokens sur le même `.vue`.
   Bloquant pour une coloration sémantique correcte. À résoudre par délégation de
   plage (script → réponse TS, template/style → réponse Vue) ou à assumer dégradé
   (coloration lexicale CodeMirror seule sur les `.vue`, pas de sémantique LSP).
   **Décider en début de la tâche multiplexeur, pas en cours.**
2. **Contexte projet (`rootUri`)** — Volar ET le serveur TS doivent trouver
   `package.json`/`tsconfig.json`/`node_modules` : même piège que documenté dans
   `editorLanguage.ts` (repli sur le cwd de spawn sinon). `root_markers = package.json`.
3. **`@vue/typescript-plugin` `location`** — doit pointer le répertoire réel du paquet
   `@vue/language-server` dans l'image (**pas** `@vue/typescript-plugin` lui-même —
   cf. config nvim de référence). À vérifier sur l'image construite.
4. **Corrélation des réponses multi-enfants** — le remapping d'ID existant de
   `LspSession` est conçu pour 1 process ↔ N abonnés. L'étendre à N process ↔ N
   abonnés sans casser le chemin mono-process actuel (rust, node-sans-vue, docker
   primaire) — la régression ici casserait *tout* le LSP, pas juste Vue.
5. **`edit_and_check` / `lsp_diagnostics`** — lisent `cached_diagnostics` /
   `wait_for_diagnostics`. Le merge doit précéder l'écriture du cache.
6. **Dépendance croisée avec `docker-lsp`** — `vue-lsp` construit l'ossature
   multi-process (rôle `tsserver-forward`) ; `docker-lsp` ajoute ensuite le rôle
   `diagnostics-merge` (plus simple). **`vue-lsp` avant `docker-lsp`.**
7. **Régression du chemin mono-process** — toute la famille `lsp_*` (`lsp-integration`,
   `lsp-agent-interface`) repose sur `LspSession`. Tests de non-régression rust +
   node-sans-vue obligatoires à chaque tâche du multiplexeur.

## Questions ouvertes

- **Stratégie `semanticTokens`** (délégation par plage vs dégradé) — trancher au
  début de la tâche multiplexeur.
- **`LspSpec` CRD gagne `aux`** (LSP composite custom possible, hors presets) **vs
  composites preset-only** — penchant : preset-only, garde la CRD petite. Question
  partagée avec `docker-lsp`, trancher une fois pour les deux, avant la tâche
  controller.
- **Merge `completion`** — dédup des items entre Vue et TS (par `label` + `kind` ?),
  et routage du `completionItem/resolve` — à préciser au moment de la tâche fusion.
- **Pin exact** `@vue/language-server` / `@vue/typescript-plugin` : `~3` (mineur
  flottant) vs `3.x.y` figé — penchant : figer au patch, vu le churn.

## Migration `docs/architecture.md` (Phase 3)

- § « Serveur LSP » : variante Volar de la toolchain `node`, session multiplexeur
  (rôles primaire / aux, forwarding `tsserver/request`, fusion par méthode).
- § « Détection de langages » : marqueur `vue`, implication toolchain `node`.
- Tableau mapping extension → toolchain/languageId : `.vue` → `node`/`vue`.
- § stack : `@vue/language-server` + `@vue/typescript-plugin` dans l'image node.
