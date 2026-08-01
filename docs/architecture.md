# Architecture — crates et packages du monorepo

Ce document décrit le découpage du monorepo — crates Rust et packages TypeScript — et
les **règles de dépendances** entre eux. Pour la vue système d'ensemble (composants,
interfaces réseau, auth), voir `AGENTS.md`.

## Vue d'ensemble

Sept crates : **trois bibliothèques feuilles** partagées (`vanyline-tools`, `vanyline-lib`,
`vanyline-crds`) et **quatre binaires** qui les consomment (`vanyline`, `vanyline-app`,
`vanyline-sandbox`, `vanyline-controller`).

| Crate | Type | Rôle | Contenu clé |
|-------|------|------|-------------|
| `vanyline-tools` | lib feuille | Implémentations d'outils, pures et framework-agnostic, SLM-friendly (v2) | `filesystem` (read/write/edit/delete/list), `search` (find_files/search), `command` (`execute` via `sh -c`, timeout, cwd), `error` (`ToolsError`, codes `VNL-TLS-*`), `output` (bornage centralisé), `mcp` (schémas JSON — source unique consommée par `cli` et `sandbox`) |
| `vanyline-crds` | lib feuille | Types CRD Owner/Project/Sandbox (spec/status/derives kube), sans runtime opérateur | `Owner`/`Project`/`Sandbox` + specs/status, `Toolchain`/`PvcRef`/`ProjectDefaults`, `crd_manifests()`, `service_name`/`MCP_PORT` (convention de nommage du Service MCP d'une sandbox, partagée avec `vanyline-lib`) — voir section "Client K8s CLI" plus bas |
| `vanyline-lib` | lib feuille | Cœur partagé LLM / MCP / chat — harness (agents, toolsets, skills, subagents) | `domain` (types name-keyed : `Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`…), `store::ConfigStore` (résolution de config par nom), `event` (`ChatEvent`/`EventSink`), `model` (construction de modèle + params), `session` (`SessionContext`, `run_agent_turn` — point d'entrée unique), `builtin` (tools `skill`/`task`), `prefixed_mcp` (connexion MCP filtrée par toolset), `types` (`ToolCall`/`Message`/`Conversation` — formats de persistance propres à chaque binaire), `k8s` (`VnlK8sClient`, **feature Cargo optionnelle `k8s`**, désactivée par défaut — voir "Client K8s CLI" plus bas), erreurs `VNL-*` |
| `vanyline` (bin `cli`) | binaire | CLI standalone de chat/agents | `run`/REPL, `FsConfigStore` (YAML deux couches, globale + workspace — voir "Configuration CLI" plus bas), enveloppe les `vanyline-tools` en `ToolDyn` locaux, active la feature `k8s` de `vanyline-lib` (commandes owner/project/sandbox + toolbox) |
| `vanyline-app` | binaire | Backend du frontend | axum (REST + WS), OIDC, sqlx/PostgreSQL, `PgConfigStore` (adapte le schéma PG en `ConfigStore`), orchestration LLM via `vanyline-lib` — voir section "Backend web" plus bas |
| `vanyline-sandbox` | binaire | Pod serveur MCP | expose les 8 `vanyline-tools` via MCP (`tools_impl.rs`/`mcp.rs`, schémas partagés avec `cli`) — voir section "Serveur MCP" plus bas ; second binaire `vanyline-maint` (maintenance des workspaces par les Jobs du controller — voir section dédiée) |
| `vanyline-controller` | binaire | Opérateur Kubernetes | kube-rs, reconcilers des CRDs Owner/Project/Sandbox v1alpha1 (types importés de `vanyline-crds`) — voir section dédiée plus bas |

## Graphe de dépendances

```
vanyline-tools  ◄──  vanyline (cli),  vanyline-sandbox
vanyline-lib    ◄──  vanyline (cli),  vanyline-app
vanyline-crds   ◄──  vanyline-controller,  vanyline-lib (feature k8s, via vanyline (cli))

vanyline-tools  : aucune dépendance interne, pas de rig/rmcp
vanyline-lib    : aucune dépendance interne obligatoire (rig-core + rmcp externes) ;
                  feature optionnelle k8s -> vanyline-crds + kube (default-features = false,
                  "client" seulement — jamais "runtime", le CLI ne doit pas embarquer le
                  reconciler)
vanyline-crds   : aucune dépendance interne, kube en "derive" seul (pas de "client"/"runtime")
vanyline-controller : dépend uniquement de vanyline-crds (types CRD) parmi les crates du
                  workspace — aucune autre dépendance interne
```

Aucun cycle. Les trois feuilles sont indépendantes l'une de l'autre (`vanyline-crds` ne
dépend ni de `vanyline-tools` ni de `vanyline-lib`, et réciproquement).

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

6. **`vanyline-controller` ne partage QUE les types CRD.** Sa seule dépendance interne est
   `vanyline-crds` (specs/status Owner/Project/Sandbox + derives kube) — pas de code de
   reconciliation partagé, pas de dépendance sur `vanyline-lib`/`vanyline-tools`. Extrait de
   `controller/src/crds.rs` (mécanique, sans changement de sémantique) pour que `vanyline-lib`
   (feature `k8s`) puisse consommer les mêmes types sans embarquer le runtime opérateur
   (`kube-runtime`, reconcilers).

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
- `extra_mcp: Vec<(McpServer, McpSelection)>` — serveurs MCP forcés par l'hôte pour CE tour,
  en plus de ceux résolus via les toolsets de l'agent (fournis directement, pas par nom via
  `ctx.store` — symétrique de `local_tools`). Connectés AVANT la boucle des toolsets
  (l'`extra_mcp` "gagne" en cas de collision de nom de serveur, même dédup que le reste).
  Vide dans le cas général ; c'est le mécanisme de la **toolbox CLI**
  (`vanyline run --toolbox <sandbox>` / `defaults.toolbox`, feature `k8s`) — le CLI y résout
  l'URL MCP de la sandbox (`VnlK8sClient::sandbox_mcp_url`) et vide `local_tools` en même
  temps, remplaçant les outils locaux par ceux de la sandbox pour tout le tour (subagents
  inclus, `extra_mcp` est propagé au contexte imbriqué du tool `task`). Voir "Client K8s CLI"
  plus bas.

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
namespace d'erreur `VNL-RPC-000` à `VNL-RPC-010`), `handlers.rs`
(dispatch, `ServerState`, logique par méthode). Réutilise `FsConfigStore`
(config, tâche 02a) et `cli/src/store.rs` (conversations, format JSON
existant, tâche 02b) tels quels — aucun nouveau stockage introduit.

**Méthodes K8s** (`owners/*`, `projects/*`, `sandboxes/*`, miroir des
commandes CLI `owner`/`project`/`sandbox` — feature `k8s`, `VNL-RPC-010`
en cas d'erreur) : `VnlK8sClient` est construit **paresseusement**, au
premier appel de ce type, PAS à `initialize` — un cluster injoignable ne
doit jamais empêcher `chat/send`/`config/*` de fonctionner. Mis en cache
dans `ServerState.k8s_client`, remis à `None` à chaque `initialize`
(le namespace peut changer avec le workspace). **Limitation v1** : le
namespace est résolu une seule fois par session (`defaults.namespace` du
`config.yaml` fusionné, sinon le contexte kubeconfig courant) — pas de
param `namespace` par appel, contrairement au `--namespace` du CLI (qui
peut varier à chaque invocation). Détails et exemples : `docs/rpc-protocol.md`
section "Ressources K8s".

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

**Socle CLI étendu** (WS-13, 2026-08-01) : en plus du substrat natif (`gcc`/`libc6-dev`/
`binutils`/`make`/`pkg-config`/`git`/`curl`/`vim`/`ca-certificates`), l'image embarque
`ripgrep`, `fd-find` (symlink `fd` → `fdfind`, Debian nomme le binaire différemment),
`jq`, `procps`, `less`, `file`, `tree`, `patch`, `diffutils`, `unzip`, `openssh-client`,
`python3` — outils courants pour un agent LLM qui explore/édite du code. Pas d'outils
réseau/debug (`dnsutils`, `netcat`, `strace`) tant qu'un besoin réel ne les réclame pas.

### Endpoints git — `GET /git/status` et `GET /git/unpushed` (WS-11)

Deux endpoints REST (`sandbox/src/git.rs`), même middleware d'authentification que
`/mcp`. Servent l'app/le frontend — pas de tool MCP git (les LLM ont déjà
`execute_command` + git dans l'image). Pas d'action git (commit/push/merge), pas de
diff de contenu — des listes, pas des patches. Erreurs `VNL-SBX-004` (commande git en
échec), `VNL-SBX-005` (sortie git non reconnue par le parseur — préféré à un JSON
mensonger), `VNL-SBX-006` (HEAD détachée, seulement pour `/git/unpushed`).

- **`GET /git/status`** : parse pur de `git status --porcelain=v2 --branch` (exécuté
  dans `VNL_SANDBOX_ROOT`). `{ branch, files: [{ path, state, staged }], clean }`.
  `state` ∈ `modified | added | deleted | renamed | untracked | conflicted` — mapping
  depuis les colonnes X (staged)/Y (unstaged) porcelain v2, X prioritaire si non `.` ;
  `typechange` traité comme `modified`, `copied` comme `renamed` (pas d'état dédié dans
  ce schéma). HEAD détachée : `branch == "(detached)"` (littéral git, pas de sentinelle
  inventée) — pas une erreur pour cet endpoint, contrairement à `/git/unpushed`.
- **`GET /git/unpushed`** : `{ branch, upstream: Option<String>, commits: [{ sha,
  title, author, date }], truncated }`. Si `refs/remotes/origin/<branch>` existe →
  compare à `origin/<branch>` (`upstream` renseigné) ; sinon (branche créée par la
  sandbox, jamais poussée) → compare à `origin/<default>`, `default` résolu
  dynamiquement via le HEAD symbolique du dépôt bare (`git rev-parse
  --git-common-dir` depuis le worktree, puis `symbolic-ref --short HEAD` sur ce
  chemin — pas de chemin codé en dur côté sandbox), repli `"main"` sur tout échec.
  Sortie bornée à 200 commits (`truncated: true` au-delà). HEAD détachée → erreur
  `VNL-SBX-006` (la comparaison n'aurait pas de sens sans branche).
- **Fraîcheur** : les refs `origin/*` ont la fraîcheur du dernier `fetch` périodique du
  Project (cron), pas du remote instantané — aucun fetch déclenché par ces endpoints.
- **Dépend du mount `repo.git`** (cf. section "Opérateur Kubernetes" ci-dessous) et de
  la refspec de fetch (cf. section "Maintenance des workspaces" ci-dessous) — sans ces
  deux fixes (découverts pendant WS-11, préexistants au design initial), aucune
  commande git ne fonctionnait dans le pod sandbox.

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
| `sandbox.rs` | Job `sandbox-checkout` (`git worktree add`, création de branche depuis la default branch si absente) + Pod (home Owner + worktree en subPath + **second mount du même PVC sur `repo.git`, cf. ci-dessous** + un `volumes[].image` par toolchain + env agrégé PATH/LD_LIBRARY_PATH/caches — supprimé si `spec.suspended`, cf. ci-dessous) + Service ClusterIP (port MCP) + NetworkPolicy ingress (restreint aux pods du namespace portant `vanyline.solidite.fr/owner: <owner>`) + NetworkPolicy egress conditionnelle (WS-13, cf. ci-dessous) + finalizer (Job `git worktree remove`, la branche survit sur le remote). |

Tous les Jobs git (`project.rs`/`sandbox.rs`) invoquent `vanyline-maint` (image sandbox)
en argv — jamais de `sh -c`, aucun champ de CRD ne s'interpole dans une commande shell
(cf. section "Maintenance des workspaces" ci-dessous pour l'outil lui-même).

**Presets toolchain** (`sandbox.rs::toolchain_preset`) : la recette d'env validée (PATH,
`LD_LIBRARY_PATH` deux arches, `RUSTUP_HOME`…) vit ici, pas répétée dans chaque CR —
`Toolchain.env` vide déclenche le preset si `Toolchain.name` matche (`rust`, `node`),
sinon aucune variable ; `Toolchain.env` explicite remplace le preset entièrement.

**Mount `repo.git` dans le pod Sandbox** (`build_sandbox_pod`, WS-11) : `git worktree
add` (Job checkout, monté sur tout le PVC à `/workspace`) écrit dans le `.git` du
worktree un pointeur **absolu** — `gitdir: /workspace/repo.git/worktrees/<sandbox>`
(comportement standard de git). Le pod Sandbox ne montait jusqu'ici que le subPath
`worktrees/<sandbox>` : ce pointeur ne résolvait donc vers rien à l'intérieur du pod, et
toute commande git y échouait (bug préexistant à `controller-bootstrap`, jamais débusqué
faute de test e2e exerçant une vraie commande git). Fix : un second `VolumeMount` sur le
même `Volume` `workspace`, subPath `repo.git`, monté à `/workspace/repo.git` — même
chemin absolu que celui utilisé par les Jobs. Lecture-écriture (git écrit
`HEAD`/`index`/objets). Isolation préservée : seul l'object store partagé du Project
devient visible, jamais les worktrees des autres sandboxes.

### NetworkPolicies egress à trois niveaux (WS-13)

Champ `egress: Vec<EgressRule>` (`#[serde(default)]`, absent = liste vide = "ne
déclare rien") présent aux **trois** niveaux (`OwnerSpec`, `ProjectSpec`,
`SandboxSpec`, `vanyline-crds`). Une `EgressRule` a une `description` (obligatoire,
auto-documentation de la white-list), soit `cidr` soit `pod_selector`/
`namespace_selector` (exclusifs — `cidr` gagne si les deux sont renseignés,
non validé au niveau du type, tranché à la construction de la netpol), et des
`ports` optionnels (liste vide = tous les ports).

`build_sandbox_egress_netpol` (`sandbox.rs`, pure, testée sans cluster) construit
l'union Owner + Project + Sandbox : **`None`** si les trois listes sont vides (pas
de netpol produite, egress libre) ; sinon une `NetworkPolicy` **distincte** de la
netpol ingress (`sandbox-<name>-egress` vs `sandbox-<name>`), avec toujours en tête
une règle DNS port 53 UDP+TCP **sans restriction de destination** (décision
2026-08-01 : pas de `podSelector`/`namespaceSelector` ciblant kube-dns, pour ne
dépendre d'aucune convention de labels du cluster — le risque d'un mauvais choix de
label y aurait cassé silencieusement toute résolution DNS des sandboxes à egress
restreint). `apply()` du reconciler Sandbox patch la netpol egress quand elle
existe, la supprime explicitement sinon (transition vers "plus aucune règle" —
contrairement à la netpol ingress, la GC par ownerReference seule ne suffit pas
puisque l'objet doit disparaître sans que la Sandbox elle-même soit supprimée).

**Propagation sans watch inter-CRD permanent** : un changement sur `Sandbox.spec`
se réconcilie déjà immédiatement (watch natif kube-runtime). Pour qu'un changement
sur `Owner.spec.egress`/`Project.spec.egress` se propage aussi vite sans watch
permanent supplémentaire, `owner.rs::reconcile()` et `project.rs::apply()` patchent,
à **chaque** reconcile (inconditionnellement, pas de détection de changement — choix
délibéré, cohérent avec le reste du controller qui ne diff jamais rien nulle part),
une annotation de bump (`vanyline.solidite.fr/egress-bump`, valeur = timestamp) sur
les Sandboxes concernées (directes pour Project, via ses Projects pour Owner) — cette
écriture déclenche leur propre watch, donc leur reconcile immédiat. Coût borné par
l'intervalle de requeue déjà en place (300s), pas un watch permanent additionnel.

### Suspension manuelle (WS-13)

`SandboxSpec.suspended: bool` (défaut `false`). `true` → le reconciler supprime le
**Pod** uniquement (worktree/PVC/Service/NetworkPolicies conservés), `status.phase`
devient `"Suspended"` (condition `Ready: False`, reason `Suspended` — distinct de
`NotRunning` pour ne pas confondre arrêt volontaire et échec/provisioning). `false` →
le Pod est recréé ; le Job checkout ne re-tourne pas s'il a déjà réussi (mécanique
d'idempotence déjà en place, inchangée) — c'est le worktree conservé qui permet une
reprise rapide. Le Job checkout n'est jamais conditionné par `suspended` : même une
Sandbox créée directement `suspended: true` obtient son worktree. Pas d'auto-arrêt
sur inactivité (décision 2026-07-12 : manuel uniquement). Requeue 300s en régime
stable (`Running` et `Suspended`), comme pour le reste du reconciler. Piloté par
`vanyline sandbox stop|start` — commandes CLI **pas encore câblées** : périmètre
restant de `ws12-sandbox-clients`, qui n'attendait que ce champ.

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
(immutable en v1). Pas de résolution FQDN dans les règles egress (limite K8s :
`ipBlock`/selectors, pas de nom de domaine — si le besoin FQDN devient réel, chantier
CNI type CiliumNetworkPolicy, hors scope v1). Pas d'auto-arrêt sur inactivité
(`suspended` est manuel uniquement, décision 2026-07-12).

## Client K8s CLI — `vanyline-crds`, `VnlK8sClient`, toolbox

Rend les Owners/Projects/Sandboxes pilotables **hors du cluster-admin** —
`kubectl`/accès direct au cluster n'est plus le seul moyen d'agir dessus.
Pas d'UI app/frontend pour ces objets en v1 (viendra avec la convergence
app ↔ sandbox) : API/CLI d'abord.

**`vanyline-crds`** (lib feuille, voir "Vue d'ensemble") : extraction
mécanique des types CRD depuis `controller/src/crds.rs`, `kube` en
`default-features = false, features = ["derive"]` — jamais `client`/
`runtime`, sinon le CLI embarquerait la même machinerie réseau que
l'opérateur. `service_name`/`MCP_PORT` (nommage du Service MCP d'une
sandbox) y vivent aussi désormais — seule source de vérité pour
`controller` (qui pose le Service) ET `vanyline-lib` (qui doit résoudre
la même URL depuis le CLI).

**`VnlK8sClient`** (`lib/src/k8s.rs`, feature Cargo `k8s` de
`vanyline-lib`, désactivée par défaut — seul le CLI l'active) :
- `discover(namespace_override: Option<String>)` — `kube::Config::infer()`
  (in-cluster ou kubeconfig), erreur `VNL-K8S-001` si injoignable.
  `namespace_override` prime sur le namespace du contexte kubeconfig
  courant si fourni.
- `list/get/create/delete_{owner,project,sandbox}` — CRUD générique par
  petites fonctions privées paramétrées sur `K: kube::Resource<...>`
  (évite la répétition x3 types x4 opérations), erreurs `VNL-K8S-002`.
- `sandbox_mcp_url(name)` — vérifie d'abord que la sandbox existe
  (`get_sandbox`, erreur claire plutôt qu'un échec de connexion confus
  plus tard) puis construit `http://<service_name(name)>.<ns>.svc:<MCP_PORT>/mcp`.
- `set_sandbox_suspended(name, suspended)` — patch merge JSON ciblé sur
  `spec.suspended` (pas de fonction générique partagée avec le CRUD
  ci-dessus : un seul type appelant, une abstraction à un seul site
  d'appel serait prématurée), retourne le `Sandbox` patché. Le champ
  `suspended` est posé par `ws13-sandbox-runtime` (voir "Opérateur
  Kubernetes" pour la sémantique côté controller) ; cette méthode ne fait
  que le patcher, aucune logique de suspension côté client.

**Convention de test, alignée sur "Opérateur Kubernetes" ci-dessus** :
aucun appel `Api<K>::list/get/create/delete` n'est unit-testé contre un
cluster réel ou mocké — même principe que les reconcilers du controller.
**Différence avec les connexions MCP** (voir "Session engine" plus haut) :
celles-ci SONT testées avec un vrai serveur HTTP local
(`lib/tests/mcp_connection_lifecycle.rs`) — la distinction tient à la
légèreté de monter un serveur HTTP en local (trivial) contre celle de
simuler une API server Kubernetes (pas d'équivalent léger disponible).

**CLI** (`vanyline owner/project/sandbox list|show|create|delete`,
`vanyline sandbox stop|start`, `cli/src/{owner,project,sandbox}_cmd.rs`) :
`stop`/`start` suivent le même patron que `delete` (juste le nom en
argument), délèguent à `set_sandbox_suspended`. Mêmes conventions que les
commandes de config existantes (sortie tabulaire). `create` prend des
flags `clap` complets par ressource (pas de `-f fichier.yaml` — jugé sans
valeur ajoutée sur un `kubectl apply -f` direct, et chaque commande n'a
qu'une poignée de champs). `--toolchain NAME=IMAGE` répétable pour
`sandbox create` (`env` vide → preset controller si `NAME` est connu,
cf. "Opérateur Kubernetes"). `resources` (`SandboxSpec`) non exposé en
flags v1 (structure `ResourceRequirements` sans mapping raisonnable, cas
rare — édition via `kubectl` si besoin). Namespace résolu par précédence :
`--namespace` (flag global) > `defaults.namespace` (`config.yaml`,
fusionné deux couches) > namespace du contexte kubeconfig courant.

**RPC stdio** : `owners/*`, `projects/*`, `sandboxes/*` (incl.
`sandboxes/stop`/`sandboxes/start`), voir section "RPC stdio" ci-dessus et
`docs/rpc-protocol.md`.

**Toolbox en inférence** (`vanyline run --toolbox <sandbox>` / REPL,
`defaults.toolbox`) : résout l'URL MCP de la sandbox
(`VnlK8sClient::sandbox_mcp_url`), construit le `SessionContext` avec
`local_tools` vide et la sandbox injectée dans `extra_mcp` (voir "Session
engine" ci-dessus pour le mécanisme lib). Toute la logique K8s reste dans
`cli/src/main.rs` — `cli/src/chat.rs` reçoit une URL déjà résolue
(`Option<String>`), ne dépend pas de K8s, reste testable sans réseau.
CLI uniquement en v1, pas de toolbox sur `chat/send` (RPC).

**Hors scope v1** : joignabilité de l'URL MCP depuis un CLI hors cluster
(cas nominal : le CLI tourne dans le cluster, ex. pod code-server — un
port-forward manuel fonctionne déjà pour le reste) ; auth SA TokenReview
sur la sandbox pour ce chemin (les sandboxes tournent en `--no-auth`
derrière NetworkPolicy — le CLI dans le cluster passe si ses
labels/namespace le permettent, cf. "Serveur MCP" plus haut).

## Maintenance des workspaces — `vanyline-maint` (crate sandbox)

Second binaire du crate `vanyline-sandbox` (`sandbox/src/bin/maint.rs`, logique dans
`sandbox/src/maint.rs`). C'est l'outil que les Jobs du controller exécutent dans un pod
à image sandbox pour toute maintenance d'un workspace Project — la règle système
correspondante ("l'image sandbox est l'outil de maintenance du controller") est dans
`AGENTS.md`, section controller.

| Sous-commande | Rôle |
|---------------|------|
| `init --repo <url> --workspace <dir> [--cache <name>]...` | mkdir des caches + clone bare si absent (idempotent) + pose la refspec de fetch (idempotent, cf. décisions) |
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
- **Refspec de fetch posée par `init`** (WS-11) : `git clone --bare` ne configure aucune
  refspec de fetch (vérifié : `[remote "origin"] url = ...` sans ligne `fetch =`) — le
  `fetch` périodique ne rafraîchissait donc que `FETCH_HEAD`, jamais les refs. `init` pose
  désormais `git config --replace-all remote.origin.fetch
  '+refs/heads/*:refs/remotes/origin/*'` (idempotent). Cible `refs/remotes/origin/*`, pas
  `refs/heads/*` — ne doit jamais écraser les branches locales des worktrees. C'est
  directement ce que `GET /git/unpushed` interroge (cf. section "Serveur MCP" ci-dessus).

## Gouvernance qualité — jobs CI (WS-15)

Quatre jobs CI (`.github/workflows/test.yml`, en plus de `test`/`fmt`/`clippy` déjà
existants), tous adossés à un chiffre mesuré directement par le compilateur/clippy —
jamais un script `grep`/`awk` maison qui approxime ce qu'ils mesurent déjà exactement.

| Job | Rôle | Baseline (2026-08-01) | Bloquant ? |
|---|---|---|---|
| `clippy` (existant) | Niveau clippy défaut, `-D warnings` | 0 warning | Oui, gate historique |
| `doc-lint` | `missing_docs` sur `lib`/`app`/`sandbox`/`tools`/`controller`/`crds` (`cli/` exclu) | 621 items non documentés | Oui, en régression uniquement |
| `clippy-pedantic` | `clippy::pedantic`/`clippy::nursery`, workspace entier | 585 warnings | Non — annotation seulement |
| `coverage` | `cargo-llvm-cov`, push sur `main` uniquement | 74,97 % lignes | Non — mesure seule, pas de seuil |

`unwrap_used`/`expect_used` n'a plus de job dédié : les 6 crates non-cli (`lib`, `app`,
`sandbox`, `tools`, `controller`, `crds`) sont tous en `#![deny(clippy::unwrap_used,
clippy::expect_used)]` directement en source — le job à cliquet `unwrap-lint` qui a existé
le temps de corriger les crates un par un a été supprimé une fois `deny` posé partout.

### Quatre pièges vérifiés empiriquement (pas des suppositions)

- **`#![warn(X)]` en source a une précédence absolue sur tout flag `-A`/`-D` de ligne de
  commande** — vérifié en inversant l'ordre des flags, aucune combinaison ne permet à
  `-A missing_docs`/`-A clippy::unwrap_used` de neutraliser un `#![warn(...)]` déjà présent
  dans le fichier. `-D warnings` promeut ensuite ce warning en erreur bloquante. Conséquence :
  **ne jamais poser `#![warn(clippy::X)]` ou `#![warn(missing_docs)]` en source** — les jobs
  à cliquet (`doc-lint`, `clippy-pedantic`) activent leur lint eux-mêmes via `-W` en ligne de
  commande à chaque invocation, sans dépendre d'un attribut en source. Seul `#![deny(...)]`
  (l'état final, une fois un crate propre) est sans risque : il converge avec `-D warnings`
  vers "erreur" des deux côtés, pas de précédence à arbitrer.
- **`cargo check`/`cargo test` n'exécutent jamais les lints clippy**, y compris
  `#![deny(clippy::X)]` — ce sont des lints clippy, pas des lints rustc ; `cargo check`/
  `cargo build`/`cargo test` utilisent rustc seul et ignorent silencieusement tout
  `#![deny(clippy::X)]` en source (aucune erreur, aucun warning). Seul `cargo clippy` connaît
  et applique ces lints. **La seule validation qui vaut, après toute tâche posant un
  `#![deny(clippy::X)]`/`#![warn(clippy::X)]`, est `cargo clippy --workspace --all-targets --
  -D warnings`** (`--all-targets` inclus, pour couvrir aussi le code de test — un module
  `#[cfg(test)] mod tests { ... }` qui utilise `.unwrap()` casse le `deny` sans que
  `cargo check`/`cargo test` ne le voient jamais).
- **La compilation incrémentale de rustc/cargo sous-compte les diagnostics de façon non
  déterministe** — un cache incrémental tiède (hérité d'invocations précédentes avec
  d'autres combinaisons de flags dans le même `target/`) peut sous-compter un total de
  plusieurs centaines par rapport à un build propre (`CARGO_INCREMENTAL=0` ou `cargo clean`
  préalable). Vérifié à deux reprises avec des écarts significatifs : `missing_docs` sur
  `sandbox` (109 vs 169 réel), `clippy::pedantic`+`clippy::nursery` sur tout le workspace
  (583 vs 959 réel avant les corrections unwrap). **`CARGO_INCREMENTAL=0` est obligatoire
  dans tout job CI qui compte des warnings** (`Swatinem/rust-cache@v2` conserve le cache
  entre les runs GitHub Actions — sans ce garde-fou, le comptage en CI serait aussi
  instable qu'en local).
- **`cargo clippy -p <crate> -- -W <lint>` (contrairement à `cargo rustc -p <crate> -- -W
  <lint>`) propage les flags de lint aux dépendances internes du workspace compilées dans
  la même invocation.** `controller` dépend de `crds` (path dependency) : sans `--no-deps`,
  compter les warnings de `controller` comptait aussi ceux de `crds`, doublant certaines
  occurrences. `--no-deps` est donc obligatoire pour toute mesure `cargo clippy -p <crate>`
  dans ce projet. `cargo rustc -p <crate> -- -W missing_docs` n'a pas ce problème (vérifié :
  résultat identique avec et sans `--no-deps`).

### Limite d'outillage — Qwen et les grosses tâches

Le modèle Qwen sous-jacent (`llm-exec`, context window 131K tokens) peut échouer par
compaction de contexte sur une tâche déléguée qui touche beaucoup de fichiers volumineux,
même bien spécifiée — la session se compacte en cours de route et finit par poser une
question au lieu d'agir (indépendant de la permission `question: deny`, qui ne bloque que
les appels d'outil, pas du texte de fin de tour). Observé sur une tâche couvrant `sandbox`
et `controller` combinés (~6000 lignes de fichiers source à lire) : deux échecs malgré une
spécification déjà complète. Scinder par crate a réduit le risque. Quand le contrat d'une
tâche est déjà entièrement écrit et le risque de récidive élevé, appliquer directement les
modifications plutôt que de multiplier les tentatives de délégation est plus efficace — ce
n'est pas un problème de spécification qu'une réécriture peut résoudre, c'est une limite
matérielle de l'outil.

## Limites connues (dette assumée, pas oubliée)

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
- **Pas de `vanyline sandbox stop/start`** : bloqué sur WS-13 (champ
  `suspended` absent de `SandboxSpec`), cf. section "Client K8s CLI"
  ci-dessus et `docs/features/ws12-sandbox-clients.md`.
- **Namespace RPC résolu une fois par session** (`owners/*`/`projects/*`/
  `sandboxes/*`) : pas de param `namespace` par appel, contrairement au
  `--namespace` du CLI — cf. section "RPC stdio" ci-dessus.
- **`cargo clippy --workspace --all-targets`** est la commande utilisée par
  la CI (`clippy` job de `.github/workflows/test.yml`), pas
  `cargo clippy --workspace` seul (documenté dans `AGENTS.md`) : `--all-targets`
  inclut aussi le code de test — cf. section "Gouvernance qualité — jobs CI (WS-15)"
  ci-dessus pour les pièges découverts depuis (précédence des attributs `#![warn(...)]`,
  `cargo check`/`cargo test` qui n'exécutent jamais les lints clippy). Le pattern
  `MutexGuard` tenu across `.await` dans les tests RPC (`isolated_data_dir()`, cf.
  section "RPC stdio" ci-dessus) déclenche un faux positif `clippy::await_holding_lock`
  — `allow` documenté au niveau du module de test plutôt que de restructurer les tests.

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
