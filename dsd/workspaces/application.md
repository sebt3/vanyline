# Solution Design: Application

## Résumé

La Solution `workspaces/application` réalise le déploiement de l'instance `app` et, optionnellement, du moteur Restate, à partir de la CR `Application` (`vanyline.solidite.fr/v1alpha1`). Le reconciler `controller/src/application.rs` applique, dans le namespace de la CR, le Deployment, le Service et l'Ingress de `app`, son identité RBAC, son Secret cookie auto-généré si nécessaire, et, si `spec.restate` est présent, le StatefulSet et le Service Restate. Les valeurs sensibles ne transitent jamais par la CR : elles sont référencées par `secretRef`. Ce document descend `workspaces#FR-4` en exigences locales, décisions et points ouverts, sans redéfinir le domaine.

## Problème et objectif

Déclarer une instance de `app` par une seule CR, lisible par `kubectl` ou GitOps, sans assembler soi-même le Deployment, les secrets de cookie, les droits RBAC ni le moteur de workflow (`workspaces#BO-2`, `workspaces#DEC-2`).

## Périmètre et non-objectifs

Couvert : les sept ressources enfants de l'Application (Secret cookie, ServiceAccount, Role, RoleBinding, Deployment, Service, Ingress), les ressources Restate déclarées par `spec.restate` (StatefulSet, Service, NetworkPolicy), la phase et le statut, la politique de requeue et d'erreur.

Hors périmètre :

- le contenu et le comportement d'`app` (domaine `app`) ;
- le moteur de workflow Restate et les workflows eux-mêmes : fonctionnalité de miryad-core, enrichie par `app` (`app`, section Périmètre et non-objectifs ; `workspaces`, section Périmètre et non-objectifs) ; seul son déploiement comme sous-ressource de l'Application relève de cette Solution ;
- le Project, la Sandbox et l'Owner (`workspaces/projects`, `workspaces/sandboxes`, `workspaces/owners`), y compris le RoleBinding `vanyline-app` qu'ils reçoivent (`workspaces/owners#FR-12`) ;
- l'installation de cert-manager et des StorageClass (prérequis externes, CON-2).

## Contexte applicable du domaine

- `workspaces#FR-4` : une Application déploie `app` (Deployment, Service, Ingress) avec ses secrets référencés ; si `restate` est déclaré, elle déploie aussi Restate. Exigence source de cette Solution.
- `workspaces#DEC-2` : quatre CRDs, une par état désiré distinct ; l'Application en est une, réconciliée en parallèle par un seul controller.
- `workspaces#CON-1` : une CRD ne se justifie que par un état désiré à réconcilier, ce qui vaut pour la CR Application.
- `workspaces`, section Périmètre et non-objectifs : le moteur Restate lui-même est hors périmètre du domaine `workspaces`.

## Acteurs

- **Administrateur du cluster ou opérateur GitOps :** crée et modifie la CR Application (`spec.image`, `spec.restate`, références de secrets).
- **Controller :** réconcilie la CR et ses enfants (SpecDD /controller/src/application.sdd, Purpose).
- **`app` :** consomme le Deployment et son environnement ; son ServiceAccount reçoit le Role décrit en FR-5.
- **Restate :** moteur de workflow déployé comme sous-ressource ; il appelle le endpoint de services d'`app` et reçoit les appels de callback des sandboxes (SpecDD /controller/src/application.sdd, Must et Tasks).
- **cert-manager :** émet le certificat de l'Ingress à partir de l'annotation posée par le controller (CON-2).

## État actuel et état cible

- **Spec :** SpecDD /controller/src/application.sdd décrit le reconciler complet, y compris le bloc Restate. Son Structure annonce 25 tests purs dans `controller/src/application.rs`, sans cluster.
- **Restate :** le contrat `RestateSpec` est marqué fait dans SpecDD /crds/src/lib.sdd (S6c, 2026-10-13), alors que `vanyline.sdd` et `workspaces#OI-4` le présentent encore comme à écrire. Voir OI-1.
- **Cible :** un StatefulSet Restate mono-nœud, un Service à trois ports nommés, un endpoint de services exposé sur le Service d'application, et la NetworkPolicy décrite en FR-10 (SpecDD /controller/src/application.sdd, Tasks).

## Hypothèses, contraintes et dépendances

- **CON-1:** Le controller observe les CR Application dans tous les namespaces, sans sélecteur de labels ; son RBAC est celui de `deploy/controller/`.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@build_controller`)
- **CON-2:** cert-manager (ingress-shim) est présent dans le cluster et émet le Certificate ; le controller ne dérive ni certificat ni issuer, et aucun mode sans TLS n'existe.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Must not
- **CON-3:** Restate est mono-nœud, amorcé au démarrage ; la haute disponibilité d'un cluster Restate est hors périmètre.
    - **Source:** SpecDD /controller/src/application.sdd, Tasks (DÉCIDÉ 2026-10-14, arbitrage du développeur)
- **CON-4:** Les noms et labels suivants sont des contrats partagés : `application-<nom>`, `<nom>-cookie`, `<nom>-restate`, le label `vanyline.solidite.fr/application`, les noms de ports du Service Restate, et le `FIELD_MANAGER` `vanyline-controller`. Les noms de Service et de label sont lus par `app`, par la sandbox et par le DNS.
    - **Source:** SpecDD /controller/src/application.sdd, Must ; `workspaces/owners#CON-3`
- **CON-5:** Les images par défaut proviennent des paramètres du controller (`--app-image-repo`/`--app-image-tag`, `--restate-image`) ; `spec.image` et `spec.restate.image` les remplacent quand ils sont présents.
    - **Source:** SpecDD /controller/src/application.sdd, Must ; SpecDD /crds/src/lib.sdd, Must (`RestateSpec`)

- **Dépendances :** types `ApplicationSpec`, `ApplicationStorageDefaults`, `RestateSpec`, `ApplicationStatus` (SpecDD /crds/src/lib.sdd) ; `FIELD_MANAGER` (SpecDD /controller/src/owner.sdd) ; convention TLS et label partagés avec SpecDD /controller/src/sandbox.sdd ; verbes du client `VnlK8sClient` (SpecDD /lib/src/k8s.rs, cité par application.sdd).

## Exigences fonctionnelles

- **FR-1 - MUST:** Pour chaque CR Application, le reconciler applique dans son namespace un Deployment `application-<nom>` exécutant `app`, un Service et un Ingress du même nom, avec un port HTTP unique 8080 et une sonde `/health`.
    - **Source:** workspaces#FR-4 ; SpecDD /controller/src/application.sdd, Must
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** La CR et les manifestes construits ne portent aucune valeur sensible : les secrets OIDC, de base et de cookie sont référencés par `secretRef` (clé incluse), jamais copiés.
    - **Source:** SpecDD /controller/src/application.sdd, Purpose et Must not
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** Si `cookieSecretRef` est absent, le controller crée une seule fois le Secret `<nom>-cookie` avec une valeur aléatoire de 64 octets ; il ne le régénère, ne le patche ni ne le supprime ensuite.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@ensure_cookie_secret`) et Handles
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** Si `cookieSecretRef` est présent, le controller ne crée ni ne vérifie aucun Secret cookie ; la référence est utilisée telle quelle.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Handles
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** L'identité de `app` est un ServiceAccount `application-<nom>` lié à un Role namespacé qui n'accorde que `get`, `list`, `create`, `delete` sur `owners` et `get` sur `applications`, sans aucun droit sur `projects`, `sandboxes` ni `serviceaccounts/token`.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Scenario `build_application_role_shape` ; `workspaces/owners#FR-12` pour la délégation par namespace d'Owner
    - **Verified by:** `AC-4`
- **FR-6 - MUST:** Si `spec.restate` est présent, le reconciler déploie un StatefulSet `<nom>-restate` à un réplica, avec un volume de données, et un Service `<nom>-restate` exposant les ports nommés `ingress` (8080), `admin` (9070) et `worker` (5122). Si `spec.restate` est absent, aucune de ces ressources n'existe.
    - **Source:** workspaces#FR-4 ; SpecDD /controller/src/application.sdd, Must (« Moteur de workflow »)
    - **Verified by:** `AC-5`
- **FR-7 - MUST:** Le Service Restate n'est jamais exposé par un Ingress.
    - **Source:** SpecDD /controller/src/application.sdd, Must
    - **Verified by:** `AC-5`
- **FR-8 - MUST:** Quand `spec.restate` est présent, le Deployment d'`app` reçoit les URL internes admin et ingress de Restate ; le conteneur `app` et le Service d'application exposent le port `workflow` 9080, et `VNL_WORKFLOW_DEPLOYMENT_URL` pointe sur le Service `application-<nom>`.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`VNL_RESTATE_*`, endpoint de services du 2026-10-19)
    - **Verified by:** `AC-6`
- **FR-9 - MUST:** Le retrait de `spec.restate` supprime le StatefulSet, le Service et la NetworkPolicy Restate, et conserve le PVC de données grâce à une politique de rétention explicite `Retain`.
    - **Source:** SpecDD /controller/src/application.sdd, Must (décision du 2026-10-14) et Scenario `retrait_spec_restate_supprime_le_moteur`
    - **Verified by:** `AC-7`
- **FR-10 - MUST:** La NetworkPolicy de Restate n'admet vers Restate que les pods `app` du namespace (ports 8080 et 9070) et, pour les sandboxes des namespaces d'Owner, le seul port 8080 (callback d'awakeable).
    - **Source:** SpecDD /controller/src/application.sdd, Scenario `build_restate_netpol_shape` et Tasks
    - **Verified by:** `AC-8`
- **FR-11 - MUST:** L'Ingress de `app` porte toujours un bloc TLS dont le secret est `application-<nom>-cert` ; l'annotation d'issuer est dérivée de `tlsIssuerKind` (`Issuer` → `cert-manager.io/issuer`, toute autre valeur → `cert-manager.io/cluster-issuer`).
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@cert_manager_annotation_key`, `@build_application_ingress`)
    - **Verified by:** `AC-9`
- **FR-12 - MUST:** Chaque cycle applique dans l'ordre le Secret cookie (si nécessaire), le ServiceAccount, le Role et le RoleBinding, puis lit le Deployment existant pour calculer la phase, puis applique Deployment, Service et Ingress, puis réconcilie Restate, puis écrit le statut.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@reconcile`) ; Tasks (DÉCIDÉ 2026-10-14, bloc Restate après l'Ingress et avant le statut)
    - **Verified by:** `AC-10`
- **FR-13 - MUST:** Le statut porte `phase` (`Running`, `Provisioning` ou `Failed`) et une condition unique `Ready`, `True` (reason `DeploymentReady`) si et seulement si la phase est `Running`.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@compute_status`, `@deployment_phase`)
    - **Verified by:** `AC-10`
- **FR-14 - MUST:** Chaque cycle requeue après 300 s si la phase est `Running`, après 15 s sinon ; le controller ne surveille aucun enfant.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Returns
    - **Verified by:** `AC-10`
- **FR-15 - MUST:** Toute erreur de reconcile est journalisée et suivie d'un requeue de 30 s, sans distinction de nature ni backoff.
    - **Source:** SpecDD /controller/src/application.sdd, Must (`@error_policy`)
    - **Verified by:** `AC-11`
- **FR-16 - MUST:** Tous les enfants créés portent exactement une ownerReference de contrôle vers la CR, sans finalizer posé par le controller ; la suppression de la CR supprime les enfants par garbage collection.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Handles
    - **Verified by:** `AC-12`

## Sécurité

- Les secrets ne sont jamais des valeurs dans la CR : seules des références (`secretRef`, `cookieSecretRef`) y figurent (FR-2). Le cookie auto-généré est le seul secret dont le controller connaît la valeur ; elle ne sort que dans le Secret Kubernetes (SpecDD /controller/src/application.sdd, Must not).
- L'URL de redirection OIDC est dérivée de `host` à chaque construction et n'est lue d'aucun secret (SpecDD /controller/src/application.sdd, Must not).
- Le ServiceAccount `app` ne porte ni secret ni automount personnalisé ; son Role est restreint aux verbes réellement appelés (FR-5). Les droits sur les Owners viennent de leurs namespaces (`workspaces/owners`, DEC-3 : le ServiceAccount d'Owner est une identité sans droit).
- La NetworkPolicy n'ouvre vers Restate que les flux de FR-10 ; le Service Restate n'est jamais exposé par Ingress (FR-7).

## Conception

Structure dans le namespace de la CR Application :

```text
CR Application <nom>  (namespace d'application)
  |-- Secret <nom>-cookie              si cookieSecretRef absent (créé une fois)
  |-- ServiceAccount application-<nom> sans RBAC propre
  |-- Role / RoleBinding application-<nom>   owners + applications
  |-- Deployment application-<nom>     conteneur app :8080, secrets par secretRef
  |-- Service application-<nom>        http 8080 (+ workflow 9080 si restate)
  |-- Ingress application-<nom>        TLS application-<nom>-cert, backend 8080
  `-- si spec.restate
        |-- StatefulSet <nom>-restate  1 réplica, volume "data"
        |-- Service <nom>-restate      ingress 8080, admin 9070, worker 5122
        `-- NetworkPolicy              nom non contracté (FR-10)
```

Interactions : `app` appelle l'admin et l'ingress de Restate ; Restate appelle le endpoint de services d'`app` via le port `workflow` du Service d'application ; les sandboxes appellent l'ingress Restate pour les callbacks. Les détails d'environnement des conteneurs (dix variables sur le Deployment, douze avec `spec.restate`, six sur Restate) sont contractés dans SpecDD /controller/src/application.sdd et ne sont pas repris ici.

Le controller ne possède aucun watch sur les enfants : la dérive d'un Deployment (repli, dégradation) n'est rattrapée qu'au requeue périodique, 15 s tant que `Running` n'est pas atteint, 300 s ensuite (DEC-5).

## Décisions

- **DEC-1 - Les secrets sont des références ; le controller ne génère que le cookie**
    - **Context:** La CR est lue et écrite par des humains, GitOps et `app`, et ne doit pas contenir de valeurs sensibles.
    - **Decision:** Toute valeur sensible est référencée par `secretRef` ; la seule valeur créée par le controller est le cookie, créé une fois si la référence est absente.
    - **Consequences:** Un secret référencé absent ne fait pas échouer la réconciliation, il bloque le démarrage du pod (OI-8).
    - **Source:** SpecDD /controller/src/application.sdd, Purpose et Must not
- **DEC-2 - Pas de finalizer : la GC par ownerReference nettoie les enfants**
    - **Context:** Le controller ne doit pas bloquer la suppression de la CR.
    - **Decision:** Chaque enfant porte une ownerReference de contrôle ; le controller ne pose aucun finalizer.
    - **Consequences:** Une Application recréée sous le même nom reçoit un cookie neuf, ce qui invalide les sessions existantes.
    - **Source:** SpecDD /controller/src/application.sdd, Must et Handles
- **DEC-3 - Restate est une sous-ressource de l'Application, mono-nœud**
    - **Context:** Le moteur de workflow doit être déployé avec l'application, sans cluster Restate.
    - **Decision:** Un StatefulSet à un réplica, dans le namespace de l'Application, piloté par `spec.restate`.
    - **Consequences:** Pas de haute disponibilité du moteur (CON-3).
    - **Source:** workspaces#FR-4 ; SpecDD /controller/src/application.sdd, Must (« Moteur de workflow », 2026-10-04) et Tasks (DÉCIDÉ 2026-10-14)
- **DEC-4 - Le PVC de Restate survit au retrait de `spec.restate`**
    - **Context:** Le volume contient le journal des runs de workflow ; la suppression par défaut de Kubernetes le détruirait.
    - **Decision:** Politique `Retain` explicite sur le StatefulSet ; la purge est manuelle.
    - **Consequences:** Remettre `spec.restate` remonte un StatefulSet qui réutilise le PVC conservé (OI-6).
    - **Source:** SpecDD /controller/src/application.sdd, Tasks (DÉCIDÉ 2026-10-14) et Scenario `build_restate_statefulset_retention_pvc_retain`
- **DEC-5 - Le rythme est piloté par requeue, sans watch des enfants**
    - **Context:** Surveiller les enfants ajouterait des watchs que le controller n'utilise pas ailleurs.
    - **Decision:** Requeue de 15 s ou 300 s selon la phase ; la dérive est rattrapée au cycle suivant.
    - **Consequences:** Une dérive peut rester visible jusqu'à 300 s (OI-4).
    - **Source:** SpecDD /controller/src/application.sdd, Must (« Requeuer… », « Dépendre d'un enfant watché »)

## Réalisation SpecDD

- `SpecDD /controller/src/application.sdd` : FR-1 à FR-16, DEC-1 à DEC-5, AC-1 à AC-12 ; implémentation `controller/src/application.rs`.
- `SpecDD /crds/src/lib.sdd` : `ApplicationSpec`, `ApplicationStorageDefaults`, `RestateSpec`, `ApplicationStatus` ; implémentation `crds/src/lib.rs` (CON-5, FR-6, FR-9).
- `SpecDD /controller/src/owner.sdd` : `FIELD_MANAGER` (CON-4), importé et non dupliqué.
- `SpecDD /controller/src/sandbox.sdd` : consommateur de `@cert_manager_annotation_key`, `@tls_secret_name`, du label `vanyline.solidite.fr/application` et des règles réseau vers Restate (FR-10, CON-4). Ce document ne décrit pas ce côté.
- `controller/src/main.rs` : injection de l'image par défaut de `app` et de Restate (CON-5).

## Acceptation et vérification

- **AC-1:** Une CR `demo` dans `ns` produit `application-demo` (Deployment, Service, Ingress), avec port 8080 et sonde `/health` (Scenarios `build_application_deployment_shape`, `build_application_service_shape`, `build_application_ingress_shape` ; intégration `reconcile_cree_toutes_les_ressources_et_cookie_secret`).
- **AC-2:** Aucun manifeste construit ne contient une valeur de secret en littéral ; seules des `secretKeyRef` sont posées (Scenario `build_application_deployment_env`).
- **AC-3:** Le cookie est créé à l'absence de référence, jamais régénéré, et non créé si la référence est fournie (intégration `reconcile_cookie_secret_jamais_regenerer`, `reconcile_cookie_secret_ref_externe_aucune_generation`).
- **AC-4:** Le Role n'a que les deux règles prescrites et aucune règle sur `projects`, `sandboxes` ni `serviceaccounts/token` (Scenarios `build_application_role_shape`, `build_application_service_account_shape`).
- **AC-5:** Avec `spec.restate`, le StatefulSet (trois containerPorts), les trois ports nommés du Service et l'absence d'Ingress sont conformes (Scenarios `build_restate_statefulset_shape`, `build_restate_statefulset_env_exhaustive`, `build_restate_service_shape`).
- **AC-6:** Le port `workflow` 9080 et `VNL_WORKFLOW_DEPLOYMENT_URL` apparaissent avec `spec.restate` et disparaissent sans lui (Scenarios `workflow_port_et_url_poses_avec_restate`, `workflow_port_et_url_absents_sans_restate`).
- **AC-7:** Le retrait de `spec.restate` supprime le moteur sans toucher au PVC (Scenarios `build_restate_statefulset_retention_pvc_retain`, `retrait_spec_restate_supprime_le_moteur`).
- **AC-8:** La NetworkPolicy Restate n'ouvre que les flux prescrits (Scenario `build_restate_netpol_shape`).
- **AC-9:** L'Ingress porte toujours le TLS, et l'annotation d'issuer suit `tlsIssuerKind` (Scenarios `build_application_ingress_shape`, `build_application_ingress_annotations`).
- **AC-10:** Phase, condition `Ready` et requeue 15 s ou 300 s sont conformes ; le requeue 15 s du premier passage lit la phase avant le apply (Scenarios `compute_status_running`, `compute_status_provisioning`, `reconcile_requeue_quinze_secondes_premier_passage`, `reconcile_requeue_trois_cents_secondes_quand_running`).
- **AC-11:** Toute erreur produit un requeue de 30 s (Scenario `error_policy_requeue_trente_secondes`).
- **AC-12:** Chaque enfant porte une ownerReference de contrôle et aucun finalizer n'est posé ; la suppression de la CR supprime les enfants (Scenarios `owner_reference_controller_sur_tous_les_builders`, `suppression_application_gc_supprime_les_enfants`).

Vérification actuelle : tests purs du module `tests` de `controller/src/application.rs` pour les builders et le statut ; les Scenarios d'intégration sur cluster réel restent à exécuter (SpecDD /controller/src/application.sdd, Tasks).

## Open Issues

- **OI-1 - État du contrat RestateSpec et de l'implémentation:** `vanyline.sdd` (tâche « Couche workflow ») indique « RESTE … S6c : écrire le contrat RestateSpec » ; SpecDD /crds/src/lib.sdd marque ce contrat fait (2026-10-13) ; `workspaces#OI-4` le présente encore comme à écrire ; SpecDD /controller/src/application.sdd garde plusieurs items Restate ouverts. **TBD :** état réel, puis mise à jour de `vanyline.sdd` et de `workspaces#OI-4`.
- **OI-2 - Séquence de reconcile incomplète:** la liste « EXACTEMENT » à sept étapes de SpecDD /controller/src/application.sdd ne contient pas le bloc Restate, que la décision du 2026-10-14 place après l'Ingress et avant le statut (FR-12). **TBD :** amender la liste, puis clore l'item correspondant.
- **OI-3 - Phase `Failed` possiblement inatteignable:** `@deployment_phase` lit les reasons d'échec sur la condition `Available` ; un apiserver réel les poserait sur `Progressing` (SpecDD /controller/src/application.sdd, `[?]`). **TBD :** corriger le filtre ou assumer « Failed n'arrive jamais ».
- **OI-4 - Retard d'un cycle sur la phase:** le statut reflète le Deployment lu avant le apply (SpecDD /controller/src/application.sdd, `[?]`). **TBD :** accepté, ou déplacement après le apply.
- **OI-5 - Churn du statut:** `lastTransitionTime` est horodaté à chaque cycle, donc réécrit à chaque patch (SpecDD /controller/src/application.sdd, `[?]`). **TBD :** accepté, ou gel quand la phase ne change pas.
- **OI-6 - Remise de `spec.restate` avec un PVC conservé:** les sources ne disent pas ce qui se passe si la taille ou la classe de stockage change alors que le PVC `Retain` existe. **TBD :** à décider et à contracter.
- **OI-7 - Collisions d'annotations Ingress:** l'annotation cert-manager dérivée écrase une clé identique de `ingressAnnotations` ; un `tlsIssuerKind` autre que `Issuer` bascule silencieusement sur `cluster-issuer` (SpecDD /controller/src/application.sdd, `[?]` ; SpecDD /crds/src/lib.sdd, `[?]`). **TBD :** défendre l'espace `cert-manager.io/*` ou assumer la confiance dans l'auteur.
- **OI-8 - Référence cookie pendante:** un `cookieSecretRef` égal au nom du Secret auto-généré fait sauter sa création et laisse la référence sans cible (SpecDD /controller/src/application.sdd, `[?]`). **TBD :** refuser, créer, ou assumer.
- **OI-9 - Cohérence Role ↔ client:** aucun test ne garantit que les verbes du Role couvrent `VnlK8sClient` ; une évolution du client produit un 403 à l'exécution (SpecDD /controller/src/application.sdd, `[!]`). **TBD :** test de cohérence ou révision manuelle assumée.
- **OI-10 - Source de la constante de secret:** FR-2 était attribué à SpecDD /vanyline.sdd (contrainte « jamais de secret en clair dans une CR », citée par SpecDD /controller/src/application.sdd, Depends on) ; aucune occurrence de cette contrainte n'a été trouvée dans `vanyline.sdd` ni dans `AGENTS.md`. **TBD :** retrouver la source ou retirer la citation. FR-2 reste sourcé par SpecDD /controller/src/application.sdd.
- **OI-11 - État de la NetworkPolicy Restate:** SpecDD /controller/src/application.sdd (Tasks) laisse ouvert le volet NetworkPolicy du namespace d'application, alors que FR-10 s'appuie sur un Scenario de forme. **TBD :** état d'implémentation.

## Références

- `workspaces` : `FR-4`, `DEC-2`, `CON-1`, `OI-4`, section Périmètre et non-objectifs.
- `workspaces/owners` : `CON-3`, `FR-12`, `DEC-3`.
- `app`, section Périmètre et non-objectifs (moteur de workflow).
- SpecDD /controller/src/application.sdd, SpecDD /crds/src/lib.sdd, SpecDD /controller/src/owner.sdd, SpecDD /controller/src/sandbox.sdd.
- `AGENTS.md`, section Architecture, composant controller (Application).
- `vanyline.sdd`, tâche « Couche workflow (2026-10-04) ».
