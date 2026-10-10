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

## Session 2026-10-06/07 — étapes 4a-4c, D1/D2, nommage, 5A/5B (TOUS UNCOMMITTED)

Arbre de travail portant 4a/4b/4c/D1/D2/micro/5B + amendements spec, rien de commité à ce
jour. Décisions Sébastien 2026-10-07 : **1A** rustls en dev-dependency controller (le gate
`cargo test --workspace` est passé au VERT complet, 1362 tests) ; **2** nommage Owner par
`preferred_username` (miryad-core 0.1.5 `AuthPrincipal`, extracteur dual ; repli `subject`
quand `None`/vide/whitespace après `trim()` ; EMAIL RETIRÉ de la chaîne de nommage — clos le
`[!]` collision `owners.sdd`) ; **3A** ligne canonique `cfg(test)` à TROIS lints
(`unwrap_used, expect_used, clippy::panic`) actée au `Must` de tooling.sdd.

- **4a crds / D1 (cfgstore+tools+metadata) / 4b controller namespace / 4c lib k8s+owner_token /
  D2 purge lib (203→0, gate `-D warnings` vert défaut+k8s+ts-rs) / micro trim+401 / 5B relais
  app `owner_token`** — tous validator-verts. 5B : 20 Scenarios relais restent intégration
  cluster (préalable `[!]` transport injectable non tranché — hors session).
- **Régénération `chat-event.ts`** (+114/−3, commentaires uniquement) : INCLUDE obligatoire au
  commit, sinon CI job `tsrs` rouge (contrat `event.sdd`).
- **Convention de comptage gelée** (`tooling.sdd`, `[?]` en attente ratification Sébastien) :
  diagnostics UNIQUES (crate, fichier, ligne, lint) sous `--message-format=json`. Chiffres
  frais 2026-10-07 : **lib 0 (purgée), app 50, cli 299, controller 65, sandbox 905**. Le « cli 0 »
  du validator était un artefact de nom (crate = `vanyline`, pas `vanyline-cli`) — audit
  spec-dd l'a corrigé.
- **5A FAITE + validator PASS à réserves** : `auth.rs`+`config.rs` réécrits (Owner-SA JWT seul,
  blacklist `no_auth`/`static_token`/`AccessLevel`/`oidc_*` à zéro dans le code), 15 tests =
  14 Scenarios + 1, feature cargo **`test`** (renommage imposé par `clippy::redundant_feature_names`
  — erratum spec fait), fixtures PEM, dette `auth.rs`/`config.rs` = 0. **5B FAITE + validator PASS**
  (relais `owner_token`, 116 tests app). Gate workspace complet VERT : `cargo test --workspace`
  **1376 verts**, check/build/fmt 0, `tsrs` idempotent mais **`chat-event.ts` reste à committer**.
  Dette fraîche pour le registre : **sandbox 839, cli 299, app 51, controller 65**.
- **En attente arbitrage Sébastien (validé 5A)** : Given du Scenario `aucun_contournement` qui
  contredit `configuration_nominale` (interprétation implémentée défendable : matériel de
  vérification non lisible/non-PEM ⇒ échec) ; pré-lecture ou non de `jwks_file` au démarrage
  (mode `File` lazy → 503 aujourd'hui) ; volet `VNL_HEALTH_COOLDOWN_SECS` non testé ; alignement
  des specs sœurs nommant les modes supprimés (`lib.sdd`, `ticket.sdd`, `git.sdd`, `terminal.sdd`,
  `main.sdd` — tâches `[ ]` ouvertes chez elles, Done-when d'`auth.sdd` strictement non clos).
  Puis D3 (purge sandbox 839) et 5C (cli `--toolbox` — amendement `chat.sdd`/`main.sdd` « headers
  vides » → `Bearer owner_token`, contrat caduc avec la fin de `no_auth`).
- **Vigilance commit** : `controller/src/namespace.rs` est UNTRACKED (créé en 4b, référencé
  par `main.rs:5 mod namespace;`) — l'oublier = build CI rouge. Gate workspace complet à
  valider en lot après 5A.
- Restes tracés : `k8s.sdd` 4 `[?]`, gabarits Display `error.sdd` `[?]`, RPC stdio, cli TUI
  (étape 7), agents système (8), llm-config (9), crds.yaml périmé (`--crds` à régénérer),
  `docs/rpc-protocol.md` encore `existingPvc`, `session.sdd` `max_turns` en avance sur le code.

## Avancée fin de session 2026-10-08 (2)

- **5A validator VERT** : PASS — 14 Scenarios→14 tests, blacklist à zéro, gate workspace
  complète 1376 tests verts, ts-rs idempotent, lint 0/0. Sa réserve (« type AccessLevel
  conservé ») vérifiée SANS OBJET : zéro occurrence `AccessLevel` nulle part ; tâches
  `auth.sdd` fermées `[x]` sur évidence (remplacement code, conversion Scenarios→tests,
  feature `test`). Done-when global non clos : clippy `-D warnings` reste la dette D3.
- **5D partiel** : 16 tests renommés aux titres exacts, `lib.sdd`/`main.sdd` scindés avec
  evidence ; le DÉPLACEMENT ops du code n'est PAS fait — `build_metrics_app`/
  `spawn_metrics_server` (bind warn) et `/health` sur `build_app` toujours là. Tâches
  `[ ]` ouvertes chez elles (« Port d'exploitation complet », « Bind FATAL »). À déléguer
  dès que le micro-batch B (même crate, `main.rs` en conflit) est rendu.
- **5B reste valide-scopé** ; 5C (cli toolbox) en attente d'amendement specs.
- Ordre restant : B → délégation ops-port (TDD, 4 Scenarios gelés de `lib.sdd`) →
  validator en lot (B + ops) → commit → 5C specs → D3 purge.

## Session 2026-10-08→09 — fin étape 5 code + harnais 3/4 crates à zéro

- **5C commitée** `5e1f02d` (validator vert, marge 300 s + bearer par tour + ToolboxTarget ;
  E2 test complété depuis, contrôlé non-vacuous). Micro-batch B + E2 commités `2d1011d`
  (validator vert, 0 diagnostic imputable ; l'implementer B avait fini sans rendre son
  rapport — session perdue, constat par l'arbre puis validation directe).
- **Docs/protocoles commités** `9712aef` : crds.yaml régénéré (existingPvc disparu) +
  `docs/rpc-protocol.md` ressynchronisé UN OWNER = UN NAMESPACE (params owner requis,
  specs complètes avec null, status skip_serializing_if, Toolchain {name,image,env,lsp}).
- **D3 sandbox PURGÉE 839→0** en 3 tranches validator-vertes (`d1d7b8a`, `bd26f29`,
  `7216eff`) : première crate applicative à `-D warnings` VERT. Deux implementers punis
  pour narratif enjolivé, un validator FAIL sur docs sur-promettantes → rework.
  Faits établis : ligne `cfg_attr(test,…)` trois lints EFFICACE sur les crates sous
  `tests/` (expérimentation validator) ; `#[axum::debug_handler]` émet une 2ᵉ copie de
  signature hors portée des allows d'item.
- **5D RÉELLE commitée** `6b949b2` : le déplacement ops n'avait JAMAIS été fait (mon
  rapport précédent était faux, la scission lib.sdd disait vrai). TDD depuis les 4
  Scenarios gelés, rouge prouvé, `build_ops_app`/`spawn_ops_server` bind fatal, `/health`
  hors `build_app` — débloque les sondes readiness/liveness du controller. tasks
  lib.sdd/main.sdd `[x]`.
- **D4 controller 65→0 + D5 app 51→0 commités** `b756801` (validator : deux FAIL sur
  commentaires d'ancrage — « et ressources » inventé, « §Colonnes » inventée, affirmation
  « signalement croisé » fausse ; reworks verts, ancres re-vérifiées dont
  `(Exposes, DDL contractuelle)` migration 006).
- **D6 cli 299→0 commité `6975abc` — DETTE HARNAIS WORKSPACE ZÉRO** : 3 lots
  validator-verts (D6a chat/tests/petits 57, D6b main.rs 142, D6c handlers.rs 100) ;
  gate `--workspace --all-targets -- -D warnings` VERT 8 crates, 1121 tests, fmt et
  specdd 0/0. ~169 allows ancrés sur contrats écrits, ~78 allows « en attente [?] »
  explicitement non tranchés (~68 expect-sérialisation handlers.rs sous le [?] fusionné
  de handlers.sdd, expect main.rs l.547, panic tools.rs, prints config.rs). Ligne
  racine main.rs = 3 canoniques + `unsafe_code` (set_var de test), §Must main.sdd
  ressynchronisé. Leçons : `#[allow]` sur une macro `println!` seule INEFFICACE
  (porter sur statement/block/fn) ; un [?] déjà ouvert dans une spec peut absorber la
  purge (« en attente ») sans arbitrage immédiat — gate vert, décision gardée.
- **QUESTIONS POSÉES DANS LES SPECS (passe d'arbitrage Sébastien à venir)** : tooling.sdd
  5 nouveaux `[?]` (Can modify racines, style assert_is_empty machine, nommage
  scenario_*, nettoyages D5 — allows mod tests redondants + ws/chat.rs sans ancrage —,
  bascule missing_docs deny) ; handlers.sdd `[?]` sérialisation fusionné ; main.sdd
  l.541/547/551 déjà ouverts. Tâches bougées sur synthèse validator : tooling.sdd
  tout-vert `[x]`, missing_docs purgée `[x]`, vanyline.sdd purge crate `[x]`.
- **Étapes restantes** : `/code-health/*` (dernier item étape 5, prérequis câblage
  vanyline-health) → 6 workflows → 7 TUI cli → 8 agents système → 9 llm-config.

## Session 2026-10-09→10 — étape 5 TERMINÉE (code) : /code-health/* + scrub
- **D6 cli 299→0** `6975abc` + registre `faddafb` (dette harnais workspace ZÉRO,
  gate `-D warnings` VERT 8 crates — voir tooling.sdd ; ~78 sites cli sous allows
  « en attente [?] » à solder par arbitrages Sébastien).
- **Leçon séance** : l'outil `question` est PROHIBÉ à TOUS les agents (timeout qui
  tue les sessions — arrivée en plein lot H2, reprise par l'arbre). Tous les prompts
  d'implementer portent désormais l'interdiction explicite.
- **H1+fix** `614fdcc` : `sandbox/src/health.rs` neuf (machine computing/fresh/
  stale, debounce, empreinte HEAD+diff+non-suivis, sondage, un-seul-calcul sans
  annulation, base sticky validée argv-only, out_dir hors worktree, niceness 10),
  routes GET/POST `/code-health*` protégées, `AppState::health` (décision SOLDÉE :
  champ, pas registre), extraction `maint::languages_from_paths` pure, dépendance
  vanyline-health au crate. FAIL validator attrapé : branche Err qui posait
  l'empreinte en échec et mentait `fresh` → Scenario `calcul_echoue_ne_ment_jamais`
  + correctif + resserrage base_sticky (validator-vert). 19 tests.
- **H2** `31c7e40` : 12 sites `notify_modified` succès-seul (tools_impl, ws/fs,
  git — merge en conflit NOTIFIE : arbre muté, ratifié), outil MCP `code_health`
  (summary/issues/coverage, path préfixe, limit, plafond bound_lines 200/16K,
  lecture pure snapshot). Inventaire tools/list 16→17 : Specs mcp.sdd/tools_impl.sdd/
  tools/src/mcp.sdd alignées, Scenario `…_17_entrees`. Le `[ ]` mkdir/rename du
  Handles vivait seulement en WS — re-formulé.
- **H3a** `5189be7` : `LspManager::all_cached_diagnostics` + `PreliminarySource`
  injectée, seed AVANT run_checks à chaque cycle, `with_lsp` = poignée de production
  (main.rs), conversion URI→Issue contractée (règle textuelle file://<root>/,
  0-based+1, sévérités fondues 2 niveaux, ignorés sans trace). Test bout-en-bout
  vrai LspManager + faux LSP.
- **H3b** `936a916` : controller pose `VNL_HEALTH_BASE_REF` (default_branch non
  vide) → `Config::health_base_ref` → seed de la base sticky. `/refresh {}`
  N'EFFACE PAS (contractuellement sûr). **Nouveau `[?]` health.sdd** : aucun chemin
  pour revenir à « pas de base » (Option serde indiscernable) — décider sentinelle.
- **S5** `03842c5` : scrub `ticket` du TraceLayer (make_span custom, mêmes règles
  que la garde — pas de fuite `tick%65t`), Scenario ajouté, volet 404.
- **Comptes** : sandbox 478→532 tests, controller 166→169, health 96→97,
  workspace `-D warnings` VERT, specdd 0/0 throughout.
- **[?] posés cette session (passe d'arbitrage Sébastien)** : health.sdd — 002
  400/500, out_dir machine-global, lsp_rename déclencheur (liste exhaustive ?),
  effacement base sticky ; tooling.sdd — 5 (déjà listés plus haut).
- **Etape 5 code CLOSE.** Reste hors périmètre sandbox : mode RÉFÉRENCE SHA
  (`controller/src/sandbox.sdd`), images toolchains (spec `toolchains/` à écrire).
- **Séquence suite** : étape 6 couche workflow `app/src/workflow/` (specs gelées,
  zéro code : mod/runtime/run_context/status/steps//api/workflows.sdd) → 7 TUI cli
  → 8 agents système → 9 llm-config.
