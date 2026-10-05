# Migration état-final 2026-10 — étapes 1-3 (2026-10-05)

Branche `docs/specs-etat-final-2026-10` : après gel des specs, implantation de la
séquence validée avec Sébastien : 1 suppression `ext/` → 2 bump miryad-core 0.1.5 →
3 crate `vanyline-health` → 4 Un Owner = un namespace → 5 auth Owner-SA → 6 couche
workflow → 7 CLI TUI → 8 agents système → 9 `llm-config` (sur décision).

## Commités (3, arbre propre, specdd lint 0/162)

- `9b8226f` **Suppression extension VS Code** — `ext/` (44 fichiers) + `docs/ext-install.md`
  + jobs `ext` de test.yml/release.yml + entrée workspace npm + script `install:local` ;
  AGENTS.md/architecture.md/release-runbook.md/bootstrap.project.md/validator.md alignés.
  Tâches `[x]` : vanyline.sdd (SUPPRESSION), workflows.sdd (moitié ext de la tâche scindée).
- `a5ade55` **miryad-core 0.1.3 → 0.1.5** — ruptures absorbées : quatuor PKCE du mock,
  `MiryadAuthState{secure_cookies: true en dur, token_pepper = COOKIE_SECRET}`, redirects
  retirés d'`OidcConfig` (seule source : MiryadAuthState dans main.rs), création REST 201,
  `preferred_username: None` sur chemin ApiToken. **Arbitrages Sébastien 2026-10-05** :
  poivre = COOKIE_SECRET (rotation ⇒ réémission tokens acceptée), secure_cookies true en
  dur (Ingress HTTPS seulement), timeouts **configurables par env** :
  `OIDC_CONNECT_TIMEOUT_SECS` (5) / `OIDC_TIMEOUT_SECS` (15), repli silencieux sur valeur
  non parseable. Noyau privé `from_env_with(read)` VALIDÉ (stratégie tests from_env,
  zéro unsafe). 4 Scenarios auth/mod.sdd convertis en tests `[x]`.
- `9bcb2d0` **crate `vanyline-health`** — 11 specs → 12 fichiers, 96 tests (97 sous
  `ts-rs`), seule crate du workspace à dette harnais zéro sur sources. Export ts-rs
  `packages/protocol/src/generated/health-report.ts` : **fichier auto-suffisant, les
  `import type` de ts-rs sont strippés à la concaténation** (le générateur EST le test
  `export_typescript`, renforcé : 0 import, 11 déclarations uniques, gate npm check).
  Job CI `tsrs` étendu. **Arbitrages Sébastien 2026-10-05** : diff = modèle « span »
  contracté (pas de positions par ligne dans FileCoverage — limite de surestimation
  assumée) ; dépendant d'un préalable `errored` ⇒ `errored` en cascade (jamais `skipped`,
  « skipped = rien à vérifier ») ; test de non-dérive checks ↔ CI étendu à fmt.
  Le nom du module de tests de l'app : `scenario_tests` (variante validée).

## Méthode éprouvée dans la session

- Chaque étape : implementer (TDD, rouge démontré) → validator (batterie + évaluation
  du DELTA de dette uniquement — le rouge clippy workspace est la dette tooling.sdd
  consentie) → `[x]` sur synthèse verte uniquement.
- **Un FAIL validator attrapé** : l'artefact ts-rs généré compilait pas (imports
  pendants) — lesson : régénérer ET passer le gate du consommateur (`tsc`) avant de
  claimed vert ; le Scenario renforcé l'empêche désormais.
- tooling.sdd re-compteur 2026-10-05 : total « méthode-dépendante » (le « 224 » d'un
  comptage ≠ format court 104 err + 239 warn) — parties sûres : 45 cargo_common_metadata
  = 5 × 9 packages, health incluse.

## Restes ouverts (tracés dans les specs, non masqués)

- ~14 Scenarios `from_env_*` de config.sdd sans tests (écrivables contre `from_env_with`).
- `[!]` collision espace `VNL-CFG-*` app ↔ cfgstore — non tranché.
- `[?]` checks.sdd : essai réel provider vitest `--no-save` (réseau sortant sandbox).
- Étape 4 à démarrer : `controller/src/namespace.sdd` (203 lignes neuves) + crds +
  lib/k8s (`owner_token`) + app ; puis étape 5 qui la suit (auth Owner-SA), étape 6
  workflow (tous les prérequis sont posés : miryad-core 0.1.5 est au lock, health est là).
