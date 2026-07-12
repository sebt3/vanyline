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
| `vanyline-app` | binaire | Backend du frontend | axum (REST + WS), OIDC, sqlx/PostgreSQL, `PgConfigStore` (adapte le schéma PG en `ConfigStore`), orchestration LLM via `vanyline-lib` |
| `vanyline-sandbox` | binaire | Pod serveur WS/MCP | expose les 8 `vanyline-tools` via MCP (`tools_impl.rs`/`mcp.rs`, schémas partagés avec `cli`) |
| `vanyline-controller` | binaire | Opérateur Kubernetes | *(stub)* — kube-rs, CRDs |

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
glob de la sélection — c'est ce qui maîtrise le contexte pour les petits modèles.

**Tools builtin** (`vanyline_lib::builtin`) :
- `skill` : charge le corps d'un `SKILL.md` par nom (`ConfigStore::load_skill`), sa
  description embarque l'index des skills résolus pour l'agent (vide si
  `SkillSelection::None` ou si rien ne matche → le tool n'est même pas exposé).
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
garantit le nettoyage même si la tâche panique.

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

## Limites connues (dette assumée, pas oubliée)

- **Pas de streaming WS live côté app** : `CollectingSink` bufferise tous les événements
  d'un tour et les envoie d'un coup en fin de tour (`flush`), comme avant la migration
  harness-core. Un vrai streaming token-par-token nécessiterait de partager la moitié
  écriture du WebSocket dans un état interior-mutable accessible depuis `EventSink::emit`
  (`&self`) — hors scope de l'adaptation mécanique tâche 9.
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
