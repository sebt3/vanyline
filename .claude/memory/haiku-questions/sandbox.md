# Questions — crate `sandbox` (dette de tests, passe Haiku 2026-10-17)

Format : spec + ligne, Scenario concerné, ce qui bloque, observation, proposition.
Périmètre : `sandbox/src/**/*.sdd`, tâches `[ ]` de type test-debt. Aucun test écrit pour
les Scenarios dépendant de LSP réels, PTY, git, cluster, PostgreSQL ou d'un binaire externe.

## Règle TTD « vu rouge » (transverse)

- Tous les tests ajoutés ici visent un comportement DÉJÀ implémenté : ils passent à l'écriture.
  Le rouge n'a pas été observé (sauf indication contraire). Le rapport le signale par test.
  Proposition : accepter « vert à l'écriture » pour la dette de couverture d'un code existant,
  ou exiger une mutation temporaire du code par Scenario (décision Sébastien).

## config.sdd

- L123 `[ ] Remplacer le code existant par ce contrat` : le code (`sandbox/src/config.rs`) expose
  déjà les 17 champs du contrat, sans `oidc_*`, `no_auth`, `static_token`, `public_url`, et
  `parse_and_validate` est absent. Question : cette tâche est-elle close (à cocher) ou reste-t-elle
  ouverte pour une raison hors code (sondes controller, `/controller/src/sandbox.sdd`) ?
- L131 `[ ] sandbox_root` : le défaut `/home/vanyline/workspace` est déjà dans `config.rs`. Restent
  `/deploy/sandbox/sandbox-test.yaml` (pose `/workspace`) — hors crate, donc non touché.
- L133 `[?] Clone` : non tranché, non traité.
- L135 `[ ] Convertir chaque Scenario` : les 17 Scenarios ont un test tagué (`// Scenario: …`,
  nom identique). Aucun n'a été vu rouge (code déjà conforme). Non coché : « vu échouer » non
  satisfait. Décision demandée.

## venv.sdd

- L143 relecture Sébastien : non faite (hors périmètre Haiku).
- L145–167 `[?]` (garde PATH vide, vérification de `bin`, validation du contenu du marqueur,
  erreurs I/O silencieuses, non-UTF-8, paramètre de `spawn_shell`) : décisions attendues.
- L179 `[ ] Aucun test possible en-process pour le cas PATH non posé` : à trancher avant
  (processus enfant avec env épuré, ou décision). Non traité.
- Tests ajoutés : 9 Scenarios (`tmpdir_vide_overlay_vide` … `chemin_non_utf8_values_en_fffd`).
  `repertoire_venv_inaccessible_traite_comme_absent` suppose un processus non-root (mode 000) :
  en root le test échoue (le stat réussit). Conforme à la CI, à signaler.

## lib.sdd

- L210 cartographie : mapping non vérifié tag par tag (les tests existants ne portent pas les
  titres de Scenario ; ajouts faits sous les titres exacts).
- L226 conversion : ajoutés `routes_git_sans_auth_retourne_401`, `flush_sans_abonne_rend_faux_au_timeout`,
  `ids_de_flush_uniques_et_monotoniques_depuis_un`, `canal_fs_events_capacite_soixante_quatre`.
  Non fait, bloqué :
  - `middleware_metriques_compte_requete_sante_avec_labels` et `middleware_metriques_compte_les_chemins_inconnus` :
    le recorder Prometheus est un global (`OnceLock`) et le scénario lit le rendu de `build_ops_app`.
    Proposition : test dédié (binaire ou recorder local via `metrics::set_default_local_recorder`
    + `PrometheusBuilder::build_recorder`), décision sur le périmètre.
  - `ticket_vide_en_query_rejete_comme_invalide` : couvert en substance par
    `parametre_ticket_vide_rejete_en_002` (ws/ticket.rs) ; non dupliqué au niveau lib.
- L232 `[?]` cardinalité des labels `track_metrics` (path brut) : décision attendue.
- L274 `[?]` garde d'auth × 404/405 : à observer puis décider ; non testé.
- L278 `[?]` branche 503 de `metrics_handler` : non testable sans process dédié.
- L281 `[!]` `#![cfg_attr(test, allow(...))]` au crate root : exemption trop large, à décider.
- L285 `[?]` parsing maison de la query du ticket : non testé.

## mcp.sdd

- L177 relecture : non faite.
- L179 conversion : 16 Scenarios couverts (tests HTTP via `build_app` + jeton d'Owner de test).
- L189–215 `[?]` (id null, corps invalide, jsonrpc non vérifié, outil inconnu -32602,
  paramètre name non-chaîne, absence de session, cursor ignoré, label `method` non borné,
  notifications `method="notification"`) : décisions attendues. Tests écrits sur le comportement
  actuel conformément aux Scenarios ; à réviser si décision contraire.
- L209 `[ ] SUPPRESSION de oauth_metadata` : la route est déjà absente (test `aucune_decouverte_oauth`
  vert, 404). Vérifier s'il reste du code mort `oauth_metadata` dans le crate (tâche de code, non traitée).

## tools_impl.sdd

- L418 relecture : non faite.
- L421–479 `[?]`/`[!]` (unreachable!, expect sur JoinError, edits chevauchants, URI non-file://,
  timeout execute_command, 0/1-based VNL-SBX-LSP-007, messages LSP-006, rendu des chemins,
  TOCTOU confine_path, coût confine, champs ignorés en mode write/edit) : décisions attendues.
- L482 `[ ] Dériver un test par Scenario` : 8 Scenarios ajoutés
  (`lsp_tools_declarent_huit_outils`, `arguments_invalides_rendus_erreur_outil`,
  `nom_etranger_a_la_famille_rend_none`, `workspace_edit_partiel_erreur_laisse_fichiers_ecrits`,
  `textedit_range_manquant_leve_008`, `hover_bloc_vide_et_contenu_null_sans_section`,
  `resolve_position_symbole_ambigu_note_seconde_occurrence`).
  NON FAIT : `references_cap_vingt_fichiers_rend_les_restes_nus` — dépend d'un serveur LSP
  (fakes `lsp_test_fakes` existants, à évaluer) et d'un `tracing::warn!` à capturer.
  Les deux autres items (`textedit_range…`, `hover_…`, `resolve_position…`) sont « à recadrer »
  selon la spec : les tests ajoutés les vérifient directement.
- L520 `[ ] spec de ./lsp.rs` : tâche de spec, hors périmètre Haiku.

## ws/ticket.sdd

- L280 cartographie : les tests existants portent des noms différents des Scenarios (ex.
  `ticket_redeem_returns_claims` ↔ `ticket_rachete_rend_les_claims_emis`). Non renommés ;
  les Scenarios sont couverts par des tests tagués ajoutés.
- L288 conversion : 13 Scenarios ajoutés (`format_ticket_…`, `emissions_successives_…`,
  `rachats_paralleles_…`, `ttl_et_default_…`, `store_clone_…`, `requete_sans_ticket_…`,
  `ticket_inconnu_…`, `parametre_ticket_vide_…`, `parametre_ticket_duplique_…`,
  `corps_401_…`, `rejets_inconnu_…`, `ticket_valide_consomme_…`, `claims_valides_…`,
  `aucun_log_du_module_…`). Le Scenario `ticket_expire_…` est tagué (nouveau test).
- L300 `[ ] Fuite du ticket dans les logs par TraceLayer` : décision maintenue dans la spec,
  hors test.

## ws/fs.sdd

- L308 relecture : non faite. L310 cartographie : non faite (noms existants, mapping à vérifier).
- L322 conversion : Scenarios de la boucle `fs_session` (relais push, multi-session, Lagged,
  close, Ping/Pong) exigent un harnais WS réel absent du crate. Non traité.
  Les Scenarios de dispatch direct (`dispatch_fs_message`) sont couvrables : non traités dans
  cette passe, à reprendre.
- L327 `[!]` faux positifs `"truncated"` par sous-chaîne, `[!]` Ping/Pong : décisions attendues.
- Plusieurs `[?]` (TicketQuery mort, ordre `bogus`/`missing path`, absence d'identifiant
  d'erreur VNL-SBX-WS-0xx) : décisions attendues.

## telemetry.sdd

- L205 `[ ] Ajouter les tests du module` : seul le Scenario `resource_otel_porte_seulement_nom_et_version_du_crate`
  est ajouté (test unitaire, `build_resource`). Les 10 autres touchent les globaux du process
  (`OnceLock`, subscriber global, `RUST_LOG`) et exigent des binaires de test dédiés (sous-processus).
  Non traités. Proposition : `sandbox/tests/telemetry_*.rs` lancés via `CARGO_BIN_EXE_…` ou
  sous-processus.
- L209 `[?]` deux `eprintln!` (violation `print_stderr` deny) : décision (exemption ou `tracing::warn!` dans shutdown_otel).
- L215–260 `[?]` (commentaire allow, RUST_LOG invalide, ordre init, idempotence, double init,
  rt-tokio, shutdown sur `?`, testabilité stderr, OTEL_* standard) : décisions attendues.

## main.sdd

- L280 `[ ] Trancher chaque entrée [?]` : décisions attendues avant toute tâche `[x]`.
- L328 `[ ] Harnais de tests de ./main.rs` : proposition `sandbox/tests/main_startup.rs`
  (lancement du binaire via `CARGO_BIN_EXE_vanyline-sandbox`, 15 Scenarios). Non écrit :
  la spec exige que les `[?]` soient tranchés avant le harnais, et plusieurs Scenarios
  exigent des ports sondés et une fixture TLS (`tls_paire_valide_sert_https`).
- L349 `[ ] Convertir en tests` : bloqué par L328.

## maint.sdd (et bin/maint.sdd)

- L499–520 : Scenarios git (bare repos, worktrees, shims PATH, `core.quotePath`) et Scenarios `cluster_`
  (harnais cluster absent du crate). Non traités : git = bloqué par la règle « git → question ».
  Question : le harnais cluster doit-il être créé dans cette passe ? (Non : tâche séparée.)
- L520 cartographie des 17 tests de `maint.rs` : non vérifiée.
- bin/maint.sdd L158 `Convertir chaque Scenario ci-dessous` : non analysé dans cette passe
  (utilitaire `vanyline-maint`, dépend de git/fichiers). À reprendre.

## git.sdd

- L418 cartographie, L427 conversion (« la majeure partie » des Scenarios dépend de git réel) :
  non traités, règle git → question.

## lsp.sdd / lsp_client.sdd / ws/lsp.sdd / bin/ruff_lsp.sdd / bin/hadolint_lsp.sdd

- lsp.sdd L536 : ~90 tests existants à re-tagger, Scenarios LSP réels (serveurs rust-analyzer,
  pyright, tsserver, ruff, hadolint). Non traités.
- lsp_client.sdd L353 `Convertir chaque Scenario` : dépend de fakes/serveur LSP, et de `[!]`
  (initialize ne teste que result ; flag `initialized` consommé avant envoi). Non traité.
- ws/lsp.sdd L334/L351 : LSP + WebSocket. Non traités.
- bin/ruff_lsp.sdd L351, bin/hadolint_lsp.sdd L280 (`Lacunes de couverture`) : binaires
  externes (ruff, hadolint). Non traités.

## ws/terminal.sdd

- L327 cartographie et conversion : PTY (`portable-pty`) et sessions shell. Non traités.

## jobs.sdd, auth.sdd, health.sdd

- jobs.sdd : conversion déjà cochée `[x]` (2026-10-10). Restent L122 (`restate_ingress_url`
  côté config) et L126 (callback awakeable, prérequis amont) : tâches de code, non traitées.
- auth.sdd : `[x]` déjà.
- health.sdd : pas de tâche de test-debt dans la spec. L253/L255 : tâches de code/images, non traitées.

## État final de la passe (reprise après crash)

Validations réelles (CARGO_TARGET_DIR=$HOME/.cache/cargo-target/vanyline) :
- `cargo test -p vanyline-sandbox` : EXIT=0, 418 passés, 2 ignorés (voir ci-dessous).
- `cargo clippy -p vanyline-sandbox --all-targets -- -D warnings` : EXIT=0.
- `cargo fmt -p vanyline-sandbox -- --check` : OK.
- `specdd lint` : 0 errors, 0 warnings.

Tests ignorés, écarts documentés (pas de faux vert) :
- `tools_impl::tests::lsp_tools_edit_and_check_mentionne_lsp_006` : la spec exige VNL-SBX-LSP-006
  dans la description de chaque outil ciblé ; `edit_and_check` ne le cite pas. Le test de
  `lsp_tools_declarent_huit_outils` exclut donc `edit_and_check` de cette assertion. Décision attendue :
  corriger la description (code) ou assouplir la spec.
- `venv::tests::repertoire_venv_inaccessible_traite_comme_absent` : le processus d'exécution possède
  CapEff=000001ffffffffff (capacités complètes, DAC contourné) ; un répertoire mode 000 reste lisible.
  Le scénario ne peut donc pas être vérifié ici. Marqué `#[ignore]` ; à lancer en non-root sans CAP_DAC_OVERRIDE.

Correction faite pendant la passe : `ws::ticket::tests::aucun_log_du_module_ne_fuit_la_valeur_d_un_ticket`
attendait 401 avec un ticket VALIDE ; la garde le laisse passer (rejet 400 de l'extracteur WS). Assertion
corrigée en `!= 401`.

Tâches cochées dans les specs (seules celles entièrement couvertes) :
- venv.sdd L171 (dérivation des 9 tests) et L184 (cartographie) ; mcp.sdd L179 (16 scénarios couverts) ;
  ws/ticket.sdd L288 (13 scénarios couverts).
Non cochées : config.sdd L135 (« vu échouer » non observé), lib.sdd L226 (2 scénarios metrics bloqués),
tools_impl.sdd L482 (`references_cap_vingt_fichiers_rend_les_restes_nus` dépend de LSP), et toutes les autres.
