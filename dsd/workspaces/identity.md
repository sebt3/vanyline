# Solution Design: Identité en base

## Résumé

La Solution `workspaces/identity` décrit les lignes d'identité en base d'`app` : une ligne `vanyline_projects` par Project et une ligne `vanyline_sandboxes` par Sandbox. Ces lignes ne portent que l'identité (clé, propriétaire, nom, première absence constatée) afin que les tables d'`app` qui s'y rattachent le fassent par clé étrangère réelle. La CR Kubernetes reste la seule source de vérité de `spec` et `status` (`workspaces#BR-2`) ; la base se réconcilie sur le cluster : une CR sans ligne est adoptée, une ligne dont la CR a disparu est supprimée après un délai de grâce (`workspaces#BR-4`, `workspaces#DEC-1`).

Statut : document rétro-documenté, écrit à partir des specs existantes. Les tables, entités et règles de nommage sont spécées ; le réconciliateur et les écritures de ligne depuis l'API ne le sont pas encore. Les contradictions entre specs sont en Open Issues.

## Problème et objectif

Les tables d'`app` qui concernent un espace de travail (workflows, runs, contextes de chat, lot ultérieur) doivent référencer un Project ou une Sandbox par clé étrangère, et non par nom en chaîne. L'objectif est de fournir cette clé de façon stable, sans dupliquer `spec` ni `status`, et sans que la base diverge durablement du cluster lorsque les objets sont créés hors interface (`workspaces#BO-2`). Source: `vanyline.sdd`, bloc `Must` « Modèle de données Project/Sandbox » ; `app/src/db/entities/projects.sdd`, Purpose.

## Périmètre et non-objectifs

Couvert : les tables `vanyline_projects` et `vanyline_sandboxes` et leurs contraintes ; le nommage technique des sandboxes ; l'écriture et le retrait des lignes lors de la création et de la suppression ; la réconciliation base vers cluster (adoption, marquage d'absence, suppression).

Hors périmètre : le contenu de `spec` et `status` des CR (`workspaces/projects`, `workspaces/sandboxes`) ; le lien utilisateur vers Owner et `ensure_owner` (`app/src/api/owners.sdd`), consommés seulement ; les routes CRUD hors écriture ou lecture d'identité ; les FK de `project_workflows`, `workflow_runs` et `chat_contexts` (lot ultérieur) ; la modification d'une sandbox (`PUT`).

## Contexte applicable du domaine

- `workspaces#BR-1` : un Owner correspond à un namespace `<app-ns>-<owner>` ; les CR Project et Sandbox y vivent, ce qui détermine où le réconciliateur les lit.
- `workspaces#BR-2` : la CR est la seule source de vérité de `spec` et `status` ; la base ne porte que l'identité.
- `workspaces#BR-3` : nom de sandbox `<projet>-<nom>`, immuable, refusé au-delà de 63 caractères. Cette Solution en réalise le calcul (FR-7).
- `workspaces#BR-4` : adoption d'une CR sans ligne si son Owner a un lien utilisateur connu ; suppression d'une ligne après `NotFound` confirmé et délai de grâce.
- `workspaces#DEC-1` : le cluster est la source de vérité, la base le suit.
- `workspaces#AC-2` et `workspaces#AC-3` : critères de haut niveau sur l'adoption, la suppression, le nommage et les collisions.

## Acteurs

- **Utilisateur :** crée un Project ou une Sandbox par l'interface (routes `app/src/api/projects.sdd`, `app/src/api/sandboxes.sdd`) ; **opérateur ou GitOps :** crée et supprime des CR hors interface (`workspaces#BO-2`).
- **Réconciliateur d'`app` :** compare les CR et les lignes, adopte, marque et supprime. Il est prescrit par `vanyline.sdd` mais n'a pas encore de spec (OI-9).

## État actuel et état cible

Actuel (constat sur les fichiers, aucun test exécuté) :
- Les migrations `m20261023_000001_create_vanyline_projects` et `m20261023_000002_create_vanyline_sandboxes`, ainsi que les entités `projects` et `sandboxes`, existent avec leurs specs. Leurs tâches d'implémentation sont ouvertes (`[ ]`).
- Aucune spec d'API n'écrit de ligne d'identité : `app/src/api/projects.sdd` et `app/src/api/sandboxes.sdd` ne citent ni `vanyline_projects` ni `vanyline_sandboxes` dans leurs effets de bord.
- `app/src/api/sandboxes.sdd` ne valide pas le nom avant le POST, et une collision `AlreadyExists` répond 500 ; `DEC-6` impose 409 et 422.
- Aucun réconciliateur n'a de spec dans `app/src/`.
- `app/src/ws/sandbox_state.sdd` observe les Sandbox d'un namespace d'application unique, modèle antérieur à « un Owner = un namespace » (OI-6).

Cible (`vanyline.sdd`, Tasks et Must) :
- création de la ligne avant la CR, suppression de la CR avant la ligne (FR-5, FR-6) ;
- réconciliateur fondé sur un watch et un resynchronisation complète (FR-1) ;
- marquage `missing_since` et suppression après 180 s (FR-2 à FR-4) ;
- FK des tables de workflow et de chat sur ces lignes (hors périmètre).

Statut des tâches `vanyline.sdd` : « Adoption et suppression par réconciliation » et « Délai de grâce 180 s » étaient cochées `[x]` (rouvertes le 2026-11-07), alors qu'aucun réconciliateur n'est spécé (OI-8).

## Hypothèses, contraintes et dépendances

- **CON-1:** Une ligne d'identité ne reflète jamais `spec` ni `status` : ni branche, ni toolchains, ni phase, ni dépôt.
    - **Source:** `app/src/db/entities/projects.sdd` (Must not) ; `app/src/db/entities/sandboxes.sdd` (Must not) ; `vanyline.sdd`, Must.
- **CON-2:** L'unicité est arbitrée par la base : `(owner_id, name)` pour les projets, `(project_id, name)` et `(owner_id, cr_name)` pour les sandboxes. La vérification applicative ne remplace pas ces contraintes.
    - **Source:** `app/src/migration/m20261023_000001_create_vanyline_projects.sdd` (Must) ; `app/src/migration/m20261023_000002_create_vanyline_sandboxes.sdd` (Must) ; `app/src/naming.sdd` (Must not).
- **CON-3:** Le réconciliateur n'accède au cluster que par le client `VnlK8sClient` de `vanyline-lib`. Ce client opère dans le namespace d'un Owner et n'offre aucun listing cluster-wide.
    - **Source:** `lib/src/k8s.sdd` (Must) ; `deploy/controller/controller.yaml` (RoleBinding `vanyline-app` par namespace d'Owner).
- **CON-4:** Les délais et la cadence sont des paramètres de configuration de l'app : délai de grâce 180 s, resynchronisation complète 60 s.
    - **Source:** `vanyline.sdd`, Must (Délai de grâce ; Cadence).
- **Dépendances :** `vanyline_owner_links` (lien `user_id` vers `k8s_owner_name`, `app/src/db/entities/owner_links.sdd`) ; `vanyline-crds` (`crds/src/lib.sdd`, `SandboxSpec` avec `project` et `branch` requis) ; `miryad_users` (table de `miryad-core`, cible des FK `owner_id`).

## Exigences fonctionnelles

- **FR-1 - MUST:** Le réconciliateur exécute un passage complet toutes les 60 s, et un passage déclenché par le watch des Project et des Sandbox.
    - **Source:** `vanyline.sdd`, Must (Cadence) ; `workspaces#OI-2` pour la valeur 60 s face à 30 s.
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Une CR Project ou Sandbox sans ligne est adoptée si son Owner a un lien utilisateur connu : la ligne reçoit `owner_id` égal à l'identifiant `miryad_users` de ce lien. Sans lien, la CR n'est pas adoptée et un signalement est émis (log et métrique).
    - **Source:** `workspaces#BR-4` (règle) ; `vanyline.sdd`, Must (signalement).
    - **Verified by:** `AC-1`

- **FR-3 - MUST:** Une ligne dont la CR n'apparaît plus dans un listing complet reçoit `missing_since` à la première constatation. Si la CR réapparaît, `missing_since` repasse à NULL et l'historique des lignes enfants reste intact.
    - **Source:** `vanyline.sdd`, Must (Garde-fous) ; `app/src/migration/m20261023_000001_create_vanyline_projects.sdd` (`missing_since`).
    - **Verified by:** `AC-2`

- **FR-4 - MUST:** Une ligne n'est supprimée que si le passage est à au moins `missing_since` + 180 s et que le GET de la CR renvoie `NotFound`. Une erreur ou un listing incomplet ne supprime rien et abandonne le passage, journalisé.
    - **Source:** `vanyline.sdd`, Must (Garde-fous ; Délai de grâce) ; `workspaces#BR-4`.
    - **Verified by:** `AC-2`

- **FR-5 - MUST:** La création d'une Sandbox insère sa ligne d'identité (`owner_id`, `project_id`, `name`, `cr_name`) avant le POST de la CR. Si le POST échoue, la ligne est retirée.
    - **Source:** `vanyline.sdd`, Must (Création).
    - **Realized by:** aucune à ce jour (OI-5).
    - **Verified by:** `AC-6`

- **FR-6 - MUST:** La suppression d'une Sandbox supprime la CR, puis la ligne. Les deux étapes sont idempotentes.
    - **Source:** `vanyline.sdd`, Must (Suppression).
    - **Verified by:** `AC-7`

- **FR-7 - MUST:** Le `metadata.name` d'une sandbox est `<projet>-<nom>`, validé en label DNS RFC 1123 puis limité à 63 caractères, car il sert d'hôte d'ingress. Un résultat trop long est refusé, jamais tronqué.
    - **Source:** `app/src/naming.sdd` (`sandbox_cr_name`) ; `vanyline.sdd`, Must.
    - **Verified by:** `AC-4`

- **FR-8 - MUST:** Le nom affiché d'une sandbox adoptée est son `cr_name` privé du préfixe `<projet>-` si le reste est non vide ; sinon le `cr_name` tel quel.
    - **Source:** `app/src/naming.sdd` (`display_name`).
    - **Verified by:** `AC-5`

- **FR-9 - MUST:** Les unicités `(owner_id, name)` pour les projets, `(project_id, name)` et `(owner_id, cr_name)` pour les sandboxes sont imposées par la base.
    - **Source:** migrations `m20261023_000001` et `m20261023_000002` (Must) ; `vanyline.sdd`, Must.
    - **Verified by:** `AC-3`

- **FR-10 - MUST:** Une violation d'unicité, en base ou par `AlreadyExists` du cluster, répond 409 avec un message qui nomme la cause, jamais 500.
    - **Source:** `vanyline.sdd`, Must.
    - **Statut :** décidé (`DEC-6`) ; les specs d'API actuelles répondent encore 500 et sont à réécrire.
    - **Verified by:** `AC-8`

## Sécurité

- La table d'identité ne contient ni jeton, ni valeur de `spec` ou de `status` (CON-1). Le jeton de l'Owner n'entre jamais dans ces lignes.
- Le réconciliateur n'a que les droits du ClusterRole `vanyline-app-owner-scope` dans chaque namespace d'Owner (`get`, `list`, `watch`, `create`, `delete`, `patch` sur `projects` et `sandboxes`). Ce RBAC n'autorise aucun listing cluster-wide, ce qui rend OI-1 bloquant.

## Conception

Relations entre tables :

```text
miryad_users (id)
  |-- vanyline_owner_links  (user_id UNIQUE, k8s_owner_name nullable)   lien vers l'Owner
  `-- vanyline_projects     (owner_id FK CASCADE, name, missing_since)
        `-- vanyline_sandboxes (project_id FK CASCADE, owner_id FK CASCADE,
                                name, cr_name, missing_since)
```

Champs et contraintes :
- `vanyline_projects` : `id` (clé), `owner_id` (FK `miryad_users.id`, `ON DELETE CASCADE`), `name` (`metadata.name` de la CR), `missing_since` (nullable), `created_at` (défaut base). Unicité `(owner_id, name)`.
- `vanyline_sandboxes` : `id`, `owner_id` (FK `miryad_users.id`, dénormalisé depuis le projet), `project_id` (FK `vanyline_projects.id`, `ON DELETE CASCADE`), `name` (nom affiché), `cr_name` (`metadata.name` de la CR), `missing_since`, `created_at`. Unicités `(project_id, name)` et `(owner_id, cr_name)`.
- `vanyline_owner_links` : `user_id` unique, `k8s_owner_name` nullable, sans contrainte d'unicité sur `k8s_owner_name` (OI-3).

Correspondance CR vers ligne :
- CR Project, dans le namespace `<app-ns>-<owner>` : `spec.owner` identifie l'Owner ; `metadata.name` devient `name`.
- CR Sandbox, dans le namespace de son Owner : `metadata.name` devient `cr_name` ; `name` vaut `display_name(project, cr_name)` ; `project_id` est la ligne du projet nommé par `spec.project` (lecture non spécée, OI-2).

Flux :
1. Création d'une Sandbox par l'API : calculer `cr_name` (FR-7) ; insérer la ligne ; POST de la CR ; si le POST échoue, supprimer la ligne. Une collision est arbitrée par la contrainte `(owner_id, cr_name)`.
2. Suppression d'une Sandbox par l'API : DELETE de la CR, puis DELETE de la ligne ; un second appel est sans effet.
3. Passage de réconciliation (watch ou 60 s) : (a) lister les CR visibles ; (b) pour une CR sans ligne, adopter si le lien existe, sinon signaler ; (c) pour une ligne absente du listing, poser `missing_since` si NULL ; (d) si `missing_since` + 180 s est dépassé, faire un GET de la CR et supprimer la ligne sur `NotFound` ; (e) pour une ligne dont la CR est revenue, remettre `missing_since` à NULL.

États d'une ligne :

```text
[presente] --CR absente--> [absente, missing_since = t]
    ^                          |
    |--- CR revenue -----------|   (missing_since = NULL)
                               |
                               `--- t + 180 s et GET NotFound ---> [supprimee]
```

Cas d'erreur :
- Erreur de GET ou de listing pendant la réconciliation : aucune suppression, passage abandonné et journalisé (FR-4).
- POST de CR en échec après insertion : ligne retirée (FR-5).
- Violation d'unicité : 409 attendu (FR-10), réponse actuelle 500, à corriger (`DEC-6`).
- Suppression d'une ligne `vanyline_projects` : cascade sur ses lignes `vanyline_sandboxes`, même si leurs CR existent encore (OI-2).

## Décisions

- **DEC-1 - Identité par clé étrangère, pas par nom en chaîne**
    - **Context:** Les tables de workflow et de chat doivent se rattacher à un Project ou une Sandbox sans dépendre d'une chaîne qui peut changer.
    - **Decision:** Chaque Project et chaque Sandbox a une ligne ; les tables qui s'y rattachent portent une FK vers cette ligne.
    - **Source:** `vanyline.sdd`, Must (Modèle de données Project/Sandbox).

- **DEC-2 - La base ne porte que l'identité**
    - **Context:** Une copie de `spec` ou de `status` en base créerait une seconde source de vérité.
    - **Decision:** Aucune colonne ne reflète `spec` ni `status` ; les champs sont uniquement la clé, le propriétaire, le nom, `missing_since` et `created_at`.
    - **Source:** `app/src/db/entities/projects.sdd` (Must not) ; `app/src/db/entities/sandboxes.sdd` (Must not) ; `workspaces#BR-2`.

- **DEC-3 - Le watch ne remplace pas la resynchronisation complète**
    - **Context:** Un watch peut manquer des événements ; l'adoption et la détection d'absence doivent rester correctes.
    - **Decision:** Un watch donne une réaction rapide ; un passage complet à 60 s rattrape tout ce qui a pu être manqué.
    - **Alternatives:** resynchronisation seule toutes les 30 s (moins de code, adoption plus lente), voir `workspaces#OI-2`.
    - **Source:** `vanyline.sdd`, Must (Cadence) ; `vanyline.sdd`, Tasks `[?]`.

- **DEC-4 - Le délai de grâce est stocké en base**
    - **Context:** Le délai doit rester correct avec plusieurs réplicas d'`app` et après un redémarrage.
    - **Decision:** `missing_since` est une colonne de la table d'identité, et non un état en mémoire.
    - **Source:** `vanyline.sdd`, Must (Délai de grâce 180 s).

- **DEC-5 - Le réconciliateur parcourt les Owners du namespace d'application**
    - **Context:** `lib/src/k8s.sdd` interdit tout listage cluster-wide. Le RBAC donne à l'app un Role dans son namespace d'application (CR `Owner` et `Application`) et un RoleBinding `vanyline-app` dans chaque namespace d'Owner (list et watch sur projects et sandboxes).
    - **Decision:** le réconciliateur liste les CR `Owner` du namespace d'application, puis liste les Projects et Sandboxes dans `<app-ns>-<owner>` (nommage de `vanyline-crds`) pour chacun. Il ne voit que les Owners qui ont le RoleBinding de délégation (`workspaces/owners#FR-12`). Un Owner sans lien utilisateur connu n'est adopté qu'une fois le lien établi (`workspaces#BR-4`).
    - **Alternatives:** parcourir les lignes `owner_links` (écartée : retarde l'adoption d'un Owner créé par GitOps) ; un ClusterRole pour l'app (écartée : contredit l'isolation par namespace d'Owner).
    - **Source:** décision du développeur, 2026-11-07 ; `deploy/controller/controller.yaml` ; SpecDD /lib/src/k8s.sdd

- **DEC-6 - Les codes 409 et 422 sont rendus par `app/src/error.rs` et une pré-validation**
    - **Context:** toute erreur kube non-NotFound sort aujourd'hui en 500 `VNL-K8S-002`, et le frontend affiche le message kube brut ; `workspaces#AC-3` exige 422 pour un nom trop long et 409 pour une collision.
    - **Decision:** `AlreadyExists` devient 409 et `Invalid` devient 422 dans `app/src/error.rs` ; le nom est validé (longueur, DNS-1123) avant tout appel au cluster, par le nommage de `vanyline-crds` ; l'unicité de la ligne d'identité arbitre aussi la collision.
    - **Consequences:** les Scenarios qui figent le 500 dans `app/src/api/projects.sdd` et `app/src/api/sandboxes.sdd`, et leurs tâches `[!]`, sont à réécrire.
    - **Source:** décision du développeur, 2026-11-07 ; `workspaces#BR-3`, `workspaces#AC-3`

## Réalisation SpecDD

- `SpecDD /app/src/db/entities/projects.sdd` : entité `vanyline_projects` (FR-1 à FR-4, FR-9, DEC-2).
- `SpecDD /app/src/db/entities/sandboxes.sdd` : entité `vanyline_sandboxes` (FR-5 à FR-9, DEC-2).
- `SpecDD /app/src/migration/m20261023_000001_create_vanyline_projects.sdd` : schéma de `vanyline_projects` (CON-2, FR-9).
- `SpecDD /app/src/migration/m20261023_000002_create_vanyline_sandboxes.sdd` : schéma de `vanyline_sandboxes` (CON-2, FR-9).
- `SpecDD /app/src/migration/m20260825_000010_create_vanyline_owner_links.sdd` et `SpecDD /app/src/db/entities/owner_links.sdd` : lien `user_id` vers `k8s_owner_name` (FR-2, OI-3).
- `SpecDD /app/src/naming.sdd` : `sandbox_cr_name` et `display_name` (FR-7, FR-8).
- `SpecDD /app/src/k8s.sdd` : client mis en cache pour les appels de réconciliation (CON-3).
- `SpecDD /lib/src/k8s.sdd` : opérations sur Project et Sandbox dans le namespace d'un Owner (CON-3).
- `SpecDD /app/src/ws/sandbox_state.sdd` : watch des Sandbox, à réaligner (FR-1, OI-6).
- Réconciliateur : aucune spec à ce jour ; emplacement à fixer (OI-9).

## Acceptation et vérification

- **AC-1:** Une CR Project ou Sandbox créée hors interface, dont l'Owner a un lien connu, reçoit une ligne au plus tard au passage suivant ; sans lien, aucune ligne n'est créée et un signalement est émis. Vérification : TBD, aucun Scenario ni réconciliateur spécé (OI-9). Référence : `workspaces#AC-2`.
- **AC-2:** Une ligne dont la CR a disparu reçoit `missing_since` ; elle n'est supprimée qu'après 180 s et un `NotFound` confirmé ; une réapparition remet `missing_since` à NULL ; un listing incomplet ne supprime rien. Vérification : TBD (OI-9). Référence : `workspaces#AC-2`.
- **AC-3:** Les unicités sont imposées par la base. Scenarios existants : `unicite_owner_et_nom` (migration projets), `nom_unique_dans_un_projet` et `cr_name_unique_par_proprietaire` (migration sandboxes), `meme_nom_affiche_dans_deux_projets` et `collision_de_cr_name_rejetee` (entité sandboxes).
- **AC-4:** Un nom `<projet>-<nom>` de plus de 63 caractères est refusé sans troncature. Scenarios : `longueur_maximale` et `nom_invalide_refuse` (`app/src/naming.sdd`). Le code HTTP 422 n'est pas couvert (OI-4). Référence : `workspaces#AC-3`.
- **AC-5:** Le nom affiché d'une CR adoptée retire le préfixe du projet. Scenario : `nom_affiche_d_une_cr_adoptee` (`app/src/naming.sdd`).
- **AC-6:** La création d'une Sandbox insère la ligne avant la CR, et la retire si la CR échoue. Vérification : TBD (OI-5).
- **AC-7:** La suppression d'une Sandbox supprime la CR puis la ligne, et un second appel est sans effet. Vérification : TBD (OI-5).
- **AC-8:** Une collision de nom répond 409 avec la cause. Vérification : à écrire avec `DEC-6` ; contredit par les specs d'API actuelles. Référence : `workspaces#AC-3`.

## Open Issues

- **OI-2 - Owner et projet d'une Sandbox:** `SandboxSpec` n'a pas de champ `owner`. L'Owner d'une sandbox adoptée n'est pas défini, ni la lecture de `project_id` à partir de `spec.project`. La cascade `project` vers `sandbox` peut supprimer une ligne sandbox dont la CR existe encore. **TBD.**
- **OI-3 - Lien inverse Owner vers utilisateur:** `vanyline.sdd` parle d'un « lien utilisateur connu », mais aucune spec ne définit la recherche par `k8s_owner_name`, qui n'est pas unique en base (`app/src/db/entities/owner_links.sdd`). **TBD.**
- **OI-5 - Ordre de création et portée:** la règle « ligne avant CR, retrait si échec » figure dans `vanyline.sdd` mais ni `app/src/api/sandboxes.sdd` ni `app/src/api/projects.sdd` ne l'appliquent. Il n'est pas dit si elle vaut aussi pour les Project. **TBD.**
- **OI-6 - Watch sur un namespace unique:** `app/src/ws/sandbox_state.sdd` décrit un watch de « tous les Sandbox du namespace du client », sans paramètre, alors que `lib/src/k8s.sdd` décrit un watch par Owner, jamais cluster-wide. `app/src/api/projects.sdd` parle encore d'un namespace unique d'application. **TBD.**
- **OI-7 - Cadence 60 s ou 30 s:** `vanyline.sdd` retient 60 s à titre provisoire, avec une alternative à 30 s marquée `[?]` ; voir `workspaces#OI-2`. **TBD.**
- **OI-9 - Réconciliateur non spécé:** emplacement de la spec (`app/src/identity.sdd`, à créer ; tâche ajoutée à `vanyline.sdd` le 2026-11-07), comportement en cas d'erreur de base pendant un passage, nom de la métrique de signalement. **TBD.**
- **OI-10 - Modèle antérieur dans les API:** `app/src/api/projects.sdd` et `app/src/api/sandboxes.sdd` contiennent encore un namespace unique et des Scenarios à reprendre (Tasks « Un Owner = un namespace », `[ ]`). **TBD.**

## Références

- `workspaces` : `BR-1`, `BR-2`, `BR-3`, `BR-4`, `BO-2`, `DEC-1`, `AC-2`, `AC-3`, `OI-2`.
- `vanyline.sdd` : bloc `Must` « Modèle de données Project/Sandbox » ; Tasks « Identité Project/Sandbox en base », « Adoption et suppression », « Délai de grâce 180 s ».
- `app/src/db/entities/projects.sdd`, `app/src/db/entities/sandboxes.sdd`, `app/src/db/entities/owner_links.sdd`.
- `app/src/migration/m20261023_000001_create_vanyline_projects.sdd`, `app/src/migration/m20261023_000002_create_vanyline_sandboxes.sdd`, `app/src/migration/m20260825_000010_create_vanyline_owner_links.sdd`.
- `app/src/naming.sdd`, `app/src/k8s.sdd`, `lib/src/k8s.sdd`, `app/src/api/owners.sdd`, `app/src/api/projects.sdd`, `app/src/api/sandboxes.sdd`, `app/src/ws/sandbox_state.sdd`, `crds/src/lib.sdd`, `deploy/controller/controller.yaml` (RBAC).
