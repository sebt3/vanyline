# Questions — crate `vanyline-lib` (tests de dette, passe Haiku)

Périmètre : `lib/src/**/*.sdd`. État : 155 tests unitaires verts (`cargo test -p vanyline-lib`),
clippy `--all-targets -D warnings` vert, fmt vert. `k8s` non traité (voir Q6).

## Point transversal — TTD sur code existant

Mêmes remarques que `tools.md` : les nouveaux tests caractérisent un code conforme ;
aucun vu rouge avant implémentation. Deux Scenarios sont contredits par le code (Q2, Q3)
et n'ont pas été écrits en faux.

## Q1 — Mock HTTP loopback (model.sdd l.210, 228, 240, 247)

- Scenarios : `build_ollama_model_cle_vide_vaut_absence_de_auth`,
  `build_openai_compat_model_sans_cle_envoie_bearer_vide`,
  `build_openai_compat_model_base_url_suffixee_v1`,
  `build_openai_compat_model_slashes_de_queue_rognes`.
- Bloque : ils exigent un serveur HTTP mock (axum, dev-dep présente) sur loopback pour
  inspecter les en-têtes et chemins. Le prompt interdit les Scenarios « dépendants du réseau »
  sans accord.
- Proposition : accepter un mock loopback 127.0.0.1 (pas de réseau sortant), ou
  accepter de ne pas les couvrir.

## Q2 — Clé non ASCII contredit le code (model.sdd l.235)

- Scenario : `build_openai_compat_model_cle_invalide_en_en_tete_rejetee` (`"clé"` → `Err` VNL-LLM-006).
- Observé : le build réussit (`Ok`). Le test a été écrit, a échoué sur `unwrap` du `Err`,
  puis retiré. `HeaderValue` accepte les octets ≥ 0x80 ; seuls les caractères de contrôle
  sont rejetés (cas ollama `"cla\nir"`, vérifié vert).
- Proposition : corriger le Scenario (rejet des seuls caractères de contrôle) ou
  décider que les non-ASCII doivent être rejetés (changement de code, hors Haiku).

## Q3 — Scenarios de `stream_agent_events` (event.sdd l.323, 330, 337)

- `tour_reussi_diffuse_en_ordre_et_finit_par_done`, `erreur_de_stream_emet_error_sans_done`,
  `stream_epuise_sans_final_response_finit_par_done`.
- Bloque : demandent un modèle rig mock (`CompletionModel`) pour alimenter `stream_agent_events`.
- Proposition : écrire un mock minimal de `CompletionModel` dans le module de test
  (sans réseau), ou valider qu'il est acceptable.

## Q4 — Scenarios de tour et de MCP (session.sdd)

Non écrits, non consignés individuellement comme bloquants ; certains sont purs
(candidats au prochain passage) :
- `override_modele_selectionne_autre_profil` (l.365), `override_modele_inconnu_rejete` (l.371),
  `index_skills_resolu_suite_selection_auto` (l.376), `arc_tool_dyn_transmet_identite_et_appel` (l.386),
  `contexte_clone_partage_todo_state` (l.416) — candidats : tests sur `resolve_turn_context`,
  `ArcToolDyn`, `SessionContext`.
- `serveur_mcp_extra_contacte_pendant_tour` (l.391), `serveur_mcp_injoignable_emet_tool_unavailable` (l.397),
  `serveur_mcp_absent_du_store_sans_tool_unavailable` (l.403), `extra_mcp_gagne_en_collision_de_nom` (l.409) :
  demandent des serveurs MCP réels ou simulés (voir Q5).
- `agent_inconnu_tour_sans_evenement` (l.421), `listing_agents_en_erreur_avorte_tour_avant_stream` (l.427) :
  tour complet à vérifier.

## Q5 — Serveurs MCP réels pour `prefixed_mcp` et `session`

- `prefixed_mcp.sdd` l.197 `prefixed_tool_nom_prefixe_definition_origine`, l.206
  `deux_appels_sequentiels_nom_origine_sur_le_fil`, l.214 `is_error_textes_joints_tool_call_error`,
  l.246 `serveur_injoignable_impacte_sa_seule_selection`, l.262 `en_tete_invalide_ecarte_sans_avorter`,
  l.287 `list_mcp_server_tools_noms_origine_sans_filtre` — demandent un serveur MCP
  (process ou HTTP). Les tests existants (`test_mcp_tool_survives_multiple_calls_in_same_connection`,
  `test_mcp_custom_headers_reach_server`) en utilisent déjà : à confirmer comme pattern accepté.
- Proposition : réutiliser le harnais des tests d'intégration `lib/tests/` pour les scénarios restants.
- Note : 6 tests de `prefixed_mcp` ont été renommés en titres de Scenario
  (`tool_matches_*`, `selected_servers_*`, `select_local_tools_trouves_absents_dans_ordre`).

## Q6 — Module `k8s` (k8s.sdd) non traité

- Compilation avec `--features k8s` (lourde) et la plupart des Scenarios exigent un cluster
  (`owner_cycle_complet`, `project_cycle_complet`, `sandbox_cycle_complet`, `discover_*`,
  `list_isolee_par_namespace`, `set_sandbox_suspended_*`, `watch_*`).
- Candidats purs (sans cluster) : `sandbox_mcp_url_*`, `sandbox_ws_ticket_url_*`,
  `sandbox_git_url_*`, `owner_token_*` (à partir de l'équivalent existant), `watch_replay_*` si le
  flux est simulable.
- Proposition : passe dédié avec `--features k8s`, cluster exclu.

## Q7 — Scenarios de tour imbriqué (builtin/task.sdd l.257, 267)

- `call_succes_emet_start_enveloppes_end`, `call_tour_imbrique_en_erreur_emet_end_erreur` :
  exécutent un tour complet de subagent. Bloqués par le même besoin que Q3 (modèle mock).

## Q8 — Scenarios de surface (lib.sdd l.240-269, error.sdd l.236)

- `surface_racine_domain_store_canoniques`, `surface_racine_glob_error_types_prefixed_mcp`,
  `surface_racine_prefixed_mcp_double_chemin`, `module_k8s_absent_sans_feature`,
  `module_k8s_present_avec_feature`, `variantes_k8s_reservees_a_la_feature` (error.sdd) :
  contrôles de compilation (`cargo check`, avec et sans `k8s`). Non écrits.

## Q9 — Tâches cochées ou non

- Aucune tâche `[x]` cochée : AGENTS.md exige la synthèse `validator` verte avant `[x]`,
  et plusieurs tâches restent partiellement couvertes (Q1–Q8).
- Candidates à `[x]` après synthèse validator : `types.sdd` « Aucun test n'existe… » (13/13),
  `builtin/todo.sdd` « Dériver un test par Scenario » (17/17),
  `builtin/skill.sdd` « Dériver… » et « Ajouter les assertions de MESSAGE » (10/10, messages vérifiés).

## Q10 — Rappel d'arbitrage sur l'orchestration

- Le prompt demande de cocher `[x]` dès que les tests sont verts ; AGENTS.md exige la
  synthèse validator. Décision laissée au développeur principal.
