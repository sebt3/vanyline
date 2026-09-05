# lsp-agent-interface — interface `lsp_*` orientée boucle agent (2026-09-05)

Feature avec design doc formel (`docs/features/lsp-agent-interface.md`, supprimé à
la clôture — contenu migré dans `docs/architecture.md` § « Serveur LSP » et
§ « WebSocket éditeur »). Partie 1 de l'item backlog « LSP orientée agent +
sélection des tools sandbox » — la partie 2 (sélection des tools sandbox par
toolset) reste au backlog (bloqueur UX non tranché). Implémentée par **Cadence**
(`.opencode/agents/cadence.md`) via `.tasks/`, branche `feat/lsp-agent-interface`,
mergée+poussée dans `main`. Suit [[lsp-integration]] (2026-08-20).

## Ce que ça fait

Remplace les tools `lsp_*` qui imitaient des gestes d'IDE (hover, goto-def brut,
positions 0-based à re-résoudre) par une interface pensée pour la boucle agent :
après une édition, savoir vite ce qui casse et qui est impacté, avec la fonction
englobante + sa signature plutôt que des coordonnées `fichier:ligne:col`. Amène au
passage l'autosave de l'éditeur (absorbe l'item `## Auto-save` du backlog).

- **8 tools** (`lsp_hover` retiré, absorbé par `lsp_definition`) : `lsp_diagnostics`,
  `lsp_definition` (+signature+doc), `lsp_references` (groupé par fichier, symbole
  englobant par réf), `lsp_rename` (param `preview`), `lsp_document_symbols`,
  `lsp_workspace_symbols`, `inspect_symbol` (composition pure def+refs), et le gros
  morceau **`edit_and_check`** (applique une édition puis rend le **diff de
  diagnostics** apparus/disparus/inchangés). 16 tools MCP au total côté sandbox.
- **Modèle de position partagé** `LspSymbolTarget` : `path` + `line` 1-based +
  `symbol` (nom d'identifiant, mode recommandé) ou `character` 1-based en
  échappatoire. `resolve_position` (pure) : 1ʳᵉ occurrence délimitée (`[A-Za-z0-9_]`,
  recherche littérale — jamais de regex depuis l'entrée, R5) ; ambiguïté → 1ʳᵉ, notée.
- **`edit_and_check` cas A/B** (design R1, décision développeur « on ne bloque
  jamais le LLM ») : cas A (aucun éditeur navigateur sur l'URI) → le tool envoie
  `didChange` full-sync versionné ; cas B (éditeur navigateur sur l'URI) → le tool
  n'envoie **jamais** `didChange` (deux émetteurs = désync interdit), il flushe
  l'éditeur (aller-retour `flush-request`/`flush-ack` borné 2 s) puis émet
  `file-changed` sur `/ws/fs`, l'éditeur recharge son buffer et ré-émet son
  `didChange`. Timeout de ré-analyse → état mou `VNL-SBX-LSP-011` (édition faite,
  retry), jamais une erreur ni un « propre » par défaut.
- **Ajout additif au manager LSP** (`sandbox/src/lsp.rs`, reste inchangé) :
  `doc_versions` (version par URI pour les didChange des tools, démarre à 2 — le
  didOpen porte la 1), `next_doc_version`, `invalidate_diagnostics` (vide le cache
  avant ré-analyse), `editor_uris` (URIs tenues par un client navigateur, alimenté
  par le bridge `ws/lsp.rs` sur didOpen/didClose, purgé sur `unsubscribe`).
- **Frames de push `/ws/fs`** (`fs_session` → boucle `tokio::select!`) : pas une
  route nouvelle, un type de frame en plus, diffusé à toutes les sessions `/ws/fs`.
  Les réponses reprennent le champ `id` de la requête (`attach_req_id`, additif).
- **Autosave éditeur** (`frontend/.../editorAutosave.ts`) : extension CodeMirror
  par onglet, `write` debouncé 300 ms vers `/ws/fs`. `Ctrl+S` et le démontage
  d'onglet forcent un flush. Transaction de reload marquée `disk-reload` (l'autosave
  n'écrit jamais une transaction marquée — anti-boucle). `SandboxFsClient` refactoré :
  listener permanent + corrélation par `id` + `onEvent` (remplace le
  `addEventListener` one-shot par requête, qui était empoisonnable).

## Codes d'erreur

Le design doc annonçait 007/008 pour les nouveaux codes — **faux** : 007 (ligne hors
limites), 008 (TextEdit invalide), 009 (confine WorkspaceEdit) étaient déjà pris par
[[lsp-integration]]. Réellement alloués : **`VNL-SBX-LSP-010`** (`resolve_position` :
`symbol` introuvable comme identifiant sur la ligne), **`VNL-SBX-LSP-011`**
(`edit_and_check` : édition appliquée, ré-analyse pas stabilisée avant le timeout).

## Livraison Cadence — trois escalades en session

Comme [[lsp-integration]] et contrairement à WS-10/editing-context-menus : Cadence a
escaladé **avant** implémentation plutôt que laisser découvrir en review —
`path` optionnel de `lsp_workspace_symbols` (indice de toolchain seulement),
englobant sur forme plate (R2 cluster a réfuté l'hypothèse hiérarchique du design :
les deux serveurs rendent `SymbolInformation` plat, `range` = nom seul → englobant =
dernier symbole démarrant à ou avant la réf), canal push `/ws/fs` + flush avant
écriture cas B. `cargo fmt` lancé cette fois. Hors feature pendant la session :
`fix/sandbox-rustup-stable-alias` (alias `stable` image toolchain, bug cluster
bloquant cargo/rust-analyzer).

## Review Phase 3

**1 bloquant** — `LspClient::did_change` portait la version dans un champ frère
`textDocumentVersion` au lieu de `textDocument.version`
(`VersionedTextDocumentIdentifier`). rust-analyzer / typescript-language-server
désérialisent un `version` requis et **droppent silencieusement** une notification
mal formée : en cas A (fichier déjà ouvert par un `lsp_*`/`inspect_symbol`
antérieur — chemin le plus fréquent de la boucle agent, `ensure_open` no-op),
aucune ré-analyse → `edit_and_check` bloqué en `VNL-SBX-LSP-011` à chaque appel. Les
fakes LSP n'échantillonnaient pas la forme (echo brut) → CI verte. **Même motif que
[[miryad-core-integration]]** : tests tous verts, cassé contre un vrai serveur.
Corrigé par Claude directement (accord développeur « corrections puis clôture »),
avec assertion de forme dans le test 17 + `next_doc_version` démarré à 2 (monotonie
serveur après le didOpen v1).

**1 mineur** — `lsp_diagnostics` rendait un chemin **absolu** confiné (préexistant
de [[lsp-integration]]), seul tool à le faire alors que `edit_and_check` /
`render_location` rendent le relatif workspace. Homogénéisé vers le relatif +
assertion.

Le reste (canal push `/ws/fs`, tracking `editor_uris` avec purge sur `unsubscribe`,
refacto `SandboxFsClient`) : solide, bien couvert.

## Vérifs runtime encore dues (pas de cluster en dev)

- Re-probe R2 après redéploiement de l'image toolchain corrigée : `workspace/symbol`
  rust non-vide une fois `cargo metadata` réparé.
- Round-trip cas B complet sur un vrai navigateur (flush→ack→édition→reload buffer +
  didChange éditeur).
- **Après le fix bloquant** : cas A `edit_and_check` contre un vrai rust-analyzer +
  typescript-language-server.
