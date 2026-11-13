# Solution Design: Widgets scriptés et types de sortie

## Résumé

La Solution `app/widgets` donne au tableau de bord (`app/dashboards`) son widget **scripté** : un script Rhai, lu dans la
définition du tableau, exécuté côté application dans un scratchpad lymaille, sous l'identité de l'utilisateur qui affiche la
page, avec les serveurs MCP et les toolsets du contexte de l'écran. Le script ne rend pas de page : il rend la **déclaration
d'un widget standard à données en ligne** (aujourd'hui `markdown`, `bar`, `pie`), que l'application valide, borne, met en
cache par utilisateur et que le frontend rend avec ses composants ordinaires. L'utilisateur et le LLM obtiennent ainsi plus
d'expressivité que le texte d'une réponse, sans qu'aucun code produit par un LLM ne tourne dans l'origine de l'application.

Elle réalise `app#BR-4`, `app#BR-5` (pour les contenus scriptés), `app#FR-4`, `app#FR-5`, `app#NFR-1` et la part « rendu des
sorties » d'`app#FR-3`. Les widgets à source applicative (projets, sandboxes, workflows, runs, santé du code…) relèvent de
`app/dashboards` ; la présentation par le LLM de tableaux éphémères relève de `app/ui-actions`.

## Problème et objectif

Un tableau de bord ne sait aujourd'hui afficher que des données que l'application détient déjà. Un utilisateur, ou son agent de
configuration, veut un widget alimenté par ses outils (un serveur MCP, un toolset du contexte) : l'état d'un cluster, le nombre
de tickets ouverts, la répartition des runs. Objectif : un widget dont la donnée vient d'un script, **sans** étendre les droits
de l'utilisateur, **sans** charger la sandbox à chaque affichage et **sans** que le contenu rendu soit du code.

## Périmètre et non-objectifs

Couvert :

- le type de widget `scripted` : sa `config`, son enregistrement au registre, sa validation à l'écriture ;
- l'exécution d'un script : moteur, identité, modules et toolsets visibles, bornes, annulation ;
- le contrat de sortie, sa validation et son chemin d'extension à de nouveaux types ;
- le cache des résultats et le rafraîchissement ;
- la route d'exécution et les erreurs `VNL-APP-WDG-*` ;
- le rendu des types de sortie dans le frontend : `markdown`, `bar`, `pie` ; la place de Mermaid et des séries temporelles.

Hors périmètre :

- les widgets à source applicative et le moteur de rendu : `app/dashboards` ;
- le tableau éphémère et sa présentation par le LLM : `app/ui-actions` ;
- un éditeur de script : la première version édite le script en texte dans la définition (`app/dashboards#DEC-6`) ;
- l'exécution de scripts dans la sandbox (shell, JavaScript) : non ouverte (`DEC-2`) ;
- les widgets d'**action** (un bouton qui agit : valider une MR, relancer un run…) : non exclus à terme, non ouverts ici (`OI-2`) ;
- les types de sortie au-delà de `markdown`, `bar`, `pie` (séries temporelles, Mermaid, tables) : le chemin existe (`DEC-5`), ils
  s'ouvrent un à un.

## Contexte applicable du domaine

- `app#BR-3` : seule une session de configuration écrit une définition ; une session productive ne modifie pas un script de widget.
- `app#BR-4` : un widget scripté s'exécute avec l'identité de l'utilisateur qui l'affiche ; le contenu du script n'étend jamais
  ses droits.
- `app#BR-5` : un contenu produit par un LLM n'est jamais du code de l'application.
- `app#FR-4` : un widget scripté produit au minimum du markdown, un diagramme en barres et un diagramme circulaire.
- `app#FR-5` : un script voit les serveurs MCP du contexte comme des modules et, quand ils sont disponibles, ses toolsets.
- `app#NFR-1` : l'exécution est bornée en durée et en volume de sortie, et son résultat est mis en cache.
- `app#CON-1` : le noyau de session, de scratchpad et de toolsets est celui de lymaille, jamais une réimplémentation.

## Acteurs

- **Utilisateur :** affiche un tableau, voit le résultat des scripts qui s'y trouvent sous ses propres droits.
- **Agent de configuration :** écrit un script de widget dans une définition de l'utilisateur, avec son droit (`app/dashboards#DEC-3`).
- **Agent productif :** n'écrit aucun script ; il peut présenter des widgets à données en ligne dans un tableau éphémère (`FR-9`).

## État actuel et état cible

- **Actuel (Observed in : `app/src/workflow/steps/rhai.rs`, `app/src/dashboards/registry.rs`, lymaille 0.2.1) :** le kind `rhai`
  des workflows évalue un script avec un pont `mcp_servers()` / `mcp_list_tools` / `mcp_call_tool`, sans toolsets ; le registre
  des tableaux réserve le nom `scripted` mais `known("scripted")` rend `None` ; `markdown`, `bar` et `pie` existent avec une
  `config` à données en ligne dont la grammaire n'est pas vérifiée à l'écriture ; lymaille fournit `ScratchpadEngine` (moteur
  Rhai sans E/S, modules et toolsets accordés, plafonds de mémoire) et `assemble` (modules et toolsets d'un contexte résolu), mais
  **ne borne volontairement ni la durée ni le nombre d'opérations** (`scratchpad/limits.sdd`).
- **Cible :** `scripted` est un type connu du registre ; un script s'exécute sur le moteur de scratchpad de lymaille avec les
  grants du contexte de l'écran ; sa sortie est la déclaration d'un type à données en ligne, validée contre le schéma du registre.

## Hypothèses, contraintes et dépendances

- **CON-1:** Le moteur est le scratchpad de lymaille (`app#CON-1`). Comme lymaille ne borne pas la durée, **l'application** impose
  son délai par l'annulation prévue au moteur (crochet de progrès).
    - **Source:** `app#CON-1` ; lymaille `scratchpad/limits.sdd`
- **CON-2:** Le script est un champ opaque de la `config` du widget (`app/dashboards#CON-1`) : il vit dans la définition, en base,
  sous le droit de son propriétaire.
    - **Source:** `app/dashboards#CON-1`, `DEC-3`
- **CON-3:** Le contexte d'un écran est celui que l'hôte applicatif résout pour lui : accueil → contexte stocké `G_home` ; projet →
  `H_project_<nom>` ; sandbox → `H_sandbox_<owner>_<nom>` (`app/sessions#DEC-6`, `DEC-9`). Les serveurs MCP et toolsets visibles
  sont ceux de ce contexte pour cet utilisateur.
    - **Source:** `app/sessions` (`host.sdd`)
- **CON-4:** L'application est mono-réplique pour les sessions (`app/sessions#DEC-1`) ; le cache de `FR-6` est donc en mémoire du
  processus et perdu au redémarrage.
    - **Source:** `app/sessions#DEC-1`

## Exigences fonctionnelles

- **FR-1 - MUST:** Le type `scripted` est connu du registre des tableaux ; sa `config` est `{script, ttl_secs?}` où `script` est un
  texte non vide (sa taille est bornée par celle de la définition, `app/dashboards#FR-3`) et `ttl_secs` un entier positif ou nul. Une `config` hors de cette forme est refusée à
  l'écriture avec `VNL-APP-DSH-*` (validation de `app/dashboards`), jamais corrigée.
    - **Source:** `app/dashboards#FR-3`, `FR-6`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** L'exécution d'un widget passe par **une route** de l'application qui désigne le widget par son tableau (écran, ou
  identifiant de tableau complémentaire) et son identifiant. **Le script n'est jamais fourni par le client** : il est relu dans la
  définition de l'utilisateur authentifié.
    - **Source:** `app#BR-4`, `DEC-6`
    - **Verified by:** `AC-6`

- **FR-3 - MUST:** Le script s'exécute sous l'identité de l'utilisateur qui affiche ; il ne voit que les serveurs MCP (comme des
  modules) et les toolsets que le contexte de l'écran accorde à cet utilisateur. Un serveur ou un outil hors droit est introuvable
  pour le script, sans distinguer « absent » de « interdit ».
    - **Source:** `app#BR-4`, `FR-5`
    - **Verified by:** `AC-2`

- **FR-4 - MUST:** Le script rend une map `{type, config}` : `type` est un type de widget standard dont la source de données est
  `Inline` (aujourd'hui `markdown`, `bar`, `pie`) ; `config` est validée contre le schéma de ce type au registre — grammaire
  réellement vérifiée, bornes de taille et de profondeur comprises. Toute autre sortie est une erreur d'exécution, jamais affichée
  telle quelle ni corrigée.
    - **Source:** `app#FR-4`, `DEC-1`
    - **Verified by:** `AC-1`, `AC-3`

- **FR-5 - MUST:** L'application pose des **plafonds de sécurité** à l'exécution — durée, mémoire, taille de la sortie sérialisée — dont
  les valeurs sont de la configuration d'application, réglée par l'administrateur, avec des défauts larges (`OI-3`). Ils protègent le
  processus (un script qui boucle ne s'arrête pas seul et l'application sert aussi les sessions des autres), pas l'utilisateur d'un
  script lent : tant qu'un plafond n'est pas atteint, rien n'interrompt le script, et le nombre d'appels MCP n'est pas limité. Un
  dépassement interrompt le script et rend une erreur typée nommant le plafond atteint, jamais un résultat partiel.
    - **Source:** `app#NFR-1`
    - **Verified by:** `AC-4`

- **FR-6 - MUST:** Le résultat est mis en cache **par (utilisateur, tableau, widget)**, jamais partagé entre utilisateurs. La durée de
  vie est `config.ttl_secs` (défaut d'une heure ; `0` désactive le cache) ; le widget offre à l'utilisateur un bouton
  « actualiser » et affiche l'ancienneté du résultat et la durée de son dernier calcul — un widget lent est un indicateur que
  l'utilisateur voit, pas une chose que l'application tranche pour lui. Une ouverture dans la durée de vie ne relance pas le
  script ; une ouverture au-delà, ou une action de l'utilisateur (« actualiser »), le relance. Une exécution déjà en cours pour
  la même clé est partagée, pas doublée. Modifier le script invalide l'entrée.
    - **Source:** `app#NFR-1`, `DEC-3`
    - **Verified by:** `AC-5`

- **FR-7 - MUST:** Une erreur de script (syntaxe, exécution, outil MCP, bornes, sortie invalide) est rendue au widget sous la forme
  d'un état d'erreur structuré (identifiant `VNL-APP-WDG-NNN`, message borné sans contenu interne) ; elle n'empêche pas le rendu
  des autres widgets du tableau.
    - **Source:** `app/dashboards#FR-4` ; convention du projet
    - **Verified by:** `AC-3`

- **FR-8 - MUST:** Le frontend rend les types à données en ligne par des composants ordinaires du registre de rendu : `markdown`
  (sans HTML brut), `bar` et `pie` par ECharts. Un type de sortie inconnu du frontend est un emplacement d'erreur lisible.
    - **Source:** `DEC-4` ; `app/dashboards#FR-4`
    - **Verified by:** `AC-1`

- **FR-9 - MUST:** Un tableau **éphémère** ne contient pas de widget `scripted` : un tableau reçu qui en porte un est refusé. Le LLM y
  présente des widgets à données en ligne, que l'application ne fait pas exécuter.
    - **Source:** `app#BR-5` ; `app/dashboards#FR-9`
    - **Verified by:** `AC-7`

- **FR-10 - MUST:** Le widget scripté a trois états visibles : **prêt** (résultat, ancienneté, durée du dernier calcul, actualisable),
  **en cours** (indicateur de travail, interruptible par l'utilisateur) et **échec** (erreur structurée de `FR-7`, relançable).
  L'interruption par l'utilisateur et le plafond de durée de `FR-5` passent par la même annulation du moteur ; l'interruption ne laisse
  aucun résultat partiel ni entrée de cache.
    - **Source:** décision du développeur principal, 2026-11-13
    - **Verified by:** `AC-8`

## Sécurité

- **Identité.** L'exécution suit l'identité de l'utilisateur authentifié (`FR-3`) ; aucun jeton de l'Owner ni de MCP n'est visible
  du script ni du navigateur ; le cache n'est jamais partagé entre utilisateurs (`FR-6`).
- **Contenu.** La sortie est une donnée validée (`FR-4`) rendue par des composants du registre : aucun HTML, aucun script, aucune
  fonction JavaScript n'est accepté dans une `config` — les options d'ECharts sont du JSON, sans les champs qui admettent une
  fonction (formateurs, callbacks). `markdown` ne rend pas de HTML brut. Mermaid, le jour où il s'ouvre, tourne en
  `securityLevel: strict` sur une version épinglée au moins égale à 11.15.0 (des failles d'injection HTML y ont été corrigées en
  2026, et les incidents publics récents viennent d'un niveau `loose`).
- **Surface d'exécution.** Un script tourne à chaque ouverture périmée de page (`app#Risques`) : le cache évite la répétition et des
  plafonds de sécurité (durée, mémoire, sortie) empêchent un script emballé de saturer le processus ; le scratchpad n'ouvre aucune E/S
  propre (accès seulement par les modules accordés).
- **Écriture.** Un script ne modifie pas de définition ; seul l'agent de configuration écrit un script, avec le droit de
  l'utilisateur et la validation de `FR-1` (`app#BR-3`). Les effets de bord d'un script sont traités en `OI-2`.

## Décisions

- **DEC-1 - La sortie d'un script est la déclaration d'un widget standard à données en ligne**
    - **Decision:** le script rend `{type, config}` d'un type dont la source est `Inline` ; la validation, le rendu et les bornes sont
      ceux du registre de `app/dashboards`. Ajouter un type à données en ligne (séries temporelles, Mermaid, table…) l'ouvre aux
      scripts sans modifier le moteur d'exécution.
    - **Alternatives:** un contrat de sortie propre aux scripts (écarté : deux modèles de rendu et de validation à maintenir) ;
      du markdown seul avec des conventions pour les graphiques (écarté : conventions fragiles, pas de contrôle de rendu).
    - **Consequences:** le registre doit vérifier la **grammaire** de `config` des types inline (aujourd'hui seule la forme objet
      l'est), au moins pour les sorties de script.
    - **Source:** décision du développeur principal, 2026-11-13 (« plus d'expressivité que le texte de réponse »)

- **DEC-2 - Le moteur est le scratchpad de lymaille, dans l'application**
    - **Decision:** le script s'exécute sur `ScratchpadEngine` avec les grants que `assemble` produit pour le contexte de l'écran
      (modules MCP, toolsets), sous l'identité de l'utilisateur ; l'application ajoute son propre délai (`CON-1`). Les toolsets du
      contexte sont donc visibles (`app#FR-5`).
    - **Alternatives:** le kind `rhai` des workflows, avec son pont MCP (écarté : ne donne pas les toolsets du contexte et dupliquerait
      ce que lymaille fournit) ; un script dans la sandbox (écarté : exige un pod en marche pour tout widget et le charge à chaque
      affichage, risque nommé par `app`).
    - **Consequences:** qualification de l'API d'exécution hors session d'agent à faire d'abord (`OI-1`).
    - **Source:** décision du développeur principal, 2026-11-13 (Rhai dans l'application, toolsets du contexte inclus)

- **DEC-3 - Cache en mémoire, par utilisateur, durée de vie portée par la config**
    - **Decision:** voir `FR-6`.
    - **Alternatives:** pas de cache, exécution sur demande (écarté : un tableau scripté n'afficherait rien à l'ouverture) ; cache en
      base (écarté : migration, purge et relecture par droits pour un gain nul en mono-réplique).
    - **Consequences:** le résultat est perdu au redémarrage et recalculé.
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-4 - ECharts pour les graphiques, Mermaid pour les diagrammes, départ minimal**
    - **Decision:** `bar` et `pie` sont rendus par ECharts à partir d'une `config` JSON validée ; la première version n'ouvre que ces
      deux graphiques, plus `markdown`. Les séries temporelles et autres graphiques passent par la même bibliothèque quand ils
      s'ouvrent. Mermaid sert aux diagrammes (`securityLevel: strict`, version épinglée ≥ 11.15.0). Une seule bibliothèque de
      graphiques, pas de D3.
    - **Alternatives:** Chart.js (écarté : moins de types, séries temporelles via adaptateur, risque de changer de bibliothèque quand
      les besoins grossiront) ; D3 (écarté : impératif — un rendu par type à écrire et à maintenir, et c'est du code, pas une donnée).
    - **Consequences:** import modulaire d'ECharts à mesurer côté bundle ; liste blanche des types de série et rejet des fonctions.
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-5 - Les nouveaux types de sortie s'ouvrent un à un par le registre**
    - **Decision:** un type à données en ligne s'ajoute par une entrée du registre (nom, schéma de `config`, source `Inline`), un composant
      frontend et ses tests, sans toucher à l'exécution ni au cache.
    - **Source:** `DEC-1`

- **DEC-6 - Le script n'est jamais fourni par le client**
    - **Decision:** la route d'exécution reçoit l'identité du widget, pas son script ; l'application le relit dans la définition de
      l'utilisateur authentifié. Un client ne peut donc ni exécuter un script arbitraire ni exécuter celui d'autrui.
    - **Source:** `app#BR-4`, `app/dashboards#CON-2`

## Réalisation SpecDD

Specs à créer ou à amender (une spec par fichier) :

- `app/src/widgets/{mod,runtime,output,cache}.sdd` : route d'exécution (`FR-2`), moteur et bornes (`FR-3`, `FR-5`), validation de la
  sortie (`FR-4`), cache (`FR-6`), erreurs (`FR-7`).
- `app/src/dashboards/{registry,mod,model}.sdd` : `scripted` devient un type connu (source `Scripted`) ; règles de `config` de `FR-1` ;
  refus de `scripted` dans un éphémère (`FR-9`) ; grammaire des types inline (`DEC-1`).
- `app/src/api/mod.sdd`, `app/src/main.sdd` : montage de la route ; `app/src/error.sdd` : variantes `VNL-APP-WDG-*`.
- `packages/protocol` : types de la réponse d'exécution (générés par ts-rs).
- Frontend : composants `bar`, `pie` (ECharts) et `markdown` du registre de rendu ; hôte de widget scripté (état de chargement, erreur,
  actualisation) ; une spec par composant.

## Acceptation et vérification

- **AC-1:** Un script qui rend du markdown, un diagramme en barres et un diagramme circulaire les affiche dans leurs widgets.
- **AC-2:** Un script qui appelle un serveur MCP que le contexte n'accorde pas à l'utilisateur échoue comme pour un serveur inexistant ;
  un utilisateur sans droit ne voit pas les outils d'un autre.
- **AC-3:** Une sortie hors contrat (type inconnu, `config` hors schéma, HTML, trop volumineuse) produit un état d'erreur `VNL-APP-WDG-*`
  et les autres widgets du tableau s'affichent.
- **AC-4:** Un script qui boucle est interrompu au plafond de durée configuré, un script qui dépasse le plafond de mémoire ou de sortie
  est refusé, avec une erreur qui nomme le plafond et sans résultat partiel ; un script lent qui reste sous les plafonds s'affiche, avec sa
  durée.
- **AC-5:** Une seconde ouverture dans la durée de vie n'exécute pas le script ; un autre utilisateur du même tableau ne reçoit pas le
  résultat du premier ; modifier le script relance l'exécution.
- **AC-6:** Une requête d'exécution porteuse d'un script ne l'exécute pas (le script vient de la définition) ; le widget d'un autre
  utilisateur est introuvable.
- **AC-7:** Un tableau éphémère portant un widget `scripted` est refusé.
- **AC-8:** Pendant qu'un script tourne, le widget indique le travail en cours et l'utilisateur peut l'interrompre : le widget revient à
  l'état prêt avec l'ancien résultat s'il en existe un, sinon à l'état d'échec relançable ; aucun résultat partiel n'est mis en cache.

## Open Issues

- **OI-1 (soldé le 2026-11-13) - API d'exécution de lymaille hors session d'agent.** Qualifiée par un essai jetable contre lymaille 0.2.1
  (`spikes/lymaille-widget`, non livré) : un script isolé s'exécute sans session — moteur construit par `assemble` (toolsets) puis
  `ScratchpadEngine::builder`, réutilisable d'un run à l'autre ; un pad neuf par exécution (200 exécutions : 40 ms) avec une annulation, un
  rédacteur de secrets et une horloge ; runtime tokio multi-thread requis. Constats : (1) un module accordé et un toolset sont appelables
  (`ops::bar(...)`) ; (2) la valeur finale du script est disponible seule, en JSON compact, dans l'enregistrement du run (sans les `print`),
  et un indicateur dit si elle dépasse le plafond de rendu — qui devient le plafond de sortie de `FR-5` ; (3) un module non accordé est
  « introuvable » pour le script (`FR-3`) ; (4) l'annulation interrompt une boucle et un `sleep(5)` en quelques millisecondes (la variante flottante `sleep(5.0)` n'existe pas : seule la
  signature entière, interruptible, est exposée aux scripts) ; (5) le plafond de taille des
  chaînes est appliqué (erreur typée). **Non établi :** une fonction native **synchrone** bloquante enregistrée par l'hôte n'est pas interrompue par
  l'annulation (cas de mon essai, pas du `sleep` de vynil-core ; les appels MCP sont asynchrones et abandonnés à l'annulation selon
  lymaille, non essayé sans serveur MCP réel), et le plafond de taille
  des tableaux ne s'est pas déclenché dans l'essai (des scripts pathologiques sont très lents — 3 millions d'ajouts dépassent une minute) :
  le plafond de durée de `FR-5` est donc aussi la protection mémoire. **Reste à faire dans la spec :** obtenir les `Toolset` et les
  modules MCP du contexte d'un écran côté application (stores de miryad-core, `resolve_mcp` de l'hôte de `app/sessions`).
- **OI-2 - Effets de bord et widgets d'action.** Un script lit aujourd'hui (taux de succès des tests d'une sandbox, nombre d'issues
  et de MR ouvertes). À terme, un widget peut porter un bouton qui agit (valider une MR) : cela n'est pas exclu. **TBD :** à concevoir
  alors — l'exécution déclenchée par l'affichage (ouverture, expiration du cache) et l'action déclenchée par un clic de l'utilisateur
  sont deux chemins distincts, et seul le second peut écrire. Rien à contracter dans la première version : les serveurs et outils
  accordés au script sont ceux du contexte, sous les droits de l'utilisateur (`FR-3`).
- **OI-3 - Valeurs par défaut des plafonds de `FR-5`.** Ce sont des réglages de l'application (variables d'environnement du déploiement),
  pas des bornes du widget : défauts proposés, larges, à ajuster à l'usage — durée 5 min, sortie sérialisée 1 Mio, mémoire celle de
  `ScratchpadLimits` ; aucun plafond sur le nombre d'appels MCP ni sur `ttl_secs`.
- **OI-4 - Grammaire des types inline.** Les schémas `config` de `bar` et `pie` sont déclaratifs et non vérifiés à l'écriture
  (`app/dashboards`) ; `DEC-1` exige de les vérifier pour les sorties de script. À trancher : validation par schéma JSON dans l'application
  (dépendance à choisir) ou validation Rust par type.

## Références

- `dsd/app.md` : `BR-3` à `BR-5`, `FR-3` à `FR-5`, `NFR-1`, `CON-1`, `DEC-5`, `Risques`.
- `dsd/app/dashboards.md` : `CON-1`, `FR-3`, `FR-4`, `FR-6`, `FR-9`, `DEC-3`, `DEC-6`.
- `dsd/app/sessions.md` : `DEC-1`, `DEC-6`, `DEC-9`.
- lymaille 0.2.1 : `scratchpad/engine.sdd`, `scratchpad/limits.sdd`, `scratchpad/run.sdd`, `toolsets/assemble.sdd`.
- `app/src/workflow/steps/rhai.sdd`, `app/src/sessions/host.sdd`, `app/src/dashboards/registry.sdd`.
- Rendu : documentation d'Apache ECharts (option JSON) ; avis de sécurité Mermaid de 2026 (`CVE-2026-41149`).
