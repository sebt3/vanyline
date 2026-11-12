# Domain Definition: Application vanyline (conversation, contextes et tableaux de bord)

## Résumé

Le domaine `app` couvre ce que l'utilisateur de vanyline voit et fait : choisir un **contexte de travail**, y converser
avec un LLM qui agit pour lui, relire le résultat (typiquement une MR sur la forge), et suivre l'état de ses projets,
sandboxes et workflows dans des **tableaux de bord** composés de widgets. Il couvre aussi l'outillage de la
**planification** : groupes d'artefacts consultables, et la méthode SpecDD + DSD comme pratique de première classe.

Statut du document : **ébauche** (2026-11-03). Les domaines voisins sont `sandbox` (qui inclut la santé du code),
`workspaces` (Owner, Project, Sandbox, Application, controller) et `cli`. Aucune Solution n'est rédigée : le paysage des
Solutions n'est qu'une proposition de découpage. Les points non tranchés sont des `TBD` ou des Open Issues.

## Objectif et problème

Le développeur principal résume son travail à discuter avec un LLM et à relire des MRs dans une forge. L'interaction
avec le LLM est donc le cœur de l'expérience. Or le tooling LLM actuel de vanyline (agents système, configuration LLM
maison, `ChatEvent`) ne répond pas à la base de harnais voulue, et l'interface ne permet ni de choisir un contexte
plutôt qu'un agent, ni de faire montrer des informations par le LLM, ni de planifier avec des artefacts.

## Objectifs métier

- **BO-1:** L'interaction avec un LLM est le cœur de l'expérience : l'utilisateur dit ce dont il a besoin (« une MR qui
  fait X, Y, Z »), relit le travail dans l'IDE web et le diff de la sandbox avant de demander la MR, puis relit la MR
  sur la forge.
    - **Source:** user instruction, 2026-11-03

- **BO-2:** L'utilisateur choisit un contexte de travail, jamais un agent parmi plusieurs dizaines ; l'agent et les
  limites en découlent.
    - **Source:** user instruction, 2026-11-03

- **BO-3:** La planification d'un projet par la conversation est outillée : plan et maquettes se consultent comme des
  artefacts, et la méthode SpecDD + DSD est intégrée à la plateforme.
    - **Source:** user instruction, 2026-11-03

- **BO-4:** Chaque écran, hors IDE et vue du diff, est un tableau de bord de widgets que le projet peut personnaliser, que des
  scripts peuvent alimenter et que le LLM peut présenter à l'utilisateur.
    - **Source:** user instruction, 2026-11-03

## Périmètre et non-objectifs

Le domaine couvre : les contextes de travail et les sessions de conversation qui en découlent, les tableaux de bord et
leurs widgets, les actions d'interface pilotées par le LLM, les artefacts de planification, l'IDE web et la vue du diff
de relecture, les écrans de configuration et l'hôte applicatif du harnais LLM (`app`, `frontend`, `packages/*`).

Hors périmètre, par décision :

- la conception des sessions, scratchpad, toolsets, délégation et inférence : domaine `agent-sessions` du dépôt
  `lymaille`, référencé et jamais copié (`DEC-1`) ;
- l'hébergement durable de ce domaine par miryad-core (feature `lymaille`), décrit dans `dsd/miryad-core/lymaille.md` de
  ce dépôt-là ;
- la sandbox (santé du code comprise) et son modèle de menace : domaine `sandbox` ; le controller, les CRDs et
  l'identité Project/Sandbox en base : domaine `workspaces` ; la CLI : domaine `cli` ;
- le moteur de workflow lui-même : fonctionnalité de miryad-core, que vanyline enrichit (voir Solution
  `app/workflows`) ;
- la revue de code et la protection des branches : elles relèvent de la forge (`DEC-4`) ;
- l'outillage SpecDD dans la boîte à outils JS de la sandbox : traité par `sandbox#FR-6` (voir `OI-6`).

Source: user instruction, 2026-11-03

## Acteurs

- **Utilisateur :** choisit un contexte, converse, relit sur la forge, consulte et personnalise ses tableaux de bord.
- **Agent productif :** LLM d'une session de nature `Work` ; agit sur le code par la sandbox, montre des pages et des
  tableaux de bord à l'utilisateur.
- **Agent de configuration :** LLM d'une session de nature `Config` ; seul à pouvoir remonter une définition vers la
  configuration (`BR-3`).
- **Administrateur :** publie des définitions partagées, administre les réglages globaux.
- **Forge :** système externe (Gitea, GitHub, GitLab) où l'utilisateur relit les MRs ; vanyline n'en est pas la source
  de vérité. Son serveur MCP, une fois configuré, est un module comme un autre (`DEC-7`).
- **Sandbox :** pod de travail de l'Owner ; fournit au LLM le code, les outils et les données des widgets scriptés.

## État actuel et état cible

- **Actuel (constat, Observed in: `app/src/`, `frontend/src/components/dashboards/`) :** `app` porte l'auth OIDC, la
  configuration LLM et les agents (`app/src/system_agents.rs`, `app/src/ws/chat.rs`), les workflows et un MCP de domaine
  (`app/src/mcp_domain.rs`). Le frontend a un tableau de bord d'accueil et un de projet, une vue d'ensemble de sandbox
  et des chats contextuels (`ContextChat`) ; ces écrans sont des **sections de page**, pas encore un modèle de widgets
  déclaré. Le chat passe par WebSocket et le type `ChatEvent`.
- **Cible :** les contextes de travail ouvrent des sessions lymaille hébergées dans `app` ; tous les écrans hors IDE
  sont des tableaux de bord de widgets ; les artefacts de planification existent ; l'ancien tooling LLM est retiré à
  parité (`DEC-6`).

## Hypothèses, contraintes et dépendances

- **CON-1:** Le noyau de session, de scratchpad et de toolsets est celui de lymaille (publiée sur crates.io), jamais
  réimplémenté dans vanyline. L'intégration de lymaille est faite dans miryad-core : vanyline en consomme l'hébergement
  (stockage, sessions, flux d'événements, droits) et ses choix s'imposent à `app` tant qu'ils ne sont pas amendés
  (`OI-1`).
    - **Source:** DEC-1 ; user instruction, 2026-11-03

- **CON-2:** Le navigateur ne voit jamais le jeton de l'Owner ni le JWT OIDC brut ; l'HTTP vers la sandbox passe par
  des relais de `app`, les WebSocket éditeur par ticket court-vécu.
    - **Source:** `vanyline.sdd`, section `Must not` (décision du 2026-10-04)

- **CON-3:** Les écrans sont responsives, sauf l'IDE et la vue du diff, qui exigent un écran d'ordinateur (au moins
  1024 px de large pour l'IDE ; même seuil pour le diff, ASSUMPTION : à confirmer).
    - **Source:** `AGENTS.md`, composant frontend ; user instruction, 2026-11-03 (vue du diff)

## Glossaire

Le mot « contexte » seul désigne ici le **contexte de travail**. Le *ToolsetContext* et la *fenêtre de contexte* de
lymaille s'écrivent toujours en entier.

- **Contexte de travail :** point d'où l'utilisateur ouvre le chat (accueil, projet, sandbox, configuration). Il
  détermine la nature de la session, l'agent principal par défaut, les modules et serveurs MCP accordés. Il est fixé à
  l'ouverture de la session.
- **Session :** conversation durable au sens de lymaille, rattachée à un utilisateur et à un contexte de travail.
- **Tableau de bord :** page composée de widgets disposés ; peut être par défaut, personnalisé, complémentaire ou
  éphémère.
- **Widget :** élément d'un tableau de bord ; porte un type et sa source de données.
- **Widget standard :** widget dont la donnée vient de l'application (projets, sandboxes, workflows, runs, santé du
  code, chat).
- **Widget scripté :** widget dont la donnée est produite par un script exécuté sous l'identité de l'utilisateur qui
  l'affiche, avec les modules du contexte.
- **Tableau de bord éphémère :** tableau créé à la volée par le LLM pour enrichir la conversation ; il n'est pas
  conservé.
- **Action d'interface :** demande du LLM adressée à l'interface (aller à une page, ouvrir un tableau de bord).
- **Artefact :** page HTML de planification (maquette, plan). **Groupe d'artefacts :** ensemble d'artefacts rattaché à
  un projet.
- **Agent productif / agent de configuration :** les natures `Work` et `Config` de lymaille
  (`agent-sessions`, glossaire).

## Règles du domaine

- **BR-1 - MUST:** Une session s'ouvre depuis un contexte de travail ; la nature de la session et l'agent principal par
  défaut en découlent, et l'utilisateur n'a pas à choisir un agent pour converser.
    - **Source:** BO-2
    - **Verified by:** `AC-1`

- **BR-2 - MUST:** Les limites de l'agent sont posées avant la session, par le contexte ; aucune approbation humaine
  n'est demandée en cours de session.
    - **Source:** user instruction, 2026-11-03 ; agent-sessions#DEC-2
    - **Verified by:** `AC-1`

- **BR-3 - MUST:** Une session productive peut modifier les fichiers du workspace (`.skills/`, `AGENTS.md`) mais ne
  modifie la configuration que par une session de configuration ; la promotion d'une définition du workspace vers la
  configuration est une action humaine hors session, et les droits d'un agent ne viennent jamais du contenu du
  workspace.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-2`

- **BR-4 - MUST:** Un widget scripté s'exécute avec l'identité de l'utilisateur qui l'affiche et les modules accordés
  par le contexte ; le contenu du script n'étend jamais ces droits.
    - **Source:** BR-3 ; user instruction, 2026-11-03 (entérinée)
    - **Verified by:** `AC-3`

- **BR-5 - MUST:** Un contenu produit par un LLM (artefact HTML, description d'un tableau de bord éphémère) n'est jamais
  interprété comme code de l'application : un artefact s'affiche isolé de l'origine de l'application, un tableau
  éphémère est une description déclarative (types de widgets et données).
    - **Source:** user instruction, 2026-11-03 (entérinée)
    - **Verified by:** `AC-4`

- **BR-6 - MUST:** Tout écran hors IDE et vue du diff est un tableau de bord ; le projet et la sandbox ont chacun un tableau par
  défaut que le projet peut personnaliser.
    - **Source:** BO-4
    - **Verified by:** `AC-5`

- **BR-7 - MUST:** L'utilisateur accède à chaque écran et à chaque action de l'interface par lui-même ; l'agent n'est
  jamais le seul chemin vers une page ou une fonction.
    - **Source:** user instruction, 2026-11-03 (« tombe sous le sens »)
    - **Verified by:** `AC-10`

## Exigences fonctionnelles transverses

- **FR-1 - MUST:** L'utilisateur converse avec un LLM depuis chaque écran de contexte (accueil, projet, sandbox,
  configuration).
    - **Source:** BO-1, `AGENTS.md` (chaque écran offre un chat)
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Un projet peut posséder des tableaux de bord complémentaires, listés et consultables par un widget
  dédié.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-5`

- **FR-3 - MUST:** Les widgets standard couvrent au moins l'état des projets et sandboxes, la santé du code, les
  workflows et les runs.
    - **Source:** user instruction, 2026-11-03 (« des widgets standard pour les workflows ») ; `AGENTS.md`
    - **Verified by:** `AC-5`

- **FR-4 - MUST:** Un widget scripté produit au minimum du contenu markdown, un diagramme en barres et un diagramme
  circulaire.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-3`

- **FR-5 - MUST:** Un script de widget voit les serveurs MCP du contexte comme des modules ; il voit aussi les toolsets
  quand ils sont disponibles (souhaité, `TBD` : faisabilité à qualifier avec lymaille).
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-3`

- **FR-6 - MUST:** L'agent productif peut naviguer dans l'interface pour l'utilisateur et lui montrer des pages.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-6`

- **FR-7 - MUST:** L'agent productif peut créer un tableau de bord éphémère pour présenter à l'utilisateur les
  informations qui enrichissent la conversation.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-6`

- **FR-8 - MUST:** Un projet porte des groupes d'artefacts ; l'utilisateur les parcourt, ouvre un artefact et zoome
  dessus.
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-4`

- **FR-9 - SHOULD:** L'utilisateur et le LLM peuvent échanger visuellement sur une idée en manipulant un schéma
  ensemble. La forme est inconnue (`OI-3`).
    - **Source:** user instruction, 2026-11-03

- **FR-10 - MUST:** La méthode SpecDD + DSD est disponible comme pratique de première classe dans les contextes de
  projet et de sandbox, avec son outillage.
    - **Source:** BO-3
    - **Verified by:** `AC-7`

- **FR-11 - MUST:** L'utilisateur relit le travail d'une sandbox dans l'IDE web avant de demander une MR.
    - **Source:** BO-1
    - **Verified by:** `AC-8`

- **FR-12 - MUST:** L'utilisateur voit le diff d'une sandbox avec la branche `main` de son projet.
    - **Source:** BO-1 ; user instruction, 2026-11-03
    - **Verified by:** `AC-8`

- **FR-13 - MUST:** La colonne de conversation se redimensionne en largeur et peut passer en plein écran, avec un
  accès à la liste des sessions (emplacement proposé : barre basse ; `TBD` à la conception).
    - **Source:** user instruction, 2026-11-03
    - **Verified by:** `AC-9`

## Qualité, sécurité et contraintes

- **NFR-1 - MUST:** L'exécution d'un widget scripté est bornée en durée et en volume de sortie, et son résultat est
  mis en cache. Seuils : `TBD` (aucune valeur attribuable à ce jour).
    - **Source:** ASSUMPTION : un script lancé à chaque affichage de page peut saturer la sandbox
    - **Verified by:** `AC-3`

- **NFR-2 - MUST:** Les événements de session transmis au frontend sont typés, et leur type est généré pour TypeScript
  depuis Rust, à la manière des types existants.
    - **Source:** `AGENTS.md` (types ts-rs) ; user instruction, 2026-11-03 (remplacement de `ChatEvent`)

## Paysage des Solutions (proposition de découpage)

Aucune Solution n'est écrite. Le découpage ci-dessous est une **recommandation** à discuter ; chaque entrée est un
sujet de la discussion qui suit cette ébauche.

- `app/sessions` (écrite : `dsd/app/sessions.md`) : hôte de lymaille dans `app` (par l'intégration de miryad-core), contextes de travail, ouverture et
  suivi des sessions, flux d'événements vers le frontend, disposition de la colonne de conversation (`FR-13`).
- `app/dashboards` (écrite : `dsd/app/dashboards.md`) : modèle de tableau de bord et de widget, tableaux par défaut, personnalisation, complémentaires,
  éphémères.
- `app/widgets` : widgets standard et widgets scriptés, y compris le contrat d'exécution et de sécurité des scripts.
- `app/ui-actions` : navigation et présentation pilotées par le LLM.
- `app/artifacts` : modèle projet, groupes d'artefacts, artefacts, et leur affichage isolé.
- `app/workflows` : enrichissement par vanyline du moteur de workflow de miryad-core : workflows de portée projet ou
  sandbox, éditeur, index des runs, widgets, kinds `shell` et `gate` appuyés sur la sandbox. **Ajout proposé par
  Claude**, à confirmer ; le moteur lui-même relève du domaine `workflow` de miryad-core.
- `app/review` : IDE web et vue du diff d'une sandbox avec `main` (relecture avant la MR). **Ajout proposé par Claude**,
  à confirmer : les exigences `FR-11` et `FR-12` n'avaient pas de Solution d'accueil.
- `app/planning` : méthode SpecDD + DSD dans l'application (à articuler avec la sandbox).
- `app/configuration` : écrans de configuration, éventuellement sous forme de tableaux de bord (`OI-2`).

Relations : `dashboards` est consommé par `widgets`, `ui-actions` et `artifacts` ; `sessions` fournit le contexte et
l'identité à tous.

## Décisions du domaine

- **DEC-1 - lymaille est le cœur générique du harnais LLM, hébergé par `app`**
    - **Context:** le tooling LLM maison ne répond pas à la base de harnais voulue ; miryad-core intègre lymaille en
      parallèle.
    - **Decision:** vanyline consomme lymaille par l'intégration de miryad-core, hébergée dans `app` ; vanyline reste le
      harnais dédié au code. Le plan d'extraire le tooling
      de vanyline vers miryad-core est abandonné. Les relais `fs` et `shell` de lymaille ne sont pas activés ; toute
      action sur le code passe par le MCP de la sandbox.
    - **Alternatives:** extraire le tooling de vanyline dans miryad-core (écarté le 2026-11-03).
    - **Consequences:** `lib`, `llm-config`, les agents système et le chat WebSocket actuels sont à retirer à parité
      (`DEC-6`). Le développeur principal a validé le 2026-11-03, par un essai sur une sandbox réelle, qu'un LLM réussit
      une tâche de code quand la sandbox lui est présentée comme des modules Rhai.
    - **Source:** user instruction, 2026-11-03

- **DEC-2 - Le contexte de travail est le seul sélecteur de session**
    - **Context:** choisir parmi des dizaines d'agents n'a pas de valeur pour l'utilisateur ; Claude Code choisit un
      répertoire, pas un agent.
    - **Decision:** la session s'ouvre depuis un contexte ; son vocabulaire technique (agent `Config`, etc.) peut rester
      masqué par l'interface.
    - **Consequences:** reformulation de `agent-sessions#BO-3` demandée à lymaille (sebt3/lymaille#4) ; amendement
      de l'ouverture de session de miryad-core (`OI-1`).
    - **Source:** user instruction, 2026-11-03

- **DEC-3 - Limites posées par le contexte, effets bornés au niveau du pod**
    - **Context:** les toolsets accordés bornent l'intention du LLM, pas les effets (`build.rs`, scripts d'installation
      exécutent du code arbitraire) ; une approbation par commande ne protège pas mieux.
    - **Decision:** `app` borne l'intention par les modules accordés ; les effets se bornent dans la sandbox (réseau,
      secrets, quotas, isolation), dont le domaine reste à écrire.
    - **Consequences:** la liste de ce que l'agent de code ne doit jamais pouvoir faire est écrite dans le domaine de la
      sandbox, pas ici.
    - **Source:** user instruction, 2026-11-03

- **DEC-4 - `git push` depuis la sandbox est normal ; la protection relève de la forge**
    - **Context:** l'Owner porte une clé SSH capable de tirer et de pousser ses dépôts ; une sandbox peut donc pousser
      sur tous les projets de l'Owner, et il en va de même pour le travail de développement de cette équipe.
    - **Decision:** vanyline ne restreint pas le `git push` d'une sandbox. La protection de `main` et la revue des MRs
      relèvent de la forge.
    - **Consequences:** le rayon d'action d'un agent de code est celui de la clé de l'Owner ; il est assumé. Un
      credential limité au projet reste possible côté forge, sans que vanyline l'exige.
    - **Source:** user instruction, 2026-11-03

- **DEC-5 - Tous les écrans hors IDE et vue du diff sont des tableaux de bord de widgets**
    - **Context:** la refonte d'interface a déjà matérialisé des widgets ; l'utilisateur veut étendre l'idée aux
      tableaux personnalisables, complémentaires, scriptés et éphémères.
    - **Decision:** le tableau de bord devient le modèle de page ; l'IDE et la vue du diff restent les seuls écrans hors modèle.
    - **Consequences:** le frontend actuel, fait de sections, est à reprendre vers un modèle de widgets déclaré.
    - **Source:** user instruction, 2026-11-03

- **DEC-6 - Migration par tranche verticale, puis retrait de l'ancien chemin à parité**
    - **Decision:** une première tranche mince de bout en bout (un chat sur une sandbox, dans un contexte, avec un
      tableau par défaut et un artefact) précède tout démontage ; l'ancien chemin est retiré une fois la parité atteinte.
    - **Source:** ASSUMPTION : recommandation de Claude du 2026-11-03, non encore confirmée par le développeur principal

- **DEC-7 - La forge est un serveur MCP ordinaire**
    - **Context:** lister les MRs et issues en cours semble un besoin particulier d'intégration.
    - **Decision:** une fois le MCP de la forge disponible et configuré, il se comporte comme tout module ; aucune
      intégration propre à la forge n'est conçue. L'agent de configuration peut, par exemple, construire un tableau de
      bord qui liste les MRs et issues en cours.
    - **Source:** user instruction, 2026-11-03

- **DEC-8 - Les définitions de tableaux de bord vivent en base de données**
    - **Context:** elles pourraient vivre dans le dépôt du projet (relisibles en MR) ou dans la base de l'application.
    - **Decision:** en base de l'application.
    - **Alternatives:** dans le dépôt du projet (écarté le 2026-11-03).
    - **Consequences:** ces définitions, scripts de widgets compris, sont des données de configuration de
      l'application ; ASSUMPTION : une session productive ne les modifie donc pas (`BR-3`), seules les modifient
      l'utilisateur et une session de configuration. La personnalisation par projet n'est pas relue en MR.
    - **Source:** user instruction, 2026-11-03

## Transition et migration

Les specs `.sdd` existantes d'`app`, du frontend et de `lib` décrivent l'**état actuel** et peuvent contredire ce
domaine ; ces écarts sont attendus et se résorbent par la descente des Solutions, qui amendent les specs. Ils ne
constituent pas un conflit bloquant au sens de DSD.

Le périmètre probablement touché n'est pas mesuré (à confirmer par un inventaire en lecture seule) : `lib`, `cfgstore`,
`cli`, une partie d'`app` (agents, configuration LLM, chat, workflows de kind `agent`, MCP de domaine) et, côté
frontend, `ChatEvent`, `@vanyline/protocol` et `@vanyline/ui`. Inchangés : sandbox, controller, crds, health, `tools`
côté sandbox. Les ~32 commits non poussés de la branche d'état final sont à trier avant tout push ; les passages
« philosophie yolo » de `docs/architecture.md` et `docs/llm-exec-gap.md` sont à reformuler en « permissions fixées par
le contexte, pas par une approbation en session ».

## Acceptation et vérification

- **AC-1:** Depuis la page d'un projet, l'utilisateur ouvre un chat sans choisir d'agent ; la session est rattachée à ce
  contexte, et l'agent ne dispose que des modules accordés par lui.
- **AC-2:** Une session productive qui modifie `.skills/` du workspace ne modifie aucune définition de la configuration ;
  seule une action humaine hors session la promeut.
- **AC-3:** Un widget scripté affiche du markdown, un diagramme en barres et un diagramme circulaire produits par un
  script qui appelle un module MCP du contexte ; un utilisateur privé d'un droit voit l'appel refusé, quel que soit le
  script ; un script trop long ou trop volumineux est interrompu sans bloquer la page.
- **AC-4:** Un artefact contenant du script s'affiche sans pouvoir lire le cookie ni appeler l'API de l'application ;
  l'utilisateur parcourt les groupes d'artefacts d'un projet et zoome sur un artefact.
- **AC-5:** Les tableaux par défaut du projet et de la sandbox s'affichent ; le projet en personnalise un ; le widget de
  liste affiche les tableaux complémentaires du projet.
- **AC-6:** Sur demande, l'agent amène l'utilisateur à une page et affiche un tableau éphémère qui n'existe plus après
  la conversation.
- **AC-7:** Dans un contexte de projet, l'outillage SpecDD est utilisable par l'agent.
- **AC-8:** Depuis une sandbox, l'utilisateur ouvre l'IDE web et voit le diff de la sandbox avec `main`, sans recourir à
  l'agent.
- **AC-9:** L'utilisateur élargit la colonne de conversation, la passe en plein écran, et y retrouve ses sessions.
- **AC-10:** Chaque écran et chaque action de l'interface sont atteignables par l'utilisateur sans conversation avec un
  agent.

## Risques

- **Surface de scripts à l'affichage :** un widget scripté exécute du code à chaque ouverture de page (`NFR-1`, `BR-4`).
- **Largeur du chantier :** tableaux, widgets, artefacts, planification et remplacement du harnais forment un ensemble
  large ; la tranche verticale de `DEC-6` en est la parade.

## Open Issues

- **OI-1 (soldé le 2026-11-12) :** livré par miryad-core 0.3.0 (`DEC-27` à `DEC-29`) ; la descente est dans `app/sessions`, qui
  l'écart résiduel (contextes `H_` sans type) est tranché par `app/sessions#DEC-6`. **Amendements à l'intégration lymaille de miryad-core (`CON-1`).** Conditions listées dans
  `NOTE-vanyline-orientation-lymaille.md` (dépôt miryad-core) : serveurs MCP fournis par l'hôte, ouverture de session
  par contexte, contexte de projet résolu par l'hôte, durée des runs de sous-agent. **TBD :** amendements de ce dépôt-là,
  à trancher dans une session dédiée avant le code de son lot 2.
- **OI-2:** **Pages de configuration en tableaux de bord ?** Étendre le mécanisme aux écrans de configuration, ou les
  garder en écrans de formulaires. **TBD :** à discuter.
- **OI-3:** **Schéma manipulé à deux (`FR-9`).** Forme inconnue. **ASSUMPTION :** une première version pourrait être un
  artefact dont le LLM écrit la source et que l'utilisateur édite en texte ; le temps réel à deux est un autre chantier.
- **OI-4 (soldé le 2026-11-12) :** mono-réplique assumée pour les sessions interactives (`app/sessions#DEC-1`). **Coordination entre réplicas de `app`.** lymaille garantit un tour actif par session et par processus,
  non entre processus (`agent-sessions#CON-7`). **TBD :** limite assumée à une réplique, ou verrou en base.
- **OI-5:** **Références entre racines DSD.** Chaque racine a sa propre bibliothèque ; `agent-sessions` vit dans le dépôt
  lymaille. **ASSUMPTION :** il se référence par son identifiant canonique, comme source externe, avec le chemin du
  dépôt voisin ; à confirmer au premier usage.
- **OI-6:** **Outillage SpecDD dans la boîte à outils JS de la sandbox** (`FR-10`) : exigence portée par
  `sandbox#FR-6`, questions ouvertes dans `sandbox#OI-4`.
- **OI-7:** **Transfert du niveau partagé avec la CLI** (`cli#DEC-4`, `cli#DEC-11`). La CLI est un hôte lymaille autonome ;
  tout le niveau partagé de `.vanyline/` (agents, skills, profils, providers, ContextType, ToolsetContext) circule entre
  elle et `app`, sans secrets, et ce transfert (import et export, dans les deux sens) est de la responsabilité d'`app`.
  Bornes à contractualiser : aucun secret, rien d'effectif avant promotion par une action humaine hors session après
  revue (séparation des rôles, `DEC-3`), les droits ne venant jamais du workspace seul. Format porté par lymaille ;
  support exact et déclenchement : **TBD.**

## Références

- Domaine `agent-sessions` du dépôt `lymaille` (`dsd/agent-sessions.md`).
- `dsd/miryad-core/lymaille.md` et `NOTE-vanyline-orientation-lymaille.md` du dépôt miryad-core.
- Issues lymaille #1 à #4 (`github.com/sebt3/lymaille`).
- `vanyline.sdd`, `AGENTS.md`, `docs/architecture.md`.
- Domaines voisins : `sandbox`, `workspaces`, `cli`.
