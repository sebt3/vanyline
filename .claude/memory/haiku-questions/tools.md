# Questions — crate `vanyline-tools` (tests de dette, passe Haiku)

Périmètre : `tools/src/*.sdd` (lib, error, command, filesystem, search, mcp, output).
État : 178 tests verts, `cargo clippy -p vanyline-tools --all-targets -- -D warnings` vert, fmt vert.

## Point transversal — TTD sur code existant

Les tests ajoutés caractérisent un code déjà conforme à la spec : aucun n'a été vu rouge
avant l'implémentation (règle implementer : « test qui passe d'emblée = à signaler »).
Exceptions volontaires : les deux Scenarios `find_files_patron_ancre_sur_chemin_relatif`
et `search_filtre_glob_sur_chemin_relatif` sont passés rouges (voir Q1) et ont été retirés.
Proposition : pour les tâches de dette, exiger une vérification par mutation ciblée
(modifier temporairement le code, constater le rouge, restaurer) plutôt qu'un rouge initial.

## Q1 — `*` traverse-t-il `/` ? (search.sdd l.305, l.396 ; mcp/find_files)

- Scenarios : `find_files_patron_ancre_sur_chemin_relatif` (search.sdd:305),
  `search_filtre_glob_sur_chemin_relatif` (search.sdd:396).
- Spec : « `*` ne traverse pas `/` » (sémantique globset « par défaut »).
- Observé : `find_files` avec `*.rs` remonte `nested/imbre.rs` ; `search` avec glob `*.rs`
  remonte `sous/neste.rs`. Cause : `globset::Glob::new` sans `literal_separator(true)` →
  `*` traverse `/`. Sortie réelle : `left: "sous/neste.rs:1: fn foo() {}\ntest.rs:1: fn foo() {}"`.
- Bloque : le test ne peut pas être écrit au contrat de la spec sans modifier la production.
- Proposition : décider le contrat. (a) Spec juste : `*` traverse `/` (documenter, figer
  les deux tests avec le comportement observé). (b) Code juste : `literal_separator(true)`
  dans `find_files` et `search` (production, hors périmètre Haiku).

## Q2 — Ligne vide en tête de `bound_head_tail` (output.sdd:248)

- Scenario : `bound_head_tail_coupure_jamais_en_milieu_de_ligne` (50 lignes de 20 octets, max 64).
- Observé : `bound_head_tail` rend `"\n[... 935 bytes truncated ...]\n"` (header vide,
  footer vide). Le Scenario exige que toute ligne non-marqueur soit une ligne entière
  de l'entrée : la ligne vide en tête la viole.
- Proposition : soit le Scenario admet la ligne vide (dégénérescence déjà tracée en
  `[?] Sorties dégénérées`), soit le code ne rend pas de `\n` initial quand le header est vide.

## Q3 — Marqueur hors budget (output.sdd:216)

- Scenario : `bound_lines_marqueur_hors_budget_octets` (10 lignes de 100 octets, max 500).
- Observé : sortie de 458 octets au total (4 lignes + marqueur). Le Scenario affirme
  « la longueur totale dépasse 500 octets à cause du seul marqueur » : faux pour cette entrée.
- Proposition : corriger l'assertion du Scenario (par ex. un cas où le marqueur fait
  réellement dépasser le budget) ou accepter qu'il n'est pas vérifiable tel quel.
  La partie « contenu avant marqueur ≤ 500 octets » est vraie et pourrait être testée seule.

## Q4 — Défauts annoncés en prose (mcp.sdd:236)

- Scenario : `defauts_annonces_en_prose_sans_cle_schema`.
- Spec : « la description de read_file contient `default 0` et `default 200` »,
  « la description d'execute_command contient `default 30` et `0 = no timeout` ».
- Observé : ces textes sont dans les `description` des propriétés (`offset`, `limit`,
  `timeout_secs`), pas dans la `description` de haut niveau du tool. Le sens de « la
  description de read_file » est ambigu.
- Proposition : préciser le Scenario (description de tool ou de propriété). Test non écrit.

## Q5 — Scenarios permission-dépendants (interdits au passage Haiku)

- `search_fichier_illisible_ignore_silencieusement` (search.sdd:435) — mode 000.
- `cwd_parent_inaccessible_erreur_io` (command.sdd:396) — mode 000.
- `write_file_parent_interdit_rejete_en_vnl_tls_004` (filesystem.sdd:464).
- `list_directory_sous_repertoire_ilisible_sans_enfants` (filesystem.sdd:571).
- Proposition : tests avec `#[cfg_attr]`/skip explicite si root (filesystem.sdd l.301 le demande),
  à écrire dans un passage qui peut tourner hors contexte Haiku.

## Q6 — Scenarios d'environnement (command.sdd)

- `processus_lance_dans_groupe_dedie` (l.472) : utilise `ps -o pgid=`. `ps` est absent de
  l'environnement de cette session (« command not found »), le test échouerait pour une raison d'environnement.
- `future_annulee_tue_le_fils_direct` (l.478) : détection par nom de processus
  (`pgrep`/`ps`), flakiness documentée dans la spec. Non écrit.
- Proposition : remplacer la sonde `ps`/`pgrep` par `/proc/<pid>/stat` (pgid) ou
  par un marqueur fichier écrit par le fils ; accepter ou refuser le risque de flakiness.

## Q7 — Scenarios de surface / compilation (lib/tools)

- `surface_racine_six_modules_publics`, `surface_racine_sans_nom_plat`,
  `harnais_test_exempte_production_deny` (tools/src/lib.sdd l.144-167) : compilation ou
  commande cargo, pas des tests comportementaux. Non écrits.
- Question déjà tracée dans lib.sdd `[?]` : trybuild absent des dev-dependencies.

## Q8 — Dérive d'état partagé `CWD_LOCK` (note de réalisation, pas une question de contrat)

- Le Scenario `find_files_path_vide_signifie_repertoire_courant` exige `set_current_dir`.
  Pour éviter une course avec `cwd_vide_herite_du_processus_courant`, le verrou
  `CWD_LOCK` (tokio `Mutex`) est défini en `#[cfg(test)] pub(crate)` dans `tools/src/command.rs`
  (hors `mod tests`) et partagé avec `search.rs`. À valider : acceptable en production
  (cfg(test) seulement) ou à déplacer ailleurs.

## Q9 — Tâches non cochées volontairement

- Les tâches `[x]` ne sont pas cochées : AGENTS.md exige une synthèse `validator` verte
  avant tout `[x]`. Les tests passent, mais la synthèse validator n'a pas été produite.
