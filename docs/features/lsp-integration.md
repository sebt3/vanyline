# Feature — lsp-integration

## Ce que la feature fait

Intègre un serveur LSP (rust-analyzer, typescript-language-server, ...) par toolchain
dans la sandbox — un process LSP **unique et partagé**, exposé à la fois à l'éditeur web
(CodeMirror 6, via `@codemirror/lsp-client`) et aux LLM (nouveaux tools MCP `lsp_*`) —
pour amener hints/diagnostics/intellisense/goto-definition/rename côté UI, et une
compréhension structurelle du code côté LLM.

## Ce qu'elle ne fait pas

- Ne remplace pas les tools MCP existants (filesystem/search/command) — les `lsp_*`
  sont additifs.
- Pas de LSP par session/onglet : un seul process par toolchain par sandbox, partagé
  entre tous les clients (éditeur navigateur, tools LLM).
- Pas de câblage kydah-code ici — ce client ne consomme toujours pas la sandbox (cf.
  `.claude/MEMORY.md`, section "reste ouvert"), hors périmètre de cette feature.
- Pas de multi-langage day one au-delà des toolchains déjà supportées (rust, ts/js) —
  python/autres laissés pour plus tard.
- Pas de garde-fou quota CPU/RAM dédié au LSP — le coût est accepté comme référence
  dans cette feature, pas optimisé (cf. risques).

## Décision d'architecture — process partagé

Un seul process LSP par toolchain vit dans le pod sandbox, démarré et géré par le
serveur sandbox lui-même — même pattern que `/ws/terminal` (bridge subprocess ⇄ WS via
`portable-pty`), ici en stdio JSON-RPC avec framing `Content-Length` au lieu d'un pty.

Toutes les sessions qui consultent ce process voient le même état : le LSP dispatch sur
le code réel de la sandbox, valide indépendamment de qui regarde. Pas de réindexation
par session, pas de divergence de diagnostics entre l'éditeur et le LLM.

Deux surfaces consomment ce process unique :

- **Navigateur** — route WS `/ws/lsp/:toolchain` sur le serveur axum existant, même
  ticket d'auth court-vécu à usage unique que `/ws/fs`/`/ws/terminal`. Consommée par
  `@codemirror/lsp-client` (package officiel CodeMirror, transport JSON pluggable).
- **LLM** — tools MCP `lsp_diagnostics`, `lsp_definition`, `lsp_references`,
  `lsp_hover`, `lsp_rename`, exposés **en plus** des tools filesystem/search/command
  existants (`sandbox/src/tools_impl.rs`).

Le serveur sandbox devient propriétaire du cycle de vie LSP (start/restart/
multiplexage multi-clients), pas juste un tuyau — c'est la pièce neuve principale de
cette feature.

## Interfaces clés et modules touchés

### Forme de `Toolchain.lsp` (décidé)

```rust
// crds/src/lib.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSpec {
    pub image: String,
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub struct Toolchain {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub lsp: Option<LspSpec>,
}
```

Résolution (`controller/src/sandbox.rs`, `resolve_toolchain_lsp`, même forme que
`resolve_toolchain_env`) :
1. `toolchain.lsp` si `Some` → utilisé tel quel (LSP custom possible, y compris pour
   une toolchain hors rust/node).
2. Sinon preset par `toolchain.name` : `image` depuis `ctx.lsp_image_rust`/
   `ctx.lsp_image_node` (nouveaux champs `Context`/CLI, env `LSP_IMAGE_RUST`/
   `LSP_IMAGE_NODE` — même mécanisme que `TOOLCHAIN_IMAGE_RUST`/`NODE`, l'image reste
   configurable au déploiement, donc pas hardcodée) ; `bin`/`args` hardcodés
   (`rust-analyzer` sans args ; `typescript-language-server --stdio`).
3. Sinon `None` → pas de route `/ws/lsp` montée pour cette toolchain (repli dégradé,
   déjà décrit plus bas).

S'applique uniformément que `spec.toolchains` soit explicite ou dérivé de
`project.status.languages` — zero-config pour rust/node dans les deux cas, sans fermer
la porte à un LSP custom. **Pourquoi cette forme plutôt qu'un champ minimal
`{bin, args}` avec image purement en preset** : `Toolchain.image` lui-même n'a jamais
de fallback preset invisible dans le CRD existant (toujours un champ explicite,
rempli côté controller uniquement dans le chemin de dérivation) — un `LspSpec` sans
`image` casserait ce précédent. Le comportement de fallback name-keyed, lui, suit le
précédent de `Toolchain.env`/`toolchain_preset` (appliqué uniformément, explicite ou
dérivé), pas celui de `image`.

Montage : volume image séparé par toolchain, `/toolchains/<name>-lsp` (à côté de
`/toolchains/<name>` du toolchain lui-même). Découverte côté sandbox : un seul env
JSON `VNL_LSP_TOOLCHAINS` (`[{name, bin, args}]`) injecté par le controller — pas
d'interpolation shell, argv array au spawn (même règle que la maintenance des
projets, cf. AGENTS.md).

- `sandbox/src/` — nouveau module de gestion du process LSP (spawn, framing
  `Content-Length`, multiplexage multi-clients) ; nouvelle route WS
  `/ws/lsp/:toolchain` ; nouveaux handlers `lsp_*` dans `tools_impl.rs`/`mcp.rs`.
- `frontend/src/components/panels/Editor.vue` + `editorLanguage.ts` — branchement
  `@codemirror/lsp-client`, rendu diagnostics/hover/intellisense.
- `frontend/src/components/ContextMenu.vue` — actions go-to-definition/rename dans le
  menu contextuel éditeur (étend la feature `editing-context-menus` déjà livrée).
- Écriture multi-fichiers pour `workspace/applyEdit` (rename) — à vérifier/étendre côté
  `/ws/fs` (WS-11) si l'écriture groupée n'existe pas déjà.

## Usages LLM (tools `lsp_*`, additifs)

- `lsp_diagnostics(path)` — structuré (fichier/ligne/sévérité/message), plus rapide
  qu'un `cargo check` complet parsé en texte pour un contrôle ponctuel. Utile
  directement au mode `diagnose` du workflow projet.
- `lsp_definition`/`lsp_references` — résolution exacte de symbole au lieu de
  grep-devinette. Gain net pour les agents Qwen (`implement`/`diagnose`) sur des
  refactors.
- `lsp_rename` — rename atomique vérifié par le LSP, au lieu d'un sed multi-fichiers
  risqué.
- `lsp_hover` — signature/type pour lever l'ambiguïté sur un identifiant avant édition.

## Risques et questions ouvertes

- **Coût ressources** : rust-analyzer fait passer un pod sandbox de ~7 Mo à
  potentiellement ~1 Go de RAM, CPU non négligeable à l'indexation. Accepté comme
  référence — comparable à code-server + extension rust-analyzer aujourd'hui.
  Différence structurelle assumée : côté code-server le LSP est enfermé dans
  l'extension, invisible à tout autre consommateur (harness LLM compris) ; ici il
  devient une capacité de première classe partagée éditeur+LLM. Pas de garde-fou
  quota/limite posé dans cette feature — à surveiller en usage réel avant d'ajouter des
  `resources.limits` dédiés si besoin.
- **Multiplexage** : plusieurs `didOpen` sur une session LSP unique pour plusieurs
  clients (onglets navigateur + tools MCP) — plomberie neuve côté sandbox, absente de
  `portable-pty`.
- **Rename multi-fichiers** : `workspace/applyEdit` peut toucher des fichiers non
  ouverts dans un onglet — à confirmer que `/ws/fs` supporte une écriture atomique
  groupée avant de le proposer côté UI/tool.
- **Maturité `@codemirror/lsp-client`** : package officiel mais tout jeune (v6.1.0,
  quelques jours au moment de l'écriture). À valider en usage réel ; repli possible sur
  `codemirror-languageserver` (communautaire, marimo-team) en cas de blocage.
- **Toolchain sans LSP connu / LSP absent de l'image** : comportement de repli si
  `toolchain.lsp` est vide — pas de route `/ws/lsp` montée, éditeur en mode dégradé
  (coloration seule, comportement actuel inchangé).

## Découpage en tâches candidates

(Indicatif — affiné au fil de l'implémentation, une tâche à la fois par tâche Qwen.)

1. `lsp-process-bridge` — module sandbox : spawn LSP, framing `Content-Length`, route
   `/ws/lsp/:toolchain`, tests sur un process LSP factice.
2. `lsp-toolchain-mount` — `LspSpec`/`Toolchain.lsp` dans `crds/src/lib.rs`,
   `resolve_toolchain_lsp` + montage volume `/toolchains/<name>-lsp` + env
   `VNL_LSP_TOOLCHAINS` côté `controller/src/sandbox.rs`, flags CLI
   `LSP_IMAGE_RUST`/`LSP_IMAGE_NODE` dans `controller/src/main.rs`.
3. `lsp-mcp-tools` — `lsp_diagnostics`/`lsp_definition`/`lsp_references`/`lsp_hover`
   côté MCP, consommant le process partagé.
4. `lsp-editor-client` — branchement `@codemirror/lsp-client` dans Editor.vue,
   diagnostics + hover + goto-definition.
5. `lsp-rename` — `lsp_rename` (tool + UI), écriture multi-fichiers atomique si
   `/ws/fs` ne le supporte pas déjà.
6. `lsp-context-menu` — actions go-to-definition/rename dans ContextMenu.vue.
