# Feature — LSP Dockerfile + hadolint dans la sandbox

## Ce que la feature fait (une phrase)

Détecte la présence de `Dockerfile` dans un projet et fournit une toolchain `docker`
dédiée avec `dockerfile-language-server` (complétion / hover / validation de base) et
`hadolint` (lint) exposés comme diagnostics dans l'éditeur web et les tools `lsp_*`.

## Ce qu'elle ne fait pas (périmètre explicite)

- Ne dépend pas de la toolchain / de l'image `node` — image `docker` autonome.
- Pas de fusion détection dérivée × `spec.toolchains` explicite (comportement actuel).
- Pas d'extension des features natives de `dockerfile-language-server` (résolution
  d'images de base distantes, etc.).
- Pas de tool `validate` (item backlog séparé). `hadolint` sur le `PATH` de la
  toolchain est disponible pour ce tool quand il arrivera — rien construit ici pour lui.
- Pas de changement de signature des tools `lsp_*`.
- Pas de nouvelle capacité / auth pour `app` ou le controller.

## Décision développeur : hadolint = A + B + C combinés

- **A** — binaire `hadolint` sur le `PATH` de la toolchain `docker` (terminal, futur
  `validate`).
- **B** — un petit serveur LSP wrapper (`vnl-hadolint-lsp`) qui émet des
  `textDocument/publishDiagnostics`.
- **C** — le wrapper relit au `didSave` et sur `didChange` debouncé (~500 ms) ; ses
  diagnostics sont **fusionnés par URI** avec ceux de `dockerfile-language-server` par
  la session, **en amont du cache de diagnostics** (navigateur ET tools MCP voient le
  même résultat).

## Interfaces clés et modules touchés

### Détection — `sandbox/src/maint.rs`
- `detect_languages` : marqueur `dockerfile` = présence de `Dockerfile`,
  `Dockerfile.*`, `*.dockerfile`, `Containerfile` dans l'arbre HEAD.
- Ordre de sortie figé étendu : `["rust","js-ts","vue","dockerfile"]` (filtré).
- `Project.status.languages` : valeur documentée dans `crds/src/lib.rs`. Pas de
  changement de struct. RBAC inchangé (merge patch `status.languages` existant).

### Controller — `controller/src/sandbox.rs`, `controller/src/main.rs`
- `effective_toolchains` : `"dockerfile"` détecté ⟹ ajoute une toolchain `docker`
  (`ctx.toolchain_image_docker`). Ordre figé : `rust`, `node`, `docker`.
- `resolve_toolchain_lsp` : `docker` → **spec composite** —
  primaire `docker-langserver --stdio`, aux
  `{ role: "diagnostics-merge", bin: "/toolchains/docker-lsp/.../vnl-hadolint-lsp",
  args: [] }` — sérialisée dans `VNL_LSP_TOOLCHAINS` (champ `aux[]` additif ;
  absent → session mono-process, comportement actuel strict).
- `toolchain_preset` : la toolchain `docker` n'a pas besoin d'env particulier
  (binaires dans `/toolchains/docker/usr/local/bin`, ajouté au `PATH` via le preset
  `PATH` standard `{root}/usr/local/bin`).
- `SandboxPodContext` + flags CLI : `TOOLCHAIN_IMAGE_DOCKER`, `LSP_IMAGE_DOCKER`
  (défaut = même image, LSP baké — même patron que rust/node).
- **`LspSpec` (CRD) reste `{ image, bin, args }`** — le composite est un concept
  interne controller. *(Question ouverte : partagée avec `vue-lsp`.)*

### Format `VNL_LSP_TOOLCHAINS` (env sandbox) — champ additif
```json
{ "name": "docker", "bin": "...docker-langserver", "args": ["--stdio"],
  "aux": [ { "role": "diagnostics-merge", "bin": "...vnl-hadolint-lsp", "args": [] } ] }
```

### Sandbox — session LSP composite — `sandbox/src/lsp.rs`
- **API publique de `LspSession` inchangée.** Bridge navigateur (`ws/lsp.rs`) et
  client MCP (`lsp_client.rs`) inchangés.
- Cas `diagnostics-merge` (le plus simple des rôles composites) :
  - lancer primaire + aux comme deux enfants ;
  - fan-out doc-sync (`didOpen`/`didChange`/`didClose`/`didSave`) aux deux ;
  - **requêtes du client → primaire seul** (l'aux ne répond à aucune requête) ;
  - `publishDiagnostics` : concat par URI (primaire + aux) **avant** écriture du cache
    et broadcast ; chaque diagnostic garde sa `source` (`"hadolint"` / `"dockerfile"`).
  - cycle de vie : primaire mort → session morte (kill aux) ; aux mort → dégradation
    silencieuse (le primaire sert encore).
- Réutiliser les patrons `Notify` existants (`diagnostics_notify`).
- **`vue-lsp` (Volar v3) introduit l'ossature multi-process en premier ; cette feature
  ne fait qu'ajouter le rôle `diagnostics-merge`** (le plus simple : l'aux ne répond à
  aucune requête, juste fan-out doc-sync + concat des `publishDiagnostics`).
  `vue-lsp` **avant** `docker-lsp`.

### Wrapper hadolint — `sandbox/src/bin/hadolint_lsp.rs` (nouveau)
- Petit serveur LSP sur stdio (réutilise le framing `Content-Length` de `lsp.rs`).
- `initialize` → capacités minimales (text sync incremental ou full, diagnostics).
- Sur `didSave` / `didChange` debouncé (~500 ms) : lance
  `hadolint --format json --no-color -` avec **le contenu du buffer sur stdin**
  (jamais le chemin en argv, jamais via un shell).
- Convertit la sortie JSON hadolint :
  - `line` / `column` 1-based → LSP `range` 0-based (fin de range = fin de ligne ou
    heuristique 1 token) ;
  - `level` (`error` / `warning` / `info` / `style`) → LSP severity (1 / 2 / 3 / 4) ;
  - `code` (ex. `DL3008`) → `Diagnostic.code`, `source = "hadolint"`.
- Émet `textDocument/publishDiagnostics` par URI ouverte.
- Baké dans l'image toolchain `docker` (copie multi-stage depuis le build sandbox).

### Sandbox — mapping — `sandbox/src/tools_impl.rs`
- `toolchain_for_path` : `Dockerfile` / `Dockerfile.*` / `*.dockerfile` /
  `Containerfile` (nom de base, pas extension) → `("docker", "dockerfile")`.
- Miroir frontend `editorLanguage.ts::lspToolchainForPath` : mêmes règles de nom →
  `{ toolchain: "docker", languageId: "dockerfile" }`. **Attention** : aujourd'hui
  `lspToolchainForPath` fait `path.split('.').pop()` — ajouter le test de nom de base
  (le helper `dockerfileName()` de la feature `editor-syntax-highlighting` s'il est
  déjà là, sinon dupliqué proprement puis mutualisé).

### Images
- `toolchains/docker/Dockerfile` (nouveau) : base node fraîche (`node:trixie-slim`) +
  `npm install -g dockerfile-language-server-nodejs` + binaire `hadolint` (release
  statique GitHub, checksum vérifié au build) + `vnl-hadolint-lsp`. **Ne monte pas**
  le volume toolchain-node.
- `.github/workflows/release.yml` : entrée matrice `toolchains-docker`
  (`dockerfile: toolchains/docker/Dockerfile`).

## Contrainte de validation / échappement (entrée utilisateur → argv/URL/chemin)

- **`VNL_LSP_TOOLCHAINS` (dont `aux`)** : valeurs = constantes controller. Aucun champ
  de CRD, aucune entrée utilisateur. Spawn en argv, jamais de shell.
- **Invocation `hadolint`** : contenu du Dockerfile **sur stdin** (`hadolint -`),
  jamais le chemin en argv, jamais via un shell. `--format json --no-color` sont des
  littéraux. Le wrapper ne construit aucune commande à partir du contenu du buffer ou
  de l'URI.
- **Téléchargement du binaire `hadolint` au build d'image** : URL de release figée +
  vérification de checksum SHA256 (même exigence que le provisioning CLI de
  l'extension VS Code, `VNL-EXT-005`).
- **Chemin / URI navigateur → LSP** : inchangé (confinement R5 + `rewrite_uris`).
- **`toolchain` / `languageId`** : littéraux dérivés du nom de fichier.

## Risques identifiés

1. **Fusion des diagnostics** — `dockerfile-language-server` et `hadolint` peuvent
   signaler la même chose (ex. instruction dépréciée). Pas de dédup automatique en
   v1 : les deux `source` sont visibles, l'utilisateur voit deux entrées. Acceptable,
   documenté ; dédup possible en évolution.
2. **Conversion des positions hadolint** — 1-based → 0-based, fin de range à définir
   (hadolint ne donne qu'un point). Heuristique : range = de `(line-1, col-1)` à fin
   de ligne. Test dédié avec un Dockerfile connu-mauvais (`DL3008`, `DL3009`, …).
3. **Ossature multi-process dans `lsp.rs`** — introduite par `vue-lsp` (Volar v3).
   `docker-lsp` doit être ordonnée **après** `vue-lsp` et se contenter d'ajouter le
   rôle `diagnostics-merge`. Si l'ordre changeait, `docker-lsp` devrait porter
   l'ossature.
4. **`edit_and_check` / `lsp_diagnostics`** — lisent `cached_diagnostics` /
   `wait_for_diagnostics`. Le merge DOIT précéder l'écriture du cache, sinon
   navigateur et MCP divergent. Contrainte, pas option.
5. **`hadolint` binaire statique** — vérifier qu'une release statique linux-x86_64 (et
   arm64 si le cluster cible en a besoin) existe et se copie proprement dans une image
   Debian slim.
6. **Taille d'image** — node frais + dockerfile-ls + hadolint (~15 Mo) + wrapper.
   Acceptable.
7. **`Dockerfile` par nom de fichier** — `toolchain_for_path` (sandbox) et
   `lspToolchainForPath` (frontend) font tous deux `split('.')` aujourd'hui.
   Cohérence à assurer avec la feature `editor-syntax-highlighting`.

## Questions ouvertes

- **`LspSpec` CRD gagne `aux` vs composites preset-only** (penchant : preset-only) —
  question partagée avec `vue-lsp`, à trancher une fois pour les deux.
- **Langage du wrapper hadolint** : binaire Rust dans le crate sandbox (penchant :
  cohérent avec « pas de script shell assemblé », réutilise le framing) vs script node
  dans l'image.
- **Déclencheur** : `didSave` seul vs `didSave` + `didChange` debouncé (penchant :
  les deux, ~500 ms).
- **`.hadolint.yaml`** du projet respecté ? `hadolint -` lit le `.hadolint.yaml` du
  cwd — le wrapper doit `chdir` dans la racine du projet (ou passer `--config`).
  À câbler proprement (pas d'interpolation).
- **arm64** : la toolchain `docker` doit-elle être multi-arch ? (dépend des clusters
  cibles ; les autres images toolchain le sont-elles ? à vérifier au moment de la
  tâche image).

## Migration `docs/architecture.md` (Phase 3)

- § « Serveur LSP » : toolchain `docker`, notion de session composite rôle
  `diagnostics-merge`, wrapper `vnl-hadolint-lsp`.
- § « Détection de langages » : marqueur `dockerfile`, dérivation toolchain `docker`.
- Tableau mapping : `Dockerfile`/`*.dockerfile`/`Containerfile` → `docker`/`dockerfile`.
- § stack : image `toolchains/docker`, `hadolint`.
