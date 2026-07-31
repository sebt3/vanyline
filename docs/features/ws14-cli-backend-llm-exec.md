# Feature — ws14-cli-backend-llm-exec (étude)

## Ce que la feature fait

Étude d'écart : ce qui manque au CLI vanyline pour remplacer opencode comme
backend de `llm-exec`. Livrable = un doc d'écart validé ensemble, qui devient le
backlog du sprint 3 (où le remplacement effectif aura lieu).

## Ce qu'elle ne fait pas

- **Aucune implémentation ce sprint** (sauf si un item se révèle trivial et
  qu'on décide ensemble de le prendre en avance)
- Pas de modification de `llm-exec` lui-même (il sera adapté au sprint 3)

## Référentiel (relevé 2026-07-12)

Tools LLM d'opencode (`packages/core/src/tool`) : `bash, read, write, edit,
apply-patch, glob, grep, skill, todowrite, question, webfetch, websearch`.

Déjà couvert par vanyline : `bash`→execute_command, `read`→read_file,
`write`→write_file, `edit`→edit_file, `glob`→find_files, `grep`→search,
`skill`→builtin skill. Bonus vanyline sans équivalent listé : `task`
(subagents), `delete_file`, `list_directory`.

## À étudier cas-par-cas (ensemble)

| Item | Question à trancher |
|------|---------------------|
| `todowrite` | Utile à Qwen pour se structurer sur des tâches longues ? Coût faible, valeur probable — mais l'état vit où (conversation ?) |
| `question` | En headless c'est forcément dégradé (opencode `run` ne peut pas poser de question non plus — vérifier ce qu'il en fait). No-op ? Erreur explicite ? Absent ? |
| `webfetch` | Les agents implement/diagnose s'en servent-ils réellement ? Si oui : quelle politique réseau depuis le Strix ? |
| `websearch` | Idem + dépend d'un provider de recherche — probablement non pour un backend d'exécution |
| `apply-patch` | Plus puissant qu'edit_file mais les SLM ratent souvent le format patch — à trancher sur données (les sessions Qwen existantes utilisent-elles apply-patch ?) |

## L'écart hors-tools (le plus important)

- **Interface d'invocation** : `llm-exec -m <model> -a <agent> -t <timeout>
  "prompt"` → équivalent vanyline (`vanyline run -a <agent> ...` existe ; mapper
  le choix de modèle par flag, le timeout, le mode non-interactif une-passe)
- **Définition des agents** : `.opencode/agents/*.md` → `.vanyline/agents/*.md`
  (formats frontmatter très proches — table de correspondance à écrire :
  `mode`, `permission`, `steps` n'ont pas d'équivalent vanyline ; `permission`
  est même sans objet, philosophie yolo)
- **Sortie** : que consomme `llm-exec` (stdout final ? logs ?) — le mode `run`
  du CLI doit produire l'équivalent
- **Injection de contexte** : AGENTS.md workspace — déjà fait côté vanyline
- **Comportement session** : reprise, contexte, limites de tours (`steps`
  opencode) — le max-turns vanyline est un défaut interne, faut-il l'exposer ?
- **Les irritants opencode qu'on veut PERDRE** : sessions cassées par les env
  vars héritées (`OPENCODE_SERVER_PASSWORD`…), permissions bash inopérantes en
  headless (Qwen ne peut pas s'auto-valider) — le doc d'écart liste aussi ce
  que vanyline doit *ne pas* reproduire, c'est le "avantageusement" de
  l'objectif

## Méthode

1. Relever l'usage réel : les fichiers de tâches et sessions Qwen du sprint 1-2
   (quels tools ont effectivement servi)
2. Lire le comportement headless d'opencode `run` sur les points ambigus
   (`question`, sortie, steps)
3. Une session de travail développeur + Claude : trancher chaque ligne du
   tableau, prioriser
4. Livrable : `docs/llm-exec-gap.md` (l'écart validé, priorisé, découpé en
   features candidates sprint 3) ; ce design doc est ensuite supprimé (phase 3)

## Risques et questions ouvertes

- Le repo opencode évolue (branche dev) — figer le relevé sur un commit précis
  au moment de l'étude
- Risque de scope creep : l'étude doit rester une étude ; tout "tiens, c'est
  facile" passe par la case décision ensemble
