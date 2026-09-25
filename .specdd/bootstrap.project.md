# SpecDD project specific overrides

## Flux de développement : SpecDD + test-first (TTD)

Unité de travail : une spec `.sdd`. Règle absolue : pas de code sans spec approuvée par
Sébastien, pas d'implémentation avant les tests dérivés de la spec.

1. **Spec** — la spec de la cible existe et est revue : rédigée en session avec l'agent
   `spec-dd`, ou produite en reverse-engineering par `spec-reverse` puis relue
   attentivement par Sébastien (le comportement accidentel du code ne devient un contrat
   qu'après cette relecture).
2. **Test d'abord** — l'agent `implementer` convertit d'abord chaque `Scenario` de la spec
   en test, et les fait compiler et échouer. Il ne touche pas encore au comportement de
   production.
3. **Implémentation** — le minimum de code qui fait passer les tests, strictement dans le
   périmètre `Owns` / `Can modify` de la spec.
4. **Validation** — l'agent `validator` relance la batterie complète (AGENTS.md, section
   "Commandes de validation") et produit une synthèse.
5. **Clôture** — les `Tasks` passent à `[x]` seulement après synthèse verte ; spec, code et
   tests avancent ensemble, sans `[x]` décoratif.

## Règle : une spec `.sdd` par fichier source Rust

- Un fichier de `src/` d'un crate du workspace = une spec `.sdd` du même nom dans le même
  répertoire (`src/foo.rs` ↔ `src/foo.sdd`), couvrant **tout** son comportement observable :
  `Exposes`, `Accepts`, `Returns`, `Raises`, `Handles`, `Must`, `Must not`, `Scenario`.
- Regrouper 2–3 fichiers dans une spec n'est admis que s'ils forment un seul contrat ; la
  justification du regroupement va dans `Purpose` de la spec.
- Un module `mod.rs` qui ne fait que réexporter n'exige pas de contrat indépendant : sa
  spec peut se limiter à `Purpose` + `Owns`, le contrat vivant dans les specs des
  sous-modules — sauf si `mod.rs` porte lui-même du comportement (fonctions, impls,
  constants).
- Hors `src/` : la CI a sa spec (`/.github/workflows/workflows.sdd`), le harnais de
  toolchain a la sienne (`/tooling.sdd` : lints clippy/rustc + fmt), et la spec racine
  `/vanyline.sdd` porte les contraintes transverses du monorepo.
- Une spec sans fichier source correspondant est légitime pour les artefacts de
  configuration ; l'inverse (source sans spec) est interdit d'amendement direct : on crée
  la spec d'abord.
- TypeScript/Vue (frontend, `packages/*`, extension VS Code) : même boucle SpecDD+TTD,
  mais la règle un-fichier-une-spec s'y applique **au fil de l'eau**, autour des zones en
  cours de changement — pas de rétro-spécification massive planifiée. Un fichier `.ts`
  ou `.vue` touché par une tâche sans spec existante exige une spec avant modification
  (production : la spec décrit le contrat du composant/module, pas chaque ligne).

## Harnais clippy (guidage des agents)

- La source de vérité du harnais est la table `[workspace.lints]` du `Cargo.toml` racine +
  `lints.workspace = true` sur chaque membre, contractualisée par `/tooling.sdd` :
  famille stricte (`unwrap_used`, `expect_used`, `panic`, `unreachable`, `dbg_macro`,
  `todo`, `unimplemented`, `print_stdout`, `print_stderr`, `arithmetic_side_effects`),
  groupes `pedantic` et `cargo`, en `deny` ; `unsafe_code` `deny` (exemptions ciblées
  justifiées uniquement) ; `missing_docs`
  `warn` (passera en `deny` après purge de la dette, décision de Sébastien).
- Workspace Cargo UNIQUE (membres `lib`, `app`, `cli`, `sandbox`, `tools`, `controller`,
  `crds`, `cfgstore`) : le harnais se déploie en un seul endroit — ne jamais recréer de
  table `[lints]` locale dans un membre.
- Dans tout fichier touché par une tâche : nouveau et nouveau code à **zéro warning**
  clippy. La dette préexistante est tracée dans `/tooling.sdd`, ne doit pas grossir, et se
  purge crate par crate, module par module.
- Code de production : jamais `unwrap()` / `expect()` / `panic!` / `todo!` /
  `unimplemented!` / `dbg!` / `println!` / `eprintln!` — propager l'erreur du crate
  (`thiserror`) et logger via `tracing`.
- `#[allow(...)]` des lints du harnais : uniquement sous `cfg(test)` (exemption portée par
  chaque racine de crate) ou avec commentaire de justification d'une ligne citant la spec.
- Frontend/packages : pas d'équivalent clippy ; les garde-fous sont `vue-tsc` + vitest.
  Pas de `console.log` — logger du projet.

## Rôle de Sébastien

Sébastien est la source de vérité sur l'intention. Toute ambiguïté de contrat, de sécurité,
de frontière ou de permission d'édition : on s'arrête et on demande, on ne suppose pas.
