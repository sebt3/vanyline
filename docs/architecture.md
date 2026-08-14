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
| `vanyline-crds` | lib feuille | Types CRD Owner/Project/Sandbox/Application (spec/status/derives kube), sans runtime opérateur | `Owner`/`Project`/`Sandbox`/`Application` + specs/status, `Toolchain`/`PvcRef`/`ProjectDefaults`/`IngressControllerRef`, `crd_manifests()`, `service_name`/`MCP_PORT` (convention de nommage du Service MCP d'une sandbox, partagée avec `vanyline-lib`) — voir sections "Client K8s CLI" et "Opérateur Kubernetes" plus bas |
| `vanyline-lib` | lib feuille | Cœur partagé LLM / MCP / chat — harness (agents, toolsets, skills, subagents) | `domain` (types name-keyed : `Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`…), `store::ConfigStore` (résolution de config par nom), `event` (`ChatEvent`/`EventSink`), `model` (construction de modèle + params), `session` (`SessionContext`, `run_agent_turn` — point d'entrée unique), `builtin` (tools `skill`/`task`), `prefixed_mcp` (connexion MCP filtrée par toolset), `types` (`ToolCall`/`Message`/`Conversation` — formats de persistance propres à chaque binaire), `k8s` (`VnlK8sClient`, **feature Cargo optionnelle `k8s`**, désactivée par défaut — voir "Client K8s CLI" plus bas), erreurs `VNL-*` |
| `vanyline` (bin `cli`) | binaire | CLI standalone de chat/agents | `run`/REPL, `FsConfigStore` (YAML deux couches, globale + workspace — voir "Configuration CLI" plus bas), enveloppe les `vanyline-tools` en `ToolDyn` locaux, active la feature `k8s` de `vanyline-lib` (commandes owner/project/sandbox + toolbox) |
| `vanyline-app` | binaire | Backend du frontend | axum (REST + WS), OIDC, sqlx/PostgreSQL, `PgConfigStore` (adapte le schéma PG en `ConfigStore`), orchestration LLM via `vanyline-lib`, client `VnlK8sClient` (feature `k8s`) pour piloter Owner/Project/Sandbox/Application et relayer les tickets WS de la sandbox — voir section "Backend web" plus bas |
| `vanyline-sandbox` | binaire | Pod serveur MCP + éditeur | expose les 8 `vanyline-tools` via MCP (`tools_impl.rs`/`mcp.rs`, schémas partagés avec `cli`) + WebSocket éditeur (`/ws/ticket`, `/ws/fs`, `/ws/terminal`) — voir section "Serveur MCP" plus bas ; second binaire `vanyline-maint` (maintenance des workspaces par les Jobs du controller — voir section dédiée) |
| `vanyline-controller` | binaire | Opérateur Kubernetes | kube-rs, reconcilers des CRDs Owner/Project/Sandbox/Application v1alpha1 (types importés de `vanyline-crds`) — voir section dédiée plus bas |

## Graphe de dépendances

```
vanyline-tools  ◄──  vanyline (cli),  vanyline-sandbox
vanyline-lib    ◄──  vanyline (cli),  vanyline-app
vanyline-crds   ◄──  vanyline-controller,  vanyline-lib (feature k8s, via vanyline (cli)
                  et vanyline-app)

vanyline-tools  : aucune dépendance interne, pas de rig/rmcp
vanyline-lib    : aucune dépendance interne obligatoire (rig-core + rmcp externes) ;
                  feature optionnelle k8s -> vanyline-crds + kube (default-features = false,
                  "client" seulement — jamais "runtime", ni le CLI ni l'app ne doivent
                  embarquer le reconciler). Activée par `cli` (commandes owner/project/
                  sandbox + toolbox) et par `app` (routes REST /api/projects,
                  /api/sandboxes — app-k8s-provisioning) ; jamais par défaut.
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
- `todo_state: Arc<std::sync::Mutex<Option<String>>>` — handle du **tool builtin
  `todowrite`/`todoread`** pour CE tour : serialisation JSON de la liste de tâches
  (`[{"content":..., "status":...}]`), `None` = aucun état posé. L'hôte (CLI) sème depuis
  `Conversation.todo` et lit après le tour pour persister — c'est la seule forme d'état
  resumable en une-passe (`-c/--continue`).
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
- `todo` : `todowrite(todos)` **remplace tout** l'état todo par la liste fournie
  (comportement opencode) et `todoread()` renvoie l'état courant (`no todo list yet` si
  aucun) — écrit/lit le handle partagé `SessionContext.todo_state`. Toujours exposés (pas
  de dépendance à un index), à la différence de `skill`/`task`. L'état est **persisté sur
  `Conversation.todo`** par le CLI (seed + relecture après tour).

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

**Correspondance opencode → vanyline** : les agents `.opencode/agents/*.md`
peuvent être copiés tels quels dans `.vanyline/agents/*.md`. Le frontmatter
est partiellement compatible — `RawAgentFrontmatter` ne déclare que
`description`, `mode`, `model`, `toolsets`, `skills`. Tout champ non déclaré
est ignoré par serde, ce qui verrouille le comportement. Table complète :

| opencode | vanyline | Statut |
|----------|----------|--------|
| `description` | `description` | direct |
| `mode` | `mode` (`AgentMode`) | direct |
| `model` | `model` | direct |
| `temperature` | sur `ModelProfile` (config.yaml), PAS l'agent | **ignoré** (single source, décision 2026-08-02) |
| `permission` | — | sans objet (philosophie yolo, aucun système de permissions) |
| `steps` | — | ne pas exposer (`max-turns` = filet anti-boucle interne, pas un plafond de travail) |
| `color` | — | UI only, sans objet backend |
| `disable`/`hidden`/`top_p` | — | sans équivalent |

**`vanyline run` — flags backend d'exécution** (backlog ws14, pour remplacer le wrapper
`llm-exec`) : `-m/--model <nom>` (override du modèle de l'agent pour ce run, sans toucher
la config), `-t/--timeout <secs>` (timeout global du tour, `0` = aucune limite, erreur
`VNL-CLI-001` et exit 1 au-delà), `-j/--json` (sortie structurée — supprime l'en-tête de
sources workspace et le `git diff --stat`). En fin de `run` **en mode texte** et sur
succès, `run_one_shot` affiche `git diff --stat` (comportement du wrapper `llm-exec`
reproduit, `cli/src/chat.rs`) : résout la racine workspace depuis le cwd
(`config::discover_workspace_root`), silencieux hors dépôt git, si `git` échoue ou si le
diff est vide.

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
dès que providers/mcp servers sont eux aussi scopés par utilisateur). `AuthUser.id_token`
(le JWT OIDC brut, jamais exposé au JS — cf. section "WebSocket éditeur" plus bas) sert
depuis `sandbox-ingress-wiring` au relais de ticket WS vers la sandbox.

**Stockage** : PostgreSQL/sqlx, migrations `app/migrations/0001_initial.sql` (schéma
MVP) + `0002_harness_parity.sql` (tables `model_profiles`/`toolsets`/`skills`, `agents`
v2 name-keyed, `user_id`+`UNIQUE(user_id, name)` ajoutés à `llm_providers`/
`mcp_servers`). `config_store.rs::PgConfigStore` implémente `vanyline_lib::ConfigStore`
sur ce schéma — une instance par requête, scopée `user_id`, convertit les lignes en
types name-keyed de la lib (le nom remplace l'UUID en sortie, comme `FsConfigStore`
côté cli). `load_skill` lit `skills.body` à la demande, même paresse que le cli.

**API REST** (`api/*.rs`, `api::api_router`) : CRUD par nom pour `model-profiles`,
`toolsets`, `skills`, `agents` ; CRUD par id pour `llm-providers`/`mcp-servers`
(`{id}/test` = discovery, `{id}/default` pour les providers) ; `conversations` +
`messages` ; `projects`/`sandboxes` (feature `k8s`, cf. ci-dessous) ; `/me` (email +
`k8s_owner_name`). Toutes les routes exigent `AuthUser` et scopent par utilisateur.

**Discovery — providers et serveurs MCP** (`frontend-dashboards-nav`) : même patron sur
les deux entités qui référencent une liste de choix, jamais peuplée à la création,
seulement après un test explicite. `POST /api/llm-providers/{id}/test` interroge le
provider et persiste les noms de modèles dans `llm_providers.available_models`.
`POST /api/mcp-servers/{id}/test` fait de même pour les tools exposés par un serveur
MCP : réutilise `connect_domain_mcp_server_inner`/`list_all_tools()` de
`lib/src/prefixed_mcp.rs` (déjà écrit pour la connexion runtime des toolsets, pas un
nouveau client), persiste dans `mcp_servers.available_tools` (colonne ajoutée par
`frontend-dashboards-nav`). Les deux listes alimentent des dropdowns dépendants côté
frontend (cf. section Frontend plus bas) — état vide explicite tant que le test n'a
jamais été lancé. `GET /api/local-tools` (nouveau, lecture seule, pas de DB) expose le
registre statique `tools::mcp::{filesystem_tools, search_tools, command_tools}` — les 8
tools intégrés de `vanyline-tools` — pour le même usage de dropdown côté `Toolset.local_tools`.

**WebSocket chat** (`ws/chat.rs`) : `run_agent_turn` avec `local_tools` vide (l'app
reste sur le chemin froid — cf. règle de dépendances plus haut). `ChannelSink` pousse
chaque `ChatEvent` sur un canal mpsc dès son émission ; une tâche `forward_events` par
connexion (pas par tour) draine le canal et écrit sur le socket au fil de l'eau — vrai
streaming token-par-token, contrairement à l'ancien `CollectingSink` qui bufferisait
tout un tour avant le premier octet (limite documentée pendant la migration
harness-core, résolue par la tâche `ws-chatevent`). **Persistance todo**
(`chat-todo-live`) : `SessionContext.todo_state` (builtin `todowrite`/`todoread` de
`vanyline-lib`, inconditionnels sur tout `run_agent_turn`) est semé depuis
`conversations.todo` au début de `handle_message`, relu et persisté après le tour
seulement s'il a changé (jamais d'update systématique qui écraserait un état
antérieur à `NULL`) — même patron que le fix `f4dfbf9` déjà en place côté CLI, migré
ici (`app/migrations/0003_conversation_todo.sql`).

**Résolution Owner** (`api/owners.rs`, `app-k8s-provisioning`) : `users.k8s_owner_name`
(colonne nullable) fait le lien entre un utilisateur OIDC et un Owner K8s — aucun
mécanisme automatique avant cette feature (l'Owner se créait jusque-là uniquement à la
main via le CLI). `resolve_owner_name` (lecture, ne crée rien — routes GET répondent
« aucun Owner » si absent) et `ensure_owner` (crée l'Owner si nécessaire et persiste le
nom résolu, **réservé au seul `POST /api/projects`** — décision développeur : lazy
provisioning restreint, pas de création implicite sur une route de lecture).
`sanitize_owner_name` dérive un nom de ressource K8s valide (RFC1123 : début **et
fin** alphanumériques) depuis l'email ou l'`oidc_sub` — la troncature à 63 caractères
retrim explicitement les tirets de fin (un bug corrigé en revue : une coupe pile sur
un `-` produisait un nom invalide).

**Routes `projects`/`sandboxes`** (`api/projects.rs`, `api/sandboxes.rs`,
`app-k8s-provisioning` + `sandbox-ingress-wiring`) : wrappers fins autour de
`VnlK8sClient` (feature `k8s` de `vanyline-lib`, activée côté `app` — absente par
défaut). Scoping IDOR systématique : chaque `get`/`delete`/`update` vérifie que le
Project référencé (directement, ou via la Sandbox pour les routes sandbox)
appartient bien à l'Owner de l'utilisateur authentifié, pas seulement au moment de la
création. `POST /api/sandboxes/{name}/ws-ticket` : relais de ticket WS (cf. section
"WebSocket éditeur" de `vanyline-sandbox` plus bas) — résout `owner →
application_ref → host` via K8s, appelle `POST /ws/ticket` de la sandbox en
interne (ClusterIP, `Authorization: Bearer {id_token OIDC de l'utilisateur}`) et ne
renvoie au navigateur que `{ ticket, wsHost }`, jamais le JWT.

**Déploiement** : image `ghcr.io/sebt3/vanyline-app:0.1.0`, build podman
multi-stage (node → rust → debian-slim), manifestes `deploy/web/` (dont
`RestEndPoint_sso.yaml` — kuberest provisionne l'app OIDC dans Authentik). Depuis
`controller-application-crd`, peut aussi être déployé via la CR `Application` du
controller (Deployment/Service/Ingress générés, secrets OIDC/DB/cookie référencés ou
auto-générés — cf. section "Opérateur Kubernetes" plus bas) ; les deux méthodes de
déploiement coexistent, aucune n'a remplacé l'autre.

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
démarrer sans le flag explicite) ou `STATIC_TOKEN` (démo, bypasse l'OIDC). **Correction
(2026-08-12, découverte pendant `sandbox-ingress-wiring`)** : `AGENTS.md` documentait
un second mécanisme, SA TokenReview, pour `app`/kydah-code — jamais implémenté, seul le
JWT/JWKS existe réellement. `app` l'utilise désormais pour de vrai (relais de ticket
WS, cf. section "Backend web" plus haut), en présentant le `id_token` OIDC de
l'utilisateur authentifié — pas un compte de service anonyme, `app` agit en son nom.
kydah-code ne consomme toujours pas la sandbox (pas démarré).

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
`ghcr.io/sebt3/vanyline-sandbox:0.1.0`.

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

### WebSocket éditeur — `/ws/ticket`, `/ws/fs`, `/ws/terminal` (`sandbox-ws-runtime`)

Trois routes ajoutées au serveur MCP existant (même port, `MCP_PORT`) — le prérequis
pour qu'un navigateur puisse éditer/utiliser un terminal sur une sandbox, jusque-là
seulement accessible en JSON-RPC MCP.

**Auth par ticket, pas par header.** L'API WebSocket native du navigateur ne permet
pas de poser un header `Authorization` sur le handshake. `POST /ws/ticket` (derrière
le `require_auth` JWT standard) émet un ticket opaque, court-vécu (30s,
`TICKET_TTL_SECS`), à usage unique — stocké en mémoire (`TicketStore`, un `HashMap`
sous mutex, pas de persistance : une sandbox = un process). `GET /ws/fs` et
`GET /ws/terminal` sortent du router `require_auth` classique : un middleware dédié
(`ws_auth_middleware`) lit `?ticket=` en query string, le consomme (`redeem` — retiré
de la map qu'il soit valide ou expiré, jamais réutilisable) avant l'upgrade
WebSocket. Contourne un piège axum connu : un handler `WebSocketUpgrade` qui
retournerait une réponse d'erreur (401) après extraction se ferait convertir en 426
par le framework — le rejet doit avoir lieu **avant** l'extracteur d'upgrade, d'où le
middleware plutôt qu'une vérification inline dans le handler.

**`/ws/fs`** : protocole JSON requête/réponse dédié (pas le JSON-RPC MCP, taillé pour
des tool calls LLM) — `{"op":"read|write|edit|delete|list","path":...}` →
`{"ok":true|false,...}`, dispatch vers les mêmes fonctions `vanyline_tools::filesystem::*`
que le chemin MCP, même confinement (`tools_impl::confine_path`). **Aucun champ de
corrélation** : strictement une requête lue, une réponse écrite, dans cet ordre — un
client qui partage la connexion entre plusieurs consommateurs (cf. `SandboxFsClient`
côté frontend) doit sérialiser ses appels, pas un détail d'implémentation optionnel.
`read` numérote les lignes et tronque par défaut (`vanyline_tools::filesystem::read_file`
réutilisé tel quel, taillé pour des sorties LLM/MCP) — **mode brut** ajouté
(`explorer-editor-terminal-wiring`, découverte en implémentant l'éditeur) :
`{"op":"read","path":...,"raw":true}` renvoie le contenu réel, non numéroté, non
tronqué (`ReadFileOptions.raw: bool`, `#[serde(default)]` → `false`, comportement par
défaut inchangé) — sans lui, `Ctrl+S` aurait réécrit un fichier corrompu (contenu
numéroté/tronqué). Aucune limite de taille en mode brut au-delà de ce que `read_to_string`
lisait déjà en mémoire dans les deux modes — pas un nouveau risque, juste un plafond
de troncature qui ne s'applique plus à la réponse envoyée.

**`/ws/terminal`** : PTY réel (`portable-pty`), pas un simulateur. Frames WS binaires =
octets stdin/stdout du PTY ; frame texte JSON de contrôle pour le resize
(`{"type":"resize","cols":N,"rows":N}`) ; axum répond automatiquement aux `Ping` par
un `Pong` — les traiter comme une fermeture (erreur corrigée en revue) coupe le
terminal au premier keepalive d'un proxy/client. Boucle de proxy `tokio::select!`
**volontairement non `biased`** : avec `biased`, un flux de sortie continu (commande
verbeuse) rendrait la lecture PTY quasi toujours prête en premier et affamerait
indéfiniment la lecture WS — donc le clavier de l'utilisateur, Ctrl-C compris.

**Cycle de vie du process — décidé après une fausse piste, vérifiée empiriquement.**
`kill_process_group` (SIGKILL sur `-pgid` du shell) tue le shell et ses jobs
**foreground** — via un mécanisme à deux étages, pas le kill seul : le shell est
leader de session (`setsid`), sa mort déclenche un hangup **noyau** du terminal
contrôlant qui envoie SIGHUP au groupe de processus **actuellement foreground** ; sous
contrôle de job bash, chaque commande externe (foreground ou backgroundée) reçoit son
propre pgid, distinct de celui du shell. Un job explicitement backgroundé (`cmd &`)
n'est jamais le groupe foreground du terminal — **il survit délibérément** à la
fermeture du terminal, vérifié via `/proc/<pid>/stat` (champ pgrp) avant d'écrire ce
paragraphe, pas supposé. Ce n'est pas qu'une tolérance de la sémantique Unix standard :
c'est nécessaire pour un cas d'usage déjà identifié (lancer un serveur de dev en
arrière-plan dans la sandbox, l'exposer ensuite via un Service/Ingress) — le tuer à la
fermeture du terminal casserait ce cas d'usage avant même qu'il existe. Contre-partie
notée, pas traitée : un job backgroundé oublié consomme des ressources du pod sans
limite garantie (`SandboxSpec.resources` reste optionnel).

## Opérateur Kubernetes — `vanyline-controller`

kube-rs, quatre CRDs namespacées (`vanyline.solidite.fr/v1alpha1`) réconciliées par un
reconciler chacun, tournant en parallèle dans le même process (`main.rs::tokio::join!`) :

```
Owner (1) ────────── (n) Project ─────────── (n) Sandbox
SA + PVC home RWX       PVC workspace RWO       pod = worktree d'une branche
(clés, dotfiles)        repo git bare + caches   (monte home + workspace + toolchains)
  │
  │ application_ref (optionnel)
  ▼
Application (0..1 référencée par Owner)
Deployment + Service + Ingress d'app — voir sous-section dédiée
```

**Pourquoi quatre CRDs** : un CRD se justifie par un état désiré à réconcilier, pas par
la possession d'un objet natif. Owner = identité (ServiceAccount `owner-<name>` — la
seule chose réellement implémentée aujourd'hui pour l'auth machine-à-machine ; le SA
TokenReview décrit ailleurs pour kydah-code/l'app n'a jamais été construit, cf.
correction dans la section "Serveur MCP" plus haut), home, référence optionnelle vers
une Application. Project = workspace, repo git, caches, Jobs/CronJob de maintenance.
Sandbox = pod + branche + (si l'Owner référence une Application) Ingress public.
Application = instance déployée d'`app` (Deployment/Service/Ingress), indépendante de
la chaîne Owner/Project/Sandbox — un Owner la référence, pas l'inverse. Zéro
chevauchement.

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
| `sandbox.rs` | Job `sandbox-checkout` (`git worktree add`, création de branche depuis la default branch si absente) + Pod (home Owner + worktree en subPath + **second mount du même PVC sur `repo.git`, cf. ci-dessous** + un `volumes[].image` par toolchain + env agrégé PATH/LD_LIBRARY_PATH/caches — supprimé si `spec.suspended`, cf. ci-dessous) + Service ClusterIP (port MCP) + NetworkPolicy ingress (restreint aux pods du namespace portant `vanyline.solidite.fr/owner: <owner>`, **+ le pod `app` + le controller Ingress réel si l'Owner référence une Application, cf. sous-section dédiée**) + NetworkPolicy egress conditionnelle (WS-13, cf. ci-dessous) + **Ingress** (`sandbox-ingress-wiring`, seulement si l'Owner référence une Application — patché/supprimé explicitement selon l'état, même patron que la netpol egress) + finalizer (Job `git worktree remove`, la branche survit sur le remote). |
| `application.rs` (`controller-application-crd`) | Deployment (1 container, env assemblé depuis 3 `secretRef` OIDC/DB/cookie + `VNL_K8S_NAMESPACE` + `OIDC_REDIRECT_URL` calculé) + Service ClusterIP + Ingress + Secret cookie auto-généré si absent — cf. sous-section dédiée. Pas de finalizer, ownerReferences suffisent. |

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

### CRD Application (`controller-application-crd`)

Comble un écart doc/code : `AGENTS.md` listait "Application" comme une des CRDs du
controller depuis le début, mais elle n'existait pas avant cette feature.
`ApplicationSpec` référence trois `secretRef` (OIDC, base de données, cookie —
**jamais** de valeur en clair dans la CR) plutôt que de provisionner quoi que ce soit
elle-même (pas de Postgres géré par le controller — cohérent avec "on assemble sur
étagère", la base est provisionnée ailleurs, ex. vynil/kuberest). `app` sert déjà le
frontend buildé (`ServeDir`/`ServeFile`) : un seul Deployment/Service pour les deux,
pas de composant frontend séparé.

**`OIDC_REDIRECT_URL` calculé, jamais dupliqué dans le secret** (`https://{spec.host}/auth/callback`)
— une source unique de vérité, pas de désync silencieux si `host` change sans que le
secret suive.

**Secret cookie : auto-généré si `spec.cookie_secret_ref` est `None`** (décision
développeur, cohérente avec la philosophie Kydah — un cookie secret n'a aucune valeur
métier à faire relire par un humain, contrairement à l'OIDC client secret ou la chaîne
Postgres). Le reconciler cherche `<application-name>-cookie` ; absent → génère 64
octets aléatoires encodés en base64 standard et crée le Secret ; présent → **jamais
régénéré** (une régénération invaliderait toutes les sessions cookie actives — le
check "existe déjà ?" précède toute écriture). Point d'encodage à retenir si le sujet
revient : la valeur générée (déjà une chaîne base64) est enveloppée dans un
`ByteString` (`k8s-openapi`, qui base64-encode lui-même à la sérialisation JSON pour
le wire K8s) — deux couches d'encodage dans l'appel API, mais le pod, via
`secretKeyRef`, ne voit que la couche K8s décodée automatiquement : `COOKIE_SECRET`
reçu par le container est la chaîne base64 simple attendue par `app/src/main.rs`
(`base64::STANDARD.decode(...)`, ≥ 64 octets). Vérifié en lisant l'implémentation
`serde` de `ByteString`, pas supposé.

### Ingress par Sandbox (`sandbox-ingress-wiring`)

`Owner.spec.application_ref: Option<String>` (cascade, même esprit que
`egress`/`project_defaults` déjà en place) — `None` = Sandbox reste ClusterIP-only
(comportement historique, pas une erreur). Si renseigné : sous-domaine par sandbox
(`{sandbox}.sandboxes.{application.spec.host}`, décision développeur — pas de routage
par chemin sur l'host de l'app), `ingressClassName`/annotations repris de
l'Application. **TLS partagé, jamais un `Certificate` par sandbox** :
`Application.spec.sandbox_tls_secret_name` référence un secret wildcard
pré-provisionné — laisser cert-manager auto-provisionner un certificat par host
poserait un risque réel de rate-limit/churn avec des sandboxes créées/détruites en
continu.

**NetworkPolicy ingress étendue à deux peers de plus** (en plus du peer historique
"même Owner") : le pod `app` (label `vanyline.solidite.fr/application`, `Exists` —
pas de valeur exacte, pas de couplage au nom de l'Application — pour l'appel
serveur-à-serveur `POST /ws/ticket`, avant même que le navigateur ne se connecte), et
le controller Ingress réel du cluster (`Application.spec.ingress_controller`,
`namespace_selector` + `pod_selector` combinés sur le même peer pour cibler
précisément ses pods dans son propre namespace — valeurs du cluster de dev : namespace
`kydah-core`, labels `app.kubernetes.io/name: traefik` +
`app.kubernetes.io/component: controller`).

**Nettoyage symétrique à la netpol egress** (bug trouvé en revue, corrigé) :
l'Ingress n'était supprimé nulle part si `application_ref` était retiré (ou
l'Application visée supprimée) après avoir déjà été créé — resterait orphelin
indéfiniment. Même patron que la netpol egress (patch si nécessaire, delete explicite
sinon), pas un nouveau mécanisme.

**Dépendances d'infra externes, hors périmètre de ce repo** : DNS wildcard
`*.sandboxes.{host}` et certificat TLS wildcard correspondant doivent exister
(provisionnés ailleurs) — sans eux, l'Ingress créé ne sert à rien en pratique.

**Tests** : unitaires purs sur les builders (spec → Pod/Job/Service/NetworkPolicy
attendus, sans cluster) — pas de mock de l'API K8s. `--crds` (flag CLI) imprime les
manifests CRD générés par `schemars`, source de `deploy/controller/crds.yaml`
(régénéré via `deploy/controller/generate-crds.sh`).

**Déploiement** : `deploy/controller/` (RBAC ClusterRole/ClusterRoleBinding — le
controller watche les quatre CRDs sur tout le cluster via `Api::all`, donc pas de
Role/RoleBinding namespacé même si les CRDs elles-mêmes le sont — + Deployment,
étendu pour `applications`(`/status`)/`deployments`/`ingresses`/`secrets` avec
`controller-application-crd`, vérifié manquant avant l'ajout, pas supposé) et
`controller/Dockerfile` (cargo-chef, rustls-tls, pas de libssl). Image publiée :
`ghcr.io/sebt3/vanyline-controller:0.1.0`. Validé en e2e sur le cluster de
dev (Owner + Project + Sandbox de démo) — a débusqué un bug réel : les trois
reconcilers réutilisaient les mêmes `PatchParams` (avec `force()`, nécessaire aux
`Patch::Apply` de PVC/SA/Service/NetworkPolicy) pour le patch de status en
`Patch::Merge`, que kube-rs rejette hors contexte Apply — corrigé en isolant
`PatchParams::default()` pour le patch de status.

**Limites connues** (v1, assumées) : pas de quotas (champ réservé dans `OwnerSpec`, non
réconcilié), pas de webhook d'admission (validation par schéma CRD uniquement), pas de
merge/push automatique des branches (le controller gère la plomberie git, pas le
contenu), pas d'openvscode-server dans le pod. Changement de spec Sandbox = recréation
du pod (immutable en v1). Pas de résolution FQDN dans les règles egress (limite K8s :
`ipBlock`/selectors, pas de nom de domaine — si le besoin FQDN devient réel, chantier
CNI type CiliumNetworkPolicy, hors scope v1). Pas d'auto-arrêt sur inactivité
(`suspended` est manuel uniquement, décision 2026-07-12). Pas de provisioning
Postgres/DNS wildcard/certificat TLS wildcard par le controller (`controller-application-crd`/
`sandbox-ingress-wiring`) — références à des ressources provisionnées ailleurs, jamais
créées par ce repo.

## Client K8s CLI — `vanyline-crds`, `VnlK8sClient`, toolbox

Rend les Owners/Projects/Sandboxes pilotables **hors du cluster-admin** —
`kubectl`/accès direct au cluster n'est plus le seul moyen d'agir dessus.
Un vrai frontend existe désormais aussi (`app-k8s-provisioning`/
`settings-real-config` : `/api/projects`, `/api/sandboxes`, écrans CRUD dans
`SettingsView`) — API/CLI n'est plus le seul chemin, cf. section "Backend
web" plus haut.

**`vanyline-crds`** (lib feuille, voir "Vue d'ensemble") : extraction
mécanique des types CRD depuis `controller/src/crds.rs`, `kube` en
`default-features = false, features = ["derive"]` — jamais `client`/
`runtime`, sinon le CLI embarquerait la même machinerie réseau que
l'opérateur. `service_name`/`MCP_PORT` (nommage du Service MCP d'une
sandbox) y vivent aussi désormais — seule source de vérité pour
`controller` (qui pose le Service) ET `vanyline-lib` (qui doit résoudre
la même URL depuis le CLI).

**`VnlK8sClient`** (`lib/src/k8s.rs`, feature Cargo `k8s` de
`vanyline-lib`, désactivée par défaut — activée par le CLI **et par `app`**
depuis `app-k8s-provisioning`, jamais par défaut) :
- `discover(namespace_override: Option<String>)` — `kube::Config::infer()`
  (in-cluster ou kubeconfig), erreur `VNL-K8S-001` si injoignable.
  `namespace_override` prime sur le namespace du contexte kubeconfig
  courant si fourni.
- `list/get/create/delete_{owner,project,sandbox,application}` — CRUD générique par
  petites fonctions privées paramétrées sur `K: kube::Resource<...>`
  (évite la répétition x4 types x4 opérations), erreurs `VNL-K8S-002`.
- `sandbox_mcp_url(name)` — vérifie d'abord que la sandbox existe
  (`get_sandbox`, erreur claire plutôt qu'un échec de connexion confus
  plus tard) puis construit `http://<service_name(name)>.<ns>.svc:<MCP_PORT>/mcp`.
- `sandbox_ws_ticket_url(name)` (`sandbox-ingress-wiring`) — même patron que
  `sandbox_mcp_url`, cible `/ws/ticket` — utilisé par `app` pour le relais de
  ticket (cf. section "Backend web" plus haut), jamais par le CLI.
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

## Frontend — shell IDE Vue (`frontend/`)

Remplace intégralement l'ancien frontend Svelte 5 (`Login.svelte`/`Chat.svelte`,
routage hash-based) — plus aucune trace de Svelte dans `frontend/`. Vue 3 +
`vue-router`, coquille dockable [dockview-vue](https://dockview.dev) (thème
`dockview-theme-abyss`, réutilisé comme design system transversal via ses custom
properties `--dv-color-abyss-*`) hébergeant les panneaux Explorer/Editor/Terminal/
Workflow/Chat, plus une vue Configuration séparée.

**Stack par rôle** :

| Rôle | Choix |
|---|---|
| Éditeur | [CodeMirror 6](https://codemirror.net) |
| Terminal | [xterm.js](https://xtermjs.org) (`@xterm/xterm` + `@xterm/addon-fit`) |
| Arbre de fichiers | [Element Plus](https://element-plus.org) (`el-tree`, restylé via ses custom properties CSS) |
| Menu / vue Configuration / modales | [Reka UI](https://reka-ui.com) (`Menubar`, `Tabs`, `Dialog`) — portage Vue headless de Radix |
| Chat | [vue-advanced-chat](https://github.com/advanced-chat/vue-advanced-chat) — theming non résolu, cf. "Limites connues" |
| Routing | `vue-router` (`/`, `/p/:projectName`, `/p/:projectName/s/:sandboxName`, `/settings`) |

**Routing** (`router.ts`, `frontend-dashboards-nav`) : trois niveaux — `/` (accueil,
liste des projets), `/p/:projectName` (dashboard du projet, liste des sandboxes),
`/p/:projectName/s/:sandboxName` (`IdeShell.vue`, `props: true`) — plus `/settings`.
Remplace l'ancien `/ide/:sandboxName` (route supprimée, aucun redirect de compat, décision
assumée). `App.vue` bascule entre deux layouts selon la route (`route.name === 'ide'`) :
`MenuBar` + `StatusBar` uniquement sur l'IDE (inchangé) ; `AppBreadcrumb.vue` sur les
trois autres routes (fil d'ariane « Accueil / … »). Point d'entrée IDE : ligne cliquable
d'un tableau de sandboxes (`ProjectDashboard.vue`) → `router.push('/p/' + projectName +
'/s/' + name)`.

**Dashboards** (`HomeDashboard.vue`, `ProjectDashboard.vue`, sous
`components/dashboards/`) : absorbent la logique des anciens écrans Settings
`ProjectsScreen.vue`/`SandboxesScreen.vue` (supprimés). Chaque dashboard = tableau
(lignes cliquables → niveau suivant, actions par ligne type Supprimer avec
`@click.stop` pour ne pas déclencher la navigation) + bouton « Créer » ouvrant une
modale reka-ui `DialogRoot` + bouton « Paramètres » → `/settings`. Pas de bouton
« gérer » séparé — le tableau *est* la gestion. `ProjectDashboard.vue` scope la
création de sandbox au projet courant (pas de champ projet éditable dans sa modale).
Fil d'ariane : `AppBreadcrumb.vue` lit un état de sélection partagé
(`settings/navState.ts`, un `ref` exporté) pour afficher le groupe/sous-groupe actif de
`/settings` — évite le prop-drilling entre `SettingsView.vue` (enfant, via
`router-view`) et `AppBreadcrumb.vue` (dans `App.vue`, parent).

**Panneaux dockview et `provide`/`inject`** : `DockviewVue` monte Explorer/Editor/
Terminal via son propre registre de composants (`components: Record<string,
VueComponent>`), **pas** comme enfants déclarés dans le template d'`IdeShell.vue` — un
`emit`/listener parent-enfant classique n'a donc aucun effet. `IdeShell.vue` fournit
(`provide`) le client `/ws/fs` partagé, le nom de la sandbox, et un handler
`open-file` + un `Ref` `open-file-path` ; Explorer/Editor les `inject`ent. Pattern à
réutiliser pour tout futur état partagé entre panneaux dockview, pas un cas
particulier.

**`SandboxFsClient`** (`api/sandboxWs.ts`) : le protocole `/ws/fs` côté serveur n'a
aucun champ de corrélation (cf. section "Serveur MCP" plus haut) — ce wrapper
sérialise les requêtes sur la connexion partagée Explorer/Editor (une requête en vol à
la fois, la suivante attend la réponse de la précédente via une chaîne de promesses).
`openSandboxWs(sandboxName, path)` mine **un ticket par connexion** (`POST
/api/sandboxes/{name}/ws-ticket`, jamais partagé entre `/ws/fs` et `/ws/terminal` —
les tickets sont à usage unique côté serveur). Dégradation propre si le ticket/l'Ingress
sont indisponibles (dépendance d'infra externe, cf. "Opérateur Kubernetes" plus haut) :
`fsClient` reste `null`, les panneaux affichent un état vide plutôt que de planter.

**Editor** : `read` envoie systématiquement `raw: true` (cf. mode brut, section
"Serveur MCP" plus haut) — sans ça `Ctrl+S` réécrirait un contenu numéroté/tronqué.
Échec de `read`/`write` affiché à l'utilisateur (bandeau temporaire) plutôt qu'avalé
silencieusement — un save silencieusement échoué serait pire qu'une fonctionnalité
manquante.

**Terminal** : `ws.binaryType = 'arraybuffer'` obligatoire (défaut navigateur = `Blob`,
incompatible avec `new Uint8Array(event.data)`). La taille initiale du PTY doit être
envoyée sur l'event `'open'` du WebSocket, **pas** juste après la résolution de la
promesse d'ouverture — celle-ci se résout à la *construction* du WebSocket
(`CONNECTING`), pas à l'ouverture réelle ; un envoi prématuré est un no-op silencieux
(`readyState !== OPEN`), bug trouvé en revue (le mock de test avait `readyState` à
`OPEN` dès la construction, ce qui le masquait).

**SettingsView** (`settings-real-config`, réorganisé par `frontend-dashboards-nav`) :
Projets/Sandboxes en sont sortis (absorbés par les dashboards, cf. ci-dessus) ; les 7
écrans CRUD restants sont groupés en chemin de configuration explicite plutôt qu'un
tas plat :

```
Modèles  → Fournisseurs LLM, Profils de modèle
Outils   → Serveurs MCP, Toolsets
Agents
Skills
Compte   (lecture seule, pas de formulaire)
```

**Champs relationnels** : les six écrans à formulaire (tous sauf Compte) utilisent des
`<select>`/multi-select plutôt que du texte libre pour tout champ qui référence une
autre entité — la table ci-dessous est la carte de référence (vérifiée contre le code
backend, pas une supposition) :

| Champ | Référence réelle | Source |
|---|---|---|
| `ModelProfile.provider` | `LlmProvider` (résolu par nom côté serveur) | `GET /api/llm-providers` |
| `ModelProfile.model` | `LlmProvider.available_models` | select dépendant du provider choisi |
| `Agent.model` (libellé affiché : « Profil de modèle ») | `ModelProfile` (résolu via `resolve_model_profile_id` — **pas** un provider/modèle brut malgré le nom du champ API) | `GET /api/model-profiles` |
| `Agent.toolsets` | `Toolset[]` | `GET /api/toolsets`, multi-select |
| `Agent.skills` (branche liste) | `Skill[]` | `GET /api/skills`, multi-select |
| `Toolset.mcp[].server` | `McpServer` | `GET /api/mcp-servers` |
| `Toolset.mcp[].tools` | tools exposés par le serveur choisi | `available_tools` (après test, cf. section Backend) |
| `Toolset.local_tools` | registre `tools::mcp::*` | `GET /api/local-tools` |

Tous les états « liste vide car jamais testé » (provider/serveur MCP) sont gérés
explicitement, pas silencieusement.

**Modales** (reka-ui `DialogRoot`) : les six écrans ont converti leurs formulaires
création/édition (auparavant inline sous le tableau) en modales — même contrat partout
(`DialogRoot`/`DialogPortal`/`DialogContent`/`DialogTitle`/`DialogClose`). Piège reka-ui
partagé avec `MenuBar.vue` (`Menubar*`) : `DialogContent`/`DismissableLayer` ne
forwardent pas `class` de manière fiable jusqu'au DOM — la surface de la modale est
stylée globalement via le sélecteur `[role='dialog']`, pas via une classe scoped.
Régression trouvée et corrigée après la conversion : les erreurs de chargement des
listes de choix (`providersError`/`optionsError`) doivent vivre dans le corps principal
de l'écran, pas dans le `DialogContent` de création — sinon invisibles au chargement de
la page et absentes de la modale d'édition.

**Aucun gating admin** sur Agents/Serveurs MCP : le commentaire "write: admin" du code
backend (`app/src/api/mod.rs`) ne correspond à aucun mécanisme réel (pas de colonne
`role`, pas de contrôle serveur) — décision cohérente avec l'état mono-utilisateur du
projet, pas un oubli.

**Limites connues (dette assumée)** : pas de multi-onglets Editor, pas de
multi-terminal, pas de reconnexion WS automatique (déconnexion réseau/sandbox
suspendue → recharger la page), pas de filesystem watch/push (Explorer ne se
rafraîchit pas si le contenu change côté serveur pendant que l'utilisateur regarde),
pas de code-splitting (bundle ~576 Ko gzippé, CodeMirror + xterm + Element Plus +
vue-advanced-chat + dockview-vue). Chat reste un mock complet (aucun appel réseau) —
priorité très basse, pas planifié. Le theming `vue-advanced-chat` reste non résolu
(plusieurs tentatives sans effet sur une partie des couleurs, cause racine jamais
identifiée faute d'inspection DOM réelle) — sans conséquence tant que Chat reste mock.

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

Le modèle Qwen sous-jacent (`llm-exec`, context natif 262 144 tokens — vLLM était
plafonné à `--max-model-len 131072` jusqu'au 2026-08-02, corrigé depuis pour matcher le
natif vu la marge de KV cache réelle) peut échouer par compaction de contexte sur une
tâche déléguée qui touche beaucoup de fichiers volumineux, même bien spécifiée — la
session se compacte en cours de route et finit par poser une question au lieu d'agir
(indépendant de la permission `question: deny`, qui ne bloque que les appels d'outil, pas
le texte de fin de tour). Observé sur une tâche couvrant `sandbox` et `controller`
combinés (~6000 lignes de fichiers source à lire) : deux échecs malgré une spécification
déjà complète. Scinder par crate a réduit le risque. Quand le contrat d'une tâche est déjà
entièrement écrit et le risque de récidive élevé, appliquer directement les modifications
plutôt que de multiplier les tentatives de délégation est plus efficace — ce n'est pas un
problème de spécification qu'une réécriture peut résoudre, c'est une limite matérielle de
l'outil.

**Point ouvert** : plusieurs compactions documentées (ws12/ws13/ws15) sont survenues sur
des tâches petites (5 fichiers, diffs courts) — bien en dessous même de l'ancien plafond
131072. Le cap vLLM explique un plafond bas, pas ces échecs-là précisément ; cause encore
non identifiée, à creuser si ça se reproduit après le passage à 262144.

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
- **Discovery de tools MCP limitée au transport `http-streamable`** :
  `POST /api/mcp-servers/{id}/test` (`frontend-dashboards-nav`) réutilise
  `connect_domain_mcp_server_inner`, dont le `match` sur `McpTransport` n'a qu'un seul
  variant (`HttpStreamable`) — un serveur `mcp_servers.server_type = "sse"` n'a pas
  d'implémentation de transport, la découverte échoue pour ce type. Limite
  pré-existante (pas introduite par cette feature), à lever le jour où `McpTransport`
  gagne un variant `Sse`.
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
| `frontend/` | app | Shell IDE web : éditeur/explorer/terminal + configuration, cliente de l'app Rust (REST + WS) — voir section "Frontend — shell IDE Vue" plus haut | Vite, Vue 3, `vue-router`, dockview-vue, CodeMirror 6, xterm.js, Element Plus, Reka UI, Vitest |
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
