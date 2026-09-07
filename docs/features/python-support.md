# Feature — Support Python (bas en haut)

## Ce que ça fait (une phrase)

Ajoute Python comme langage de première classe : toolchain OCI `python` auto-dérivée
à la détection (comme rust/node), LSP composite pyright + ruff alimentant l'éditeur web
et les tools `lsp_*`, coloration `.py` (déjà là), et activation automatique d'un
`.venv/` de workspace pour les commandes shell (terminal end-user + `execute_command`
du LLM).

## Ce que ça ne fait pas (périmètre explicite)

- **Pas de détection de version Python** — image figée sur `3.13` (cohérent rust/node,
  cf. `ws10-language-support` : les tags Docker Hub d'une version détectée ne sont pas
  garantis exister).
- **Pas de `ruff format` via LSP** — `ruff` est sur le PATH de la toolchain, disponible
  pour le futur tool `validate` ; le rôle `diagnostics-merge` de l'aux ne route aucune
  requête (porte `aux_answers_requests`, cf. `docker-lsp`), donc pas de formatage ni de
  code action ruff par le LSP en v1.
- **Pas de walk-up pour le `.venv`** — seul `<VNL_SANDBOX_ROOT>/.venv` est reconnu
  (convention quasi-universelle : `python -m venv .venv`, `uv`, PDM, poetry-in-project).
  Un venv niché (`services/api/.venv`) s'active à la main. Extension possible, notée.
- **Pas de re-scan pyright sur `.venv` créé à chaud** — la session LSP est spawnée une
  fois au premier `.py` ouvert ; un `.venv` créé après n'est pris en compte qu'au reload
  éditeur / prochain spawn de session.
- **Pas de composite formaté** — un seul aux (`vnl-ruff-lsp`, rôle `diagnostics-merge`),
  pas de `tsserver-forward`.
- **Isolation multi-projets du user-site** — `PYTHONUSERBASE` est par-Owner (partagé
  entre ses sandboxes), pas par-projet. C'est le fallback « pip install sans venv » ;
  le `.venv` est la vraie isolation. Tradeoff assumé, revisitable si ça mord.
- **Pas testé sur cluster réel** dans la session de dev (pas de backend K8s/Postgres) —
  comme toute la famille toolchain/LSP. Vérifs runtime listées en fin de doc.
- **Extension `dependabot.yml`** (écosystème `docker` pour suivre les bases `FROM`,
  npm pour `packages/*`/`ext/`) — chore séparé `chore/dependabot-coverage`, hors de
  cette branche (atomicité).

## Contexte — ce qui existe déjà

| Brique | État |
|---|---|
| Coloration `.py` CodeMirror | **Fait** — `@codemirror/lang-python` importé, `py: () => python()` dans `frontend/src/components/panels/editorLanguage.ts` |
| Ossature multiplexeur LSP + rôle `diagnostics-merge` | **Fait** — livré par `docker-lsp` (`sandbox/src/lsp.rs`), réutilisé tel quel |
| Wrapper LSP diagnostics-seuls sur stdin | **Modèle** — `sandbox/src/bin/hadolint_lsp.rs`, `vnl-ruff-lsp` en est un clone adapté |
| Pipeline détection → status → dérivation toolchain | **Fait** — `maint.rs::detect_languages` → `Project.status.languages` → `controller/src/sandbox.rs::effective_toolchains` |
| Presets composite preset-only | **Modèle** — `node_lsp_composite` / `docker_lsp_composite` |

## Interfaces clés et modules touchés

### 1. Détection — `sandbox/src/maint.rs`

- Nouveau marqueur `python` : `detect_languages` positionne `has_python` si l'arbre HEAD
  contient **un `*.py` ou `*.pyi` (n'importe quel niveau)** OU un `pyproject.toml` /
  `setup.py` / `setup.cfg` (n'importe quel niveau). Comparaison sensible à la casse comme
  les autres marqueurs.
- `LANGUAGE_ORDER` : `["rust", "js-ts", "vue", "python", "dockerfile"]` (dockerfile
  reste dernier ; python avant lui).
- Pas de helper partagé avec le frontend : l'extension `.py`/`.pyi` est triviale et les
  manifests ne concernent que la détection sandbox (le frontend `lspToolchainForPath`
  reste purement extension-based).

### 2. Image `toolchains/python/` — `toolchains/python/Dockerfile` + `test-toolchain-python.sh`

Base : `docker.io/library/python:3.13-slim-trixie` (famille trixie = contrainte glibc
AGENTS.md). Multi-stage :

- **stage `pyright`** : `FROM node:trixie-slim`, `npm install -g pyright` (flottant —
  bump = modifier la ligne, comme `typescript-language-server`). Copié dans le final :
  `/usr/local/lib/node_modules/pyright` **et** le binaire `/usr/local/bin/node`
  (`pyright-langserver` est un script Node — `node` doit être résolvable ; il finira sur
  le PATH de la toolchain via `/toolchains/python/usr/local/bin`).
- **stage `ruff`** : binaire `ruff` téléchargé depuis une release GitHub épinglée,
  **checksum SHA256 vérifié au build** (`echo "<sha> ruff" | sha256sum --check`) — un
  checksum qui bouge doit CASSER le build (contrainte de validation, même exigence que
  hadolint / VNL-EXT-005). Jamais `|| true`, jamais de désépinglage. amd64 (ce que build
  la CI, `release.yml` linux/amd64).
- **stage `wrapper`** : `FROM rust:1-slim-trixie`, `cargo build --release --locked
  -p vanyline-sandbox --bin vnl-ruff-lsp`. Cohérence glibc : compilé trixie, exécuté dans
  le pod (debian:trixie-slim) via volume.
- **final** : `python:3.13-slim-trixie` + `node` + `node_modules/pyright` + `ruff` +
  `vnl-ruff-lsp`, tous binaires dans `/usr/local/bin`.

`test-toolchain-python.sh` : rejoue le montage read-only non-root du pod, vérifie
`pyright-langserver --version`, `ruff --version`, `python3 --version`, `vnl-ruff-lsp`
présent. Ligne `toolchains-python` dans la matrice `image` de
`.github/workflows/release.yml` (même tag que app/sandbox/controller).

### 3. `vnl-ruff-lsp` — `sandbox/src/bin/ruff_lsp.rs` (4ᵉ binaire du crate sandbox)

Clone adapté de `hadolint_lsp.rs` :
- Rôle `diagnostics-merge` : **ne répond à aucune requête** (`initialize` → réponse
  minimale, tout le reste → method-not-found ; l'intelligence vient du primaire pyright).
- Déclencheurs : `didOpen` / `didSave` immédiats, `didChange` debouncé 500 ms ; résultat
  périmé abandonné (génération par URI, même mécanisme que hadolint).
- Exécution : `Command::new("ruff").args(["check", "--output-format", "json",
  "--force-exclude", "-"])`, buffer écrit sur **stdin**. **Aucun shell**, argv littéraux
  uniquement. `current_dir` hérité du spawn (`sandbox_root`, cf. `lsp.rs::spawn_aux_startup`)
  → ruff y trouve `pyproject.toml` / `ruff.toml` / `.ruff.toml` du projet, zéro
  interpolation.
- Conversion JSON ruff → `Diagnostic` LSP : ruff émet un tableau d'objets
  `{code, message, location:{row,column}, end_location:{row,column}, url, filename}`.
  `row`/`column` sont **1-based** → LSP `Range` **0-based** : soustraire 1 (même
  conversion que `convert_hadolint_output`). `severity` = Warning (ruff `check` ne
  distingue pas les niveaux en v1 ; `E9`/`F` restent des warnings côté LSP, cf. hadolint).
  `source` = `"ruff"`. `code` = la règle (`F401`, `E501`…). `codeDescription.href` = `url`.
- Filtrer sur `filename` == fichier courant (ruff `-` sur stdin met `filename` à `-`
  ou au path si passé — on passe `-`, donc pas de filtre nécessaire, mais garder le
  garde-fou).
- Identifiants d'erreur : réutilise la plage `VNL-SBX-LSP-*` — prochains libres à
  confirmer à l'implémentation (007/008 pris, cf. `lsp-agent-interface` ; docker a pris
  010/011). Le fichier de tâche fixera les numéros réels après `grep`.

### 4. Controller — `controller/src/{main,sandbox,project}.rs`

- `main.rs` : `TOOLCHAIN_IMAGE_PYTHON` (`--toolchain-image-python`, défaut
  `ghcr.io/sebt3/vanyline-toolchains-python:v<CARGO_PKG_VERSION>`) et `LSP_IMAGE_PYTHON`
  (défaut idem) ; passés à `SandboxPodContext` (`toolchain_image_python`,
  `lsp_image_python`).
- `sandbox.rs::toolchain_preset("python")` :
  - `PATH` = `{root}/usr/local/bin`
  - `LD_LIBRARY_PATH` = standard (`{root}/usr/lib/x86_64-linux-gnu:{root}/usr/lib/aarch64-linux-gnu:{root}/usr/local/lib`)
  - `PYTHONUSERBASE` = `/home/vanyline/.local` (`HOME_MOUNT_PATH`/.local — posé tel quel,
    pas de substitution `{root}` ; PVC Owner, writable ; partagé entre sandboxes de
    l'Owner — cf. périmètre)
  - `PIP_USER` = `1` (bare `pip install` va dans le user-site sans `--user`)
  - segment PATH additionnel `/home/vanyline/.local/bin` (literal dans la string PATH du
    preset — `aggregate_toolchain_env` concatène déjà les segments PATH)
- `sandbox.rs::resolve_toolchain_lsp` : preset `python` → `LspSpec { image:
  ctx.lsp_image_python, bin:
  "/toolchains/python-lsp/usr/local/bin/pyright-langserver", args: ["--stdio"] }`.
- `sandbox.rs::python_lsp_composite(ctx) -> (LspSpec, Vec<Value>)` : primaire pyright
  (comme ci-dessus), un aux :
  ```json
  { "role": "diagnostics-merge",
    "bin": "/toolchains/python-lsp/usr/local/bin/vnl-ruff-lsp",
    "args": [] }
  ```
  **Pas d'`initializationOptions` sur le primaire** — pyright auto-découvre
  `<workspace>/.venv` s'il existe, sinon `python3` du PATH (toolchain) + user-site
  (`PYTHONUSERBASE` hérité). Preset-only : `aux` n'existe que dans ce JSON interne, la
  CRD `LspSpec` ne l'expose pas (décision `vue-lsp` 2026-09-06).
- `sandbox.rs::effective_toolchains` : `languages` contient `"python"` → toolchain
  `python` avec `ctx.toolchain_image_python`, `lsp: None` (le composite remplace dans
  `build_sandbox_pod`). Ordre : rust, node, **python**, docker.
- `sandbox.rs::build_sandbox_pod` : gate composite comme vue/docker —
  `python_lsp_composite` appliqué uniquement au `python` **DÉRIVÉ** (`spec.toolchains`
  vide) d'un projet dont `status.languages` contient `"python"` ET `toolchain.lsp` est
  `None`. `spec.toolchains` explicite ou `lsp` custom ⟹ jamais d'aux.
- `project.rs` : `cache_dir_name("pip")` → `"pip"` ; `effective_caches` défaut →
  `["cargo", "pnpm", "pip"]` ; `sandbox.rs::cache_env_var("pip")` → `("PIP_CACHE_DIR",
  "/project-cache/pip")`.

### 5. Mapping chemin → toolchain

- `sandbox/src/tools_impl.rs::toolchain_for_path` : `.py` / `.pyi` → `("python",
  "python")` (après la règle dockerfile, avant le fallback `None`).
- `frontend/src/components/panels/editorLanguage.ts::lspToolchainForPath` : `case 'py':
  case 'pyi':` → `{ toolchain: 'python', languageId: 'python' }`. Ajouter `pyi` à
  `byExtension` (alias de `python()`), doc-comment mis à jour.

### 6. Activation `.venv/` — `sandbox/src/venv.rs` (nouveau) + `tools/src/command.rs` + `sandbox/src/ws/terminal.rs`

- `sandbox/src/venv.rs::venv_overlay(sandbox_root: &Path) -> Vec<(String, String)>` :
  - Marqueur : `<sandbox_root>/.venv/pyvenv.cfg` existe (fichier, pas juste le dossier).
  - Si présent : `[("VIRTUAL_ENV", "<sandbox_root>/.venv"), ("PATH",
    "<sandbox_root>/.venv/bin:<PATH courant>")]` où `<PATH courant>` =
    `std::env::var("PATH").unwrap_or_default()`.
  - Sinon : `vec![]`.
  - **Recalculé à chaque appel** — jamais mis en cache. Un `.venv` créé en cours de
    session est vu par le prochain terminal / la prochaine commande.
  - `sandbox_root` vient de `state.config.sandbox_root` (config du process, **pas** une
    entrée utilisateur).
- `tools/src/command.rs::ExecuteCommandOptions` : nouveau champ
  `#[serde(skip)] pub envs: Vec<(String, String)>` — **jamais désérialisé depuis les
  arguments du tool** (un LLM ne peut pas injecter d'env via `execute_command`), posé
  uniquement par l'appelant. `command::execute` applique `for (k, v) in &opts.envs {
  cmd.env(k, v); }` après le reste de la config, avant `spawn()`.
- `sandbox/src/tools_impl.rs::dispatch_command` : calcule `venv_overlay(sandbox_root)` et
  le passe dans `ExecuteCommandOptions { envs, .. }`. `cwd` continue de passer par
  `confine()` (inchangé).
- `sandbox/src/ws/terminal.rs::spawn_shell` : applique l'overlay au `CommandBuilder`
  (`cmd.env(k, v)` pour chaque paire) avant `spawn_command`. Signature :
  `spawn_shell(cwd, size)` inchangée — l'overlay se calcule depuis `cwd` (qui vaut
  `sandbox_root` au seul site d'appel réel ; les tests passent leur propre tmpdir).

## Ordre de résolution du PATH dans le pod

```
<sandbox_root>/.venv/bin        (si .venv — overlay runtime, shell uniquement)
/toolchains/python/usr/local/bin (preset toolchain — pyright/node absents ici : c'est /toolchains/python-lsp)
/toolchains/<autres>/...         (rust, node… si présents)
/home/vanyline/.local/bin        (user-site PYTHONUSERBASE — preset)
/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin  (BASE_PATH)
```

L'overlay `.venv` n'affecte **que** les process shell (PTY + `execute_command`). Le LSP
(process séparé, spawné par `LspManager`) n'en dépend pas : pyright fait sa propre
découverte `<workspace>/.venv`.

## Contraintes de validation / sécurité (règle config.md)

Aucune entrée utilisateur/réseau ne transite vers un argv, une URL ou un chemin shell :

- `LspSpec { image, bin, args }` du preset `python` et de `python_lsp_composite` :
  **constantes de code**, jamais un champ de CR interpolé (miroir rust/node/docker).
- `vnl-ruff-lsp` : `Command::new("ruff")` + argv littéraux ; le contenu du buffer passe
  sur **stdin**, jamais en argument ; `current_dir` hérité, aucun chemin construit
  depuis une entrée. Aucun shell.
- `venv_overlay` : `sandbox_root` est `state.config.sandbox_root` (config process). Le
  chemin `.venv` est un `Path::join` de constantes ; le marqueur est un `try_exists`.
  Aucune valeur issue du réseau ou du LLM.
- `ExecuteCommandOptions.envs` : `#[serde(skip)]` — impossible à peupler depuis les
  arguments du tool `execute_command`. Seul `dispatch_command` (code sandbox) l'écrit,
  avec la sortie de `venv_overlay`.
- `execute_command` : `cwd` continue de passer par `confine(sandbox_root, …)` (traversal
  déjà couvert, inchangé).
- Détection : `pyproject.toml`/`setup.py`/`setup.cfg`/`*.py` sont des **noms comparés**,
  jamais ouverts ni exécutés (`detect_languages` liste `git ls-tree`, ne lit rien).
- Image : URL de release `ruff` **épinglée + checksum SHA256 vérifié au build**, le
  build casse si le checksum diverge.

## Risques identifiés

1. **pyright a besoin de Node dans l'image** — inévitable (distribué en npm uniquement).
   Multi-stage : copier `node_modules/pyright` + le binaire `node`. `pyright-langserver`
   doit trouver `node` : le placer sur le PATH toolchain (`/usr/local/bin/node` dans
   l'image → `/toolchains/python-lsp/usr/local/bin/node` monté). À vérifier sur l'image
   construite (classe du bug `location` de `vue-lsp` : chemin dans l'image ≠ chemin
   supposé).
2. **Schéma JSON ruff → Diagnostic** — `row`/`column` 1-based (comme hadolint) ;
   colonnes ruff en unités de scalaires Unicode, LSP attend UTF-16 par défaut → décalage
   possible sur lignes non-ASCII (mineur, même classe que le « CRLF cosmétique » de
   docker-lsp). Dérouler 2-3 sorties ruff concrètes dans le fichier de tâche.
3. **Timing découverte venv par pyright** — session spawnée une fois ; `.venv` créé
   après n'est pas re-scanné. Documenté, pas de fix v1.
4. **`didChange` incrémental** — si pyright négocie le sync incrémental côté navigateur,
   le wrapper `vnl-ruff-lsp` doit gérer les deltas OU forcer le full sync côté aux (même
   drift noté sur `vnl-hadolint-lsp`). Le wrapper reconstruit le buffer complet : suivre
   le choix de hadolint (full text tracking interne).
5. **`PYTHONUSERBASE` partagé entre sandboxes d'un Owner** — conflits de versions
   possibles entre projets sur le user-site. Le `.venv` est la vraie isolation ; le
   user-site est un fallback. Revisitable (déplacer vers `/project-cache/pip-user`).
6. **`ruff check` sans config projet** — comportement par défaut de ruff (règles `E`+`F`)
   ; bruyant sur du code legacy. Acceptable : c'est ce que ruff fait partout ailleurs,
   et un `pyproject.toml`/`ruff.toml` projet est respecté (cwd hérité).

## Questions ouvertes

- Numéros `VNL-SBX-LSP-*` réels pour `vnl-ruff-lsp` — à fixer dans le fichier de tâche
  après `grep` de l'existant.
- Version `ruff` à épingler + son SHA256 — à figer à l'implémentation (dernière stable).
- Faut-il aussi injecter `VIRTUAL_ENV` au spawn du **LSP** quand `.venv` existe déjà, en
  plus de l'auto-découverte pyright ? (Défaut proposé : non — auto-découverte suffit
  pour le cas « venv avant édition ».)

## Vérifs runtime dues (pas de cluster en dev)

- Round-trip `.py` réel : complétion + diagnostics pyright + diagnostics ruff fondus,
  contre l'image construite.
- `.venv/bin/python` bien pris par le terminal PTY et par `execute_command` quand
  `<sandbox_root>/.venv` existe ; retour au python toolchain quand il est absent.
- `pip install <pkg>` sans venv → user-site `PYTHONUSERBASE`, `import <pkg>` OK ensuite.
- pyright voit les deps d'un `.venv` pré-existant.
- Image `toolchains/python` à rebuild + republier au prochain tag.

## Delivery

Design Claude → délégation Cadence en mode `.tasks/` (comme `vue-lsp` / `docker-lsp`),
tâches just-in-time une à une. `cargo fmt --all -- --check` + `cargo clippy --workspace
--all-targets -- -D warnings` obligatoires par tâche (cf. AGENTS.md). Review Phase 3
Claude avant merge, puis migration dans `docs/architecture.md` et suppression de ce
fichier.
