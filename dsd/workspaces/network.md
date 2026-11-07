# Solution Design: Réseau des sandboxes

## Résumé

La Solution `workspaces/network` décrit les politiques réseau qui encadrent le pod d'une Sandbox : une NetworkPolicy de sortie (`sandbox-<n>-egress`) qui réalise l'union des règles `egress` de l'Owner, du Project et de la Sandbox ; une NetworkPolicy d'entrée (`sandbox-<n>`), toujours présente, qui n'ouvre que le port MCP et le port d'exploitation ; et la propagation par annotation de bump qui force la reconstruction de la sortie quand l'Owner ou le Project change. Elle est réalisée par le reconciler Sandbox (`controller/src/sandbox.rs`) et alimentée par les reconcilers Owner et Project. Le pod, sa suspension et son Ingress relèvent de `workspaces/sandboxes`, non décrits ici.

Ce document est un reverse-engineering : il descend les specs existantes en exigences, sans fabriquer de comportement. Les Scenarios d'intégration cluster ne sont pas encore écrits.

## Problème et objectif

Le pod de travail exécute du code produit par un agent LLM et des scripts d'installation (`sandbox#BR-9`). Sans règle, son trafic sortant n'est pas restreint (`sandbox`, section Risques). La Solution doit permettre à l'Owner, au Project et à la Sandbox de restreindre la sortie de façon déclarative (`workspaces#BR-6`), n'ouvrir l'entrée du pod qu'aux chemins de service connus, et garder le port d'exploitation joignable par les sondes kubelet sans publication (SpecDD /controller/src/sandbox.sdd, Tasks, décision 2026-10-18). La NetworkPolicy n'est pas le contrôle d'accès : l'authentification du pod est décidée ailleurs (SpecDD /controller/src/sandbox.sdd, Must).

## Périmètre et non-objectifs

Couvert : la politique de sortie `sandbox-<n>-egress` ; la politique d'entrée `sandbox-<n>` et sa règle du port d'exploitation ; l'annotation `vanyline.solidite.fr/egress-bump`, posée par les reconcilers Owner et Project ; le champ `egress` des CR Owner, Project et Sandbox (SpecDD /crds/src/lib.sdd).

Hors périmètre : le pod, le Service, la suspension, l'Ingress public et la modification d'une sandbox (`workspaces/sandboxes`) ; le Project et l'Owner au-delà du bump (`workspaces/projects`, `workspaces/owners`) ; la liste de menaces et les bornes de sortie (`sandbox/threat-model`) ; l'isolation noyau (`sandbox#OI-2`) ; les quotas (`workspaces#OI-3`) ; l'authentification par jeton (SpecDD /sandbox/src/auth.sdd).

## Contexte applicable du domaine

- `workspaces#BR-6` : le réseau sortant est l'union des règles des trois niveaux ; sans règle, aucune politique. Non redéfini ici.
- `workspaces#BR-5` : la suspension conserve les politiques réseau.
- `workspaces#DEC-2` : la Sandbox est l'une des quatre CRDs réconciliées par le controller.
- `sandbox#BR-9` et `sandbox#DEC-3` : les effets de l'agent se bornent au pod ; les moyens, dont le réseau sortant, sont décidés dans `sandbox/threat-model` avec `workspaces`.

## Acteurs

- Utilisateur : déclare `egress` dans les CR Owner, Project et Sandbox (SpecDD /crds/src/lib.sdd).
- Controller, reconciler Sandbox : construit et applique les deux politiques (SpecDD /controller/src/sandbox.sdd).
- Controller, reconcilers Owner et Project : posent l'annotation de bump (SpecDD /controller/src/owner.sdd, SpecDD /controller/src/project.sdd).
- Administrateur du cluster : fournit le CNI qui applique les politiques et la variable `VNL_OPS_ALLOWED_NAMESPACES` (SpecDD /controller/src/sandbox.sdd, Tasks).
- Pairs réseau : pods `app` (label `APP_LABEL`, namespace d'application), contrôleur d'ingress (si Application), Service Restate, kubelet.

## État actuel et état cible

- Actuel (Observed in: `controller/src/sandbox.rs`) : builders purs `@build_sandbox_netpol` et `@build_sandbox_egress_netpol`, avec tests unitaires nommés d'après les Scenarios. Le crate n'a aucun test de reconcile sur cluster (SpecDD /controller/src/sandbox.sdd, Tasks).
- Cible : Scenarios `_cluster` taggés `integration` (harnais à écrire), et rejeu des Scenarios `netpol_*` avec deux namespaces, tâche « ALIGNEMENT » ouverte (SpecDD /controller/src/sandbox.sdd, Tasks).

## Hypothèses, contraintes et dépendances

- **CON-1:** Le DNS de la politique de sortie ne cible aucun label kube-dns. Les seuls labels de pairs autorisés hors pairs explicites sont `vanyline.solidite.fr/*`, `APP_LABEL` et `kubernetes.io/metadata.name`.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Must not (décision 2026-08-01)
- **CON-2:** La propagation Owner ou Project vers Sandbox passe uniquement par l'annotation de bump et les requeues ; aucun watch inter-CRD.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (`@build_controller`) ; SpecDD /controller/src/project.sdd, Must
- **CON-3:** Les politiques ne sont effectives que si le CNI les applique. Le trafic kubelet vers pod est autorisé par le CNI (Cilium : entité `host`) ; sur un autre CNI, les CIDR des nœuds doivent être ajoutés.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Tasks (décision 2026-10-04, validée 2026-10-18)
- **CON-4:** La règle de pair Restate ne matche que si le pod template du StatefulSet Restate porte `@RESTATE_LABEL`.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (renvoi à SpecDD /controller/src/application.sdd)
- **Dépendances :** `workspaces/owners` (`workspaces/owners#FR-13`, bump Owner) ; SpecDD /controller/src/project.sdd (bump Project) ; `vanyline-crds` (`EgressRule`, `EgressPort`).

## Exigences fonctionnelles

- **FR-1 - MUST:** La sortie de la Sandbox est une politique unique, union des règles `egress` de l'Owner, du Project puis de la Sandbox, dans cet ordre. Aucune règle aux trois niveaux ne produit aucune politique (FR-4).
    - **Source:** `workspaces#BR-6` ; SpecDD /controller/src/sandbox.sdd, Build (`@build_sandbox_egress_netpol`)
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** La règle DNS est toujours la première de la politique de sortie, sans restriction de destination, sur 53/UDP et 53/TCP.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (décision 2026-08-01)
    - **Verified by:** `AC-1`
- **FR-3 - MUST:** Une règle portant `cidr` produit un pair `ipBlock` et ignore ses sélecteurs, même posés. Sans `cidr`, les sélecteurs sont passés tels quels. `ports` vide signifie tous les ports ; un port sans `protocol` reçoit explicitement `TCP`.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build ; SpecDD /crds/src/lib.sdd, `EgressRule`, `EgressPort`
    - **Verified by:** `AC-2`
- **FR-4 - MUST:** Quand les trois niveaux sont vides, aucune politique de sortie n'est produite et une politique existante est supprimée explicitement. L'egress du pod reste alors non restreint.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build et Apply (`@apply`)
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** Si `restate` est fourni et que l'union est non vide, une règle terminale autorise le seul port 8080/TCP du Service Restate de l'Application, via un pair combinant le namespace d'application et `@RESTATE_LABEL`.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build ; Scenario `netpol_egress_restate_regle_terminale_combinee`
    - **Verified by:** `AC-4`
- **FR-6 - MUST:** La politique d'entrée `sandbox-<n>` est toujours produite. Elle n'ouvre que le port MCP/TCP, depuis les pods du même namespace, les pods portant `APP_LABEL` dans le namespace d'application et, si `ingress_controller` est posé, les pods `podLabels` de son namespace.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (`@build_sandbox_netpol`) ; Scenarios `netpol_ingress_cible_owner_app_et_port_mcp`, `netpol_ingress_ajoute_peer_controleur_ingress`
    - **Verified by:** `AC-5`, `AC-6`
- **FR-7 - MUST:** Le port d'exploitation `OPS_PORT`/TCP est ouvert par une règle d'entrée distincte, jamais publiée par Service ni Ingress. Sa source est restreinte aux namespaces de `VNL_OPS_ALLOWED_NAMESPACES` quand la variable est posée, et ne l'est pas sinon.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Tasks (décision 2026-10-18), Scenarios `netpol_port_ops_ouvert_au_cluster_par_defaut`, `netpol_port_ops_restreinte_par_env`, `port_ops_jamais_publie`
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** Un changement d'`egress` de l'Owner ou du Project est répercuté sur la politique de sortie des Sandboxes concernées, par l'annotation `vanyline.solidite.fr/egress-bump` posée à chaque reconcile réussi, sans détection de changement. Le déclenchement du reconcile Sandbox est ouvert (OI-5).
    - **Source:** `workspaces/owners#FR-13` ; SpecDD /controller/src/project.sdd, Handles (bump) ; SpecDD /controller/src/sandbox.sdd, Build (`@build_controller`)
    - **Verified by:** `AC-8`, `AC-9`
- **FR-9 - MUST:** Les deux politiques sont appliquées en server-side apply sous `vanyline-controller` avec `force`, portent un ownerReference vers la Sandbox, et déclarent `[Egress]` pour la sortie, `[Ingress]` pour l'entrée.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build et Apply ; Scenario `netpol_egress_nom_selecteur_politypes_ownerref`
    - **Verified by:** `AC-10`

## Sécurité

- Entrée : le pod n'accepte le MCP que depuis les trois familles de pairs de FR-6. Le port d'exploitation est ouvert sans `from` par défaut (DEC-5). La protection réelle du MCP est le jeton du ServiceAccount de l'Owner ; `--no-auth` ne doit jamais être posé (SpecDD /controller/src/sandbox.sdd, Must ; SpecDD /sandbox/src/auth.sdd).
- Conséquence du pair `APP_LABEL` : tout pod portant ce label dans le namespace d'application peut atteindre le MCP. La portée réelle dépend donc des pods déployés dans ce namespace (SpecDD /controller/src/sandbox.sdd, Build).
- Sortie : sans règle, le trafic sortant est libre (`sandbox`, section Risques ; `sandbox#OI-1`). Les bornes relèvent de `sandbox/threat-model` (`sandbox#DEC-3`).
- Restate : le pod n'atteint que le port 8080 du Service de la seule Application. L'admin 9070 reste réservé au Deployment `app` (SpecDD /controller/src/sandbox.sdd, Build).

## Conception

```text
Sandbox <n>  (namespace <app-ns>-<owner>)
  |-- NetworkPolicy sandbox-<n>             Ingress, toujours produite
  |     MCP/TCP <- même namespace | APP_LABEL du <app-ns> | podLabels du contrôleur
  |     OPS/TCP <- toute source (ou namespaces de VNL_OPS_ALLOWED_NAMESPACES)
  `-- NetworkPolicy sandbox-<n>-egress    Egress, si l'union est non vide
        1. DNS 53/UDP, 53/TCP -> toute destination
        2. règles Owner, puis Project, puis Sandbox (cidr -> ipBlock | sélecteurs)
        n. [restate] namespace <app-ns> + @RESTATE_LABEL : 8080/TCP (règle terminale)
```

- Entrées de l'union : champs `egress` (`#[serde(default)]`) de `OwnerSpec`, `ProjectSpec` et `SandboxSpec` (SpecDD /crds/src/lib.sdd). Le type ne valide ni l'exclusivité `cidr`/`podSelector` ni le contenu (OI-6).
- Ordre d'apply : la politique d'entrée puis celle de sortie ; la sortie absente est supprimée explicitement (SpecDD /controller/src/sandbox.sdd, Apply).
- Propagation : Owner (`workspaces/owners#FR-13`) ou Project pose `egress-bump` sur les Sandboxes ; le reconcile Sandbox reconstruit la sortie. En l'absence de déclenchement, le requeue sert de repli : 300 s si la phase est `Running` ou `Suspended`, 15 s sinon (SpecDD /controller/src/sandbox.sdd, Apply).

## Décisions

- **DEC-1 - Deux politiques distinctes pour l'entrée et la sortie**
    - **Context:** La sortie est conditionnelle (absente si l'union est vide), l'entrée est toujours produite.
    - **Decision:** `sandbox-<n>` pour l'entrée, `sandbox-<n>-egress` pour la sortie ; la suppression de la sortie n'affecte pas l'entrée.
    - **Consequences:** Une sortie retirée ne retire pas les règles d'entrée.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (objet DISTINCT de la netpol ingress)
- **DEC-2 - Bump inconditionnel, sans watch inter-CRD**
    - **Context:** Les changements d'`egress` doivent atteindre les Sandboxes sans watch permanent entre CRD.
    - **Decision:** L'annotation est posée à chaque reconcile réussi, sans comparaison.
    - **Consequences:** Efficacité du bump liée à OI-5.
    - **Source:** `workspaces/owners#DEC-6` ; SpecDD /controller/src/project.sdd, Must (décision WS-13)
- **DEC-3 - DNS sans destination ni label kube-dns**
    - **Context:** La résolution doit fonctionner sans dépendre de la configuration du cluster.
    - **Decision:** Règle DNS sans `to`, sur 53/UDP et 53/TCP.
    - **Alternatives:** Cibler kube-dns par labels (écarté le 2026-08-01).
    - **Consequences:** Résolution ouverte vers toute destination (OI-7).
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build et Must not (décision 2026-08-01)
- **DEC-4 - Pas de politique « restate seul »**
    - **Context:** Une politique ne contenant que la règle Restate fermerait tout le reste du trafic sortant.
    - **Decision:** Le gate « union vide ⇒ aucune politique » précède la règle Restate.
    - **Consequences:** Un pod sans règle aux trois niveaux garde une sortie libre, même avec Restate (OI-1).
    - **Source:** SpecDD /controller/src/sandbox.sdd, Build (gate d'absence)
- **DEC-5 - Port d'exploitation ouvert sans `from` par défaut**
    - **Context:** Les sondes kubelet doivent atteindre `/health` sans source fixe, selon le CNI.
    - **Decision:** Règle sans `from` ; restriction optionnelle par `VNL_OPS_ALLOWED_NAMESPACES`.
    - **Consequences:** Le périmètre réel dépend du CNI (CON-3) ; le port est sans authentification.
    - **Source:** SpecDD /controller/src/sandbox.sdd, Tasks (décision 2026-10-18)

## Réalisation SpecDD

- `SpecDD /controller/src/sandbox.sdd` : `@build_sandbox_netpol`, `@build_sandbox_egress_netpol`, apply des politiques (FR-1 à FR-7, FR-9).
- `SpecDD /controller/src/owner.sdd` : bump Owner (FR-8).
- `SpecDD /controller/src/project.sdd` : bump Project (FR-8).
- `SpecDD /crds/src/lib.sdd` : `EgressRule`, `EgressPort`, champs `egress` (FR-3, CON-1).
- Code : `controller/src/sandbox.rs` (Owns de `sandbox.sdd`), `controller/src/owner.rs`, `controller/src/project.rs`, `crds/src/lib.rs`.

## Acceptation et vérification

- **AC-1:** Owner, Project et Sandbox ayant chacun une règle produisent une politique de sortie avec le DNS en tête, puis les trois règles dans l'ordre (Scenarios `netpol_egress_union_proprietaire_projet_sandbox`, `netpol_egress_dns_toujours_en_tete_sans_destination`).
- **AC-2:** Un `cidr` donne un `ipBlock` qui prime sur les sélecteurs ; `ports` vide signifie tous les ports ; un protocole absent devient `TCP` (Scenarios `netpol_egress_cidr_devient_ip_block`, `netpol_egress_cidr_predominant_sur_selecteurs`, `netpol_egress_ports_vides_signifient_tous_ports`, `netpol_egress_protocole_absent_pose_tcp`).
- **AC-3:** Trois niveaux vides, `restate` posé ou non : aucune politique de sortie (Scenario `netpol_egress_none_quand_les_trois_niveaux_sont_vides`). La suppression d'une politique existante est un Scenario cluster, non écrit (OI-8).
- **AC-4:** `restate` posé et union non vide : règle terminale unique 8080/TCP ; `restate` absent : aucune règle Restate (Scenarios `netpol_egress_restate_regle_terminale_combinee`, `netpol_egress_sans_regle_restate_quand_none`).
- **AC-5:** L'entrée n'ouvre que le MCP, avec des pairs sans `ipBlock` (Scenario `netpol_ingress_cible_owner_app_et_port_mcp`, à amender, OI-2).
- **AC-6:** `ingress_controller` posé ajoute un troisième pair sans `ipBlock` (Scenario `netpol_ingress_ajoute_peer_controleur_ingress`).
- **AC-7:** La règle OPS n'a pas de `from` par défaut, se restreint par `VNL_OPS_ALLOWED_NAMESPACES`, et ni le Service ni l'Ingress ne publient `OPS_PORT` (Scenarios `netpol_port_ops_ouvert_au_cluster_par_defaut`, `netpol_port_ops_restreinte_par_env`, `port_ops_jamais_publie`).
- **AC-8:** Un bump Project ne modifie que les Sandboxes de ce Project, à chaque reconcile (Scenario `cluster_bump_annotation_sur_sandboxes_du_project_seulement`, intégration, non écrit ; côté Owner, `workspaces/owners#AC-7`).
- **AC-9:** Le bump déclenche effectivement le reconcile Sandbox (Scenario `bump_annotation_declenche_effectivement_reconcile_sandbox`, cité en `[?]` par SpecDD /controller/src/owner.sdd, non écrit). **TBD** (OI-5).
- **AC-10:** La politique de sortie se nomme `sandbox-<n>-egress`, déclare `[Egress]` et porte un ownerReference vers la Sandbox (Scenario `netpol_egress_nom_selecteur_politypes_ownerref`).

## Open Issues

- **OI-1 - Restate seul :** SpecDD /controller/src/sandbox.sdd, Must (l.62-64), dit que la politique de sortie « autorise ce seul Service » ; Build (gate) et Scenario `netpol_egress_none_quand_union_vide_malgre_restate` disent qu'avec union vide il n'y a aucune politique, donc sortie libre. **TBD :** quel contrat fait foi.
- **OI-2 - Peer owner obsolète :** Scenario `netpol_ingress_cible_owner_app_et_port_mcp` décrit un pair `owner: alice`, que Build (l.200-201) ne porte plus ; tâche « ALIGNEMENT » ouverte. **TBD :** rejouer le Scenario.
- **OI-3 - Formulation du port d'exploitation :** Must (l.316-317) dit « ouvre au cluster », alors que la tâche clôturée du 2026-10-18 demande « toute source pouvant atteindre le pod ». **TBD :** amender le Must.
- **OI-4 - Ingress sans contrôleur :** SpecDD /controller/src/sandbox.sdd, Tasks `[?]` (l.572-577) : Ingress créé sans pair de contrôleur dans la politique d'entrée. **TBD :** créer l'Ingress seulement si `ingress_controller` est posé, ou assumer.
- **OI-5 - Déclenchement du bump :** SpecDD /controller/src/owner.sdd, `[?]` (l.316-320) : kube-runtime ne réveille pas sur une annotation ; la propagation repose alors sur le requeue (300 s en `Running`). Question voisine de `workspaces#OI-2` (cadence watch/resync), non identique. **TBD.**
- **OI-6 - Validation des règles :** SpecDD /crds/src/lib.sdd, Tasks `[?]` : exclusivité `cidr`/`podSelector` non validée, `protocol` en chaîne libre. **TBD :** `oneOf`, webhook, ou accepté.
- **OI-7 - DNS comme canal de sortie :** la spec ne traite pas la résolution DNS vers toute destination comme canal de sortie ; à traiter dans `sandbox#OI-1`. **TBD.**
- **OI-8 - Harnais cluster absent :** les Scenarios `reconcile_netpol_egress_supprimee_quand_niveaux_vides_cluster` et `cluster_bump_annotation_sur_sandboxes_du_project_seulement` ne sont pas écrits (SpecDD /controller/src/sandbox.sdd, Tasks). **TBD.**

## Références

- `workspaces` : BR-5, BR-6, DEC-2, OI-2, OI-3 ; `workspaces/owners` : FR-13, DEC-6, OI-2.
- `sandbox` : BR-9, DEC-3, OI-1, OI-2 ; section Risques.
- SpecDD /controller/src/sandbox.sdd, /controller/src/owner.sdd, /controller/src/project.sdd, /crds/src/lib.sdd, /sandbox/src/auth.sdd.
- `docs/architecture.md`, section « NetworkPolicies egress à trois niveaux (WS-13) », citée par les specs, non relue pour ce document.
