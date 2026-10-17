# Questions Haiku — crate `vanyline-crds`

Session de réduction de dette de tests (2026-10-17). Tâche cochée : « Convertir en tests taggés
par leur titre les quatre Scenarios ajoutés le 2026-10-06 » (`crds/src/lib.sdd`).

Résultats : `cargo test -p vanyline-crds` 39 passés, 0 échec ; `cargo clippy -p vanyline-crds
--all-targets -- -D warnings` exit 0 ; `cargo fmt -p vanyline-crds -- --check` OK ; `specdd lint`
0 errors, 0 warnings.

## Q1 — retag plutôt que création : suppression des anciens noms
- Spec : `crds/src/lib.sdd`, tâche : « les retagger sous les titres des Scenarios ».
- Fait : trois tests renommés, sans doublon : `owner_namespace_name_concatene_app_et_owner` →
  `owner_namespace_name_valide` (ajout de l'exemple `media-test`/`seb`),
  `owner_namespace_name_rejette_hors_dns1123` → `owner_namespace_name_hors_dns_1123`,
  `owner_namespace_name_rejette_plus_de_63_caracteres` → `owner_namespace_name_trop_long`.
  Aucune référence externe à ces anciens noms (grep vérifié).
- À confirmer : préférez-vous conserver les anciens noms en plus (doublons) ?

## Q2 — nom de test non ASCII
- `git_secret_transport_conservé` (Scenario au titre accentué) : identifiant Rust valide, mais
  non ASCII. À confirmer comme convention acceptable pour le titre exact.

## Q3 — tests passés dès la première exécution (non vus rouges)
- `owner_namespace_name_valide`, `owner_namespace_name_hors_dns_1123`, `owner_namespace_name_trop_long`
  (code existant, tests retaggés).
- `git_secret_transport_conservé` : nouveau test, passe dès le premier run. Conforme à la spec
  (`gitSecret` se désérialise, `null` par défaut sans `skip_serializing_if`). Test inutile ou
  spec fausse ? Le champ est bien sérialisé sans `skip_serializing_if` dans `lib.rs`.

## Q4 — `[?]` de Tasks non tranchés
- Spec : `crds/src/lib.sdd`, tâche « Trancher chaque entrée `[?]` » reste `[ ]` (hors périmètre).
  Le `Done when` demande des décisions de Sébastien sur les `[?]` ; la tâche cochée ne les
  couvre pas. Pas de modification de ces entrées.
