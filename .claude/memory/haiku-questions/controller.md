# Questions Haiku — crate `vanyline-controller` (dette de tests, Scenarios de construction pure)

Passe 1 : 2026-10-17. Passe 2 (owner, project, sandbox) : 2026-10-17, reprise après crash de session.
Périmètre : tâches `[ ]` de type « convertir/écrire/compléter les tests » dans `controller/src/*.sdd`.

## Q1 — Harnais des Scenarios `main.sdd` (CLI) : décision à trancher (non traité)
- Spec : `controller/src/main.sdd`, tâches « Harnais de tests du binaire à trancher » (ligne ~301) et « Convertir en tests … » (ligne ~316).
- Scenario concerné : `crds_imprime_quatre_manifests_et_sort`, `crds_sort_avant_tracing_et_client_kube`, `cli_defauts_compiles_sans_env`, `cli_env_surcherge_defaut_et_flag_surcharge_env`, `cli_lsp_surcharge_separement_de_la_toolchain`, `cli_drapeau_inconnu_exit_2`, `cli_aide_liste_les_quatorze_drapeaux`, `cli_version_absente_rejelee`, `client_kube_injoignable_exit_1`.
- Bloquant : la spec marque le harnais « à trancher avant le premier implementer ». Décision non prise, rien écrit.
- Observé : aucun `controller/tests/`, aucun `mod tests` dans `controller/src/main.rs`.
- Proposition : valider la proposition de la spec (test d'intégration binaire via `CARGO_BIN_EXE_vanyline-controller`, env via `temp_env` plutôt qu'`unsafe`).

## Q2 — Tests passés d'emblée (discipline « voir rouge »)
Tous les tests ajoutés (passes 1 et 2) passent à leur première exécution, car ils caractérisent un comportement déjà présent. Liste :
- passe 1, `application.rs` : `tls_secret_name_suffixe_cert`, `cert_manager_annotation_key_selon_kind`, `build_application_deployment_namespace_absent`, `deployment_phase_available_true_prioritaire_sur_failed`, `compute_status_failed`, `error_policy_requeue_trente_secondes` (un premier échec était dû à l'absence de runtime tokio, pas au comportement), `owner_reference_controller_sur_tous_les_builders`.
- passe 2, `owner.rs` : `compute_status_observed_generation_reprise`, `compute_status_serialisation_merge_patch`.
- passe 2, `project.rs` : les 17 tests de `build_workspace_pvc_classe_absente_sans_defaut` à `compute_status_conditions_generation_et_transition`, plus les extensions.
- passe 2, `sandbox.rs` : `agregat_collision_non_path_derniere_gagne`, `job_checkout_default_branch_vide_non_passe`, `status_failed_ready_false_not_running`, `status_service_toujours_pose_et_generation_suivie`, `error_policy_worktree_pending_requeue_dix_secondes`, `error_policy_project_not_ready_requeue_quinze_secondes`, `error_policy_erreur_courante_requeue_trente_secondes`, `ingress_annotation_certmanager_ecrase_celle_de_l_application`, plus les 4 extensions.
- Question : pour des Scenarios qui décrivent un comportement déjà existant, accepte-t-on un test vert d'emblée (et la tâche cochée), ou faut-il une mutation témoin pour prouver que le test peut échouer ?
- Les tâches cochées `[x]` (application, namespace) sont sous réserve de cette réponse. Les autres tâches de passe 2 restent `[ ]` (voir plus bas).

## Q3 — `[x]` posé sur `namespace.sdd` « Convertir chaque Scenario en test »
- Vérification par correspondance de titres : les 13 Scenarios ont un test homonyme dans `controller/src/namespace.rs`, tous verts. Ces tests sont antérieurs à la discipline TTD.
- Question : cette coche est-elle acceptable sans relecture du contenu Given/When/Then de chaque test ? Sinon, à décocher.

## Q4 — Décision d'harnais intégration cluster (non traité)
- Specs : `application.sdd` (« Décider l'harnais des Scenarios intégration », ligne ~460), `owner.sdd` (« Écrire les Scenarios d'intégration », ligne ~368), `project.sdd` (« Créer le harnais d'intégration cluster », ligne ~447), `sandbox.sdd` (« Rédiger le harnais d'intégration cluster », ligne ~602 ; « Vérifier en revue que la batterie locale reste la source de vérité CI », ligne ~609).
- Question : script de smoke sur le cluster de dev, ou tests `#[ignore]` / feature explicite hors CI ? Aucun n'a été écrit (cluster requis, hors périmètre).

## Q5 — Périmètre pur : TRAITÉ (passe 2) avec précisions
- `owner.sdd` : `compute_status_observed_generation_reprise` et `compute_status_serialisation_merge_patch` faits. Précision : la partie « le merge patch REMPLACE tout le tableau conditions » est une propriété de RFC 7386 (merge patch), non testée unitairement ici ; le test vérifie la forme JSON. `compute_status_serialisation_merge_patch` pose une génération (sans elle `observedGeneration` est omis par `skip_serializing_if`).
- `project.sdd` : les 17 Scenarios purs faits, plus les extensions `build_workspace_pvc_default_size` (namespace et nom), `build_fetch_cronjob_shape` (ownerRefs, concurrencyPolicy absente), `build_purge_job_shape` (volumes, absence de SA). Voir Q7 pour `blockOwnerDeletion`.
- `sandbox.sdd` : les 9 Scenarios de la liste faits, SAUF `pod_toolchain_nom_duplique_volumes_en_double` (voir Q8). Extensions faites : `cache_mounts_and_env` (commentaire corrigé en `["cargo","pnpm","pip"]`, mont et `PIP_CACHE_DIR` pip ajoutés, Scenario `pod_caches_defaut_cargo_pnpm_pip`), `build_checkout_job_owner_reference` (backoffLimit 3, TTL 3600), `mcp_port_and_probes` (initialDelaySeconds 5, periodSeconds 10 sur les deux sondes), `build_sandbox_service_shape` (ownerReference Sandbox unique).
- Précision sur `error_policy_worktree_pending_requeue_dix_secondes` : le test vérifie la classification pure (10 s). Le `[!]` de la spec (atteignabilité réelle, `Finalizer(String)` par `map_err` dans `./reconcile`) n'est pas tranché par ce test (voir Q6).

## Q6 — Points `[!]` de sandbox.sdd (non tranchés, non touchés)
- Atteignabilité des branches dédiées d'`error_policy` (`WorktreeRemovalPending`, `ProjectNotReady` probablement enveloppées en `Finalizer` par `./reconcile`) : à vérifier en cluster, puis à trancher (code ou doc).
- `@cleanup` branche `Some(_)` : `jobs.create` rappelé sur un Job existant (409 `AlreadyExists`). Code, hors périmètre.

## Q7 — CONTRADICTION spec/code : `blockOwnerDeletion` (nouveau)
- Spec : `project.sdd`, Scenarios `rbac_trois_objets_owner_reference_controle` et `jobs_et_cronjob_owner_references_vers_project`. Ils exigent `blockOwnerDeletion: true` « (dérive controller_owner_ref) ».
- Code : `controller_owner_ref` (kube) pose `block_owner_deletion` à `None`. Observé par les tests (`left: None, right: Some(true)`).
- Conséquence : `application.sdd` (`owner_reference_controller_sur_tous_les_builders`) exige au contraire « block_owner_deletion non posé ». Les deux specs ne sont donc pas cohérentes entre elles non plus.
- Ce qui est fait : le test ne verrouille que `kind`, `name`, `controller: true` et l'unicité. L'assertion `blockOwnerDeletion` est retirée, avec commentaire renvoyant ici.
- Question : quel est le contrat voulu ? (a) poser `block_owner_deletion: Some(true)` dans les builders Project (code, à décider), ou (b) corriger le Scenario project.sdd en « non posé » ? Décision de Sébastien requise avant d'assertion.

## Q8 — `pod_toolchain_nom_duplique_volumes_en_double` : NON écrit (contrat `[?]` ouvert)
- Spec : `sandbox.sdd` Scenario ligne ~903 et `[?]` « Toolchains du même nom » (ligne ~577). La tâche elle-même est conditionnelle : « si la [?] toolchains dupliquées tranche vers un contrat builder ».
- Le contrat (rejet apiserver ou validation amont, ou déduplication) n'est pas tranché. Rien écrit pour ne pas figer une décision.
- Question : validation amont (erreur utilisateur ou déduplication) ou rejet apiserver assumé ?

## Q9 — Hors périmètre (tâches de code ou décisions, non touchées)
- Retrait `existing_pvc` (project.sdd ~347, sandbox.sdd ~490), alignement « Owner = namespace » (owner.sdd ~302, project.sdd ~351, sandbox.sdd ~493), faille de sécurité sandbox.sdd ~499, `token_issuer` sandbox.sdd ~509, port d'exploitation sandbox.sdd ~514, RoleBinding et ClusterRole namespace.sdd ~123 (vérification cluster), `ControllerError` variantes `VNL-CTL-007..010` namespace.sdd ~133, RBAC ↔ `VnlK8sClient` application.sdd ~465 (revue manuelle).
- `application.sdd` ~382 : retrait de `spec.restate` avec `persistentVolumeClaimRetentionPolicy` Retain (tâche de code).
- Question de sous-périmètre : `owner.sdd` « Écrire les Scenarios unitaires sans test » (ligne ~359) est désormais entièrement traité (les 2 restants sont faits).
