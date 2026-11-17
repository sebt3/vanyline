# Solution Design: Widgets scriptés et types de sortie

## Résumé

La Solution `app/widgets` donne au tableau de bord (`app/dashboards`) son widget **scripté** : un script Rhai, lu dans la définition du
tableau, exécuté côté application par le `ScriptRunner` de miryad-core, sous l'identité de l'utilisateur qui affiche la page, avec les
modules d'hôte, les toolsets et les rôles MCP que le **type du contexte de l'écran** accorde aux scripts. Le script ne rend pas de page : il
rend la **déclaration d'un widget graphique** (le type de graphique, ses données et un peu de configuration d'affichage ; aujourd'hui
`bar` et `pie`), que l'application valide, met en cache par utilisateur et que le frontend rend avec ses composants ordinaires. Les
widgets de texte et de diagramme — `markdown` et `mermaid` — n'ont pas besoin de script : ils prennent leur contenu directement en
paramètre. L'utilisateur et le LLM obtiennent ainsi plus d'expressivité que le texte d'une réponse, sans qu'aucun code produit par un LLM
ne tourne dans l'origine de l'application.

Elle réalise `app#BR-4`, `app#BR-5` (pour les contenus scriptés), `app#FR-4`, `app#FR-5`, `app#NFR-1` et la part « rendu des
sorties » d'`app#FR-3`. Les widgets à source applicative (projets, sandboxes, workflows, runs, santé du code…) relèvent de
`app/dashboards` ; la présentation par le LLM de tableaux éphémères relève de `app/ui-actions`.

## Problème et objectif

Un tableau de bord ne sait aujourd'hui afficher que des données que l'application détient déjà. Un utilisateur, ou son agent de
configuration, veut un widget alimenté par ses outils (un serveur MCP, un toolset du contexte) : le taux de succès des tests d'une sandbox,
le nombre d'issues et de MR ouvertes. Objectif : un widget dont la donnée vient d'un script, **sans** étendre les droits de l'utilisateur,
**sans** charger la sandbox à chaque affichage et **sans** que le contenu rendu soit du code.

## Périmètre et non-objectifs

Couvert :

- le type de widget `scripted` : sa `config`, son enregistrement au registre, sa validation à l'écriture ;
- l'exécution d'un script par le `ScriptRunner`, les droits qui s'y appliquent et ses plafonds ;
- le contrat de sortie, sa validation et son chemin d'extension à de nouveaux types ;
- le cache des résultats, le rafraîchissement et l'interruption ;
- la route d'exécution et les erreurs `VNL-APP-WDG-*` ;
- le rendu dans le frontend des widgets à données en ligne : `bar`, `pie` (sortie des scripts), `markdown` et `mermaid` (contenu direct) ;
  la place des séries temporelles.

Hors périmètre :

- les widgets à source applicative et le moteur de rendu : `app/dashboards` ;
- le tableau éphémère et sa présentation par le LLM : `app/ui-actions` ;
- un éditeur de script : la première version édite le script en texte dans la définition (`app/dashboards#DEC-6`) ;
- les widgets d'**action** (un bouton qui agit : valider une MR, relancer un run…) : voulus à terme, non ouverts ici (`OI-5`) ;
- les types de graphique au-delà de `bar` et `pie` (séries temporelles, tables…) : le chemin existe (`DEC-5`), ils s'ouvrent un à un ;
- un markdown ou un diagramme **calculé** par un script : en première version, ces deux widgets prennent leur contenu directement (`OI-8`) ;

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
- **Administrateur :** règle les droits que chaque type de contexte accorde aux scripts (`script_grants`) et les plafonds d'exécution.

## État actuel et état cible

- **Actuel (Observed in : `app/src/workflow/steps/rhai.rs`, `app/src/dashboards/registry.rs`, miryad-core 0.3.1
  `lymaille/script_runner.rs` et `dsd/miryad-core/scripts-hote.md`) :** le kind `rhai` des workflows évalue un script avec un pont
  `mcp_servers()` / `mcp_list_tools` / `mcp_call_tool`, sans toolsets ; le registre des tableaux réserve le nom `scripted` mais
  `known("scripted")` rend `None` ; `markdown`, `bar` et `pie` existent avec une `config` à données en ligne dont la grammaire n'est
  pas vérifiée à l'écriture. miryad-core 0.3.1 fournit `ScriptRunner::run(contexte, sujet, script, limites, jeton)` : fonction de
  bibliothèque sans route ni cache, qui rend une valeur JSON, applique les droits du type de contexte intersectés avec ceux du
  principal relu à chaque appel, monte `resources`, les modules d'hôte accordés et les rôles MCP des toolsets accordés (jamais `fs`
  ni `shell`), et borne durée, opérations et taille du résultat par des erreurs typées.
- **Cible :** `scripted` est un type connu du registre ; un script s'exécute par le `ScriptRunner` dans le contexte de l'écran ; sa
  sortie est la déclaration d'un type à données en ligne, validée contre le schéma du registre.

## Hypothèses, contraintes et dépendances

- **CON-1:** Le moteur est le `ScriptRunner` de miryad-core (qui s'appuie sur lymaille), jamais un moteur propre à vanyline
  (`app#CON-1`). Il n'a ni cache ni route : ils sont à l'application.
    - **Source:** `app#CON-1` ; miryad-core `scripts-hote#FR-4`, `FR-7`, `OI-3`, `OI-4`
- **CON-2:** Le script est un champ opaque de la `config` du widget (`app/dashboards#CON-1`) : il vit dans la définition, en base,
  sous le droit de son propriétaire.
    - **Source:** `app/dashboards#CON-1`, `DEC-3`
- **CON-3:** Les droits d'un script viennent du **type du contexte** (`script_grants` : toolsets et modules d'hôte accordables) et du
  principal qui l'exécute, jamais du script ni de son auteur. Le contexte d'un écran est celui que l'hôte applicatif résout pour lui :
  accueil → contexte stocké `G_home` ; projet → `H_project_<nom>` ; sandbox → `H_sandbox_<owner>_<nom>` (`app/sessions#DEC-6`,
  `DEC-9`). Un contexte `H_…` n'a pas de type en base : l'hôte rend ses `script_grants` (miryad-core 0.4.0, #70) depuis des types de
  catalogue dédiés, `G_project` et `G_sandbox` (`app/sessions`, `catalog.sdd`, `host.sdd`), que l'administrateur règle comme `G_home` ; les
  trois écrans exécutent donc des scripts.
    - **Verified by:** voir les specs `app/sessions`
    - **Source:** miryad-core `scripts-hote#FR-5`, `DEC-2`, `OI-6` ; `app/sessions` (`host.sdd`)
- **CON-4:** L'application est mono-réplique pour les sessions (`app/sessions#DEC-1`) ; le cache de `FR-6` est donc en mémoire du
  processus et perdu au redémarrage.
    - **Source:** `app/sessions#DEC-1`
- **CON-5:** La limite de durée du `ScriptRunner` coupe le script **entre deux opérations** : elle n'interrompt pas un appel natif en
  cours (un appel MCP passe par un pont bloquant). Le délai d'un appel MCP est celui de son connecteur.
    - **Source:** miryad-core `script_runner.rs` (crochet de progression) ; à vérifier sur un serveur MCP réel

## Exigences fonctionnelles

- **FR-1 - MUST:** Le type `scripted` est connu du registre des tableaux ; sa `config` est `{script, ttl_secs?}` où `script` est un
  texte non vide (sa taille est bornée par celle de la définition, `app/dashboards#FR-3`) et `ttl_secs` un entier positif ou nul. Une
  `config` hors de cette forme est refusée à l'écriture avec `VNL-APP-DSH-*` (validation de `app/dashboards`), jamais corrigée.
    - **Source:** `app/dashboards#FR-3`, `FR-6`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** L'exécution d'un widget passe par **une route** de l'application qui désigne le widget par son tableau (écran, ou
  identifiant de tableau complémentaire) et son identifiant. **Le script n'est jamais fourni par le client** : il est relu dans la
  définition de l'utilisateur authentifié, puis passé au `ScriptRunner` avec le contexte de l'écran et le sujet authentifié.
    - **Source:** `app#BR-4`, `DEC-6`
    - **Verified by:** `AC-6`

- **FR-3 - MUST:** Les droits du script sont ceux que le type du contexte accorde aux scripts, intersectés avec ceux de l'utilisateur
  authentifié, relus à chaque appel de module. Un module, un toolset ou un outil hors de ces droits est introuvable pour le script. Le
  script ne reçoit aucun droit de son propre contenu.
    - **Source:** `app#BR-4`, `FR-5` ; miryad-core `scripts-hote#FR-5`
    - **Verified by:** `AC-2`

- **FR-4 - MUST:** Le script rend une map `{type, config}` : `type` est un type de widget **graphique** que le registre marque comme
  scriptable (aujourd'hui `bar` et `pie`) ; `config` porte les données du graphique et, au besoin, un peu de configuration d'affichage ;
  elle est validée contre le schéma de ce type au registre — grammaire réellement vérifiée. Toute autre sortie, y compris un type qui n'est
  pas graphique (`markdown`, `mermaid`), est une erreur d'exécution, jamais affichée telle quelle ni corrigée. Une sortie plus grosse que
  le plafond du `ScriptRunner` est refusée (erreur typée), jamais tronquée.
    - **Source:** `app#FR-4` (restreint, `OI-8`), `DEC-1` ; miryad-core `scripts-hote#FR-7`
    - **Verified by:** `AC-1`, `AC-3`

- **FR-5 - MUST:** L'application fournit au `ScriptRunner` des plafonds de sécurité — durée, opérations Rhai, taille du résultat — dont les
  valeurs sont de la configuration d'application, réglée par l'administrateur, avec des défauts larges (`OI-6`). Ils protègent le
  processus (un script qui boucle ne s'arrête pas seul et l'application sert aussi les sessions des autres), pas l'utilisateur d'un
  script lent : tant qu'un plafond n'est pas atteint, rien n'interrompt le script, et le nombre d'appels MCP n'est pas limité. Un
  dépassement rend l'erreur typée de miryad-core, nommant le plafond atteint, jamais un résultat partiel.
    - **Source:** `app#NFR-1` ; miryad-core `scripts-hote#FR-7`
    - **Verified by:** `AC-4`

- **FR-6 - MUST:** Le résultat est mis en cache **par (utilisateur, tableau, widget)**, jamais partagé entre utilisateurs. La durée de
  vie est `config.ttl_secs` (défaut d'une heure ; `0` désactive le cache). Une ouverture dans la durée de vie ne relance pas le
  script ; une ouverture au-delà, ou une action de l'utilisateur (« actualiser »), le relance. Une exécution déjà en cours pour
  la même clé est partagée, pas doublée. Modifier le script invalide l'entrée. Le widget affiche l'ancienneté du résultat et la durée de
  son dernier calcul — un widget lent est un indicateur que l'utilisateur voit, pas une chose que l'application tranche pour lui.
    - **Source:** `app#NFR-1`, `DEC-3` ; miryad-core `scripts-hote#OI-4` (le cache est à l'hôte)
    - **Verified by:** `AC-5`

- **FR-7 - MUST:** Une erreur de script (syntaxe, exécution, outil MCP, plafonds, sortie invalide, droits) est rendue au widget sous la
  forme d'un état d'erreur structuré (identifiant `VNL-APP-WDG-NNN`, message borné sans contenu interne, sans source ni jeton) ; elle
  n'empêche pas le rendu des autres widgets du tableau.
    - **Source:** `app/dashboards#FR-4` ; convention du projet
    - **Verified by:** `AC-3`

- **FR-8 - MUST:** Le frontend rend les widgets à données en ligne par des composants ordinaires du registre de rendu : `bar` et `pie` par
  ECharts (options JSON uniquement), `markdown` sans HTML brut, `mermaid` par Mermaid en `securityLevel: strict`. `markdown` et `mermaid`
  prennent leur contenu en paramètre (`config.text`, `config.source`), sans script. Un type inconnu du frontend est un emplacement d'erreur
  lisible.
    - **Source:** `DEC-4` ; `app/dashboards#FR-4`
    - **Verified by:** `AC-1`

- **FR-9 - MUST:** Un tableau **éphémère** ne contient pas de widget `scripted` : un tableau reçu qui en porte un est refusé. Le LLM y
  présente des widgets à données en ligne, que l'application ne fait pas exécuter. Un script exécuté agit sous tous les droits de
  l'utilisateur (écritures comprises) : il ne peut pas venir d'un contenu produit par un LLM sans passer par une définition que
  l'utilisateur ou son agent de configuration a écrite.
    - **Source:** `app#BR-5` ; `app/dashboards#FR-9` ; miryad-core `scripts-hote#DEC-3`
    - **Verified by:** `AC-7`

- **FR-10 - MUST:** Le widget scripté a trois états visibles : **prêt** (résultat, ancienneté, durée du dernier calcul, actualisable),
  **en cours** (indicateur de travail, interruptible par l'utilisateur) et **échec** (erreur structurée de `FR-7`, relançable).
  L'interruption par l'utilisateur ne laisse aucun résultat partiel ni entrée de cache. L'interruption passe par le jeton d'annulation de
  `ScriptRunner::run_cancellable` (miryad-core 0.4.0, #71) : le script s'arrête entre deux opérations.
    - **Source:** décision du développeur principal, 2026-11-13
    - **Verified by:** `AC-8`

## Sécurité

- **Identité.** L'exécution suit l'identité de l'utilisateur authentifié (`FR-3`) ; aucun jeton de l'Owner ni de MCP n'est visible
  du script ni du navigateur ; le cache n'est jamais partagé entre utilisateurs (`FR-6`).
- **Droits.** Un script agit sous les droits de son spectateur, sans mode lecture seule (miryad-core `scripts-hote#DEC-3`) : un widget peut
  agir. Ses droits viennent du `script_grants` du type de contexte, que seul un administrateur règle, et du principal relu à chaque appel.
  Les modules natifs de lymaille (`fs`, `shell`) ne sont jamais montés.
- **Contenu.** La sortie est une donnée validée (`FR-4`) rendue par des composants du registre : aucun HTML, aucun script, aucune
  fonction JavaScript n'est accepté dans une `config` — les options d'ECharts sont du JSON, sans les champs qui admettent une
  fonction (formateurs, callbacks). `markdown` ne rend pas de HTML brut. `mermaid` tourne en
  `securityLevel: strict` sur une version épinglée au moins égale à 11.15.0 (des failles d'injection HTML y ont été corrigées en
  2026, et les incidents publics récents viennent d'un niveau `loose`).
- **Surface d'exécution.** Un script tourne à chaque ouverture périmée de page (`app#Risques`) : le cache évite la répétition et des
  plafonds de sécurité (durée, opérations, résultat) empêchent un script emballé de saturer le processus.
- **Écriture de définitions.** Seul l'agent de configuration écrit un script de widget, avec le droit de l'utilisateur et la validation de
  `FR-1` (`app#BR-3`).

## Décisions

- **DEC-1 - Un script produit un widget graphique ; le texte et les diagrammes prennent leur contenu directement**
    - **Decision:** le script rend `{type, config}` d'un type graphique marqué scriptable au registre (`bar`, `pie`) ; la validation, le rendu
      et les bornes sont ceux du registre de `app/dashboards`. Ajouter un graphique (séries temporelles…) l'ouvre aux scripts sans modifier
      le moteur d'exécution. Les widgets `markdown` (`config.text`) et `mermaid` (`config.source`) sont des widgets standard à données en
      ligne dont le contenu est saisi ou écrit directement dans la définition : aucun script n'est nécessaire pour les afficher.
    - **Alternatives:** un script qui rend aussi du markdown ou un diagramme (écarté pour la première version : un texte statique n'a pas
      besoin d'être généré ; à rouvrir si un besoin de texte calculé apparaît, `OI-8`) ; un contrat de sortie propre aux scripts (écarté : deux
      modèles de rendu et de validation à maintenir).
    - **Consequences:** le registre porte un indicateur « scriptable » par type et vérifie la **grammaire** de `config` des types graphiques
      (aujourd'hui seule la forme objet l'est), au moins pour les sorties de script.
    - **Source:** décision du développeur principal, 2026-11-13 (« plus d'expressivité que le texte de réponse » ; le script est pour les
      widgets graphiques)

- **DEC-2 - Le moteur est le `ScriptRunner` de miryad-core, appelé par l'application**
    - **Decision:** l'application appelle `ScriptRunner::run` avec le contexte de l'écran, le sujet authentifié, le script de la définition,
      des `ScriptLimits` issues de sa configuration et, si le serveur MCP l'exige, le jeton de l'utilisateur. Les droits, le montage des
      toolsets et des rôles MCP, les erreurs typées et la sortie JSON sont ceux de la crate. L'application ajoute la route, le cache et la
      validation de la sortie.
    - **Alternatives:** assembler un `ScratchpadEngine` de lymaille soi-même (écarté : réimplémenter la chaîne d'autorité, les rôles MCP et
      le plafond d'opérations que `scripts-hote#DEC-6` explique déjà ; son texte masqué et plafonné n'est pas une valeur) ; le kind
      `rhai` des workflows (écarté : pas de toolsets, pont MCP propre à vanyline).
    - **Consequences:** les toolsets et rôles MCP visibles sont ceux de `script_grants` du type de contexte (`CON-3`) ; l'interruption par
      l'utilisateur passe par `run_cancellable`.
    - **Source:** décision du développeur principal, 2026-11-13 (Rhai dans l'application, toolsets du contexte inclus) ; miryad-core
      `scripts-hote#DEC-1`, `DEC-6`

- **DEC-3 - Cache en mémoire, par utilisateur, durée de vie portée par la config**
    - **Decision:** voir `FR-6`.
    - **Alternatives:** pas de cache, exécution sur demande (écarté : un tableau scripté n'afficherait rien à l'ouverture) ; cache en
      base (écarté : migration, purge et relecture par droits pour un gain nul en mono-réplique).
    - **Consequences:** le résultat est perdu au redémarrage et recalculé.
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-4 - ECharts pour les graphiques, Mermaid pour les diagrammes, embarqués dès la première version**
    - **Decision:** `bar` et `pie` sont rendus par ECharts à partir d'une `config` JSON validée ; la première version n'ouvre que ces deux
      graphiques. `mermaid` est un widget standard livré dès cette version : sa `config` est `{title?, source}` et il est rendu par Mermaid en
      `securityLevel: strict` sur une version épinglée au moins égale à 11.15.0. Les séries temporelles et autres graphiques passent par
      ECharts quand ils s'ouvrent. Une seule bibliothèque de graphiques, pas de D3.
    - **Alternatives:** Chart.js (écarté : moins de types, séries temporelles via adaptateur, risque de changer de bibliothèque quand
      les besoins grossiront) ; D3 (écarté : impératif — un rendu par type à écrire et à maintenir, et c'est du code, pas une donnée).
    - **Consequences:** import modulaire d'ECharts à mesurer côté bundle ; liste blanche des types de série et rejet des fonctions ;
      `mermaid` s'ajoute au registre des types livrés de `app/dashboards` (onze types).
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-5 - Les nouveaux types s'ouvrent un à un par le registre**
    - **Decision:** un type à données en ligne s'ajoute par une entrée du registre (nom, schéma de `config`, source `Inline`, indicateur
      scriptable pour un graphique), un composant frontend et ses tests, sans toucher à l'exécution ni au cache.
    - **Source:** `DEC-1`

- **DEC-6 - Le script n'est jamais fourni par le client**
    - **Decision:** la route d'exécution reçoit l'identité du widget, pas son script ; l'application le relit dans la définition de
      l'utilisateur authentifié. Un client ne peut donc ni exécuter un script arbitraire ni exécuter celui d'autrui.
    - **Source:** `app#BR-4`, `app/dashboards#CON-2`

## Réalisation SpecDD

Specs à créer ou à amender (une spec par fichier) :

- `app/src/widgets/{mod,runtime,output,cache}.sdd` : route d'exécution (`FR-2`), appel du `ScriptRunner` et configuration des plafonds
  (`FR-3`, `FR-5`), validation de la sortie (`FR-4`), cache (`FR-6`), erreurs (`FR-7`).
- `app/src/dashboards/{registry,mod,model}.sdd` : `scripted` devient un type connu (source `Scripted`) ; nouveau type `mermaid` ; indicateur
  scriptable des graphiques ; règles de `config` de `FR-1` ; refus de `scripted` dans un éphémère (`FR-9`) ; grammaire des types
  inline (`DEC-1`).
- `app/src/sessions/catalog.sdd` : `script_grants` des types de contexte publiés (`OI-3`).
- `app/src/config.sdd`, `app/src/api/mod.sdd`, `app/src/main.sdd`, `app/src/error.sdd` : configuration des plafonds, route, montage du
  `ScriptRunner` (registre des ressources et des modules d'hôte), variantes `VNL-APP-WDG-*`.
- `packages/protocol` : types de la réponse d'exécution (générés par ts-rs).
- Frontend : composants `bar`, `pie` (ECharts), `markdown` et `mermaid` du registre de rendu ; hôte de widget scripté (états prêt, en cours, échec ;
  actualisation et interruption) ; une spec par composant.

## Acceptation et vérification

- **AC-1:** Un script qui rend un diagramme en barres, puis un diagramme circulaire, les affiche ; un widget `markdown` et un widget `mermaid`
  affichent leur contenu directement, sans script.
- **AC-2:** Un script qui appelle un module, un toolset ou un serveur MCP que le type du contexte n'accorde pas, ou que l'utilisateur ne
  peut pas utiliser, échoue comme pour un élément inexistant ; deux utilisateurs obtiennent chacun le résultat de leurs droits.
- **AC-3:** Une sortie hors contrat (type inconnu ou non graphique, `config` hors schéma, HTML) produit un état d'erreur `VNL-APP-WDG-*` et les autres
  widgets du tableau s'affichent.
- **AC-4:** Un script qui boucle est interrompu au plafond de durée ou d'opérations configuré ; un résultat qui dépasse le plafond de taille
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

- **OI-3 - `script_grants` des types de contexte et ressources visibles.** Le catalogue publié par l'application (`G_home`, `G_config`,
  `G_project`, `G_sandbox`) ne porte aucun `script_grants` : sans eux, un script ne voit aucun toolset ni module d'hôte. À décider : ce que
  chaque type accorde par défaut (rien, ou des toolsets de base) ; quels `HostModule` l'application enregistre ; quelles entités le registre
  de ressources du `ScriptRunner` expose à la lecture et à l'écriture (un script y agit sous les droits de l'utilisateur).
- **OI-4 - Jeton `forward`.** `ScriptRunner::run` accepte le jeton de l'utilisateur pour les serveurs MCP en mode `forward`. L'application
  détient-elle ce jeton lors d'une exécution déclenchée par l'affichage (session par cookie) ? Sinon ces serveurs restent inutilisables
  dans un widget, comme dans un workflow batch.
- **OI-5 - Widgets d'action.** Un widget qui agit (valider une MR) est voulu et le `ScriptRunner` ne restreint pas les effets. **TBD :** à
  concevoir — distinguer l'exécution déclenchée par l'affichage (cachée, rejouable) de l'exécution déclenchée par un clic (non cachée,
  explicite), pour qu'un effet ne se rejoue pas à chaque ouverture périmée de la page.
- **OI-6 - Valeurs par défaut des plafonds de `FR-5`.** Ce sont des réglages de l'application (variables d'environnement du déploiement),
  pas des bornes du widget : défauts proposés, larges, à ajuster à l'usage — durée 5 min, résultat 1 Mio, opérations illimitées (`0`).
  Aucun plafond sur le nombre d'appels MCP ni sur `ttl_secs`. Les défauts de la crate (5 s, 1 000 000 opérations, 256 Kio) sont trop
  serrés pour un widget qui appelle un serveur MCP.
- **OI-7 - Grammaire des types inline.** Les schémas `config` de `bar` et `pie` sont déclaratifs et non vérifiés à l'écriture
  (`app/dashboards`) ; `DEC-1` exige de les vérifier pour les sorties de script (graphiques). À trancher : validation par schéma JSON dans l'application
  (dépendance à choisir) ou validation Rust par type.

- **OI-8 - Markdown ou diagramme calculé par un script.** `app#FR-4` demande qu'un widget scripté produise « au minimum du contenu
  markdown » ; la première version le restreint aux graphiques (instruction du 2026-11-13 : le texte d'un widget `markdown` est un paramètre,
  pas le produit d'un script). Un texte calculé à partir d'un MCP (une liste de MR ouvertes en tableau markdown) n'est donc pas possible.
  **TBD :** à rouvrir si le besoin apparaît ; amendement de `app#FR-4` à confirmer.

## Références

- `dsd/app.md` : `BR-3` à `BR-5`, `FR-3` à `FR-5`, `NFR-1`, `CON-1`, `DEC-5`, `Risques`.
- `dsd/app/dashboards.md` : `CON-1`, `FR-3`, `FR-4`, `FR-6`, `FR-9`, `DEC-3`, `DEC-6`.
- `dsd/app/sessions.md` : `DEC-1`, `DEC-6`, `DEC-9`.
- miryad-core 0.3.1 : `dsd/miryad-core/scripts-hote.md` (`FR-4`, `FR-5`, `FR-7`, `DEC-1` à `DEC-6`, `OI-3` à `OI-6`), `src/lymaille/script_runner.rs`.
- `app/src/workflow/steps/rhai.sdd`, `app/src/sessions/host.sdd`, `app/src/sessions/catalog.sdd`, `app/src/dashboards/registry.sdd`.
- Rendu : documentation d'Apache ECharts (option JSON) ; avis de sécurité Mermaid de 2026 (`CVE-2026-41149`).
