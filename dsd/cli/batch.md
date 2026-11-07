# Solution Design: Usage batch de la CLI

## Résumé

Cette Solution décrit les deux commandes non interactives de la CLI : `run`, un tour de conversation avec LLM dont on
lit la réponse, et `script`, l'exécution d'un fichier Rhai sans LLM sur une sandbox. Elles servent à enchaîner la CLI
dans un script shell, une tâche planifiée ou une CI.

## Problème et objectif

Le développeur veut automatiser sans ouvrir le TUI : poser une question ou une tâche à l'agent d'un contexte et récupérer la
réponse (`run`), ou rejouer une opération déterministe sur une sandbox avec les mêmes modules que l'agent (`script`). Le
`run` actuel dépend de `vanyline-lib` et de `ChatEvent` (`cli#DEC-2`) ; `script` est nouveau (`cli#BO-3`).

## Périmètre et non-objectifs

Couvre : `run`, `script`, leurs entrées et sorties, leurs codes de sortie, leurs délais.

Hors périmètre : le chat interactif (Solution `cli/chat`), l'hôte (Solution `cli/hote`), `run_batch` de lymaille
(boucle jusqu'à un résultat structuré), réservé à la délégation et aux workflows d'`app` : la CLI ne l'expose pas.

## Contexte applicable du domaine

`cli#FR-3` (`run`), `cli#FR-4` (`script`), `cli#BR-1` (jeton d'Owner), `cli#BR-3` (un script n'a pas plus de pouvoir qu'une
session du contexte), `cli#DEC-5` (contexte = Project). `agent-sessions#BR-9` : l'agent découle du contexte, aucune
approbation en cours de session.

## Acteurs

- **Développeur ou orchestrateur** (script shell, CI) : lance la commande et lit sa sortie et son code de retour.
- **Agent (LLM)** : conduit le tour de `run`. Absent de `script`.

## État actuel et état cible

- **Actuel (Observed in: `cli/src/chat.rs`, `cli/src/main.sdd`) :** `run <message> [-m] [-t] [-j]`, sortie texte ou JSONL de
  `ChatEvent`, tout échec en code 1, timeout porté par l'erreur `VNL-CLI-001`.
- **Cible :** les mêmes usages sur lymaille, entrée par argument ou entrée standard, codes de sortie distincts, commande
  `script`.

## Hypothèses, contraintes et dépendances

- **CON-1:** Lymaille n'interrompt jamais un tour qui dépasse son délai : elle émet `Overrun`. Interrompre au délai est donc
  une décision de la CLI, par le handle d'annulation.
    - **Source:** `agent-sessions/session#FR-6`

- **CON-3:** `execute_command` a un paramètre `timeout_secs` par appel (`0` : aucun délai), annoncé « default 30 » dans son
  schéma ; une absence de paramètre donne pourtant 30 dans l'ancien CLI et aucun délai dans la sandbox
  (`SpecDD /tools/src/mcp.sdd`, tâche `[?]`). Ce défaut incohérent relève de `tools` et de `sandbox`, non de la CLI.
    - **Source:** `SpecDD /tools/src/mcp.sdd`

- **CON-2:** Une valeur de secret ne passe jamais en paramètre de ligne de commande : elle apparaîtrait dans l'historique du
  shell et la liste des processus.
    - **Source:** `cli/hote#CON-3`

## Exigences fonctionnelles

- **FR-1 - MUST:** `vanyline run <message> --context <projet> [--sandbox <nom>]` ouvre une session par `cli/hote`
  (`hote#FR-7`), exécute un seul tour avec l'agent du contexte et rend la réponse. Le message est lu sur l'entrée standard
  quand l'argument vaut `-`. Sans option la session est neuve ; `-c/--continue` reprend la dernière session du contexte et
  `--resume <identifiant>` une session précise, comme en `chat` (`chat#FR-9`). La session est sauvegardée et son
  identifiant est écrit sur la sortie d'erreur.
    - **Source:** `cli#FR-3` ; décision du développeur, 2026-11-07
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** En sortie texte, seule la réponse finale est écrite sur la sortie standard ; le déroulé (exécutions de
  script, avertissements) et les erreurs vont sur la sortie d'erreur. Avec `--json`, un événement de session de lymaille
  est écrit par ligne sur la sortie standard, dans l'ordre d'émission ; les erreurs restent sur la sortie d'erreur.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** `-t/--timeout <secondes>` borne la durée du tour : à l'échéance, la CLI annule le tour par le handle de
  lymaille. Sans cette option, aucun délai n'est imposé par la CLI.
    - **Source:** `cli#FR-3` ; `CON-1`
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** `vanyline script <fichier.rhai> --context <projet> --sandbox <nom>` exécute le script avec les seuls
  modules que le contexte accorde (`cli#BR-3`), sans LLM et sans ouvrir de session. Chaque exécution est inscrite au
  journal d'audit (`hote#FR-9`).
    - **Source:** `cli#FR-4`, `cli#BR-3`
    - **Verified by:** `AC-4`

- **FR-5 - MUST:** `--param clé=valeur`, répétable, expose chaque paire au script comme constante Rhai en lecture seule. Le
  nom doit être un identifiant Rhai valide et unique. Une valeur que le détecteur de secrets de lymaille reconnaît est
  refusée.
    - **Source:** décision du développeur, 2026-11-07 ; `CON-2`
    - **Verified by:** `AC-5`

- **FR-6 - MUST:** Le texte rendu par le script est écrit sur la sortie standard. Une erreur de script, avec sa classe et sa
  position, est écrite sur la sortie d'erreur. Avec `--json`, la sortie standard porte un seul objet : le statut et le
  texte.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-4`

- **FR-7 - MUST:** `-t/--timeout <secondes>` s'applique aussi à `script` : à l'échéance, l'exécution est annulée.
    - **Source:** `cli#FR-4`
    - **Verified by:** `AC-3`

- **FR-9 - MUST:** `script` ne fixe aucun délai par commande de la sandbox : un script (ou un agent) passe `timeout_secs`
  dans son appel d'`execute_command` quand il en a besoin. La CLI n'impose que le délai global de `-t` (`FR-7`).
    - **Source:** décision du développeur, 2026-11-07 ; `cli#FR-4`

- **FR-8 - MUST:** Codes de sortie, communs à `run` et `script` : 0 succès ; 1 échec du tour ou du script ; 2 usage invalide ;
  3 délai dépassé ; 130 interruption par l'utilisateur.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-3`

## Sécurité

`script` est écrit par un humain : le contexte ne borne pas son intention, il borne ses effets par les modules accordés et
par le jeton d'Owner (`cli#BR-3`, `cli#BR-1`). Il ne dispose d'aucun accès au système de fichiers ni au shell locaux :
tout passe par le MCP de la sandbox. Les paramètres ne transportent pas de secrets (`FR-5`).

## Conception

`run` réutilise le `TurnRunner` du chat (Solution `cli/chat`), sans TUI : un `EventSink` écrit les événements selon `FR-2`.
`script` appelle directement l'exécution de scripts de lymaille avec le même environnement d'exécution (modules accordés,
connexion MCP à la sandbox de `hote#FR-7` et `hote#FR-8`), sans passer par une session. Les deux partagent le calcul des
codes de sortie (`FR-8`) à partir de l'issue de lymaille (`Valid`, `Failed`, `Cancelled`, ou le statut du script).

## Décisions

- **DEC-1 - `run` conduit un seul tour ; `run_batch` n'est pas exposé**
    - **Context:** lymaille offre `run_batch`, qui boucle jusqu'à un résultat structuré accepté par un collecteur et exige
      un agent de nature `Sub` avec un schéma de sortie.
    - **Decision:** la CLI ne propose qu'un tour. `run_batch` reste pour la délégation et les workflows d'`app`.
    - **Alternatives:** exposer `run_batch` (écarté : un agent `Sub` et son schéma par usage) ; les deux par option
      (écarté : une règle de plus sans besoin avéré).
    - **Source:** décision du développeur, 2026-11-07

- **DEC-2 - Le délai d'une commande de la sandbox est l'affaire du script, pas de la CLI**
    - **Context:** `execute_command` prend déjà `timeout_secs` par appel ; ajouter une option de la CLI exigerait de
      surcharger les fonctions MCP générées par lymaille.
    - **Decision:** aucune option de délai par commande dans la CLI ; `-t` borne l'ensemble.
    - **Alternatives:** option `--command-timeout` qui injecte un défaut (écartée : un wrapper de plus) ; attendre un défaut
      unique dans la sandbox (écartée : bloquerait `script`).
    - **Source:** décision du développeur, 2026-11-07

## Réalisation SpecDD

À écrire sous `cli/src/` (**ASSUMPTION** de découpage) : une spec pour `run`, une pour `script`, une pour les codes de
sortie. À retirer ou réduire : la partie one-shot de `SpecDD /cli/src/chat.sdd` et la grammaire de `run` dans
`SpecDD /cli/src/main.sdd`.

## Compatibilité et migration

`--json` change de contenu : il porte les événements de lymaille, plus les `ChatEvent`. Les consommateurs actuels de ce flux
sont à prévenir. `-m` désigne un profil de modèle de lymaille (**ASSUMPTION** : à confirmer à l'écriture de la spec). Les
timeouts cessent d'être portés par `VNL-CLI-001` mais par le code de sortie 3.

## Acceptation et vérification

- **AC-1:** `run -` avec un message sur l'entrée standard exécute un tour, écrit la réponse sur la sortie standard et
  l'identifiant de session sur la sortie d'erreur.
- **AC-2:** `run --json` écrit un événement valide par ligne ; la sortie standard n'a pas d'autre contenu.
- **AC-3:** Un tour ou un script plus long que `-t` est annulé et la commande sort avec le code 3 ; Ctrl+C sort avec 130.
- **AC-4:** Un script correct écrit son texte et sort avec 0 ; un script en erreur écrit la classe et la position sur la
  sortie d'erreur et sort avec 1.
- **AC-6:** `run -c` après un premier `run` sur le même contexte reprend la session : l'agent connaît l'échange précédent.
- **AC-5:** `--param api_key=<valeur ressemblant à un secret>` est refusé avant toute exécution.

## Références

`cli` (`cli#BO-3`, `FR-3`, `FR-4`, `BR-1`, `BR-3`), `cli/hote`, `cli/chat`, `agent-sessions` de lymaille (`session`,
`delegation`).
