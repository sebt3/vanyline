# chat-todo-live — persistance réelle de l'état todo côté app

## Ce que la feature fait

Rend fonctionnelle, côté `app`, la persistance de l'état todo que les outils builtin
`todowrite`/`todoread` de `vanyline-lib` manipulent déjà pendant un tour — aujourd'hui cet
état est jeté à la fin de chaque tour au lieu de survivre à la conversation.

## Contexte — comment on en arrive là

`run_agent_turn` (`lib/src/session.rs:445-451`) enregistre **inconditionnellement**
`TodoWriteTool`/`TodoReadTool` sur tout tour, quel que soit l'hôte (CLI, app, futur
JSON-RPC) — ce n'est pas un choix de toolset, ces deux outils sont toujours dans la
boîte de l'agent. `ws14-cli-backend-llm-exec` a donné à la CLI la mécanique de
persistance correspondante (`SessionContext.todo_state`, `Conversation.todo`,
seed/save autour du tour — CLI, commit `f4dfbf9`). Le cutover `app` sur
`run_agent_turn` (`harness-core`, avant ws14) puis les ajouts mécaniques de ws14
(`beb22ae`, `adb4fdb` — un seul `+1` ligne dans `app/src/ws/chat.rs` à chaque fois)
n'ont fait que garder `app` compilable : `todo_state` y est construit à `None` sur
**chaque** `handle_message`, et `app`'s `Conversation` (`app/src/db/models.rs`) n'a
jamais eu de colonne `todo`. Résultat en l'état actuel : l'agent voit `todowrite`/
`todoread` dans son contexte à chaque tour de chat `app`, peut les appeler, et l'effet
disparaît instantanément — y compris entre deux messages consécutifs de la **même**
conversation, pas seulement à la reconnexion WS.

Décidé avec le développeur (2026-08-09) : pas de flag de build pour désactiver ces
outils côté `app` (option envisagée puis écartée) — l'outil est désiré côté `app`
aussi, on le répare au lieu de le cacher.

## Ce qu'elle ne fait pas

- **Pas d'affichage temps réel dans le panneau Assistant du frontend.** Explicitement
  hors scope de cette tranche, malgré l'intérêt (le `ChatEvent::ToolCall{name:
  "todowrite", args}` part déjà tel quel sur le WebSocket — rien à changer dans
  `lib/src/event.rs` pour ça). Raison du report : `frontend/src/components/panels/
  Chat.vue` est aujourd'hui un mock complet, sans aucune connexion réseau (contrat du
  design `frontend-ui-shell.md`, § "ce qu'elle ne fait pas") — le brancher sur
  `/api/ws/chat/{conversation_id}` implique la création/sélection d'une conversation,
  la gestion de l'auth WS, la reconnexion, etc. : un périmètre à part entière, pas
  atomique avec ce correctif backend. Cette tranche se limite à rendre l'état todo
  **correctement persisté et interrogeable** ; l'affichage ira dans une feature
  ultérieure de câblage réel du panneau Assistant (aujourd'hui non planifiée, webchat =
  priorité très basse selon `.claude/MEMORY.md`).
- Ne touche pas à `lib/src/event.rs`, `lib/src/builtin/todo.rs`, ni au comportement CLI
  (déjà correct depuis `f4dfbf9`) — travail 100% côté `app`.
- Pas de flag Cargo `builtin-todo` ni aucune autre façon de désactiver ces outils
  (décidé, cf. ci-dessus).
- Pas d'UI d'édition manuelle de la todo list par l'utilisateur — écriture réservée à
  l'agent via `todowrite`.

## Interfaces clés et modules touchés

### 1. Migration — nouvelle colonne

`app/migrations/0003_conversation_todo.sql` (nouveau fichier, suit `0002_harness_parity.sql`) :

```sql
ALTER TABLE conversations ADD COLUMN todo TEXT;
```

Nullable, pas de défaut — `NULL` = aucun état todo posé, cohérent avec
`SessionContext.todo_state: Arc<Mutex<Option<String>>>` côté `lib`. Suivre le patron de
test structurel existant (`app/tests/migrations.rs`, actuellement scopé sur
`0002_harness_parity.sql` via `include_str!`) : soit étendre ce fichier avec la nouvelle
migration, soit un fichier `migrations_0003.rs` séparé — au choix de qui écrit la tâche,
tant que la structure SQL est vérifiée sans serveur DB (même approche que l'existant).

### 2. Modèle

`app/src/db/models.rs::Conversation` (actuellement `id`, `user_id`, `agent_id`, `title`,
`created_at`, `updated_at`) : ajouter `pub todo: Option<String>`.

### 3. Wiring — `app/src/ws/chat.rs::handle_message`

Deux changements, à l'intérieur de la fonction existante (signature inchangée) :

- **Avant construction du `SessionContext`** : relire `todo` en base pour CETTE
  conversation (`SELECT todo FROM conversations WHERE id = $1`), **pas** réutiliser la
  valeur de `conv` capturée à l'ouverture du WebSocket dans `run_socket` — une
  connexion WS peut porter plusieurs messages, donc plusieurs `handle_message`, et le
  todo écrit par le tour N doit être visible au tour N+1 de la même connexion. C'est
  exactement le bug que `f4dfbf9` a corrigé côté CLI (seed figé, jamais rafraîchi) ; ne
  pas le réintroduire ici sous une autre forme.
- **Après `run_agent_turn`** : relire `ctx.todo_state` (`Arc<Mutex<Option<String>>>`,
  déjà présent dans `SessionContext`) et, si `Some`, `UPDATE conversations SET todo =
  $1 WHERE id = $2`. Écrire uniquement si l'état a été touché (pas d'update
  systématique à `NULL` qui écraserait un état antérieur — utiliser un `Option` de
  changement, pas juste la valeur finale du mutex si `run_agent_turn` peut échouer
  avant d'atteindre ce point : dans ce cas ne rien persister, cohérent avec le `?` sur
  `run_agent_turn` qui empêche déjà d'atteindre la persistance du message assistant).

Remplace :
```rust
model_override: None,
todo_state: Arc::new(std::sync::Mutex::new(None)),
```
par la valeur seedée depuis l'étape précédente.

### 4. Exposition REST (mineur, cohérence)

`app/src/api/conversations.rs::ConversationOut`/`to_output` : ajouter `todo:
Option<String>` au DTO existant, même traitement défensif que `agent_name` (pas de
`.unwrap()`). Pas d'endpoint dédié — juste ne pas cacher un champ qui existe
maintenant en base.

## Risques identifiés et questions ouvertes

- **Confirmer avec le développeur avant de lancer Cadence** : le report de
  l'affichage frontend (point le plus visible de cette tranche) est ma recommandation,
  pas encore validé explicitement — si le développeur préfère inclure un branchement
  minimal de `Chat.vue` dans cette même feature plutôt que d'attendre une feature de
  câblage réel du panneau Assistant, le périmètre ci-dessus doit être révisé avant
  handoff à Cadence.
- **Coexistence avec `try_acquire_busy`** : `handle_message` tourne dans une tâche
  spawnée par message (verrou busy par conversation, cf. commentaire R4 dans
  `chat.rs`) — deux messages sur la même conversation ne peuvent déjà pas s'exécuter en
  parallèle (le busy-lock le bloque), donc pas de race sur la lecture/écriture de
  `conversations.todo` à anticiper au-delà de ce qui existe déjà.
- **Pas de rollback de migration testé** : comme `0002_harness_parity.sql`, on ajoute
  une colonne nullable sans donnée à backfiller — pas de risque de migration bloquante,
  mais à vérifier que `sqlx migrate run` reste idempotent en local avant de clore.
