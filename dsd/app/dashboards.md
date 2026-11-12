# Solution Design: Tableaux de bord et widgets standard

## Résumé

La Solution `app/dashboards` fait du tableau de bord le modèle de page de l'application (`app#DEC-5`) : une définition
déclarative (une grille de widgets) résolue pour un écran (accueil, projet, sandbox), livrée par défaut avec l'application,
personnalisable par l'utilisateur, complétée de tableaux complémentaires par projet, et présentable de façon éphémère par le
LLM. Elle fixe le modèle de données, la résolution d'un tableau pour un écran, le registre des types de widget, les widgets
standard (projets, sandboxes, workflows, runs, santé du code, chat, liste de tableaux, contenu markdown et diagrammes) et leur
rendu. Les widgets **scripté** (exécution, sécurité) relèvent de `app/widgets` ; la **présentation** par le LLM relève de
`app/ui-actions` ; cette Solution ne fournit que le modèle et les points d'accueil.

Elle réalise `app#BO-4`, `app#BR-5`, `app#BR-6`, `app#FR-2`, `app#FR-3`, `app#DEC-5` et `app#DEC-8`.

## Problème et objectif

Les écrans actuels (`frontend/src/components/dashboards/HomeDashboard.vue`, `ProjectDashboard.vue`, la vue d'ensemble de
sandbox) sont des **sections de page écrites à la main** : rien n'est personnalisable, aucun widget n'est déclaré, et le
LLM ne peut pas composer une page. Objectif : un seul moteur de rendu piloté par une définition, pour que (1) un projet puisse
personnaliser ses écrans, (2) un projet possède des tableaux complémentaires, (3) des scripts alimentent des widgets, (4) le LLM
puisse montrer un tableau éphémère, sans jamais lui faire exécuter de code dans l'origine de l'application (`app#BR-5`).

## Périmètre et non-objectifs

Couvert :

- le modèle d'un tableau et d'un widget, ses bornes, sa validation ;
- la résolution d'un tableau pour un écran : défaut, personnalisation, complémentaires ;
- le registre des types de widget et le contrat d'un type (configuration, données, rafraîchissement) ;
- les widgets standard listés en résumé et le rendu responsive ;
- le point d'accueil du tableau éphémère : sa forme, pas son déclenchement ;
- la reprise des écrans actuels (accueil, projet, vue de sandbox) sur ce moteur.

Hors périmètre :

- l'exécution et la sécurité d'un widget scripté : `app/widgets` ;
- la navigation et la présentation pilotées par le LLM, et l'événement `Host` qui porte un tableau éphémère : `app/ui-actions` et
  `app/sessions#FR-8` ;
- l'IDE et la vue du diff, hors modèle (`app#DEC-5`) ;
- l'éditeur visuel de tableaux : une première version édite la définition en texte (`DEC-6`) ;
- le partage d'un tableau entre utilisateurs (`DEC-4`).

## Contexte applicable du domaine

- `app#BR-4` : un widget scripté s'exécute sous l'identité de l'utilisateur qui l'affiche.
- `app#BR-5` : un contenu produit par un LLM n'est jamais du code de l'application ; un tableau éphémère est déclaratif.
- `app#BR-6` : tout écran hors IDE et diff est un tableau ; projet et sandbox ont un tableau par défaut, que le projet peut
  personnaliser.
- `app#BR-7` : l'utilisateur accède à chaque écran par lui-même.
- `app#CON-3` : écrans responsives, sauf l'IDE et le diff.
- `app#DEC-8` : les définitions vivent en base de l'application ; une session productive ne les modifie pas (`app#BR-3`).

## Acteurs

- **Utilisateur :** consulte, personnalise et crée ses tableaux.
- **Agent de configuration :** peut écrire les définitions de l'utilisateur (même droit que lui, `DEC-3`).
- **Agent productif :** peut présenter un tableau éphémère, ne modifie aucune définition.
- **Administrateur :** aucune prérogative propre sur les tableaux d'un utilisateur.

## État actuel et état cible

- **Actuel (Observed in : `frontend/src/components/dashboards/`, `frontend/src/router.ts`) :** `HomeDashboard.vue` (543 lignes) et
  `ProjectDashboard.vue` (816 lignes) sont écrits à la main ; routes `/`, `/p/:projectName` et
  `/p/:projectName/s/:sandboxName/overview` ; les phases de sandbox arrivent par `/api/ws/sandbox-state`, les statuts de runs par
  interrogation (10 s tant qu'un run est en cours) ; états vides, message dédié « Moteur de workflow indisponible » (503), étape
  « clone en cours » sont des comportements à conserver.
- **Cible :** un composant unique rend la définition résolue ; les trois écrans actuels deviennent des définitions par défaut
  composées de widgets standard ; le code des écrans écrits à la main est retiré à parité (`DEC-8`).

## Hypothèses, contraintes et dépendances

- **CON-1:** Une définition est une donnée déclarative, sérialisable en JSON, bornée (`FR-3`) ; elle ne contient ni code ni
  balisage interprété. Un script de widget scripté en est un champ opaque de plus, traité par `app/widgets`.
    - **Source:** `app#BR-5`
- **CON-2:** Les définitions sont des ressources de l'application, en base, sous le droit de leur propriétaire (`DEC-3`).
    - **Source:** `app#DEC-8` ; `miryad-core#DEC-14`
- **CON-3:** L'application est mono-réplique pour les sessions (`app/sessions#DEC-1`) ; ce document n'en dépend pas, hors
  tableau éphémère, porté par un événement non rejoué (`app/sessions#CON-4`).
- **CON-4:** Les phases de sandbox arrivent par le WebSocket `/api/ws/sandbox-state` ; ce flux reste la source des widgets
  « sandboxes » tant que `app/sessions` et lui coexistent.
    - **Source:** `app/src/ws/sandbox_state.rs`

## Exigences fonctionnelles

- **FR-1 - MUST:** Un tableau est `{id, scope, kind, project?, title, layout, widgets[]}` : `scope` ∈ `home`, `project`, `sandbox` ;
  `kind` ∈ `default`, `custom`, `complementary`, `ephemeral` ; un widget est `{id, type, config, x, y, w, h}` sur une grille de
  12 colonnes ; `config` est un objet JSON validé par le type (`FR-4`).
    - **Source:** `app#Glossaire` (tableau de bord, widget)
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Pour un écran donné (`scope`, projet éventuel, sandbox éventuelle), l'application résout le **tableau effectif** :
  la personnalisation de l'utilisateur pour cet écran si elle existe, sinon le tableau par défaut livré avec l'application. Rendre
  les défauts à l'état d'origine supprime la personnalisation.
    - **Source:** `app#BR-6`
    - **Verified by:** `AC-2`, `AC-3`

- **FR-3 - MUST:** Une définition est bornée et validée à l'écriture : au plus 24 widgets, au plus 64 Kio sérialisée, types
  connus du registre, `x`, `y`, `w`, `h` entiers dans la grille, identifiants de widget uniques dans le tableau. Une définition
  invalide est refusée avec `VNL-APP-DSH-*`, jamais corrigée en silence.
    - **Source:** `app#BR-5` ; ASSUMPTION : bornes à ajuster à l'usage
    - **Verified by:** `AC-4`

- **FR-4 - MUST:** Un type de widget est déclaré au registre par son nom, un schéma de `config`, la liste de ses besoins de
  contexte (projet, sandbox) et sa source de données. Un type inconnu au rendu (définition plus récente que l'application) est
  affiché comme un emplacement d'erreur lisible, sans casser les autres widgets.
    - **Source:** DEC-1
    - **Verified by:** `AC-5`

- **FR-5 - MUST:** Un projet possède des tableaux complémentaires (`kind: complementary`, `scope: project`), listés et ouverts par
  le widget standard `dashboards` ; l'utilisateur en crée, renomme et supprime.
    - **Source:** `app#FR-2`
    - **Verified by:** `AC-6`

- **FR-6 - MUST:** Les widgets standard sont au moins : `projects` (état de clonage, nombre de sandboxes, création),
  `sandboxes` (phase en direct, création, suspension, ouverture de l'IDE et de la vue d'ensemble), `workflows` (liste, portée,
  création, édition, suppression, lancement), `runs` (14 derniers jours, 50 au plus, statut), `code-health` (santé du code d'une
  sandbox), `chat` (session du contexte, `app/sessions`), `dashboards` (liste des tableaux complémentaires), `markdown`,
  `bar`, `pie`. Le type `scripted` est réservé au registre, implémenté par `app/widgets`.
    - **Source:** `app#FR-3`, `app#FR-4`
    - **Verified by:** `AC-7`

- **FR-7 - MUST:** Les comportements des écrans actuels sont conservés par les widgets standard : états vides explicites, message
  dédié quand le moteur de workflow est indisponible (503), étape « clone en cours » tant que le projet n'est pas cloné (bouton de
  création de sandbox désactivé), lancement d'un workflow de sandbox seulement avec une sandbox `Running`, ouverture de l'IDE
  désactivée avec explication sous 1024 px.
    - **Source:** `frontend/src/components/dashboards/dashboards.sdd`, `Must`
    - **Verified by:** `AC-7`

- **FR-8 - MUST:** Le rendu est responsive : en une colonne sous 768 px (widgets empilés dans l'ordre `y`, `x`), en grille à partir de
  768 px ; la page ne défile jamais, le défilement est dans les widgets (`frontend/src/layout.sdd`).
    - **Source:** `app#CON-3`
    - **Verified by:** `AC-8`

- **FR-9 - MUST:** Un tableau éphémère (`kind: ephemeral`) a la même forme, n'est jamais stocké, n'a ni identifiant persistant ni
  route propre, et est rendu par le moteur ordinaire dans un emplacement fourni par `app/ui-actions`. Les mêmes bornes
  s'appliquent (`FR-3`) à sa réception.
    - **Source:** `app#BR-5`, `app#FR-7`
    - **Verified by:** `AC-9`

- **FR-10 - MUST:** Le rafraîchissement d'un widget suit sa source : phases de sandbox par le flux d'états, statuts de runs par
  interrogation (10 s tant qu'un run est actif, sinon aucune), le reste à l'ouverture et sur action de l'utilisateur ; aucun widget
  ne ré-interroge de façon indépendante les données déjà portées par un autre widget du même tableau.
    - **Source:** `frontend/src/components/dashboards/dashboards.sdd`, `Must`
    - **Verified by:** `AC-7`

- **FR-11 - MUST:** Les erreurs propres à cette Solution portent un identifiant `VNL-APP-DSH-NNN` (définition invalide, type
  inconnu, tableau introuvable, écran sans défaut).
    - **Source:** convention du projet
    - **Verified by:** `AC-4`

## Sécurité

- **Contenu.** `config` est une donnée validée : le type `markdown` rend du markdown sans HTML brut ; aucun type n'accepte du
  HTML, un script ou une URL de feuille de style. Un widget `scripted` n'exécute rien ici (`app/widgets`).
- **Droits.** Un widget n'élargit jamais les droits de l'utilisateur : ses données viennent des API de l'application sous la session
  de l'utilisateur ; une sandbox ou un projet hors de portée rend une erreur de widget, pas des données.
- **Écriture.** Une session productive ne modifie pas une définition (`app#BR-3`) ; l'agent de configuration le peut, avec le droit
  de l'utilisateur et la validation de `FR-3`.

## Décisions

- **DEC-1 - Un registre de types, un moteur de rendu**
    - **Decision:** tous les widgets, standard comme scripté, passent par un registre `type → {schéma, besoins, source,
      composant}` ; le moteur place les widgets, ne connaît rien de leur contenu.
    - **Alternatives:** un composant par écran (état actuel, écarté) ; des widgets libres en HTML (écarté, `app#BR-5`).
    - **Source:** `app#DEC-5`

- **DEC-2 - Les défauts sont livrés avec l'application, pas en base**
    - **Decision:** les tableaux par défaut (accueil, projet, sandbox) sont des définitions versionnées dans le code d'`app`
      (`app/src/dashboards/defaults.rs`) ; la base ne porte que les personnalisations et les tableaux complémentaires.
      Rendre les défauts supprime la ligne de personnalisation.
    - **Alternatives:** défauts insérés en base au démarrage (écarté : une montée de version ne pourrait pas faire évoluer un
      défaut non personnalisé sans écraser les personnalisations).
    - **Consequences:** `app#DEC-8` (« en base ») est respecté pour tout ce que l'utilisateur écrit.
    - **Source:** ASSUMPTION levée par l'instruction du 2026-11-12

- **DEC-3 - Les définitions sont des `MiryadResource` d'un propriétaire**
    - **Decision:** une ressource `dashboards` de miryad-core (REST, MCP, OpenAPI, module `resources` de lymaille), lecture et
      écriture `OwnerOnly` ; la définition est stockée en colonnes de clé (`scope`, `kind`, `project`) plus une colonne JSON
      (`miryad-core#DEC-17`). L'agent de configuration l'écrit par le module `resources` sous le droit de l'utilisateur, sans
      route propre.
    - **Consequences:** aucune API CRUD maison ; seule la **résolution** (`FR-2`) est une route de `app`.
    - **Source:** `miryad-core#DEC-14`, `DEC-17`, `DEC-21`

- **DEC-4 - Les tableaux sont propres à l'utilisateur, sans partage**
    - **Decision:** un tableau appartient à l'utilisateur qui l'écrit ; la personnalisation « du projet » est celle de chaque
      utilisateur pour ce projet. Pas de partage entre utilisateurs liés au même Owner.
    - **Alternatives:** tableaux attachés au projet et partagés par ses utilisateurs (écarté : `MiryadResource` n'a pas
      d'appartenance par projet ; usage actuel mono-utilisateur).
    - **Consequences:** rouvrable avec un besoin d'équipe ; sans effet sur le modèle (`project` est déjà une clé).
    - **Source:** ASSUMPTION levée par l'instruction du 2026-11-12

- **DEC-5 - Une seule route de résolution**
    - **Decision:** `GET /api/dashboards/resolve?scope=&project=&sandbox=` rend le tableau effectif et son origine (`default` ou
      `custom`) ; le reste passe par l'API générique des ressources. Forme exacte des chemins : ASSUMPTION.
    - **Source:** `FR-2`

- **DEC-6 - Édition en texte d'abord**
    - **Decision:** la première version offre l'édition de la définition JSON (formulaire minimal : titre, liste de widgets,
      champ de configuration en texte) et l'édition par l'agent de configuration ; le déplacement visuel des widgets est une
      amélioration ultérieure.
    - **Source:** ASSUMPTION levée par l'instruction du 2026-11-12 (avancer, ajuster ensuite)

- **DEC-7 - Les écrans actuels sont reconstruits, non adaptés**
    - **Decision:** `HomeDashboard.vue`, `ProjectDashboard.vue` et la vue d'ensemble de sandbox sont remplacés par le moteur et les
      définitions par défaut ; leurs specs et leurs tests sont réécrits sur les Scenario de cette Solution (`app#DEC-6`
      amendé par `app/sessions#DEC-8`).
    - **Source:** user instruction, 2026-11-12

## Réalisation SpecDD

Specs à créer ou à amender (une spec par fichier) :

- `app/src/dashboards/{mod,model,registry,defaults,resolve}.sdd` : `FR-1` à `FR-4`, `FR-11`, `DEC-2`, `DEC-5` ; entité
  `app/src/db/entities/dashboards.sdd` et migration `app/src/migration/m<date>_create_vanyline_dashboards.sdd` (`DEC-3`).
- `packages/protocol` : types `dashboard-domain` (générés par ts-rs depuis `model.rs`).
- Frontend : `frontend/src/components/dashboards/` (moteur de rendu, hôte de widget, un composant par widget standard, tests) ;
  amender `frontend/src/frontend.sdd`, `router.ts` (route des tableaux complémentaires) ; retirer les specs des écrans écrits à
  la main (`dashboards.sdd`, `workflows/`, `ContextChat.sdd`).
- `app/src/main.sdd` : montage de la ressource et de la route de résolution.

## Acceptation et vérification

- **AC-1:** Un tableau valide s'écrit, se relit et se sérialise de façon stable ; la forme est exportée en TypeScript et la CI échoue
  si elle diverge.
- **AC-2:** Sans personnalisation, `resolve` rend le défaut de l'écran avec l'origine `default` ; avec elle, la personnalisation avec
  l'origine `custom`.
- **AC-3:** Rendre les défauts supprime la personnalisation et la résolution revient au défaut.
- **AC-4:** Une définition de plus de 24 widgets, de plus de 64 Kio, de type inconnu, à coordonnées hors grille ou à identifiants
  dupliqués est refusée avec `VNL-APP-DSH-*`.
- **AC-5:** Un type inconnu au rendu produit un emplacement d'erreur sans empêcher le rendu des autres widgets.
- **AC-6:** Un projet liste, ouvre, crée, renomme et supprime ses tableaux complémentaires ; ceux d'un autre utilisateur ne sont pas
  visibles.
- **AC-7:** Les trois tableaux par défaut reproduisent les écrans actuels (états vides, 503 du moteur de workflow, clone en cours,
  lancement conditionné, phases en direct, runs interrogés à 10 s).
- **AC-8:** Sous 768 px les widgets s'empilent ; à partir de 768 px ils se placent sur la grille ; la page ne défile pas.
- **AC-9:** Un tableau éphémère reçu s'affiche, n'est écrit nulle part et disparaît au rechargement ; un tableau éphémère hors
  bornes est refusé.

## Open Issues

Aucun Open Issue bloquant. Reste à vérifier à l'implémentation, sans décision à prendre :

- **OI-1 - Place du widget `chat` face à la colonne de conversation** (`app#FR-13`) : le chat est-il un widget placé dans la grille,
  ou la colonne du shell, ou les deux selon l'écran ? À établir avec `app/sessions` et la Solution du shell ; **TBD** dans les specs
  frontend concernées.
- **OI-2 - Bornes de `FR-3`** (24 widgets, 64 Kio) : valeurs de départ, à ajuster à l'usage.

## Références

- `dsd/app.md` : `BO-4`, `BR-4` à `BR-7`, `FR-2`, `FR-3`, `FR-4`, `FR-7`, `FR-13`, `DEC-5`, `DEC-8`, `CON-3`.
- `dsd/app/sessions.md` : `FR-8`, `FR-9`, `CON-4`, `DEC-1`, `DEC-4`.
- Dépôt voisin `miryad-core` : `dsd/miryad-core/lymaille.md` (`DEC-14`, `DEC-17`, `DEC-21`).
- `frontend/src/components/dashboards/`, `frontend/src/router.ts`, `app/src/ws/sandbox_state.rs`.
