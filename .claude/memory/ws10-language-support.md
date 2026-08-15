# WS-10 — détection de langages + toolchains automatiques (2026-08-15)

## Ce qui a été livré

Scope réduit du design initial (`docs/features/ws10-language-support.md`, supprimé à
la clôture — contenu migré dans `docs/architecture.md` section "Opérateur
Kubernetes — Détection de langages et toolchains automatiques (WS-10)" et section
"Maintenance des workspaces") : détection Rust/JS-TS, remontée au status du Project,
dérivation automatique des toolchains de Sandbox. **Le tool `validate` (tasks 6-10
du design initial, jamais démarré) reste hors scope** — idée pas perdue (design
détaillé dans l'historique git du fichier supprimé), mais pas un chantier actif ; à
re-designer si repris.

Chaîne complète : `vanyline-maint detect` (marqueurs `Cargo.toml`
racine/imbriqué → `rust`, `package.json`/`tsconfig.json` racine seulement → `js-ts`,
présence seulement, jamais de version) → patch dédié `Project.status.languages`/
`detectedAt` (merge patch JSON ciblé, pas via `compute_status`) → `sandbox.rs::
effective_toolchains` dérive `Sandbox.spec.toolchains` quand il est vide (`rust`/
`js-ts` → images `TOOLCHAIN_IMAGE_RUST`/`TOOLCHAIN_IMAGE_NODE`, presets d'env
existants réutilisés).

## Décisions actées cette session

- **Pas de détection de version** — décision explicite du développeur principal en
  réponse à une question posée avant l'implémentation (ni `rust-toolchain.toml`/
  `rust-version`/edition, ni `.nvmrc`/`engines.node`). Si le besoin apparaît, ce sera
  une extension délibérée, pas une déduction implicite risquée (les tags Docker Hub
  correspondant à une version détectée ne sont pas garantis exister).
- **`spec.toolchains` explicite gagne toujours, jamais fusionné** avec la dérivation
  automatique (tout ou rien) — c'est le mécanisme par lequel un utilisateur choisit
  une image de toolchain custom (version pinnée, registry privé) quand le défaut ne
  convient pas. Confirmé explicitement par le développeur principal en réponse à la
  question sur le pinning de version.

## Pièges techniques trouvés en review (corrigés avant clôture)

- **RBAC trop large** : le `Role` du ServiceAccount de maintenance par Project
  (`project-<name>-maint`) donnait `patch` sur `projects/status` sans
  `resourceNames` — n'importe quel Project du même namespace aurait pu patcher le
  status d'un autre. Fix : `resourceNames: [project.name]`. Rappel pour toute future
  CRD RBAC scopée par ressource dans ce projet : `resourceNames` n'est **pas**
  automatique, à vérifier explicitement en review.
- **Chemin relatif cassé dans `list_head_tree`** (`sandbox/src/maint.rs`) :
  `--git-dir` était construit depuis `workspace.join("repo.git")` **et**
  `.current_dir(workspace)` était posé en plus — un `--workspace` relatif était donc
  résolu deux fois (`ws/ws/repo.git`). Invisible en cluster (`--workspace` toujours
  absolu, `/workspace`) et invisible dans les tests (`TempDir` toujours absolu) — un
  bug de ce type ne serait sorti qu'en usage local/dev avec un chemin relatif.
  Convention du fichier : soit `--git-dir` absolu sans `current_dir` (`run_fetch`),
  soit `current_dir` + chemin relatif — jamais les deux en même temps.
- **`CryptoProvider` rustls ambigu** : `kube` (feature `ring`) et `axum-server`/
  `reqwest` (feature `aws_lc_rs`, mêmes dépendances du binaire `vanyline-sandbox`)
  activent deux providers rustls différents dans le même binaire →
  `kube::Client::try_from` panique à l'auto-détection. Fix trouvé par Cadence :
  installer explicitement `rustls::crypto::ring::default_provider()
  .install_default()` avant tout `Client`, idempotent (erreur "déjà installé"
  ignorée). **Piège générique à surveiller** pour tout futur binaire de ce
  workspace qui combinerait `kube` et `reqwest`/`axum-server` dans le même crate.
- **`cargo fmt --check`** n'avait pas tourné avant les commits de Cadence (6
  fichiers). Rappel : `cargo fmt` fait partie des commandes de validation
  obligatoires au même titre que `test`/`clippy` (cf. `AGENTS.md`), pas une étape
  qu'on peut sauter parce que ça compile.

## Process — délégation à Cadence

Contrairement aux features précédentes de cette famille (design Claude → tâches
Qwen une-à-une via fichiers `.tasks/`), cette feature a été entièrement déléguée à
l'agent opencode `cadence` par le développeur principal après que Claude a commencé
à écrire le design/les tâches détaillées — Cadence a produit l'implémentation
complète (6 commits) sans repasser par le format `.tasks/<feature>/task-XX.md`
habituel. Résultat : code correct, tests verts, mais RBAC trop large + un bug de
chemin relatif + `fmt` non lancé, tous trouvés par la review Claude a posteriori et
corrigés dans un commit dédié avant clôture. Pas de conclusion à tirer sur
Cadence en général sur un seul point de données — mais **la review Claude post-
implémentation reste nécessaire même quand l'agent d'exécution change** (Phase 3
"Clôture" du workflow projet, cf. `.claude/config.md`).
