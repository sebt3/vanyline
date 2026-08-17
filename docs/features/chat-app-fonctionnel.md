# Feature : chat app fonctionnel

**Statut : design initial, pas encore implémentée.**

## Ce que la feature fait

Rend le chat de l'app réellement utilisable pour une conversation humain-LLM dans le
contexte d'une sandbox : les tools de la sandbox ouverte sont effectivement
utilisables par l'agent (pas juste déclarés), les paramètres de sampling du modèle
sont réglables depuis l'UI web, et l'affichage remplace un composant de chat
humain-humain par un composant conçu pour du chat LLM (streaming, tool calls,
reasoning).

## Ce qu'elle ne fait pas

- Pas la CLI — hors périmètre de cette session, uniquement l'app web.
- Pas d'autre type de contexte que `sandbox` — le modèle de données est conçu pour
  en accueillir d'autres (ex: un contexte "settings" pour un chat d'aide au
  paramétrage) mais aucun autre type n'est implémenté ici.
- Pas de partage/déduplication des lignes `chat_contexts` entre conversations —
  chaque conversation a sa propre ligne de contexte, même si plusieurs conversations
  pointent la même sandbox.
- Pas de migration des conversations existantes vers un contexte — base de dev
  solo, pas de données de prod à préserver ; `context_id` est `NOT NULL` dès le
  départ plutôt que nullable + backfill.
- Pas de refonte du protocole `ChatEvent` en profondeur — on ajoute une variante
  (voir Interfaces), on ne réécrit pas l'existant.
- Pas d'implémentation d'un builtin toolset "aide au paramétrage" pour un futur
  contexte settings — seulement la place dans le modèle de données pour l'ajouter
  plus tard sans migration.

## Interfaces clés et modules touchés

### Axe 1 — contexte de conversation + tools sandbox

**Nouvelle table** (migration `app/migrations/`) :
```sql
chat_contexts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  kind TEXT NOT NULL,        -- 'sandbox' pour l'instant, extensible
  data JSONB NOT NULL,       -- kind='sandbox' -> { "sandbox_name": "..." }
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
)
```
`conversations.context_id UUID NOT NULL REFERENCES chat_contexts(id)` (nouvelle
colonne, `app/src/db/models.rs`).

**`POST /api/conversations`** (`app/src/api/conversations.rs`) : `CreateConversation`
gagne un champ `context: ChatContextInput { kind: String, data: serde_json::Value }`
(non optionnel). Le handler crée la ligne `chat_contexts` puis la `Conversation`.

**`GET /api/conversations`** : gagne un filtre par contexte (query param, ex.
`?sandbox=<name>`) pour que `Chat.vue` ne liste que les conversations de la
sandbox courante — aujourd'hui l'historique est global à l'utilisateur, sans
distinction de sandbox.

**Résolution du toolset** (`app/src/ws/chat.rs`, construction de `SessionContext`) :
remplace `extra_mcp: Vec::new()` par un match sur `chat_contexts.kind`. Pour
`sandbox` : résout l'URL MCP via `sandbox_mcp_url` (`lib/src/k8s.rs`, déjà utilisé
côté CLI, jamais appelé côté `app` aujourd'hui).

**Frontend** : `Chat.vue` gagne un `inject<string>('sandbox-name', '')` (pattern
déjà utilisé par `Explorer.vue`), passé comme contexte à la création de
conversation et comme filtre à la liste.

**Fix erreur silencieuse** (`lib/src/prefixed_mcp.rs::connect_mcp_servers_selected`) :
le `tracing::warn!` sur échec de connexion MCP ne produit aujourd'hui aucun signal
utilisateur. Nouvelle variante `ChatEvent::ToolUnavailable { server: String, reason:
String }` (`lib/src/event.rs`), non terminale (contrairement à `Error`), émise une
fois par serveur MCP indisponible au moment de la construction du toolset —
`Chat.vue` l'affiche comme bandeau d'avertissement sans couper le streaming en
cours.

### Axe 2 — paramètres de profil de modèle (web)

Aucun changement backend : `options` (JSONB) est déjà exposé de bout en bout dans
`CreateModelProfile`/`UpdateModelProfile` (`app/src/api/model_profiles.rs`).
Seul le frontend change : `ModelProfilesScreen.vue` gagne un éditeur clé/valeur
pour `options` (en plus des champs déjà présents `temperature`/`max_tokens`), sans
liste figée de clés — les noms de paramètres varient trop selon le backend
(Ollama/vLLM/llama.cpp n'exposent pas tous top_p/min_p/top_k/repeat_penalty/
thinking_mode/reasoning_effort de la même façon).

### Axe 3 — composant de chat (Nuxt UI Chat)

Remplace `vue-advanced-chat` par le composant Chat de Nuxt UI (utilisable en Vue
pur/Vite, sans framework Nuxt) dans `frontend/src/components/panels/Chat.vue`.
Nouvelles dépendances : `ai`, `@ai-sdk/vue`, `@comark/vue`.

Le composant attend le protocole `UIMessage` du Vercel AI SDK, pas `ChatEvent`
tel quel — un adaptateur traduit le flux `ChatEvent` (WS `app/src/ws/chat.rs`) en
parts `UIMessage` :

| `ChatEvent` | Part `UIMessage` |
|---|---|
| `Token` | `text` (delta) |
| `ToolCall` | `tool-call` (state streaming) |
| `ToolResult` | `tool-call` (state result/output) |
| `Usage` | métadonnées de fin de message |
| `Error` | affichage bloquant, fin du tour |
| `ToolUnavailable` (nouveau, axe 1) | bandeau non bloquant, hors `UIMessage` |
| `SkillLoaded`, `SubagentStart/Event/End` | pas d'équivalent natif AI SDK — voir risques |

## Risques et questions ouvertes

- **Mapping `SubagentStart`/`SubagentEvent`/`SubagentEnd`** : ce sont des concepts
  vanyline sans équivalent dans le modèle `UIMessage` de l'AI SDK (pensé pour un
  seul agent séquentiel). À trancher en implémentant l'axe 3 : soit les aplatir
  dans le flux principal avec un préfixe visuel, soit les ignorer dans un premier
  temps (mono-agent) et les traiter dans une itération suivante.
- **`GET /api/conversations` avec filtre contexte** : le détail exact du filtre
  (par `sandbox_name` directement, ou par `context_id` déjà résolu côté frontend)
  sera précisé à l'implémentation de la tâche correspondante — l'un ou l'autre
  n'a pas d'impact sur le reste du design.
- **Ordre des axes** : proposition — axe 1 d'abord (le contexte est un prérequis
  pour tester axe 3 avec de vrais tool calls en conditions réelles), puis axe 2
  (indépendant, petit, purement frontend), puis axe 3 (le plus gros, bénéficie
  d'avoir axe 1 en place pour valider le rendu des tool calls).
- **Style Nuxt UI vs Element Plus/Reka UI** : Nuxt UI a son propre système de
  style (pas Tailwind requis d'après la doc consultée, mais pas non plus
  Element Plus) — risque de dissonance visuelle avec le reste du shell IDE.
  À vérifier concrètement en l'intégrant ; pas bloquant pour un premier rendu
  fonctionnel, mais peut demander un passage de theming dédié.
