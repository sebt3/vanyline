# harness-core — cœur LLM/MCP name-keyed (terminé)

Refonte complète de `vanyline-lib` : domaine name-keyed (`Provider`, `ModelProfile`,
`McpServer`, `Toolset`, `Agent`, `SkillMeta` — plus d'UUID exposé), `ConfigStore` (trait de
résolution par nom), `ChatEvent`/`EventSink` (un seul type d'événement pour REPL/WS/futur
JSON-RPC), `SessionContext`/`run_agent_turn` (point d'entrée unique), tools builtin
`skill`/`task` (subagents avec garde de profondeur). `cli/` et `app/` migrés dessus
(`CliConfigStore` adapte les fichiers JSON existants, `PgConfigStore` adapte le schéma PG
existant — aucun des deux n'a introduit de nouveau stockage, adaptation mécanique
uniquement). Ancien cœur (`ChatSink`/`run_chat_turn`/types UUID-keyed) supprimé. Détails :
`docs/architecture.md` (section "Session engine"). Stratégie qui a bien fonctionné : tâches
additives strictes (nouveaux modules, jamais toucher l'existant) jusqu'à une tâche finale de
bascule mécanique — le workspace est resté vert après chaque tâche, permettant une revue
incrémentale fiable. Dette assumée et documentée (pas streaming WS live, pas
d'annulation, historique appauvri) plutôt que du scope creep pour "bien faire tout de suite".
