# Questions Haiku — crate `vanyline-cfgstore`

Session de réduction de dette de tests (2026-10-17). Tests ajoutés : 60 (error 8, domain 18,
layers 10, store 14, fs_store 7, `tests/surface.rs` 3). Tâches cochées : `error.sdd` (tests du
module), `layers.sdd` (compléter les tests), `domain.sdd` (convertir les Scenario sans test).

Note générale : quasi tous les nouveaux tests passent dès la première exécution (code existant,
tests de rattrapage). Aucun n'a été vu rouge avant implémentation : conformément à la consigne,
ils sont listés ici. Seule exception : `update_patch_partiellement_applique_apres_rejet`, qui a
échoué et a été retiré (voir Q3).

## Q1 — lib.sdd : trois Scenario non traduits en test (non-tests)
- Spec : `cfgstore/src/lib.sdd`, Scenarios `crate_sans_feature_compile_avec_no_default_features`,
  `exemption_cfg_test_couvre_les_tests_sans_allow_local`, `exemption_inopérante_hors_profil_test`.
- Bloque : ce sont des commandes cargo / une compile-fail, pas des tests comportementaux. Les
  `[?]` de `Tasks` (dont « accepter la traduction par revue ou ajouter trybuild ») ne sont pas
  tranchés.
- Observé : `cfgstore/src/lib.rs` porte bien `#![cfg_attr(test, allow(clippy::unwrap_used,
  clippy::expect_used, ...))]`, aucune feature dans `Cargo.toml`.
- Proposition : trancher les `[?]`. Les deux scénarios « exemption » peuvent être couverts par la
  batterie clippy ; le compile-fail demande trybuild (dev-dependency absente).
- Conséquence : tâche `lib.sdd` « Convertir chaque Scenario… » laissée `[ ]`.

## Q2 — lib.sdd : `[?]` de Tasks toujours ouverts
- Spec : `cfgstore/src/lib.sdd`, tâches `[?]` (surface `error` publique, re-export unique, doc
  crate `//!`, exemption des tests). Pas tranchés. La tâche « Trancher… » reste `[ ]` (hors
  périmètre). Pas de modification.

## Q3 — store.sdd : Scenario `update_patch_partiellement_applique_apres_rejet` contredit le code
- Spec : `cfgstore/src/store.sdd`, Scenario `update_patch_partiellement_applique_apres_rejet`.
  Le Scenario affirme « l'itération lexicographique applique "model" avant "mode" ».
- Observé : `serde_json::Map` est un `BTreeMap` (pas de `preserve_order`), donc l'ordre est
  lexicographique et `"mode"` < `"model"` (4e caractère : fin de chaîne vs `l`). `mode: "inconnu"`
  est donc rejeté en premier et `model` reste `"qwen2.5"`. Le test écrit (attendu `qwen3`) a
  échoué (`left: "qwen2.5"`, `right: "qwen3"`) et a été **retiré** de `store.rs`.
- Proposition : corriger la prémisse du Scenario (ordre réel : `mode` puis `model`, donc aucune
  mise à jour de `model` ; ou bien figer l'ordre comme contrat). Décision de Sébastien requise
  (`[?]` non-atomicité de Tasks).
- Conséquence : `store.sdd` reste `[ ]` (ce Scenario est dans la liste de la tâche).

## Q4 — store.sdd : Scenarios figés « arbitrés en Tasks » écrits selon la spec
- Spec : `cfgstore/src/store.sdd`, Scenarios `update_skill_forme_patch_verifiee_avant_existence`
  (divergence `update_skill` CFG-001 vs `update_agent` CFG-006) et
  `set_default_agent_sans_validation_ni_couche` (pas de validate_name, pas d'existence, Layer
  ignoré).
- Observé : le code se comporte comme la spec le décrit ; les tests passent. Mais les `[?]`
  correspondants (ordre de vérification, contrat voulu ou bug) ne sont pas tranchés.
- Proposition : test écrit selon la spec ; à confirmer une fois `[?]` tranchés.

## Q5 — fs_store.sdd : correspondance « prose → Scenario » ambiguë
- Spec : `cfgstore/src/fs_store.sdd`, tâche `[ ] Dériver les Scenarios sans test équivalent`.
  Les 10 titres n'y figurent pas, seulement des comportements en prose.
- Choix fait (tests ajoutés, titres exacts) :
  - patch non-objet sur update map → `update_provider_patch_non_objet_rejette`
  - clé inconnue ignorée + préservation des champs inconnus → `update_provider_api_key_null_efface_et_inconnue_ignoree`
  - create_provider en workspace quand global a déjà la clé → `create_provider_en_workspace_masque_sans_conflit`
  - name/clé inconnue ignorée sur toolset → `update_toolset_cle_inconnue_ignoree_sans_rename`
  - écriture couche jamais initialisée (toolsets/) → `ecritures_isolees_par_couche` et `create_toolset_forme_canonique_et_roundtrip`
  - round-trip Toolset (patch partiel) → `update_toolset_patch_partiel_et_normalisation`
- Non couverts, à trancher : « entrée de config.yaml non-map » (aucun Scenario de titre clair ;
  `list_providers_entree_mal_typee_aborte_avec_le_nom` porte sur un `endpoint` manquant, pas sur
  une entrée non-map) ; « round-trip Toolset mcp multi-serveurs via update_toolset » (aucun titre
  dédié).
- Faiblesse : `ecritures_isolees_par_couche` ne vérifie l'absence dans le global que si le fichier
  global existe (le test passe trivialement sinon).
- Conséquence : tâche `fs_store.sdd` laissée `[ ]`.

## Q6 — domain.sdd : message serde divergent
- Spec : `cfgstore/src/domain.sdd`, Scenario `skill_meta_description_manquante_rejete`, attend
  `missing field "description"` (guillemets doubles).
- Observé : serde_json émet ``missing field `description` `` (backticks). Le test asserte le
  message réel.
- Proposition : corriger le Scenario (backticks). Non modifié.
- Note : `model_profile_max_tokens_flottant_rejete` n'assert que `is_err()`, alors que la spec
  parle d'une erreur de type serde `u64` ; assertion plus fine possible si besoin.

## Q7 — error.sdd : tâche cochée alors que Done when reste ouvert
- Spec : `cfgstore/src/error.sdd`, tâche « Ajouter les tests du module » cochée `[x]` (les 8
  Scenario ont un test taggé vert). Le `Done when` exige en plus que les `[?]` soient tranchés
  par Sébastien : ce point n'est pas rempli. À confirmer que cocher la tâche est bien voulu.
- Autre remarque : tests du module `error` écrits avec `include_str!("error.rs")` pour
  `marqueurs_vnl_cfg_uniques_dans_le_fichier`, comme demandé par la spec.

## Q8 — store.sdd : tâche laissée ouverte
- Spec : `cfgstore/src/store.sdd`, tâche « Compléter les tests manquants » reste `[ ]` : 14 des
  15 Scenario sont couverts, `update_patch_partiellement_applique_apres_rejet` ne l'est pas (Q3).
