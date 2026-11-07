# Domain Definition: Espaces de travail (Owner, Project, Sandbox, Application)

## Résumé

Le domaine `workspaces` couvre ce qui fabrique et fait vivre, sur Kubernetes, les espaces de travail : l'identité
cluster d'un utilisateur (**Owner**), ses dépôts (**Project**), ses pods de travail (**Sandbox**) et l'instance
déployée de l'application (**Application**). Il est réalisé par le controller, les CRDs et, pour l'identité en base, par
`app`. Sa direction ne change pas avec la réorientation du harnais LLM.

Statut du document : **ébauche** (2026-11-03), écrite depuis les décisions déjà consignées (`vanyline.sdd`, `AGENTS.md`,
`docs/architecture.md`, specs du controller et des CRDs). Aucune Solution n'est rédigée.

## Objectif et problème

Un utilisateur doit obtenir, sans manipuler Kubernetes, un espace par projet et par branche, isolé des autres
utilisateurs, avec son dépôt, ses caches et ses toolchains. Le cluster doit rester l'unique source de vérité, que les
objets soient créés par l'interface, par la ligne de commande ou par GitOps.

## Objectifs métier

- **BO-1:** Un utilisateur dispose d'un espace de travail par projet et par branche, créé et entretenu automatiquement.
    - **Source:** `AGENTS.md`, composant controller

- **BO-2:** Les objets créés hors de l'interface (CLI, `kubectl`, GitOps) sont reconnus par l'application sans
  intervention.
    - **Source:** `vanyline.sdd`, section `Must` (arbitrage du 2026-10-23)

## Périmètre et non-objectifs

Le domaine couvre : les quatre CRDs `vanyline.solidite.fr/v1alpha1`, leur réconciliation, le namespace de chaque Owner,
le stockage, la maintenance des dépôts, la suspension et la modification des sandboxes, le réseau des sandboxes,
l'instance déployée de l'application, et l'identité Project/Sandbox en base avec sa réconciliation.

Hors périmètre, par décision :

- le contenu et le comportement du pod : domaine `sandbox` ;
- l'interface de gestion de ces objets : domaine `app` ;
- le moteur de workflow Restate lui-même : domaine `workflow` de miryad-core ; seul son déploiement comme
  sous-ressource de l'Application relève de ce domaine.

## Acteurs

- **Utilisateur :** crée projets et sandboxes par l'interface, la CLI ou GitOps.
- **Controller :** réconcilie les CRDs.
- **`app` :** pilote les objets avec les droits de l'utilisateur et réconcilie la base sur le cluster.
- **Administrateur du cluster :** installe les CRDs, le controller et l'Application.

## État actuel et état cible

- **Actuel (Observed in: `controller/src/`, `crds/src/lib.rs`) :** quatre reconcilers, Jobs de maintenance par
  `vanyline-maint`, NetworkPolicies à trois niveaux, suspension manuelle, Ingress par sandbox.
- **Cible :** un Owner = un namespace ; identité Project/Sandbox en base avec réconciliation ; modification d'une
  sandbox sans redémarrage forcé ; Restate comme sous-ressource de l'Application. Ces chantiers sont des tâches ouvertes
  de `vanyline.sdd`.

## Hypothèses, contraintes et dépendances

- **CON-1:** Une CRD ne se justifie que par un état désiré à réconcilier, pas par la possession d'un objet natif.
    - **Source:** `docs/architecture.md`, section « Opérateur Kubernetes »

- **CON-2:** Le workspace d'un Project est un volume local en bloc `RWO` (les surveillances de fichiers ne traversent
  pas les systèmes de fichiers réseau) ; le home d'un Owner est un volume petit et `RWX`.
    - **Source:** `docs/architecture.md`, section « Opérateur Kubernetes »

- **Dépendance :** l'image de la sandbox exécute la maintenance des dépôts (`sandbox#BR-8`).

## Glossaire

- **Owner :** identité cluster d'un utilisateur : namespace, ServiceAccount sans droit, PVC home, référence optionnelle
  vers une Application.
- **Project :** dépôt git : PVC workspace, dépôt bare, caches, Jobs de maintenance.
- **Sandbox :** pod de travail d'un Owner sur une branche d'un Project.
- **Application :** instance déployée de `app` (Deployment, Service, Ingress, Restate éventuel).
- **Identité en base :** ligne par Project et par Sandbox qui permet aux tables d'`app` de s'y rattacher par clé
  étrangère ; elle ne reflète jamais `spec` ni `status`.

## Règles du domaine

- **BR-1 - MUST:** Un Owner correspond à un namespace `<app-ns>-<owner>` qui contient son ServiceAccount, son PVC home,
  ses Projects, ses Sandboxes et tout ce qu'ils produisent ; supprimer l'Owner supprime son namespace.
    - **Source:** `vanyline.sdd`, `AGENTS.md` (décision du 2026-10-04) ; SpecDD /controller/src/namespace.sdd
    - **Verified by:** `AC-1`

- **BR-2 - MUST:** La CR Kubernetes est l'unique source de vérité de la `spec` et du `status` d'un Project ou d'une
  Sandbox ; la base ne porte que leur identité, et se réconcilie toujours sur le cluster.
    - **Source:** `vanyline.sdd`, section `Must` (décision du 2026-10-23)
    - **Verified by:** `AC-2`

- **BR-3 - MUST:** Le nom d'une sandbox est `<projet>-<nom>` ; nom, projet et propriétaire sont immuables après
  création ; un dépassement de la limite d'un label DNS (63) est refusé, jamais tronqué.
    - **Source:** `vanyline.sdd`, section `Must`
    - **Verified by:** `AC-3`

- **BR-4 - MUST:** Une CR sans ligne d'identité est adoptée si son Owner a un lien utilisateur connu ; une ligne dont la
  CR a disparu n'est supprimée qu'après un `NotFound` confirmé et un délai de grâce de 180 s.
    - **Source:** `vanyline.sdd`, section `Must` (arbitrage du 2026-10-23)
    - **Verified by:** `AC-2`

- **BR-5 - MUST:** La suspension d'une sandbox est manuelle ; elle supprime le pod et conserve le worktree, le Service et
  les politiques réseau. Il n'y a pas d'arrêt automatique sur inactivité.
    - **Source:** `docs/architecture.md`, section « Suspension manuelle » (décision du 2026-07-12)
    - **Verified by:** `AC-4`

- **BR-6 - MUST:** Le réseau sortant d'une sandbox est l'union des règles déclarées par l'Owner, le Project et la
  Sandbox ; sans aucune règle, aucune politique n'est produite.
    - **Source:** `docs/architecture.md`, section « NetworkPolicies egress à trois niveaux » (2026-08-01)
    - **Verified by:** `AC-5`

- **BR-7 - MUST:** Le controller ne passe aucun champ de CRD à un shell : les Jobs de maintenance invoquent
  `vanyline-maint` en argv.
    - **Source:** `vanyline.sdd`, section `Must not` ; `sandbox#BR-8`
    - **Verified by:** `AC-6`

## Exigences fonctionnelles transverses

- **FR-1 - MUST:** Créer un Project clone son dépôt, prépare ses caches et détecte ses langages ; un fetch périodique
  maintient le dépôt à jour.
    - **Source:** SpecDD /controller/src/project.sdd

- **FR-2 - MUST:** Créer une Sandbox crée un worktree de sa branche, un pod avec les toolchains explicites ou détectées,
  un Service et ses politiques réseau ; un Ingress public est créé si l'Owner référence une Application.
    - **Source:** SpecDD /controller/src/sandbox.sdd

- **FR-3 - MUST:** Modifier une sandbox est possible en tout temps, sans suspension ni redémarrage automatique ; si le
  changement demande un redémarrage, l'état de la sandbox le signale.
    - **Source:** `vanyline.sdd`, tâche ouverte (arbitrage du 2026-10-23, « option C »)
    - **Verified by:** `AC-7`

- **FR-4 - MUST:** Une Application déploie `app` (Deployment, Service, Ingress) avec ses secrets référencés ; si `restate`
  est déclaré, elle déploie aussi Restate.
    - **Source:** `AGENTS.md`, composant controller ; SpecDD /controller/src/application.sdd
    - **Verified by:** `AC-8`

## Paysage des Solutions (proposition de découpage)

- `workspaces/owners` : identité, namespace, ServiceAccount, home.
- `workspaces/projects` : dépôt, maintenance, détection de langages.
- `workspaces/sandboxes` : pod, suspension, modification, Ingress.
- `workspaces/network` : politiques réseau d'entrée et de sortie.
- `workspaces/application` : déploiement d'`app` et de Restate.
- `workspaces/identity` : lignes d'identité en base, adoption et suppression par réconciliation (réalisée dans `app`).
- `workspaces/controller` : câblage du binaire `vanyline-controller` (démarrage des reconcilers, erreurs `VNL-CTL-*`).
- `workspaces/client` : la librairie d'interface avec le cluster et les sandboxes (`vanyline-lib`), partagée par `app` et la
  CLI (`DEC-3`). À écrire.

## Décisions du domaine

- **DEC-1 - Le cluster est la source de vérité, la base le suit**
    - **Decision:** voir `BR-2` et `BR-4`.
    - **Alternatives:** la base comme source (écarté le 2026-10-23).
    - **Source:** `vanyline.sdd`, section `Must` (arbitrage du 2026-10-23)

- **DEC-2 - Quatre CRDs, une par état désiré distinct**
    - **Decision:** Owner, Project, Sandbox, Application, réconciliés en parallèle par un seul controller.
    - **Source:** `docs/architecture.md`, section « Opérateur Kubernetes »

- **DEC-3 - `vanyline-lib` est la librairie d'interface avec le cluster et les sandboxes**
    - **Context:** `vanyline-lib` portait le moteur LLM du monorepo, qui part (`app#DEC-1`) ; elle porte aussi le client
      Kubernetes (`VnyK8sClient`, jeton d'Owner par TokenRequest), indépendant du LLM. `app` (relais de ticket, de git et de
      santé du code) et la CLI (ticket, `/ws/fs`, `/git/status`, connexion MCP) appellent la même sandbox avec le même jeton.
    - **Decision:** `vanyline-lib` reste, réduite à cette interface : le client Kubernetes et le client de la sandbox. Tout
      code commun à `app` et à la CLI pour parler au cluster ou à une sandbox y vit, plutôt que d'être écrit deux fois. Sa
      partie LLM (sessions, événements, MCP préfixé, modèles) est retirée au profit de lymaille.
    - **Consequences:** la spec racine de la crate (`SpecDD /lib/src/lib.sdd`, qui la décrit encore comme cœur LLM, MCP et chat)
      est à réécrire avant tout retrait. Le périmètre exact du client de la sandbox est à fixer dans la Solution
      `workspaces/client`.
    - **Source:** décision du développeur, 2026-11-07

## Transition et migration

Les specs existantes du controller et des CRDs décrivent l'état actuel ; certaines portent encore un modèle à namespace
unique (tâche ouverte « Un Owner = un namespace » de `vanyline.sdd`). La descente par Solution les amende.

## Acceptation et vérification

- **AC-1:** Créer un Owner crée son namespace ; le supprimer supprime le namespace et son contenu.
- **AC-2:** Une CR de sandbox créée par `kubectl` reçoit sa ligne d'identité ; une CR supprimée voit sa ligne supprimée
  entre 180 et 240 s plus tard, jamais sur un listing incomplet.
- **AC-3:** Créer une sandbox dont le nom dépasse la limite est refusé en 422 ; une collision de nom répond 409.
- **AC-4:** Suspendre une sandbox supprime son pod et garde son worktree ; la relancer la remet en `Running` sans
  nouveau checkout.
- **AC-5:** Avec des règles sur deux niveaux, la politique de sortie de la sandbox est leur union.
- **AC-6:** Aucun champ de CRD ne se retrouve dans une commande shell d'un Job.
- **AC-7:** Modifier les toolchains d'une sandbox en cours signale un redémarrage requis et ne redémarre pas le pod.
- **AC-8:** Une Application avec `restate` déploie le StatefulSet et le Service de Restate.

## Open Issues

- **OI-1:** **Forme du signal de redémarrage requis** : `RestartPending` en condition ou en phase (`vanyline.sdd`, tâche ouverte, choix « à
  relire »). **TBD.**
- **OI-2:** **Watch + resynchronisation à 60 s, ou resynchronisation seule à 30 s ?** Contrat provisoire dans
  `vanyline.sdd`. **TBD.**
- **OI-3:** **Isolation noyau et quotas des sandboxes** (`sandbox#OI-1`, `sandbox#OI-2`) : leur réalisation relève du
  controller. **TBD.**
- **OI-4:** **Restate comme sous-ressource de l'Application** : contrat `RestateSpec` dans `crds/src/lib.sdd` restant à
  écrire (tâche ouverte de `vanyline.sdd`). **TBD.**

## Références

- `vanyline.sdd`, `AGENTS.md`, `docs/architecture.md`.
- `sandbox` (`sandbox#BR-8`, `sandbox#OI-1`, `sandbox#OI-2`).
