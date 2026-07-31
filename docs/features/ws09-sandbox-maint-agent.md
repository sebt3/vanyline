# Feature — ws09-sandbox-maint-agent

## Ce que la feature fait

Fige la règle **"l'image sandbox est l'outil de maintenance du controller"** et
la matérialise : un utilitaire `vanyline-maint` embarqué dans l'image sandbox
remplace les scripts shell inline des jobs git du controller. Corrige au passage
R1 (injection shell via champs de CRD) et R2 (presets toolchain hardcodés x86_64).

## Ce qu'elle ne fait pas

- Pas encore de détection de langages (`detect` est livré par WS-10 — cette
  feature pose juste le binaire et sa place dans l'image)
- Pas de changement du cycle de vie des jobs (init/fetch/purge/checkout/remove
  gardent leur sémantique, seuls leurs pods changent de commande)
- Pas d'"agent LLM" dans l'image — `vanyline-maint` est un utilitaire
  déterministe ; un agent viendra peut-être plus tard, la règle le permet

## La règle (à inscrire dans AGENTS.md et architecture.md)

Toute action de maintenance du controller sur les projets (clone, fetch, purge,
worktrees, détection) s'exécute dans un pod portant **l'image sandbox**, via un
utilitaire dédié de l'image — jamais un script shell assemblé par le controller.
Conséquences : une seule image à maintenir, l'outillage git/langages disponible
au même endroit pour la maintenance ET pour les sessions LLM, et des arguments
qui passent en argv (pas d'interpolation shell).

## `vanyline-maint` — interface

Nouveau binaire du sous-workspace sandbox (`sandbox/src/bin/maint.rs`), clap :

```
vanyline-maint init      --repo <url> --workspace <dir> [--cache <name>]...
vanyline-maint fetch     --workspace <dir>
vanyline-maint purge     --workspace <dir>
vanyline-maint checkout  --workspace <dir> --sandbox <name> --branch <ref> [--default-branch <ref>]
vanyline-maint remove    --workspace <dir> --sandbox <name>
vanyline-maint detect    --workspace <dir>       # stub ce sprint, implémenté par WS-10
```

- Reprend la logique exacte des scripts actuels (`controller/src/project.rs`,
  `controller/src/sandbox.rs`) : idempotence (`init` si bare absent, `checkout`
  si worktree absent), création de branche depuis la default branch, repli
  `rm -rf` + `worktree prune` sur worktree incohérent.
- **Validation des entrées** : `--branch`/`--default-branch` passés à
  `git check-ref-format --branch` (ou équivalent en logique Rust) avant usage ;
  `--repo` parsé comme URL/chemin git plausible. Les invocations git se font par
  `std::process::Command` en argv — R1 clos par construction.
- Layout (worktrees/, cache/, repo.git) : constantes partagées avec le
  controller — les helpers `bare_repo_path`/`worktree_path`/`cache_path` de
  `controller/src/project.rs` sont la référence ; garder la duplication minime
  et testée des deux côtés (pas de crate partagée pour 3 chemins).

## Côté controller

- `git_pod_template` : `command: ["vanyline-maint", ...]` construit en Vec —
  plus aucun `sh -c` dans `project.rs`/`sandbox.rs`.
- **R2** : les presets toolchain incluent les deux arches dans
  `LD_LIBRARY_PATH` (`{root}/usr/lib/x86_64-linux-gnu:{root}/usr/lib/aarch64-linux-gnu:…`)
  — le loader ignore les répertoires absents, aucune logique par-nœud à
  introduire dans le controller.

## Risques et questions ouvertes

- Le binaire `vanyline-maint` grossit l'image sandbox (marginal — même workspace,
  binaire strippé).
- L'ancienne image en circulation ne connaît pas `vanyline-maint` : le controller
  et l'image doivent avancer ensemble (même release — acceptable, ils sortent du
  même repo ; le README de WS-8 le documente).
- La duplication des constantes de layout controller/maint : risque de dérive —
  couvert par un test de chaque côté sur les mêmes valeurs littérales.

## Découpage en tâches candidates

1. `maint-binaire` — squelette clap + `init`/`fetch`/`purge` (logique reprise des
   scripts, validation des entrées, tests unitaires sur la validation)
2. `maint-worktrees` — `checkout`/`remove` (+ `detect` stub qui sort un JSON vide)
3. `controller-cutover` — `git_pod_template` en argv, suppression des scripts
   inline, fix R2 (presets deux arches), tests du pod spec
4. `docs-regle` — AGENTS.md + architecture.md : la règle et ses conséquences
