# Architecture — crates et packages du monorepo

Ce document décrit le découpage du monorepo — crates Rust et packages TypeScript — et
les **règles de dépendances** entre eux. Pour la vue système d'ensemble (composants,
interfaces réseau, auth), voir `AGENTS.md`.

## Vue d'ensemble

Huit crates : **quatre bibliothèques feuilles** partagées (`vanyline-tools`, `vanyline-cfgstore`,
`vanyline-lib`, `vanyline-crds`) et **quatre binaires** qui les consomment (`vanyline`, `vanyline-app`,
`vanyline-sandbox`, `vanyline-controller`).

| Crate | Type | Rôle | Contenu clé |
|-------|------|------|-------------|
| `vanyline-tools` | lib feuille | Implémentations d'outils, pures et framework-agnostic, SLM-friendly (v2) | `filesystem` (read/write/edit/delete/list), `search` (find_files/search), `command` (`execute` via `sh -c`, timeout, cwd), `error` (`ToolsError`, codes `VNL-TLS-*`), `output` (bornage centralisé), `mcp` (schémas JSON — source unique consommée par `cli` et `sandbox`) |
| `vanyline-cfgstore` | lib feuille | Couche de configuration : types de domaine + trait `ConfigStore` (lecture **et** écriture) + impl fs deux-couches YAML. Zéro dépendance harness (serde/serde_json/yaml_serde/async-trait/thiserror) — consommable par un serveur léger (sandbox) sans tirer rig/rmcp | `domain` (types name-keyed : `Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`…), `store` (`ConfigStore` — `list_*`/`get_*` + `create_*`/`update_*`/`delete_*` par domaine + `set_default_agent` ; défaut `ReadOnly` sur les écritures ; `InMemoryConfigStore`), `layers` (`Layers`, `RawConfigFile`, fusion des deux couches), `fs_store` (`FsConfigStore` + `validate_name` anti-traversal), `error` (`CfgStoreError`, codes `VNL-CFG-001..010`) |
| `vanyline-crds` | lib feuille | Types CRD Owner/Project/Sandbox/Application (spec/status/derives kube), sans runtime opérateur | `Owner`/`Project`/`Sandbox`/`Application` + specs/status, `Toolchain`/`PvcRef`/`ProjectDefaults`/`IngressControllerRef`, `crd_manifests()`, `service_name`/`MCP_PORT` (convention de nommage du Service MCP d'une sandbox, partagée avec `vanyline-lib`) — voir sections "Client K8s CLI" et "Opérateur Kubernetes" plus bas |
| `vanyline-lib` | lib | Cœur partagé LLM / MCP / chat — harness (agents, toolsets, skills, subagents) | dépend de `vanyline-cfgstore` et **re-exporte** `domain` + `store` (`vanyline_lib::domain::…` / `::store::ConfigStore` restent les chemins canoniques) ; `impl From<CfgStoreError> for VnyError` ; `event` (`ChatEvent`/`EventSink`), `model` (construction de modèle + params), `session` (`SessionContext`, `run_agent_turn` — point d'entrée unique), `builtin` (tools `skill`/`task`), `prefixed_mcp` (connexion MCP filtrée par toolset), `types` (`ToolCall`/`Message`/`Conversation` — formats de persistance propres à chaque binaire), `k8s` (`VnlK8sClient`, **feature Cargo optionnelle `k8s`**, désactivée par défaut — voir "Client K8s CLI" plus bas), erreurs `VNL-*` |
| `vanyline` (bin `cli`) | binaire | CLI standalone de chat/agents | `run`/REPL, `vanyline_cfgstore::FsConfigStore` (YAML deux couches, globale + workspace — voir "Configuration CLI" plus bas), câblage hôte de la découverte de couches (`cli/src/config.rs::discover_layers` : `dirs`, remontée cwd), enveloppe les `vanyline-tools` en `ToolDyn` locaux, active la feature `k8s` de `vanyline-lib` (commandes owner/project/sandbox + toolbox) |
| `vanyline-app` | binaire | Backend du frontend | axum (REST + WS), auth/RBAC/CRUD générique via `miryad-core` (crate publique), SeaORM/PostgreSQL, `PgConfigStore` (adapte le schéma SeaORM en `ConfigStore`), orchestration LLM via `vanyline-lib`, client `VnlK8sClient` (feature `k8s`) pour piloter Owner/Project/Sandbox/Application et relayer les tickets WS de la sandbox — voir section "Backend web" plus bas |
| `vanyline-sandbox` | binaire | Pod serveur MCP + éditeur | expose les 8 `vanyline-tools` via MCP (`tools_impl.rs`/`mcp.rs`, schémas partagés avec `cli`) + WebSocket éditeur (`/ws/ticket`, `/ws/fs`, `/ws/terminal`) — voir section "Serveur MCP" plus bas ; second binaire `vanyline-maint` (maintenance des workspaces par les Jobs du controller — voir section dédiée) |
| `vanyline-controller` | binaire | Opérateur Kubernetes | kube-rs, reconcilers des CRDs Owner/Project/Sandbox/Application v1alpha1 (types importés de `vanyline-crds`) — voir section dédiée plus bas |

## Graphe de dépendances

```
vanyline-tools    ◄──  vanyline (cli),  vanyline-sandbox
vanyline-cfgstore ◄──  vanyline-lib,  vanyline (cli),  vanyline-app   (sandbox : prévu, cf. F2)
vanyline-lib      ◄──  vanyline (cli),  vanyline-app
vanyline-crds     ◄──  vanyline-controller,  vanyline-lib (feature k8s, via vanyline (cli)
                    et vanyline-app)

vanyline-tools    : aucune dépendance interne, pas de rig/rmcp
vanyline-cfgstore : aucune dépendance interne, aucun harness (serde/serde_json/yaml_serde/
                    async-trait/thiserror uniquement). C'est le crate feuille que la sandbox
                    pourra consommer pour éditer la config d'un workspace sans tirer rig/rmcp.
vanyline-lib      : dépend de vanyline-cfgstore (re-export domain + store) ; rig-core + rmcp
                    externes ; feature optionnelle k8s -> vanyline-crds + kube
                    (default-features = false, "client" seulement — jamais "runtime", ni le
                    CLI ni l'app ne doivent embarquer le reconciler). Activée par `cli`
                    (commandes owner/project/sandbox + toolbox) et par `app` (routes REST
                    /api/projects, /api/sandboxes — app-k8s-provisioning) ; jamais par défaut.
vanyline-crds     : aucune dépendance interne, kube en "derive" seul (pas de "client"/"runtime")
vanyline-controller : dépend uniquement de vanyline-crds (types CRD) parmi les crates du
                    workspace — aucune autre dépendance interne
```

Aucun cycle. `vanyline-tools`, `vanyline-cfgstore` et `vanyline-crds` sont des feuilles
indépendantes l'une de l'autre ; `vanyline-lib` n'est plus une feuille (elle dépend de
`vanyline-cfgstore`).

## Règles de dépendances

1. **`vanyline-tools`, `vanyline-cfgstore` et `vanyline-crds` sont des feuilles.** Elles ne
   dépendent d'aucun autre crate du workspace. `vanyline-lib` dépend uniquement de
   `vanyline-cfgstore` (dont elle re-exporte `domain` + `store`). Toute la logique réutilisable
   vit dans ces libs ; les binaires ne font que composer. `vanyline-cfgstore` en particulier est
   tenu sans harness (pas de rig/rmcp/tokio-full) pour rester consommable par la sandbox.

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
- `store: Arc<dyn ConfigStore>` — `ConfigStore` (trait de `vanyline-cfgstore`, re-exporté
  par `vanyline_lib::store`) : résolution de config par nom (providers, modèles, toolsets,
  agents, skills). Chaque binaire fournit sa **propre implémentation** : `FsConfigStore`
  (`vanyline-cfgstore`, YAML deux couches natif — voir "Configuration CLI" plus bas ;
  remplace l'ancien `CliConfigStore`/JSON, supprimé) et `PgConfigStore` (app, requête
  les entités SeaORM de `miryad-core-integration` — voir "Backend web" plus bas). Le
  session engine n'utilise que le versant lecture ; les méthodes d'écriture du trait
  (`create_*`/`update_*`/`delete_*`) ne servent qu'au RPC — voir "RPC stdio" plus bas.
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

**Modules** : toute la mécanique de couches (`Layers`, `RawConfigFile`,
fusion, la "source" d'une entrée — global vs workspace) et `FsConfigStore`
lui-même vivent dans **`vanyline-cfgstore`** (`layers.rs` / `fs_store.rs`) —
`FsConfigStore` implémente `ConfigStore` (lecture + écriture) et reste le
store actif de toutes les commandes CLI et du RPC. `cli/src/config.rs` ne
garde que le **câblage hôte** : `config_dir`/`data_dir` (`dirs`),
`discover_workspace_root` (remontée cwd), `discover_layers` (assemble un
`Layers` à partir de ces morceaux). Dépendance `yaml_serde` (fork maintenu
de `serde_yaml`, devenu archivé — API identique :
`from_str`/`to_string`/`Value`/`Error`).

**Écriture** : `create_*`/`update_*`/`delete_*` par domaine + `set_default_agent`,
cible de couche explicite (`Layer::Global` / `Workspace`). `validate_name`
(`^[a-zA-Z0-9][a-zA-Z0-9._-]*$`, ≤ 64, rejet `..`/`/`/`\`/absolu — contrat
anti-traversal, `VNL-CFG-005`) est appliquée **avant toute opération disque**
dans chaque chemin d'écriture. Les `update` de `config.yaml`
(providers/models/mcp) revalident l'entrée patchée via son type `Raw*Entry`
(celui que la lecture consomme) avant d'écrire : un patch qui rendrait le
domaine illisible (`null` sur requis, valeur mal typée) est refusé en entier
(`VNL-CFG-010`), rien n'est écrit. Écritures non atomiques (`std::fs::write`
direct) — dette assumée, à revoir quand la sandbox consommera le crate.

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
namespace d'erreur `VNL-RPC-000` à `VNL-RPC-015`), `handlers.rs`
(dispatch, `ServerState`, logique par méthode). Réutilise
`vanyline_cfgstore::FsConfigStore` (config) et `cli/src/store.rs`
(conversations, format JSON existant) tels quels — aucun nouveau stockage
introduit.

**Config — lecture et écriture** (`config/*`, F2) : `config/<domain>` liste
les 6 domaines (`providers`, `models`, `mcpServers`, `toolsets`, `agents`,
`skills`) ; `config/<domain>/{create,update,delete}` les édite via les
méthodes d'écriture de `ConfigStore`, avec `layer?` optionnel (`"global"` /
`"workspace"` — défaut : workspace si résolu à `initialize`, sinon global).
`CfgStoreError` est traduit en `VNL-RPC-011..015` (write error / not found /
name conflict / invalid name / validation) ; le reste retombe sur
`VNL-RPC-006`. Actions : `config/providers/test` et `config/mcpServers/test`
sondent la cible réseau **stockée dans la config** (SSRF assumée — serveur
local, config de l'utilisateur), chacune **avec un timeout de 10 s** (le
dispatch RPC est série : un sondage qui pend gèlerait tout le serveur) ;
`config/localTools` renvoie le registre statique des 8 tools intégrés.

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

Backend axum du frontend. Depuis `miryad-core-integration` (2026-08-27), la couche
auth/session/BDD/CRUD générique n'est plus maison — `app` consomme la crate publique
`miryad-core` (crates.io, moteur générique auth/RBAC/REST/GraphQL/MCP derrière le
template `miryad`, cf. `$HOME/projets/miryad-core`). Bascule complète, en une fois,
sans chemin de migration ni cohabitation ancien/nouveau (aucune donnée réelle à
préserver — pas d'instance vanyline en prod à cette date).

**Auth** : `miryad_core::auth::{auth_router, MiryadAuthState, AuthUser}` — session
cookie OIDC pour le navigateur (dual-auth cookie/token API géré par le crate, `app` ne
consomme que le flow cookie). `AuthUser { subject, email, id_token }` remplace
l'ancien extracteur maison ; `AuthUser.id_token` (le JWT OIDC brut, jamais exposé au
JS) sert toujours au relais de ticket WS vers la sandbox (cf. section "WebSocket
éditeur" plus bas) et au relais `/git/*`. `app/src/auth/{cookie,middleware,oidc}.rs`
(implémentation maison précédente) supprimés.

**Stockage** : PostgreSQL/SeaORM (`sea-orm`/`sea-orm-migration`, plus sqlx direct) —
entités `app/src/db/entities/*.rs`, tables préfixées `vanyline_*`, migrations
`app/src/migration/m20260825_*.rs` (table de suivi dédiée `seaql_migrations_app`, pour
ne pas collisionner avec celle de miryad-core). **Clés primaires `i32`
auto-incrémentées** (pas `Uuid`) — imposé par `miryad_core::rest::RestEntity`, qui
suppose une seule colonne de PK de ce type. Le schéma `User` de miryad-core
(`miryad_users` : id, subject, email, display_name) est **fixe**, sans mécanisme
d'extension par l'app consommatrice — le lien vers l'Owner CRD K8s (ancien
`users.k8s_owner_name`) vit donc dans une table séparée côté `app`,
`vanyline_owner_links` (FK vers `miryad_users.id`, upsert atomique `ON CONFLICT` dans
`api/owners.rs::ensure_owner`).

`config_store.rs::PgConfigStore` implémente toujours `vanyline_lib::store::ConfigStore`
(trait re-exporté de `vanyline-cfgstore` depuis F2 ; renvoie donc `CfgStoreError`,
`app/src/error.rs` a le `From` vers `AppError`), mais requête désormais les entités
SeaORM plutôt que du sqlx brut — même contrat, même résolution par nom (le nom remplace
l'id en sortie de `ConfigStore`, comme `FsConfigStore` côté cli). `load_skill` lit
`skills.body` à la demande, même paresse qu'avant. Les méthodes d'écriture du trait ne
sont **pas** implémentées ici (défaut `ReadOnly`) — les writes de l'app passent par les
handlers REST / `miryad-core`, jamais par `ConfigStore`.

**RBAC générique** (`miryad_core::resource::{MiryadResource, AccessPolicy}`) : chaque
entité déclare `read_policy()`/`write_policy()` (`Public` / `OwnerOnly` / `Group` /
`AdminOnly`) et `owner_column()`. `ModelProfile`/`Toolset`/`Skill`/`AgentRecord` sont
`OwnerOnly` (propriétaire = utilisateur courant, injecté serveur-side, jamais
acceptée du client). `LlmProvider`/`McpServer` sont devenus des **ressources globales
partagées** (`read_policy = Public`, `write_policy = AdminOnly`, `owner_column =
None`) — rupture avec le MVP initial où chaque utilisateur avait ses propres
providers/serveurs MCP ; tout utilisateur authentifié voit la liste pour choisir un
modèle/toolset, seul un admin (groupe OIDC `admin`) les configure. Validation
métier perdue avec les anciennes contraintes `CHECK` (types énumérés `provider_type`/
`server_type`/`mode`) restaurée via `MiryadResource::before_create` (hook pur, **sans
accès BDD** — limite du framework : une validation nécessitant une requête, comme
l'existence d'un nom référencé, ne peut pas y vivre).

**API REST** — deux familles de routes, préfixes distincts par construction :
- **CRUD générique** (`miryad_core::rest::resource_router::<Entity, AppState>()`,
  monté dans `main.rs`) pour `llm-providers`, `mcp-servers`, `model-profiles`,
  `toolsets`, `skills`, `agents` — `GET/POST /api/v1/{resource}` (paginé,
  `PagedResult<T>`) et `GET/PUT/DELETE /api/v1/{resource}/{id}`. Aucune route écrite à
  la main pour ces six entités.
- **Handlers custom** (`api/*.rs`, `api::api_router` sous `/api`, `api::api_v1_router`
  sous `/api/v1`) pour tout ce qui dépasse le CRUD générique : `owners`/`projects`/
  `sandboxes` (CRDs K8s, pas des lignes Postgres — hors du moule `MiryadResource`,
  qui suppose une entité SeaORM), `conversations`/`messages` (effet de bord à la
  création, résolution de nom côté serveur, sous-route `/messages`, filtre au-delà
  d'une égalité simple — cf. plus bas), `/me`, `/local-tools`, et les actions
  non-CRUD des deux ressources globales (`POST /api/v1/llm-providers/{id}/test`,
  `PUT /api/v1/llm-providers/{id}/default`, `POST /api/v1/mcp-servers/{id}/test` —
  montées sous `/api/v1`, au même préfixe que le CRUD généré de ces mêmes ressources,
  RBAC vérifié via les helpers publics `miryad_core::rbac::can_read`/`can_write`, pas
  réinventé). Toutes ces routes exigent `AuthUser` et scopent par utilisateur (sauf
  Owner/Project/Sandbox/Conversation/Message qui vérifient l'appartenance à la main,
  cf. plus bas).

`ChatContext`/`Conversation`/`Message` restent des entités SeaORM mais **volontairement
hors `MiryadResource`** : `create_conversation` crée un `ChatContext` avant la
`Conversation` (effet de bord), résout `agent_name` en id côté serveur,
`list_conversations` filtre par jointure au-delà d'un `filter_column()` à une seule
colonne — le moule générique ne les couvre pas. `Message`/`Conversation` portent un
`owner_id` posé directement à la création (pas dérivé d'une FK) : `before_create` n'a
de toute façon pas accès BDD pour vérifier qu'un `conversation_id` fourni appartient au
même propriétaire — limite acceptée sciemment (détail de la décision et des bugs trouvés en
revue Phase 3 : `.claude/memory/miryad-core-integration.md`).

**Discovery — providers et serveurs MCP** (`frontend-dashboards-nav`) : même patron sur
les deux entités qui référencent une liste de choix, jamais peuplée à la création,
seulement après un test explicite. `POST /api/v1/llm-providers/{id}/test` interroge le
provider et persiste les noms de modèles dans `vanyline_llm_providers.available_models`.
`POST /api/v1/mcp-servers/{id}/test` fait de même pour les tools exposés par un serveur
MCP : réutilise `connect_domain_mcp_server_inner`/`list_all_tools()` de
`lib/src/prefixed_mcp.rs` (déjà écrit pour la connexion runtime des toolsets, pas un
nouveau client), persiste dans `vanyline_mcp_servers.available_tools`. Les deux listes
alimentent des dropdowns dépendants côté
frontend (cf. section Frontend plus bas) — état vide explicite tant que le test n'a
jamais été lancé. `GET /api/local-tools` (nouveau, lecture seule, pas de DB) expose le
registre statique `tools::mcp::{filesystem_tools, search_tools, command_tools}` — les 8
tools intégrés de `vanyline-tools` — pour le même usage de dropdown côté `Toolset.local_tools`.

**WebSocket chat** (`ws/chat.rs`) : `run_agent_turn` avec `local_tools` vide (l'app
reste sur le chemin froid — cf. règle de dépendances plus haut). `extra_mcp` est en
revanche résolu dynamiquement depuis le contexte de la conversation
(`chat_contexts`, `chat-app-fonctionnel`) — `kind = "sandbox"` résout l'URL MCP de la
sandbox nommée (`VnlK8sClient::sandbox_mcp_url`, même scoping owner que
`api::sandboxes::get_sandbox`), les autres `kind` n'ont pas encore de toolset associé
(liste vide, pas une panne). `ChannelSink` pousse
chaque `ChatEvent` sur un canal mpsc dès son émission ; une tâche `forward_events` par
connexion (pas par tour) draine le canal et écrit sur le socket au fil de l'eau — vrai
streaming token-par-token, contrairement à l'ancien `CollectingSink` qui bufferisait
tout un tour avant le premier octet (limite documentée pendant la migration
harness-core, résolue par la tâche `ws-chatevent`). **Persistance todo**
(`chat-todo-live`) : `SessionContext.todo_state` (builtin `todowrite`/`todoread` de
`vanyline-lib`, inconditionnels sur tout `run_agent_turn`) est semé depuis
`conversations.todo` au début de `handle_message`, relu et persisté après le tour
seulement s'il a changé (jamais d'update systématique qui écraserait un état
antérieur à `NULL`) — même patron que le fix `f4dfbf9` déjà en place côté CLI (colonne
`conversations.todo`, portée par l'entité SeaORM depuis `miryad-core-integration`).

**WebSocket sandbox-state** (`ws/sandbox_state.rs`, `sandbox-state-ws`) : `GET
/api/ws/sandbox-state` (same-origin, cookie OIDC — pas de ticket) pousse au
navigateur les changements de `status.phase` des sandboxes de l'utilisateur, en
temps réel. Un **seul** watch kube-runtime sur les CRD `Sandbox` du namespace
(`VnlK8sClient::watch_sandboxes`, timeout serveur 5 min), partagé par toutes les
connexions via `AppState.shared_sandbox_state` (`SharedState` : `parking_lot`,
liste de subscribers, cache `project` → `owner`). La tâche `watch_loop` est
lancée à la première connexion (double-checked locking sur `watch_handle` —
course entre connexions concurrentes), tourne ensuite pour la vie du process, et
se met en pause tant qu'aucun subscriber n'est connecté. Chaque événement de
watch est dispatché aux seuls subscribers dont l'Owner correspond : le namespace
étant multi-tenant, on résout `Sandbox.spec.project` → `Project.spec.owner`
(cache avec mémorisation des miss pour ne pas rappeler l'API ni re-logger sur une
sandbox orpheline). Le payload est minimal (`{ sandbox, phase }`, `phase: null`
en suppression) ; côté frontend le hub singleton `useSandboxState` s'en sert
surtout comme **signal** — il débounce un refetch du listing CRUD (300 ms) plutôt
que de reconstruire l'état depuis le seul `phase`. RBAC : le `Role` de `app`
généré par le controller porte le verbe `watch` sur `sandboxes` (sans quoi le
watcher boucle sur des 403).

**Résolution Owner** (`api/owners.rs`, `app-k8s-provisioning`, table dédiée depuis
`miryad-core-integration`) : `vanyline_owner_links.k8s_owner_name` (FK vers
`miryad_users.id`, colonne nullable) fait le lien entre un utilisateur OIDC et un Owner
K8s — aucun mécanisme automatique avant cette feature (l'Owner se créait jusque-là
uniquement à la main via le CLI). `resolve_owner_name` (lecture, ne crée rien — routes
GET répondent « aucun Owner » si absent) et `ensure_owner` (crée l'Owner si nécessaire
et persiste le nom résolu via un upsert atomique `INSERT ... ON CONFLICT (user_id) DO
UPDATE`, **réservé au seul `POST /api/projects`** — décision développeur : lazy
provisioning restreint, pas de création implicite sur une route de lecture).
`sanitize_owner_name` dérive un nom de ressource K8s valide (RFC1123 : début **et
fin** alphanumériques) depuis l'email ou le `subject` OIDC — la troncature à 63
caractères retrim explicitement les tirets de fin (un bug corrigé en revue : une coupe
pile sur un `-` produisait un nom invalide).

**Routes `projects`/`sandboxes`** (`api/projects.rs`, `api/sandboxes.rs`,
`app-k8s-provisioning` + `sandbox-ingress-wiring`) : wrappers fins autour de
`VnlK8sClient` (feature `k8s` de `vanyline-lib`, activée côté `app` — absente par
défaut). Scoping IDOR systématique : chaque `get`/`delete`/`update` vérifie que le
Project référencé (directement, ou via la Sandbox pour les routes sandbox)
appartient bien à l'Owner de l'utilisateur authentifié, pas seulement au moment de la
création. Même scoping repris dans `ws/chat.rs::resolve_extra_mcp` pour la sandbox
nommée dans le contexte d'une conversation (`chat-app-fonctionnel`) — absent de la
version initiale, trouvé en revue Phase 3 (cf. section Chat, plus bas). `POST /api/sandboxes/{name}/ws-ticket` : relais de ticket WS (cf. section
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

### Endpoints git — statut, diff, staging, commit, branches, merge, push (WS-11 + `git-integration`)

`sandbox/src/git.rs`, même middleware d'authentification que `/mcp`. Servent l'app/le
frontend via un relais REST (voir "Relais `app` → `/git/*`" ci-dessous) — pas de tool
MCP git (les LLM ont déjà `execute_command` + git dans l'image). Historiquement
(WS-11, jusqu'en 2026-08) limité à deux endpoints en lecture seule, explicitement
« pas d'action git, pas de diff de contenu » — décision revue en session
(`git-integration`, 2026-08-22) : c'était un arbitrage de scope pour livrer vite, pas
une contrainte permanente. Rouvert jusqu'à diff/commit/branches/merge/push inclus ;
restent hors périmètre : rebase interactif, cherry-pick, stash, résolution de conflit
3-way (résolution = éditer les marqueurs `<<<<<<<` dans l'éditeur normal + geste
« marquer résolu » = stage), staging par hunk, force-push, graphe multi-branches
complet (`git log --graph --all` — le graphe livré est linéaire, branche courante +
refs, cf. section Frontend).

```
GET  /git/status                          → { branch, files: [...], clean }
GET  /git/unpushed                        → { branch, upstream, commits: [...], truncated }
GET  /git/diff?path=<rel>[&staged=bool]   → { path, diff }             # patch unifié texte
POST /git/stage    { paths: [...] }       → { ok }
POST /git/unstage  { paths: [...] }       → { ok }
POST /git/commit   { message }            → { sha, title }             # sur le contenu déjà staged
POST /git/push                            → { ok, pushed }
GET  /git/branches                        → { current, merging, branches: [...] }
POST /git/branches { name, from? }        → { ok }
POST /git/checkout { branch }             → { ok } | refus si working tree sale
DELETE /git/branches/{name}               → { ok }
POST /git/merge    { branch }             → 200 { conflicted, sha }    # conflit = résultat normal, pas une erreur
POST /git/merge/abort                     → { ok }
GET  /git/log?limit=<n>[&all=bool]        → { branch, commits: [{ sha, parents, refs, ... }], truncated }
GET  /git/ssh-key                         → { exists, public_key }
POST /git/ssh-key                         → { public_key }             # idempotent, ne régénère jamais
```

- **`GET /git/status`** : parse pur de `git status --porcelain=v2 --branch` (exécuté
  dans `VNL_SANDBOX_ROOT`). `state` ∈ `modified | added | deleted | renamed |
  untracked | conflicted` — mapping depuis les colonnes X (staged)/Y (unstaged)
  porcelain v2, X prioritaire si non `.` ; `typechange` traité comme `modified`,
  `copied` comme `renamed`. HEAD détachée : `branch == "(detached)"` (littéral git) —
  pas une erreur pour cet endpoint, contrairement à `/git/unpushed`.
- **`GET /git/unpushed`** : compare à `origin/<branch>` si la ref existe, sinon à
  `origin/<default>` (résolu dynamiquement via le HEAD symbolique du dépôt bare, repli
  `"main"`). Bornée à 200 commits. HEAD détachée → erreur `VNL-SBX-006`. Coexiste avec
  `/git/log` (historique complet, pas seulement le delta local/upstream) sans
  dupliquer la logique de parsing — usages différents.
- **`POST /git/checkout`** : refus strict si le working tree est sale (`git diff
  --quiet` sur le worktree ET l'index) — pas de stash automatique. Le check propage
  une vraie erreur (`VNL-SBX-004`) si `git` ne peut pas être lancé ou renvoie un code
  inattendu, plutôt que de retomber silencieusement sur « propre » (bug trouvé et
  corrigé en review Phase 3, 2026-08-22 — la première version avalait ces échecs).
- **`POST /git/merge`** : conflit = `200 { conflicted: true, sha: None }`, pas une
  erreur HTTP — l'état qui en résulte est bien défini (`MERGE_HEAD` présent, marqueurs
  dans les fichiers), déjà exposé par `status.state: conflicted` et
  `branches.merging`. Erreur HTTP seulement si le merge n'a pas pu être *lancé*
  (branche introuvable, merge déjà en cours, working tree sale).
- **Validation des refs/noms** (`merge`, `checkout`, `branches` create/delete) : toute
  valeur commençant par `-` est rejetée (`VNL-SBX-014`) avant de devenir un argument
  positionnel de commande git — sans ce garde-fou, une valeur comme `--abort` passée à
  `branch` sur `/git/merge` est interprétée comme un flag (`git merge --abort` annule
  un merge en cours au lieu d'en démarrer un) plutôt qu'une ref. Bug trouvé en review
  Phase 3 (2026-08-22) ; le fix est une validation en amont, pas un séparateur `--`
  partout — `git checkout -- <x>` a une sémantique différente (pathspec, pas branche).
- **`GET`/`POST /git/ssh-key`** : provisionne une clé SSH ed25519 **dans le PVC Owner**
  (`/home/vanyline/.ssh/id_ed25519`), pas dans un Secret K8s dédié — le PVC Owner
  existe déjà pour ce cas d'usage (déjà monté au même chemin dans le pod Sandbox ET le
  Job d'init/fetch du Project, cf. section "Opérateur Kubernetes" ci-dessous),
  généraliser les Secrets dédiés par type de credential (SSH, GPG, kubeconfig...)
  multiplierait les objets à gérer sans raison. `POST` est idempotent : ne régénère
  **jamais** une clé existante (casserait les deploy keys déjà enregistrées côté host
  git), retourne juste la clé publique existante. `Project.spec.git_secret` (ancien
  mécanisme à Secret K8s référencé) est déprécié — le controller ne le consomme plus
  (cf. section "Opérateur Kubernetes"), le champ CRD reste pour compatibilité, pas
  supprimé.
- **Fraîcheur** : les refs `origin/*` ont la fraîcheur du dernier `fetch` périodique du
  Project (cron), pas du remote instantané — aucun fetch déclenché par ces endpoints.
- **Dépend du mount `repo.git`** (cf. section "Opérateur Kubernetes" ci-dessous) et de
  la refspec de fetch (cf. section "Maintenance des workspaces" ci-dessous).

### Relais `app` → `/git/*` (canal d'auth frontend → sandbox)

`/git/*` est sous `require_auth` (JWT OIDC) côté sandbox ; le navigateur ne détient
jamais de JWT brut (principe déjà acté plus haut). Le frontend n'appelle donc jamais
la sandbox directement pour ces endpoints — `app` relaie (`ANY
/api/sandboxes/{name}/git/{*path}`, `app/src/api/sandboxes.rs::git_proxy`), même
pattern que le relais de ticket WS (JWT présenté à la sandbox, jamais renvoyé au
navigateur), même scoping owner (`resolve_user` → `resolve_owner_name` →
`get_sandbox` → `get_project` → assert owner match, dupliqué dans les deux handlers).

- **Sous-chemin extrait positionnellement depuis l'URI brute de la requête**
  (`raw_git_tail`), pas via l'extractor `Path` d'axum qui décode le wildcard
  `{*path}` avant que le handler ne le voie — vérifié empiriquement : axum décode
  `%2F` en `/` et ne le ré-encode jamais. Deux bugs trouvés en review Phase 3
  (2026-08-22) partageaient cette cause : (1) traversal de chemin — `/git/../mcp`
  atteignait un endpoint hors périmètre après normalisation d'URL côté client HTTP ;
  (2) toute branche contenant un `/` dans son nom (`feature/x`, convention courante)
  cassait systématiquement (`%2F` redevenu `/` littéral avant d'atteindre la route
  sandbox à un seul segment). Fix : extraction positionnelle sur les octets bruts +
  rejet explicite des segments décodés `.`/`..`/vides, plutôt qu'un décodage suivi
  d'une reconstruction (qui perd l'information nécessaire pour distinguer les deux
  cas).
- **Passthrough réel** : le proxy retourne le statut/JSON de la sandbox tels quels —
  y compris un body non-JSON (404/405 texte brut d'axum pour une route non matchée),
  enveloppé plutôt que transformé en 502 générique (bug trouvé en review, même
  session).

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
des tool calls LLM) — `{"op":"read|write|edit|delete|list|mkdir|rename|root","path":...}` →
`{"ok":true|false,...}`, dispatch vers les mêmes fonctions `vanyline_tools::filesystem::*`
que le chemin MCP, même confinement (`tools_impl::confine_path`). **Aucun champ de
corrélation** : strictement une requête lue, une réponse écrite, dans cet ordre — un
client qui partage la connexion entre plusieurs consommateurs (cf. `SandboxFsClient`
côté frontend) doit sérialiser ses appels, pas un détail d'implémentation optionnel.

`mkdir`/`rename`/`root` (`editing-context-menus`, CRUD arbre côté frontend) :
`mkdir` crée aussi les dossiers parents manquants (`create_dir_all`, cohérent avec
`write_file`) et est idempotent sur un dossier déjà existant. `rename` confine
**les deux** chemins (`path` source et `to` destination) via `confine_path` — même
contrainte de sécurité que les autres ops, pas de mécanisme distinct pour la
destination. `delete` sur un dossier utilise `remove_dir` (non récursif) : erreur
explicite `directory is not empty` plutôt qu'une suppression en cascade silencieuse.
`root` (sans `path`) renvoie la racine absolue confinée du sandbox — seul moyen pour
le frontend de connaître `sandbox_root`, nécessaire pour afficher un chemin absolu
(le frontend ne voit sinon que des chemins relatifs).
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

### Serveur LSP — `/ws/lsp/:toolchain` et tools MCP `lsp_*` (`lsp-integration`)

**Un process LSP par toolchain, partagé entre tous les clients** (`sandbox/src/lsp.rs`,
`LspManager`/`LspSession`) — pas un process par session. `get_or_spawn` rend la
session vivante existante ou en spawn une nouvelle (course au spawn concurrent
gérée : la seconde tâche à finir accepte la session déjà posée par la première).
Multiplexage par client (`ClientId`, `subscribe`/`unsubscribe`) : les `id` JSON-RPC
entrants sont réécrits en id de session (`pending: HashMap<session_id, (ClientId,
orig_id)>`) pour router chaque réponse vers le bon abonné sans collision entre
clients. `try_mark_initialized` (test-and-set atomique) garantit qu'un seul client
envoie `initialize` au process, peu importe lequel s'y prend en premier. Toutes les
sessions voient le même état — le LSP dispatch sur le code réel de la sandbox, pas
une vue par onglet.

Deux surfaces consomment ce process unique :
- **Navigateur** — `GET /ws/lsp/:toolchain` (`sandbox/src/ws/lsp.rs`), même
  middleware ticket que `/ws/fs`/`/ws/terminal`. Un message JSON-RPC par frame texte
  (le framing `Content-Length` ne concerne que le stdio du process LSP, pas la WS).
  Repli dégradé si la toolchain n'a pas de LSP configuré (close code `4004`) ou si le
  spawn échoue (`4005`).
- **LLM** — tools MCP `lsp_diagnostics`/`lsp_hover`/`lsp_definition`/
  `lsp_references`/`lsp_rename` (`sandbox/src/tools_impl.rs`, client dédié
  `sandbox/src/lsp_client.rs` qui construit ses URIs absolues directement côté
  serveur), **additifs** aux tools filesystem/search/command existants — ne les
  remplacent pas.

**Traduction d'URIs, uniquement côté bridge navigateur** (`ws/lsp.rs::rewrite_uris`) :
le navigateur ne connaît jamais `VNL_SANDBOX_ROOT` (cohérent avec `/ws/fs`, jamais de
chemin absolu exposé côté client — cf. section "WebSocket éditeur" ci-dessus), mais
LSP exige des URIs absolues dans quasi tous ses messages. Walker JSON récursif —
réécrit toute valeur string `file://…` portée par une clé `uri`/`*Uri` (pas un
allowlist de champs figé, donc valable pour tout futur type de message LSP sans
retouche) — **plus un cas spécial** pour `WorkspaceEdit.changes` (`workspace/
applyEdit`) où l'URI est une **clé d'objet**, pas la valeur d'un champ `*Uri` : ce
cas-là est réécrit séparément, le walker générique ne l'attrape pas. Direction
`ToAbsolute` (navigateur → process) / `ToRelative` (process → navigateur), les deux
sont l'inverse exacte l'une de l'autre (testé en roundtrip).

**Rename cross-file côté UI — flux custom, pas le helper du package.**
`renameSymbol`/`doRename` de `@codemirror/lsp-client` (v6.1.0, tout jeune — risque
identifié en amont, matérialisé ici) ignore silencieusement les fichiers non ouverts
dans un onglet (`workspace.getFile(uri)` → `null` → aucun `updateFile`). Décision :
`frontend/src/api/lspRename.ts` envoie sa propre requête `textDocument/rename` (API
publique du client) et applique le `WorkspaceEdit` lui-même — fichiers ouverts par
transaction CodeMirror (buffer, **pas persisté sur disque**, l'éditeur n'a pas
d'autosave), fichiers fermés par `read` (raw) + application locale des `TextEdit`
(`applyTextEditsToString`, miroir TS de `apply_text_edits` côté sandbox) + `write` de
`/ws/fs`. Séquentiel, best-effort — un fichier en échec n'interrompt pas les
suivants, pas de rollback — même contrat que `apply_workspace_edit`
(`sandbox/src/tools_impl.rs`, déjà utilisé par le tool MCP `lsp_rename`), donc pas de
nouvel endpoint batch/atomique introduit sur `/ws/fs`. Le message de statut
distingue explicitement les fichiers écrits sur disque de ceux modifiés seulement
dans l'éditeur (« non enregistré — ⌘S ») — cet éditeur n'a aucun indicateur visuel
« modifications non enregistrées » sur les onglets (cf. "Limites connues" plus bas),
un message qui ne ferait pas la différence laisserait croire le rename entièrement
persisté.

**Menu contextuel éditeur** (`ContextMenu.vue`, étend `editing-context-menus`) :
« Aller à la définition » (`jumpToDefinition` du package) et « Renommer le symbole »
(`renameSymbolFromView`), en plus des entrées couper/copier/coller déjà en place.

**Mapping toolchain LSP par chemin** (`frontend/src/components/panels/
editorLanguage.ts::lspToolchainForPath`) : réplique côté frontend le mapping
extension → toolchain/languageId de la sandbox — `.rs` → `rust`/`rust`, `.ts`/`.tsx`/
`.mts`/`.cts` → `node`/`typescript`, `.js`/`.jsx`/`.mjs`/`.cjs` → `node`/`javascript`.
Extension non couverte → `null`, mode dégradé (coloration seule, pas de LSP).

**Images toolchain = images LSP, décidé après coup (2026-08-20)** : le premier jet de
cette feature pointait `LSP_IMAGE_RUST`/`LSP_IMAGE_NODE` sur les mêmes images
toolchain génériques (`rust:slim-trixie`/`node:trixie-slim`), qui ne contiennent pas
le LSP — code complet mais non fonctionnel sur un cluster réel, confirmé en
diagnostiquant `rustup component add rust-analyzer` en pod : `Read-only file system`,
attendu, `volumes[].image` monte toujours en lecture seule (propriété K8s, pas une
contrainte vanyline) — installer un LSP au runtime dans le pod est structurellement
impossible, il faut le baker à la construction de l'image. Résolu par deux nouvelles
images publiées avec le monorepo (`toolchains/rust/Dockerfile`,
`toolchains/node/Dockerfile` — mêmes bases que les anciens défauts toolchain, LSP
ajouté au build : `rustup component add rust-analyzer` + symlink vers
`/usr/local/bin`, `npm install -g typescript-language-server`), publiées avec le même
tag que app/sandbox/controller (`.github/workflows/release.yml`). `TOOLCHAIN_IMAGE_*`
et `LSP_IMAGE_*` pointent désormais par défaut sur la **même image** par langage — un
piste écartée en cours de route : `mcr.microsoft.com/devcontainers/typescript-node`
semblait tout inclure, vérifié en lisant son Dockerfile réel, elle ne contient pas le
LSP. Contre-partie acceptée : le pod monte deux fois la même image (`/toolchains/rust`
et `/toolchains/rust-lsp`) — redondant mais inoffensif (contenu en cache côté
kubelet), pas simplifié en un montage unique pour rester sur `LspSpec`/le mécanisme de
montage déjà testé (cf. section "Serveur LSP" ci-dessus) plutôt que de le rouvrir pour
un gain marginal.

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
| `project.rs` | PVC workspace (créé ou référence vérifiée) + ServiceAccount/Role/RoleBinding `project-<name>-maint` (droit `projects/status: patch` scopé à ce seul Project, cf. sous-section "Détection de langages" ci-dessous) + Job `project-init` (clone bare + mkdir caches + `detect`, une fois) + CronJob `project-fetch` (`git fetch --prune` + `detect`, planning dérivé de `fetch_interval`) + finalizer (Job purge puis suppression du PVC créé — un PVC référencé n'est jamais supprimé). |
| `sandbox.rs` | Job `sandbox-checkout` (`git worktree add`, création de branche depuis la default branch si absente) + Pod (home Owner + worktree en subPath + **second mount du même PVC sur `repo.git`, cf. ci-dessous** + un `volumes[].image` par toolchain, **explicite ou dérivée de `project.status.languages`, cf. sous-section "Détection de langages"** + env agrégé PATH/LD_LIBRARY_PATH/caches — supprimé si `spec.suspended`, cf. ci-dessous) + Service ClusterIP (port MCP) + NetworkPolicy ingress (restreint aux pods du namespace portant `vanyline.solidite.fr/owner: <owner>`, **+ le pod `app` + le controller Ingress réel si l'Owner référence une Application, cf. sous-section dédiée**) + NetworkPolicy egress conditionnelle (WS-13, cf. ci-dessous) + **Ingress** (`sandbox-ingress-wiring`, seulement si l'Owner référence une Application — patché/supprimé explicitement selon l'état, même patron que la netpol egress) + finalizer (Job `git worktree remove`, la branche survit sur le remote). |
| `application.rs` (`controller-application-crd`) | Deployment (1 container, env assemblé depuis 3 `secretRef` OIDC/DB/cookie + `VNL_K8S_NAMESPACE` + `OIDC_REDIRECT_URL` calculé) + Service ClusterIP + Ingress + Secret cookie auto-généré si absent — cf. sous-section dédiée. Pas de finalizer, ownerReferences suffisent. |

**Clé SSH git — PVC Owner, pas un Secret dédié** (`git-integration`, 2026-08-22) :
`git_pod_template` (`project.rs`, Jobs `init`/`fetch`) monte le PVC Owner (volume
`"home"`) au même chemin `HOME_MOUNT_PATH = /home/vanyline` que le pod Sandbox — déjà
vrai avant cette feature, confirmé en l'utilisant. `GIT_SSH_COMMAND` est désormais
**inconditionnel**, pointé en dur sur `/home/vanyline/.ssh/id_ed25519` (absence de
fichier sans effet pour un remote HTTPS, juste inopérant pour un remote SSH tant que
la clé n'existe pas — provisionnée à la demande via `POST /git/ssh-key`, cf. section
"Serveur MCP"). Remplace l'ancien mécanisme (`project.spec.git_secret` → Secret K8s
monté conditionnellement sur `/git-secret`) : le champ CRD reste (compatibilité, pas
de migration forcée) mais n'est plus consommé par le controller.

Tous les Jobs git (`project.rs`/`sandbox.rs`) invoquent `vanyline-maint` (image sandbox)
en argv — jamais de `sh -c`, aucun champ de CRD ne s'interpole dans une commande shell
(cf. section "Maintenance des workspaces" ci-dessous pour l'outil lui-même).

**Presets toolchain** (`sandbox.rs::toolchain_preset`) : la recette d'env validée (PATH,
`LD_LIBRARY_PATH` deux arches, `RUSTUP_HOME`…) vit ici, pas répétée dans chaque CR —
`Toolchain.env` vide déclenche le preset si `Toolchain.name` matche (`rust`, `node`),
sinon aucune variable ; `Toolchain.env` explicite remplace le preset entièrement. La
liste de `Toolchain` elle-même peut être explicite (`spec.toolchains`) ou dérivée
automatiquement de la détection de langages — cf. sous-section "Détection de langages
et toolchains automatiques (WS-10)" ci-dessous.

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

### Détection de langages et toolchains automatiques (WS-10)

**Détection** (`vanyline-maint detect`, cf. section "Maintenance des workspaces"
ci-dessous pour l'outil) : marqueurs de fichiers sur l'arbre HEAD du clone bare —
`rust` si un `Cargo.toml` existe (racine ou n'importe quel sous-chemin, membre de
workspace compris), `js-ts` si `package.json` **ou** `tsconfig.json` existe **à la
racine uniquement** (un `package.json` imbriqué ne compte pas). **Présence
seulement, jamais de version** (décision 2026-08-15 : ni `rust-toolchain.toml`/
`rust-version`/edition, ni `.nvmrc`/`engines.node` — si le besoin apparaît, ce sera
une extension explicite, pas une déduction implicite). Périmètre définitivement
limité à Rust et JS/TS.

**Chaînage dans les Jobs `init`/`fetch`** (`project::git_pod_template`) : le pod du
Job exécute la commande git (`init` ou `fetch`) comme **initContainer**, puis
`vanyline-maint detect --workspace /workspace --project <name>` comme container
principal — toujours en argv, jamais de shell (même règle que le reste). Les Jobs
`purge`/`checkout`/`remove` restent à un seul container, inchangés.

**Remontée au status — patch dédié, pas le reconciler** : `detect` patche
directement `Project.status.{languages,detectedAt}` via l'API K8s (merge patch
JSON ciblé, `kube::Api::patch_status`), pas via `compute_status`/le reconciler
Project. Nécessaire car `compute_status` tourne sur un rythme différent (chaque
reconcile, ~300s) et ne connaît rien de la détection. Piège évité : `ProjectStatus`
sérialiserait `languages`/`detectedAt` à leur valeur par défaut (`[]`/`null`) à
chaque reconcile de routine, et un merge patch écraserait alors ce que `detect` a
écrit — les deux champs portent donc `skip_serializing_if` pour qu'une valeur par
défaut n'apparaisse jamais dans le corps du patch, laissant le merge JSON (RFC
7386) intact sur ces clés. Symétriquement, le patch dédié de `detect` ne doit
**jamais** sérialiser un `ProjectStatus` complet (écraserait `cloned`/`worktrees`/
`conditions`) — il construit son JSON à la main avec seulement ces deux clés.

**RBAC des Jobs** : `vanyline-maint` n'avait auparavant aucun droit K8s (pur
filesystem/git). `detect --project` a besoin de `projects/status: patch` — le Job
tourne donc avec un ServiceAccount dédié `project-<name>-maint`, un `Role`
namespaced scopé via `resourceNames: [<name>]` (pas de droit sur les autres
Projects du même namespace) et un `RoleBinding`, tous les trois avec
`ownerReference` vers le Project (GC en cascade à sa suppression) — même patron
que `application::build_application_service_account`/`build_application_role`.

**Toolchains automatiques** (`sandbox.rs::effective_toolchains`) : si
`Sandbox.spec.toolchains` est non vide, il est utilisé tel quel — **jamais
fusionné** avec la dérivation automatique (tout ou rien). C'est le mécanisme par
lequel un utilisateur choisit une image de toolchain custom (version pinnée,
registry privé) quand le défaut ne convient pas. Sinon, dérivé de
`project.status.languages` : `rust` → toolchain `rust` (image
`TOOLCHAIN_IMAGE_RUST`, défaut `docker.io/library/rust:slim-trixie`), `js-ts` →
toolchain `node` (image `TOOLCHAIN_IMAGE_NODE`, défaut
`docker.io/library/node:trixie-slim`) — mêmes presets d'env que le mode manuel
(`toolchain_preset`), ordre fixe rust puis node. Les deux images par défaut sont
des flags CLI du controller (`env` clap), surchargeables sans rebuild, recette
alignée sur `deploy/sandbox/sandbox-test.yaml`.

**LSP par toolchain** (`lsp-integration`, `Toolchain.lsp: Option<LspSpec>` —
`{ image, bin, args }`, `crds/src/lib.rs`) : résolution (`resolve_toolchain_lsp`,
même forme que `resolve_toolchain_env`) — `toolchain.lsp` explicite s'il est
renseigné (LSP custom possible, y compris hors rust/node) ; sinon preset par
`toolchain.name` (`image` depuis `ctx.lsp_image_rust`/`ctx.lsp_image_node`, flags CLI
`LSP_IMAGE_RUST`/`LSP_IMAGE_NODE` — l'image doit rester configurable au déploiement,
contrairement à `bin`/`args` qui sont hardcodés : `rust-analyzer` sans args,
`typescript-language-server --stdio`) ; sinon `None` (pas de route `/ws/lsp` montée
pour cette toolchain, éditeur en mode dégradé). S'applique uniformément que
`spec.toolchains` soit explicite ou dérivé — zero-config pour rust/node dans les deux
cas. Monté en volume image séparé, `/toolchains/<name>-lsp` (à côté de
`/toolchains/<name>` du toolchain lui-même) ; découverte côté sandbox via un seul env
JSON `VNL_LSP_TOOLCHAINS` (`[{name, bin, args}]`, pas d'interpolation shell, argv
array au spawn — cf. section "Serveur LSP" plus haut pour le process manager
consommateur).

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

**`Role` de `app` — verbes tenus au strict** (`build_application_role`,
namespaced, jamais `ClusterRole`) : CRUD sur `owners`/`projects`, CRUD + `patch`
(suspension) + `watch` sur `sandboxes`, `get` seul sur `applications` (`app` ne
modifie jamais sa propre CR). Le `watch` a été ajouté par `sandbox-state-ws` pour
le hub WS `/api/ws/sandbox-state` (cf. section `vanyline-app`) — la
`ClusterRole` du controller détient déjà `watch` cluster-wide sur `sandboxes`,
donc la délégation namespacée n'est pas une escalade.

### Ingress par Sandbox (`sandbox-ingress-wiring`)

`Owner.spec.application_ref: Option<String>` (cascade, même esprit que
`egress`/`project_defaults` déjà en place) — `None` = Sandbox reste ClusterIP-only
(comportement historique, pas une erreur). Si renseigné : sous-domaine par sandbox
(`{sandbox}.sandboxes.{application.spec.host}`, décision développeur — pas de routage
par chemin sur l'host de l'app), `ingressClassName`/annotations repris de
l'Application. **TLS : un Ingress = un Certificate**, même mécanisme que
`build_application_ingress` — annotation cert-manager dérivée de
`Application.spec.tls_issuer_name`/`tls_issuer_kind` + bloc `spec.tls` (secret
`sandbox-<name>-cert`), cert-manager (ingress-shim) émet lui-même le certificat.
Décision développeur (2026-08-14, revirement sur le design initial qui
pré-provisionnait un secret wildcard `*.sandboxes.{host}` hors du repo pour éviter
un risque de rate-limit/churn côté issuer ACME sous churn de sandboxes) : le risque
ne s'applique pas à un issuer CA type `self-sign`, et la cohérence avec l'Ingress de
l'app (déjà auto-géré de cette façon) l'emporte pour le cas par défaut.

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

**Dépendance d'infra externe, hors périmètre de ce repo** : DNS wildcard
`*.sandboxes.{host}` doit exister (provisionné ailleurs) — sans lui, l'Ingress créé
ne sert à rien en pratique. Le certificat TLS, lui, est désormais auto-provisionné
(cert-manager doit être présent dans le cluster, même prérequis que pour l'Ingress
de l'app).

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
merge/push **automatique** des branches par le *controller* (il gère la plomberie git —
clone/fetch/worktree —, pas le contenu ; toujours vrai). Ne pas confondre avec le merge/
push **manuel, déclenché par l'utilisateur** désormais possible depuis la sandbox
(`POST /git/merge`, `POST /git/push`, cf. section "Serveur MCP" ci-dessus,
`git-integration`) — composant différent, portée différente. Pas d'openvscode-server
dans le pod. Changement de spec Sandbox = recréation
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
| Chat | [Nuxt UI](https://ui.nuxt.com) — composant `Chat`, cf. section dédiée plus bas |
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
(`provide`) le client `/ws/fs` partagé et le nom de la sandbox ; Explorer/Editor les
`inject`ent. Pattern à réutiliser pour tout futur état partagé entre panneaux dockview,
pas un cas particulier.

**Editor multi-onglets** : un panel dockview par fichier ouvert (id `editor:<path>`,
posé dans le groupe centre via `params: { path }` — pas un état partagé injecté).
`IdeShell.openFile(path)` réactive l'onglet s'il existe déjà (`api.getPanel(id)`), le
crée sinon ancré sur un onglet fichier déjà ouvert ou, à défaut, sur le panel Workflow
(seul panel fixe du groupe centre — cf. `centerAnchor`/`relativeToCenter`). Chaque
instance d'`Editor.vue` reçoit son `path` via les props dockview (`params`, `api`) et
ne (ré)enregistre `saveActiveFile` (l'action Ctrl+S/menu) que lorsqu'elle devient active
(`api.onDidActiveChange`) — nécessaire dès qu'il existe plusieurs instances
simultanées, sans quoi `registerIdeActions` (fusion "dernier appelant gagne") pointerait
vers le dernier onglet **monté**, pas le dernier **actif**.

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

**Menus contextuels et affordances d'édition** (`editing-context-menus`) : composant
partagé [`ContextMenu.vue`](../frontend/src/components/ContextMenu.vue) (`reka-ui`
`ContextMenu*`), même pattern déclaratif que `MenuBar.vue` (liste d'entrées `{label,
action}` + séparateurs). Appliqué à l'arbre (nouveau fichier/dossier, copier chemin
relatif/absolu, renommer, supprimer avec confirmation), à l'éditeur (couper/copier/
coller sur la sélection CodeMirror, copier le chemin du fichier), et au terminal
(copier la sélection xterm, coller dans le PTY). **Sélecteur de style partagé avec
`Menubar*`** : l'attribut réellement rendu par `ContextMenuContent` est
`[data-reka-menu-content]` — pas `[data-reka-context-menu-content]`, à ne pas deviner
par analogie avec `[data-reka-menubar-content]` (même piège de forwarding de `class`
que documenté pour `Menubar`, cf. plus bas). Les **onglets** dockview utilisent en
revanche le menu contextuel **natif** de dockview (`getTabContextMenuItems`), pas ce
composant — envelopper les tabs internes de dockview dans un `ContextMenu` reka-ui
aurait demandé un rendu custom des tabs pour un gain nul.

Renommer un fichier actuellement ouvert ferme son onglet plutôt que de mettre à jour
le `path` du panel en place — `Editor.vue` le fige à la création (cf. "Editor
multi-onglets" plus haut), changer cet invariant pour ce seul cas n'en valait pas la
peine. Coller (menu contextuel éditeur) remplace explicitement la plage sélectionnée
(`changes: {from, to, insert}`) — une version initiale insérait seulement à la tête de
sélection sans la supprimer, un comportement qui divergeait du Ctrl+V natif du
navigateur. Le menu "Édition" de `MenuBar.vue` (Rechercher/Remplacer) route les deux
vers le même panneau CodeMirror (`openSearchPanel`) : il n'existe pas d'API publique
pour ouvrir directement avec le champ remplacement visible. Icônes de fichier par
extension (`fileIcon.ts`, arbre) : réutilisent les icônes génériques déjà présentes
via `@element-plus/icons-vue` plutôt qu'un pack d'icon-theme dédié — pas de
correspondance visuelle par langage (ex. Rust → icône générique "connexion"), accepté
en l'état, pas un pack de logos par langage.

**Chat — contexte de conversation, tools sandbox, Nuxt UI** (`chat-app-fonctionnel`) :
trois axes livrés ensemble.

- **Contexte de conversation** : table `vanyline_chat_contexts` (`kind`/`data` JSONB,
  entité SeaORM depuis `miryad-core-integration`), `conversations.context_id NOT NULL`. `kind
  = "sandbox"` (`data = { "sandbox_name": "..." }`) est le seul type géré aujourd'hui —
  le modèle est volontairement polymorphe pour accueillir un futur contexte "settings"
  (chat d'aide au paramétrage) sans migration de schéma. `POST /api/conversations`
  exige ce contexte ; `GET /api/conversations?sandbox_name=...` filtre l'historique en
  conséquence (avant cette feature, l'historique de chat d'un utilisateur était global,
  sans distinction de sandbox). Côté frontend, `Chat.vue` lit le nom de la sandbox
  courante via `inject('sandbox-name', ...)` (même pattern qu'`Explorer.vue`).
- **Tools sandbox réellement utilisables** (`app/src/ws/chat.rs::resolve_extra_mcp`) :
  jusqu'à cette feature, `SessionContext.extra_mcp` restait `Vec::new()` en dur côté
  `app` — un agent pouvait référencer des tools sandbox dans sa config sans jamais
  pouvoir les utiliser en pratique. Résolution désormais dynamique via
  `VnlK8sClient::sandbox_mcp_url` (`lib/src/k8s.rs`, déjà utilisée côté CLI par
  `--toolbox`, jamais appelée côté `app` avant ce commit), avec le même scoping IDOR
  que `api::sandboxes::get_sandbox` (`project.spec.owner == owner` de l'utilisateur
  authentifié) — **trouvé en revue Phase 3** : la première version résolvait la
  sandbox nommée dans le contexte sans vérifier qu'elle appartenait à l'utilisateur,
  ce qui aurait permis à un utilisateur authentifié de faire résoudre les tools MCP
  de la sandbox de quelqu'un d'autre en la nommant dans le contexte à la création.
  Un échec de résolution (sandbox absente, hors périmètre, K8s injoignable) est non
  bloquant pour le tour : `ChatEvent::ToolUnavailable { server, reason }` (nouvelle
  variante, non terminale contrairement à `Error`) le signale à l'UI. Fix connexe dans
  `lib/src/prefixed_mcp.rs::connect_mcp_servers_selected` : retourne désormais aussi
  les échecs de connexion MCP (avant, `tracing::warn!` seul les absorbait
  silencieusement — un serveur MCP indisponible ne produisait aucun signal utilisateur).
- **Composant Chat** : `vue-advanced-chat` (pensé chat humain-humain — bulles,
  statuts lu/distribué) remplacé par le composant `Chat` de [Nuxt UI](https://ui.nuxt.com),
  conçu pour du chat LLM (tool calls, reasoning, streaming, markdown). Décision actée
  avec le développeur après correction d'une annonce erronée : contrairement à ce qui
  avait été dit en validant le plan, Nuxt UI **exige** Tailwind CSS + un wrapper
  `<UApp>` autour de toute l'app (pas seulement le panneau Chat) — accepté en
  connaissance de cause, coexiste avec Element Plus/Reka UI sans conflit constaté.
  `VanylineChatTransport` (`frontend/src/api/chatTransport.ts`) est le pont entre le
  WS existant d'`app` (`ChatEvent`, JSON custom) et le protocole `ChatTransport`/
  `UIMessageChunk` du [Vercel AI SDK](https://ai-sdk.dev) qu'attend le composant Nuxt
  UI (`ai` + `@ai-sdk/vue`) : une connexion WS par tour (`sendMessages` = un tour), pas
  de connexion partagée entre tours — le backend (`run_socket`, boucle sur les messages
  entrants) supporte les deux, pas de gain de latence mesurable à partager ici, et ça
  évite de gérer l'état d'une connexion à cheval sur plusieurs tours. `tool_call`/
  `tool_result` sont mappés en tool `dynamic-tool` (`dynamic: true` explicite sur le
  chunk `tool-input-available`) — les noms de tools viennent du MCP de la sandbox, pas
  d'un jeu de tools déclaré statiquement côté frontend. `ChatSession.vue` (un `useChat`
  par conversation, remonté via `:key="activeConversationId"` plutôt que de gérer la
  réinitialisation d'état à la main) appelle `stop()` à l'unmount — sans ça, fermer la
  session ou changer de conversation en plein streaming laissait le WS ouvert côté
  navigateur jusqu'au `done`/`error` naturel du tour. `skill_loaded`/`subagent_*`/
  `usage` n'ont pas d'équivalent dans le modèle `UIMessage` de l'AI SDK (pensé
  mono-agent) : ignorés pour ce v1, pas encore tranché pour une itération suivante.
  `Markdown` (`@comark/vue`) a un `setup()` async — rendu sous `<Suspense>`, obligatoire
  sinon Vue n'affiche rien et avertit en silence.
- **Paramètres de profil de modèle** (`ModelProfilesScreen.vue`) : `ModelProfile.options`
  (JSONB, déjà exposé de bout en bout côté API) gagne un éditeur clé/valeur libre côté
  formulaire — pas de champs typés pour `top_p`/`top_k`/`min_p`/`repeat_penalty`/
  `thinking_mode`/`reasoning_effort` etc., ces paramètres varient trop selon le backend
  LLM (Ollama/vLLM/llama.cpp). Chaque valeur est tentée en JSON (nombre, booléen) avec
  repli sur chaîne brute si elle n'est pas du JSON valide.
- **Fix post-déploiement (2026-08-18)**, trouvés en usage réel sur `media-test` :
  le raisonnement du modèle n'était pas affiché — pas un manque côté UI, `rig-core`
  0.38.1 expose bien `StreamedAssistantContent::Reasoning`/`ReasoningDelta` dans son
  stream mais `lib/src/event.rs::StreamAccumulator::apply` les jetait explicitement
  (`_ => (Vec::new(), false)`). Nouvelle variante `ChatEvent::ReasoningDelta { content
  }` (même sémantique que `Token`, canal séparé, pas accumulée dans
  `response_text`/pas persistée — même limite que les tool calls, non rechargés non
  plus à la réouverture d'une conversation) ; `VanylineChatTransport` la mappe en
  `reasoning-start`/`reasoning-delta`/`reasoning-end` (fermé par le premier `token`,
  le raisonnement précédant toujours la réponse), rendue via `UChatReasoning`. Contraste
  illisible sur les couleurs du composant Chat : `@nuxt/ui/vue-plugin` pose son propre
  plugin de dark mode (`useDark()` de `@vueuse/core`) qui suit par défaut la
  préférence système, indépendant du thème sombre fixe du reste du shell — forcé
  (`useDark().value = true` dans `main.ts`) plutôt que de suivre une préférence système
  sans rapport avec le thème de l'app (qui n'a jamais eu de mode clair).

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

**Git dans l'IDE** (`GitPanel.vue`, `git-integration`) : rail gauche à côté d'Explorer
— statut (staged/unstaged/conflits), staging, commit, branches (créer/switcher/
supprimer), push, historique. Consomme `gitClient.ts` (`/api/sandboxes/{name}/git/*`,
cf. section "Serveur MCP" pour le relais). `Explorer.vue` colore les fichiers modifiés/
conflictés à partir de `GET /git/status` (même endpoint). Diff : onglet dockview
dédié (`DiffView.vue`, pattern `diff:<path>` mêmes ancrages que `editor:<path>`),
rendu via **`@codemirror/merge`** (`unifiedMergeView`) plutôt qu'un rendu texte
maison — cohérent avec le reste de l'éditeur (`editorLanguage.ts` réutilisé pour la
coloration). Le contenu « avant » est reconstruit côté client
(`diffPatch.ts::reconstructBase`, rejoue le patch unifié à l'envers) plutôt que
demandé à la sandbox — fragile sur les cas de hunks qui ne matchent plus exactement
le working tree (silencieux, pas de repli propre), candidat à une meilleure approche
(endpoint sandbox dédié type `git show`) si ça s'avère insuffisant à l'usage.
Historique : **`@gitgraph/js`**, grain volontairement réduit à la branche courante +
refs (pas `git log --graph --all` — l'API de `@gitgraph/js` est un rejouement
d'actions, pas un import de DAG arbitraire ; le multi-branches complet resterait un
problème de layout non trivial, différé). Résolution de conflit : pas de UI 3-way —
un fichier `conflicted` s'ouvre dans l'éditeur normal (marqueurs visibles), bouton
« marquer résolu » activé par `branches.merging`.

**Limites connues (dette assumée)** : pas de reconnexion WS automatique dans le
shell IDE (déconnexion réseau/sandbox suspendue → recharger la page ; le hub de
dashboard `useSandboxState`, lui, se reconnecte avec back-off), pas de filesystem
watch/push (Explorer et GitPanel ne se
rafraîchissent pas si le contenu change côté serveur pendant que l'utilisateur
regarde — GitPanel ne refetch qu'après ses propres actions ; les dashboards
Projets/Sandboxes, eux, ont un push temps réel des phases via
`/api/ws/sandbox-state`, cf. section `vanyline-app`),
pas de code-splitting (bundle ~755 Ko gzippé — CodeMirror + xterm + Element Plus +
Tailwind CSS/Nuxt UI + dockview-vue, la CSS Tailwind/Nuxt UI ayant sensiblement
augmenté le poids par rapport à `vue-advanced-chat`, cf. section Chat plus haut).
`skill_loaded`/`subagent_*`/`usage` (`ChatEvent`) n'ont pas de rendu dans le chat —
pas d'équivalent dans le modèle `UIMessage` de l'AI SDK, cf. section Chat. Explorer
force un remount complet de l'arbre (`el-tree`) après chaque création/
renommage/suppression (changement de `:key`) — replie les dossiers dépliés à chaque
opération plutôt que de rafraîchir juste le nœud concerné. Pas d'undo applicatif sur
delete/rename dans l'arbre (irréversible côté `/ws/fs`, cf. section "Serveur MCP"),
compensé côté UI par une confirmation avant Supprimer, pas avant Renommer. Aucun
indicateur « modifications non enregistrées » sur les onglets éditeur, ni de garde à
la fermeture — préexistant, mais rendu plus sensible par le rename cross-file LSP
(cf. section "Serveur LSP" plus haut) qui peut laisser un fichier ouvert modifié en
mémoire sans autre signal que le message de statut ponctuel. LSP : images
rust-analyzer/typescript-language-server désormais construites (`toolchains/rust/`,
`toolchains/node/Dockerfile`, cf. section "Serveur LSP" plus haut), mais
fonctionnalité toujours non testée en conditions réelles (aucun cluster K8s dans
l'environnement de dev de ces sessions).

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
| `detect --workspace <dir> [--project <name>]` | marqueurs de fichiers → JSON `{"languages": [...]}` sur stdout ; si `--project` est fourni, patche en plus `Project.status.{languages,detectedAt}` via l'API K8s (cf. section "Opérateur Kubernetes" pour le détail RBAC/chaînage) |

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
- **Provider TLS rustls ambigu** (WS-10, `detect --project`) : `kube` active la feature
  `ring`, tandis qu'`axum-server`/`reqwest` (mêmes dépendances du crate `vanyline-sandbox`,
  binaire commun) activent `aws_lc_rs` — rustls voit alors deux `CryptoProvider` possibles
  et son auto-détection panique à la construction du premier `kube::Client`
  (`get_default_or_install_from_crate_features`). Fix : `vanyline-maint` installe
  explicitement `rustls::crypto::ring::default_provider().install_default()` avant tout
  `Client::try_from` — idempotent, l'erreur (provider déjà installé) est ignorée sans
  conséquence.

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

`unwrap_used`/`expect_used` n'a plus de job dédié : les 7 crates non-cli (`lib`, `app`,
`sandbox`, `tools`, `controller`, `crds`, `cfgstore`) sont tous en
`#![deny(clippy::unwrap_used, clippy::expect_used)]` directement en source — le job à
cliquet `unwrap-lint` qui a existé le temps de corriger les crates un par un a été
supprimé une fois `deny` posé partout. (Corollaire : tout `mod tests` d'un de ces crates
doit porter `#![allow(clippy::unwrap_used, clippy::expect_used)]` — oublié sur deux
modules de `cfgstore` en F2, invisible sans `--all-targets`, cf. piège plus bas.)

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
- **`git-integration` (2026-08-22), dette non bloquante survivant à la review Phase 3** :
  `CreateProjectBody.git_secret` (`app/src/api/projects.rs`) toujours accepté et
  transmis silencieusement alors que le controller ne le consomme plus (cf. section
  "Opérateur Kubernetes") — pas de warning à l'appelant, la dépréciation n'est écrite
  que côté CRD. Duplication non résolue : boilerplate `-C <root>` + conversion
  `Vec<&str>` répété ~15× dans `sandbox/src/git.rs` (un `run_git_in(root, args)`
  manque), et le bloc de scoping owner dupliqué à l'identique dans 5 handlers
  d'`app/src/api/sandboxes.rs` (`git_proxy` est le 5ᵉ). Le bouton « Diff » de
  `GitPanel.vue` est proposé sur les fichiers `deleted` mais échoue toujours
  (`DiffView.vue` lit le contenu working-tree actuel, qui n'existe plus). Pas de
  validation sur cluster réel à ce jour (comme la plupart des features livrées
  jusqu'ici, cf. entrées `.claude/memory/`).

## Workspace TypeScript (npm workspaces)

Le monorepo n'est pas que du Rust : le `package.json` racine fédère les packages
TypeScript (`workspaces: ["frontend", "packages/*"]` ; `ext/` ajouté par F3).

| Package | Type | Rôle | Stack |
|---------|------|------|-------|
| `frontend/` | app | Shell IDE web : éditeur/explorer/terminal + configuration, cliente de l'app Rust (REST + WS) — voir section "Frontend — shell IDE Vue" plus haut | Vite, Vue 3, `vue-router`, dockview-vue, CodeMirror 6, xterm.js, Element Plus, Reka UI, Vitest |
| `packages/protocol` (`@vanyline/protocol`) | lib feuille | Types partagés Rust↔TS + client RPC | TypeScript pur, zéro dépendance UI |
| `packages/ui` (`@vanyline/ui`) | lib | Composants chat + les 6 écrans de configuration + `ConfigShell`, **agnostiques du backend** (ports injectés) | Vue 3, `@nuxt/ui`, `reka-ui`, `@ai-sdk/vue`, `@comark/vue` |
| `ext/` *(F3)* | app | Extension VS Code : front-end graphique du CLI via JSON-RPC stdio | Host TS + webview Vue |

### `@vanyline/protocol`

- **`ChatEvent`** (`generated/chat-event.ts`) : défini en Rust (`vanyline-lib::event`),
  **généré par `ts-rs`** (feature `ts-rs` gated, `TS_RS_LARGE_INT="number"` dans
  `.cargo/config.toml`). Fichier commité ; job CI `tsrs` régénère + `git diff --exit-code`.
- **Enveloppes RPC** (`rpc.ts`) : miroir de `cli/src/rpc/protocol.rs` (`JsonRpcRequest/
  Response/Notification`, `InitializeParams/Result`, `ConversationSummary`,
  `ChatSendParams/Result`, table `VNL-RPC-*`). **`RpcConnection`** (`connection.ts`) :
  client ndjson **transport-injecté** (`{ write, onLine }`), corrélation `id → Promise`,
  dispatch des notifications, timeouts — aucune API Node.
- **`config-domain.ts`** : miroir TS **manuel** des 6 structs serde de
  `cfgstore/src/domain.rs` (déplacé de `lib/` en F2 ; `vanyline_lib::domain` le
  re-exporte, `domain.rs` tests `*_wire_shape` sont dans `vanyline-cfgstore`).
  `Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`/`SkillDetail`.
  Forme = serde à la lettre : discriminant `type` (pas `provider_type`/`server_type`),
  `snake_case`, **name-keyed** (`ModelProfile.provider`, `Agent.model` = noms, jamais
  des id). Plus 3 champs web-augmentés optionnels (`Provider.available_models`/`is_default`,
  `McpServer.available_tools`). Conformité vérifiée des deux côtés
  (`config-domain.conformance.spec.ts` + les tests `*_wire_shape`).
  `McpTransport = 'sse' | 'http-streamable'` : les deux côtés modélisent les deux
  variantes (`Sse` ajouté à `domain.rs` avant F2, commit `d5aaa54`).

### `@vanyline/ui` — découplage par ports injectés

Les composants ne connaissent aucun backend. Trois ports fournis par `provide`/`inject` :

| Port | Clé | Fourni par (web) | Rôle |
|---|---|---|---|
| `ChatTransport<UIMessage>` (AI SDK) | `vanyline.chatTransport` | `VanylineChatTransport` (`frontend/src/api/chatTransport.ts`) | ouverture WS `app` + délègue le mapping à `chatEventsToUIStream` |
| `ChatBackend` | `vanyline.chatBackend` | `httpChatBackend` | `listConversations` / `loadMessages` / `createConversation` (la politique de contexte — sandbox, 1ᵉʳ agent — vit dans l'impl, pas dans les composants) |
| `ConfigRepo` | `CONFIG_REPO_KEY` = `vanyline.configRepo` | `httpConfigRepo` | CRUD **name-keyed** des 6 domaines (`providers`/`profiles`/`mcp`/`toolsets`/`agents`/`skills`) + `setDefaultProvider` / `testProvider` / `testMcpServer` / `listLocalTools` |

- `ChatWindow.vue` : sélecteur de session + boutons new/close + hôte de `ChatSession.vue`
  (tour de streaming, `useChat` de l'AI SDK). `activeConversationId` en `v-model` —
  l'embarqueur le possède (web : lié au singleton `useIdeSession` ; VS Code : ref locale).
  `chatEventsToUIStream(events, { abortSignal })` : `ReadableStream<ChatEvent>` →
  `ReadableStream<UIMessageChunk>` (switch `ChatEvent` → chunks, gestion des blocs
  texte/reasoning, `abort`/`finish`). `notify-fs-change` : `inject` optionnel no-op
  (web-only, refresh de l'explorer).
- `ConfigShell.vue` : coquille de nav Settings (groupes + sous-items, slot `pending`),
  `groups: ConfigNavGroup[]` + `screens: Record<string, Component>` fournis par
  l'embarqueur, émet `nav-change`. `useCrudResource(repo, domain)` : fetch/loading/error
  + CRUD name-keyed (`create`/`update` propagent, `fetch`/`remove` capturent).
- **`httpConfigRepo`** porte **toute** la traduction wire REST `app` ↔ forme canonique :
  `provider_type`/`server_type` ↔ `type`, FK `provider_id`/`model_profile_id` ↔ nom (cache
  `name↔id` **par instance**, l'id `i32` ne sort jamais du repo), champs web-augmentés,
  `get('skills')` → `GET /{id}` pour le `body`. L'API REST de `app` est **inchangée**
  (pas de migration). L'impl RPC de `ConfigRepo` (F4) sera un pass-through — c'est ici que
  vit l'asymétrie name/id.
- **RBAC exposée sans masquage** : `create`/`update` sur `providers`/`mcp` renvoient 403
  pour un non-admin, l'écran affiche l'erreur. Côté CLI (F4) tout est local — l'UI est
  identique, la capacité diffère.
- **Restent dans `frontend/`** : `AccountScreen` (pas de compte hors web),
  `src/components/common/` + `src/composables/useCrudResource.ts` (`(client, basePath)`,
  consommés par les dashboards Projects/Sandboxes — hors périmètre de l'extraction).
  `SettingsView.vue` ≈ 20 lignes : monte `ConfigShell` + `provide(CONFIG_REPO_KEY,
  httpConfigRepo())`.

### Règles de dépendances TypeScript

1. **`packages/protocol` est une feuille** : aucune dépendance UI, consommable par
   n'importe quel client (extension, frontend, scripts de test).
2. **Un seul schéma par type partagé** : `ChatEvent` (ts-rs) et `config-domain.ts`
   (miroir manuel + fixtures de conformité) sont définis en Rust d'abord. La dérive
   Rust↔TS est un bug, pas une fatalité.
3. **Les apps dépendent des libs, jamais l'inverse** — même règle que côté Rust.
   `frontend/` dépend de `@vanyline/ui` + `@vanyline/protocol` via alias source
   (`vite.config.ts` + `tsconfig.app.json`), pas de build intermédiaire.
4. Pas de `console.log` dans les sources (logger projet), pas de code partagé par
   copier-coller entre `frontend/` et `ext/` : ce qui doit être partagé remonte dans
   `packages/`.
5. **CI** : job « Frontend + packages » — `check` + `test` de `@vanyline/protocol` puis
   `@vanyline/ui` **avant** le build `frontend`. Job `tsrs` séparé pour la vérification
   ts-rs.
