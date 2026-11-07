# Solution Design: Santé du code

## Résumé

- La Solution `sandbox/code-health` réalise la santé du code du pod : la crate feuille `vanyline-health` (presets de vérifications par langage, parseurs, diff, runner, rapport typé exporté en TypeScript) et son exposition par la sandbox (cycle de vie de l'état, `GET /code-health`, `POST /code-health/refresh`, outil MCP `code_health`).
- Elle réalise `sandbox#BO-2` et `sandbox#BR-7` : l'état est toujours disponible, recalculé après un délai de calme, et lu de la même façon par tous les consommateurs.
- Un seul code calcule le verdict en local et au central. Les règles (lints, tests, seuils) restent dans le projet analysé ; la crate n'en déclare aucune.
- Document reverse : il reprend les contrats de `health/src/*.sdd` et de `sandbox/src/health.sdd`, et relève les contradictions entre specs en Open Issues sans les trancher.

## Problème et objectif

- Un relecteur de code (humain ou LLM) doit voir la même synthèse verte ou rouge que l'auteur ; l'invariant visé est « même commande, mêmes versions d'outils, même rapport ». Avant, la batterie vivait à trois endroits qui dérivaient (SpecDD /health/src/lib.sdd, Purpose ; AGENTS.md).
- Objectif de la Solution : fournir ce rapport à tout instant dans le pod, borné pour un LLM, avec une fraîcheur déclarée honnêtement.

## Périmètre et non-objectifs

Couvert :

- la crate `vanyline-health` (SpecDD /health/src/*.sdd) : presets, parseurs, diff, runner, rapport, erreurs ;
- son exposition dans la sandbox (SpecDD /sandbox/src/health.sdd) : cycle de vie de l'état, routes HTTP, outil MCP.

Non-objectifs :

- définir un seuil, un gate ou un verdict autre que `status()` : le crate rapporte, les consommateurs interprètent (SpecDD /health/src/lib.sdd, Must not) ;
- persister l'état au-delà du pod (DEC-4) ;
- les relais de `app` et la revue de MR, consommateurs de la santé (`sandbox`, section Acteurs).

## Contexte applicable du domaine

- `sandbox#BO-2` : santé toujours disponible, identique en local et au central. Réalisé par FR-1, FR-10, FR-13 et DEC-2.
- `sandbox#BR-1` : accès tout ou rien par le jeton d'Owner, appliqué aux routes (FR-12). `sandbox#BR-4` : `/health` reste sur le port d'exploitation, d'où `/code-health` (DEC-3).
- `sandbox#BR-5` : toolchains par injection d'environnement ; les vérifications héritent de cet environnement (FR-15).
- `sandbox#BR-7` : état toujours disponible, recalcul après délai de calme, même état pour tous. Réalisé par FR-1 à FR-4 et FR-11.
- `sandbox#BR-9` : effets de l'agent bornés au pod ; les vérifications exécutent du code du projet (Sécurité).
- `sandbox#FR-1` et `sandbox#FR-5` : santé offerte par MCP (FR-9) ; langages couverts (FR-19).
- `sandbox#CON-3` : outils de vérification présents dans les images toolchain (OI-12).

## Acteurs

- **Owner :** seul titulaire accepté du jeton qui ouvre les routes (`sandbox#BR-1`).
- **Développeur :** lit l'état dans l'IDE vanyline ; **cli et kydah-code :** consomment les routes et l'outil (SpecDD /sandbox/src/health.sdd, Purpose).
- **Agent de code :** lit l'outil MCP `code_health`, en lecture seule (FR-9).
- **Service central de revue de MR :** consommateur déclaré, sans spec lue (OI-13).
- **`app` et Controller :** `app` relaie la santé du code (`sandbox`, section Acteurs) ; le Controller fournit la base par défaut du Project, `health_base_ref` (SpecDD /sandbox/src/health.sdd, Tasks).

## État actuel et état cible

- **Actuel (Observed in: `sandbox/src/health.rs`, `sandbox/src/lib.rs` lignes 233-234) :** routes `GET /code-health` et `POST /code-health/refresh` montées sur le routeur protégé, état unique `CodeHealth` dans `AppState`, outil `code_health` déclaré. Ce constat est conforme à SpecDD /sandbox/src/health.sdd et contraire à SpecDD /sandbox/src/lib.sdd (OI-1).
- **Cible (SpecDD /sandbox/src/health.sdd, Tasks ouvertes) :** sandbox sur une référence (SHA) et non une branche, prérequis du mode central ; images toolchain avec outils épinglés (spec `toolchains/` à écrire).

## Hypothèses, contraintes et dépendances

- **CON-1:** La crate est une feuille : aucune dépendance interne au workspace, ni `rig-core`, `rmcp` ou `kube`. Les I/O sont confinées à `runner.rs`.
    - **Source:** SpecDD /health/src/lib.sdd, Must ; Scenario `crate_feuille_sans_harnais_llm`.
- **CON-2:** Les règles (lints, tests, seuils, configuration ruff) appartiennent au projet analysé ; la crate n'en déclare aucune.
    - **Source:** SpecDD /health/src/lib.sdd, Purpose et Must not ; AGENTS.md, section Architecture.
- **CON-3:** Un outil de vérification absent du run est un échec visible de cette vérification (`errored` ou `failed`), jamais un vert.
    - **Source:** SpecDD /health/src/checks.sdd, Returns ; `sandbox#CON-3`.
- **CON-4:** Le calcul s'appuie sur `auth::require_auth`, le cache LSP, le plafond de `vanyline_tools::output` et la détection de langages de `vanyline-maint`.
    - **Source:** SpecDD /sandbox/src/health.sdd, Can read et Exposes ; SpecDD /health/src/runner.sdd, Exposes.

## Exigences fonctionnelles

- **FR-1 - MUST:** `GET /code-health` répond 200 avec l'état (`phase`, `report`, `summary`, `computed_at`, `stale_since`, `base`), à tout instant, jamais 404, y compris pendant le premier calcul (`phase: computing`, `report: null`, `summary: null`). Au démarrage, le premier calcul part en tâche de fond dès que le worktree existe, sans bloquer le serveur ni les sondes ; l'état vit en mémoire et se recalcule après redémarrage.
    - **Source:** SpecDD /sandbox/src/health.sdd, Routes, Handles (Démarrage, État en mémoire seulement), Scenario `etat_toujours_disponible_des_le_demarrage` ; `sandbox#BO-2`.
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** Une modification notifiée par un point de la liste des déclencheurs lance un délai de calme de `health_cooldown_secs` (30 s par défaut), relancé à chaque nouvelle notification ; `0` calcule immédiatement. Seuls les succès notifient ; les lectures et opérations git purement lectures ne notifient pas.
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles (liste exhaustive, ratifiée 2026-10-14) ; Scenarios `rafale_de_modifications_un_seul_calcul`, `cooldown_zero_recalcul_immediat`, `lsp_rename_notifie_en_succès_seul` ; `sandbox#BR-7`.
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** Avant chaque calcul, l'empreinte du worktree (`HEAD`, `git diff HEAD`, fichiers non suivis) est comparée à celle du dernier calcul terminé ; identique, il n'y a pas de calcul et la phase repasse à `fresh`. Un sondage toutes les `health_poll_secs` (60 s par défaut, `0` désactive) recalcule l'empreinte et rattrape les modifications faites hors outils.
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles ; Tasks `[-]` TRANCHÉ 2026-10-04 (sondage) ; Scenarios `empreinte_identique_aucun_recalcul`, `edition_au_terminal_detectee_par_sondage`.
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** Un seul calcul tourne à la fois par pod. Une modification ou un `refresh` pendant un calcul n'annule pas celui-ci : la phase passe à `stale` et un cycle suit sa fin. `POST /code-health/refresh` répond 202 et lance le recalcul sans délai de calme ; si un calcul tourne, le nouveau est replanifié derrière lui.
    - **Source:** SpecDD /sandbox/src/health.sdd, Must et Routes ; Scenarios `modification_pendant_un_calcul`, `un_seul_calcul_a_la_fois`, `refresh_force_ignore_le_delai`.
    - **Verified by:** `AC-4`
- **FR-5 - MUST:** Le champ `base` de `/refresh` a trois effets : absent, la base sticky est inchangée ; une chaîne, elle devient la base sticky ; `null`, elle est effacée. Une base syntaxiquement invalide ou refusée par `git rev-parse --verify` donne 400 et laisse la base inchangée ; une base non vérifiable (`git` non lançable) donne 500. Une base non vérifiée n'est jamais utilisée.
    - **Source:** SpecDD /sandbox/src/health.sdd, Routes, Raises, Tasks `[x]` 2026-10-14 ; Scenarios `base_invalide_refusee`, `base_nul_efface_la_sticky`, `base_absente_laisse_la_sticky`, `git_non_lançable_rend_500`.
    - **Verified by:** `AC-5`
- **FR-6 - MUST:** Les langages du calcul sont ceux détectés dans le worktree, ou ceux passés à `/refresh`. Un langage sans preset est rapporté dans `unchecked_languages`, jamais omis ni présenté comme vérifié.
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles ; SpecDD /health/src/checks.sdd, Exposes (`unchecked_languages`) ; SpecDD /health/src/report.sdd, Exposes ; Scenario `langage_sans_preset_rapporte`.
    - **Verified by:** `AC-6`
- **FR-7 - MUST:** Le calcul ne lance aucun shell et n'accepte aucune commande du client. Les seules entrées client sont `base`, validée, et les noms de langages, qui ne sélectionnent que des presets.
    - **Source:** SpecDD /sandbox/src/health.sdd, Must ; SpecDD /health/src/checks.sdd, Must ; SpecDD /health/src/runner.sdd, Must.
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** Les diagnostics déjà connus des serveurs LSP du pod sont collectés avant les commandes et versés comme `preliminary_issues`. Seuls les URI `file://<racine>/` sont retenus, avec chemin relatif, ligne et colonne décalées de 1 ; sévérité `1` donne `error`, les autres `warning`. Un diagnostic hors racine ou sans message est ignoré sans trace.
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles (contrat ratifié 2026-10-10) ; SpecDD /health/src/runner.sdd, Handles ; Scenario `diagnostics_lsp_transmis_au_calcul`.
    - **Verified by:** `AC-8`
- **FR-9 - MUST:** L'outil MCP `code_health` est en lecture seule et rend un texte borné. Par défaut (`summary`) : phase, statut, une ligne par vérification, totaux de couverture, langages non vérifiés, 20 premières issues. `detail` `issues` ou `coverage`, filtres `path` (préfixe) et `limit`.
    - **Source:** SpecDD /sandbox/src/health.sdd, Exposes (MCP) ; SpecDD /sandbox/src/mcp.sdd (déclaration, 17 outils) ; Scenarios `outil_mcp_rend_un_resume_borne`, `outil_mcp_filtre_par_chemin` ; `sandbox#FR-1`.
    - **Verified by:** `AC-9`
- **FR-10 - MUST:** `summary` (`status`, `coverage_percent`, `files` triés du moins couvert au plus couvert, puis par chemin) est dérivé du `report` au moment du `snapshot()`. `coverage_percent` vaut `null` sans mesure. Le `report` est renvoyé tel que produit par le crate.
    - **Source:** SpecDD /sandbox/src/health.sdd, Exposes et Must ; Scenarios `resume_derive_du_rapport`, `resume_sans_mesure_de_couverture`, `resume_nul_pendant_le_premier_calcul`.
    - **Verified by:** `AC-10`
- **FR-11 - MUST:** Un rapport dont l'empreinte ne correspond plus au worktree n'est jamais présenté comme `fresh`. Un calcul échoué (`VNL-SBX-CH-006`) ne pose pas l'empreinte du dernier calcul terminé et est retenté au cycle suivant. `fresh` n'est permis que si le rapport porte déjà l'empreinte du calcul échoué.
    - **Source:** SpecDD /sandbox/src/health.sdd, Must ; Scenario `calcul_echoue_ne_ment_jamais`.
    - **Verified by:** `AC-3`
- **FR-12 - MUST:** Les routes `/code-health*` et l'outil `code_health` exigent le jeton de l'Owner : 401 sans jeton, 403 pour le jeton d'un autre Owner. `GET /health` n'existe pas sur le port principal (404) : la santé du code n'est jamais exposée sous `/health`.
    - **Source:** SpecDD /sandbox/src/health.sdd, Must ; Scenarios `auth_exigee`, `ne_pas_confondre_avec_la_sonde` ; `sandbox#BR-1`, `sandbox#BR-4`.
    - **Verified by:** `AC-11`
- **FR-13 - MUST:** `HealthReport::status()` est la seule définition du vert : `failed` si une vérification est `failed`, sinon `errored` si une est `errored`, sinon `passed`. `skipped` et `unchecked_languages` n'y entrent pas ; un rapport sans vérification est `passed`. Aucun consommateur ne le recalcule.
    - **Source:** SpecDD /health/src/report.sdd, Returns et Must ; Scenarios `status_failed_prime_sur_errored`, `status_errored_sans_failed`, `rapport_vide_est_passed`.
    - **Verified by:** `AC-12`
- **FR-14 - MUST:** Toutes les vérifications d'un calcul s'exécutent et sont rapportées dans l'ordre reçu. Une vérification `errored` n'interrompt pas les autres. Une vérification dont le préalable `after` est `errored` n'est pas lancée et devient `errored`. Au plus `max_parallel` (2 par défaut) s'exécutent à la fois, jamais deux du même groupe de sérialisation.
    - **Source:** SpecDD /health/src/runner.sdd, Handles ; SpecDD /health/src/checks.sdd, Tasks `[-]` 2026-10-04 ; Scenarios `toutes_les_verifications_sont_rapportees_malgre_un_echec`, `prealable_errore_erore_le_dependant`, `parallelisme_borne`, `meme_repertoire_de_build_jamais_en_parallele`.
    - **Verified by:** `AC-13`
- **FR-15 - MUST:** Chaque commande est lancée en argv direct, avec `cwd` = racine plus le `cwd` de la vérification, sous `nice -n 10`, et avec l'environnement hérité complété de `env` (additif). Les sorties vont dans `out_dir`, hors de la racine, plafonnées à `output_cap_bytes` (64 MiB) par fichier. Le timeout tue le groupe de processus entier.
    - **Source:** SpecDD /health/src/runner.sdd, Exposes et Handles ; Scenarios `sortie_dans_out_dir_pas_dans_root`, `timeout_tue_le_groupe`, `sortie_plafonnee_tronquee_avec_marqueur` ; `sandbox#BR-5`.
    - **Verified by:** `AC-14`
- **FR-16 - MUST:** Une vérification est `passed` (code 0), `failed` (code non nul, la vérification a produit son résultat) ou `errored` (spawn impossible, timeout, sortie inexploitable). Un code non nul dont la sortie structurée est produite donne `failed` avec ses issues. Une couverture exigée absente ou vide sur une vérification réussie donne `errored`, jamais 0 %.
    - **Source:** SpecDD /health/src/runner.sdd, Handles (Statut, Sorties structurées) ; SpecDD /health/src/report.sdd, Must ; Scenarios `defauts_trouves_par_ruff_sont_failed_pas_errored`, `lcov_vide_sur_succes_est_erreur`.
    - **Verified by:** `AC-15`
- **FR-17 - MUST:** Les parseurs sont purs et déterministes. Une sortie JSON illisible rend `InvalidCheck` (ruff, hadolint). Un rapport lcov tronqué est rejeté, jamais rendu partiel. Le diff refuse les noms de fichiers cités et les hunks mal formés plutôt que de les omettre.
    - **Source:** SpecDD /health/src/ruff_json.sdd et hadolint_json.sdd, Raises ; SpecDD /health/src/lcov.sdd, Raises et Must ; SpecDD /health/src/diff.sdd, Raises et Must ; SpecDD /health/src/cargo_messages.sdd et tsc_text.sdd, Must (jamais de panique).
    - **Verified by:** `AC-16`
- **FR-18 - MUST:** Si une `base` est connue, le code neuf est le diff `git diff -U0` entre le merge-base de `base` et l'état du worktree, modifications non commitées incluses. La couverture et les issues y sont restreintes aux lignes ajoutées ou modifiées. Un échec ajoute un résultat `new-code` `errored` et laisse `new_code` à `None`.
    - **Source:** SpecDD /health/src/runner.sdd, Handles ; SpecDD /health/src/diff.sdd, Accepts et Returns ; Scenarios `code_neuf`, `code_neuf_inclut_le_non_committe`, `echec_du_diff`.
    - **Verified by:** `AC-17`
- **FR-19 - MUST:** Les presets couvrent `rust` (format, clippy, couverture avec tests), `js-ts` et `vue` (check, fournisseur de couverture, couverture par workspace npm), `python` (ruff, tests, couverture) et `dockerfile` (hadolint sur les fichiers suivis). Les commandes présentes dans `test.yml` (fmt, clippy) portent les mêmes drapeaux que la CI ; seul `--message-format=json` s'y ajoute.
    - **Source:** SpecDD /health/src/checks.sdd, Returns et Must ; Scenarios `preset_rust_complet`, `preset_ts_par_workspace`, `preset_python_et_dockerfile`, `clippy_memes_drapeaux_que_la_ci` ; SpecDD /health/src/lib.sdd, Structure.
    - **Verified by:** `AC-18`

## Sécurité

- Argv seulement : aucun `sh -c`. La `base` n'entre qu'en argv, après refus d'une valeur vide, commençant par `-`, contenant un espace, un NUL ou un retour à la ligne (SpecDD /health/src/runner.sdd, Handles ; SpecDD /sandbox/src/health.sdd, Raises).
- Aucune route publique (FR-12). Le rapport ne porte aucun chemin absolu (SpecDD /health/src/report.sdd, Must not) ; les sorties vont hors du worktree, `out_dir` (SpecDD /sandbox/src/health.sdd, Handles).
- Les vérifications exécutent du code du projet analysé (scripts, `build.rs`, `npm install --no-save` du fournisseur de couverture). Leurs effets sont bornés au pod (`sandbox#BR-9`) ; la liste de ce que l'agent ne doit jamais pouvoir faire reste ouverte (`sandbox#OI-1`).
- Les sorties (`output_tail`, messages d'issues) sont renvoyées telles quelles aux consommateurs ; aucune rédaction n'est contractée (OI-10).

## Conception

Structure, en deux niveaux :

```text
vanyline-health (crate feuille, health/)
  checks     presets argv par langage, pur
  runner     seule I/O : spawn, sorties dans out_dir, diff, assemblage
  parseurs   cargo_messages, tsc_text, ruff_json, hadolint_json, lcov (purs)
  diff       lignes du code neuf (pur)
  report     HealthReport et status() (pur, export ts-rs)
  error      HealthError (VNL-HLT-*)

sandbox (sandbox/src/health.rs)
  CodeHealth (Arc, dans AppState)
    notify_modified()   -> délai de calme -> empreinte -> calcul (runner)
    snapshot()          -> HealthState avec summary dérivé
    refresh(base, langues), spawn_background() (idempotent)
  routes GET /code-health, POST /code-health/refresh (routeur protégé)
  outil MCP code_health (lecture seule, sortie bornée)
```

Cycle de la phase :

- `computing` : un calcul tourne ; `report` est le dernier terminé (`null` au tout premier).
- `stale` : une modification est connue depuis le dernier calcul terminé ; un cycle est planifié.
- `fresh` : le rapport correspond au worktree. À la fin d'un calcul, la phase est `fresh` si rien n'a été modifié pendant ce calcul, sinon `stale` (SpecDD /sandbox/src/health.sdd, Handles).

Presets (SpecDD /health/src/checks.sdd, Returns) :

| Langage | Vérifications (id, kind, sortie) |
| --- | --- |
| `rust` | `rust-fmt` (format) ; `rust-clippy` (lint, CargoMessages) ; `rust-coverage` (coverage, Lcov) |
| `js-ts`, `vue` | par workspace npm : `ts-check` (lint, TscText) ; `ts-coverage-provider` (tests, installation `--no-save`) ; `ts-coverage` (coverage, Lcov) |
| `python` | `py-ruff` (lint, RuffJson) ; `py-tests` (tests) ; `py-coverage` (coverage, Lcov) |
| `dockerfile` | `docker-hadolint` (lint, HadolintJson, sur les fichiers suivis) |


## Décisions

- **DEC-1 - Crate feuille séparée de `vanyline-tools`**
    - **Context:** `vanyline-tools` est borné à 8 KiB pour les LLM ; la santé exige des sorties complètes et des types exportés en TypeScript.
    - **Decision:** `vanyline-health` est une crate feuille distincte (CON-1).
    - **Source:** SpecDD /health/src/lib.sdd, Purpose et Must.
- **DEC-2 - Un seul code de calcul pour local et central**
    - **Context:** les batteries de vérification dérivaient entre AGENTS.md, la CI et l'agent `validator`.
    - **Decision:** presets, parseurs, runner et `status()` vivent dans le crate ; les consommateurs lisent `HealthReport` sans parser de sortie brute.
    - **Source:** SpecDD /health/src/lib.sdd, Purpose ; SpecDD /health/src/report.sdd, Purpose ; `sandbox#BO-2`.
- **DEC-3 - Routes sous `/code-health`, distinctes de `/health`**
    - **Decision:** la santé du code est servie sous `/code-health`, `/health` étant la sonde du port d'exploitation.
    - **Source:** SpecDD /sandbox/src/health.sdd, Purpose ; `sandbox#BR-4`.
- **DEC-4 - État en mémoire, pas de persistance**
    - **Context:** l'état doit exister à tout instant ; aucune persistance n'est contractée.
    - **Decision:** l'état vit dans `CodeHealth`, champ d'`AppState`, et se recalcule au démarrage. Un registre statique global a été écarté (SOLDÉ 2026-10-09).
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles et Tasks `[-]` SOLDÉ ; `sandbox#BO-2`.
- **DEC-5 - Déclencheurs explicites et sondage de rattrapage**
    - **Context:** les éditions hors outils (terminal, éditeur externe) ne passent par aucun point notifié.
    - **Decision:** la liste des déclencheurs est exhaustive ; le sondage `health_poll_secs` rattrape le reste (FR-2, FR-3).
    - **Source:** SpecDD /sandbox/src/health.sdd, Handles ; Tasks `[x]` 2026-10-14 et `[-]` 2026-10-04.
- **DEC-6 - Toutes les vérifications rapportées, sans fail-fast**
    - **Context:** une commande cassée ne doit pas masquer les autres résultats.
    - **Decision:** exécution de toutes les vérifications, parallélisme borné, `nice -n 10` (FR-14).
    - **Alternatives:** arrêt au premier échec, écarté (SpecDD /health/src/checks.sdd, Tasks `[-]` 2026-10-04).
    - **Source:** SpecDD /health/src/runner.sdd, Handles.
- **DEC-7 - `errored` distinct de `failed` ; `skipped` réservé à « rien à vérifier »**
    - **Context:** un préalable mort n'est pas « rien à vérifier ».
    - **Decision:** un préalable `errored` fait passer le dépendant en `errored` sans lancement ; `skipped` seulement sans cible (par ex. aucun Dockerfile suivi).
    - **Source:** SpecDD /health/src/runner.sdd, Handles (arbitrage 2026-10-05) ; SpecDD /health/src/report.sdd, Exposes.
- **DEC-8 - Pas de mesure vaut `null`, jamais 0 %**
    - **Decision:** `coverage` est absente si aucune vérification de couverture n'a produit de données ; sans ligne instrumentée, `percent()` vaut `100.0`.
    - **Source:** SpecDD /health/src/report.sdd, Must ; Scenario `percent_sans_ligne_instrumentee`.
- **DEC-9 - Code neuf par diff de l'état du worktree, modèle de span assumé**
    - **Context:** `FileCoverage` ne porte que les lignes non couvertes ; la position des lignes couvertes n'est pas au contrat.
    - **Decision:** l'instrumenté est modélisé par `[1..span_end]`. Limite assumée : la couverture du neuf peut être surestimée sur une instrumentation très discontinue.
    - **Source:** SpecDD /health/src/diff.sdd, Returns (arbitrage 2026-10-05) ; SpecDD /health/src/runner.sdd, Handles.
- **DEC-10 - Base sticky à trois états**
    - **Decision:** champ absent, valeur ou `null` (double `Option` avec désérialisation dédiée) ; `null` n'est jamais refusé (FR-5).
    - **Source:** SpecDD /sandbox/src/health.sdd, Tasks `[x]` 2026-10-14.

- **DEC-11 - L'état unique `GET /code-health` et `POST /code-health/refresh` fait foi**
    - **Context:** `SpecDD /sandbox/src/lib.sdd` listait une table avec `POST /code-health/run`, `GET /code-health/runs/{run_id}`, `GET /code-health/latest` et un registre de runs ; le code (`sandbox/src/lib.rs`) et `SpecDD /sandbox/src/health.sdd` portent un état unique `CodeHealth` lu par `GET /code-health` et recalculé par `POST /code-health/refresh`. Les consommateurs (nœud `gate` d'`app`, relais, `mcp_domain`) n'appellent que ces deux routes.
    - **Decision:** le contrat est l'état unique de `health.sdd` ; la table de `lib.sdd` est corrigée en conséquence. Un registre de runs n'est pas promis.
    - **Consequences:** l'historique des runs n'existe pas ; s'il devient un besoin, il sera une évolution décidée à part.
    - **Source:** décision du développeur, 2026-11-07 ; `AGENTS.md` (« état toujours disponible, recalcul après un délai de calme »)

## Réalisation SpecDD

- `SpecDD /sandbox/src/health.sdd` : cycle de vie, routes, outil MCP (FR-1 à FR-12, DEC-4, DEC-5, DEC-10). Implémentation : `sandbox/src/health.rs`, tests `sandbox/tests/health.rs`.
- `SpecDD /sandbox/src/mcp.sdd` et `SpecDD /tools/src/mcp.sdd` : enregistrement de `code_health` (FR-9).
- `SpecDD /sandbox/src/lib.sdd` : câblage des routes. Réalisation contestée, voir OI-1.
- `SpecDD /health/src/runner.sdd` : FR-14, FR-15, FR-16, FR-18 (DEC-6, DEC-7). Implémentation : `health/src/runner.rs`.
- `SpecDD /health/src/checks.sdd` : FR-6, FR-7, FR-19. Implémentation : `health/src/checks.rs`.
- `SpecDD /health/src/report.sdd` : FR-10, FR-13, DEC-8. Implémentation : `health/src/report.rs` et `packages/protocol/src/generated/health-report.ts`.
- `SpecDD /health/src/diff.sdd` : FR-17, FR-18, DEC-9. Implémentation : `health/src/diff.rs`.
- `SpecDD /health/src/lcov.sdd`, `cargo_messages.sdd`, `tsc_text.sdd`, `ruff_json.sdd`, `hadolint_json.sdd` : FR-17. `SpecDD /health/src/error.sdd` : `VNL-HLT-001` à `008` (OI-2).
- `SpecDD /health/src/lib.sdd` : CON-1, CON-2, DEC-1, DEC-2. Implémentation : `health/src/lib.rs`.

## Acceptation et vérification

- **AC-1:** Au démarrage, `GET /code-health` répond 200 `computing` avec `report` et `summary` à `null` ; après le premier calcul, `fresh` avec `computed_at` (Scenarios `etat_toujours_disponible_des_le_demarrage`, `calcul_au_demarrage_puis_fresh`). Vérifie FR-1.
- **AC-2:** Une modification notifiée passe en `stale` et ne calcule qu'après le délai ; une rafale donne un seul calcul ; `cooldown` à 0 calcule tout de suite ; un succès seul notifie (Scenarios `modification_declenche_apres_le_delai_de_calme`, `rafale_de_modifications_un_seul_calcul`, `lsp_rename_notifie_en_succès_seul`). Vérifie FR-2.
- **AC-3:** Une empreinte identique ne déclenche aucun calcul ; le sondage détecte une édition externe ; un calcul échoué ne rend jamais `fresh` sur une empreinte divergente (Scenarios `empreinte_identique_aucun_recalcul`, `edition_au_terminal_detectee_par_sondage`, `calcul_echoue_ne_ment_jamais`). Vérifie FR-3, FR-11.
- **AC-4:** Un seul calcul à la fois ; `refresh` répond 202 et ignore le délai ; une modification pendant un calcul ne l'annule pas (Scenarios `un_seul_calcul_a_la_fois`, `refresh_force_ignore_le_delai`, `modification_pendant_un_calcul`). Vérifie FR-4.
- **AC-5:** Les trois états de `base` et le refus 400 se comportent comme FR-5 ; une `base` non vérifiable donne 500 sans modifier la base (Scenarios `base_*`, `git_non_lançable_rend_500`). Vérifie FR-5.
- **AC-6:** Un langage sans preset figure dans `unchecked_languages` (Scenario `langage_sans_preset_rapporte`). Vérifie FR-6.
- **AC-7:** Aucun `argv[0]` de preset n'est un shell (Scenario `argv_sans_shell`, SpecDD /health/src/checks.sdd). Aucun Scenario côté sandbox ne couvre l'absence de shell du calcul : partiel, voir OI-7 et Open Issues. Vérifie FR-7.
- **AC-8:** Les diagnostics LSP connus arrivent comme `preliminary_issues` avant les commandes (Scenario `diagnostics_lsp_transmis_au_calcul`). Vérifie FR-8.
- **AC-9:** `code_health` sans argument rend un texte borné ; `detail` et `path` filtrent (Scenarios `outil_mcp_rend_un_resume_borne`, `outil_mcp_filtre_par_chemin`). Vérifie FR-9.
- **AC-10:** `summary` est dérivé, `coverage_percent` vaut `null` sans couverture, `files` est trié (Scenarios `resume_*`). Vérifie FR-10.
- **AC-11:** 401 sans jeton, 403 pour un autre Owner, 404 pour `GET /health` sur le port principal (Scenarios `auth_exigee`, `ne_pas_confondre_avec_la_sonde`). Vérifie FR-12.
- **AC-12:** `status()` suit l'ordre `failed`, `errored`, `passed` ; `skipped` est neutre ; un rapport vide est `passed` (Scenarios `status_*`, `rapport_vide_est_passed`). Vérifie FR-13.
- **AC-13:** Toutes les vérifications sont rapportées ; le cascade `errored` ne lance rien ; au plus deux à la fois ; un groupe ne se chevauche jamais (Scenarios `prealable_errore_erore_le_dependant`, `parallelisme_borne`, `meme_repertoire_de_build_jamais_en_parallele`). Vérifie FR-14.
- **AC-14:** La sortie est plafonnée avec troncature en tête et marqueur unique ; `root` reste intact ; le timeout tue le groupe (Scenarios `sortie_plafonnee_tronquee_avec_marqueur`, `sortie_dans_out_dir_pas_dans_root`, `timeout_tue_le_groupe`). Vérifie FR-15.
- **AC-15:** Un code non nul avec sortie structurée donne `failed` ; une couverture vide sur succès donne `errored` (Scenarios `defauts_trouves_par_ruff_sont_failed_pas_errored`, `lcov_vide_sur_succes_est_erreur`). Vérifie FR-16.
- **AC-16:** Un lcov tronqué est rejeté ; un JSON illisible rend `InvalidCheck` ; le diff refuse noms cités et hunks mal formés (Scenarios lcov `rapport_tronque`, diff `nom_cite_refuse`, `hunk_mal_forme`, ruff `sortie_illisible`). Vérifie FR-17.
- **AC-17:** Le code neuf couvre les lignes commitées et non commitées ; un échec de diff ne touche pas aux autres résultats (Scenarios `code_neuf`, `code_neuf_inclut_le_non_committe`, `echec_du_diff`). Vérifie FR-18.
- **AC-18:** Les presets produisent les vérifications attendues dans l'ordre attendu, et les drapeaux partagés avec la CI sont identiques (Scenarios `preset_*`, `clippy_memes_drapeaux_que_la_ci`). Vérifie FR-19.

## Open Issues

- **OI-2 - Deux types `HealthError` et deux jeux d'identifiants.** La crate définit `HealthError::InvalidRef` en `VNL-HLT-007` (`health/src/error.rs`). Le sandbox définit son propre `HealthError` (`VNL-SBX-CH-001` à `006`, dont `RefCheckUnavailable`, `sandbox/src/health.rs`). `SpecDD /sandbox/src/health.sdd` écrit « corps = `@Display` verbatim » ; `CH-003` n'existe dans aucune spec de crate. **TBD :** nom et périmètre de chaque enum, identifiant rendu en HTTP.
- **OI-3 - Règle « ne pas écrire dans `root` » contre les outils.** `SpecDD /health/src/runner.sdd` (Must) interdit toute écriture dans `root`, mais `cargo` écrit `target/` par défaut et `ts-coverage-provider` lance `npm install` dans le workspace. `CARGO_TARGET_DIR` n'est pas contracté (runner, Tasks). **TBD :** la règle vise-t-elle `git status` seul, et où vont les artefacts de build ?
- **OI-4 - Réécriture de `after` après expansion.** `ts-coverage` dépend de `ts-coverage-provider`. Après expansion, les ids deviennent `<id>:<workspace>` (checks.sdd), mais la réécriture de `after` n'est spécifiée nulle part. **TBD.**
- **OI-5 - Statut du préalable `py-coverage`.** `SpecDD /health/src/checks.sdd` dit que `py-coverage` s'exécute « quand même » si `py-tests` a échoué ; `runner.sdd` cascade en `errored` tout préalable `errored`. **TBD :** quel statut du préalable est visé.
- **OI-6 - BR-7 contre `health_poll_secs = 0`.** `sandbox#BR-7` dit « toute modification » suit le délai de calme. Avec `health_poll_secs = 0`, une édition hors outils n'est jamais détectée. **TBD :** plancher de sondage, ou exception assumée au domaine ?
- **OI-7 - Déclencheurs git et couverture de l'absence de shell.** `commit`, `fetch` et `pull` ne figurent pas dans la liste exhaustive, alors que `HEAD` entre dans l'empreinte : un commit n'est vu qu'au sondage. Par ailleurs, aucun Scenario côté sandbox ne prouve l'absence de shell du calcul (AC-7). **TBD :** intentionnel ?
- **OI-8 - Nom d'opération `/ws/fs`.** `SpecDD /sandbox/src/health.sdd` (Handles) nomme `rename` dans la liste et `rename_file` dans le texte. **TBD :** lequel est le nom réel ?
- **OI-9 - Bornes de ressources non contractées.** `max_parallel` compte des commandes, pas des jobs de compilation ; aucune borne de jobs `cargo` ni de mémoire n'est contractée. `SpecDD /health/src/checks.sdd` dit que les trois jobs Rust « peuvent s'exécuter en parallèle ». **TBD.**
- **OI-10 - Rédaction des sorties.** `output_tail` (4096 octets) et les messages d'issues sont renvoyés au HTTP, au MCP et au central sans rédaction contractée. **TBD :** faut-il rédiger, et quoi ?
- **OI-11 - Artefacts partagés et collisions de tests.** `out_dir` (`<tmp>/vanyline-health/<n>`) est machine-global : deux `cargo test` parallèles peuvent se croiser (SpecDD /sandbox/src/health.sdd, Tasks `[?]`). Le drain des sorties attend l'EOF même si un descendant garde le flux ouvert (SpecDD /health/src/runner.sdd, Tasks `[?]`). **TBD.**
- **OI-12 - Chaîne d'outils et réseau.** `cargo llvm-cov` exige `llvm-tools` ; `ruff`, `hadolint`, `coverage`, `pytest` doivent être sur le PATH ; `ts-coverage-provider` exige un accès npm sortant, à valider sur `frontend/` (SpecDD /health/src/checks.sdd, Tasks `[?]`). La spec `toolchains/` n'existe pas. **TBD.**
- **OI-13 - Consommateur central et clôture.** Le service central de revue de MR n'est décrit par aucune spec lue ; le mode central demande une base par SHA (SpecDD /sandbox/src/health.sdd, Tasks `[ ]`). `Done when` de SpecDD /health/src/lib.sdd reste ouvert (clippy `-D warnings`, purge de `tooling.sdd`). **TBD.**

## Références

- `sandbox` (domaine) : `BO-2`, `BR-1`, `BR-4`, `BR-5`, `BR-7`, `BR-9`, `FR-1`, `FR-5`, `CON-3`, `OI-1` ; SpecDD /health/src/*.sdd, /sandbox/src/health.sdd, lib.sdd, mcp.sdd, /tools/src/mcp.sdd ; AGENTS.md, section Architecture.
- Non relus : `sandbox/src/config.sdd` (défauts `health_cooldown_secs` et `health_poll_secs`, repris de SpecDD /sandbox/src/health.sdd) et `sandbox/src/maint.sdd`.
