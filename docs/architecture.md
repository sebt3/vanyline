# Architecture — crates et packages du monorepo

Ce document décrit le découpage du monorepo — crates Rust et packages TypeScript — et
les **règles de dépendances** entre eux. Pour la vue système d'ensemble (composants,
interfaces réseau, auth), voir `AGENTS.md`.

## Vue d'ensemble

Six crates : **deux bibliothèques feuilles** partagées (`vanyline-tools`, `vanyline-lib`) et
**quatre binaires** qui les consomment (`vanyline`, `vanyline-app`, `vanyline-sandbox`,
`vanyline-controller`).

| Crate | Type | Rôle | Contenu clé |
|-------|------|------|-------------|
| `vanyline-tools` | lib feuille | Implémentations d'outils, pures et framework-agnostic, SLM-friendly (v2) | `filesystem` (read/write/edit/delete/list), `search` (find_files/search), `command` (`execute` via `sh -c`, timeout, cwd), `error` (`ToolsError`, codes `VNL-TLS-*`), `output` (bornage centralisé), `mcp` (schémas JSON — source unique consommée par `cli` et `sandbox`) |
| `vanyline-lib` | lib feuille | Cœur partagé LLM / MCP / chat — harness (agents, toolsets, skills, subagents) | `domain` (types name-keyed : `Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`…), `store::ConfigStore` (résolution de config par nom), `event` (`ChatEvent`/`EventSink`), `model` (construction de modèle + params), `session` (`SessionContext`, `run_agent_turn` — point d'entrée unique), `builtin` (tools `skill`/`task`), `prefixed_mcp` (connexion MCP filtrée par toolset), `types` (`ToolCall`/`Message`/`Conversation` — formats de persistance propres à chaque binaire), erreurs `VNL-*` |
| `vanyline` (bin `cli`) | binaire | CLI standalone de chat/agents | `run`/REPL, `FsConfigStore` (YAML deux couches, globale + workspace — voir "Configuration CLI" plus bas), enveloppe les `vanyline-tools` en `ToolDyn` locaux |
| `vanyline-app` | binaire | Backend du frontend | axum (REST + WS), OIDC, sqlx/PostgreSQL, `PgConfigStore` (adapte le schéma PG en `ConfigStore`), orchestration LLM via `vanyline-lib` — voir section "Backend web" plus bas |
| `vanyline-sandbox` | binaire | Pod serveur MCP | expose les 8 `vanyline-tools` via MCP (`tools_impl.rs`/`mcp.rs`, schémas partagés avec `cli`) — voir section "Serveur MCP" plus bas ; second binaire `vanyline-maint` (maintenance des workspaces par les Jobs du controller — voir section dédiée) |
| `vanyline-controller` | binaire | Opérateur Kubernetes | kube-rs, CRDs Owner/Project/Sandbox v1alpha1 — voir section dédiée plus bas |

## Graphe de dépendances

```
vanyline-tools  ◄──  vanyline (cli),  vanyline-sandbox
vanyline-lib    ◄──  vanyline (cli),  vanyline-app

vanyline-tools  : aucune dépendance interne, pas de rig/rmcp
vanyline-lib    : aucune dépendance interne (rig-core + rmcp externes)
vanyline-controller : isolé — aucune dépendance sur les autres crates
```

Aucun cycle. Les deux feuilles sont indépendantes l'une de l'autre.

## Règles de dépendances

1. **`vanyline-tools` et `vanyline-lib` sont des feuilles.** Elles ne dépendent d'aucun autre
   crate du workspace. Toute la logique réutilisable vit là ; les binaires ne font que composer.

2. **`vanyline-lib` ne dépend PAS de `vanyline-tools`.** La lib orchestre des outils *opaques*
   (`Arc<dyn ToolDyn>` référencés par nom dans `SessionContext.local_tools`) — elle ignore leur
   implémentation. C'est l'appelant (`build_session_context` côté cli, `handle_message` côté app)
   qui fournit ses outils concrets. Cette séparation est ce qui permet à chaque binaire d'apporter
   son propre jeu d'outils (ou aucun, côté app) sans dupliquer le cœur chat/MCP/session.

3. **`vanyline-tools` est framework-agnostic.** Pas de `rig-core` ni de `rmcp` : ce sont des
   capacités pures (filesystem, command). C'est chaque binaire qui les enveloppe (`ToolDyn` côté
   CLI, tools MCP côté sandbox à venir).

4. **`vanyline-app` ne dépend PAS de `vanyline-tools`.** L'app est sur le chemin froid : elle
   n'exécute pas d'outils localement. Les outils lui parviennent depuis la sandbox via MCP. Seuls
   `cli` (exécuteur local) et `sandbox` (fournisseur MCP) tirent `vanyline-tools`.

5. **Les binaires dépendent des libs, jamais l'inverse.** Un besoin partagé entre plusieurs
   binaires remonte dans `vanyline-lib` (logique LLM/MCP) ou `vanyline-tools` (capacité).

6. **`vanyline-controller` reste isolé.** Il ne partage pas de code avec les autres crates (kube-rs
   uniquement).

## Session engine — `run_agent_turn`

`run_agent_turn` (`vanyline_lib::session`) est le **point d'entrée unique** d'un tour de
chat, pour les deux binaires. Il résout l'agent par nom, assemble le system prompt, peuple
un `ToolServerHandle` frais (local tools + MCP filtrés par toolset + tools builtin
`skill`/`task`) et streame le tour :

```rust
pub async fn run_agent_turn(
    ctx: &SessionContext,       // store + sink + local_tools + subagent_depth_max
    agent_name: &str,           // résolu par nom, jamais par UUID
    history: Vec<rig_core::message::Message>,
    user_msg: &str,
    workspace_context: Option<&str>,   // le CLI y met AGENTS.md ; l'app n'en a pas
) -> Result<event::ChatTurnResult, VnyError>;
```

`SessionContext` porte tout ce qui varie par binaire :
- `store: Arc<dyn ConfigStore>` — résolution de config par nom (providers, modèles,
  toolsets, agents, skills). Chaque binaire fournit sa **propre implémentation** :
  `FsConfigStore` (cli, YAML deux couches natif — voir "Configuration CLI" plus bas ;
  remplace l'ancien `CliConfigStore`/JSON, supprimé) et `PgConfigStore` (app, requête le
  schéma PostgreSQL existant, synthétise encore `ModelProfile`/`Toolset` en attendant sa
  propre bascule vers un stockage natif — `app-harness-parity.md`).
- `sink: Arc<dyn EventSink>` — un seul type d'événement, `ChatEvent`, pour tous les
  transports (REPL stdout, WebSocket, JSON-RPC stdio). `EventSink::emit` est
  appelée pour chaque `ChatEvent` produit pendant le tour (tokens, tool calls/résultats,
  usage, événements de subagent…) ; chaque binaire décide comment les afficher/transmettre
  (cli : impression directe ; app : accumulation puis flush WS en fin de tour — pas de
  streaming live aujourd'hui, cf. « Limites connues » plus bas).
- `local_tools: HashMap<String, Arc<dyn ToolDyn>>` — outils fournis par l'hôte,
  référencés par nom via `Toolset.local_tools`. Le CLI y met ses 8 outils (issus de
  `vanyline-tools`) ; l'app n'en fournit aucun (elle reste sur le chemin froid, les outils
  lui parviennent via MCP).
- `subagent_depth_max: u8` — profondeur maximale d'imbrication pour le tool builtin `task`
  (voir plus bas).

**Résolution des MCP servers** : `prefixed_mcp::connect_mcp_servers_selected` ne contacte
QUE les serveurs référencés par les `McpSelection` des toolsets de l'agent (jamais tous les
serveurs configurés), et ne remonte au modèle que les tools dont le nom matche un pattern
glob de la sélection — c'est ce qui maîtrise le contexte pour les petits modèles. Les
en-têtes HTTP custom (`McpServer.headers`) sont appliqués via
`StreamableHttpClientTransportConfig::custom_headers` (`StreamableHttpClientTransport::
from_config`, jamais un `reqwest::Client` construit à la main — `rmcp` embarque son propre
reqwest interne, une version différente de celle du workspace, les deux ne sont pas
interchangeables au niveau des types). Deux serveurs/tools locaux référencés par plusieurs
toolsets de l'agent ne sont ni recontactés ni réajoutés en double (dédup par nom de
serveur / nom de tool sur la durée du tour).

**Cycle de vie des connexions MCP** : `connect_mcp_servers_selected` retourne les
`RunningService` (alias `McpRunningService`) de chaque serveur connecté — `run_agent_turn_at_depth`
les garde en vie jusqu'à la fin du tour et les annule proprement (`.cancel().await`) après.
Point d'attention pour toute évolution de ce code : `RunningService` porte un `DropGuard` qui
annule sa tâche de fond au drop — un `RunningService` local qui sort de portée AVANT la fin du
tour coupe la connexion avant tout appel de tool réel (bug réel corrigé, pas hypothétique).

**Tools builtin** (`vanyline_lib::builtin`) :
- `skill` : charge le corps d'un `SKILL.md` par nom (`ConfigStore::load_skill`), sa
  description embarque l'index des skills résolus pour l'agent (vide si
  `SkillSelection::None` ou si rien ne matche → le tool n'est même pas exposé). `call`
  refuse tout nom absent de cet index (`VnyError::UnknownReference`) même s'il existe dans
  le store — la portée `SkillSelection` de l'agent est une garantie du tool, pas seulement
  de sa description.
- `task` : délègue à un subagent (`mode: Subagent|All` seulement — un agent `Primary` est
  refusé). Lance un `run_agent_turn` imbriqué à `current_depth + 1`, avec un historique
  vierge et un sink qui encapsule chaque événement du subagent en
  `ChatEvent::SubagentEvent`. Refuse au-delà de `subagent_depth_max` (garde vérifiée à
  la fois à l'exposition du tool et à l'appel — double sécurité contre la récursion).

## Configuration CLI — `FsConfigStore` (deux couches YAML)

`vanyline` (cli) résout sa configuration en **deux couches YAML**
superposées, name-keyed comme le reste du harness (aucun UUID manipulé) :

| Couche | Racine | Découverte |
|--------|--------|------------|
| Globale | `~/.config/vanyline/` | toujours présente |
| Workspace | `<racine>/.vanyline/` | remontée depuis le cwd jusqu'à trouver `.vanyline/` ou `.git/` — le premier marqueur trouvé (quel qu'il soit) fixe la racine ; absente si aucun marqueur jusqu'à la racine du système de fichiers |

**Fusion** : par nom, l'entrée workspace remplace intégralement l'entrée
globale homonyme (pas de deep-merge intra-entrée). `config.yaml` fusionne
en plus clé par clé au niveau de ses quatre maps nommées (`providers`,
`models`, `mcp`, `defaults` — `cli/src/config.rs::merge_config_layers`).
Agents/toolsets/skills (un fichier/répertoire par entité) fusionnent par
nom de fichier (`list_layer_files`/`list_layer_skill_dirs` +
`merge_layer_files`, même module).

**Formats** : `config.yaml` (providers/models/mcp/defaults, un fichier par
couche) ; `agents/<name>.md` (frontmatter YAML + corps = system prompt,
délimiteurs `---` parsés à la main, pas de crate) ; `toolsets/<name>.yaml` ;
`skills/<name>/SKILL.md` (frontmatter `name`+`description` compatible
écosystème externe, mais le nom canonique reste celui du répertoire —
chargement paresseux : `list_skills` ne lit que la description, `load_skill`
le corps à la demande).

**Modules** : `cli/src/config.rs` porte toute la mécanique de couches
(découverte, fusion, et la "source" d'une entrée — global vs workspace,
réutilisée par les commandes `list` et par l'affichage "sources workspace"
au lancement) ; `cli/src/fs_store.rs::FsConfigStore` implémente
`ConfigStore` dessus — store actif de toutes les commandes CLI. Dépendance
`yaml_serde` (fork maintenu de `serde_yaml`, devenu archivé — API
identique : `from_str`/`to_string`/`Value`/`Error`).

**`vanyline config check`** (`cli/src/config_check.rs`) : charge toutes les
entités et croise leurs références (model→provider, agent→model/toolset/
skill nommé, toolset→mcp_server, `defaults.agent`→agent) — best-effort, un
`list_x()` qui échoue ne bloque pas les autres vérifications, tous les
problèmes sont rapportés ensemble (pas de fail-fast).

**Sécurité workspace assumée** : un `.vanyline/` de repo cloné peut définir
des agents/mcp arbitraires — accepté (usage solo, agents de confiance,
philosophie yolo du projet), simplement affiché au lancement de `run`/REPL
(« sources workspace : … ») pour la visibilité, pas de sandboxing ni de
confirmation.

**Données ≠ config** : les conversations vivent sous
`~/.local/share/vanyline/` (XDG data, `config::data_dir`), pas
`~/.config`. Leur UUID reste interne au stockage — `conversations
show|delete|set` acceptent un index 1-based (position dans `conversations
list`) ou un préfixe d'UUID (`store::resolve_conversation_reference`),
jamais l'UUID complet obligatoire.

**Rupture assumée, pas de migration automatique** : l'ancien format JSON
(`providers.json`/`agents.json`/`default-agent.json`, `CliConfigStore`) a
été entièrement supprimé — une config JSON pré-existante doit être
réécrite en YAML à la main. Même logique pour l'emplacement des
conversations (ancien `~/.config/vanyline/conversations/` → nouveau XDG
data) : les anciens fichiers restent orphelins sur disque, jamais lus ni
déplacés.

## RPC stdio — `vanyline serve --stdio` (JSON-RPC 2.0)

Serveur JSON-RPC 2.0 sur stdio (ndjson — une trame JSON par ligne, `\n`,
stdout réservé au protocole, logs sur stderr), exposant tout le harness
CLI pour l'extension VS Code (ou tout autre client programmatique) sans
passer par l'app. **Spec complète (transport, enveloppes, table des codes
d'erreur, exemples de trames pour chaque méthode) : `docs/rpc-protocol.md`
— ce qui suit est le résumé architectural, pas une référence.**

**Modules** : `cli/src/rpc/mod.rs` (boucle stdin/stdout, writer unique via
canal mpsc), `protocol.rs` (types serde requêtes/réponses/notifications,
namespace d'erreur `VNL-RPC-000` à `VNL-RPC-009`), `handlers.rs`
(dispatch, `ServerState`, logique par méthode). Réutilise `FsConfigStore`
(config, tâche 02a) et `cli/src/store.rs` (conversations, format JSON
existant, tâche 02b) tels quels — aucun nouveau stockage introduit.

**Concurrence de `chat/send`** — la seule méthode asynchrone du protocole
(le tour LLM peut être long) : la boucle stdio reste séquentielle pour
tout le reste, mais `chat/send` est **spawné** en tâche tokio indépendante
pour ne pas bloquer la lecture d'autres requêtes (y compris un autre
`chat/send` sur une AUTRE conversation — plusieurs conversations tournent
en vrai parallèle). Pas de `Arc<Mutex<ServerState>>` global : seuls les
champs qui doivent être visibles/mutables depuis une tâche spawnée
(`busy: Arc<Mutex<HashSet<Uuid>>>`, `seq: Arc<Mutex<HashMap<Uuid,u64>>>`,
`tx` — déjà `Clone` nativement) le sont individuellement ; `store` est un
`Arc<FsConfigStore>` partagé sans verrou (lecture seule après
`initialize`). Un `chat/send` sur une conversation déjà occupée répond
`VNL-RPC-002` immédiatement, sans rien spawner. `BusyGuard` (`Drop`)
garantit le nettoyage même si la tâche panique. `conversations/delete`
purge aussi `busy`/`seq` pour l'id supprimé (sinon croissance non bornée
de ces maps sur la durée de vie du process).

Même pattern verrou-busy-par-conversation + spawn répliqué côté app
(`app/src/ws/chat.rs`, `AppState.busy` + `BusyGuard` + `try_acquire_busy` —
implémentation séparée, pas de code partagé entre les deux binaires, mais
la même sémantique : un message reçu pendant un tour actif reçoit une
erreur busy (`VNL-WS-001` côté WS, `VNL-RPC-002` côté RPC) au lieu d'être
mis en file ou de bloquer la lecture du transport). Persistance
identique sur les deux surfaces : le message user est enregistré AVANT le
tour (il a bien été envoyé, quelle que soit l'issue du tour), le message
assistant seulement APRÈS un tour réussi.

**Passthrough camelCase/snake_case** : les enveloppes RPC propres à ce
protocole (`InitializeResult`, `ConversationSummary`, `ChatSendParams`/
`Result`, notifications) sont en camelCase ; `config/*` et
`conversations/get` retournent les types `vanyline_lib`/CLI **tels
quels**, en snake_case natif (`system_prompt`, `tool_calls`…) — aucune
conversion, documenté explicitement dans `rpc-protocol.md` pour ne pas
piéger le développeur de l'extension.

**Dette assumée** (cohérente avec « Limites connues » plus bas) :
`chat/cancel` est un no-op protocolaire (valide juste l'UUID, accepte,
n'annule rien) — l'annulation réelle dépend du support d'annulation de
`run_agent_turn`, toujours absent. Pas de watcher de config : `FsConfigStore`
relit le disque à chaque appel (déjà son comportement natif, pas
spécifique au RPC). Pas de détachement propre des tâches `chat/send`
encore en vol à l'arrêt du serveur (shutdown/EOF) — `writer.await` peut
attendre un tour qui ne se termine jamais si le client ferme la
connexion en plein tour ; acceptable pour v1, le process est de toute
façon sur le point de sortir.

## Backend web — `vanyline-app`

Backend axum du frontend (MVP `initial-app-frontend` + tables/API name-keyed de
`app-harness-parity`). Auth OIDC stateless (pas de session serveur) : `openidconnect`
4.0, cookie `HttpOnly` chiffré (`cookie::Key` 64 octets depuis `COOKIE_SECRET`,
payload `{id_token}|{email}`, `auth/cookie.rs`) revalidé à chaque requête par
l'extractor `AuthUser` (`auth/middleware.rs`) — tout endpoint `/api/*` scope ses
requêtes par utilisateur (`get_or_create_user`), aucune notion d'admin distincte
(`AdminAuth`/`ADMIN_SECRET` du MVP initial ont été retirés une fois l'API CRUD
name-keyed en place — l'ancienne distinction admin/utilisateur n'avait plus de sens
dès que providers/mcp servers sont eux aussi scopés par utilisateur).

**Stockage** : PostgreSQL/sqlx, migrations `app/migrations/0001_initial.sql` (schéma
MVP) + `0002_harness_parity.sql` (tables `model_profiles`/`toolsets`/`skills`, `agents`
v2 name-keyed, `user_id`+`UNIQUE(user_id, name)` ajoutés à `llm_providers`/
`mcp_servers`). `config_store.rs::PgConfigStore` implémente `vanyline_lib::ConfigStore`
sur ce schéma — une instance par requête, scopée `user_id`, convertit les lignes en
types name-keyed de la lib (le nom remplace l'UUID en sortie, comme `FsConfigStore`
côté cli). `load_skill` lit `skills.body` à la demande, même paresse que le cli.

**API REST** (`api/*.rs`, `api::api_router`) : CRUD par nom pour `model-profiles`,
`toolsets`, `skills`, `agents` ; CRUD par id pour `llm-providers`/`mcp-servers`
(`{id}/test` = discovery modèles, `{id}/default`) ; `conversations` + `messages`.
Toutes les routes exigent `AuthUser` et scopent par utilisateur.

**WebSocket chat** (`ws/chat.rs`) : `run_agent_turn` avec `local_tools` vide (l'app
reste sur le chemin froid — cf. règle de dépendances plus haut). `ChannelSink` pousse
chaque `ChatEvent` sur un canal mpsc dès son émission ; une tâche `forward_events` par
connexion (pas par tour) draine le canal et écrit sur le socket au fil de l'eau — vrai
streaming token-par-token, contrairement à l'ancien `CollectingSink` qui bufferisait
tout un tour avant le premier octet (limite documentée pendant la migration
harness-core, résolue par la tâche `ws-chatevent`).

**Déploiement** : image `docker.io/sebt3/vanyline-app:0.0.1-alpha.1`, build podman
multi-stage (node → rust → debian-slim), manifestes `deploy/web/` (dont
`RestEndPoint_sso.yaml` — kuberest provisionne l'app OIDC dans Authentik).

**Frontend actuel** (`frontend/src/`) : deux pages (`Login.svelte`, `Chat.svelte`),
routage hash-based. `Chat.svelte` assemble `ConversationList` + `AgentSelector` +
`ChatMessage` + `ChatInput`. `ChatMessage` rend le texte + les tool calls à plat —
pas encore de repli/dépliage par tool result, de badge usage ni de sous-fil pour les
événements subagent, et aucun écran de gestion CRUD (model profiles/toolsets/skills/
agents n'ont pas d'UI, seule l'API existe) : ce sont les tâches `front-chat` et
`front-crud` de `app-harness-parity`, pas encore faites — cf.
`docs/features/app-harness-parity.md`, laissé ouvert pour cette raison.

## Outils (`vanyline-tools`) — conventions SLM-friendly

La crate `vanyline-tools` cible explicitement des modèles plus petits que Qwen3.6
(SLM), pas seulement les modèles haut de gamme. Surface volontairement réduite à
8 outils orthogonaux, un par capacité évidente :

| Outil | Params requis | Notes |
|-------|---------------|-------|
| `read_file` | `path` | `offset`/`limit` optionnels (0 = défaut), sortie numérotée `NNN\tligne` |
| `write_file` | `path`, `content` | crée les répertoires parents |
| `edit_file` | `path`, `old_string`, `new_string` | remplacement exact ; `replace_all` optionnel. Seul outil à dépasser le principe « ≤2 params requis » — tension acceptée, inhérente à un remplacement exact (path + old + new sont tous les trois indispensables) |
| `delete_file` | `path` | fichier ou répertoire **vide** uniquement |
| `list_directory` | `path` | arbre compact, `depth` optionnel (0 = défaut 1, pas de récursion) |
| `find_files` | `pattern` | glob (`**/*.rs`), `path` optionnel (défaut `.`) |
| `search` | `pattern` | regex, `path`/`glob` optionnels, résultats `fichier:ligne: extrait` |
| `execute_command` | `command` | `timeout_secs`/`cwd` optionnels, sortie = exit code + durée + stdout/stderr bornés tête+queue |

Conventions transverses (`tools/src/error.rs`, `tools/src/output.rs`) :
- **Erreurs actionnables** : chaque variante de `ToolsError` porte un code
  `VNL-TLS-NNN` et un message qui dit quoi faire (`FileNotFound` inclut le
  contenu du répertoire parent ; `EditNoMatch` inclut la ligne la plus proche
  par distance de Levenshtein ; `EditAmbiguous` suggère `replace_all`).
- **Sorties bornées, jamais de coupure silencieuse** : `bound_lines` (lignes,
  avec `offset` de reprise explicite dans le message) et `bound_head_tail`
  (tête+queue, pour les commandes) — constantes centralisées
  (`READ_MAX_LINES`, `SEARCH_MAX_MATCHES`, `COMMAND_MAX_BYTES`…), jamais de
  nombre magique dans un outil.
- **Schéma unique** : `tools/src/mcp.rs` porte les schémas JSON des 8 outils
  (description calibrée + mini-exemple d'arguments, un seul `required`
  minimal) — consommés à l'identique par `cli/src/tools.rs`
  (`ToolDefinition`) et par `sandbox/src/mcp.rs` (MCP). Ajouter un outil ou
  changer un schéma se fait à un seul endroit.

**Validation manuelle SLM** (au-delà des tests unitaires par outil) : avant de
faire évoluer cette surface, vérifier avec un vrai petit modèle plutôt qu'avec
Claude — les schémas/erreurs qui semblent clairs à un modèle haut de gamme ne
le sont pas toujours pour un SLM. Pratique établie : un agent CLI configuré
sur un modèle local (`~/.config/vanyline/agents.json`, provider `openai-compatible`
pointant vers l'inférence locale), **sans serveur MCP** pour isoler les 8
outils locaux, et une poignée de prompts forçant un enchaînement réaliste
(explorer → chercher → lire → éditer → vérifier). Observer : le modèle
choisit-il le bon outil, construit-il des arguments valides, se corrige-t-il
tout seul sur une erreur (nom de fichier approximatif, occurrence ambiguë) ?
Cette pratique a débusqué un bug réel du moteur de session (`default_max_turns`
jamais configuré, cf. `git log --grep default_max_turns`) qu'aucun test
unitaire n'aurait attrapé — un SLM enchaîne spontanément plus d'outils par
tour qu'un scénario de test écrit à la main.

## Serveur MCP — `vanyline-sandbox`

Fork adapté de `kydah-mcp-template` (transport MCP HTTP streamable POST-only fait
main sur axum, JSON-RPC 2.0 dispatch `tools/list`/`tools/call` dans `mcp.rs` — pas de
dépendance à l'API server `rmcp`, contrairement au client côté `vanyline-lib`).
`tools_impl.rs` est la glue vers les 8 outils de `vanyline-tools` (mêmes schémas JSON
que `cli/src/tools.rs`, source unique `tools/src/mcp.rs`).

**Auth** (`auth.rs`, héritée du template telle quelle) : OIDC/JWKS + niveaux d'accès
par groupe (`AUTH_GROUPS_ADMIN`/`AUTH_GROUPS_READ`), `--no-auth` (dev, refuse de
démarrer sans le flag explicite) ou `STATIC_TOKEN` (démo, bypasse l'OIDC). C'est un
modèle **distinct** des deux modes JWT-app/SA-TokenReview décrits dans `AGENTS.md`
pour le frontend et kydah-code — celui-ci reste à câbler quand ces clients
consommeront réellement la sandbox (P2/P3 du design d'origine, pas encore démarrés).

**Confinement** (`tools_impl.rs`, garde-fou d'ergonomie — la frontière de sécurité
réelle est le pod) : tout chemin est résolu sous `VNL_SANDBOX_ROOT` (canonicalisation
+ vérification de préfixe, y compris pour un suffixe non encore existant — résolution
lexicale du parent le plus proche qui existe déjà, `VNL-SBX-003`), erreur `VNL-SBX-001`
sinon ; `execute_command` a `cwd = VNL_SANDBOX_ROOT`.

**Observabilité** (`telemetry.rs`, du template) : métriques Prometheus sur un port
séparé (`METRICS_LISTEN`, `0.0.0.0:9090` par défaut — délibérément jamais exposé par
le Service K8s, cf. commentaire `config.rs`), export OTLP optionnel
(`OTEL_EXPORTER_OTLP_ENDPOINT`, dégradation silencieuse si le collecteur est injoignable).

**Déploiement** : `sandbox/Dockerfile` (multi-stage → `debian:trixie-slim` + substrat
natif validé). `deploy/sandbox/sandbox-test.yaml` (pod + PVC + toolchains rust/node en
`volumes[].image`) remplace les pods d'expérimentation `deploy/sandbox-imagevol-*.yaml`
une fois la recette absorbée par le Dockerfile et le controller — validée en
conditions réelles le 2026-07-01 sur cluster K8s 1.36.2/cri-o 1.36.1 (cf. section
"vanyline-maint" pour la recette elle-même). Image publiée :
`docker.io/sebt3/vanyline-sandbox:0.0.1-alpha.1`.

## Opérateur Kubernetes — `vanyline-controller`

kube-rs, trois CRDs namespacées (`vanyline.solidite.fr/v1alpha1`) réconciliées par un
reconciler chacun, tournant en parallèle dans le même process (`main.rs::tokio::join!`) :

```
Owner (1) ────────── (n) Project ─────────── (n) Sandbox
SA + PVC home RWX       PVC workspace RWO       pod = worktree d'une branche
(clés, dotfiles)        repo git bare + caches   (monte home + workspace + toolchains)
```

**Pourquoi trois CRDs** : un CRD se justifie par un état désiré à réconcilier, pas par la
possession d'un objet natif. Owner = identité (ServiceAccount `owner-<name>` — pilier de
l'auth TokenReview pour kydah-code et l'app), home. Project = workspace, repo git,
caches, Jobs/CronJob de maintenance. Sandbox = pod + branche. Zéro chevauchement.

**Répartition du stockage — dictée par les filewatchers** : openvscode-server et
rust-analyzer reposent sur inotify, qui ne traverse pas les filesystems réseau. PVC
Project (workspace, arbres de code) : bloc local **RWO** — colocalisation limitée aux
branches actives d'un même projet, le scheduler co-place via le volume. PVC Owner
(home) : petit, read-mostly, zéro watcher → **RWX**, suit l'utilisateur sur tous les
nœuds sans contrainte de colocalisation.

| Reconciler (`controller/src/*.rs`) | Objets gérés |
|---|---|
| `owner.rs` | PVC home (créé ou référence `existing_pvc` vérifiée) + ServiceAccount `owner-<name>` + condition `Ready`. Pas de finalizer (rien à nettoyer côté cluster que la suppression K8s de l'Owner n'efface pas déjà via owner references). |
| `project.rs` | PVC workspace (créé ou référence vérifiée) + Job `project-init` (clone bare + mkdir caches, une fois) + CronJob `project-fetch` (`git fetch --prune`, planning dérivé de `fetch_interval`) + finalizer (Job purge puis suppression du PVC créé — un PVC référencé n'est jamais supprimé). |
| `sandbox.rs` | Job `sandbox-checkout` (`git worktree add`, création de branche depuis la default branch si absente) + Pod (home Owner + worktree en subPath + un `volumes[].image` par toolchain + env agrégé PATH/LD_LIBRARY_PATH/caches) + Service ClusterIP (port MCP) + NetworkPolicy (ingress restreint aux pods du namespace portant `vanyline.solidite.fr/owner: <owner>`) + finalizer (Job `git worktree remove`, la branche survit sur le remote). |

Tous les Jobs git (`project.rs`/`sandbox.rs`) invoquent `vanyline-maint` (image sandbox)
en argv — jamais de `sh -c`, aucun champ de CRD ne s'interpole dans une commande shell
(cf. section "Maintenance des workspaces" ci-dessous pour l'outil lui-même).

**Presets toolchain** (`sandbox.rs::toolchain_preset`) : la recette d'env validée (PATH,
`LD_LIBRARY_PATH` deux arches, `RUSTUP_HOME`…) vit ici, pas répétée dans chaque CR —
`Toolchain.env` vide déclenche le preset si `Toolchain.name` matche (`rust`, `node`),
sinon aucune variable ; `Toolchain.env` explicite remplace le preset entièrement.

**Tests** : unitaires purs sur les builders (spec → Pod/Job/Service/NetworkPolicy
attendus, sans cluster) — pas de mock de l'API K8s. `--crds` (flag CLI) imprime les
manifests CRD générés par `schemars`, source de `deploy/controller/crds.yaml`
(régénéré via `deploy/controller/generate-crds.sh`).

**Déploiement** : `deploy/controller/` (RBAC ClusterRole/ClusterRoleBinding — le
controller watche les trois CRDs sur tout le cluster via `Api::all`, donc pas de
Role/RoleBinding namespacé même si les CRDs elles-mêmes le sont — + Deployment) et
`controller/Dockerfile` (cargo-chef, rustls-tls, pas de libssl). Image publiée :
`docker.io/sebt3/vanyline-controller:0.0.1-alpha.1`. Validé en e2e sur le cluster de
dev (Owner + Project + Sandbox de démo) — a débusqué un bug réel : les trois
reconcilers réutilisaient les mêmes `PatchParams` (avec `force()`, nécessaire aux
`Patch::Apply` de PVC/SA/Service/NetworkPolicy) pour le patch de status en
`Patch::Merge`, que kube-rs rejette hors contexte Apply — corrigé en isolant
`PatchParams::default()` pour le patch de status.

**Limites connues** (v1, assumées) : pas de CRD Application (viendra avec la
convergence app ↔ sandbox), pas de quotas (champ réservé dans `OwnerSpec`, non
réconcilié), pas d'ingress/JWT sur la Sandbox (le frontend n'y accède pas encore), pas
de webhook d'admission (validation par schéma CRD uniquement), pas de merge/push
automatique des branches (le controller gère la plomberie git, pas le contenu), pas
d'openvscode-server dans le pod. Changement de spec Sandbox = recréation du pod
(immutable en v1). `fetch` ne rafraîchit pas les branches du clone bare (cf.
"Limites connues" générales plus bas — bug préexistant, hors périmètre du controller
lui-même, à traiter dans WS-11).

## Maintenance des workspaces — `vanyline-maint` (crate sandbox)

Second binaire du crate `vanyline-sandbox` (`sandbox/src/bin/maint.rs`, logique dans
`sandbox/src/maint.rs`). C'est l'outil que les Jobs du controller exécutent dans un pod
à image sandbox pour toute maintenance d'un workspace Project — la règle système
correspondante ("l'image sandbox est l'outil de maintenance du controller") est dans
`AGENTS.md`, section controller.

| Sous-commande | Rôle |
|---------------|------|
| `init --repo <url> --workspace <dir> [--cache <name>]...` | mkdir des caches + clone bare si absent (idempotent) |
| `fetch --workspace <dir>` | `git fetch --prune` sur le clone bare |
| `purge --workspace <dir>` | supprime `repo.git`, `worktrees/`, `cache/` (idempotent) |
| `checkout --workspace <dir> --sandbox <n> --branch <ref> [--default-branch <ref>]` | worktree idempotent ; branche créée depuis la default branch (résolue par `symbolic-ref`, repli `main`) si absente du bare |
| `remove --workspace <dir> --sandbox <n>` | `worktree remove --force`, repli `rm -rf`, puis `worktree prune` |
| `detect --workspace <dir>` | stub — sort `{}` ; implémentation réelle : WS-10 |

Décisions structurantes :

- **Validation avant toute action** : branches via `git check-ref-format --branch`
  (subprocess — sémantique exacte de git, pas de réimplémentation), nom de sandbox comme
  composant de chemin sûr (`[A-Za-z0-9._-]`, ni `.` ni `..`, pas de `-` initial — le nom
  entre dans `worktrees/<n>`, ferme le path traversal), URL de repo plausible (non vide,
  pas de `-` initial, pas de caractère de contrôle). Erreurs `VNL-MAINT-001` à `005`.
- **Jamais de shell** : toutes les invocations git en argv (`std::process::Command`),
  `--` avant les positionnels de `clone`. Côté controller, les 5 Jobs git construisent
  `command: Vec<String>` (`git_pod_template`) — plus aucun `sh -c` dans `controller/`,
  un champ de CRD ne peut plus s'échapper dans un shell (R1 clos par construction ;
  test `git_pod_template_no_shell` le fige).
- **Layout dupliqué délibérément** : `repo.git`, `worktrees/<sandbox>`, `cache/<dir>`
  et le mapping `pnpm` → `pnpm-store` existent des deux côtés
  (`controller/src/project.rs` et `sandbox/src/maint.rs`) — conforme à la règle
  "`vanyline-controller` reste isolé", pas de crate partagée pour 3 chemins. La dérive
  est couverte par des tests aux mêmes littéraux des deux côtés (`layout_constants`,
  `cache_dir_name_mapping`, `worktree_path_value` côté maint ;
  `bare_repo_and_worktree_paths`, `cache_dir_name_mapping` côté controller).
- **Les identifiants de cache passent tels quels** (`--cache pnpm`) : le mapping vers le
  nom de répertoire vit dans `vanyline-maint`, plus dans la commande du Job.
- **R2** : les presets toolchain du controller listent x86_64 **et** aarch64 dans
  `LD_LIBRARY_PATH` — le loader ignore silencieusement les répertoires absents, aucune
  logique par nœud.
- **Release couplée** : controller et image sandbox sortent du même repo et avancent
  ensemble — une image antérieure à WS-9 ne contient pas `vanyline-maint`.

## Limites connues (dette assumée, pas oubliée)

- **`fetch` ne met pas à jour les branches du clone bare** : `git clone --bare` ne
  configure aucune refspec de fetch, donc le `vanyline-maint fetch` périodique (comme le
  script shell qu'il remplace — parité voulue par WS-9) ne rafraîchit que `FETCH_HEAD`,
  pas `refs/heads/*` ; un `checkout` d'une branche apparue sur le remote après le clone
  ne la verra pas, et `--prune` n'a rien à élaguer. Bug latent **préexistant** du design
  controller, hors périmètre WS-9 (parité stricte) — à traiter dans WS-11 (sandbox-git),
  probablement via une refspec `+refs/heads/*:refs/heads/*` posée par `init`.

- **Historique appauvri** : seul le texte user/assistant est rejoué d'un tour à l'autre
  (`cli/src/chat.rs`, `app/src/ws/chat.rs`) — pas les tool calls/résultats intermédiaires.
- **Pas d'annulation** : `run_agent_turn` ne prend pas de token d'annulation. Le RPC
  `chat/cancel` existe déjà dans le protocole (no-op v1, cf. section RPC stdio ci-dessus)
  pour ne pas casser les clients qui l'appellent ; son ajout futur devra rester
  compatible signature (paramètre sur `SessionContext` plutôt que sur la fonction).
- **`additional_params` par provider non validé en conditions réelles** : `ModelProfile.options`
  (ex. `num_ctx` pour ollama) est correctement transmis à `AgentBuilder::additional_params`
  côté code, mais la transmission effective jusqu'à la requête HTTP par le provider ollama
  de rig 0.38 n'a pas été vérifiée contre un serveur réel (nécessite un ollama vivant).
- **Bornes de sortie non calibrées en conditions réelles** : les constantes de
  `tools/src/output.rs` (`READ_MAX_LINES=200`, `SEARCH_MAX_MATCHES=50`,
  `COMMAND_MAX_BYTES=8Ko`…) sont des valeurs de départ raisonnables, pas
  mesurées contre de gros fichiers/sorties réels. La validation manuelle SLM
  menée jusqu'ici portait sur des petits fichiers de test — à revisiter si un
  usage réel montre qu'elles tronquent trop tôt (ou pas assez).
- **`ChatEvent::ToolResult.is_error` toujours `false`** : `rig-core` 0.38.1
  (`agent/prompt_request/streaming.rs`, autour de la ligne 1530) convertit
  toute erreur de tool call en texte (`Err(e) => e.to_string()`) *avant* de
  construire `StreamedUserContent::ToolResult` — `rig_core::message::ToolResult`
  n'expose aucun champ `is_error` dans cette version, l'information est perdue
  en amont de tout code vanyline. Vérifié en lisant les sources vendored de
  `rig-core`, pas une supposition. Pas de heuristique de détection par contenu
  texte (fragile, faux positifs/négatifs) — limite documentée et acceptée
  plutôt que contournée. À revisiter si une version future de `rig-core`
  expose l'information.
- **`cargo clippy --workspace --all-targets`** est la commande utilisée par
  la CI (`clippy` job de `.github/workflows/test.yml`), pas
  `cargo clippy --workspace` seul (documenté dans `AGENTS.md`) : `--all-targets`
  inclut aussi le code de test, jamais vérifié localement jusqu'à WS-8, ce qui
  a révélé une vingtaine d'erreurs préexistantes (corrigées, cf. commit
  `clippy-all-targets-cleanup`). Le pattern `MutexGuard` tenu across `.await`
  dans les tests RPC (`isolated_data_dir()`, cf. section "RPC stdio"
  ci-dessus) déclenche un faux positif `clippy::await_holding_lock` — `allow`
  documenté au niveau du module de test plutôt que de restructurer les tests.

## Workspace TypeScript (npm workspaces)

Le monorepo n'est pas que du Rust : le `package.json` racine fédère les packages
TypeScript. Cible (les packages marqués *(à venir)* sont créés par les workstreams
WS-2/WS-6 de `docs/roadmap.md`) :

| Package | Type | Rôle | Stack |
|---------|------|------|-------|
| `frontend/` | app | Web app : éditeur + chat LLM, cliente de l'app Rust (REST + WS) | Vite, Svelte 5, CodeMirror 6, Tailwind 4, Vitest, Storybook |
| `packages/protocol` *(à venir)* | lib feuille | Types partagés `ChatEvent` + protocole JSON-RPC stdio, client ndjson | TypeScript pur, zéro dépendance UI |
| `ext/` *(à venir)* | app | Extension VS Code : front-end graphique du CLI via JSON-RPC stdio | Host TS + webview Svelte 5/Tailwind |
| `packages/ui` *(plus tard)* | lib | Composants de chat partagés frontend ↔ webview — extrait quand les deux fronts auront convergé | Svelte 5 |

### Règles de dépendances TypeScript

1. **`packages/protocol` est une feuille** : aucune dépendance UI, consommable par
   n'importe quel client (extension, frontend, scripts de test).
2. **Un seul schéma d'événement** : `ChatEvent` est défini en Rust (`vanyline-lib`) ;
   les types TS de `packages/protocol` en sont le miroir, **générés par `ts-rs`**
   (feature-gated côté lib, fichier généré commité et vérifié en CI). La dérive
   Rust↔TS est un bug, pas une fatalité.
3. **Les apps dépendent des libs, jamais l'inverse** — même règle que côté Rust.
4. Pas de `console.log` dans les sources (logger projet), pas de code partagé par
   copier-coller entre `frontend/` et `ext/` : ce qui doit être partagé remonte dans
   `packages/`.
