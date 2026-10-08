//! Tools builtins du harnais, exposés au LLM en plus des local tools et des
//! tools MCP (specs sœurs `./skill.sdd`, `./task.sdd`, `./todo.sdd`).

/// Tool builtin `skill` : chargement du corps d'un skill par son nom.
pub mod skill;
/// Tool builtin `task` : délégation de tâche à un subagent (tour imbriqué).
pub mod task;
/// Tools builtins `todowrite`/`todoread` : état todo partagé de la session.
pub mod todo;
