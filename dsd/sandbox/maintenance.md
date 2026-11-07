# Solution Design: Maintenance des workspaces

## Résumé

La Solution `sandbox/maintenance` réalise `vanyline-maint`, le second binaire du crate `vanyline-sandbox`, embarqué dans l'image sandbox. Elle exécute toute la maintenance git d'un workspace de Project (clone bare, fetch, purge, worktree par sandbox, détection de langages) dans les pods de Jobs lancés par le controller. Elle réalise `sandbox#BR-8` : invocation en argv, jamais par un shell, validation des entrées libres avant tout effet de bord, et rejouabilité de chaque sous-commande. La logique vit dans `sandbox/src/maint.rs`, le câblage CLI dans `sandbox/src/bin/maint.rs`. La création des Jobs, leur argv côté controller et leur RBAC relèvent de `workspaces/projects` et `workspaces/sandboxes` ; ce document ne les redécrit pas.

## Problème et objectif

Le controller doit faire exécuter du git sur des dépôts et des noms fournis par l'utilisateur (URL, branche, nom de sandbox) sans qu'aucune de ces valeurs ne soit interprétée par un shell, et sans qu'une reprise de Job (redémarrage, TTL, recréation) ne corrompe le workspace. Cette Solution fournit un point d'entrée unique à argv fixe, dont le contrat est co-livré avec le controller. Elle contribue à `workspaces#BR-7` (aucun champ de CRD dans un shell) pour la part exécution.

## Périmètre et non-objectifs

Couvert : les six sous-commandes `init`, `fetch`, `purge`, `checkout`, `remove`, `detect` et leurs flags ; les validateurs d'entrée ; le layout du workspace ; la sémantique git de chaque sous-commande ; la fonction de détection des langages et le patch `status.languages` / `status.detectedAt` ; les codes de sortie et la discipline stdout/stderr.

Hors périmètre : la création, l'argv et le cycle de vie des Jobs (TTL, backoff, requeue) et le RBAC `project-<name>-maint`, décrits par `workspaces/projects` (FR-6, FR-9, FR-13, FR-14) ; les Jobs `sandbox-<n>-checkout` et `sandbox-<n>-remove`, décrits par `workspaces/sandboxes` (FR-2, FR-14) ; la consommation des langages par la santé du code (`sandbox/src/health.sdd`) et le mapping LSP docker (`sandbox/src/tools_impl.rs`), qui ne sont que consommateurs. Non-objectifs : suppression de références git (`refs/heads/*`), gestion des branches distantes au-delà du fetch, détection de versions de toolchain.

## Contexte applicable du domaine

- `sandbox#BR-8` : toute maintenance d'un workspace s'exécute dans un pod à image sandbox par `vanyline-maint`, en argv, sans interpolation d'un champ de CRD dans un shell. Règle réalisée ici, non redéfinie.
- `sandbox#AC-6` : un Job de maintenance rejoué n'échoue pas et ne duplique rien. Critère du domaine que les sous-commandes doivent satisfaire (FR-15).
- `sandbox#NFR-1` : identifiants uniques `VNL-SBX-*` et `VNL-HLT-*`. Les erreurs de cette Solution portent `VNL-MAINT-*` ; le domaine ne couvre pas ce préfixe (voir OI-14).

## Acteurs

- **Controller :** construit les argv des Jobs `init`, `fetch`, `purge`, `detect` (projects) et `checkout`, `remove` (sandboxes). Source du contrat argv.
- **Pods de Jobs :** exécutent `vanyline-maint` avec le volume `/workspace` (PVC du Project) et `/home/vanyline` (PVC home de l'Owner). Les pods init et fetch portent le ServiceAccount `project-<name>-maint` ; le pod purge n'en porte aucun (`workspaces/projects`, Acteurs).
- **Pod sandbox :** monte `repo.git` et `worktrees/<sandbox>` produits par cette Solution ; sa route `GET /git/unpushed` lit `refs/remotes/origin/*` (`docs/architecture.md`, section « Maintenance des workspaces », cité par la spec source).
- **Opérateur en CLI :** peut lancer le binaire à la main ; c'est la seule voie qui accepte un `--default-branch` vide (SpecDD /sandbox/src/maint.sdd, Accepts).
- **Consommateurs internes :** `sandbox/src/health.sdd` (`@languages_from_paths`), `sandbox/src/tools_impl.rs` (`@is_dockerfile_path` via `@toolchain_for_path`).

## État actuel et état cible

- **Actuel :** les six sous-commandes et leurs validateurs existent (SpecDD /sandbox/src/maint.sdd, Exposes ; SpecDD /sandbox/src/bin/maint.sdd, Must). Selon la spec `maint.sdd` (Tasks, cartographie du 2026-09-26), 17 tests unitaires vivent dans `sandbox/src/maint.rs` et 47 dans `sandbox/tests/maint.rs`. Aucun test n'exécute le binaire lui-même (SpecDD /sandbox/src/bin/maint.sdd, Tasks).
- **Cible :** chaque Scenario des deux specs porte un test de même titre ; les Scenarios du binaire sont exécutés via `CARGO_BIN_EXE_vanyline-maint` ; un harnais d'intégration cluster existe pour les Scenarios `cluster_*` (SpecDD /sandbox/src/maint.sdd, Done when ; SpecDD /sandbox/src/bin/maint.sdd, Done when). Aucun Scenario `cluster_*` n'est écrit à ce jour.

## Hypothèses, contraintes et dépendances

- **CON-1:** L'image sandbox et le controller avancent ensemble : l'argv est un contrat co-diffusé depuis le même dépôt.
    - **Source:** `workspaces/projects#CON-2` ; SpecDD /sandbox/src/bin/maint.sdd, Must

- **CON-2:** Le binaire suppose un exécutable `git` sur le `PATH` offrant `clone --bare`, `fetch --prune`, `worktree add/remove/prune`, `show-ref`, `symbolic-ref`, `ls-tree`, `config`, `check-ref-format --branch`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Accepts et Depends on

- **CON-3:** Les littéraux de layout (`repo.git`, `worktrees`, `cache`, mapping `pnpm` vers `pnpm-store`) sont dupliqués avec `controller/src/project.rs` et figés par des tests jumeaux.
    - **Source:** `workspaces/projects#CON-3` ; SpecDD /sandbox/src/maint.sdd, Purpose

- **CON-4:** Les pods de Jobs tournent en UID/GID 1000, non-root, avec `HOME` = `/home/vanyline` et `GIT_SSH_COMMAND` posé par le controller ; le binaire ne les modifie jamais.
    - **Source:** `workspaces/projects#CON-4`, `workspaces/projects#FR-16` ; SpecDD /sandbox/src/maint.sdd, Accepts

- **CON-5:** Hors cluster, `kube::Config::infer()` lit le kubecontext de l'appelant ; en cluster, le ServiceAccount du pod. Le patch de status n'a de droit que sur `projects/status` du seul Project nommé.
    - **Source:** `workspaces/projects#FR-13` ; SpecDD /sandbox/src/maint.sdd, Accepts et Depends on

- **Dépendances :** `clap`, `tokio`, `thiserror` 2, `serde_json` 1, `kube` 4, `k8s-openapi` 0.28, `rustls` 0.23 (feature `ring`), `vanyline-crds` (type `Project` seul). Source : SpecDD /sandbox/src/maint.sdd, Depends on ; SpecDD /sandbox/src/bin/maint.sdd, Depends on.

## Exigences fonctionnelles
- **FR-1 - MUST:** Le binaire expose exactement six sous-commandes, `init`, `fetch`, `purge`, `checkout`, `remove`, `detect`, avec les flags `--workspace`, `--repo`, `--cache` (répétable), `--sandbox`, `--branch`, `--default-branch`, `--project`. Un flag requis absent est une erreur clap avant toute exécution.
    - **Source:** SpecDD /sandbox/src/bin/maint.sdd, Must et Accepts
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** Le binaire et la logique n'invoquent git que par `std::process::Command` avec `.args(argv)` ; aucun `sh`, aucun `-c`, aucune chaîne interpolée en commande. `--` précède les positionnels de `git clone`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must et Must not ; SpecDD /sandbox/src/bin/maint.sdd, Must not ; `sandbox#BR-8`
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** `init` crée inconditionnellement chaque `cache/<dir>` dans l'ordre reçu (doublons et chaînes vides acceptés, seul `pnpm` est mappé), clone en bare vers `repo.git` seulement si ce chemin n'existe pas, puis pose la refspec de fetch. Le répertoire `workspace` n'est ni créé ni validé.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Accepts et Handles (`init`)
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** La refspec de fetch vaut exactement `+refs/heads/*:refs/remotes/origin/*`, posée par `git config --replace-all remote.origin.fetch`. Elle n'écrit jamais dans `refs/heads/*`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must et Must not (correctif WS-11, cité par la spec)
    - **Verified by:** `AC-4`
- **FR-5 - MUST:** `fetch` exécute `git --git-dir <workspace>/repo.git fetch --prune` sans vérification préalable ; un bare absent remonte en `VNL-MAINT-004`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`fetch`) ; Scenario `fetch_without_bare_fails`
    - **Verified by:** `AC-5`
- **FR-6 - MUST:** `checkout` valide `sandbox`, puis `branch`, puis `default_branch` si non vide, avant toute action. Si `worktrees/<sandbox>` existe, il rend `Ok` sans autre contrôle. Sinon il crée le worktree : `worktree add` si `refs/heads/<branch>` existe dans le bare (test `show-ref`), `worktree add -b <branch>` depuis le défaut sinon.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must (ordre contractuel) et Handles (`checkout`)
    - **Verified by:** `AC-6`
- **FR-7 - MUST:** Le point de création du défaut vaut `default_branch` s'il est non vide ; sinon (absent ou vide) il vaut `git symbolic-ref --short HEAD` du bare, et en cas d'échec de cette commande (code non nul ou spawn) le repli silencieux est `"main"`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`checkout`, résolution du défaut) ; Scenarios `checkout_resolution_symbolique_defaut_non_main_du_bare`, `checkout_repli_main_quand_resolution_echoue`
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** `remove` exécute `git worktree remove --force worktrees/<sandbox>`. Un code de sortie non nul, un échec de spawn ou une erreur du `remove_dir_all` de repli ne sont pas des erreurs : le repli supprime le répertoire et son erreur est ignorée. `git worktree prune` est toujours exécuté ; son échec est la seule erreur de `remove`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`remove`) et Must ; Scenarios `remove_incoherent_worktree_falls_back`, `remove_nonexistent_worktree_ok`, `remove_supprime_un_repertoire_non_inscrit`
    - **Verified by:** `AC-8`
- **FR-9 - MUST:** `remove` ne supprime jamais une référence `refs/heads/*` du bare ni un commit. `--force` détruit sans confirmation les modifications non commitées du worktree.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must not ; Scenarios `remove_ne_supprime_pas_la_branche_du_bare`, `remove_force_detruit_les_modifications_non_committees`
    - **Verified by:** `AC-8`
- **FR-10 - MUST:** `purge` supprime dans l'ordre `repo.git`, `worktrees`, `cache`, et seulement ceux-là. Un `NotFound` est ignoré par répertoire ; toute autre erreur d'I/O interrompt la purge.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`purge`) ; Scenarios `purge_removes_everything`, `purge_on_empty_workspace_ok`
    - **Verified by:** `AC-9`
- **FR-11 - MUST:** `detect` lit uniquement l'arbre de HEAD du bare (`git ls-tree -r --name-only HEAD`) et rend un JSON compact à clé unique, `{"languages":[...]}`, dans l'ordre fixe `rust`, `js-ts`, `vue`, `python`, `dockerfile`. Un marqueur est une présence de chemin : `rust` pour `Cargo.toml` (racine ou sous-répertoire) ; `js-ts` pour `package.json` ou `tsconfig.json` à la racine seulement ; `vue` pour tout `.vue` ; `python` pour `.py`, `.pyi`, `pyproject.toml` ou `setup.cfg` ; `dockerfile` selon `@is_dockerfile_path`. Toutes les comparaisons sont sensibles à la casse, sauf `dockerfile`. `detect` sur un bare absent ou sans commit est une erreur.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`detect`), Returns et Scenarios `detect_*`
    - **Verified by:** `AC-11`
- **FR-12 - MUST:** Si `--project` est fourni, `detect` n'effectue l'appel réseau qu'après une détection réussie et ne patche que `status.languages` et `status.detectedAt`, par un merge patch (`Patch::Merge`) construit à la main sur le sous-résultat `status`. Sans `--project`, le comportement est strictement celui de la détection locale.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`detect --project`) et Must ; Scenarios `cluster_patch_status_langues_seules_sans_projectstatus`, `detect_and_patch_without_project_matches_plain_detect`
    - **Verified by:** `AC-10`
- **FR-13 - MUST:** Les entrées libres sont validées avant toute action : `--repo` par `validate_repo` dans la couche CLI avant `run_init` ; `sandbox` par `validate_sandbox_name` (charset `[A-Za-z0-9._-]`, refus de `.`, `..` et du préfixe `-`) ; `branch` et `default_branch` non vide par `validate_branch` (refus local du vide, du préfixe `-` et des caractères de contrôle, puis `git check-ref-format --branch`). Les erreurs portent `VNL-MAINT-001` à `003`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must, Raises et Scenarios `validate_*` ; SpecDD /sandbox/src/bin/maint.sdd, Must (`init` valide `--repo` avant `mkdir`)
    - **Verified by:** `AC-12`
- **FR-14 - MUST:** Codes de sortie : `0` pour `Ok`, `1` pour toute `MaintError` (une ligne `vanyline-maint: {e}` sur stderr), `2` pour une erreur clap. Le code d'un sous-processus git n'est jamais propagé. stdout ne porte que la ligne JSON de `detect`, et seulement après succès de toutes les phases.
    - **Source:** SpecDD /sandbox/src/bin/maint.sdd, Must et Returns ; Scenarios `detect_imprime_seule_la_ligne_json_sur_stdout`, `detect_avant_init_n_imprime_rien_sur_stdout`
    - **Verified by:** `AC-13`
- **FR-15 - MUST:** Chaque sous-commande est rejouable sans effet indésirable : `init` ne reclone pas un `repo.git` existant et ne duplique pas la refspec ; `checkout` sur répertoire existant est un no-op ; `remove` répare les états incohérents ; `purge` ignore l'absence. Le controller compte sur cette propriété pour ses Jobs recréés.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Purpose et Must ; `sandbox#AC-6`
    - **Verified by:** `AC-14`
- **FR-16 - MUST:** Les littéraux de layout et le mapping `pnpm` vers `pnpm-store` restent identiques à ceux du controller ; `is_dockerfile_path` suit la table de vérité de `dockerfileName()` du frontend.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must (alignement) et Scenarios `layout_constants`, `cache_dir_name_mapping`, `is_dockerfile_path_truth_table` ; `frontend/src/components/panels/dockerfileName.ts`
    - **Verified by:** `AC-15`

## Sécurité

- L'argv est la seule voie d'entrée : pas de shell, pas de `sh -c`, pas d'interpolation (FR-2, DEC-1).
- Les entrées libres (repo, branche, nom de sandbox, cache, projet) sont validées avant effet de bord, sauf `--project` et `--workspace` (OI-10, OI-11).
- Le binaire ne lit ni n'écrit aucun secret. Il hérite `HOME` et `GIT_SSH_COMMAND` sans les modifier ; le chemin de la clé SSH, en dur côté controller, relève de `workspaces/projects#OI-13`.
- Seul `detect --project` appelle l'API Kubernetes, et seulement sur `projects/status` du Project nommé ; les autres sous-commandes n'ont besoin d'aucun ServiceAccount (SpecDD /sandbox/src/maint.sdd, Consequence K8s).
- `remove --force` et `purge` détruisent sans confirmation : perte de modifications non commitées assumée côté cleanup de Sandbox (SpecDD /sandbox/src/maint.sdd, Handles `remove`).

## Conception

Deux couches, une seule logique :

```text
Job (command: ["vanyline-maint", <sous-commande>, ...])
  └── bin/maint.rs      clap, dispatch, codes de sortie, stdout/stderr
        └── maint.rs    validateurs, layout, git (Command + argv), detect, patch K8s
              ├── git (runtime, PATH)
              └── kube API  (detect --project seulement)
```

Layout sous `workspace` (répertoire fourni, jamais créé par ce binaire) :

```text
<workspace>/
|-- repo.git                 clone bare ; refspec origin -> refs/remotes/origin/*
|-- worktrees/<sandbox>      un worktree git par sandbox
`-- cache/<dir>              pnpm -> cache/pnpm-store ; autres ids à l'identique
```

Ordre des opérations contractées :

- `init` : mkdir des caches, puis clone si `repo.git` absent, puis refspec `--replace-all`.
- `fetch` : `fetch --prune`.
- `checkout` : validation sandbox, branche, défaut ; existence de `worktrees/<sb>` ; résolution du défaut ; `show-ref` ; `worktree add` ou `worktree add -b`.
- `remove` : `worktree remove --force` ; repli `rm -rf` si échec ; `worktree prune` (seule erreur).
- `purge` : `repo.git`, `worktrees`, `cache`.
- `detect` : `ls-tree` de HEAD ; marqueurs ; JSON ; si `--project`, patch de status.

Marqueurs de détection (présence seulement, jamais de version, FR-11) : voir FR-11. Le JSON est sérialisé par `serde_json`, sans newline ; le `\n` est ajouté par le binaire.

## Décisions
- **DEC-1 - Logique en argv, validation dans la couche logique**
    - **Context:** Les entrées viennent de CR modifiables par l'utilisateur ; un shell ouvrirait l'injection.
    - **Decision:** Chaque commande est un argv ; les validateurs sont dans `maint.rs`, et `init` valide `--repo` dans le CLI.
    - **Alternatives:** Script shell assemblé par le controller, écarté par la règle de maintenance.
    - **Consequences:** Un futur appelant bibliothèque qui oublie `validate_repo` atteint `git clone` (OI-9).
    - **Source:** SpecDD /sandbox/src/maint.sdd, Purpose ; `workspaces/projects#DEC-2` ; `sandbox#BR-8`
- **DEC-2 - Rejouabilité comme contrat**
    - **Context:** Le controller recrée les Jobs (TTL 3600 s, backoff 3).
    - **Decision:** Chaque sous-commande est idempotente ; l'effet est atteint même si la commande a déjà été jouée (FR-15).
    - **Consequences:** Les échecs réels sont des erreurs ; les états déjà atteints sont des `Ok`.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Purpose ; `workspaces/projects#FR-14`
- **DEC-3 - Binaire de câblage, logique dans la bibliothèque**
    - **Context:** La logique doit être testable sans processus externe ; la surface publique est celle du CLI.
    - **Decision:** `bin/maint.rs` ne fait que clap, dispatch et rendu ; `maint.rs` contient tout le reste et n'écrit aucune ligne.
    - **Consequences:** Les tests de la logique appellent les fonctions directement ; les tests du binaire restent à écrire (OI-18).
    - **Source:** SpecDD /sandbox/src/bin/maint.sdd, Purpose ; SpecDD /sandbox/src/maint.sdd, Purpose
- **DEC-4 - Layout et table de marqueurs dupliqués, figés par tests jumeaux**
    - **Context:** Le controller et l'éditeur doivent partager les mêmes chemins et la même détection, sans dépendance de crate.
    - **Decision:** Dupliquer littéraux et table, et verrouiller la dérive par tests jumeaux.
    - **Consequences:** Toute modification doit être répercutée des deux côtés.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Purpose et Must ; `workspaces/projects#CON-3`
- **DEC-5 - Fetch vers `refs/remotes/origin/*`**
    - **Context:** Le fetch périodique ne doit pas écraser les branches locales des worktrees.
    - **Decision:** Refspec de fetch cible `refs/remotes/origin/*` (FR-4).
    - **Consequences:** `checkout` ne voit que les branches présentes au clone (OI-1).
    - **Source:** SpecDD /sandbox/src/maint.sdd, Must et Handles (`init`) ; `docs/architecture.md`, décision WS-11 (citée par la spec)
- **DEC-6 - Détection par présence, sensible à la casse**
    - **Context:** La détection doit rester simple et ne jamais lire de version.
    - **Decision:** Marqueurs par présence de chemin (FR-11) ; la casse n'est ignorée que pour `dockerfile`.
    - **Consequences:** Un marqueur en casse différente ou sous un répertoire non-ASCII n'est pas détecté (OI-6).
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`detect`) ; `docs/architecture.md`, décision du 2026-08-15 (citée par la spec)
- **DEC-7 - Patch de status construit à la main**
    - **Context:** Une sérialisation complète de `ProjectStatus` écraserait `cloned`, `worktrees` et `conditions`.
    - **Decision:** Le merge patch ne contient que `languages` et `detectedAt`.
    - **Consequences:** `detect` est le producteur exclusif de ces deux clés ; un tableau vide écrase la liste précédente.
    - **Source:** SpecDD /sandbox/src/maint.sdd, Handles (`detect --project`) et Must ; SpecDD /crds/src/lib.sdd (`skip_serializing_if`)

## Réalisation SpecDD

- `SpecDD /sandbox/src/maint.sdd` : logique, validateurs, layout, sous-commandes, détection (FR-2 à FR-13, FR-15, FR-16).
- `SpecDD /sandbox/src/bin/maint.sdd` : CLI, argv, codes de sortie, discipline stdout/stderr (FR-1, FR-14).
- `SpecDD /controller/src/project.sdd` et `SpecDD /controller/src/sandbox.sdd` : producteurs des argv (hors périmètre, voir `workspaces/projects` et `workspaces/sandboxes`).
- Code : `sandbox/src/maint.rs`, `sandbox/src/bin/maint.rs`, `sandbox/tests/maint.rs`, `sandbox/Cargo.toml` (déclaration `[[bin]]`).

## Acceptation et vérification

- **AC-1:** Les argv exacts du controller sont acceptés ; un flag inconnu sort en `2` sans effet. Scenarios `init_accepte_l_argv_du_controller`, `checkout_accepte_l_argv_avec_default_branch`, `flag_inconnu_sur_fetch_sort_deux_anti_derive`, `absence_de_sous_commande_sort_deux_sans_stdout`.
- **AC-2:** Aucune invocation shell n'existe dans le code de la Solution. **TBD :** aucun Scenario ne le vérifie aujourd'hui ; une revue ou un test de source reste à définir.
- **AC-3:** `init` crée les caches, clone une seule fois et ne duplique pas la refspec. Scenarios `init_creates_bare_and_caches`, `init_is_idempotent`, `init_recre_les_caches_supprimes_sans_reclone`, `init_idempotent_refspec_not_duplicated`, `init_bad_repo_fails`.
- **AC-4:** La refspec vaut `+refs/heads/*:refs/remotes/origin/*` et alimente les tracking refs. Scenarios `init_sets_fetch_refspec`, `fetch_populates_remote_tracking_refs`.
- **AC-5:** `fetch` est `--prune` et échoue sans bare. Scenarios `fetch_ok`, `fetch_prune_supprime_les_tracking_refs_disparues`, `fetch_without_bare_fails`, `fetch_sans_clone_sort_un_en_vnl_maint_004`.
- **AC-6:** `checkout` valide avant action, crée ou réutilise la branche, et est no-op sur répertoire existant. Scenarios `checkout_existing_branch`, `checkout_creates_branch_from_default`, `checkout_explicit_default_branch`, `checkout_is_idempotent`, `checkout_rejects_invalid_default_branch`, `checkout_rejette_branche_invalide_sans_action`.
- **AC-7:** Le défaut est résolu par `symbolic-ref`, avec repli `main`. Scenarios `checkout_resolution_symbolique_defaut_non_main_du_bare`, `checkout_default_branch_vide_resout_par_symbolic_ref`, `checkout_repli_main_quand_resolution_echoue`.
- **AC-8:** `remove` répare les états incohérents, ignore les erreurs de suppression, ne touche pas aux branches. Scenarios `remove_removes_worktree`, `remove_incoherent_worktree_falls_back`, `remove_nonexistent_worktree_ok`, `remove_supprime_un_repertoire_non_inscrit`, `remove_force_detruit_les_modifications_non_committees`, `remove_ne_supprime_pas_la_branche_du_bare`.
- **AC-9:** `purge` supprime le trio et ignore l'absence. Scenarios `purge_removes_everything`, `purge_on_empty_workspace_ok`, `purge_workspace_vide_sort_zero_silencieux`.
- **AC-10:** Un Job `detect --project` ne patche que deux clés et ne contacte pas l'API sans `--project`. Scenarios `cluster_patch_status_langues_seules_sans_projectstatus` et `cluster_patch_ecrase_liste_langues_preexistante` (non écrits, harnais absent) ; `detect_and_patch_without_project_matches_plain_detect`, `detect_and_patch_with_project_fails_without_cluster` (unitaires, selon la spec).
- **AC-11:** La détection respecte la table des marqueurs et l'ordre fixe. Scenarios `detect_rust_only`, `detect_js_ts_package_json`, `detect_nested_package_json_ignored`, `detect_python_order_fixed_full`, `detect_dockerfile_negative_names`, `detect_marqueurs_sensibles_a_la_casse_sauf_dockerfile`, `detect_no_markers`.
- **AC-12:** Les entrées invalides sont refusées avant toute action. Scenarios `validate_branch_rejects_*`, `validate_repo_rejects`, `validate_sandbox_name_rejects`, `init_repo_invalide_ne_cree_aucun_cache`, `remove_rejects_invalid_sandbox`.
- **AC-13:** Sorties et codes respectent le contrat. Scenarios `detect_imprime_seule_la_ligne_json_sur_stdout`, `detect_avant_init_n_imprime_rien_sur_stdout`, `version_repond_zero_et_nomme_le_binaire`.
- **AC-14:** Les sous-commandes rejouées ne dupliquent rien. Scenarios `init_is_idempotent`, `checkout_is_idempotent`, `remove_nonexistent_worktree_ok`, `purge_on_empty_workspace_ok`.
- **AC-15:** Les littéraux et la table de marqueurs sont alignés. Scenarios `layout_constants`, `cache_dir_name_mapping`, `cache_path_uses_mapping`, `worktree_path_value`, `is_dockerfile_path_truth_table`.

Vérification actuelle : tests unitaires et d'intégration git (SpecDD /sandbox/src/maint.sdd, Tasks) ; aucun test du binaire (SpecDD /sandbox/src/bin/maint.sdd, Tasks) ; harnais cluster absent.

## Open Issues

Les points suivants restent `TBD`. Chacun est repris des `[?]` ou `[!]` des specs sources, sauf mention contraire.

- **OI-1 - Branche créée après le clone:** `checkout` ne teste que `refs/heads/<branch>` du bare ; une branche créée en amont après `init` est silencieusement créée depuis le défaut (SpecDD /sandbox/src/maint.sdd, `[?]` checkout aveugle). **TBD :** contrat « branche de travail locale », ou création depuis `refs/remotes/origin/<branch>`. Touche FR-6.
- **OI-2 - Idempotence de `checkout` au seul `exists`:** un `worktrees/<sb>` non inscrit, ou inscrit sur une autre branche, rend `Ok` sans effet ; un worktree inscrit dont le répertoire a disparu fait échouer `worktree add` sans prune préalable (SpecDD /sandbox/src/maint.sdd, `[?]` idempotence et `[?]` prune). **TBD :** verrouiller le « premier-arrivant », ou resserrer la vérification et ajouter un `prune`. Touche FR-6.
- **OI-3 - Deux sandboxes sur la même branche:** le second `worktree add` échoue ; le Job reste en échec après ses backoffs (SpecDD /sandbox/src/maint.sdd, `[?]`). **TBD :** cas d'usage (worktree partagé, détaché) ou blocage plus haut, côté validité de `spec.branch` par Sandbox. Non relu : `workspaces/sandboxes`.
- **OI-4 - Repli silencieux sur `main`:** un échec de `symbolic-ref` (git absent, HEAD détachée) fait utiliser `main` sans log, et la branche par défaut amont modifiée après clone n'est jamais suivie (SpecDD /sandbox/src/maint.sdd, `[?]`). **TBD :** tracer un warn, distinguer spawn et code non nul. Touche FR-7.
- **OI-5 - Dépôt source vide:** `init` réussit, mais `detect` et `checkout` échouent ; le Job init d'un projet vide boucle en échec (SpecDD /sandbox/src/maint.sdd, `[?]`). **TBD :** « projet vide = erreur » ou `{"languages":[]}` avec message d'attente. À rapprocher de `workspaces/projects#OI-5` et `#OI-6`.
- **OI-6 - Quoting `core.quotePath`:** `ls-tree` sans `-z` entoure de guillemets les chemins non-ASCII ; les marqueurs sous ces répertoires ne sont jamais détectés (SpecDD /sandbox/src/maint.sdd, `[?]` et Scenario `detect_markers_sous_repertoire_non_ascii_exclus_par_le_quoting`). **TBD :** passer `-z`, ou assumer. Touche FR-11.
- **OI-7 - `repo.git` occupé par un fichier:** le clone est sauté sur simple `exists()`, puis la refspec échoue ; état sans réparation autre que `purge` (SpecDD /sandbox/src/maint.sdd, `[?]` et Scenario `init_repo_git_fichier_regular_echoue_en_refspec`). **TBD :** détection explicite ou impossibilité assumée. Touche FR-3.
- **OI-8 - Classement de l'échec de spawn de `check-ref-format`:** classé `InvalidBranch` (`VNL-MAINT-001`), alors que les autres commandes le classent `GitFailed` (SpecDD /sandbox/src/maint.sdd, `[?]`). **TBD :** accepter ou reclasser. Touche FR-13.
- **OI-9 - Validation de `repo` hors CLI:** `run_init` ne valide pas `repo` ; un futur appelant bibliothèque passe directement à `git clone` (SpecDD /sandbox/src/maint.sdd, `[?]`). **TBD :** déplacer `validate_repo` dans `run_init` ou figer la responsabilité du CLI. Touche FR-13 et DEC-1.
- **OI-10 - `--workspace` non validé:** une valeur vide ou relative est résolue depuis le CWD ; `purge --workspace ""` supprimerait `repo.git`, `worktrees` et `cache` sous le CWD. Les Jobs passent toujours `/workspace` (SpecDD /sandbox/src/bin/maint.sdd, `[?]`). **TBD :** refuser vide et non absolu dans le binaire, ou assumer. Risque destructif, à trancher en priorité.
- **OI-11 - Valeurs vides de `--project` et `--default-branch`:** `--project ""` part en patch API et échoue ; `--default-branch ""` est traité comme absent. Le nom de projet n'est pas validé côté maint (SpecDD /sandbox/src/maint.sdd, `[?]` ; SpecDD /sandbox/src/bin/maint.sdd, `[?]`). **TBD :** vide = absent partout, ou filtre local. Touche FR-7 et FR-12.
- **OI-12 - Contrat des codes de sortie et de `--version`:** le code `2` de clap n'est épinglé par aucun test ni spec controller ; `--version` affiche la version du workspace sans test (SpecDD /sandbox/src/bin/maint.sdd, `[?]`). **TBD :** contractualiser `0`/`1`/`2` ou seulement `0` contre non-`0`. Touche FR-14.
- **OI-13 - Exemption clippy du binaire:** la spec du binaire dit qu'aucun `#[allow]` local ne couvre `println!`/`eprintln!`, alors que `sandbox/src/bin/maint.rs` porte `#[allow(clippy::print_stdout, clippy::print_stderr)]` sur `main`, justifié par `tooling.sdd` D3c. La spec du binaire est donc en retard sur le code. **TBD :** mettre à jour la spec sœur, et vérifier que `tooling.sdd` couvre bien cette exemption. Touche FR-14.
- **OI-14 - Préfixe d'erreur hors domaine:** `sandbox#NFR-1` ne couvre que `VNL-SBX-*` et `VNL-HLT-*`, alors que cette Solution porte `VNL-MAINT-001` à `006`. **TBD :** étendre le NFR du domaine ou accepter l'écart.
- **OI-15 - URL dans les erreurs:** `GitFailed` contient l'argv complet ; une URL portant des identifiants y apparaîtrait sur stderr. ASSUMPTION fondée sur `args = argv complet` (SpecDD /sandbox/src/maint.sdd, Raises). **TBD :** vérifier le cas et décider d'un masquage. Touche Sécurité.
- **OI-16 - Documentation d'erreur incomplète:** `docs/architecture.md` (section « Maintenance des workspaces ») cite `VNL-MAINT-001` à `005`, alors que `006` existe dans le code et la spec (SpecDD /sandbox/src/maint.sdd, `[!]`). **TBD :** corriger la doc.
- **OI-17 - Deux styles de ciblage du bare:** `init`, `fetch`, `detect` et la refspec utilisent un `--git-dir` absolu ; `checkout` et `remove` utilisent un chemin relatif avec `cwd = workspace` (SpecDD /sandbox/src/maint.sdd, `[!]`). Équivalents aujourd'hui. **TBD :** unifier. `@run_detect` est `pub` mais n'a que des appelants de test (même spec, `[!]`).
- **OI-18 - Chemins de référence erronés dans les specs sources:** `sandbox/src/maint.sdd` cite `../../tests/maint.rs`, `sandbox/src/bin/maint.sdd` cite `../tests/maint.rs` et `../../Cargo.toml`. Le fichier réel est `sandbox/tests/maint.rs` (cité par `tooling.sdd`) et la déclaration `[[bin]]` est dans `sandbox/Cargo.toml`. **TBD :** corriger les références dans les specs.
- **OI-19 - Contrat issu d'une reverse-engineering:** les deux specs sont marquées « Relecture » en `[ ]` ; le comportement accidentel ne devient contrat qu'après relecture du développeur principal (SpecDD /sandbox/src/maint.sdd, Tasks ; SpecDD /sandbox/src/bin/maint.sdd, Tasks). Toutes les exigences de ce document en dépendent.
- **OI-20 - Reprise de checkout après TTL:** le Job `sandbox-<n>-checkout` est recréé une heure après sa fin, et son idempotence est supposée (`workspaces/sandboxes#OI-6`). **TBD :** voir ce point côté sandboxes.

## Références

- `sandbox#BR-8`, `sandbox#AC-6`, `sandbox#NFR-1` ; `workspaces#BR-7`.
- `workspaces/projects` : `#CON-2`, `#CON-3`, `#CON-4`, `#FR-13`, `#FR-14`, `#FR-16`, `#DEC-2`, `#OI-5`, `#OI-6`, `#OI-13` ; `workspaces/sandboxes` : `#FR-2`, `#FR-14`, `#OI-6`.
- SpecDD `/sandbox/src/maint.sdd`, `/sandbox/src/bin/maint.sdd`, `/controller/src/project.sdd`, `/controller/src/sandbox.sdd`, `/crds/src/lib.sdd`, `/tooling.sdd` ; `vanyline.sdd` ; `AGENTS.md` ; `docs/architecture.md` ; `frontend/src/components/panels/dockerfileName.ts`.
