# Feature — ws14-cli-backend-llm-exec

## Ce que la feature fait

Comble l'écart validé dans `docs/llm-exec-gap.md` pour que `vanyline` puisse
remplacer opencode comme backend d'exécution de `llm-exec` :
- Flags d'invocation manquants sur `vanyline run` : `-m/--model` (override du
  modèle de l'agent pour ce run), `-t/--timeout` (timeout global), `-j/--json`
  (sortie structurée)
- Outil builtin `todowrite`/`todoread` — usage massif constaté chez Qwen en
  session interactive, état porté par la conversation
- Affichage `git diff --stat` en fin de `run` (comportement du wrapper
  `llm-exec` actuel à reproduire côté CLI)
- Table de correspondance agents `.vanyline/agents/*.md`, notamment le sort de
  `temperature` (déjà sur `ModelProfile`, pas sur l'agent — à trancher :
  override par agent ou ignoré)

## Ce qu'elle ne fait pas

- Pas de `question`/`webfetch`/`websearch`/`apply-patch` — aucun usage réel
  constaté (cf. gap doc, relevé sur les sessions sprint 1-2)
- Pas d'exposition de `max-turns` — filet anti-boucle interne
  (`DEFAULT_MAX_TURNS=100`, `lib/src/session.rs:267`), pas un plafond de
  travail
- Pas de `--fork`
- Pas de modification du wrapper `/usr/local/bin/llm-exec` lui-même (hors du
  repo vanyline — bascule infra séparée, une fois le CLI prêt)
- Pas de système de permissions/approbation des tools (philosophie yolo,
  décision déjà actée dans le gap doc)

## Interfaces clés et modules touchés

- `cli/src/main.rs` + `cli/src/chat.rs` : flags `-m`/`-t`/`-j` sur `Run`
- `lib/src/builtin/` : nouveau module `todo` (`todowrite` + `todoread`),
  suivant le pattern `skill.rs`/`task.rs` ; enregistrement dans l'index des
  tools (`cli/src/tools.rs`)
- `cli/src/fs_store.rs` (`RawAgentFrontmatter`) : décision `temperature`
- `cli/src/chat.rs` : `git diff --stat` en fin de `run_one_shot`

## Risques et questions ouvertes

- `temperature` sur l'agent : override par agent vs ignoré — à trancher au
  moment d'écrire la tâche `agent-mapping`, pas ici
- `todowrite` sans mécanisme de résumé/compaction de l'historique
  (`-c/--continue` rejoue tout en brut, `cli/src/chat.rs:183`) — non
  bloquant vu la fenêtre de contexte réelle (262 144 tokens natifs,
  vLLM recalé le 2026-08-02) et l'usage massif déjà observé, mais à
  surveiller sur des sessions interactives très longues
- La cause des compactions mi-session historiques (ws12/ws13/ws15) reste non
  identifiée malgré la correction du plafond vLLM — hors scope de cette
  feature, à surveiller si ça se reproduit
