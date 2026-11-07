# Solution Design: Chat en terminal

## Résumé

Cette Solution décrit le chat interactif de la CLI : un TUI ratatui qui ouvre une session lymaille sur un contexte de
travail, affiche le déroulé d'un tour (texte, exécutions de script, tâches, avertissements) et laisse l'utilisateur
converser, annuler, reprendre. Le TUI actuel est conservé ; son backend (`vanyline-lib`, `ChatEvent`) est remplacé par
les événements de lymaille.

## Problème et objectif

Le chat actuel dépend de `vanyline-lib` et du vocabulaire `ChatEvent`, que `app#DEC-1` retire, et suppose des outils
nommés (`read_file`, ...) alors que l'agent de lymaille agit surtout par scripts Rhai. Il faut garder l'interface (saisie,
rendu markdown, défilement, repli) et changer ce qu'elle affiche et comment elle pilote la session (`cli#DEC-2`, `DEC-8`).

## Périmètre et non-objectifs

Couvre : `vanyline chat`, l'architecture du TUI, l'adaptateur d'événements, les commandes slash, la reprise de session.

Hors périmètre : l'hôte lymaille et ses stores (Solution `cli/hote`), `run` et `script` (Solution `cli/batch`), la
synchronisation de `.vanyline/` (Solution `cli/sync`), et le serveur RPC (`cli#OI-2`).

## Contexte applicable du domaine

`cli#FR-2` (commande `chat`), `cli#DEC-2` (hôte permanent), `cli#DEC-5` (contexte = Project), `cli#DEC-8` (script replié et
ligne de statut), `cli#CON-1` (terminal requis). `agent-sessions#BR-9` : l'agent découle du contexte, aucune approbation
en cours de session.

## Acteurs

- **Développeur :** converse avec l'agent d'un contexte, corrige en route, annule, reprend.
- **Agent (LLM) :** conduit le tour ; ses exécutions de script et ses tâches sont rendues, jamais pilotées par le terminal.

## État actuel et état cible

- **Actuel (Observed in: `cli/src/tui/`, `cli/src/chat.rs`) :** architecture « état pur + vue » (`state`, `view`, `editor`,
  `markdown`, `sink`, `mod`), événements `ChatEvent`, outils nommés, approbation de commande, résumé git local.
- **Cible :** même architecture, événements `Event` de lymaille, exécutions de script, pas d'approbation, une session
  durable ouverte par `cli/hote`.

## Hypothèses, contraintes et dépendances

- **CON-1:** L'état et la vue restent purs (aucune E/S, aucune horloge dans `update` ni `render`) ; seul `mod` touche au
  terminal, pour garder le TUI testable avec le `TestBackend` de ratatui.
    - **Source:** `SpecDD /cli/src/tui/mod.sdd` (état actuel), conservé

- **CON-2:** Le contenu venu du modèle ne peut jamais émettre de séquence d'échappement vers le terminal.
    - **Source:** `SpecDD /cli/src/tui/markdown.sdd` (état actuel), conservé

## Exigences fonctionnelles

- **FR-1 - MUST:** `vanyline chat --context <projet> [--sandbox <nom>]` ouvre une session par `cli/hote`
  (`hote#FR-7`) et lance le TUI. Sans terminal sur l'entrée et la sortie standard, la commande échoue avec une erreur
  d'usage.
    - **Source:** `cli#FR-2`, `cli#CON-1`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Entrée envoie la saisie non blanche. Pendant un tour actif, le message n'ouvre pas de nouveau tour : il
  est injecté dans le tour en cours par l'entrée utilisateur de lymaille, et la transcription le marque comme envoyé
  pendant le tour.
    - **Source:** décision du développeur, 2026-11-07 ; `agent-sessions/session#FR-14`
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** Ctrl+C pendant un tour l'annule par le handle d'annulation de lymaille et l'indique dans la
  transcription. Au repos, Ctrl+C vide la saisie ; deux Ctrl+C consécutifs sur saisie vide quittent.
    - **Source:** `agent-sessions/session#FR-5` ; comportement actuel du TUI conservé
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** Le texte de l'agent s'affiche au fil de l'eau, en markdown, à partir des fragments de texte du tour ; le
  texte complet fait foi à la fin du tour.
    - **Source:** `agent-sessions/session` (événements `TextDelta` et `TurnEnded`)

- **FR-5 - MUST:** Une exécution de script s'affiche en une ligne (fonctions MCP appelées, durée, succès ou erreur) ; le
  script complet et sa sortie se déplient à la demande. La source de ces lignes est l'événement d'audit du script.
    - **Source:** `cli#DEC-8` ; `agent-sessions/session#FR-8`
    - **Verified by:** `AC-4`

- **FR-6 - MUST:** Le panneau des tâches suit la todo de la session : il est mis à jour à chaque événement de changement de
  la todo, et Ctrl+T l'affiche ou le masque. Un changement de mémoire apparaît comme une notice (niveau et identifiant,
  jamais le corps).
    - **Source:** `agent-sessions/modules` (todo, Memory) ; comportement actuel conservé

- **FR-7 - MUST:** Les événements d'état de l'inférence et des connexions (nouvelle tentative, reconnexion MCP, dépassement
  de délai) et toute erreur de tour sont des notices visibles. Aucune erreur n'est silencieuse.
    - **Source:** `agent-sessions/session#FR-6`, `FR-7` ; comportement actuel conservé

- **FR-8 - MUST:** Les commandes slash sont `/help`, `/new`, `/todo`, `/sessions` et `/quit`. `/agent` n'existe pas : l'agent
  découle du contexte. Une commande inconnue produit une notice, jamais un tour ; un message commençant par `/` s'écrit
  `//`.
    - **Source:** décision du développeur, 2026-11-07 ; `agent-sessions#BR-9`
    - **Verified by:** `AC-5`

- **FR-9 - MUST:** Sans option, `chat` ouvre une session neuve. `-c/--continue` reprend la dernière session du contexte,
  `--resume <identifiant>` une session précise ; `/sessions` liste celles du contexte (index de `cli/hote`, `hote#FR-9`) et
  en reprend une.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-6`

- **FR-10 - MUST:** Aucune demande de confirmation n'est posée en cours de session : les limites de l'agent sont celles de
  son contexte.
    - **Source:** `app#DEC-3`, `agent-sessions#BR-9`

- **FR-11 - MUST:** À la sortie, le terminal est restauré dans tous les cas (normale, erreur, panique, signal), puis une
  dernière ligne donne l'identifiant de la session et la commande de reprise.
    - **Source:** comportement actuel conservé (`SpecDD /cli/src/tui/mod.sdd`)
    - **Verified by:** `AC-7`

## Conception

Le TUI garde ses six modules. La frontière avec lymaille est un `EventSink` (`TuiSink`) qui transmet les `Event` de la
session à la boucle sous forme d'`Action`. Le modèle de transcription change : l'élément « appel d'outil » devient
« exécution de script » (`FR-5`), l'élément « sous-agent » est conservé tant que les événements de délégation le permettent
(`OI-2`). La boucle lance un tour sur un `TurnRunner` de lymaille dans une tâche dédiée ; un seul tour à la fois (`FR-2`
passe par l'entrée utilisateur, pas par un second tour). L'éditeur et le rendu markdown ne changent pas.

## Décisions

- **DEC-1 - L'architecture du TUI est conservée, son vocabulaire d'événements est remplacé**
    - **Decision:** `state`, `view`, `editor`, `markdown`, `sink` et `mod` restent ; `editor` et `markdown` sont inchangés,
      `state`, `view`, `sink` et `mod` passent des `ChatEvent` aux `Event` de lymaille.
    - **Alternatives:** réécrire le TUI (écarté : le comportement testé de l'éditeur et du rendu est la partie chère à
      reconstruire, et rien ne l'invalide).
    - **Source:** `cli#DEC-2`

## Réalisation SpecDD

À adapter : `SpecDD /cli/src/tui/state.sdd`, `SpecDD /cli/src/tui/view.sdd`, `SpecDD /cli/src/tui/sink.sdd`,
`SpecDD /cli/src/tui/mod.sdd`. Inchangées : `SpecDD /cli/src/tui/editor.sdd`, `SpecDD /cli/src/tui/markdown.sdd`. À
remplacer : `SpecDD /cli/src/chat.sdd` (la boucle REPL et le one-shot passent à `cli/batch` ; l'ouverture de session passe
à `cli/hote`), la partie chat de `SpecDD /cli/src/main.sdd` et `SpecDD /cli/src/tools.sdd`.

## Compatibilité et migration

Le mode REPL non plein écran disparaît avec `chat.rs` : `run` couvre l'usage sans TUI. Les options `--toolbox`, `-m`,
`-t` et `--agent` du chat sont retirées ou déplacées (`--toolbox` devient `--sandbox`, `-m` et `-t` vont à `run`).

## Acceptation et vérification

- **AC-1:** `chat --context <projet>` sans terminal échoue avec une erreur d'usage ; avec un terminal, la session s'ouvre
  sur le contexte demandé.
- **AC-2:** Un message envoyé pendant un tour actif n'ouvre pas de second tour et apparaît dans la transcription marqué
  comme envoyé pendant le tour.
- **AC-3:** Ctrl+C pendant un tour l'annule, la transcription garde le texte déjà reçu, et le TUI repasse au repos.
- **AC-4:** Un tour qui exécute un script affiche une ligne de statut ; la déplier montre le script complet et sa sortie.
- **AC-5:** `/agent` produit une notice « commande inconnue » ; `//ceci` envoie le message `/ceci`.
- **AC-6:** `chat -c --context <projet>` reprend la dernière session de ce contexte avec son historique affiché.
- **AC-7:** Un signal ou une panique restaure le terminal avant tout autre affichage.

## Open Issues

- **OI-1:** **Résumé git dans la barre d'état.** Le TUI actuel lance `git diff --shortstat` dans le répertoire courant ;
  avec une sandbox, le code modifié est dans son worktree, pas en local. Source du résumé : le répertoire local, le relais
  git de la sandbox (`/git/*`), ou rien. **TBD :** non repris tant que non tranché.
- **OI-2:** **Raisonnement et sous-agents.** Je n'ai pas relevé dans `EventKind` de lymaille d'événement de fragment de
  raisonnement ni de progression d'un sous-agent (la délégation a ses propres types : `RunProgress`, `WatchEvent`).
  L'affichage replié du raisonnement et l'imbrication des sous-agents dépendent de ce qui est exposé. **TBD.**
- **OI-3:** **Tour annulé.** Le TUI actuel ne persiste pas un tour annulé ; lymaille a sa propre règle de sauvegarde
  d'état. Aligner la CLI sur lymaille, ou garder la règle actuelle. **TBD.**

## Références

`cli` (`cli#FR-2`, `DEC-2`, `DEC-5`, `DEC-8`), `cli/hote`, `agent-sessions` de lymaille (`session`, `modules`),
`SpecDD /cli/src/tui/mod.sdd`.
