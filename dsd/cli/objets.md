# Solution Design: Gestion des objets Kubernetes en CLI

## Résumé

Cette Solution décrit les commandes `owner`, `project` et `sandbox` : créer, lister, afficher, modifier, supprimer, et
pour les sandboxes démarrer et arrêter. Elles agissent directement sur le cluster avec le kubeconfig de l'utilisateur,
sans passer par `app`. Presque tout existe ; la Solution ajoute la modification (`apply`, `edit`), la résolution de
l'Owner et la validation côté CLI.

## Problème et objectif

Le développeur doit piloter ses espaces de travail sans navigateur, et pouvoir modifier une Sandbox ou un Project en cours
de vie, ce que le controller permet (`workspaces#FR-3`) mais que la CLI actuelle n'expose pas : `create` n'atteint ni les
ressources, ni la sortie réseau, ni les variables d'environnement et serveurs de langage par toolchain.

## Périmètre et non-objectifs

Couvre : les commandes d'objets, la résolution de l'Owner et du namespace, la validation locale du nommage, la modification.

Hors périmètre : le fonctionnement des objets (domaine `workspaces`), le chat et les scripts (autres Solutions), le chemin
RPC `owners/*`, `projects/*`, `sandboxes/*` qui atteint les mêmes méthodes du client K8s (`cli#OI-2`).

## Contexte applicable du domaine

`cli#FR-1`, `cli#CON-2`, `cli#BR-1`, `cli#BR-2`, `cli#DEC-5`. `workspaces#BR-3` (nom d'une sandbox `<projet>-<nom>`,
immuabilité), `workspaces#BR-5` (suspension manuelle), `workspaces#FR-3` (modification sans redémarrage forcé),
`workspaces#DEC-1` (le cluster est la source de vérité).

## Acteurs

- **Développeur :** crée et modifie ses Projects et Sandboxes avec son kubeconfig.
- **Administrateur :** crée et supprime les Owners.
- **Controller :** réconcilie les objets créés ; `app` adopte ceux qui ont un Owner lié à un utilisateur.

## État actuel et état cible

- **Actuel (Observed in: `cli/src/owner_cmd.rs`, `project_cmd.rs`, `sandbox_cmd.rs`, dispatch `run_*_k8s` de `main.rs`) :**
  `list|show|create|delete` pour les trois familles, plus `stop|start` pour les sandboxes (câblés par un changement de
  `suspended`). `--owner` obligatoire sur `project create` et pour toute sandbox, `defaults.namespace` en repli du namespace.
- **Cible :** les mêmes commandes, plus `apply -f` et `edit` pour les Projects et les Sandboxes, Owner résolu par option
  puis réglage utilisateur.

## Hypothèses, contraintes et dépendances

- **CON-1:** Les commandes de cette Solution n'ont pas besoin de session lymaille ni de LLM : elles ne dépendent que du client
  Kubernetes et de `vanyline-crds`.
    - **Source:** `cli#CON-2`

- **CON-2:** Le nom, le projet et le propriétaire d'une sandbox sont immuables après création.
    - **Source:** `workspaces#BR-3`

## Exigences fonctionnelles

- **FR-1 - MUST:** `vanyline owner|project|sandbox list|show|create|delete` créent, listent, affichent et suppriment les
  objets de ce nom, avec le contexte kubeconfig de l'utilisateur.
    - **Source:** `cli#FR-1`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** `vanyline sandbox stop <nom>` suspend la sandbox (le pod disparaît, le worktree est conservé) et
  `vanyline sandbox start <nom>` la relance.
    - **Source:** `cli#FR-1`, `workspaces#BR-5`
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** `vanyline project apply -f <fichier>` et `vanyline sandbox apply -f <fichier>` créent l'objet décrit ou
  mettent à jour sa spécification complète, de façon idempotente. `edit <nom>` ouvre la spécification dans l'éditeur de
  l'utilisateur (`$EDITOR`) et applique le résultat s'il est valide.
    - **Source:** décision du développeur, 2026-11-07 ; `workspaces#FR-3`
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** Une modification qui touche un champ immuable (`CON-2`) est refusée avant tout appel au cluster, avec le
  nom du champ.
    - **Source:** `workspaces#BR-3`, `cli#BR-2`
    - **Verified by:** `AC-4`

- **FR-5 - MUST:** Après `apply` ou `edit`, et dans la sortie de `show`, la commande affiche le signalement « redémarrage
  requis » quand l'état de la sandbox le porte ; elle ne redémarre jamais le pod d'elle-même.
    - **Source:** `workspaces#FR-3`
    - **Verified by:** `AC-5`

- **FR-6 - MUST:** `vanyline sandbox create <nom> --project <projet> --branch <branche>` construit le nom de l'objet
  `<projet>-<nom>` et valide longueur et caractères avant tout appel au cluster, par le nommage de `vanyline-crds`.
    - **Source:** `workspaces#BR-3`, `cli#BR-2`
    - **Verified by:** `AC-6`

- **FR-7 - MUST:** L'Owner d'une commande se résout par `--owner`, puis par `defaults.owner` du niveau utilisateur
  (`$HOME/.config/vanyline/`) ; sans l'un ni l'autre, la commande échoue avec une erreur d'usage qui nomme les deux
  moyens de le fournir. Le namespace de l'Owner est calculé par le nommage de `vanyline-crds`.
    - **Source:** décision du développeur, 2026-11-07 ; `cli#BR-2`
    - **Verified by:** `AC-7`

- **FR-8 - MUST:** Le namespace d'application se résout par `--namespace`, puis `defaults.namespace` du niveau utilisateur,
  puis le contexte kubeconfig.
    - **Source:** comportement actuel conservé (`SpecDD /cli/src/main.sdd`)

## Sécurité

Ces commandes ne contournent ni le RBAC du cluster ni l'authentification de la sandbox : elles présentent l'identité du
kubeconfig, et le jeton d'Owner reste réservé aux appels à la sandbox (`cli#BR-1`). La CLI n'écrit jamais de secret dans
une spécification ; les champs de secret des objets ne contiennent que des références.

## Conception

Les commandes forment une famille par type d'objet, chacune une spécification clap d'un côté et un dispatch de l'autre. Les
dispatchs `run_owner_k8s`, `run_project_k8s` et `run_sandbox_k8s`, aujourd'hui dans `main.rs`, vont chacun dans le module de
leur famille (**ASSUMPTION** de découpage), car `main.sdd` ne doit plus porter d'orchestration. `apply` et `edit` s'appuient sur
la spécification complète du CRD, pas sur les flags de `create`, qui gardent le cas simple. `edit` valide la spécification
(champs immuables, nommage) avant de l'appliquer, et conserve le fichier temporaire en cas d'erreur pour ne pas perdre la
saisie.

## Décisions

- **DEC-1 - Modifier passe par la spécification complète, pas par des flags**
    - **Context:** les flags de `create` n'atteignent ni `resources`, ni `egress`, ni `env`/`lsp` par toolchain ; en ajouter un
      par champ grossirait la surface à chaque évolution du CRD.
    - **Decision:** `apply -f` et `edit` manipulent la spécification entière ; `create` garde ses flags.
    - **Alternatives:** `set` avec un flag par champ (écartée : surface qui grossit) ; modifier = supprimer et recréer
      (écartée : perd le worktree, contredit `workspaces#FR-3`).
    - **Source:** décision du développeur, 2026-11-07

- **DEC-2 - L'Owner se donne explicitement, sans déduction**
    - **Decision:** option, puis réglage utilisateur, puis erreur. Aucune déduction depuis le kubeconfig (la correspondance
      identité vers Owner n'existe pas dans les CRDs, elle est en base d'`app`), aucune recherche dans tous les namespaces.
    - **Source:** décision du développeur, 2026-11-07

## Réalisation SpecDD

À conserver : `SpecDD /cli/src/owner_cmd.sdd`, `SpecDD /cli/src/project_cmd.sdd`, `SpecDD /cli/src/sandbox_cmd.sdd`, avec
leurs variants `apply` et `edit` à ajouter. À réduire : les dispatchs et leurs scénarios dans `SpecDD /cli/src/main.sdd`.
À traiter à l'écriture des specs, les tâches ouvertes de ces trois fichiers : flag `--git-secret` déprécié côté CRD
(**ASSUMPTION** : le retirer), aide de `--image`, `--image ""`.

## Compatibilité et migration

`defaults.toolbox`, qui rendait tout chat dépendant du cluster, disparaît avec `--toolbox` : `--sandbox` est toujours explicite
(`cli#DEC-5`, `cli/chat#FR-1`). Le fichier de réglages utilisateur (`defaults.namespace`, `defaults.owner`) est
`$HOME/.config/vanyline/config.yaml` (**ASSUMPTION** : il remplace le `config.yaml` à deux couches de `cfgstore`, réduit à ces
réglages).

## Acceptation et vérification

- **AC-1:** `project create` puis `project show` rendent l'objet créé ; `project delete` le supprime.
- **AC-2:** `sandbox stop` laisse le worktree et supprime le pod ; `sandbox start` remet la sandbox en `Running`.
- **AC-3:** `sandbox apply -f` avec une spec modifiée (par exemple `egress`) met à jour la sandbox sans la recréer ; rejouer le
  même fichier ne change rien.
- **AC-4:** Un `apply` qui change le projet d'une sandbox existante est refusé, sans appel au cluster, avec le nom du champ.
- **AC-5:** Modifier les toolchains d'une sandbox en cours affiche « redémarrage requis » et ne redémarre pas le pod.
- **AC-6:** `sandbox create` avec un nom qui dépasse la limite est refusé avant tout appel au cluster ; le nom créé est
  `<projet>-<nom>`.
- **AC-7:** Sans `--owner` ni `defaults.owner`, `project create` échoue avec une erreur d'usage qui nomme les deux moyens ; avec
  `defaults.owner`, il réussit sans `--owner`.

## Risques

- Le nommage `<projet>-<nom>` de la CLI doit rester identique à celui d'`app` et du controller : il vient de `vanyline-crds`,
  et ne doit jamais être recopié dans la CLI.

## Open Issues

- **OI-1:** **Owner en `apply`.** Les Owners n'ont pas d'`apply` ni d'`edit` (modification de leur stockage home, réservée à
  l'administrateur) ; à ajouter si le besoin apparaît. **TBD.**

## Références

`cli` (`cli#FR-1`, `BR-1`, `BR-2`, `CON-2`), `workspaces` (`BR-3`, `BR-5`, `FR-3`, `DEC-1`), `vanyline-crds`.
