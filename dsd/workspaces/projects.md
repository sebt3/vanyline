# Solution Design: Projects

## Résumé

La Solution `workspaces/projects` réalise le dépôt d'un Owner : la CR `Project`, son PVC workspace, son dépôt bare et ses caches, les Jobs `init`, `fetch` et `purge`, le CronJob de fetch, le trio RBAC de maintenance, la détection de langages et la propagation vers les Sandboxes du Project. Elle est réalisée par le reconciler `controller/src/project.rs` (SpecDD /controller/src/project.sdd) et par le type `Project` de `vanyline-crds` (SpecDD /crds/src/lib.sdd, partie Project). Toute la maintenance git s'exécute dans des pods à image sandbox via `vanyline-maint` en argv ; le contenu de cet utilitaire relève du domaine `sandbox` (`sandbox#BR-8`) et n'est pas décrit ici.

## Problème et objectif

Un utilisateur déclare un dépôt git ; le cluster doit lui fournir un espace git persistant (dépôt bare, workspace, caches), maintenu à jour sans intervention, et supprimé avec le dépôt. Cette Solution réalise `workspaces#FR-1` (transverse) et contribue à `workspaces#BO-1`.

## Périmètre et non-objectifs

Couvert : la CR Project (spec et status) ; le PVC `project-<name>` ; le ServiceAccount, le Role et le RoleBinding `project-<name>-maint` ; les Jobs `project-<name>-init` et `project-<name>-purge` ; le CronJob `project-<name>-fetch` ; le finalizer `vanyline.solidite.fr/project-cleanup` ; l'annotation de bump des Sandboxes ; le watch des Projects.

Hors périmètre, par décision :

- la validation des entrées libres (`repoUrl`, `fetchInterval`, `storageSize`, identifiants de caches) : le controller ne valide pas (SpecDD /controller/src/project.sdd, Must not) ;
- le pod Sandbox, les worktrees et le checkout : domaine `workspaces/sandboxes` (consommés depuis `controller/src/sandbox.rs`) ;
- l'Owner et son namespace : `workspaces/owners` ;
- l'algorithme de clone, fetch et détection : `vanyline-maint` (`sandbox#BR-8`).

## Contexte applicable du domaine

- `workspaces#BR-1` : un Owner est un namespace ; le workspace, les Jobs et la CR Project y vivent.
- `workspaces#BR-2` : la CR est la seule source de vérité de la spec et du status ; la base ne porte pas l'identité Project ici.
- `workspaces#BR-7` : aucun champ de CRD n'est passé à un shell ; les Jobs invoquent `vanyline-maint` en argv (FR-6).
- `workspaces#CON-1` : une CRD se justifie par un état désiré à réconcilier (le Project en est une).
- `workspaces#CON-2` : workspace en volume bloc `RWO`, home de l'Owner en `RWX` (voir OI-3).
- `workspaces#DEC-2` : un reconciler par CRD, réconciliés par un seul controller.
- `workspaces#OI-2` : resynchronisation et watch du controller ; touche la propagation par bump (OI-19).
- `workspaces/owners#FR-7`, `workspaces/owners#FR-8` : noms `owner-<owner>-home` et `owner-<owner>`, status Owner (`pvcName`) lus par ce reconciler.

## Acteurs

- **Utilisateur :** crée un Project par l'interface, la CLI ou GitOps (`workspaces#BO-2`).
- **Controller :** réconcilie la CR Project et ses objets dérivés (SpecDD /controller/src/project.sdd, Purpose).
- **Pods de maintenance (image sandbox) :** exécutent `vanyline-maint` ; les pods init et fetch portent le SA `project-<name>-maint` pour patcher le status via `detect` ; le pod purge n'a pas de SA.

## État actuel et état cible

- **Actuel (Observed in: `controller/src/project.rs`) :** reconcile, builders et finalizer écrits ; 35 tests unitaires de construction pure ; aucun harnais d'intégration cluster dans le crate (SpecDD /controller/src/project.sdd, Structure et Tasks).
- **Cible :** Owner lu dans le namespace d'application via `namespace::app_namespace_of` ; retrait de `existing_pvc` (SpecDD /controller/src/project.sdd, Tasks « ALIGNEMENT » et « RETRAIT », décision du 2026-10-04). État contradictoire entre Must et Tasks : voir OI-1 et OI-2.

## Hypothèses, contraintes et dépendances

- **CON-1:** Un PVC ne se monte pas d'un namespace à l'autre ; le workspace et le home sont donc dans le namespace de l'Owner, et les Jobs n'ont aucun montage inter-namespaces.
    - **Source:** SpecDD /controller/src/project.sdd, Must « UN OWNER = UN NAMESPACE » (2026-10-04) ; `workspaces#BR-1`
- **CON-2:** Le contrat d'argv avec `vanyline-maint` (sous-commandes `init`, `fetch`, `purge`, `detect`) est fixé par le controller et l'image sandbox ; ils avancent ensemble (release couplée).
    - **Source:** SpecDD /controller/src/project.sdd, Depends on (contrat runtime externe) ; `workspaces#BR-7`
- **CON-3:** Les littéraux de layout (`repo.git`, `worktrees/`, `cache/`, mapping `pnpm` vers `pnpm-store`) sont dupliqués avec `sandbox/src/maint.rs` et figés des deux côtés par des tests jumeaux.
    - **Source:** SpecDD /controller/src/project.sdd, Structure et Must
- **CON-4:** Les pods de Jobs tournent avec `runAsUser` = `runAsGroup` = `fsGroup` = 1000 et `runAsNonRoot: true` ; le home est monté sur `/home/vanyline`, qui est aussi la valeur de `HOME`.
    - **Source:** SpecDD /controller/src/project.sdd, Must (`@sandbox_security_context`, `@HOME_MOUNT_PATH`)
- **CON-5:** `@FIELD_MANAGER` (`vanyline-controller`) et `@HOME_MOUNT_PATH` sont partagés avec le reconciler Owner.
    - **Source:** SpecDD /controller/src/project.sdd, Depends on (`./owner.rs`) ; `workspaces/owners#CON-3`
- **Dépendances :** `workspaces/owners` (status `pvcName`) ; les CR Sandbox du namespace (bump, `workspaces/sandboxes`) ; crate `vanyline-crds` ; `kube` 4 et `k8s-openapi` 0.28 (SpecDD /controller/src/project.sdd, Depends on).

## Exigences fonctionnelles

- **FR-1 - MUST:** Une CR Project n'est réconciliée que si son Owner (`spec.owner`, même namespace) existe avec `status.pvcName` posé. Sinon le reconcile échoue en `VNL-CTL-002` (Owner sans `pvcName`) ou `VNL-CTL-001` (Owner introuvable), et aucun objet dérivé n'est créé.
    - **Source:** SpecDD /controller/src/project.sdd, Must (précondition), Raises, Scenarios `cluster_owner_absent_reconcile_en_erreur_vnl_ctl_001`, `cluster_owner_sans_pvc_name_reconcile_en_erreur_vnl_ctl_002`
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** À chaque `Event::Apply`, le reconciler applique dans l'ordre : SA, Role, RoleBinding `project-<name>-maint`, PVC workspace, Job init (une fois), CronJob fetch (si `cloned`), patch du status, bump. Les objets passent en SSA `vanyline-controller` avec force, sauf les Jobs (create) et le status (merge patch).
    - **Source:** SpecDD /controller/src/project.sdd, Must (ordre d'application)
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** Le PVC workspace `project-<name>` est toujours créé dans le namespace du Project. Taille, mode d'accès et classe suivent la priorité spec, puis défaut Owner (`projectDefaults`), puis constantes (`10Gi`, `ReadWriteOnce`) ; sans classe, aucun repli constant n'est posé.
    - **Source:** SpecDD /controller/src/project.sdd, Must (priorité) ; Scenarios `build_workspace_pvc_*`
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** Le Job `project-<name>-init` est créé par `create` seulement s'il est absent, n'est jamais patché, et `cloned` vaut vrai si et seulement si son `status.succeeded` est supérieur à 0.
    - **Source:** SpecDD /controller/src/project.sdd, Must (lecture de `cloned`, discipline des Jobs)
    - **Verified by:** `AC-4`
- **FR-5 - MUST:** Le CronJob `project-<name>-fetch` n'est appliqué que lorsque `cloned` est vrai, avec `concurrencyPolicy: Forbid` posé par l'apply et un planning `@every <fetchInterval>` (défaut `@every 1h`), transcrit sans validation.
    - **Source:** SpecDD /controller/src/project.sdd, Must (CronJob, `@fetch_schedule`) ; Scenarios `fetch_schedule_default`, `fetch_schedule_custom`
    - **Verified by:** `AC-5`
- **FR-6 - MUST:** Chaque conteneur de Job a un `command` en argv dont le premier élément est `vanyline-maint`, sans `sh`, `-c`, ni interpolation d'un champ de CR.
    - **Source:** SpecDD /controller/src/project.sdd, Must et Must not ; `workspaces#BR-7`
    - **Verified by:** `AC-6`
- **FR-7 - MUST:** Les caches sont passés à l'init dans l'ordre et avec les doublons de la spec (`--cache <id>` répété) ; sans `spec.caches`, la liste est `cargo`, `pnpm`, `pip`.
    - **Source:** SpecDD /controller/src/project.sdd, Must (argv init, `@effective_caches`)
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** Après chaque Apply réussi, le status est écrit en merge patch avec `pvcName`, `cloned`, `observedGeneration` et une condition unique `Ready`. Les clés `languages` et `detectedAt` n'apparaissent jamais dans ce patch.
    - **Source:** SpecDD /controller/src/project.sdd, Must (`@compute_status`) ; SpecDD /crds/src/lib.sdd, Must (`ProjectStatus`)
    - **Verified by:** `AC-8`
- **FR-9 - MUST:** Les Jobs init et fetch enchaînent `git` (initContainer) puis `detect` (conteneur principal, `vanyline-maint detect --project <name>`), dans le même pod. `detect` écrit `languages` et `detectedAt` dans le status du Project.
    - **Source:** SpecDD /controller/src/project.sdd, Must (`init_command`), Scenario `init_job_detect_container_order` ; SpecDD /crds/src/lib.sdd, Purpose
    - **Verified by:** `AC-9`
- **FR-10 - MUST:** Après chaque Apply réussi, l'annotation `vanyline.solidite.fr/egress-bump` (horodatage courant) est posée par merge patch sur toutes les Sandboxes dont `spec.project` vaut le nom du Project, sans détection de changement.
    - **Source:** SpecDD /controller/src/project.sdd, Must (bump) ; `docs/architecture.md`, WS-13 (cité par la spec)
    - **Verified by:** `AC-10`
- **FR-11 - MUST:** Le requeue vaut 15 s tant que le Project n'est pas cloné et 300 s ensuite. Une erreur `PurgePending` produit un log `info` et un requeue de 10 s ; toute autre erreur produit un log `warn` et un requeue de 30 s.
    - **Source:** SpecDD /controller/src/project.sdd, Must (requeue, `@error_policy`)
    - **Verified by:** `AC-11`
- **FR-12 - MUST:** Le finalizer `vanyline.solidite.fr/project-cleanup` n'est levé qu'après la réussite du Job `project-<name>-purge` (créé une fois, sans SA). Si l'Owner est absent ou sans `status.pvcName`, le finalizer est levé sans purge, avec un log `warn` (best-effort v1).
    - **Source:** SpecDD /controller/src/project.sdd, Must (`@cleanup`, best-effort), Handles
    - **Verified by:** `AC-12`
- **FR-13 - MUST:** Le SA `project-<name>-maint` ne reçoit qu'une règle : `patch` sur `projects/status` de ce seul Project (`resourceNames`). Le SA, le Role et le RoleBinding homonymes portent chacun un ownerReference vers le Project.
    - **Source:** SpecDD /controller/src/project.sdd, Must (trio RBAC) ; Scenarios `build_maint_role_shape`, `rbac_trois_objets_owner_reference_controle`
    - **Verified by:** `AC-13`
- **FR-14 - MUST:** Les Jobs init, fetch et purge ont `backoffLimit: 3` et `ttlSecondsAfterFinished: 3600`. Le Job init, le CronJob fetch et le Job purge portent un ownerReference vers le Project (`controller: true`).
    - **Source:** SpecDD /controller/src/project.sdd, Must (discipline des Jobs) ; Scenarios `git_jobs_backoff_limit_3_et_ttl_3600`, `jobs_et_cronjob_owner_references_vers_project`
    - **Verified by:** `AC-14`
- **FR-15 - MUST:** Le watch du reconciler couvre les Projects de tous les namespaces, sans filtre, et n'observe ni les Owner ni les Sandbox.
    - **Source:** SpecDD /controller/src/project.sdd, Must (`@build_controller`, interdiction de regrouper un watch)
    - **Verified by:** `AC-15`
- **FR-16 - MUST:** Les pods de Jobs ont `HOME` = `/home/vanyline` et `GIT_SSH_COMMAND` posé inconditionnellement ; ils montent exactement les volumes `workspace` (PVC `project-<name>`) et `home` (PVC de l'Owner), sur `/workspace` et `/home/vanyline`, sans `subPath` ; `restartPolicy: Never` ; label de template `vanyline.solidite.fr/project: <name>` seulement.
    - **Source:** SpecDD /controller/src/project.sdd, Must (`@git_pod_template`) ; Scenarios `git_pod_template_env_home_et_ssh_inconditionnel`, `git_pod_template_volumes_et_mounts_fixes`, `git_pod_template_labels_du_template_seulement`
    - **Verified by:** `AC-16`

## Sécurité

- Le SA de maintenance ne peut patcher que le status de ce Project (`resourceNames`, FR-13) ; le Job purge n'a pas de SA (FR-12).
- Les pods tournent non root, UID/GID 1000 (CON-4). La clé SSH vit dans le home de l'Owner, sans Secret monté (FR-16, OI-13) ; aucune valeur de secret n'est décrite ici.

## Conception

Structure dans le namespace de l'Owner (`<app-ns>-<owner>`), où vit aussi la CR Project :

```text
Namespace <app-ns>-<owner>
  |-- CR Project <name>                 finalizer vanyline.solidite.fr/project-cleanup
  |-- PVC project-<name>                workspace, /workspace, ownerRef Project
  |-- ServiceAccount project-<name>-maint
  |-- Role / RoleBinding project-<name>-maint   patch projects/status[<name>]
  |-- Job project-<name>-init           initContainer git (init), conteneur detect
  |-- CronJob project-<name>-fetch      @every <fetchInterval>, Forbid, git fetch puis detect
  `-- Job project-<name>-purge          git purge, créé au cleanup uniquement
Sandboxes du namespace (spec.project = <name>)  annotation egress-bump
```

Ordre d'un `Event::Apply` : précondition Owner, RBAC, PVC, Job init, CronJob si `cloned`, status, bump (FR-2). Le cleanup est un chemin distinct : Job purge, `PurgePending` jusqu'au succès, levée du finalizer ; le PVC part ensuite par GC (FR-12).

## Décisions

- **DEC-1 - Le Project et son workspace vivent dans le namespace de l'Owner**
    - **Context:** Un PVC ne se monte pas entre namespaces (CON-1) ; un PVC externe référencé ne peut pas être monté par les Jobs.
    - **Decision:** Le PVC `project-<name>` est toujours créé par le controller dans le namespace de l'Owner ; aucun PVC existant n'est référencé.
    - **Alternatives:** Référencer un PVC existant (écarté le 2026-10-04).
    - **Source:** SpecDD /controller/src/project.sdd, Must (2026-10-04) ; `workspaces#BR-1`
- **DEC-2 - Maintenance git par argv, sans shell**
    - **Context:** Des champs de CR (URL, noms de caches) entrent dans les commandes des pods.
    - **Decision:** Les Jobs invoquent `vanyline-maint` en argv (`command:`), et les assainissements relèvent de `vanyline-maint`.
    - **Alternatives:** Un script shell assemblé par le controller (écarté par la règle de maintenance de l'architecture).
    - **Source:** `workspaces#BR-7` ; SpecDD /controller/src/project.sdd, Must not ; AGENTS.md, section Architecture (« Règle — maintenance des projets »)
- **DEC-3 - La protection de `languages` et `detectedAt` repose sur `skip_serializing_if`**
    - **Context:** Le patch de routine du reconciler et le Job `detect` écrivent le même status.
    - **Decision:** Les deux champs ne sont pas sérialisés quand ils sont vides ou absents ; le patch de routine ne les contient donc jamais.
    - **Consequences:** Le reconciler ne doit jamais sérialiser le status complet dans le patch de `detect` ; la décision de contrat est tracée en OI-9.
    - **Source:** SpecDD /crds/src/lib.sdd, Must (`ProjectStatus`) ; SpecDD /controller/src/project.sdd, Purpose et Must not
- **DEC-4 - `cloned` ne se déduit que de l'état du Job init, qui n'est jamais patché**
    - **Context:** Le status doit refléter le clone réel sans champ supplémentaire.
    - **Decision:** `cloned` = `status.succeeded > 0` du Job init courant ; un Job absent est recréé par `create`.
    - **Consequences:** Un Job purgé par TTL est recréé et `cloned` repasse faux (OI-5) ; un échec définitif n'est pas distingué d'une attente (OI-6).
    - **Source:** SpecDD /controller/src/project.sdd, Must
- **DEC-5 - Propagation Project vers Sandboxes par annotation de bump, sans watch inter-CRD**
    - **Context:** Les changements d'un Project doivent atteindre les Sandboxes de son dépôt.
    - **Decision:** Le reconciler horodate l'annotation `vanyline.solidite.fr/egress-bump` sur chaque Sandbox du Project, à chaque reconcile réussi.
    - **Consequences:** La propagation dépend du déclenchement du reconcile Sandbox par une annotation (`workspaces/owners#OI-2`, OI-19).
    - **Source:** SpecDD /controller/src/project.sdd, Must (bump) ; `docs/architecture.md`, WS-13
- **DEC-6 - Cleanup best-effort quand l'Owner a disparu**
    - **Context:** Un Owner supprimé supprime son namespace, donc le PVC et ses données, avant la purge.
    - **Decision:** Si l'Owner est absent ou sans `pvcName`, le finalizer est levé sans Job purge.
    - **Consequences:** Aucun git purge n'est exécuté ; le PVC et ses données partent avec le namespace de l'Owner.
    - **Source:** SpecDD /controller/src/project.sdd, Must (« best-effort v1 »), Handles

## Réalisation SpecDD

- `SpecDD /controller/src/project.sdd` : reconcile, builders de Jobs, RBAC, PVC, status, bump, cleanup et requeues (FR-1 à FR-16).
- `SpecDD /crds/src/lib.sdd`, partie Project : `ProjectSpec`, `ProjectStatus`, printcolumns, sérialisation anti-écrasement (FR-8, DEC-3).
- `controller/src/sandbox.rs` (consommateur interne) : sa spec n'est pas relue ; `sandbox/src/maint.rs` (contrat `vanyline-maint`) est hors périmètre.

## Acceptation et vérification

Les Scenarios `cluster_*` nécessitent un harnais d'intégration qui n'existe pas (OI-18). Les AC ci-dessous s'appuient sur les tests unitaires existants, à compléter, ou sur les Scenarios cluster non écrits.

- **AC-1 (FR-1):** Owner absent : `VNL-CTL-001` ; Owner sans `pvcName` : `VNL-CTL-002` ; aucun objet créé. Vérif. : `cluster_owner_absent_reconcile_en_erreur_vnl_ctl_001`, `cluster_owner_sans_pvc_name_reconcile_en_erreur_vnl_ctl_002` (non écrits).
- **AC-2 (FR-2):** Premier Apply complet : SA, Role, RoleBinding, PVC, Job init présents ; `cloned: false`, requeue 15 s. Vérif. : `cluster_premier_reconcile_cree_rbac_pvc_et_init_job` (non écrit).
- **AC-3 (FR-3):** Taille, mode d'accès et classe selon la priorité. Vérif. : `build_workspace_pvc_default_size`, `build_workspace_pvc_owner_default_size`, `build_workspace_pvc_spec_overrides_default`, `build_workspace_pvc_access_mode_*` (unitaires) ; `build_workspace_pvc_classe_*` (à convertir).
- **AC-4 (FR-4):** Job init créé une fois, uid stable. Vérif. : `build_init_job_shape` (unitaire) ; `cluster_init_job_cree_une_seule_fois` (non écrit).
- **AC-5 (FR-5):** CronJob `Forbid` et planning. Vérif. : `build_fetch_cronjob_shape`, `fetch_schedule_default`, `fetch_schedule_custom` (unitaires) ; `cluster_reussite_init_cree_cronjob_concurrency_forbid` (non écrit).
- **AC-6 (FR-6):** Argv sans shell. Vérif. : `git_pod_template_no_shell`, `build_purge_job_shape` (unitaires).
- **AC-7 (FR-7):** Caches ordonnés et dupliqués. Vérif. : `effective_caches_default`, `effective_caches_custom`, `init_job_caches_repetes_dans_l_ordre_de_la_spec` (à convertir).
- **AC-8 (FR-8):** Patch sans `languages` ni `detectedAt`. Vérif. : `compute_status_cloned_true`, `compute_status_cloned_false`, `compute_status_json_omet_languages_et_detected_at` ; `cluster_status_merge_n_ecrase_pas_languages` (non écrit).
- **AC-9 (FR-9):** Ordre init puis detect. Vérif. : `init_job_detect_container_order` (unitaire) ; `project_status_languages_skipped_when_empty` (crds).
- **AC-10 (FR-10):** Bump sur les seules Sandboxes du Project, valeur renouvelée. Vérif. : `cluster_bump_annotation_sur_sandboxes_du_project_seulement` (non écrit).
- **AC-11 (FR-11):** Requeues 15 s / 300 s, 10 s pour `PurgePending`. Vérif. : `cluster_requeue_15s_sans_clone_300s_apres_clone`, `cluster_purge_pending_requeue_10s` (non écrits).
- **AC-12 (FR-12):** Job purge créé, finalizer tenu jusqu'au succès ; Owner absent : pas de purge. Vérif. : `cluster_cleanup_cree_job_purge_puis_leve_finalizer`, `cluster_cleanup_owner_absent_purge_sautee` (non écrits).
- **AC-13 (FR-13):** Role à règle unique, ownerRefs. Vérif. : `build_maint_role_shape`, `build_maint_role_binding_shape`, `rbac_trois_objets_owner_reference_controle` (à convertir), `git_jobs_service_account`.
- **AC-14 (FR-14):** `backoffLimit: 3`, TTL 3600, ownerRefs. Vérif. : `git_jobs_backoff_limit_3_et_ttl_3600`, `jobs_et_cronjob_owner_references_vers_project` (à convertir).
- **AC-15 (FR-15):** Watch sur tous les namespaces. Vérif. : `cluster_watch_couvre_tous_les_namespaces` (non écrit).
- **AC-16 (FR-16):** Env, volumes et mounts fixes. Vérif. : `git_pod_template_env_home_et_ssh_inconditionnel`, `git_pod_template_volumes_et_mounts_fixes`, `git_pod_template_labels_du_template_seulement`, `git_pod_template_restart_policy_never`.

## Open Issues

- **OI-1 - Lecture de l'Owner : namespace courant ou cible ?** Le Must décrit la lecture cross-namespace (`namespace::app_namespace_of`) au présent, mais la tâche « ALIGNEMENT » est `[ ]` et les Scenarios supposent encore le même namespace. **TBD :** quel comportement est en vigueur. Lié à `workspaces/owners#OI-1`.
- **OI-2 - Contradiction sur le PVC référencé.** La Purpose annonce « PVC existant référencé jamais géré » ; le Must et la tâche « RETRAIT » disent que `existingPvc` est retiré ; `crds/src/lib.sdd` (Must) écrit « retirés ». **TBD :** texte de référence, et état réel du code.
- **OI-3 - Mode d'accès du workspace.** `workspaces#CON-2` impose un workspace `RWO`, mais `storageAccessMode` est une chaîne libre et le défaut Owner peut être `ReadWriteMany` (`workspaces/owners#FR-6`). **TBD :** contrainte du domaine (défaut ou invariant) ?
- **OI-4 - Documentation du PVC référencé non vérifiée.** `docs/architecture.md` décrit le PVC comme « créé ou référence vérifiée » ; le code ne vérifie jamais le PVC référencé (tâche `[!]`). **TBD :** corriger la doc ou ajouter la vérification.
- **OI-5 - Recréation du Job init après TTL.** Après 3600 s, le Job init disparaît, est recréé, `cloned` repasse faux, `Ready: False` et `init` rejoué en boucle quasi permanente (tâche `[?]`). **TBD :** comportement voulu, ou ne jamais recréer après succès ?
- **OI-6 - Échec définitif du Job init invisible.** Avec `backoffLimit` épuisé, `cloned` reste faux et la raison reste `WaitingForInitJob` (tâche `[?]`). **TBD :** troisième état `InitJobFailed` ou mécanisme de recréation assumé ?
- **OI-7 - `repoUrl` figé après clone.** Un changement de `spec.repoUrl` ne redirige pas le bare (`origin` garde l'URL d'origine ; tâche `[?]`). **TBD :** contrat assumé, ou action de reconcile (`set-url`, Job régénéré) ?
- **OI-8 - `lastTransitionTime` re-stampé à chaque reconcile.** La condition est réécrite à chaque tour (tâche `[?]`). Même question que `workspaces/owners#OI-5`. **TBD.**
- **OI-9 - Champs `null` et `cloned` toujours sérialisés.** `lastFetch: null` et `worktrees: []` sont écrits à chaque tour ; un champ alimenté plus tard serait effacé. Décision à prendre dans `crds/src/lib.sdd` (tâches `[?]` de la Purpose et de la sérialisation). **TBD.**
- **OI-10 - Owner `Ready` non exigé.** Seul `status.pvcName` est testé (tâche `[?]`). **TBD :** le garde-fou actuel est-il le contrat ?
- **OI-11 - `fetchInterval` non validé.** `"1heures"` est transmis tel quel, le CronJob échoue silencieusement, sans reflet dans le status (tâche `[?]`). **TBD :** rejet explicite ou entrée système assumée.
- **OI-12 - Changements immuables sur PVC lié.** Modifier `storageSize` à la baisse, `storageAccessMode` ou `storageClass` fait échouer le SSA avec requeue 30 s en boucle (tâche `[?]`). **TBD :** erreur visible assumée, ou patch conditionné ?
- **OI-13 - Chemin SSH en dur.** `GIT_SSH_COMMAND` contient `/home/vanyline/.ssh/id_ed25519` en dur alors que `HOME` vient de `@HOME_MOUNT_PATH` (tâche `[?]`). **TBD :** construire depuis la constante, ou test d'arrêteur.
- **OI-14 - `concurrencyPolicy: Forbid` hors builder.** Le builder le laisse à `Allow`, seul l'apply le pose (tâche `[?]`). **TBD :** le contrat doit-il vivre dans le builder avec son Scenario ?
- **OI-15 - Suppression de `gitSecret`.** Le champ est déprécié sans échéance et ignoré (`crds/src/lib.sdd`, tâche `[?]`). **TBD :** version cible de suppression.
- **OI-16 - Fallback namespace `"default"`.** Présent dans le reconcile Project (Handles), comme dans `workspaces/owners#OI-7`. **TBD :** garder le repli ou erreur explicite.
- **OI-17 - Commentaire périmé sur les caches par défaut.** Un test de `controller/src/sandbox.rs` annonce `["cargo", "pnpm"]` alors que le défaut est `["cargo", "pnpm", "pip"]` (`crds/src/lib.sdd`, tâche `[?]`). **TBD :** à tracer dans la spec du reconciler Sandbox.
- **OI-18 - Harnais d'intégration cluster absent (tâche ouverte).** Les Scenarios `cluster_*` ne sont pas écrits ; plusieurs Scenarios unitaires sont à convertir ou compléter (tâches `[ ]`). Pas de décision requise.
- **OI-19 - Déclenchement du reconcile Sandbox par annotation.** La propagation DEC-5 suppose qu'une annotation de bump réveille le reconcile Sandbox ; ce n'est pas démontré (`workspaces/owners#OI-2`, `workspaces#OI-2`). **TBD.**

## Références

- `workspaces` : `BO-1`, `BO-2`, `FR-1`, `BR-1`, `BR-2`, `BR-7`, `CON-1`, `CON-2`, `DEC-2`, `OI-2`.
- `workspaces/owners` : `FR-6`, `FR-7`, `FR-8`, `CON-3`, `OI-1`, `OI-2`, `OI-5`, `OI-7`.
- SpecDD /controller/src/project.sdd, SpecDD /crds/src/lib.sdd (partie Project).
- `sandbox` : `sandbox#BR-8` (maintenance des workspaces par `vanyline-maint`).
- AGENTS.md, section Architecture ; `docs/architecture.md`, WS-13 et « Maintenance des workspaces », cités par les specs, non relus pour ce document.
