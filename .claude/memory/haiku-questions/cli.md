# Questions Haiku — crate `vanyline` (cli), dette de tests

Session du 2026-10-17. Passe 2 (owner / project / sandbox, tests de parse via `Cli::try_parse_from`).

## Q1 — `project create` sans `--owner` : Scenario contradictoire avec le code
- Spec : `cli/src/project_cmd.sdd`, Scenario « `project create` sans `--owner` est rejeté ».
- Bloque : `--owner` est un flag GLOBAL de `Cli` (`cli/src/main.rs`, `owner: Option<String>`), pas un champ requis de `Create`. Le parse réussit sans `--owner`, donc le Scenario échoue.
- Observé : le Scenario « project create minimal » parle de `owner == "alice"` sur `Commands::Create`, champ inexistant ; j'ai testé `cli.owner` à la place.
- Non écrit : le test de rejet (il serait rouge). Proposition : trancher entre rendre `--owner` requis pour `project` (changement de code, hors périmètre) ou réécrire le Scenario pour le flag global.

## Q2 — Flags répétés (`--home-size`, `--image`) : comportement clap verrouillé
- Spec : `cli/src/owner_cmd.sdd` Scenario `owner_flag_repete_en_home_size_refuse` ; `cli/src/sandbox_cmd.sdd` Scenario `sandbox_flag_image_repete_refuse`. Tâches `[?]` associées.
- Observé : clap 4 rend `ArgumentConflict` avec le message « cannot be used multiple times ». Les tests verrouillent ce rendu.
- Question : ce rendu est-il le contrat voulu ? (la tâche `[?]` demandait une vérification, pas une décision) Relecture de Sébastien requise avant de cocher.

## Q3 — Scenarios d'aide : flags lus sur la sous-commande `create`
- Spec : `owner_cmd.sdd` et `sandbox_cmd.sdd`, Scenario « aide ... documente variants et flags ».
- Observé : les longs flags n'apparaissent pas dans l'aide du parent (`owner --help`), seulement dans `owner create --help`. Les tests vérifient les quatre/six variants sur le parent et les flags sur `create`.
- Question : la spec doit-elle préciser que les flags sont lus sur `create` ?

## Q4 — Scenarios non couverts (volontairement)
- Intégration cluster (non écrits) : `owner_create_avec_un_seul_project_default_flag_agrege_partiel`, `owner_create_sans_flag_project_default_omet_le_bloc`, `owner_list_*`, `sandbox_create_*`, `sandbox_stop_puis_start_*`, `sandbox_list_*`, `project_*` d'intégration, erreurs sans kubeconfig.
- Non écrits aussi : `project` et `sandbox` pour les Scenarios `[?]`-dépendants décrits en Q1, et les tâches `[?]` restantes de chaque spec (`--git-secret`, `egress`, `--image ""`, etc.).

## Q5 — Tâches de cochage
- Aucune tâche `[ ]` cochée dans cette passe : les `Done when` exigent les tests d'intégration et la résolution des `[?]` par Sébastien.

## Q6 — Périmètre (mis à jour, passe 3)
- Passe 2 : `owner_cmd.rs`, `project_cmd.rs`, `sandbox_cmd.rs` (voir Q1 à Q5).
- Passe 3 : `skill_cmd`, `toolset_cmd`, `config_check`, `config`, `store`, `tools`, `rpc/protocol`, `rpc/mod` (intégration), `rpc/handlers` (constat seul). Hors périmètre : `tui`, `chat`, tâches de code, cluster.

## Q7 — skill_cmd / toolset_cmd : tests ajoutés, passent dès la première exécution
- Spec : `cli/src/skill_cmd.sdd` (4 Scenario), `cli/src/toolset_cmd.sdd` (2 Scenario).
- Écrits dans `skill_cmd.rs` / `toolset_cmd.rs` (Owns de chaque spec), via `crate::Cli` (le `Cli` privé de `main.rs` est visible aux modules enfants). La spec dit « via ./main.rs » : même effet, à confirmer.
- Observé : les 6 tests passent dès le premier run (pas de rouge vu). Discipline TTD : parse de code existant, à consigner.
- Non cochée : `skill_cmd` a une `Done when` qui exige les `[?]` tranchés (asymétrie CLI/RPC, `skills` sans sous-commande) et une tâche `[ ] Relecture`.

## Q8 — config_check : Scenario contradictoire avec le code (sorte de référence)
- Spec : `cli/src/config_check.sdd`, Scenario `erreur_de_lecture_collectee_sans_bloquer_les_autres` : « UnknownReference de sorte `model` » pour un model `ghost-model` dont le provider `ghost` est absent.
- Observé : le code produit `UnknownReference("provider", ...)`, cohérent avec le Scenario `provider_inconnu_de_model_message_exact` (« Unknown provider reference »). Le test asserte `provider`.
- Proposition : corriger le Scenario (sorte `provider`).
- Écrits : 19 tests (config_saine, erreur_de_lecture, lecture_en_erreur_cascade, doublon_supplementaire, provider/mcp/defaults message exact, agent_problemes_dans_l_ordre, skills_auto_none, default_agent, noms_identiques, absente, corrompu x4). Les 7 `*_message_exact` passent dès le premier run (caractérisation).
- Non écrits : `ordre_global_lecture_puis_doublon_puis_reference` (store à `list_providers` en erreur, à ajouter), `check_config_n_appelle_jamais_load_skill_ni_une_ecriture` (spy à implémenter sur tout le trait `ConfigStore`), `les_six_sortes_de_doublon_detectees_dans_l_ordre`, `fs_store_bout_en_bout` (déjà couvert par un test existant, non renommé), `rendu_stdout_*` (intégration binaire, `cli/tests/`, arbitrage `[?]` harnais).
- Non cochée : `[ ] Relecture complète` et `[?]` ouverts.

## Q9 — config.rs : Scenario non testables sans changement de code
- Écrits : 6 tests `configured_namespace` / `configured_toolbox` (couches tempdir, pas d'env).
- Non écrits : `discover_layers` (x3) et `config_dir`/`data_dir` (lisent `XDG_CONFIG_HOME`/`XDG_DATA_HOME` : non isolable sans muter l'env du process, `unsafe` en édition 2024), `ensure_config_dir`/`ensure_data_dir` (terminent le process via `exit(1)`).
- Proposition : rendre le répertoire de base injectable (paramètre ou `Layers`), ou un harnais process pour les scénarios `exit`.
- Non cochée : `[?]` ouverts (repli `~` littéral, `eprintln!`/`exit`, `#[allow(dead_code)]`, marqueurs asymétriques).

## Q10 — store.rs : IO non testables + tâche cochée
- Coché : `[x] Les sept tests existants...` (store.sdd l.332). Motif : les 4 extensions de résolution (`no-fallback`, `index vs préfixe`, `préfixe majuscule`, `zéro initial`) sont écrites et vertes ; aucune réserve `[?]` propre à cette tâche. Les `[?]` de store.sdd (atomicité, repli data_dir) restent ouverts et sans lien.
- Non écrits : scénarios de persistance (l.178-223 : list/save/get/delete/pointeur actif), car `@data_dir` lit `dirs::data_dir()` (non injectable). Proposition : variantes `*_in(dir)`.
- Non écrit : branche « erreur read_dir » (permissions) : hors périmètre (permissions).

## Q11 — tools.rs : 11 tests sur 19 Scenario
- Écrits : `local_tools_map` (8), `definition_ignorer_le_prompt` (schéma égal à `mcp::filesystem_tools()`), `tool_definition_inconnu_panique` (`should_panic`), `lecture_inexistant`, `json_malforme`, `requis_manquant`, `offset_negatif`, `write_file` (octets), `delete_file`, `aucun_chemin_confine`, `champs_inconnus`. Tous verts.
- Non écrits : 4 Scenario timeout (durées réelles de 25 à 30 s, arbitrage « variante rapide » à trancher), `limite_zero` (format de sortie à vérifier), `cwd_vide` (change le cwd du process, à sérialiser), `local_tools_absent_du_mode_toolbox` (`build_session_context` : chat, hors périmètre).
- Non cochée : tâche « Convertir en tests » incomplète. `[?]` (triplication des noms, message `wrote` non vérifié) ouverts.

## Q12 — rpc/protocol.rs : 17 tests, tous les Scenario couverts
- Écrits : les 17 Scenario de `protocol.sdd` (l.482-651), dont `seize_codes_vnl_rpc` (lecture de `include_str!`). Tous verts dès le premier run (caractérisation : pas de rouge vu).
- Non cochée : la `Done when` exige que les `[?]`/`[!]` soient clos (dérive doc `chat/event` et `ociImage`, `deny_unknown_fields`, annotations `[!]` fausses). Le fichier ne contient plus de `r#"{}"#`, clippy vert.

## Q13 — rpc/mod : 6 tests d'intégration ajoutés (tests/rpc_stdio_smoke.rs)
- Écrits : `eof_stdin_clot_le_processus_apres_drain`, `stdin_ferme_immediatement_aucune_trame_sur_stdout`, `lignes_blanches_sautees_sans_reponse_ni_erreur`, `trame_invalide_repond_vnl_rpc_000_puis_boucle_vivante`, `reponses_dans_lordre_des_requetes_sur_un_flux_ndjson`, `shutdown_clot_le_processus_meme_quand_stdin_reste_ouvert`. Verts.
- Non écrits : `stdin_non_utf8_arrete_la_boucle_silencieusement` (contrat de fait à confirmer), `stdout_refermee_le_processus_sort_sans_panique` (pipe cassé, à stabiliser), `chat_send_en_cours_draine_sa_reponse` (provider TCP fermé, tour spawné).
- Non cochée : la tâche reste `[ ]`. Les `[?]` (écriture, code de sortie, backpressure, taille de trame) sont ouverts.

## Q14 — rpc/handlers.rs : rien ajouté
- Tâches non dérivables sans décision : cas K8s (cluster d'essai), `VNL-RPC-011` (répertoire en lecture seule), résurrection d'une conversation supprimée pendant un tour (`[!]`), `ensure_data_dir` qui termine le process (`[!]`), `workspace` relatif (`[?]`).
- Proposition : arbitrer avec Sébastien le `[!]` `ensure_data_dir` → `VNL-RPC-007` avant de dériver les tests.

## Q15 — Validation finale (passe 3)
- `cargo test -p vanyline` : 298 + 2 + 28 + 2 passed, 0 failed.
- `cargo clippy -p vanyline --all-targets -- -D warnings` : vert (après 3 corrections de lints de tests).
- `cargo fmt -p vanyline -- --check` : vert.
- `specdd lint` : 0 errors, 0 warnings in 162 specs.
- Cochage : uniquement `cli/src/store.sdd` l.332.
