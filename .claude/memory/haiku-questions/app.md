# Questions Haiku — crate `vanyline-app` (dette de tests, passe 2026-10-17)

Périmètre : tâches `[ ]` de type test-debt dans `app/src/**/*.sdd`. Hors périmètre (non traité) :
BUG DE SÉCURITÉ, alignement « Owner = namespace », workflow, tâches de code.

## Q1 — `error.sdd` `conversion_dberr_vers_internal_error_prefixe_vnl_db_006` : Display de `DbErr::Custom`
- Spec : `app/src/error.sdd`, Scenario `conversion_dberr_vers_internal_error_prefixe_vnl_db_006`.
- Bloque : le Scenario attend `VNL-INT-001: Internal error: VNL-DB-006: x` pour `DbErr::Custom("x")`.
- Observé : le `Display` réel de `sea_orm::DbErr::Custom` ajoute le préfixe `Custom Error: ` →
  `VNL-INT-001: Internal error: VNL-DB-006: Custom Error: x`. Le code (`From<DbErr>`) est correct.
- Traitement : le test construit l'attendu depuis le `Display` réel de `DbErr` (pas en dur) et vérifie le double marqueur.
- Proposition : corriger le Scenario (`… VNL-DB-006: Custom Error: x`) ou fixer un autre `DbErr` d'exemple.

## Q2 — `error.sdd` `display_exhaustif_des_variantes_apperror` / `bareme_exhaustif_statuts_http_des_variantes` : `SandboxRelayUnavailable` absente de la spec
- Spec : `app/src/error.sdd`, les deux Scenario exhaustifs (énumérations `Exposes`).
- Bloque : la spec parle de 30 variantes et n'en liste pas `SandboxRelayUnavailable` (`VNL-SBX-017`, 502, défini dans `error.rs`, doc renvoyant à `./api/sandboxes.sdd` §Must « Erreurs du RELAIS opaques »). L'enum en compte 31.
- Traitement : le test inclut la variante (rendu `VNL-SBX-017: sandbox unavailable`, statut 502). Les deux Scenario sont donc écrits sur 31 variantes.
- Proposition : ajouter la variante à `Exposes` et au barème (502) de `error.sdd`.

## Q3 — `error.sdd` `ws_ne_render_jamais_apperror_en_http` : non écrit
- Bloque : le Scenario exige une capture `tracing` (log `ws chat error: …` / `ws sandbox-state error: …`) et un handshake WS simulé. Aucune capture de logs n'existe dans les tests du crate (`tracing-subscriber` est en dépendance normale, non vérifié plus avant).
- Proposition : décider le harnais (dev-dep de capture, ou assertion sur une fonction extraite). Ne pas toucher au code WS sans accord.

## Q4 — `error.sdd` tâche « Combler les tests du module » laissée `[ ]`
- 29 Scenario couverts ; reste Q3 (WS). Le « Done when » exige aussi que Sébastien ait tranché tous les `[?]`/`[!]` du fichier : hors de portée d'une passe test.
- Tests écrits sur des comportements déjà implémentés : passent à l'écriture, pas de rouge observé (cf. règle transverse). Signalé ici.

## Q5 — `api/local_tools.sdd` Scenario `un_descripteur_sans_nom_ou_description_est_silencieusement_elimine` : non écrit
- Bloque : la boucle d'aplatissement est collée à `flatten_local_tools` ; le Scenario demande un « refactor testable nécessaire » (extraction d'une fonction sur un `serde_json::Value` simulé). C'est une modification de production non autorisée dans ma passe.
- Proposition : autoriser l'extraction (ex. `fn flatten_descriptors(iter) -> Vec<LocalTool>`) dans une tâche de code.

## Q6 — `api/local_tools.sdd` Scenario `list_local_tools_authentifie_repond_200_et_tableau_de_paires` : spec inexacte sur l'auth
- Spec : « porteur valide obtenu par `miryad_core::auth::issue_token` (précédent `bearer_for`) ».
- Observé : l'extracteur `miryad_core::auth::AuthUser` (utilisé par `list_local_tools`) lit **uniquement le cookie de session** (`auth/middleware.rs` de miryad-core 0.1.5) ; un `Authorization: Bearer` renvoie 401. Le précédent `bearer_for` passe par `resource_router` (extracteur dual `AuthPrincipal`), pas par `AuthUser`.
- Traitement : le test scelle un cookie de session avec `miryad_core::auth::cookie::build_set_cookie` (même clé que `make_app`, id_token factice avec `exp` en 2100). Test vert.
- Proposition : corriger la spec (cookie de session, pas porteur) — ou décider si `list_local_tools` doit passer à l'extracteur dual (c'est alors un changement de code).

## Q7 — `api/local_tools.sdd` tâche « Convertir les 4 Scenario À ÉCRIRE » laissée `[ ]`
- 3 sur 4 écrits et verts (ordre du flatten, 200 authentifié, sérialisation de `LocalTool`). Reste Q5.
- La tâche « Relecture par Sébastien » reste `[ ]` (spec en reverse-engineering).

## Q8 — `db/entities/chat_contexts.rs` : renommage d'un test existant
- Le test `chat_context_roundtrip_serializes` a été renommé `chat_context_model_roundtrip_serde` pour porter le titre exact du Scenario. Aucune logique changée. Signalé pour relecture.

## Q9 — `db/entities/messages.rs` Scenario « aucune relation ni hook déclarés »
- Seule la partie relations est testée (`Relation::iter().count() == 0`). La partie « hook » (`ActiveModelBehavior` vide) n'a pas d'assertion exécutable propre. Scenario considéré couvert sur la partie testable seulement.

## Q10 — Tests écrits sur comportements existants (règle « vu rouge »)
- `skills.rs` : `skill_payload_minimal_deserialise_avec_defauts`, `skill_payload_sans_name_rejete`, `skill_round_trip_serialisation_fidele`.
- `toolsets.rs` : `minimal_creation_payload_deserializes_with_defaults`, `model_serializes_with_exact_snake_case_keys`, `entity_declares_no_relations`.
- `messages.rs` : `message_forme_json_de_fil`, `message_aucune_relation_ni_hook`, `message_insert_relecture_reelle`.
- `chat_contexts.rs` : `chat_context_model_se_serialize_en_quatre_cles_snake_case`, `chat_context_relation_enum_vide`.
- `local_tools.rs` : `flatten_ordre_filesystem_puis_search_puis_commande`, `local_tool_serialize_en_name_et_description`, `list_local_tools_authentifie_repond_200_et_tableau_de_paires`.
- `error.rs` : tous les tests `display_*`, `bareme_*`, `corps_json_*`, `conversion_*`, `heuristic_*`, `source_*`, `marqueurs_*`, `unprocessable_reference_rend_*`.
- Ce sont des tests de caractérisation : ils passent à l'écriture, le rouge n'a pas été observé. Signalé pour relecture.

## Q11 — Non traités dans cette passe (hors périmètre accepté ou non attaqués)
- Migrations (`migration/*.sdd`) : tâches DDL exigeant PostgreSQL réel (FK, RESTRICT, `JSONB`, `down`) — à consigner en question côté spec, non tentées.
- `api/mcp_servers.sdd` (12 Scenario « À ÉCRIRE »), `config_store.sdd` (méthodes), `api/owners.sdd`, `api/projects.sdd`, `ws/sandbox_state.sdd`, `ws/chat.sdd`, `db/entities/mod.sdd` (`[ ]` de structure) : non attaqués dans cette passe.
- `k8s.sdd`, `main.sdd` (démarrage, codes de sortie) : dépendent d'un cluster ou du réseau — hors périmètre.

---

# Passe 2 (2026-10-17) — specs non attaquées de `app`

Règle : tests écrits uniquement pour les Scenario purs (sans cluster, sans PostgreSQL). Aucun test
ne verrouille un comportement bogué ou soumis à une décision `[!]`/`[?]`. Tâches `[ ]` NON cochées
(périmètre partiel).

## Écrits (8 nouveaux tests, 4 renommés)

- `api/owners.rs` : `sanitize_owner_name` — 9 Scenario couverts (4 tests existants renommés sur leurs
  titres : `local_part_d_email_est_assaini_en_etiquette_minuscule`, `email_a_local_part_vide_retombe_sur_le_repli_owner`,
  `local_part_plus_long_que_63_caracteres_est_coupe_a_63`, `la_coupe_a_63_caracteres_ne_finit_jamais_par_un_tiret`).
  Tâche `[ ]` « Dériver chaque Scenario » NON cochée : reste `resolve_owner_name`, `ensure_owner` (base + Kubernetes,
  `[!]` collision de noms / course CR).
- `api/mcp_servers.rs` : 3 des 12 Scenario « À ÉCRIRE » : `build_domain_server_elimine_les_valeurs_de_header_non_chainees`,
  `build_domain_server_headers_non_objet_donne_une_map_vide`, `mcp_test_result_serialize_en_un_tableau_sous_la_cle_tools`.
  Tâche `[ ]` NON cochée.

## Q12 — `build_domain_server_elimine_les_valeurs_de_header_non_chainees` : réserve de relecture
- La spec marque l'élimination silencieuse des headers non-chaînes comme comportement accidentel à relire
  (tâche « Relecture par Sébastien » de `api/mcp_servers.sdd`). Test écrit car Scenario clair ; à reconsidérer
  si la relecture tranche autrement (ex. warn + skip).

## Q13 — `api/mcp_servers.sdd` : 9 Scenario « À ÉCRIRE » non écrits
- `test_server_non_admin_recoit_403_et_ne_connecte_rien`, `test_server_ligne_absente_repond_502_vnl_mcp_002`
  (le Scenario attend 502 « comportement actuel, PAS 404 — cf. Tasks [!] » : écrire le test reviendrait à
  verrouiller un choix ouvert), `..._server_type_valide_a_la_creation_puis_invalide_..._vnl_mcp_003` (corruption
  via update, base réelle), `test_server_transport_sse_repond_502_vnl_mcp_004_sans_ecrire`,
  `test_server_transport_http_injoignable_repond_502_vnl_mcp_005`, `test_server_decouverte_reussie_..._200`
  (faux serveur MCP in-process), `test_server_pair_silencieuse_...` (timeout 11 s, `[!]` timeout non tranchée),
  `test_server_id_non_numerique_repond_400_...`. Proposition : écrire après décision `[!]` sur 404/502 et timeout.

## Q14 — `api/owners.sdd` : Scenario DB/Kubernetes non écrits
- `resolve_owner_name_*` (3), `ensure_owner_*` (5), `la_spec_de_l_owner_cr_...`, `project_defaults_...`,
  `appels_simultanes_...`. Bloqués : base réelle (sqlite possible) et/ou client Kubernetes (`ensure_owner`
  « sans toucher Kubernetes » exige un faux client ou un refus de conception). `[!]` collision de noms et course
  CR à trancher par Sébastien avant tout test de `ensure_owner`.

## Q15 — `config_store.sdd` : tâche `[ ]` « Écrire les tests des Scenario de méthode » NON traitée
- 22 Scenario sans test sous le titre exact (huit lectures de `PgConfigStore`, défauts hérités, lignes poison).
  Faisable en `real_db()` (sqlite) avec `Entity::insert` pour contourner les hooks. Non entrepris dans cette
  passe (volume). À reprendre en passe dédiée ; attention aux Scenario marqués `[!]`/`[?]` du fichier.

## Q16 — `api/conversations.sdd` : tâche `[ ]` « Lacune de couverture » NON traitée
- 17 Scenario sans test (liste authentifiée, filtre sandbox, 201/204/403/404/422, `VNL-DB-007`). Harnais
  `@real_db` + `@issue_token` possible (précédent `db/entities/agents.rs`) mais exige la forme de réponse et le
  barème 403/404/204 « à confirmer avant de figer » (Q du fichier). Non entrepris.

## Q17 — Tâches `[ ]` de `ws/chat.sdd`, `ws/sandbox_state.sdd`, `k8s.sdd`, `main.sdd`, `api/projects.sdd`, `api/sandboxes.sdd`
- Tests WS live, clients Kubernetes (découverte, cache), démarrage (PG/réseau), ou tâches bloquées par `[!]`.
  Conventions de la spec : intégration manuelle ou `@real_db`. Non entrepris dans cette passe.
- `api/sandboxes.sdd` `Erreurs du relais` (`error_for_status()?` / `RequestError`) : tâche de code (hors périmètre).
- `api/sandboxes.sdd` « Écrire les tests de chaque Scenario » : préalables `[!]` non levés.

## Q18 — Tests-tagging seul (non prioritaires)
- `system_agents.sdd` : `seul_le_modele_est_modifiable`, `suppression_refusee`, `creation_client_avec_cle_systeme`
  (hooks de `db/entities/agents.rs`, déjà testés sous d'autres noms, aucune tâche `[ ]`) ; `parametres_exige_l_agent_systeme`
  (création de conversation `settings`, `ws/chat`).
- `config.sdd` : `moteur_workflow_*` (3) déjà couverts par `from_env_workflow_*` ; pas de tâche `[ ]`. Renommage non fait.
- `workflow/status.sdd` `restate_indisponible`, `workflow/steps/shell.sdd` `echec_toleré` : non tagués, pas de tâche `[ ]`.
- Hors périmètre demandé : `api/workflows.sdd`, `mcp_domain.sdd`, `workflow/*` (couverts par tests de même logique ; tagging non fait).

## Q19 — Hors périmètre (rappel)
- BUG DE SÉCURITÉ (`conversations`, `llm_providers`, `mcp_servers` headers, `agents` résolution, `chat` résolution),
  alignement Owner=namespace, features, migrations PostgreSQL : non traités, aucun test verrouillant le comportement bogué.

---

# Passe 3 (2026-10-17) — `config_store.sdd` et `api/conversations.sdd`

Règle : tests uniquement pour les Scenario sans bug de sécurité confirmé, sans posture `[!]` ni
arbitrage `Tasks` ouvert, et sans barème 403/404/204 incertain. Aucun test ne verrouille de
comportement bogué. Tâches `[ ]` NON cochées (périmètre partiel).

## Écrits — `config_store.rs` (14 tests, base sqlite via `real_db`, inserts `Entity::insert`)

`domain_provider_type_inconnu_est_erreur`, `list_providers_retourne_le_domaine_sans_filtrer_owner`
(partiel, voir Q21), `erreur_requete_remappe_en_vnl_cfg_001` (sqlite sans table),
`list_models_resout_le_provider_par_nom_table_globale`, `list_models_traverse_un_type_de_provider_inconnu`,
`list_mcp_servers_saute_le_type_inconnu_sans_erreur`, `list_mcp_servers_filtre_les_headers_non_string`,
`list_toolsets_et_list_skills_sont_infaillibles`, `list_agents_skill_selection_auto_none_named`,
`les_quatre_tables_owner_filtrees_isolent_les_owners`, `load_skill_dun_autre_owner_rend_vnl_cfg_003`,
`default_agent_ne_designe_jamais_rien`, `get_skill_rend_les_meta_pas_le_corps`,
`get_mcp_server_sur_ligne_sautee_rend_vnl_cfg_003`.

## Écrits — `api/conversations.rs` (8 tests, dans `mod tests`, cookie scellé pour les routes HTTP)

`liste_authentifiee_filtre_sur_owner_et_tri_par_updated_at_descendant`,
`liste_filtree_par_sandbox_name_selon_kind_et_valeur_exacte`, `filtre_sandbox_name_exclut_casse_et_type_differents`,
`creation_avec_agent_connu_retourne_201_et_cree_contexte_et_conversation`,
`creation_sans_agent_name_retourne_201_avec_agent_name_null`,
`creation_avec_agent_inconnu_retourne_422_sans_aucune_ecriture`,
`liste_d_un_owner_sans_conversation_retourne_tableau_vide`,
`messages_tries_en_six_cles_brutes_ou_tableau_vide`.
Le test `creation_contexte_settings_amorce_les_agents_systeme` existait déjà (l.455) : non dupliqué.
`id_regression` restauré à l'identique à HEAD (vérifié : `git diff` sans suppression).

## Q20 — BUG DE SÉCURITÉ `config_store` : `list_agents_profil_dun_autre_owner_est_saute_sans_empoisonner_la_liste` NON écrit
- Le Scenario décrit le comportement CORRIGÉ (saut de l'agent au profil cross-owner) ; le code actuel
  empoisonne toute la liste (cf. `config_store.sdd` tâche « BUG CONFIRMÉ » et `[!]` posture). Écrire le
  test maintenant le ferait échouer et figerait une attente non implémentée. Consigné, à traiter avec le bug.

## Q21 — `list_providers_retourne_le_domaine_sans_filtrer_owner` : assertion `api_key` omise
- Le Scenario exige `api_key` en clair : c'est l'exposition consignée dans `llm_providers.sdd` (bug de
  sécurité). Le test vérifie nom, endpoint, type et l'absence de filtre owner, pas la valeur de `api_key`.

## Q22 — `config_store.sdd` Scenario posture tout-ou-rien (`[!]`) NON écrits
- `list_providers_type_inconnu_empoisonne_toute_la_liste`, `ligne_poison_fait_echouer_get_provider_valide`,
  `list_agents_mode_invalide_fait_echouer_toute_la_liste` : la spec dit la posture « tout-ou-rien » non
  tranchée (`[!]`, `config_store.sdd` l.378-380). Décision Sébastien à obtenir avant de figer.

## Q23 — `config_store.sdd` arbitrages `Tasks` NON écrits
- `list_models_max_tokens_negatif_bascule_en_u64` : `as u64` qui wrappe en silence, « arbitrage Tasks ».
- `load_skill_retourne_le_corps_brut_sans_trim` : divergence avec `fs_store` (trim), « arbitrage Tasks ».

## Q24 — `cfg_err_produit_un_double_marqueur_verrouille` NON écrit
- Verrouille le double marqueur `VNL-CFG-001: … VNL-LLM-005: …`, politique encore `[?]` dans
  `cfgstore/src/error.sdd` et `app/src/error.sdd` (double marqueur assumé ou dette). Ne pas verrouiller avant arbitrage.

## Q25 — `config_store.sdd` `dix_neuf_ecritures_rendent_vnl_cfg_009` NON entrepris
- 19 appels d'écriture à construire un par un (volume), à faire en passe dédiée.

## Q26 — `api/conversations.sdd` Scenario à barème 403/404/204 incertain : NON écrits (consigne demandée)
- `lecture_unitaire_retourne_404_403_ou_200_selon_appartenance` (403 vs 404 : oracle `[!]`, « comportement
  verrouillé tel quel, cf. Tasks [!] », `conversations.sdd` l.146-149, 186 « RISQUE ACCEPTÉ » à confirmer).
- `suppression_retourne_204_supprime_en_cascade_les_messages_et_le_404_suivant` (204 + cascade + contexte
  résiduel documenté `[!]`, l.244-248).
- `messages_d_autrui_retourne_403_avant_toute_requete` (403 du handler messages, barème `[!]`).

## Q27 — `api/conversations.sdd` Scenario hors barème : NON écrits
- `conversation_au_contexte_incohérent_fait_tomber_la_liste_en_500_vnl_db_007` : verrouille l'amplification
  (une ligne incohérente fait échouer toute la liste), conditionnée à `PRAGMA foreign_keys` (`[?]`).
- `chemin_id_non_numérique_est_rejeté_avant_le_handler` : statut du rejet `Path` non vérifié (`[?]`).

## Q28 — Note sur `to_output` (bug de sécurité `conversations`)
- Le bug (résolution d'agent sans filtre owner dans `to_output`) n'est pas verrouillé : les tests écrits
  n'exercent que le chemin positif de l'agent propre à alice (`creation_avec_agent_connu…`).

## Q29 — Tests écrits sur comportements existants (vu rouge non observé)
- Tous les tests des passes 3 passent à l'écriture (caractérisation). Signalé ici.
