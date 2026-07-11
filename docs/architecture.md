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
| `vanyline` (bin `cli`) | binaire | CLI standalone de chat/agents | REPL + one-shot, `CliConfigStore` (adapte les fichiers JSON `~/.config/vanyline` en `ConfigStore`), enveloppe les `vanyline-tools` en `ToolDyn` locaux |
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
  `CliConfigStore` (cli, lit les fichiers JSON existants) et `PgConfigStore` (app, requête
  le schéma PostgreSQL existant). Aucun des deux n'a introduit de nouveau format de
  stockage — ce sont des adaptateurs mécaniques vers le modèle name-keyed, qui
  **synthétisent** `ModelProfile`/`Toolset` (absents des schémas d'origine, un par agent,
  conventionnellement nommé comme l'agent lui-même). Le vrai stockage natif (YAML
  layered pour le CLI, schéma PG étendu pour l'app) est différé à des features séparées
  (`cli-harness.md`, `app-harness-parity.md`).
- `sink: Arc<dyn EventSink>` — un seul type d'événement, `ChatEvent`, pour tous les
  transports (REPL stdout, WebSocket, futur JSON-RPC stdio). `EventSink::emit` est
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
- **Pas d'annulation** : `run_agent_turn` ne prend pas de token d'annulation ; son ajout
  futur (requis par un éventuel RPC `chat/cancel`) devra rester compatible signature
  (paramètre sur `SessionContext` plutôt que sur la fonction).
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
