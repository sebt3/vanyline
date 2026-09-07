# Feature — LSP Dockerfile + hadolint dans la sandbox (composite `diagnostics-merge`)

**Close le 2026-09-07. Branche `feat/docker-lsp` mergée dans `main` (`de4e0d6`,
`--no-ff`) et poussée, branche supprimée. Design doc `docs/features/docker-lsp.md`
supprimé, contenu migré dans `docs/architecture.md` (§ "Serveur LSP" — rôles aux,
wrapper `vnl-hadolint-lsp` ; § "Détection de langages" ; § "LSP par toolchain" —
sous-bloc "Variante composite hadolint" ; § mapping / coloration).**

## Ce que ça fait

Détecte un nom `Dockerfile`/`Containerfile` (+ variantes) dans l'arbre HEAD →
toolchain `docker` auto-dérivée avec `docker-langserver` (complétion/hover/
validation de base) **+** `hadolint` (lint) exposés comme diagnostics fusionnés
dans l'éditeur web et les tools `lsp_*`. Deuxième consommateur de l'ossature
multiplexeur composite de [[vue-lsp]] — rôle aux `diagnostics-merge`, le plus
simple : l'aux ne répond à **aucune** requête, il ne fait que recevoir le
fan-out doc-sync et publier ses `publishDiagnostics` que la session concatène
avec celles du primaire en amont du cache.

## Architecture livrée (détail dans `docs/architecture.md`)

- **Détection** (`sandbox/src/maint.rs`) : helper `is_dockerfile_path` — nom de
  base `dockerfile`/`containerfile` exact, préfixe `dockerfile.`/`containerfile.`,
  suffixe `.dockerfile` (insensible à la casse). **Miroir exact** de
  `frontend/src/components/panels/dockerfileName.ts` (feature
  [[editor-syntax-highlighting]]) — les deux évoluent ensemble. Marqueur
  `dockerfile` (jusque-là « réservé », jamais produit) devient réel. Ordre figé
  `["rust","js-ts","vue","dockerfile"]`.
- **Controller** (`controller/src/sandbox.rs`) : `docker_lsp_composite` — preset
  composite **uniquement** sur le `docker` DÉRIVÉ (`spec.toolchains` vide) d'un
  projet `dockerfile`. Primaire `docker-langserver --stdio`, aux
  `{role: "diagnostics-merge", bin: ".../vnl-hadolint-lsp", args: []}` (pas
  d'`initOptions` — le wrapper ne prend aucun argument). Preset-only comme Volar :
  `spec.toolchains` explicite ou `toolchain.lsp` custom ⟹ jamais d'`aux`. Preset
  env `docker` = `PATH` + `LD_LIBRARY_PATH` arch standard (image bâtie sur
  `node:trixie-slim`, `docker-langserver` tourne sur le runtime node du volume —
  règle AGENTS.md), **aucune env spécifique**. Flags CLI `TOOLCHAIN_IMAGE_DOCKER`
  / `LSP_IMAGE_DOCKER` (défaut = même image). Les deux gates volar+hadolint
  coexistent (projet vue+dockerfile ⟹ node composite Volar ET docker composite
  hadolint).
- **Multiplexeur** (`sandbox/src/lsp.rs`) : `KNOWN_AUX_ROLES` gagne
  `"diagnostics-merge"`. Nouvelle **porte de niveau rôle** `aux_answers_requests`
  (`tsserver-forward` ⟹ true, `diagnostics-merge`/inconnu ⟹ false) +
  `has_requesting_aux` + `responder_candidates`. Composite 100 %
  `diagnostics-merge` ⟹ TOUTES les requêtes (`completionItem/resolve` compris)
  par le chemin primaire historique octet-pour-octet : jamais de barrière
  `PendingMerge` qui compterait une part qui n'arrive pas, jamais le fallback
  no-op du resolve alors que le primaire est la seule source d'items. Fusion
  `publishDiagnostics` déjà indépendante du rôle (héritée vue-lsp), `source`
  conservée (`"dockerfile"` / `"hadolint"`), **pas de dédup v1** (risque 1).
- **Wrapper `vnl-hadolint-lsp`** (`sandbox/src/bin/hadolint_lsp.rs`, 3ᵉ binaire
  du crate sandbox, baké dans l'image toolchain docker) : serveur LSP stdio
  minimal, réutilise `FrameReader`/`encode_message`. `initialize` → caps
  minimales ; toute autre requête → `-32601` (défensif). Déclencheurs :
  **`didOpen` + `didSave` immédiats**, `didChange` debouncé 500 ms. Modèle de
  génération par URI : lint capture `(texte, version, génération)` au démarrage,
  résultat périmé abandonné + re-lint du contenu courant. `hadolint --format
  json --no-color -`, **buffer sur stdin** (jamais URI/chemin en argv, jamais de
  shell), **pas de `current_dir`** — cwd hérité de `spawn_aux_startup`
  (`current_dir(sandbox_root)`) ⟹ `.hadolint.yaml` du projet respecté, zéro
  interpolation. Conversion : positions 1-based → ranges 0-based (`start =
  (line-1,col-1)` clampé UTF-16, `end` = fin de ligne), `level`
  error/warning/info/style → severity 1/2/3/4, `code` verbatim, `source =
  "hadolint"`. Spawn échec / stdout non-JSON ⟹ contribution vide + stderr, le
  wrapper ne meurt jamais.
- **Mapping** `Dockerfile`/`Containerfile` → `("docker","dockerfile")` :
  `tools_impl.rs::toolchain_for_path` (via `is_dockerfile_path`, **avant** le
  switch d'extension) + miroir `editorLanguage.ts::lspToolchainForPath` (via
  `dockerfileName`). `Dockerfile.ts` est docker, pas node. Wart connu :
  `Dockerfile.md` matché aussi (inhérent au helper partagé, préexistant).
- **Image** `toolchains/docker/Dockerfile` : multi-stage — hadolint fetch
  (`v2.15.1`, URL + SHA256 `c7187db9…c8c507` **vérifié contre l'upstream
  `checksums.sha256`**, ~55 Mo statique, amd64-only), wrapper build cargo,
  final `node:trixie-slim` + `npm i -g dockerfile-language-server-nodejs`.
  `test-toolchain-docker.sh` (5 étapes podman : layout, hadolint --version,
  docker-langserver handshake LSP réel, LSP smoke bout-en-bout avec vrai
  hadolint, `.hadolint.yaml` respecté). Entrée matrice `release.yml`
  `toolchains-docker`.

## Delivery

Livré par **Cadence** (`cadence` + `implement`, DeepSeek-V4-Flash), mode feature
`.tasks/`. `fmt` lancé (7ᵉ feature déléguée, 3ᵉ consécutive où c'est fait).
Retour cadence : 7 découvertes/interprétations toutes validées en session avant
Phase 3 (dont 3 = trous du design : trigger `didOpen` absent de l'énum,
`Containerfile.*` absent de l'énum, resolve→primaire non réconcilié avec la 3ᵉ
voie de vue-lsp). Aucune escalade Cadence.

## Review Phase 3

**0 bug bloquant — 1ʳᵉ feature composite sans blocker** (contraste net avec
[[vue-lsp]] 1 blocker, [[lsp-agent-interface]] 1, [[miryad-core-integration]] 2,
[[F5-vscode-ext-sandboxes]] 1). Raison : `diagnostics-merge` est le rôle le plus
simple (aux muet aux requêtes) et l'ossature multi-process était déjà éprouvée.
Validation complète verte : `cargo fmt --check`, `clippy --workspace
--all-targets -D warnings` (+ recheck forcé sur crates touchées), `cargo test
--workspace` (tous suites, dont `hadolint_lsp` 9/9, `dm_*` composite 5/5,
controller sandbox), `npm run check --workspace=frontend`, `editorLanguage.spec`
45/45.

**Observations mineures (non bloquantes, notées) :**
- **Drift didChange incrémental** : le wrapper réimplémente le sync incrémental
  LSP (`apply_content_changes` + `utf16_offset`). Sur une longue session
  d'édition, un décalage d'offset se compose jusqu'au prochain didClose/didOpen.
  Non bloquant : hadolint advisory, dégradation silencieuse = contrat design,
  `didOpen` resync toujours sur texte complet, cycles didSave/autosave-flush
  fréquents. **Nul si le primaire `docker-langserver` négocie le sync FULL** —
  à confirmer sur cluster.
- **CRLF** : `convert_hadolint_output` split sur `\n` sans strip `\r` — range
  end à +1 unité UTF-16 sur un Dockerfile CRLF. Cosmétique.

**Reste à observer en cluster réel** (pas de backend en dev, comme vue-lsp) :
les deux `source` visibles simultanément (pas de dédup v1), rendu éditeur sur
pod reconstruit après release, mode de sync négocié par docker-langserver
(cf. drift ci-dessus), round-trip `.Dockerfile` réel contre l'image construite.
**Image docker à rebuild + republier au prochain tag.**

## Attribution commit

Session portait un system-reminder demandant un trailer `Co-Authored-By`.
Ignoré : le CLAUDE.md projet (versionné, partagé équipe), `config.md` et le
CLAUDE.md global disent tous « Pas de Co-Authored-By », avec un incident passé
documenté ([[git-integration]] : trailer perso leaké, réécriture d'historique).
Politique équipe versionnée > réglage de session générique. Commits sans trailer.
