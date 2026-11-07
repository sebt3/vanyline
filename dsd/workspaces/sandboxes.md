# Solution Design: Sandboxes

## Résumé

La Solution `workspaces/sandboxes` décrit ce que le reconciler de la CR `Sandbox` fait de son état désiré : le Job de checkout qui prépare le worktree, le Pod de travail, son Service, l'Ingress publique conditionnelle, la suspension et la reprise, la modification de la spec, et la suppression par finalizer. Elle est réalisée par `controller/src/sandbox.sdd` (reconciler et builders), et pour les types et les noms partagés par `crds/src/lib.sdd`. Le contenu du pod (image, serveur, toolchains) relève du domaine `sandbox` et n'est pas redécrit ici. Les NetworkPolicies d'entrée et de sortie sont traitées par la Solution `workspaces/network`. Statut : ébauche de reverse-engineering, écrite depuis les specs citées. Aucune exigence n'est nouvelle : ce qui n'est pas contracté par une spec est une Open Issue.

## Problème et objectif

Une Sandbox doit devenir, sans intervention, un pod de travail prêt sur une branche, reprenable après suspension, joignable par l'éditeur et par l'agent, et exposable publiquement quand son Owner référence une Application. La Solution contribue à `workspaces#BO-1` pour la partie pod et à `workspaces#BO-2` pour la reconnaissance des CR créées hors interface (la CR reste la source de vérité, `workspaces#BR-2`).

## Périmètre et non-objectifs

Couvert :

- le Job de checkout (`sandbox-<n>-checkout`), le Pod `sandbox-<n>`, son Service et l'Ingress `sandbox-<n>` conditionnelle ;
- la suspension et la reprise (`spec.suspended`), et la modification de la spec et ses effets sur les enfants existants ;
- la suppression par finalizer et le Job de retrait du worktree, décrits parce qu'ils sont dans la même spec et conditionnent la fin de vie du pod.

Hors périmètre, par décision :

- le contenu et le comportement du pod (image, serveur MCP, toolchains, auth côté serveur) : domaine `sandbox` (`sandbox#BR-1`, `sandbox#BR-5`) ;
- les NetworkPolicies d'entrée et de sortie (y compris le port d'exploitation ouvert au cluster) : Solution `workspaces/network` ;
- l'Owner, le Project et l'Application qui fournissent les statuts lus : `workspaces/owners`, `workspaces/projects`, `workspaces/application` ;
- la création des lignes d'identité en base : `workspaces/identity`.

## Contexte applicable du domaine

- `workspaces#BR-1` : un Owner = un namespace ; la Sandbox et ses enfants y vivent.
- `workspaces#BR-2` : la CR est la seule source de vérité de `spec` et `status`.
- `workspaces#BR-3` : nom `<projet>-<nom>`, immuable après création (la limite de 63 caractères est reprise en Open Issue).
- `workspaces#BR-5` : suspension manuelle ; le pod est supprimé, worktree, Service et politiques réseau sont conservés.
- `workspaces#BR-7` : aucun champ de CRD n'est interpolé dans un shell ; la maintenance git passe par `vanyline-maint` en argv.
- `workspaces#FR-2` et `workspaces#FR-3` : création et modification d'une Sandbox ; `FR-3` exige un signal de redémarrage (`DEC-10`, `OI-1`).
- `sandbox#BR-1`, `sandbox#BR-2`, `sandbox#BR-3` : accès tout ou rien par le jeton du ServiceAccount de l'Owner, SA sans droit RBAC, navigateur sans jeton.

## Acteurs

- **Utilisateur :** crée, suspend, reprend et modifie la Sandbox par l'interface, la CLI ou GitOps (`workspaces#BO-2`) ; l'**opérateur** obtient le jeton de l'Owner avec son kubeconfig (`sandbox#BR-1`).
- **Controller :** réconcilie la CR Sandbox et ses enfants (`controller/src/sandbox.sdd`, Purpose).
- **`app` :** pilote la CR avec les droits de l'utilisateur et lit `status.phase` pour le push `ws/sandbox-state` (`controller/src/sandbox.sdd`, Purpose).
- **Développeur et agent de code :** consomment le pod par l'éditeur, le MCP et le Service (`AGENTS.md`, section « Clients de la sandbox »).

## État actuel et état cible

- **Actuel (Observed in: `controller/src/sandbox.rs`, `controller/src/sandbox.sdd`) :** les builders purs sont testés unitairement (environ 85 tests selon `controller/src/sandbox.sdd`, Tasks). La boucle de reconcile n'a aucun test cluster : le crate n'a pas de harnais d'intégration.
- **Cible :** la section Must de `controller/src/sandbox.sdd`, qui pose notamment un Owner = un namespace et l'authentification par le seul jeton du SA de l'Owner.
- **Transitoire :** la tâche `[ ] ALIGNEMENT « UN OWNER = UN NAMESPACE »` et la tâche `[ ] RETRAIT DE existing_pvc` restent ouvertes dans la spec (voir OI-15).

## Hypothèses, contraintes et dépendances

- **CON-1:** Les toolchains et les LSP sont montés en volumes `image:`, qui exigent Kubernetes 1.31 ou plus ; le contrôleur suppose cette fonctionnalité à l'exécution.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, « Supposer à l'exécution ») ; `sandbox#CON-1`
- **CON-2:** cert-manager, une IngressClass et un DNS wildcard `*.sandboxes.{host}` sont fournis hors du dépôt.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, « Supposer à l'exécution » ; Exécution hors repo)
- **CON-3:** Le JWKS de l'apiserver est lisible par les pods (`system:service-account-issuer-discovery`), car la sandbox s'en sert pour vérifier les jetons entrants.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, « Supposer à l'exécution ») ; SpecDD /sandbox/src/auth.sdd
- **CON-4:** L'image sandbox écoute sur `MCP_PORT` (3000) et `OPS_PORT` (9090) et n'est configurée que par `VNL_OWNER_SUBJECT` et `VNL_TOKEN_ISSUER`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, « Supposer à l'exécution ») ; SpecDD /crds/src/lib.sdd (constantes `MCP_PORT`, `OPS_PORT`) ; SpecDD /sandbox/src/config.sdd
- **CON-5:** Le contrôleur ne valide ni le nom, ni la branche, ni les images, ni le format des CIDR : cette validité est reportée à l'apiserver.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must not, « Valider ici ») ; SpecDD /crds/src/lib.sdd (Must not)

Dépendances :
- `workspaces/owners` : statut de l'Owner `pvcName` et `serviceAccount` (`workspaces/owners#FR-8`) ; sans eux, aucune Sandbox ne progresse.
- `workspaces/projects` : statut du Project `cloned`, `languages` et `default_branch`, lus par la Sandbox (SpecDD /controller/src/sandbox.sdd, Gate d'@apply et Handles).
- `workspaces/application` : `ingressClassName`, `host`, `tlsIssuerName`, `tlsIssuerKind`, `ingressController` et `restate` de l'Application référencée.
- `workspaces/network` : NetworkPolicies, ports d'exploitation.

## Exigences fonctionnelles

- **FR-1 - MUST:** La création d'une Sandbox attend que son Project soit cloné (`status.cloned: true`) et que son Owner publie `status.pvcName` et `status.serviceAccount`. Tant que ce n'est pas le cas, aucun Job ni Pod n'est créé et l'erreur `VNL-CTL-005` ou `VNL-CTL-002` est rendue.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Gate d'@apply ; Handles) ; `workspaces#FR-2`
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** Le worktree de la branche est créé par le Job `sandbox-<n>-checkout` (argv `vanyline-maint checkout`). Le Pod n'est créé qu'après un Job réussi (`succeeded > 0`). Tant que le Job existe, il n'est ni recréé ni modifié.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Gérer le Job checkout dans @apply ; Build des Jobs git)
    - **Verified by:** `AC-1`
- **FR-3 - MUST:** Le Pod `sandbox-<n>` vit dans le namespace de la CR. Il comporte un seul conteneur `sandbox` (image `spec.image`, sinon l'image par défaut du contrôleur), un port nommé `mcp` (3000) et un port nommé `ops` (9090). Ses sondes readiness et liveness sont HTTP GET `/health` sur le port `ops`, avec `initialDelaySeconds: 5` et `periodSeconds: 10`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Construire le Pod)
    - **Verified by:** `AC-2`
- **FR-4 - MUST:** Le Pod s'exécute sous le ServiceAccount de l'Owner `owner-<nom>`, avec `VNL_OWNER_SUBJECT` = `system:serviceaccount:<namespace>:owner-<nom>` et `VNL_TOKEN_ISSUER`. Aucun argument d'authentification contournable (`--no-auth`, jeton statique) n'est posé, que l'Owner référence une Application ou non.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must ; Must not ; correction de la faille d'authentification, FAIT 2026-10-18) ; `sandbox#BR-1`
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** Le Pod monte, dans l'ordre : le PVC home de l'Owner entier sur `/home/vanyline` ; le PVC workspace du Project sur `/home/vanyline/workspace`, sous-chemin propre au worktree (voir OI-16) ; le même PVC workspace une seconde fois sur `/workspace/repo.git`, en écriture ; les caches du Project sous `/project-cache/<nom>` ; les volumes image des toolchains et de leurs LSP sous `/toolchains/<nom>` et `/toolchains/<nom>-lsp`, en lecture seule.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Construire le Pod ; Scenarios `pod_second_montage_repo_bare_sur_chemin_gitdir`, `pod_montage_repo_bare_non_lecture_seule`)
    - **Verified by:** `AC-2`
- **FR-6 - MUST:** L'environnement du Pod suit un ordre contractuel : `VNL_SANDBOX_ROOT`, puis `VNL_HEALTH_BASE_REF` si la branche par défaut du Project est non vide, puis `VNL_RESTATE_INGRESS_URL` seulement si l'Application porte `restate`, puis les variables de cache, puis `PATH` et `LD_LIBRARY_PATH` agrégés des toolchains, puis `VNL_LSP_TOOLCHAINS`. Le détail des variables de toolchain relève de `sandbox#BR-5`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Poser l'env du Pod) ; `sandbox#BR-5`
    - **Verified by:** `AC-2`
- **FR-7 - MUST:** Le Service `sandbox-<n>` n'expose que le port `mcp` (3000) et ne sélectionne que le label `vanyline.solidite.fr/sandbox=<n>`. Le port `ops` n'est jamais publié, ni par le Service, ni par l'Ingress.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Construire le Service ; Must, ne jamais publier `OPS_PORT`)
    - **Verified by:** `AC-5`
- **FR-8 - MUST:** Un Ingress `sandbox-<n>` n'existe que si `owner.spec.application_ref` résout une Application. Son hôte est `<n>.sandboxes.<host de l'Application>`, sa classe et ses annotations viennent de l'Application, l'annotation cert-manager est dérivée de `tlsIssuerKind` (`cluster-issuer` par défaut), le TLS utilise le secret `sandbox-<n>-cert`, et le backend est le Service `sandbox-<n>` sur le port numérique 3000. Quand la référence disparaît, l'Ingress est supprimé explicitement.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Construire l'Ingress ; Must, suppression explicite)
    - **Verified by:** `AC-6`
- **FR-9 - MUST:** `spec.suspended: true` supprime le Pod s'il existe et pose la phase `Suspended`. Le Job de checkout, le Service, les NetworkPolicies et l'Ingress sont conservés. Le Job de checkout n'est jamais conditionné par `suspended`.
    - **Source:** `workspaces#BR-5` ; SpecDD /controller/src/sandbox.sdd (Must, Gérer la suspension ; Must not)
    - **Verified by:** `AC-4`
- **FR-10 - MUST:** `spec.suspended: false` recrée le Pod à partir de la spec courante (Pod absent ⇒ création du manifeste). La phase repasse par `Provisioning` puis `Running`. Le worktree est repris sans nouveau checkout tant que le Job de checkout existe (voir OI-6).
    - **Source:** SpecDD /controller/src/sandbox.sdd (Calculer la phase sinon) ; `workspaces#AC-4`
    - **Verified by:** `AC-4`
- **FR-11 - MUST:** Le contrôleur ne patche ni ne recrée jamais un Pod existant : une modification de `spec.image`, `spec.resources`, `spec.toolchains` ou `spec.branch` n'est pas répercutée sur un Pod vivant. Les Services et les Ingress sont en revanche ré-appliqués à chaque reconcile, à partir de leur manifeste reconstruit.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, Pod immutable en v1 ; Must not, Modifier un Pod existant ; Must, Maintenir les enfants)
    - **Verified by:** `AC-7`
- **FR-12 - MUST:** `status.phase` vaut `Provisioning` tant que le checkout n'est pas fait ou que le Pod n'est pas `Running` ; `Running` si la phase K8s du Pod est `Running` ; `Failed` si elle est `Failed` ; `Suspended` si la spec le demande. Un Pod `Failed` reste en place et n'est pas redémarré automatiquement. `status.service` vaut toujours `sandbox-<n>`. La condition unique `Ready` vaut `True/PodRunning`, `False/Suspended` ou `False/NotRunning`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Build compute_status ; Handles, Pod en phase `Failed`)
    - **Verified by:** `AC-8`
- **FR-13 - MUST:** Le requeue est de 300 s après un passage en `Running` ou `Suspended`, et de 15 s sinon. Une erreur est relancée à 30 s.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Requeue ; Build @error_policy)
    - **Verified by:** `AC-9`
- **FR-14 - MUST:** À la suppression de la CR, le finalizer `vanyline.solidite.fr/sandbox-cleanup` crée le Job `sandbox-<n>-remove` (argv `vanyline-maint remove`) et ne le lève qu'après `succeeded > 0`. Si le Project ou l'Owner est absent, ou si l'Owner n'a plus de `pvcName`, le finalizer est levé sans Job : le worktree est abandonné (best-effort v1).
    - **Source:** SpecDD /controller/src/sandbox.sdd (Gérer @cleanup ; Handles, Cleanup avec Project ou Owner disparus) ; `sandbox#BR-8` ; `workspaces#BR-7`
    - **Verified by:** `AC-10`

## Sécurité

- L'accès au pod repose sur le seul jeton du ServiceAccount de l'Owner (FR-4) ; l'Ingress publique n'ajoute ni ne retire aucune exigence d'authentification : la sandbox n'accepte que ce jeton, que l'Ingress soit public ou non (SpecDD /controller/src/sandbox.sdd, Must).
- L'hôte de l'Ingress n'est pas un secret (il apparaît dans les journaux de Certificate Transparency) : la seule barrière est donc le jeton (SpecDD /controller/src/sandbox.sdd, faille d'authentification).
- Le ServiceAccount de l'Owner n'a aucun droit RBAC ; son jeton est monté par défaut dans le pod, car la sandbox en a besoin pour lire le JWKS (`workspaces/owners#FR-11`, `sandbox#BR-2`).
- Le port `ops` (`/health`, `/metrics`, sans authentification) n'est jamais publié par le Service ni par l'Ingress. Son ouverture au cluster est dans la NetworkPolicy, traitée par `workspaces/network`.
- Le pod s'exécute en non-root, uid et gid 1000, `fsGroup` 1000 (SpecDD /controller/src/sandbox.sdd, Must, `security_context`).
- Les effets de l'agent sont bornés au niveau du pod (`sandbox#BR-9`) ; réseau sortant, quotas et isolation noyau sont en Open Issue (`sandbox#OI-1`, `sandbox#OI-2`).

## Conception

Structure des enfants d'une CR Sandbox, dans son namespace d'Owner :

```text
CR Sandbox <n>  (finalizer vanyline.solidite.fr/sandbox-cleanup)
  |-- Job    sandbox-<n>-checkout   vanyline-maint checkout   ownerRef Sandbox
  |-- Pod    sandbox-<n>            SA owner-<owner>          ownerRef Sandbox
  |-- Service sandbox-<n>           port mcp 3000             ownerRef Sandbox
  |-- Ingress sandbox-<n>           si application_ref résout ownerRef Sandbox
  |-- NetworkPolicy ingress et egress      -> workspaces/network
  `-- Job    sandbox-<n>-remove     à la suppression          ownerRef Sandbox
```

Ordre d'un reconcile : gate (Project et Owner) ; finalizer ; Job de checkout ; phase et Pod ; enfants (Application, Service, NetworkPolicies, Ingress) ; patch de status. Les suppressions explicites des enfants conditionnels (egress, Ingress) sont faites par `get_opt` puis `delete`, car le GC par ownerReference ne les retire pas quand la condition tombe.

Machine d'états de `status.phase` :

```text
Provisioning --(checkout ok, Pod créé)--> Provisioning --(Pod Running)--> Running
Running --(suspended: true)--> Suspended --(suspended: false, Pod recréé)--> Provisioning
Running ou Provisioning --(Pod Failed)--> Failed   (pas de redémarrage automatique)
```

Interfaces produites et consommées :

- le nom du Service `sandbox-<n>` est donné par `@service_name` de `vanyline-crds` (SpecDD /crds/src/lib.sdd) et consommé par `vanyline-lib` (URL MCP) et `app` ;
- `status.phase` est lu par le push `ws/sandbox-state` de `app` (SpecDD /controller/src/sandbox.sdd, Purpose) ;
- l'hôte `{sandbox}.sandboxes.{host}` est le point d'entrée de l'éditeur web (`AGENTS.md`, « Clients de la sandbox ») ;

Propagation des changements : le controller n'observe que les CR Sandbox. Les changements venant de l'Owner ou du Project arrivent par l'annotation `vanyline.solidite.fr/egress-bump` posée par `owners` et `projects`, et par les requeues (SpecDD /controller/src/sandbox.sdd, `@build_controller`). Le délai de propagation est donc borné par les requeues, non par un watch (`workspaces#OI-2`).

## Décisions

- **DEC-1 - Pod immuable en v1, reconstruit à la reprise**
    - **Context:** Le Pod porte l'image, les ressources et les toolchains au démarrage ; le contrôleur ne sait pas le remplacer sans perdre l'état de l'agent.
    - **Decision:** Le contrôleur ne patche ni ne recrée jamais un Pod existant. La prise en compte d'une nouvelle spec passe par une suspension et une reprise, ou par la suppression manuelle du Pod.
    - **Alternatives:** recréation sur empreinte de spec, « feature à spécifier » (SpecDD /controller/src/sandbox.sdd, `[?]` Pod jamais muté vs doc).
    - **Consequences:** FR-11 ; OI-1 et OI-2.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must ; Must not)
- **DEC-2 - Suspension manuelle, sans arrêt sur inactivité**
    - **Context:** Le pod consomme des ressources même inactif ; le worktree doit survivre pour une reprise rapide.
    - **Decision:** La suspension est pilotée par `spec.suspended` et ne supprime que le Pod.
    - **Alternatives:** arrêt automatique sur inactivité, écarté par `workspaces#BR-5`.
    - **Consequences:** FR-9 et FR-10.
    - **Source:** `workspaces#BR-5` ; SpecDD /controller/src/sandbox.sdd (Must, suspension)
- **DEC-3 - Checkout indépendant de la suspension**
    - **Context:** Une Sandbox créée suspendue doit pouvoir être reprise vite.
    - **Decision:** Le Job de checkout est créé même si la Sandbox est suspendue.
    - **Alternatives:** conditionner le checkout à `suspended`, interdit par la spec.
    - **Consequences:** une Sandbox créée suspendue fait un checkout sans Pod (Scenario `reconcile_suspension_cree_quand_meme_job_checkout_cluster`).
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must not, conditionner le checkout à `spec.suspended`)
- **DEC-4 - L'exposition publique passe par l'Ingress, jamais par un contournement de l'authentification**
    - **Context:** Un Ingress public expose un hôte non secret ; un contrôle d'authentification conditionnel à l'Application ouvrirait la sandbox.
    - **Decision:** L'Ingress n'existe que si l'Application est résolue. L'authentification du Pod ne dépend jamais de l'Ingress ni de l'Application.
    - **Alternatives:** OIDC réel quand une Application est résolue, décision du 2026-09-27 déclarée caduque par la spec.
    - **Consequences:** FR-4 et FR-8 ; la sécurité repose sur le jeton du SA.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, « Ne JAMAIS poser `--no-auth` » ; tâche de correction, FAIT 2026-10-18) ; `sandbox#DEC-1`
- **DEC-5 - Suppression explicite des enfants conditionnels**
    - **Context:** Un enfant rendu `None` par le builder n'a pas de ownerReference à suivre quand sa condition tombe.
    - **Decision:** Egress NetworkPolicy et Ingress sont supprimés explicitement quand leur condition disparaît.
    - **Alternatives:** non consignées dans la spec.
    - **Consequences:** FR-8 ; sans ce cas, un Ingress survivrait au retrait de `application_ref`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, suppression explicite ; Scenario `reconcile_ingress_supprime_quand_application_ref_retire_cluster`)
- **DEC-6 - Enfants de la Sandbox liés par ownerReference**
    - **Context:** Pod, Service, Ingress et Jobs sont dans le même namespace que la Sandbox ; un ownerReference inter-namespaces est interdit (`workspaces/owners`, CON-1).
    - **Decision:** Chaque enfant porte une ownerReference unique vers la Sandbox, et la GC en cascade le supprime.
    - **Alternatives:** ownerReference vers le Project, écartée par la spec.
    - **Consequences:** la suppression de la Sandbox supprime ses enfants, à l'exception du worktree, traité par FR-14.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Returns ; Must, ownerReference du Pod)
- **DEC-7 - Port d'exploitation hors Service et Ingress**
    - **Context:** Les sondes kubelet doivent atteindre `/health` ; ce port n'a pas d'authentification.
    - **Decision:** Le port `ops` n'est jamais publié par le Service ni par l'Ingress ; son ouverture est une règle dédiée de la NetworkPolicy.
    - **Alternatives:** aucune règle `from` par défaut, restriction par `VNL_OPS_ALLOWED_NAMESPACES` recommandée en déploiement.
    - **Consequences:** la règle réseau relève de `workspaces/network`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must ; Tasks, décision du 2026-10-18, recommandation validée)
- **DEC-8 - Application serveur pour les enfants, merge patch pour le status**
    - **Context:** Le contrôleur ne doit jamais écrire la spec et ne doit pas prendre la propriété des champs de la CR.
    - **Decision:** Les enfants sont appliqués en server-side apply avec `force` et le field manager `vanyline-controller` ; le status est écrit en `Patch::Merge` sur le sous-ressource `status`.
    - **Alternatives:** SSA sur le status, interdit par la spec.
    - **Consequences:** le status n'est jamais en conflit de champs avec un autre client.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, PatchParams ; Must not, écrire la spec)
- **DEC-9 - Propagation par annotation de bump et requeues**
    - **Context:** Un watch sur les dépendances de chaque Sandbox coûterait cher.
    - **Decision:** Le contrôleur ne surveille que les CR Sandbox ; les changements de l'Owner et du Project passent par l'annotation de bump et les requeues.
    - **Alternatives:** watch inter-CRD, écarté par la spec.
    - **Consequences:** latence de propagation bornée par les requeues (`workspaces#OI-2`).
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, `@build_controller`)

- **DEC-10 - Le redémarrage requis est une condition `RestartPending`, détectée par empreinte**
    - **Context:** `workspaces#FR-3` autorise la modification d'une sandbox en cours sans redémarrage automatique et exige que l'état de la sandbox signale qu'un redémarrage est requis ; l'IDE et les relais sont conditionnés par `phase == Running`.
    - **Decision:** le signal est une condition `RestartPending` du status, jetée dès que le Pod a redémarré avec la spec courante ; la phase ne change pas. La dérive se détecte par l'empreinte de la spec utile portée par le Pod, comparée à la spec courante.
    - **Alternatives:** une nouvelle valeur de `phase` (écartée : la sandbox deviendrait inutilisable pour l'IDE et les relais en attendant le redémarrage) ; détection par `metadata.generation` (écartée : signalerait aussi des changements qui n'exigent pas de redémarrage, comme labels et annotations de bump).
    - **Source:** décision du développeur, 2026-11-07 ; `vanyline.sdd`, tâche « Modification d'une sandbox » (option C)

## Réalisation SpecDD

- `SpecDD /controller/src/sandbox.sdd` : reconciler, builders du Pod, des Jobs, du Service et de l'Ingress, phase et status (FR-1 à FR-14, à l'exception des NetworkPolicies).
- `SpecDD /crds/src/lib.sdd` : types `SandboxSpec` et `SandboxStatus`, `@service_name`, `MCP_PORT`, `OPS_PORT`, schéma de la CRD `sandboxes.vanyline.solidite.fr`.
- `SpecDD /sandbox/src/auth.sdd` et `SpecDD /sandbox/src/config.sdd` : côté pod, vérification du jeton et configuration par variables (FR-4, CON-3, CON-4).
- Les NetworkPolicies et le port d'exploitation : `workspaces/network`.

## Acceptation et vérification

Les Scenarios cités sont ceux de `controller/src/sandbox.sdd`. Les Scenarios `_cluster` ne sont pas encore écrits ; le statut de chacun est noté.

- **AC-1:** Un Project non cloné ne déclenche aucune création (VNL-CTL-005) ; un Owner sans `pvcName` déclenche VNL-CTL-002 ; sinon le Job de checkout est créé, puis le Pod après succès. Scenarios : `reconcile_attend_clone_du_project_cluster`, `reconcile_attend_pvc_et_sa_de_l_owner_cluster`, `reconcile_cree_job_checkout_puis_pod_cluster`, `reconcile_checkout_job_existant_non_recree_cluster`. Statut : cluster à écrire (OI-15).
- **AC-2:** Le Pod porte le nom, les ports, les sondes, le montage et l'env attendus. Scenarios unitaires : `pod_nom_sandbox_prefixe_et_namespace`, `pod_non_root_uid_1000`, `pod_ports_mcp_et_ops_et_sondes_sur_ops`, `pod_montage_worktree_sous_chemin_worktrees`, `pod_second_montage_repo_bare_sur_chemin_gitdir`, `pod_env_racine_sandbox`, `pod_env_restate_ingress_url_presente_apres_base_ref`.
- **AC-3:** Le Pod n'a aucun argument d'authentification contournable et son identité est celle du SA de l'Owner. Scenarios : `pod_sans_aucun_contournement_d_auth`, `pod_identite_de_l_owner_dans_l_env`. Un controller sans issuer ne réconcilie pas : `sandbox_non_reconciliable_sans_issuer` (voir OI-5). Une CR sans namespace est refusée : `sandbox_sans_namespace_est_une_erreur` (voir OI-4).
- **AC-4:** Suspendre supprime le Pod et garde le reste ; reprendre recrée le Pod sans nouveau checkout tant que le Job existe. Scenarios : `reconcile_suspension_supprime_pod_et_phase_suspended_cluster`, `reconcile_suspension_cree_quand_meme_job_checkout_cluster`. Statut : cluster à écrire ; la condition « sans nouveau checkout » dépend de OI-6.
- **AC-5:** Le Service ne sélectionne que le label de la Sandbox et ne publie que `mcp`. Scenarios : `service_selecteur_unique_label_et_port_mcp`, `port_ops_jamais_publie`.
- **AC-6:** L'Ingress n'existe que si l'Application est résolue, avec son hôte, son TLS et son backend. Scenarios : `ingress_hotete_backend_tls_certmanager`, `ingress_issuer_namespaced_quand_kind_issuer`, `ingress_annotations_application_reprises_derivee_gagne`, `reconcile_ingress_cree_quand_application_ref_pose_cluster`, `reconcile_ingress_supprime_quand_application_ref_retire_cluster`.
- **AC-7:** Une modification de spec ne touche pas un Pod vivant. Scenarios : `reconcile_job_pod_existant_jamais_mute_cluster`, `reconcile_changement_branche_ne_redecheckoute_pas_cluster`. Statut : contrat « figé après arbitrage » dans la spec ; la vérification est ouverte (OI-2, OI-3). Le signal de redémarrage exigé par `workspaces#AC-7` n'a aucun Scenario (OI-1).
- **AC-8:** Les phases et conditions de `status` suivent FR-12. Scenarios : `status_running_ready_true_pod_running`, `status_provisioning_ready_false_not_running`, `status_suspended_motif_distinct_suspended`, `status_failed_ready_false_not_running`, `status_service_toujours_pose_et_generation_suivie`, `reconcile_pod_failed_non_recree_cluster`.
- **AC-9:** Les requeues suivent FR-13. Scenarios : `error_policy_project_not_ready_requeue_quinze_secondes`, `error_policy_erreur_courante_requeue_trente_secondes`, `reconcile_phase_pod_running_requeue_long_cluster`. Le Scenario `error_policy_worktree_pending_requeue_dix_secondes` n'est pas atteignable tel quel (OI-10).
- **AC-10:** La suppression retire le worktree par Job avant de lever le finalizer. Scenarios : `cleanup_supprime_worktree_via_job_puis_leve_finalizer_cluster`, `cleanup_project_disparu_leve_finalizer_cluster`, `cleanup_job_remove_exist_non_succeeded_attente_cluster` (voir OI-11).

## Open Issues

- **OI-1 - Détection du redémarrage requis à spécifier:** `DEC-10` fixe la forme du signal, mais `controller/src/sandbox.sdd` ne spécifie ni la détection de dérive (empreinte de la spec utile portée par le Pod, comparée à la spec courante), ni le checkout de la nouvelle branche au redémarrage. Lié à `OI-2` (Pod immuable ou recréé). **TBD :** à écrire dans `controller/src/sandbox.sdd`.
- **OI-2 - Pod immuable ou recréé ?** Le Must de `controller/src/sandbox.sdd` dit « v1 immutable » ; `docs/architecture.md`, cité par le `[?]` de cette spec, dit « recréation du pod ». **TBD :** immunité totale (compléter la doc) ou recréation sur empreinte de spec.
- **OI-3 - Changement de `spec.branch` après checkout:** le Job est nommé sans empreinte de branche, donc pas de nouveau checkout ; le worktree reste sur la branche de départ. `workspaces#FR-3` ne tranche pas. **TBD :** voulu (une Sandbox = une branche de naissance) ou à répercuter.
- **OI-4 - Namespace par défaut, contradiction interne:** le reconcile et la section Accepts de `controller/src/sandbox.sdd` prévoient un repli sur `"default"`, alors que le Scenario `sandbox_sans_namespace_est_une_erreur` interdit tout repli et rend `VNL-CTL-012`. Le Scenario est plus récent, mais la récence ne tranche pas un conflit entre specs. Même question pour `owner.namespace` (`workspaces/owners`, OI-7). **TBD :** décision du développeur.
- **OI-5 - Issuer requis ou découvert ?** Le Must et le Scenario `sandbox_non_reconciliable_sans_issuer` exigent `--sa-issuer` ; la tâche `[ ]` de la même spec prévoit une découverte par défaut à partir de `/.well-known/openid-configuration`, le flag n'étant qu'une surcharge (décision du 2026-10-04). Les deux ne sont pas compatibles. **TBD :** lequel fait foi.
- **OI-6 - Checkout relancé après TTL:** le Job de checkout expire une heure après sa fin (`ttl_seconds_after_finished: 3600`) ; la reconcile suivante le recrée et relance `vanyline-maint checkout`, dont l'idempotence est supposée. Cela contredit `workspaces#AC-4` (« sans nouveau checkout ») pour une reprise après plus d'une heure. **TBD :** idempotence confirmée, ou preuve de succès durable hors du Job.
- **OI-7 - `application_ref` pendant:** si l'Application référencée est absente, `@fetch_application` échoue en VNL-CTL-001 et l'apply s'interrompt avant Service, Ingress et status (Scenario `reconcile_application_ref_pendant_bloque_statut_cluster`). La spec propose aussi de traiter ce cas comme `None` (Ingress supprimé). Le double appel à l'Application est signalé en `[?]`. **TBD :** dégradation voulue ou traitement équivalent à `None`.
- **OI-8 - Application sans `ingressController`:** l'Ingress est créé, mais la NetworkPolicy n'ouvre pas le pair contrôleur. Le domaine routable reste injoignable. **TBD :** ne créer l'Ingress que si `ingressController` est posé, ou assumer le cas (côté réseau : `workspaces/network`).
- **OI-9 - Vocabulaire de phase:** `crds/src/lib.sdd` documente `Provisioning | Running | Failed`, alors que ce contrôleur émet aussi `Suspended` ; `Failed` n'est jamais émis hors d'un Pod `Failed`. La valeur attendue par le push `ws/sandbox-state` de `app` n'est pas vérifiée. **TBD.**
- **OI-10 - Branches de requeue inatteignables:** `reconcile` enveloppe toute erreur du `finalizer()` dans `ControllerError::Finalizer`, donc les requeues de 10 s et 15 s de `error_policy` ne sont pas atteints (Tasks, `[!]`). **TBD :** propager l'erreur utilisateur, ou assumer 30 s partout.
- **OI-11 - 409 sur le Job de retrait:** si le Job `sandbox-<n>-remove` existe sans `succeeded`, le code tente un `create` et reçoit un 409 au lieu d'attendre (Tasks, `[!]`). **TBD.**
- **OI-12 - Churn du status:** `lastTransitionTime` est recalculé à chaque reconcile (toutes les 300 s), ce qui réécrit le status en permanence (Tasks `[?]`, `crds/src/lib.sdd` `[?]`). **TBD :** accepté, ou horodatage conservé si seul le timestamp change.
- **OI-13 - Toolchains homonymes:** deux toolchains de même nom produisent deux volumes `toolchain-<nom>`, rejetés par l'apiserver, avec un warn répété (Scenario `pod_toolchain_nom_duplique_volumes_en_double`). **TBD :** validation amont ou rejet assumé.
- **OI-14 - Variables dupliquées:** une toolchain qui pose `CARGO_HOME`, `PIP_CACHE_DIR` ou `PATH` entre en collision avec les variables de cache sans déduplication, et sans test. **TBD :** confirmer la règle « dernier gagne » ou dédupliquer.
- **OI-15 - Vérification de la boucle reconcile:** aucun test cluster n'existe, le harnais d'intégration est une tâche ouverte, et la tâche `ALIGNEMENT « UN OWNER = UN NAMESPACE »` reste `[ ]`. Les AC-1, AC-4 et AC-10 n'ont pas de vérification exécutable. **TBD :** calendrier et périmètre du harnais.
- **OI-16 - Sous-chemin du worktree:** le Must dit « subPath worktree », le Scenario `pod_montage_worktree_sous_chemin_worktrees` dit `worktrees/<nom>`. **TBD :** confirmer le second, à l'écrire dans le Must.
- **OI-17 - Limite de 63 caractères non localisée:** `workspaces#BR-3` et `workspaces#AC-3` exigent un refus 422 ou 409 pour un nom trop long. Aucune spec lue ici ne porte cette validation ; `crds/src/lib.sdd` ne valide pas les noms, et `controller/src/sandbox.sdd` renvoie la validité à l'apiserver. Le Service et le Pod dérivés, qui sont des labels DNS, sont les premiers concernés. **TBD :** localiser la règle, ou l'ajouter ici.
- **OI-18 - Suspension et trafic entrant:** l'Ingress et le Service sont conservés quand le Pod est supprimé, sans que la spec dise quelle réponse le trafic reçoit. **TBD.**

## Références

- `workspaces` : `BO-1`, `BO-2`, `BR-1`, `BR-2`, `BR-3`, `BR-5`, `BR-7`, `FR-2`, `FR-3`, `DEC-4`, `OI-2`, `OI-3`, `AC-3`, `AC-4`, `AC-7`.
- `workspaces/owners` : `FR-8`, `FR-11`, `CON-1`, `OI-7`.
- `sandbox` : `BR-1`, `BR-2`, `BR-3`, `BR-5`, `BR-8`, `BR-9`, `CON-1`, `DEC-1`, `OI-1`, `OI-2`.
- SpecDD /controller/src/sandbox.sdd, SpecDD /crds/src/lib.sdd, SpecDD /sandbox/src/auth.sdd, SpecDD /sandbox/src/config.sdd.
- `AGENTS.md`, sections « Clients de la sandbox » et « Architecture » ; `docs/architecture.md`, cité par les specs, non relu pour ce document.
