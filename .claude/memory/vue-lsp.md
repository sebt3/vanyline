# Feature — LSP Vue dans la sandbox (multiplexeur composite Volar v3)

**Close le 2026-09-07. Branche `feat/vue-lsp` mergée dans `main` et poussée,
branche supprimée. Design doc `docs/features/vue-lsp.md` supprimé, contenu
migré dans `docs/architecture.md` (§ "Serveur LSP" — sous-bloc "Multiplexeur LSP
composite" ; § "Détection de langages" ; § "LSP par toolchain" — sous-bloc
"Variante composite Volar").**

## Ce que ça fait

LSP `.vue` réel (diagnostics / complétion / hover / goto-def / rename) pour
l'éditeur web et les tools `lsp_*`, via la toolchain `node` — **Volar v3 mode
hybride**, deux serveurs (`vue-language-server` primaire + `typescript-language-server`
+ `@vue/typescript-plugin` en aux `tsserver-forward`), **multiplexés côté sandbox**
dans `LspSession` sans changer son API publique ni le bridge navigateur ni le
client MCP.

Décision développeur (2026-09-05) : v3 multiplexeur, **pas** v1.8 takeover
(recommandation Claude initiale) — takeover mode supprimé en Volar v3.0, v1.8 =
Volar figé ~2023 / TS ~5.3 / pas de Vue 3.4+.

## Architecture livrée (détail dans `docs/architecture.md`)

- **Détection** : marqueur `vue` = un `*.vue` n'importe où dans l'arbre HEAD.
  Ordre figé étendu `["rust","js-ts","vue","dockerfile"]`. `vue` ⟹ toolchain
  `node` indépendamment de `js-ts` (un seul `node` si les deux).
- **Controller** : `node_lsp_composite` — preset composite **uniquement** sur le
  `node` DÉRIVÉ (`spec.toolchains` vide) d'un projet `vue`. `spec.toolchains`
  explicite ou `toolchain.lsp` custom ⟹ jamais d'`aux` (preset-only, décision
  2026-09-06 — la CRD `LspSpec` n'expose pas `aux`). Clé `aux` omise hors
  composite : bytes `VNL_LSP_TOOLCHAINS` inchangés pour les pods non-Vue.
- **`VNL_LSP_TOOLCHAINS`** : champ additif `aux: [{role, bin, args, initOptions}]`.
  Rôles : `tsserver-forward` (ici), `diagnostics-merge` (réservé `docker-lsp`).
  Rôle inconnu au spawn ⟹ warn + enfant ignoré, jamais une erreur de session.
- **Multiplexeur `LspSession`** : primaire garde son espace d'ids historique ;
  aux = canaux stdin dédiés + ids internes jamais routés client. `aux` vide ⟹
  chemin mono-process **strictement** inchangé (invariant de non-régression —
  tout le LSP déployé en dépend).
  - doc-sync + `initialize` : fan-out à chaque aux vivant (`try_send`, aux
    mort/saturé ignoré).
  - requêtes : `hover`/`definition`/`references`/`codeAction`/`rename`/
    `completion` → barrière `PendingMerge` fusionnée primary-first ; tout le
    reste (`formatting`, `documentSymbol`, `signatureHelp`, `semanticTokens/*`)
    → primaire seul. `semanticTokens` primaire par cas EXPLICITE (risque n° 1 —
    délégation de plage portée par Volar via `tsserver/request`).
  - `completionItem/resolve` : 3ᵉ voie par cache de provenance `(label,kind) →
    enfant`, borné 64 URIs, purgé au didChange/didClose. Ambiguë/absente ⟹
    fallback no-op.
  - `publishDiagnostics` : fondu par parts (`diag_parts[uri][child_idx]`,
    remplacement) concat primary-first écrit dans `diagnostics_cache` AVANT
    broadcast (navigateur == MCP). EOF aux ⟹ parts purgées + cache recompté.
  - `tsserver/request` (Volar hybride) : intercepté, exécuté contre l'aux via
    `workspace/executeCommand typescript.tsserverRequest`, `result.body`
    renvoyé en `tsserver/response`. Aucun aux vivant ⟹ body `null` immédiat.
  - cycle de vie : mort primaire = mort session (kill aux) ; mort aux =
    dégradé (`is_alive()` = primaire seul).
- **Image** `toolchains/node/Dockerfile` : `@vue/language-server@3.3.11` +
  `@vue/typescript-plugin@3.3.11` épinglés au patch. `--install-links`
  **obligatoire** sur `@vue/language-server` (sinon npm hoiste le plugin
  top-level, `<location>/node_modules/@vue/typescript-plugin` niché n'existe
  plus). Même image que `ctx.lsp_image_node`, montée `/toolchains/node-lsp`.
- **Mapping** `.vue` → `("node","vue")` : `tools_impl.rs::toolchain_for_path` +
  miroir `editorLanguage.ts::lspToolchainForPath`.

## Delivery

Livré par **Cadence** (`cadence` + `implement`, DeepSeek-V4-Flash), mode feature
`.tasks/` — 11 tâches (`task-04` absente, numérotation). fmt lancé (6ᵉ feature
déléguée, 2ᵉ consécutive où c'est fait d'emblée).

Écarts cadence arbitrés en cours de cadence, tous confirmés en review :
- **task-02** : hoisting npm cassait le nesting `@vue/typescript-plugin` →
  `--install-links` (le design ne l'anticipait pas).
- **task-07** : la sonde `composite_other_request_primary_only` utilisait
  `hover`, qui est passé `MergeRoute::All` → méthode changée en `signatureHelp`
  (reste `Primary`). Correct.
- **task-10** : `resolve_toolchain_lsp` gardée intacte, `node_lsp_composite`
  appelée par `build_sandbox_pod`. Propre.
- **task-11** : doc-comment frontend réécrit avec liste explicite d'extensions.

## Review Phase 3

**1 bug bloquant** — `node_lsp_composite` émettait
`location: /toolchains/node-lsp/lib/node_modules/@vue/language-server`, **`usr/local/`
manquant** (le paquet est en `/usr/local/lib/node_modules/...`, cohérent avec les
`bin` frères `/toolchains/node-lsp/usr/local/bin/...` et avec le doc-comment de la
fonction lui-même). Impact : `@vue/typescript-plugin` ne charge pas → tsserver
aveugle au `.vue` → **toute l'intelligence `<script>` morte**. CI verte (test
d'égalité de chaîne verrouillait le mauvais chemin).
**Origine** : le mauvais chemin était déjà dans `.tasks/vue-lsp/task-10` et
`task-02` — écrit par Claude dans le fichier de tâche, suivi fidèlement par
Cadence, verrouillé par un test. **Même classe que F5** (`treeView.refresh()`
halluciné dans `task-01`) : le bug vient de la spec, pas du modèle. Motif
récurrent CI-verte-mais-cassé-au-runtime (cf. [[miryad-core-integration]],
[[lsp-agent-interface]]). Corrigé par Claude directement (constante + 2 tests +
commentaire Dockerfile).

**1 mineur** — commentaire `tools_impl.rs` « le ts-ls enfant ignore cette
notification sans effet de bord » : contredit le modèle hybride. Réécrit :
l'intelligence `<script>` vient de tsserver via le canal `tsserver/request` que
le primaire relaie à l'aux.

**Reportés passe cluster** (pas de backend en dev) :
- `tsdk` du primaire Volar : le design le laissait conditionnel (« si requis »),
  Cadence l'a retiré. **Aucun mécanisme pour injecter des `initializationOptions`
  dans le primaire** (seul `LspAux` a `init_options`). Si `vue-language-server`
  3.3.11 en a besoin et ne l'autorésout pas depuis le cwd du projet → template
  type-check cassé. Fix éventuel : `--tsdk <path>` dans `spec.args`.
- Capabilities de l'`initialize` : le navigateur ne voit que celles de Volar
  (routage `Primary`), jamais l'union Volar+tsserver. Si Volar v3 annonce un jeu
  réduit en hybride, le client n'émet pas certaines requêtes.
- Round-trip `.vue` réel (complétion + diagnostics `<script>`) contre
  `vue-language-server` 3.3.11 + `typescript-language-server` dans l'image
  construite. Image node à rebuild+republier au prochain tag.

## Suite

`docker-lsp` (rôle `aux` `diagnostics-merge`, plus simple — l'ossature
multi-process est là). `vue-lsp` avant `docker-lsp` : fait.
