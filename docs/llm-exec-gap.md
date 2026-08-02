# Écart validé — remplacer opencode comme backend de `llm-exec`

Statut : **validé** (Sébastien + Claude, session d'étude, 2026-08-02).
Produit de l'étude `docs/features/ws14-cli-backend-llm-exec.md` (supprimé en phase 3).
Ce document devient le **backlog du sprint 3** où le remplacement effectif a lieu.

## Méthode appliquée

1. Relevé d'usage réel : 123 fichiers de tâches `.tasks/` (sprints 1-2) + sessions
   documentées dans `.claude/MEMORY.md` — quels tools opencode ont effectivement servi.
2. Lecture du comportement headless `opencode run` (docs opencode 1.15.10 + wrapper
   `/usr/local/bin/llm-exec`).
3. Session de travail développeur + Claude : chaque ligne du tableau tranchée, priorisée.

## Référentiel (relevé 2026-07-12, recoupé)

Tools LLM opencode (`packages/core/src/tool`) : `bash, read, write, edit, apply-patch,
glob, grep, skill, todowrite, question, webfetch, websearch`.

### Déjà couvert par vanyline

| opencode | vanyline | État |
|----------|----------|------|
| `bash` | `execute_command` (cli/src/tools.rs) | couvert |
| `read` | `read_file` | couvert |
| `write` | `write_file` | couvert |
| `edit` | `edit_file` | couvert |
| `glob` | `find_files` | couvert |
| `grep` | `search` | couvert |
| `skill` | builtin `skill` (lib/src/builtin/skill.rs) | couvert |

Bonus vanyline sans équivalent opencode listé : `task` (subagents,
lib/src/builtin/task.rs), `delete_file`, `list_directory`.

### Usage réel (sprints 1-2)

Les fichiers de tâches et les sessions documentées montrent que Qwen n'a utilisé
en pratique que : `bash` (git add/check/test), `read`, `write`, `edit`, `glob`,
`grep` — **tous déjà couverts**. `webfetch`, `websearch`, `apply-patch` n'ont
**jamais** servi ; `question` n'apparaît que comme texte de fin de tour (pas un
appel d'outil, confirmé 3× dans MEMORY), jamais comme tool.

**Correction de la développeur (source de vérité)** : `todowrite` est utilisé
**énormément** par Qwen en session interactive — à implémenter, contrairement au
relevé initial qui ne voyait que les specs (les fichiers `.tasks` sont des prompts,
pas des logs d'usage).

## Décisions tranchées (tableau « À étudier cas-par-cas »)

| Item | Décision | Justification |
|------|----------|---------------|
| `todowrite` | **Implémenter** | Usage réel massif. État **dans la conversation** (persistée, résumé via `-c/--continue`) — seule forme d'état resumable en une-passe. Accompagné de `todoread` (lecture de l'état) — opencode les groupe sous une seule permission. |
| `question` | **Absent** | En headless une-passe il n'y a pas d'utilisateur à qui poser la question. opencode `question: deny` = retiré de l'index des tools. En `ask` il s'auto-rejette et peut **casser la session** (vu en réel) — à ne pas reproduire. |
| `webfetch` | **Non implémenté** | Aucun usage en sprints 1-2. Aucune politique réseau définie depuis le Strix. À re-évaluer s'il faut donner du réseau aux agents (dépend d'une décision réseau). |
| `websearch` | **Non implémenté** | Dépend d'un provider de recherche (Exa/opencode). Inadapté à un backend d'exécution. |
| `apply-patch` | **Non implémenté** | Aucun usage réel. `edit_file` (recherche exacte) suffit et les SLM ratent souvent le format patch. |

## L'écart hors-tools (le plus important)

### 1. Interface d'invocation

`llm-exec -m <model> -a <agent> -t <timeout> "prompt"` wrap `opencode run
--agent --model --format`. État vanyline (`cli/src/main.rs`, `cli/src/chat.rs`) :

| Flag llm-exec | vanyline | Écart |
|---------------|----------|-------|
| `-a/--agent` | `-a/--agent` ✓ | couvert |
| mode non-interactif une-passe | `run <message>` ✓ (run_one_shot) | couvert |
| `-m/--model` | absent | **à ajouter** : override le modèle de l'agent pour ce run (sans toucher la config) |
| `-t/--timeout` | absent | **à ajouter** : timeout global du run (équivalent du `timeout N` du wrapper) |
| `-j/--json` | absent | **à ajouter** : sortie structurée (équivalent `--format json`) |

### 2. Définition des agents

`.opencode/agents/*.md` → `.vanyline/agents/*.md` (même format frontmatter, déjà
supporté par vanyline : `cli/src/fs_store.rs::RawAgentFrontmatter`).

| opencode frontmatter | vanyline | Mapping |
|----------------------|----------|---------|
| `description` | `description` | ✓ direct |
| `mode` (`primary/subagent/all`) | `mode` (`AgentMode`) | ✓ direct |
| `model` | `model` | ✓ direct |
| `temperature` | sur `ModelProfile` (config.yaml), pas l'agent | **table de correspondance** : à décider sprint 3 (override par agent ou ignoré) |
| `permission` | **aucun** | **sans objet — philosophie yolo**. À ne pas reproduire (voir irritants) |
| `steps` | `max-turns` interne (DEFAULT_MAX_TURNS=100) | **ne pas exposer** (voir décision) |
| `color` | — | UI only, sans objet backend |
| `disable`/`hidden`/`top_p` | — | sans équivalent |

### 3. Sortie

`llm-exec` consomme le **stdout du `run`** (`--format default`, texte formaté) +
`git diff --stat` ajouté par le wrapper. Pas de logs parsés.

vanyline `run` produit déjà l'équivalent via `StdoutSink` (cli/src/chat.rs) :
`Token` → texte, `ToolCall` → ligne `[tool] name(args)`, `Done` → nouvelle ligne.
**Couvert** ; le `git diff --stat` est un ajout du wrapper, à reproduire au sprint 3.

### 4. Injection de contexte

`AGENTS.md` workspace : **déjà fait** côté vanyline (`read_workspace_context`,
cli/src/chat.rs:172 → `run_agent_turn`).

### 5. Comportement session

- Reprise : opencode `-c` (dernière session) / `-s` (session précise) / `--fork`.
  vanyline `-c/--continue` (conversation active) + `conversations set <id>` ✓.
  **Couvert**, pas de `--fork` (pas besoin pour un backend d'exécution).
- Limites de tours : opencode `steps` → vanyline `DEFAULT_MAX_TURNS=100`
  (lib/src/session.rs:283) est un **filet anti-boucle interne**, pas un plafond
  de travail. Décision : **ne pas l'exposer** (jamais utilisé en réel ici).

### 6. Les irritants opencode qu'on veut PERDRE (« avantageusement »)

| Irritant opencode | Cause | Ce que vanyline doit faire |
|-------------------|-------|----------------------------|
| Sessions cassées par les env héritées (`OPENCODE_SERVER_PASSWORD`, `OPENCODE_BINARY` → "Session not found") | le wrapper `llm-exec` doit `env -u ...` | **ne pas dépendre de ces variables** : le CLI vanyline est un binaire local, pas un serveur — aucune dépendance à reproduire ni contourner |
| Permissions bash inopérantes en headless (Qwen ne peut pas s'auto-valider) | `ask` s'auto-rejette en `run`, sans `--auto` | **aucun système de permission** (philosophie yolo) — l'agent a accès direct |
| Une permission auto-rejetée casse toute la session `run` (diff vide) | auto-reject → plantage | pas de permissions → **pas de rejet qui casse une session** |

Ce sont les « avantageusement » de l'objectif : le remplacement perd ces trois
frictions de headless.

## Backlog sprint 3 — features candidates (priorisées)

### P0 — Interface d'invocation (le blocage de substitution)

- **run-flags** : ajouter `-m/--model` (override modèle de l'agent), `-t/--timeout`
  (timeout global), `-j/--json` (sortie structurée) à `vanyline run`.
  Périmètre : `cli/src/main.rs` + `cli/src/chat.rs`. Tests : smoke sur les flags.

### P1 — Agents

- **agent-mapping** : table de correspondance `.vanyline/agents/*.md` — décider le
  sort de `temperature` (override par agent vs ignoré). `permission`/`color`/
  `disable`/`hidden`/`top_p` explicitement hors périmètre (yolo / UI).
  Périmètre : `cli/src/fs_store.rs` + doc.

### P1 — Outil todowrite

- **builtin-todo** : `todowrite` + `todoread`, état **dans la conversation**
  (résumé via `-c`). Périmètre : `lib/src/builtin/` (nouveau module), index des
  tools, `cli/src/tools.rs`.

### P2 — Sortie

- **run-output** : reproduire le `git diff --stat` du wrapper en fin de `run`
  (comportement, pas un tool). Périmètre : `cli/src/chat.rs`.

### Hors périmètre (décidé)

- `question`, `webfetch`, `websearch`, `apply-patch` : non implémentés.
- `max-turns` non exposé ; `--fork` non requis.

## Risques et questions ouvertes

- Le repo opencode évolue (branche dev) — le relevé ci-dessus est figé sur la doc
  1.15.10 + le wrapper `/usr/local/bin/llm-exec` relevés 2026-08-02.
- `todowrite` : l'état dans la conversation est la décision retenue (seule forme
  resumable en une-passe) ; alternative (état global hors conversation) rejetée.
- `webfetch` : re-évaluer uniquement si une politique réseau Strix est décidée.
- Scope creep : tout « tiens, c'est facile » passe par la case décision ensemble.
