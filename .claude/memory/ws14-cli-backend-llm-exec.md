# ws14-cli-backend-llm-exec (terminée et close, 2026-08-02)

Implémente le backlog validé dans `docs/llm-exec-gap.md` — flags `run` `-m/-t/-j`, builtin
`todowrite`/`todoread` (état **dans la conversation**, `Conversation.todo` persisté, resumé
via `-c`), mapping agents (décision `temperature` **ignorée** — single source sur
`ModelProfile`, validée par le développeur le 2026-08-02), `git diff --stat` en fin de
`run` (mode texte). 9 commits, 566 → 590 tests, 0 régression. Design doc supprimé à la
clôture ; détails migrés dans `docs/architecture.md` (section "Session engine" :
tool builtin `todo` + `SessionContext.todo_state` ; section "Configuration CLI" :
flags `run` + `git diff --stat`, table de correspondance agents déjà ajoutée par la
tâche `agent-mapping`).

**Expérience de pilotage** : comme pour l'étude qui a produit le gap doc, le développeur
fait rédiger les fichiers de tâche et prendre les décisions au fil de l'eau par DeepSeek
plutôt que Claude, sur une branche dédiée. Claude n'intervient qu'à la clôture (Phase 3) :
review des fichiers de tâche DeepSeek (contrat complet, périmètre atomique, chemins
scratch valides) ET du code Qwen produit à partir de ces fichiers (comme pour toute
feature), plus les décisions prises en cours de route qui s'écarteraient du design.
Compte de tests et format de commit revérifiés par Claude lui-même, indépendamment de
qui a rédigé la tâche.

**Bug de contrat trouvé en review** : le fichier de tâche DeepSeek `builtin-todo`
faisait `state.lock().map_err(|e| ToolError::ToolCallError(Box::new(e)))`, ce qui propage
un `PoisonError<MutexGuard>` **non-`Send`** dans le `Box<dyn Error + Send + Sync>` de
`ToolCallError` → ne compile pas. Corrigé en review (Claude, `unwrap_or_else(|e|
e.into_inner())` — récupère le guard même empoisonné), vérifié vert avant commit. Leçon :
un contrat DeepSeek peut contenir un bug de typage que Qwen appliquerait verbatim sans
le voir ; la vérification de compilation en review (Claude) reste indispensable même
quand la rédaction est déléguée.

**Second bug trouvé en review, plus grave — persistance jamais câblée** : la clôture
initiale (commit `01b073c`) affirmait l'état todo "persisté sur `Conversation.todo`...
resumé via `-c`" — faux dans le code livré. `cli/src/chat.rs` créait toujours
`todo_state` vide et ne relisait jamais l'état après le tour pour le sauver dans
`conv.todo`. À l'intérieur d'un seul `run`, `todowrite`/`todoread` fonctionnaient
(l'`Arc<Mutex>` est bien partagé sur toute la boucle d'appels d'outils du tour) ; mais
`-c/--continue` sur la même conversation repartait systématiquement avec un todo vide —
exactement la justification qui avait fait accepter `todowrite` en P1 (état resumable
en une-passe) n'était pas livrée. Aucun des 586 tests de l'époque ne couvrait ce chemin
(même pattern que le deadlock shutdown de cli-rpc-stdio : invisible en tests unitaires,
visible seulement en traçant l'intégration à la main — grep exhaustif de `conv.todo`
dans tout le code pour confirmer avant de conclure). Fix délégué à Qwen (fichier de
tâche `task-08-fix-todo-persist.md`, commit `f4dfbf9`) : `build_session_context` sème
désormais `todo_state` depuis `Conversation.todo`, `run_one_shot`/`run_repl` relisent
l'état après le tour et le sauvent (+4 tests). Qwen a par ailleurs lancé un
`cargo fmt --all` (au lieu du `--check` demandé par les commandes de validation) qui a
reformaté 17 fichiers sans rapport avec la tâche — vérifié purement cosmétique (aucun
changement logique, `fmt --all --check` repassait vert ensuite), écarté du commit
(`git restore`, resté non stagé) plutôt que mêlé au fix. Leçon : même une feature
"terminée et close" avec tests verts peut cacher un écart doc/code sur son point le
plus central si l'intégration bout-en-bout n'est jamais tracée à la main en review.

## Point ouvert issu de ws08, résolu — `svelte-check`/storybook (2026-07-31)

Branche `fix/svelte-check-skiplibcheck`. Root cause : les fichiers `*.stories.svelte`
importent `@storybook/addon-svelte-csf`, dont les types importent
`storybook/internal/types` (générique multi-renderer, touche React/Node même en usage
Svelte pur) — `frontend/tsconfig.json` ne définissait pas `skipLibCheck`, donc
TypeScript type-checkait aussi ces `.d.ts` tiers. Fix : `skipLibCheck: true` (réglage
standard pour ce cas, pas un compromis) — 70 erreurs → 0. Étape `check` réactivée dans
`.github/workflows/test.yml`, bullet retiré de `docs/architecture.md`.
