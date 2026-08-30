# Feature — F1-vscode-ext-foundations

Première des cinq features de la famille « extension VS Code `vanyline` ». Séquence
complète et état d'avancement : `.claude/memory/vscode-ext-sequence.md`.

## Ce que la feature fait

Extrait les composants Chat et les 6 écrans de configuration du `frontend/` dans un
package partagé `packages/ui` **agnostique du backend**, et crée `packages/protocol`
(types TS générés depuis Rust). Le frontend web continue de fonctionner à l'identique.

## Ce qu'elle ne fait pas

- **Aucune ligne d'extension VS Code** (F3) ni de webview.
- **Aucune nouvelle méthode RPC CLI** (F2).
- Pas de refonte visuelle : extraction à l'identique, mêmes specs qui passent.
- **Pas de migration DB.** Les contraintes `UNIQUE(owner_id, name)` existent déjà sur
  agents / toolsets / model_profiles / skills. `llm_providers` et `mcp_servers` sont des
  ressources globales sans `owner_id` (RBAC `read=Public` / `write=AdminOnly`, décidé en
  `miryad-core-integration`) — cette asymétrie est **laissée telle quelle**, transparente
  pour un `ConfigRepo` name-keyed.
- Pas de `dockview` / `vue-router` / panels IDE (Explorer/Editor/Terminal/Git/LSP) dans
  `packages/ui` — ils restent spécifiques à `frontend/`.
- Pas de `packages/config-shell` : la coquille de navigation Settings (`SettingsView.vue`,
  nav gauche + groupes) **est** extraite ici en composant neutre `ConfigShell` (elle est
  déjà générique) — pour éviter que F4 la réinvente.

## Layout cible

```
vanyline/
├── package.json            workspaces: ["frontend", "packages/*"]   (ext ajouté en F3)
├── packages/
│   ├── protocol/           @vanyline/protocol — TS pur, zéro dépendance UI
│   │   └── src/
│   │       ├── generated/chat-event.ts   généré ts-rs, commité, vérifié CI
│   │       ├── rpc.ts                     enveloppes JSON-RPC, InitializeResult, etc.
│   │       ├── config-domain.ts           6 types config (miroir manuel de lib/src/domain.rs)
│   │       └── connection.ts              client ndjson (corrélation id→promesse)
│   └── ui/                 @vanyline/ui — Vue 3, agnostique du backend
│       └── src/
│           ├── chat/       ChatWindow.vue, ChatSession.vue, chatEventsToUIStream.ts
│           ├── config/     6 écrans + AccountScreen + ConfigShell.vue (types ré-exportés de @vanyline/protocol)
│           ├── common/     Field, DialogShell, ErrorCard, EmptyState, LoadingSkeleton, CheckboxList
│           ├── composables/ useCrudResource.ts  (repo: ConfigRepo, domain) — distinct de celui de frontend/
│           └── ports.ts    interfaces ConfigRepo + ChatBackend + ré-export du ChatTransport AI SDK
└── frontend/               → dépend de @vanyline/ui + @vanyline/protocol
    └── src/api/
        ├── httpConfigRepo.ts   impl HTTP de ConfigRepo (traduit provider_id/model_profile_id ↔ nom, jamais exposé)
        ├── httpChatBackend.ts  impl HTTP de ChatBackend (résolution sandbox + 1er agent en interne)
        └── chatTransport.ts    ne garde que l'ouverture WS + branchement sur le mapper partagé
```

## Interfaces clés

### `@vanyline/protocol`

- `ChatEvent` et ses sous-types (`ToolCallRecord`…) : **générés par `ts-rs`** depuis
  `vanyline-lib::event`. Feature Cargo `ts-rs` gated dans `vanyline-lib`
  (`#[cfg_attr(feature = "ts-rs", derive(TS))]`), fichier généré commité, job CI qui
  régénère et `git diff --exit-code`. Repli documenté (cf. risques) : types manuels +
  tests de conformité sur fixtures JSON produites par la lib.
- Enveloppes RPC (miroir de `cli/src/rpc/protocol.rs`) : `JsonRpcRequest/Response/
  Notification`, `InitializeParams/Result`, `ConversationSummary`, `ChatSendParams/
  Result`, `ChatEventNotificationParams`, table `VNL-RPC-*`.
- **Types de domaine config** (`config-domain.ts`) : miroir TS **manuel** des 6 structs
  serde de `lib/src/domain.rs` — `Provider`, `ModelProfile`, `Toolset`, `Agent`,
  `McpServer`, `SkillDetail` (`SkillMeta` + `body`). Ce sont exactement les payloads
  `item` / `patch` du RPC config de F2 (params `item: <entité snake_case>`) : une seule
  source de vérité côté protocol, `@vanyline/ui` les ré-exporte pour les écrans.
  **Forme = serde de `domain.rs` à la lettre** (décision 2026-08-30) : discriminant
  `type` (pas `provider_type`/`server_type`), `snake_case` (`api_key`, `max_tokens`,
  `local_tools`, `system_prompt`), et **name-keyed** — `ModelProfile.provider: string`,
  `Agent.model: string` (nom du profil), `Agent.toolsets/skills`, `Toolset.local_tools`,
  `Toolset.mcp[].server`. Plus 3 champs **web-augmentés optionnels** (`available_models`,
  `is_default` sur `Provider` ; `available_tools` sur `McpServer`). `httpConfigRepo` (F1)
  porte **toute** la traduction REST↔canonique ; `rpcConfigRepo` (F4) est pass-through.
  Pas de `ts-rs` ici (le scope ts-rs de F1 reste `ChatEvent` seul) ; conformité par
  fixtures JSON produites par la lib dans les tests du package, comme le repli
  `ChatEvent`.
- `RpcConnection` : client ndjson **transport-injecté** — prend `{ write(line: string):
  void; onLine(cb: (line: string) => void): void }`, ne dépend d'aucune API Node. Gère
  corrélation `id → Promise`, dispatch des notifications par méthode, timeouts.

### `ConfigRepo` (le contrat central, `@vanyline/ui/ports.ts`)

Name-keyed. Une seule interface, deux implémentations (HTTP en F1, RPC en F4).

```ts
type ConfigDomain = 'providers' | 'profiles' | 'mcp' | 'toolsets' | 'agents' | 'skills';

/** Forme typée par domaine — union discriminée par la clé `domain` passée à
 *  chaque méthode, pas un type opaque unique. Les membres sont les types de
 *  `@vanyline/protocol/config-domain.ts` (miroir de `lib/src/domain.rs`). */
interface ConfigItemByDomain {
  providers: Provider;
  profiles: ModelProfile;   // { name, provider: string, model, temperature?, max_tokens?, options? }
  mcp: McpServer;
  toolsets: Toolset;
  agents: Agent;            // { name, mode, model: string, toolsets: string[], skills, ... }
  skills: SkillDetail;      // list() renvoie SkillMeta (sans body) ; get() renvoie SkillDetail (avec body)
}
type ConfigItem<D extends ConfigDomain = ConfigDomain> = ConfigItemByDomain[D];

interface ConfigRepo {
  list<D extends ConfigDomain>(domain: D): Promise<ConfigItem<D>[]>;
  get<D extends ConfigDomain>(domain: D, name: string): Promise<ConfigItem<D>>;
  create<D extends ConfigDomain>(domain: D, item: ConfigItem<D>): Promise<ConfigItem<D>>;
  update<D extends ConfigDomain>(domain: D, name: string, patch: Partial<ConfigItem<D>>): Promise<ConfigItem<D>>;
  remove(domain: ConfigDomain, name: string): Promise<void>;
  setDefaultProvider(name: string): Promise<void>;
  testProvider(name: string): Promise<{ models: string[] }>;
  testMcpServer(name: string): Promise<{ tools: string[] }>;
  listLocalTools(): Promise<string[]>;
}
```

- Fourni aux écrans par `provide`/`inject` (clé `vanyline.configRepo`).
- Le domaine s'appelle `profiles` côté UI (côté CLI c'est `models`, côté app
  `model-profiles`) — le mapping de nom vit dans chaque impl, jamais dans les écrans.
- **Références inter-domaines : toujours des noms côté UI.** Le modèle canonique
  (`lib/src/domain.rs`, ce que parle le RPC de F2) est déjà entièrement name-keyed. Seule
  la couche REST de `app` porte deux FK numériques — `ModelProfile.provider_id` et
  `Agent.model_profile_id` — que **`httpConfigRepo` traduit ↔ nom dans les deux sens**
  (réponses `list`/`get` **et** bodies `create`/`update`), via son listing en cache. Le
  reste (`Agent.toolsets`/`skills`, `Toolset.local_tools`, `Toolset.mcp[].server`) est
  déjà en noms des deux côtés. L'API REST reste **inchangée** (pas de migration) ; l'impl
  RPC (F4) est un pass-through.
- **Détail skill (`body`)** : pas de méthode dédiée — `SkillsScreen` appelle
  `get('skills', name)` (renvoie `SkillDetail` avec `body`). `list('skills')` reste léger
  (`SkillMeta`).
- **`setDefaultProvider`** : `is_default` sur provider est un concept **web-only**
  (absent de `domain::Provider` — la CLI a un agent par défaut, pas de provider par
  défaut). Impl HTTP → `PUT /api/v1/llm-providers/{id}/default`. Impl RPC (F4) → rejet
  « non supporté » (même précédent que la RBAC ci-dessous).
- Les états locaux keyés par id dans les écrans (`discovering[id]`, `testResults[id]`,
  `seenToolResults`) passent à une clé `name` — état de composant, hors contrat.
- **Champs augmentés au runtime web** : `Provider.availableModels: string[]` +
  `Provider.isDefault: boolean` + `McpServer.availableTools: string[]` ne sont pas dans
  `domain.rs` (persistés côté `app` après un test / `set-default`). `config-domain.ts`
  les porte en **optionnels documentés web-authoritative** ; l'impl RPC (F4) renvoie
  `[]` / `false`. Les écrans les lisent en lecture seule (jamais dans un body
  `create`/`update`).
- **`useCrudResource`** : le composable actuel (`frontend/src/composables/`) est aussi
  consommé par `HomeDashboard`/`ProjectDashboard` sur des ressources **non-config**
  name-keyed nativement (`/api/projects`, `/api/sandboxes`). Il **reste tel quel** pour
  elles. Les écrans config utilisent un **nouveau** `@vanyline/ui/composables/
  useCrudResource.ts` de signature `(repo: ConfigRepo, domain: ConfigDomain)`.
- `ConfigRepo` **expose la RBAC du backend sans la masquer** : côté web, `create`/
  `update` sur `providers`/`mcp` renvoient 403 pour un non-admin — l'écran affiche
  l'erreur, c'est le comportement actuel. Côté CLI (F4) tout est local et non restreint.
  L'UI est identique, la capacité diffère.

### `ChatTransport`

L'interface `ChatTransport<UIMessage>` de l'AI SDK, réutilisée telle quelle.
`packages/ui` en reçoit une instance par `provide`/`inject` (clé `vanyline.chatTransport`).
`chatEventsToUIStream(ws-or-source)` : extraction de la partie générique de
[`chatTransport.ts`](../../frontend/src/api/chatTransport.ts) (le `ReadableStream<
UIMessageChunk>` et le switch `ChatEvent` → chunks, lignes 56-193). Ne connaît ni WS ni
RPC — prend une source d'événements `ChatEvent` déjà décodés + un `abortSignal`.

### `ChatBackend` (le contrat session/historique, `@vanyline/ui/ports.ts`)

Symétrique de `ConfigRepo`. `Chat.vue` et `ChatSession.vue` sont couplés au backend
au-delà du transport : liste des conversations (`GET /api/conversations`), création
(`POST /api/conversations` + résolution du 1ᵉʳ agent via `GET /api/v1/agents`),
historique (`GET /api/conversations/{id}/messages`). Ces trois opérations passent par un
port injecté ; les deux composants partent **entièrement** dans `@vanyline/ui`
(`ChatWindow.vue` = sélecteur + boutons new/close + hôte de session, `ChatSession.vue` =
tour de streaming).

```ts
interface ChatConversation { id: string; title: string | null; createdAt: string }
interface ChatMessageRecord { id: string; role: 'user' | 'assistant'; content: string }

interface ChatBackend {
  /** Conversations du sélecteur, plus récentes d'abord. */
  listConversations(): Promise<ChatConversation[]>;
  /** Historique persisté d'une conversation, ordre chronologique. Vide pour une neuve. */
  loadMessages(conversationId: string): Promise<ChatMessageRecord[]>;
  /** Crée une conversation, renvoie son id. L'impl porte son propre contexte :
   *  web = contexte sandbox + résolution du 1ᵉʳ agent configuré (aujourd'hui dans
   *  `useIdeSession.startAgentSession`) ; CLI (F4) = agent du YAML. Aucun paramètre
   *  côté port — la politique de contexte ne remonte jamais dans les composants. */
  createConversation(): Promise<string>;
}
```

- Fourni par `provide`/`inject` (clé `vanyline.chatBackend`).
- `activeConversationId` devient un **`v-model`** sur `ChatWindow.vue` — l'embarqueur le
  possède. Côté web, `Chat.vue` (fin wrapper restant dans `frontend/`, ou câblage direct
  dans `IdeShell.vue`) le lie au singleton `useIdeSession().activeConversationId`, qui
  reste consommé par `IdeShell.vue` (colonne assistant) et `MenuBar.vue`. Côté VS Code
  (F3), une ref locale.
- `endAgentSession` (bouton « × ») = simple remise à `null` du `v-model`, pas d'appel
  backend — reste tel quel.
- `inject('notify-fs-change')` dans `ChatSession.vue` : conservé en `inject<() => void>(
  'notify-fs-change', () => {})` avec défaut no-op (pattern déjà utilisé par `Explorer`/
  `GitPanel`/`Editor`). Non fourni en VS Code → no-op. Pas un élément du port.

## Modules touchés

| Module | Changement |
|---|---|
| `package.json` (racine) | `workspaces: ["frontend", "packages/*"]` |
| `lib/Cargo.toml`, `lib/src/event.rs` | feature `ts-rs`, derives gated |
| `packages/protocol/src/config-domain.ts` | **nouveau** — 6 types miroir de `lib/src/domain.rs` + fixtures de conformité |
| `.github/workflows/test.yml` | ordre de build workspaces (packages avant frontend) + job vérif ts-rs |
| `frontend/src/components/settings/*Screen.vue` (×6 + `AccountScreen`) | déplacés dans `@vanyline/ui/config/` (+ specs) ; types locaux → import de `@vanyline/protocol` ; FK `provider_id`/`model_profile_id` → `provider`/`model` (nom) ; états keyés id → keyés `name` ; skill `body` via `repo.get` ; `setDefault` → `repo.setDefaultProvider` ; `createApiClient` → `inject('vanyline.configRepo')` |
| `frontend/src/components/panels/Chat.vue` → `ChatWindow.vue`, `ChatSession.vue` | → `@vanyline/ui` ; `frontend` fournit `provide(VanylineChatTransport)` + `provide('vanyline.chatBackend', httpChatBackend)` et lie le `v-model:activeConversationId` au singleton `useIdeSession` |
| `frontend/src/api/httpChatBackend.ts` | **nouveau** — impl HTTP de `ChatBackend` ; `createConversation` absorbe la logique de `useIdeSession.startAgentSession` (résolution 1ᵉʳ agent + contexte sandbox) |
| `frontend/src/composables/useIdeSession.ts` | `startAgentSession`/`endAgentSession` : la création de conversation migre dans `httpChatBackend` ; le composable ne garde que l'état `activeConversationId`/`startingSession`/`sessionError` et `registerIdeActions` |
| `frontend/src/composables/useCrudResource.ts` | **inchangé** (dashboards Projects/Sandboxes) ; les écrans config passent au nouveau `@vanyline/ui/composables/useCrudResource.ts` |
| `frontend/src/components/SettingsView.vue` | monte `ConfigShell` de `@vanyline/ui`, `groups` = `ConfigNavGroup[]`, `provide('vanyline.configRepo', httpConfigRepo())` ; `navState` re-câblé sur l'event `nav-change` |
| `frontend/src/api/httpConfigRepo.ts` | **existe** (tâche 4) — rework : traduit `provider_id`/`model_profile_id` ↔ nom (les 2 sens) via listing en cache ; `setDefaultProvider` → endpoint `/default` ; `availableModels`/`isDefault`/`availableTools` mappés depuis la réponse REST |
| `frontend/src/api/chatTransport.ts` | réduit à l'ouverture WS + `chatEventsToUIStream` |
| `frontend/vite.config.ts`, `tsconfig*.json` | alias `@vanyline/ui`, `@vanyline/protocol` |
| specs des 6 écrans + `Chat.spec.ts` + `useCrudResource.spec.ts` | `id: number` → `name` ; les écrans mockent un `ConfigRepo` (plus `createApiClient`) ; `Chat.spec.ts` mocke un `ChatBackend` |
| `frontend/src/api/httpConfigRepo.spec.ts` | **nouveau** — round-trips par domaine + traduction FK↔nom dans les 2 sens + 403 RBAC propagé |

## Sécurité (argv / URL / chemin)

Aucune surface nouvelle en F1 : REST HTTP existant + génération de types au build. La
contrainte « télécharger et exécuter un binaire » est portée par F3 ; la validation
anti-traversal des `name` de config est portée par F2.

## Risques et questions ouvertes

- **ts-rs sur enum serde taggée** : `ChatEvent` est `#[serde(tag = "type")]` snake_case.
  Risque connu depuis le doc WS-6 d'origine. Tâche 1 : essayer, sinon basculer sur le
  repli (types manuels + fixtures de conformité générées par la lib). Décision prise en
  tâche 1, pas avant.
- **`@nuxt/ui` v4 hors contexte Nuxt, dans un package lib** : déjà le cas dans
  `frontend/` (Tailwind global depuis `chat-app-fonctionnel`), mais l'extraction en
  package peut casser la résolution Tailwind/CSS. À valider tôt.
- **Poids de `@vanyline/ui`** : Element Plus + Nuxt UI tirés ensemble. Prévoir des
  entrypoints séparés (`@vanyline/ui/chat` vs `@vanyline/ui/config`) si F3 mesure un
  `.vsix` rédhibitoire — anticipable ici par des exports conditionnels propres.
- **Refactor `useCrudResource`** : 6 call sites + spec, à faire d'un bloc. Le pattern
  dépagination (`PagedResult`) déjà dans le composable reste côté impl HTTP.
- **Course `name→id`** (HTTP) : deux onglets qui renomment en parallèle. Acceptable
  (usage solo), documenté.
- **`ext` dans `workspaces`** : ajouté en F3 seulement (pas de dossier vide entre-temps).

## Découpage en tâches (état réel)

Faites et commitées (`.tasks/F1-vscode-ext-foundations/`) :

1. ✅ `packages/protocol` scaffold + ts-rs sur `ChatEvent` + `RpcConnection` + tests.
2. ✅ `packages/ui` scaffold + `common/` + config Vitest/tsconfig + alias `frontend/`.
3. ✅ `ConfigShell` + `config-nav`.
4. ✅ Extraction chat : `ChatBackend`/`ChatWindow`/`ChatSession`/`chatEventsToUIStream`,
   `httpChatBackend`, `useIdeSession` allégé, `frontend` re-câblé.
5. ✅ `ConfigRepo` (`ports.ts`, forme opaque `{ name, [k]: unknown }`) + `httpConfigRepo`
   v1 (résolution name→id **un seul sens**, pas de traduction FK, pas de
   `setDefaultProvider`).

Reste (candidate 5 + 6 du design d'origine, redécoupées après les blocages tranchés) :

6. **Plumbing config** : `config-domain.ts` (6 types miroir `domain.rs` + `available*`/
   `isDefault`, + fixtures de conformité) dans `@vanyline/protocol` ; `ports.ts` →
   union discriminée par domaine + `setDefaultProvider` ; `httpConfigRepo` rework
   (traduction FK↔nom **les 2 sens**, `setDefaultProvider`, `available*` mappés) +
   `httpConfigRepo.spec.ts`. **Aucun écran touché — tout reste vert.**
7. **`useCrudResource` (ui) + `SettingsView`→`ConfigShell` + extraction Account +
   Skills + LlmProviders** dans `@vanyline/ui/config/` (+ specs). `SettingsView` monte
   ConfigShell avec ces 3 écrans depuis le package + les 3 autres encore locaux
   (fonctionnels, `createApiClient`). `provide('vanyline.configRepo', httpConfigRepo())`.
8. **Extraction McpServers + ModelProfiles** dans `@vanyline/ui/config/` (+ specs).
   `SettingsView` bascule ces 2 sur le package.
9. **Extraction Toolsets + Agents** dans `@vanyline/ui/config/` (+ specs). `SettingsView`
   n'importe plus que depuis `@vanyline/ui` ; suppression des fichiers écrans locaux.
10. **CI** : ordre de build workspaces (packages avant frontend) + job de vérification
    ts-rs (`git diff --exit-code`).
