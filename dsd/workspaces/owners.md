# Solution Design: Owners

## Résumé

La Solution `workspaces/owners` réalise l'identité cluster d'un utilisateur : la CR `Owner`, le namespace `<app-ns>-<owner>` qui le contient, son ServiceAccount d'identité et son PVC home. Elle est réalisée par le reconciler `controller/src/owner.rs`, le cycle de vie du namespace `controller/src/namespace.rs` et les types de `vanyline-crds`. La règle « un Owner = un namespace » est `workspaces#BR-1` ; ce document en décrit la descente en exigences locales, décisions et points ouverts.

## Problème et objectif

Un utilisateur doit disposer d'une identité cluster qui contient tout ce qu'il produit (Projects, Sandboxes, ressources) et disparaît entièrement avec lui. Le namespace est la frontière d'isolation : un jeton de ServiceAccount, un RoleBinding ou une NetworkPolicy ne dépasse jamais le namespace d'un Owner (SpecDD /controller/src/namespace.sdd, Purpose). Cette Solution contribue à `workspaces#BO-1` pour la part identité.

## Périmètre et non-objectifs

Couvert : la CR Owner (spec et status), le namespace de l'Owner et ses labels, le ServiceAccount, le PVC home, le RoleBinding délégué à `app`, le bump egress vers les Sandboxes, la suppression par finalizer.

Hors périmètre : le Project et ses Jobs (`workspaces/projects`) ; la Sandbox, son Ingress et ses NetworkPolicies (`workspaces/sandboxes`, `workspaces/network`) ; la construction des règles egress (SpecDD /controller/src/sandbox.sdd) ; les quotas, inexistants (SpecDD /controller/src/owner.sdd, Purpose ; `workspaces#OI-3`) ; le jeton de sandbox et sa vérification (SpecDD /sandbox/src/auth.sdd).

## Contexte applicable du domaine

* `workspaces#BR-1` : un Owner = un namespace `<app-ns>-<owner>` ; sa suppression supprime le namespace. Non redéfini ici.
* `workspaces#BR-2` : la CR est la seule source de vérité de `spec` et `status`.
* `workspaces#BR-7` : aucun champ de CRD dans un shell ; cette Solution n'introduit aucune commande.
* `workspaces#DEC-2` : quatre CRDs ; l'Owner en est une.

## Acteurs

* **Utilisateur :** crée son Owner par l'interface, la CLI ou GitOps (`workspaces#BO-2`).
* **Controller :** réconcilie la CR Owner, son namespace et ses enfants.
* **`app` :** demande le jeton du ServiceAccount après avoir vérifié le lien utilisateur-Owner (AGENTS.md, Architecture) ; reçoit le RoleBinding de délégation si l'Owner référence une Application.
* **Opérateur :** peut demander le même jeton avec son kubeconfig (SpecDD /controller/src/owner.sdd, Must).
* **Administrateur du cluster :** installe CRDs, controller et ClusterRole `vanyline-app-owner-scope` (`deploy/controller/controller.yaml`, SpecDD /controller/src/namespace.sdd).

## État actuel et état cible

* **Actuel :** les 13 Scenarios de SpecDD /controller/src/namespace.sdd ont un test homonyme dans `controller/src/namespace.rs` (vérifié le 2026-10-17). `owner.rs` a dix tests unitaires sur constructeurs et `compute_status`, sans test sur cluster réel (SpecDD /controller/src/owner.sdd, Tasks).
* **Cible :** un Owner = un namespace, sans ownerReference sur les enfants, avec FR-1 à FR-13. La purge des passages qui décrivent encore des ownerReferences est ouverte (OI-8).

## Hypothèses, contraintes et dépendances

* **CON-1:** Un objet namespacé ne peut pas posséder un objet cluster-scopé ; Kubernetes interdit l'ownerReference inter-namespaces.
    - **Source:** SpecDD /controller/src/namespace.sdd (Build) ; SpecDD /controller/src/owner.sdd (Must not)

* **CON-2:** Un PVC ne se monte pas d'un namespace à un autre ; le namespace de l'Owner n'existe qu'après la création de l'Owner.
    - **Source:** SpecDD /controller/src/namespace.sdd (Tasks, décision du 2026-10-04) ; SpecDD /controller/src/owner.sdd (Tasks)

* **CON-3:** `@FIELD_MANAGER` = `vanyline-controller` et `@HOME_MOUNT_PATH` = `/home/vanyline` sont des contrats partagés : changer le premier change la possession des champs côté apiserver, diverger du second casse la clé SSH documentée.
    - **Source:** SpecDD /controller/src/owner.sdd (Must)

* **CON-4:** Le controller observe la CR Owner dans tous les namespaces, sans sélecteur de labels.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, `@build_controller`)

* **Dépendances :** crate `vanyline-crds` ; kube-runtime. Consommateurs : SpecDD /controller/src/project.sdd et SpecDD /controller/src/sandbox.sdd (voir Conception, Interfaces).

## Exigences fonctionnelles

* **FR-1 - MUST:** Le reconcile d'un Owner exécute dans cet ordre : finalizer, création ou adoption du namespace `<app-ns>-<owner>`, PVC home, ServiceAccount, patch du status, bump egress, puis requeue à 300 s. Une erreur interrompt les étapes suivantes, sans rollback.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, ordre de `@reconcile`) ; SpecDD /controller/src/namespace.sdd (Handles)
    - **Verified by:** `AC-1`

* **FR-2 - MUST:** Le nom `<app-ns>-<owner>` est validé (label DNS-1123, 63 caractères au plus) avant tout appel à l'apiserver ; un nom invalide est refusé en `VNL-CTL-007`.
    - **Source:** SpecDD /controller/src/namespace.sdd (Must ; Scenarios `nom_trop_long`, `nom_invalide`) ; SpecDD /crds/src/lib.sdd (`owner_namespace_name`)
    - **Verified by:** `AC-2`

* **FR-3 - MUST:** Un namespace homonyme sans les labels de cet Owner n'est ni adopté, ni modifié, ni supprimé ; `VNL-CTL-008` est rendu et le namespace reste intact.
    - **Source:** SpecDD /controller/src/namespace.sdd (Must ; Scenarios `collision_avec_un_namespace_etranger`, `suppression_ne_touche_pas_un_namespace_etranger`)
    - **Verified by:** `AC-3`

* **FR-4 - MUST:** À la suppression de la CR, le namespace est supprimé seulement s'il porte les labels de l'Owner ; le finalizer `vanyline.solidite.fr/owner-namespace` n'est retiré qu'à la disparition effective du namespace (`Pending` entre-temps).
    - **Source:** SpecDD /controller/src/namespace.sdd (Handles ; Scenario `suppression_par_finalizer`)
    - **Verified by:** `AC-4`

* **FR-5 - MUST:** Le PVC `owner-<owner>-home` et le ServiceAccount `owner-<owner>` sont créés dans le namespace de l'Owner, sans ownerReference, labels ni annotations, avec la seule spec déclarée par le constructeur.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, constructeurs ; Scenarios `build_home_pvc_metadata_champs_nus`, `build_service_account_champs_nus`)
    - **Verified by:** `AC-1`

* **FR-6 - MUST:** Le PVC home a pour taille `homeSize` (défaut `1Gi`), pour mode d'accès `homeAccessMode` (défaut `ReadWriteMany`) et pour StorageClass `homeStorageClass`, à défaut celle du cluster.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, défauts métier) ; SpecDD /crds/src/lib.sdd (Must, `OwnerSpec`)
    - **Verified by:** `AC-1`

* **FR-7 - MUST:** Les noms `owner-<owner>-home` et `owner-<owner>` sont le contrat de nommage lu par le controller et par `app`.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, `@home_pvc_name`, `@service_account_name`)
    - **Verified by:** `AC-1`

* **FR-8 - MUST:** Après des apply réussis, le status porte `namespace`, `pvcName`, `serviceAccount` et une condition unique `Ready` (`True`, reason `Reconciled`), écrits en merge patch. Une erreur de reconcile n'écrit aucun status.
    - **Source:** SpecDD /controller/src/owner.sdd (Must ; Must not)
    - **Verified by:** `AC-8`

* **FR-9 - MUST:** Un reconcile réussi répété ne produit ni `AlreadyExists`, ni condition dupliquée, et rend un requeue de 300 s.
    - **Source:** SpecDD /controller/src/owner.sdd (Scenario `reconcile_double_pass_idempotent_sans_duplication`)
    - **Verified by:** `AC-6`

* **FR-10 - MUST:** Toute erreur de reconcile d'un Owner est suivie d'un requeue à 30 s, sans distinction de nature ni backoff.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, `@error_policy` ; Scenario `error_policy_requeue_trente_secondes`)
    - **Verified by:** `AC-8`

* **FR-11 - MUST:** Le ServiceAccount `owner-<owner>` n'est désigné par aucun Role, ClusterRole, RoleBinding ni ClusterRoleBinding.
    - **Source:** SpecDD /controller/src/owner.sdd (Must not ; Scenario `sa_owner_sans_aucun_droit_rbac`)
    - **Verified by:** `AC-5`

* **FR-12 - MUST:** Le RoleBinding `vanyline-app` délègue à l'application le seul ClusterRole `vanyline-app-owner-scope`, et n'existe que si `application_ref` résout une Application (voir OI-1).
    - **Source:** SpecDD /controller/src/namespace.sdd (Must ; Handles ; Scenarios `role_binding_de_l_app`, `pas_de_role_binding_sans_application`)
    - **Verified by:** `AC-5`

* **FR-13 - MUST:** À chaque reconcile réussi, l'annotation `vanyline.solidite.fr/egress-bump` (horodatage RFC 3339) est posée sur les Sandboxes dont le Project appartient à l'Owner ; sans Project, aucune écriture (voir OI-2).
    - **Source:** SpecDD /controller/src/owner.sdd (Must, bump ; Scenarios `reconcile_bump_annotation_sur_sandboxes_des_projects`, `reconcile_bump_inerte_sans_project`)
    - **Verified by:** `AC-7`

## Sécurité

* L'isolation repose sur le namespace : jeton, RoleBinding et NetworkPolicy ne dépassent pas l'Owner (DEC-1).
* Le ServiceAccount est une pure identité : son jeton est la clé de la sandbox de l'Owner, et des droits attachés à ce SA seraient acquis par quiconque obtient ce jeton ou le lit dans le pod (FR-11, DEC-3).
* Le controller ne détient pas `serviceaccounts/token` ; la règle cluster-wide qui le donnait a été retirée (SpecDD /controller/src/namespace.sdd, Tasks, revue du 2026-10-14). Le ClusterRole délégué à `app` contient en revanche `serviceaccounts/token` en `create` (même source).
* Un namespace n'est ni adopté ni supprimé sans les labels de l'Owner (FR-3, DEC-5).

## Conception

Structure, dans le namespace d'application (la CR Owner y reste) :

```text
CR Owner (<app-ns>)        finalizer vanyline.solidite.fr/owner-namespace
  `-- Namespace <app-ns>-<owner>
        labels: vanyline.solidite.fr/owner=<owner>
                vanyline.solidite.fr/app-namespace=<app-ns>
                app.kubernetes.io/managed-by=vanyline-controller
        |-- ServiceAccount owner-<owner>     identité, sans RBAC
        |-- PVC owner-<owner>-home           RWX, 1Gi par défaut
        `-- RoleBinding vanyline-app         si application_ref résout (ClusterRole vanyline-app-owner-scope)
```

`owner.rs` orchestre ; `namespace.rs` crée, étiquette et nettoie le namespace. Le status est un merge patch sur la sous-ressource `status`. Le bump liste les Projects du namespace de l'Owner, puis les Sandboxes de ces Projects.

Interfaces consommées par d'autres Solutions :

* `owner-<owner>-home`, `owner-<owner>`, `<app-ns>-<owner>` (FR-2, FR-5, FR-7).
* `app_namespace_of(namespace)` : retrouve le namespace d'application à partir du label du namespace d'Owner, pour les CR Project et Sandbox (SpecDD /controller/src/namespace.sdd, Exposes).
* `status.pvcName` et `status.serviceAccount` : lus par le reconciler Project, qui rend `OwnerNotReady` si le status manque (SpecDD /controller/src/owner.sdd, Consommateurs internes).
* `@HOME_MOUNT_PATH` : monté par les Jobs git et les pods Sandbox (CON-3).

## Décisions

* **DEC-1 - Un Owner = un namespace dédié**
    - **Context:** L'Owner doit contenir son SA, son home, ses Projects et Sandboxes ; un PVC ne se monte pas d'un namespace à l'autre (CON-2).
    - **Decision:** Un namespace `<app-ns>-<owner>` par Owner, créé par le controller avant tout enfant ; la CR reste dans le namespace d'application.
    - **Consequences:** Le namespace est la frontière d'isolation et l'unité de nettoyage.
    - **Source:** SpecDD /controller/src/namespace.sdd (Purpose) ; SpecDD /controller/src/owner.sdd (Purpose)

* **DEC-2 - Pas d'ownerReference ; le namespace est le seul nettoyage**
    - **Context:** Kubernetes interdit l'ownerReference inter-namespaces (CON-1).
    - **Decision:** Aucun enfant n'a d'ownerReference ; la CR nettoie par son finalizer, qui supprime le namespace.
    - **Consequences:** Aucun enfant n'est supprimé individuellement ; `owner_reference` et son `expect` ont disparu.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, Must not, Tasks résolue) ; SpecDD /controller/src/namespace.sdd (Handles)

* **DEC-3 - Le ServiceAccount de l'Owner est une identité sans droit**
    - **Context:** Le jeton du SA est la clé d'entrée dans la sandbox.
    - **Decision:** SA créé avec une spec par défaut, sans RBAC ; la délégation à `app` passe par un RoleBinding dans le namespace, jamais par le SA.
    - **Consequences:** Un jeton d'Owner n'ouvre que la sandbox de son Owner, sans droit Kubernetes.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, Must not) ; SpecDD /controller/src/namespace.sdd (Must not)

* **DEC-4 - Le home est toujours créé par le controller**
    - **Context:** `existingPvc` retiré le 2026-10-04 (CON-2).
    - **Decision:** Le PVC `owner-<owner>-home` est créé dans le namespace de l'Owner, sans référence à un PVC externe.
    - **Consequences:** Un PVC préexistant de même nom est adopté par l'apply (OI-4).
    - **Source:** SpecDD /crds/src/lib.sdd (Must, `OwnerSpec`) ; SpecDD /controller/src/owner.sdd (Tasks, retrait de `existing_pvc`)

* **DEC-5 - Un namespace se reconnaît à ses labels, pas à son nom**
    - **Context:** Un Owner nommé pour viser `kube-system` ou le namespace d'un autre Owner ne doit pas en prendre le contrôle.
    - **Decision:** Un namespace n'est géré que s'il porte les labels `owner` et `app-namespace` aux valeurs attendues.
    - **Consequences:** Un namespace étranger n'est jamais modifié ni supprimé, même au nom attendu (FR-3).
    - **Source:** SpecDD /controller/src/namespace.sdd (Must, Purpose)

* **DEC-6 - Patch à chaque reconcile, sans détection de changement**
    - **Context:** Les changements d'`Owner.spec.egress` doivent atteindre les Sandboxes sans watch inter-CRD permanent.
    - **Decision:** Bump et status sont posés à chaque reconcile réussi, sans comparaison (choix WS-13, cité par owner.sdd).
    - **Consequences:** Le status est réécrit à chaque passage (OI-5) ; l'efficacité du bump dépend de OI-2.
    - **Source:** SpecDD /controller/src/owner.sdd (Must, bump ; Handles)

## Réalisation SpecDD

* `SpecDD /controller/src/owner.sdd` : reconciler Owner, constructeurs, status, bump (FR-1, FR-5 à FR-11, FR-13).
* `SpecDD /controller/src/namespace.sdd` : namespace, labels, finalizer, RoleBinding (FR-2 à FR-4, FR-12).
* `SpecDD /crds/src/lib.sdd` : partie Owner de `vanyline-crds` (`OwnerSpec`, `OwnerStatus`, `owner_namespace_name`, CRD).

## Acceptation et vérification

* **AC-1:** Un Owner `alice` dans `app-ns` produit `app-ns-alice`, qui contient `owner-alice-home` (RWX, `1Gi`) et `owner-alice`, sans ownerReference, et un status renseigné (Scenario `reconcile_nominal_cree_pvc_sa_et_status_requeue_300s`, voir OI-8).
* **AC-2:** Un nom donnant un namespace de plus de 63 caractères ou non DNS-1123 est refusé en `VNL-CTL-007`, sans appel à l'apiserver.
* **AC-3:** Un namespace `app-ns-<owner>` sans les labels de l'Owner rend `VNL-CTL-008` et ne reçoit aucun patch.
* **AC-4:** La suppression d'un Owner supprime son namespace, reste `Pending` tant qu'il est `Terminating`, puis retire le finalizer ; le namespace d'un autre Owner reste intact.
* **AC-5:** Aucun Role ni RoleBinding ne désigne le SA `owner-<owner>` ; `vanyline-app` n'existe que si `application_ref` résout, et renvoie vers `vanyline-app-owner-scope`.
* **AC-6:** Deux reconciles successifs réussissent sans `AlreadyExists` et laissent une seule condition `Ready`.
* **AC-7:** Seule la Sandbox dont le Project appartient à l'Owner reçoit l'annotation de bump, dont la valeur change à chaque reconcile. Le déclenchement du reconcile Sandbox reste à vérifier (OI-2).
* **AC-8:** Une erreur de reconcile laisse le status inchangé et rend un requeue de 30 s.

Vérification actuelle : tests unitaires des constructeurs, de `compute_status`, de `error_policy` et de `namespace.rs`. Les Scenarios d'intégration sur cluster réel ne sont pas écrits (SpecDD /controller/src/owner.sdd, Tasks).

## Open Issues

* **OI-1 - Lecture de l'Application par le reconcile Owner:** SpecDD /controller/src/owner.sdd affirme que le reconciler ne consomme pas `application_ref` et ne lit aucune CR Application ; SpecDD /controller/src/namespace.sdd crée le RoleBinding quand `application_ref` résout, via `ensure_namespace` appelé depuis `owner.rs`. **TBD :** quelle spec fait foi, et où a lieu la résolution.
* **OI-2 - Déclenchement du bump:** aucun `reconciler_trigger` n'est posé et kube-runtime ne réveille par défaut que sur changement de `metadata.generation` ; une annotation pourrait ne pas déclencher le reconcile Sandbox avant son requeue de 300 s (SpecDD /controller/src/owner.sdd, `[?]`). Scenario d'arbitrage non écrit. Voisin de `workspaces#OI-2` (watch et resynchronisation) sans le recouvrir : celui-ci porte sur la fréquence de resynchronisation, pas sur le déclenchement par annotation. **TBD.**
* **OI-3 - Sémantique de `Ready: True`:** la condition signale l'acceptation des apply, pas un PVC `Bound`, et aucun état dégradé n'est écrit (SpecDD /controller/src/owner.sdd, `[?]`). **TBD :** contrat assumé, ou conditions d'échec à ajouter.
* **OI-4 - PVC préexistant:** l'adoption par apply `force` d'un PVC `owner-<owner>-home` créé hors du controller, et le blocage permanent après agrandissement hors bande (shrink `Forbidden`), sont figés par Scenarios (`reconcile_pvc_preexistant_adopte_par_apply_force`, `reconcile_pvc_grossi_hors_bande_bloque_le_reconcile`). **TBD :** comportements voulus ?
* **OI-5 - Churn du status:** `lastTransitionTime` est horodaté à chaque appel de `compute_status` ; chaque patch réécrit l'objet (SpecDD /controller/src/owner.sdd, `[?]`). **TBD :** accepté, ou horodatage conservé si seul le timestamp change.
* **OI-6 - Validation du stockage:** `homeStorageClass = ""` est transmis tel quel ; un `homeSize` mal formé n'est rejeté qu'à l'apiserver, avec erreur répétée à 30 s (SpecDD /controller/src/owner.sdd, `[?]`). **TBD :** valider, ou pass-through assumé.
* **OI-7 - Repli de namespace `default`:** `owner.namespace().unwrap_or("default")` est jugé branche morte pour une CRD namespacée (SpecDD /controller/src/owner.sdd, `[?]`). **TBD :** garder le repli, ou rendre une erreur explicite.
* **OI-8 - Textes résiduels d'ownerReference:** le Must et le Scenario `build_home_pvc_sans_owner_reference` excluent les ownerReferences, mais Handles, les Scenarios `reconcile_nominal_cree_pvc_sa_et_status_requeue_300s` et `reconcile_pvc_preexistant_adopte_par_apply_force`, et la tâche `[ ] ALIGNEMENT` de SpecDD /controller/src/owner.sdd en parlent encore. **TBD :** confirmer leur purge.
* **OI-9 - Verbe `bind` sur le ClusterRole:** la création du RoleBinding `vanyline-app` avec le seul `bind` n'est pas vérifiée sur cluster réel (SpecDD /controller/src/namespace.sdd, Tasks). **TBD.**
* **OI-10 - Lecture des namespaces par `app`:** un `get` sur les namespaces avec `resourceNames` est envisagé si un besoin apparaît (SpecDD /controller/src/namespace.sdd, `[?]`). **TBD.**

## Références

* `workspaces` : `BR-1`, `BR-2`, `BR-7`, `BO-1`, `BO-2`, `DEC-2`, `OI-2`, `OI-3`.
* SpecDD /controller/src/owner.sdd, SpecDD /controller/src/namespace.sdd, SpecDD /crds/src/lib.sdd.
* AGENTS.md, section Architecture ; `docs/architecture.md` (WS-13) et `deploy/controller/controller.yaml`, cités par les specs, non relus pour ce document.
