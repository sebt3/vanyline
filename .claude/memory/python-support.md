# Feature — Support Python (bas en haut)

**Close le 2026-09-08. Branche `feat/python-support` mergée dans `main`
(`b7c18bc`, `--no-ff`) et poussée, branche supprimée. Design doc
`docs/features/python-support.md` supprimé, contenu migré dans
`docs/architecture.md` (§ "Serveur LSP" — rôle `diagnostics-merge` 2ᵉ
consommateur, wrapper `vnl-ruff-lsp`, mapping/coloration `.py`/`.pyi` ;
§ "Détection de langages" ; § "Toolchains automatiques" — preset `python`,
cache `pip` ; nouvelles sous-sections "Variante composite pyright" et
"Activation `.venv/`").**

## Ce que ça fait

Python langage de première classe : toolchain OCI `python` auto-dérivée à la
détection (comme rust/node/docker), LSP composite **pyright** (primaire) +
**ruff** (aux `diagnostics-merge`) alimentant l'éditeur web et les tools
`lsp_*`, coloration `.py`/`.pyi`, et **activation automatique d'un `.venv/` de
workspace** pour les commandes shell (terminal PTY + `execute_command` du LLM).

3ᵉ consommateur de l'ossature multiplexeur composite de [[vue-lsp]] — même rôle
aux trivial que [[docker-lsp]] (`diagnostics-merge`, l'aux ne répond à aucune
requête).

## Architecture livrée (détail dans `docs/architecture.md`)

- **Détection** (`sandbox/src/maint.rs`) : marqueur `python` = `*.py`/`*.pyi`
  **ou** `pyproject.toml`/`setup.cfg` n'importe où dans l'arbre HEAD (`setup.py`
  couvert par `.py`). **Pas de helper partagé** avec le frontend (contrairement
  à `is_dockerfile_path`) — l'extension est triviale, les manifests ne
  concernent que la détection. `LANGUAGE_ORDER` = `["rust","js-ts","vue",
  "python","dockerfile"]` (python avant dockerfile).
- **Controller** (`controller/src/sandbox.rs`) : `python_lsp_composite` — preset
  composite **uniquement** sur le `python` DÉRIVÉ (`spec.toolchains` vide) d'un
  projet `python`. Primaire `pyright-langserver --stdio`, aux
  `{role:"diagnostics-merge", bin:".../vnl-ruff-lsp", args:[]}`. **Aucune
  `initializationOptions` sur le primaire** (question ouverte tranchée : pas de
  `pythonPath` injecté) — pyright auto-découvre `<workspace>/.venv`, sinon
  `python3` du PATH toolchain + user-site `PYTHONUSERBASE`. Preset-only comme
  volar/hadolint. Preset env `python` : `PATH` (`{root}/usr/local/bin` +
  `/home/vanyline/.local/bin` littéral) + `LD_LIBRARY_PATH` standard +
  `PYTHONUSERBASE=/home/vanyline/.local` + `PIP_USER=1` (fallback « pip sans
  venv » → user-site du PVC Owner, **partagé entre sandboxes d'un Owner** —
  tradeoff assumé, le `.venv/` est la vraie isolation). Flags CLI
  `TOOLCHAIN_IMAGE_PYTHON`/`LSP_IMAGE_PYTHON`. `effective_caches` défaut →
  `["cargo","pnpm","pip"]` (`pip` → `PIP_CACHE_DIR=/project-cache/pip`).
- **Wrapper `vnl-ruff-lsp`** (`sandbox/src/bin/ruff_lsp.rs`, **4ᵉ binaire** du
  crate sandbox, baké dans l'image toolchain python) : clone quasi verbatim de
  `vnl-hadolint-lsp` — même framing, rôle `diagnostics-merge`, déclencheurs
  `didOpen`/`didSave` immédiats + `didChange` debouncé 500 ms, capture
  `(texte,version,génération)` + abandon de résultat périmé, dégradation
  silencieuse. `ruff check --output-format json --force-exclude -`, **buffer sur
  stdin**, cwd hérité (`.ruff.toml`/`pyproject.toml` respectés). Conversion :
  positions ruff 1-based → ranges 0-based (clamp UTF-16, `end` depuis
  `end_location` sinon fin de ligne), `code`→`Diagnostic.code`,
  `url`→`codeDescription.href`, `source="ruff"`. **Severity : `ruff_severity()`
  relaie la valeur ruff** quand c'est une chaîne connue (`error`/`fatal`→1,
  `warning`→2, `info`/`information`/`notice`→3, `hint`→4), **défaut 2 (Warning)**
  si absente/inconnue — **correctif développeur en Phase 3** (le design disait
  « tout → Warning » quand on croyait ruff sans niveaux ; ruff 0.16.6 pose
  `"error"` y compris sur F401/I001 ⟹ ces lints remontent en Error). Codes
  `VNL-SBX-LSP-012` (spawn/E-S) / `-013` (stdout non-JSON).
- **Activation `.venv/`** (`sandbox/src/venv.rs`) : `venv_overlay(sandbox_root)`
  teste `<sandbox_root>/.venv/pyvenv.cfg` **en tant que fichier** → overlay
  `VIRTUAL_ENV` + `PATH` préfixé de `.venv/bin`, sinon vide. **Recalculé à
  chaque spawn/invocation** (jamais de cache — `.venv` créé à chaud vu au
  suivant). Seul `<sandbox_root>/.venv` (pas de walk-up). Appliqué au **PTY**
  (`ws/terminal.rs::spawn_shell`, `cmd.env`) et à **`execute_command`**
  (`tools_impl.rs::dispatch_command`). Nouveau champ
  `vanyline_tools::command::ExecuteCommandOptions.envs` en **`#[serde(skip)]`** —
  jamais désérialisé des arguments du tool, un LLM ne peut pas injecter d'env
  (test de sécurité `tools_call_execute_command_envs_non_injectables`). Pas
  appliqué au LSP (pyright fait sa propre découverte).
- **Mapping** `.py`/`.pyi` → `("python","python")` :
  `tools_impl.rs::toolchain_for_path` (après la règle dockerfile) + miroir
  `editorLanguage.ts::lspToolchainForPath`. Coloration : `@codemirror/lang-python`
  était **déjà là** (`py:()=>python()`), ajout de l'alias `pyi`.
- **Image** `toolchains/python/Dockerfile` (multi-stage) : base
  `python:3.13-slim-trixie` + `apt-get install libatomic1` (le `node` copié la
  lie, la base python ne l'embarque pas). `npm i -g pyright` sur `node:trixie-slim`
  → `node_modules/pyright` **et** le binaire `node` copiés dans le final.
  **`pyright-langserver` en SYMLINK RELATIF** `../lib/node_modules/pyright/
  langserver.index.js`, pas en `COPY` (le shim npm résout son propre répertoire ;
  copié en fichier régulier → `MODULE_NOT_FOUND` sous le montage volume — même
  classe que `ln -sr` de rust-analyzer). `ruff` binaire épinglé **0.16.6**,
  SHA256 **du tarball** vérifié au build (le `.sha256` officiel de la release ;
  jamais `|| true`). `vnl-ruff-lsp` baké par copie multi-stage. Entrée matrice
  `release.yml` `toolchains-python`. `test-toolchain-python.sh` (5 étapes podman :
  layout, versions read-only non-root, handshake LSP pyright réel — **pas
  `--version` qui n'existe pas, classe `docker-langserver`** —, smoke
  `vnl-ruff-lsp` + vrai ruff, `pyproject.toml` du cwd respecté).

## Delivery

Livré par **Cadence** (mode feature `.tasks/`, 6 tâches). Claude a écrit le
design doc + `task-01`, Cadence a produit son propre découpage pour la suite.
`fmt` lancé. **0 escalade Cadence.** Les découvertes runtime-class ont été
attrapées **pendant l'implémentation via de vrais smoke tests**, pas en review
Phase 3 — contraste net avec [[vue-lsp]]/[[F5-vscode-ext-sandboxes]] où le
chemin faux était dans le fichier de tâche écrit par Claude. Le process a
fonctionné.

Découvertes Cadence (toutes légitimes, toutes gérées avant Phase 3) :
1. `pyright-langserver` doit rester un symlink relatif (design §2 disait
   « binaire copié ») — `MODULE_NOT_FOUND` sinon.
2. `pyright-langserver --version` n'existe pas → preuve par handshake LSP
   complet (leçon `docker-langserver` de [[docker-lsp]] appliquée).
3. `python:3.13-slim-trixie` sans `libatomic.so.1` que node lie → `libatomic1`
   au final (piège node-volume AGENTS.md, `ldd` vérifié).
4. IDs d'erreur : le design croyait `docker-lsp` preneur de 010/011 — ce sont
   des IDs des tools `lsp_*`. 001–011 pris → `vnl-ruff-lsp` prend 012/013.
5. Ruff 0.16.6 émet bien `"severity":"error"` (design disait « ne distingue pas
   les niveaux ») → **décision révisée par le développeur en Phase 3** : relayer
   la severity, Warning en défaut seulement.

## Review Phase 3

**0 bug bloquant** — 2ᵉ feature composite sans blocker (après [[docker-lsp]]).
Validation complète verte en local : `cargo fmt --check`, `clippy --workspace
--all-targets -D warnings`, `cargo test --workspace` (sandbox 350, controller
88, tools 47, http intégration, `ruff_lsp` unit + smoke), frontend 400/400.

**1 correctif développeur intégré** (`98066e9`/`78880ad` après rebase) : relayer
la severity ruff (cf. wrapper ci-dessus).

**Observations mineures notées (non bloquantes) :**
- **Overlay PATH du PTY fragile** : un fichier d'init de shell qui réassigne
  `PATH` sans `$PATH` détruit le préfixe `.venv/bin` (le `~/.bashrc` de la
  machine de dev le fait via `/etc/profile`, le bashrc Debian slim du pod
  **non**). `VIRTUAL_ENV` survit quoi qu'il arrive. `execute_command` (`sh -c`,
  pas de rc) non concerné.
- **Découverte `.venv` par pyright limitée à la racine du `rootUri` client** :
  un `.py` profondément niché ouvert dans l'éditeur navigateur avec un `.venv`
  à la racine du workspace peut ne pas être analysé venv-aware (cohérent avec
  le contrat « racine seulement » de l'overlay shell).
- **Décalage colonne UTF-16** pour un char hors-BMP avant la violation
  (conversion triviale `-1`, assumée — classe « CRLF cosmétique » de
  [[docker-lsp]]).
- `deploy/controller/crds.yaml` périmé (pré-existant) — se régénère à la
  release via `generate-crds.sh`, les doc-comments `languages`/`caches` touchés
  s'y retrouveront.

**Vérifs runtime dues (pas de cluster en dev)** : round-trip `.py` réel
(complétion pyright + diagnostics ruff fondus), `.venv/bin/python` pris par PTY
et `execute_command`, `pip install` sans venv → user-site, pyright voit les deps
d'un `.venv` racine pré-existant, image `toolchains/python` à rebuild+republier
au prochain tag.

## Suivi hors feature

**Chore `chore/dependabot-coverage`** (pas démarré) : ajouter l'écosystème
`docker` au `.github/dependabot.yml` (suit les `FROM` des Dockerfiles, dont les
bases `python:3.13-slim-trixie`/`node:trixie-slim`/`rust:slim-trixie`) + npm
pour `packages/*`/`ext/`. Dependabot ne suit **pas** les `curl | sha256sum`
épinglés (hadolint, ruff) — ceux-là restent manuels (une ligne + checksum).

## Attribution commit

Comme [[docker-lsp]] : le system-reminder de session demandait un trailer
`Co-Authored-By: Claude Sonnet 5`. Les 3 commits Claude l'ont d'abord porté
(erreur), corrigé par rebase avant le merge/push — `.claude/config.md` et le
CLAUDE.md global disent tous « Pas de `Co-Authored-By` », politique équipe
versionnée > réglage de session. Commits finaux sans trailer.
