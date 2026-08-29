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
│   │       └── connection.ts              client ndjson (corrélation id→promesse)
│   └── ui/                 @vanyline/ui — Vue 3, agnostique du backend
│       └── src/
│           ├── chat/       ChatWindow.vue, ChatSession.vue, chatEventsToUIStream.ts
│           ├── config/     6 écrans + ConfigShell.vue
│           ├── common/     Field, DialogShell, ErrorCard, EmptyState, LoadingSkeleton, CheckboxList
│           ├── composables/ useCrudResource.ts (générique sur ConfigRepo)
│           └── ports.ts    interfaces ConfigRepo + ré-export du ChatTransport AI SDK
└── frontend/               → dépend de @vanyline/ui + @vanyline/protocol
    └── src/api/
        ├── httpConfigRepo.ts   impl HTTP de ConfigRepo (name→id interne, jamais exposé)
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
- `RpcConnection` : client ndjson **transport-injecté** — prend `{ write(line: string):
  void; onLine(cb: (line: string) => void): void }`, ne dépend d'aucune API Node. Gère
  corrélation `id → Promise`, dispatch des notifications par méthode, timeouts.

### `ConfigRepo` (le contrat central, `@vanyline/ui/ports.ts`)

Name-keyed. Une seule interface, deux implémentations (HTTP en F1, RPC en F4).

```ts
type ConfigDomain = 'providers' | 'profiles' | 'mcp' | 'toolsets' | 'agents' | 'skills';

interface ConfigRepo {
  list(domain: ConfigDomain): Promise<ConfigItem[]>;
  get(domain: ConfigDomain, name: string): Promise<ConfigItem>;
  create(domain: ConfigDomain, item: ConfigItem): Promise<ConfigItem>;
  update(domain: ConfigDomain, name: string, patch: Partial<ConfigItem>): Promise<ConfigItem>;
  remove(domain: ConfigDomain, name: string): Promise<void>;
  testProvider(name: string): Promise<{ models: string[] }>;
  testMcpServer(name: string): Promise<{ tools: string[] }>;
  listLocalTools(): Promise<string[]>;
}
```

- Fourni aux écrans par `provide`/`inject` (clé `vanyline.configRepo`).
- Le domaine s'appelle `profiles` côté UI (côté CLI c'est `models`, côté app
  `model-profiles`) — le mapping de nom vit dans chaque impl, jamais dans les écrans.
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

## Modules touchés

| Module | Changement |
|---|---|
| `package.json` (racine) | `workspaces: ["frontend", "packages/*"]` |
| `lib/Cargo.toml`, `lib/src/event.rs` | feature `ts-rs`, derives gated |
| `.github/workflows/test.yml` | ordre de build workspaces (packages avant frontend) + job vérif ts-rs |
| `frontend/src/components/settings/*Screen.vue` (×6) | déplacés dans `@vanyline/ui`, ré-exportés |
| `frontend/src/components/SettingsView.vue` | → `ConfigShell` de `@vanyline/ui` + `provide(httpConfigRepo)` |
| `frontend/src/components/panels/Chat.vue`, `ChatSession.vue` | → `@vanyline/ui` + `provide(VanylineChatTransport)` |
| `frontend/src/composables/useCrudResource.ts` | signature : `(repo: ConfigRepo, domain)` au lieu de `(client, basePath)` |
| `frontend/src/api/httpConfigRepo.ts` | **nouveau** — impl HTTP, résolution `name→id` via listing en cache |
| `frontend/src/api/chatTransport.ts` | réduit à l'ouverture WS + `chatEventsToUIStream` |
| `frontend/vite.config.ts`, `tsconfig*.json` | alias `@vanyline/ui`, `@vanyline/protocol` |
| specs des 6 écrans + `Chat.spec.ts` + `useCrudResource.spec.ts` | `id: number` → `name` |

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

## Découpage en tâches candidates

1. `packages/protocol` : scaffold + ts-rs sur `ChatEvent` (ou repli) + `RpcConnection` + tests vitest sur transport mocké.
2. `packages/ui` : scaffold + migration `common/` + `ConfigShell` + config Vitest/tsconfig, alias côté `frontend/`.
3. Extraction chat : `ChatWindow`/`ChatSession`/`chatEventsToUIStream`, `frontend` re-câblé, specs.
4. `ConfigRepo` (`ports.ts`) + `httpConfigRepo` + tests.
5. Extraction des 6 écrans + `useCrudResource` générique + `frontend` re-câblé + specs.
6. CI : ordre de build workspaces + job de vérification ts-rs.
