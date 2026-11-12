# Chantier lymaille — tâches d'exécution

Fichier de tâches pour l'agent d'orchestration (`spec-dd`, modèle Qwen) et ses sous-agents. Les décisions sont prises : ce
fichier ne demande aucun arbitrage. **Toute ambiguïté se remonte dans la spec concernée, elle ne se tranche pas.**

## Contexte en une minute

vanyline migre son harnais LLM vers **lymaille** : hébergé dans `app` par **miryad-core 0.3.1** (features `lymaille` et
`matrix`), et directement dans `cli` (hôte autonome, sans miryad-core). Sources de vérité, dans l'ordre :

1. Le DSD : `dsd/app.md` et `dsd/app/sessions.md` ; `dsd/cli.md` et `dsd/cli/*.md` (contrats `FR-*`, décisions `DEC-*`,
   critères `AC-*`, section « Réalisation SpecDD » qui nomme les specs à créer, adapter ou retirer).
2. Le DSD de la stack, dépôt voisin `~/projets/miryad-core/dsd/miryad-core/` (`lymaille.md`, `hooks-async.md`) et son
   `CHANGELOG.md` (0.3.0 et 0.3.1 : ruptures d'API) ; la doc de lymaille 0.2.1.
3. Les specs `.sdd` existantes. Elles décrivent l'état actuel et peuvent contredire le DSD : **le DSD fait foi**, la spec
   s'amende (`app#Transition et migration`). Le DSD lui-même ne se modifie pas.

La migration est **destructive pour le code actuel et c'est voulu** : on casse pour reconstruire. Pas de compatibilité
ascendante, pas de couche de transition, pas de migration de données. Décisions prises pour ce chantier : les fichiers YAML de
`cfgstore` ne sont pas migrés (`cli/hote#OI-3`) ; les conversations existantes ne sont pas reprises ; les écrans de configuration
actuels sont retirés avec leur backend (reconstruction : Solution `app/configuration`, non ouverte).

## Règles d'exécution

- Boucle SpecDD + TTD de `.specdd/bootstrap.project.md` : spec amendée et `specdd lint` à « 0 errors, 0 warnings »
  (`~/.npm-global/bin/specdd lint`) → tests dérivés des `Scenario`, vus échouer → code minimal → `validator` → `[x]`.
  Une spec par fichier `*.rs` ; TS/Vue au fil de l'eau. Pas de code sans spec.
- Cargo, toujours : `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` et
  `CARGO_TARGET_DIR=~/.cache/cargo-target/vanyline`. **Un seul build à la fois**, jamais deux agents qui compilent en
  parallèle (pod de 6 Gi, déjà tué par OOM). Les tâches « specs » se parallélisent librement, pas les tâches de code.
- Pas de `Co-Authored-By`. Pas de prénom dans un fichier commité. Rien n'est poussé : commits locaux, un par tâche.
- Batterie de validation complète : `AGENTS.md`, section « Commandes de validation ». `cargo clean` en fin de session.
- Harnais clippy : aucune nouvelle dette (`tooling.sdd`).

**Table propre.** Aucune production : le schéma `vanyline_*` est reconstruit, pas migré. Retirer une migration d'origine invalide les
bases de test déjà migrées (sea-orm-migration refuse une version appliquée dont le fichier manque) : après C2 et C3, les bases de
développement et d'e2e sont à recréer, ce que Qwen signale dans le message de commit sans l'exécuter.

## Remonter une question

Une question vit **dans la spec concernée** : un `[?]` dans sa section `Tasks`, avec la question, ce qui a été observé et une
proposition. Si la spec n'existe pas encore, le `[?]` est dans la spec en cours de création. Marquer la tâche liée `[?]` (ou
`[!]` si elle est bloquée), puis **continuer sur une autre tâche non bloquée**. Cas : une spec ou le DSD contredit le code ou
lui-même ; un contrat de la stack se comporte autrement que le DSD ne le dit ; un identifiant `H_` est refusé par
`identity::classify` (`sessions#DEC-9`) ; une tâche exigerait de modifier miryad-core, lymaille ou vynil-core (interdit :
on construit avec ce qu'ils livrent, `sessions#DEC-6`). Les `TBD` des Open Issues du DSD restent à l'état de `[?]` dans les
specs correspondantes, sans être tranchés.

## A — Socle (séquentiel)

- [ ] **A1 — Specs : montée de version.** Citer `miryad-core 0.3.1` partout où 0.1.5 ou 0.2.0 est cité (`vanyline.sdd`,
  `app/src/main.sdd`, `config.sdd`, `auth/mod.sdd`, `api/owners.sdd`, `api/projects.sdd`, `workflow/*.sdd`). Hooks
  `before_create`, `before_update`, `before_delete` : `async`, paramètre `db: &C` (`C: ConnectionTrait`), futur `Send`, dans
  les specs de `db/entities/{agents,llm_providers,mcp_servers,project_workflows,app_settings}.sdd` et leurs Scenario.
  `workflow/steps/{mod,agent}.sdd` : `RunInfo` gagne `vars` et `trigger`. `error.sdd` : variante `ScriptTimeout` de
  `StepError` si le code la rencontre.
- [ ] **A2 — Code : bump et compilation** (après A1). `app/Cargo.toml` : `miryad-core = "0.3.1"`, features `swagger-ui`,
  `workflow`, `mcp`, **`lymaille`, `matrix`**. Adapter les cinq entités à hooks async, les fixtures `RunInfo`
  (`workflow/steps/rhai.rs`, `agent.rs`, `mcp_domain.rs`), tout `match` sur `PrincipalSource`. Valider : `cargo check
  --workspace`, puis la batterie complète.
- [ ] **A3 — Test de migrations** (après A2). Dans `app/src/db/test_support.rs` (et sa spec), vérifier qu'après
  `miryad_core::migration::Migrator::up` puis `crate::migration::Migrator::up` les tables `miryad_lymaille_*` existent et que la
  seconde passe est sans effet. Aucune migration `vanyline_*` à écrire pour elles (`sessions#CON-2`). La vérification sur
  PostgreSQL réel est faite à part, lors d'un déploiement de test (`docs/release-runbook.md`).
- [ ] **A4 — Specs : feuille de route.** `vanyline.sdd`, `Tasks` : clore comme caduques « Agents système », « Autonomie des
  agents » et « Démarrer l'étape 9 (`llm-config`) » (remplacés par ce chantier, `app#DEC-1`), retirer `llm-config` de la liste
  des membres à venir, ajouter un pointeur vers ce fichier.
- [ ] **A5 — Docs.** `docs/architecture.md` (l. 233 et 278) et `docs/llm-exec-gap.md` (l. 84 et 117) : remplacer les passages
  « philosophie yolo, `permission` sans objet » par « permissions fixées par le contexte, pas par une approbation en session »
  (`app#BR-2`).

## B — `app` : sessions (`dsd/app/sessions.md`)

Specs, **parallélisables entre elles et avec A** :

- [ ] **B1 — `app/src/sessions/mod.sdd`** : `FR-1`, `FR-12`, `CON-1`, `CON-2`, `DEC-1` (montage, routes `/api/v1/lymaille`).
- [ ] **B2 — `app/src/sessions/host.sdd`** : `FR-2` à `FR-4`, `DEC-2`, `DEC-3`, `DEC-9`, `AC-1` à `AC-3`.
- [ ] **B3 — `app/src/sessions/catalog.sdd`** : `FR-5` à `FR-7`, `DEC-6`, `DEC-7`, `AC-4`, `AC-5`.
- [ ] **B4 — `app/src/sessions/events.sdd`** : `FR-8`, `FR-9`, `DEC-4`, `AC-6`.
- [ ] **B5 — Amendements** : `app/src/main.sdd` (montage), `app/src/config.sdd` (plafond de durée des sous-agents, `FR-11`,
  `AC-8`), `controller/src/application.sdd` (une réplique pour `app`, `DEC-1`).

Code, **séquentiel**, après A2 et la spec correspondante :

- [ ] **B6 — `LymailleHost`** (`host.rs`) : réutilise `VnlK8sClient::owner_token` et `sandbox_mcp_url` ; lien
  utilisateur↔Owner par `vanyline_owner_links`.
- [ ] **B7 — Catalogue** (`catalog.rs`) : types de contexte, contextes `G_home` et `G_config`, agents `G_vanyline_*`, création
  idempotente au démarrage, table écran → agent pour Project et Sandbox (`DEC-6`).
- [ ] **B8 — Événements** (`events.rs`) : `HostEvent{kind, payload}`, publication par `EventHub::publish_host`, export
  TypeScript (`export_typescript`) vers `packages/protocol/src/generated/`, job CI `tsrs` étendu (spec
  `.github/workflows/workflows.sdd`).
- [ ] **B9 — Montage** (`mod.rs`, `main.rs`, `config.rs`) : état miryad-core avec l'hôte, routes de conversation, plafond de
  durée des sous-agents. **Fin de tranche mince** : `AC-1` à `AC-4` verts.

## C — `app` : retrait du tooling LLM maison (déclenché par B9, `sessions#DEC-8`)

Specs (parallélisables après B9), puis code séquentiel :

- [ ] **C1 — Nœud `agent` de workflow → step `subagent` de miryad-core.** Specs `workflow/steps/agent.sdd`, `workflow/mod.sdd`,
  `mcp_domain.sdd` ; code `workflow/steps/agent.rs`. Le type de contexte de liaison est `SUBAGENT_CONTEXT_TYPE`
  (`miryad-core#CHANGELOG 0.3.0`).
- [ ] **C2 — Chat maison.** Retirer `ws/chat.rs`, `api/conversations.rs`, `system_agents.rs`, `api/local_tools.rs`, les entités
  `chat_contexts`, `conversations`, `messages` **et leurs migrations d'origine** (`m20260825_000007` à `000009` et leurs `.sdd`),
  retirées de `migration/mod.rs` : table propre, aucune migration `drop` ; specs associées.
- [ ] **C3 — Configuration LLM maison.** Retirer `config_store.rs`, les entités et l'API de `agents`, `llm_providers`,
  `mcp_servers`, `model_profiles`, `skills`, `toolsets` (remplacées par les ressources de miryad-core, `miryad-core#DEC-14`),
  migrations d'origine retirées de la même façon (`m20260825_000001` à `000006`, hors `000010` `owner_links` qui reste ; la
  migration `m20261005_000003` qui altère `agents` disparaît avec elle). `app_settings` : conserver (`[?]` dans sa spec : les réglages globaux restent-ils nécessaires ?).
- [ ] **C4 — Tests.** Les tests supprimés avec le code ne sont pas remplacés par d'autres que ceux des Scenario des specs
  nouvelles ou amendées ; la batterie complète reste verte.

## D — Frontend (après B8 pour les types, B9 pour le serveur)

- [ ] **D1 — Specs TS** : adaptateur REST + SSE du port `ChatTransport` de `@vanyline/ui` (historique puis SSE, `FR-10`,
  `DEC-5`) ; types générés dans `@vanyline/protocol` ; `packages/ui/chat/` sur les événements de lymaille (fin de
  `chatEventsToUIStream.ts` sur `ChatEvent`).
- [ ] **D2 — Code** : l'adaptateur, le chat, et l'ouverture d'une session depuis chaque écran de contexte par `(contexte, agent
  optionnel)` sans sélecteur d'agent (`FR-5`).
- [ ] **D3 — Retrait** (après C2 et C3) : `frontend/src/api/chatTransport.ts`, `chatWs.ts`, `generated/chat-event.ts`, écrans de
  configuration `packages/ui/config/` et leurs appels, devenus sans backend.

## E — CLI : hôte lymaille autonome (`dsd/cli*.md`) — indépendant de miryad-core, parallèle à B, C et D

La CLI ne dépend que de lymaille 0.2.1. Specs d'abord (parallélisables) :

- [ ] **E1 — Specs `cli/src/host/`** (`dsd/cli/hote.md`, section « Réalisation SpecDD ») : stores de session et d'index, de
  catalogue, de toolsets et de contextes, de mémoires, secrets, contexte dérivé du cluster, jeton d'Owner, journal d'audit.
  Les `OI-1`, `OI-2`, `OI-4` de ce DSD (disposition d'une mémoire, format de `secrets.yaml`, version du format) restent des
  `[?]` dans ces specs.
- [ ] **E2 — Specs TUI** (`dsd/cli/chat.md`) : adapter `tui/{state,view,sink,mod}.sdd` ; remplacer `chat.sdd`, la partie chat de
  `main.sdd` et `tools.sdd`.
- [ ] **E3 — Specs `run` et `script`** (`dsd/cli/batch.md`) : une spec pour `run`, une pour `script`, une pour les codes de sortie ;
  réduire `chat.sdd` et la grammaire de `run` dans `main.sdd`.
- [ ] **E4 — Specs objets** (`dsd/cli/objets.md`) : `owner_cmd.sdd`, `project_cmd.sdd`, `sandbox_cmd.sdd` avec `apply` et `edit`.
- [ ] **E5 — Specs `sync`** (`dsd/cli/sync.md`) : client `/ws/fs` et ticket, comparaison à trois, liste des empreintes, `sync push`
  et `sync pull`.

Code, **séquentiel**, dans cet ordre (chaque étape après sa spec) :

- [ ] **E6 — Hôte** : implémentations des traits de stockage sur fichiers, secrets, contexte dérivé, jeton d'Owner, audit.
- [ ] **E7 — TUI sur lymaille** : backend remplacé, adaptateur d'événements, approbation en session retirée.
- [ ] **E8 — `run` et `script`.**
- [ ] **E9 — Commandes objets** (`apply`, `edit`) et **E10 — `sync`.**
- [ ] **E11 — Retrait** : crate `cfgstore` du workspace, `cli/src/store.rs`, `config.rs`, `config_check.rs` et leurs specs.

## F — `lib` allégée : librairie d'interface avec le cluster et les sandboxes (après C et E)

- [ ] **F1 — Spec** : réécrire l'identité de `lib/src/lib.sdd` (client Kubernetes, jeton d'Owner, sandbox) ; retirer des specs
  `session`, `event`, `prefixed_mcp`, `model`, `types` (`.claude/memory/inventaire-vanyline-lymaille-2026-11.md`).
- [ ] **F2 — Code** : supprimer ces fichiers de `lib` ; `k8s.rs` reste ; `error.rs` est trié (`[?]` pour ce qui n'est pas évident).
  Retirer `lib` du graphe de dépendances LLM de `app` et de `cli`.
- Hors tâche : l'unification du client de sandbox entre `cli` et les relais d'`app` (« piste à valider » de l'inventaire).

## G — `app` : tableaux de bord (`dsd/app/dashboards.md`) — indépendant de B pour tout sauf `FR-9`

Specs, **parallélisables entre elles et avec tout le reste** :

- [ ] **G1 — `app/src/dashboards/{mod,model,registry,defaults,resolve}.sdd`** : `FR-1` à `FR-4`, `FR-11`, `DEC-2`, `DEC-5`.
- [ ] **G2 — Entité et migration** : `db/entities/dashboards.sdd`, `migration/m<date>_create_vanyline_dashboards.sdd`, ressource
  `MiryadResource` en `OwnerOnly` (`DEC-3`). Le montage de la route `resolve` va dans `main.sdd`.
- [ ] **G3 — Specs frontend** : moteur de rendu, hôte de widget, un composant par widget standard (`FR-6`, `FR-7`, `FR-8`,
  `FR-10`), route des tableaux complémentaires ; types `dashboard-domain` dans `@vanyline/protocol`.

Code, **séquentiel**, après A2 et la spec correspondante :

- [ ] **G4 — Modèle, registre, défauts, résolution** (`app/src/dashboards/`), entité, migration, ressource, route `resolve`.
- [ ] **G5 — Types** : export ts-rs vers `packages/protocol/src/generated/`, job CI `tsrs` étendu.
- [ ] **G6 — Moteur de rendu et widgets standard** (après G5) : un widget à la fois, chacun avec ses tests.
- [ ] **G7 — Écrans par défaut** : accueil, projet et vue d'ensemble de sandbox en définitions par défaut ; retrait de
  `HomeDashboard.vue`, `ProjectDashboard.vue` et des specs et tests associés (`DEC-7`). `FR-9` (tableau éphémère) attend B8.

## Non ouvert

Les Solutions `app/widgets`, `app/ui-actions`, `app/artifacts`, `app/workflows`, `app/review`,
`app/planning`, `app/configuration` ne sont pas écrites : aucune tâche d'exécution n'existe pour elles. Elles s'ouvriront quand
leur Solution (`dsd/app/<nom>.md`) existera. Ne pas en rédiger.
