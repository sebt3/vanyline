# F1 — vscode-ext-foundations (close 2026-08-31)

Première des 5 features « extension VS Code `vanyline` » (cf.
[[vscode-ext-sequence]]). **Aucune ligne d'extension** ici : F1 extrait du frontend web
deux packages partagés que l'extension (F3+) consommera.

Branche `feat/F1-vscode-ext-foundations` — **pas encore poussée ni mergée** à la clôture.

## Ce qui a été livré

### `@vanyline/protocol` (TS pur, zéro dépendance UI)

- `generated/chat-event.ts` — `ChatEvent` **généré par ts-rs** depuis `vanyline-lib::event`
  (feature `ts-rs` v12 gated, `TS_RS_LARGE_INT="number"` dans `.cargo/config.toml`). Le
  repli « types manuels » prévu au design n'a **pas** été nécessaire.
- `rpc.ts` — enveloppes JSON-RPC (miroir de `cli/src/rpc/protocol.rs`). `connection.ts` —
  `RpcConnection`, client ndjson **transport-injecté** (`{ write, onLine }`), aucune API
  Node. *(Ces deux-là ne sont pas encore consommés — c'est F2/F4.)*
- `config-domain.ts` — miroir TS **manuel** des 6 structs serde de `lib/src/domain.rs`
  (`Provider`, `ModelProfile`, `McpServer`, `Toolset`, `Agent`, `SkillMeta`/`SkillDetail`).
  Forme = serde à la lettre : discriminant `type`, `snake_case`, **name-keyed**. +3 champs
  web-augmentés optionnels (`Provider.available_models`/`is_default`,
  `McpServer.available_tools`). Conformité des deux côtés (`*.conformance.spec.ts` +
  `domain.rs` tests `*_wire_shape` qui figent les clés JSON).

### `@vanyline/ui` (Vue 3, agnostique du backend)

Trois ports injectés (`provide`/`inject`) :

| Port | Clé | Impl web |
|---|---|---|
| `ChatTransport<UIMessage>` (AI SDK) | `vanyline.chatTransport` | `VanylineChatTransport` |
| `ChatBackend` | `vanyline.chatBackend` | `httpChatBackend` |
| `ConfigRepo` | `CONFIG_REPO_KEY` = `vanyline.configRepo` | `httpConfigRepo` |

- Chat : `ChatWindow.vue` (sélecteur + new/close + hôte) + `ChatSession.vue` (streaming
  `useChat`) + `chatEventsToUIStream` (le mapper `ChatEvent → UIMessageChunk` extrait de
  l'ancien `chatTransport.ts`). `activeConversationId` en `v-model`. `notify-fs-change` =
  `inject` optionnel no-op (web-only).
- Config : les **6 écrans** + `ConfigShell.vue` (coquille de nav Settings) + `common/`
  (Field/DialogShell/…/CheckboxList) + `useCrudResource(repo, domain)` + `useConfigRepo()`.

### `frontend/`

- `httpConfigRepo` porte **toute** la traduction wire REST `app` ↔ forme canonique :
  `provider_type`/`server_type` ↔ `type`, FK `provider_id`/`model_profile_id` ↔ **nom**
  (cache `name↔id` **par instance**). `get('skills')` → `GET /{id}` pour le `body`.
  API REST de `app` **inchangée**, zéro migration.
- `httpChatBackend`, `VanylineChatTransport` (WS + délégation au mapper).
- `SettingsView.vue` : ~20 lignes, monte `ConfigShell`. `useIdeSession` allégé (-48 l,
  `startAgentSession`/`endAgentSession` → `httpChatBackend`). `MenuBar` re-câblé.
- **Restent locaux** : `AccountScreen` (pas de compte hors web), `components/common/` +
  `composables/useCrudResource.ts` `(client, basePath)` — consommés par les dashboards
  Projects/Sandboxes, hors périmètre.

### CI

Job « Frontend » → « Frontend + packages » : `check`+`test` de `@vanyline/protocol` puis
`@vanyline/ui` **avant** le build `frontend` (les packages n'étaient pas du tout en CI).
Nouveau job `tsrs` : régénère `generated/` + `git diff --exit-code`.

## Décisions structurantes (prises en session, ne pas re-litiguer)

1. **Extraction chat = port `ChatBackend`** (pas un simple split). `Chat.vue`/`ChatSession`
   étaient couplés au backend au-delà du transport (liste/création/historique de
   conversation). Contrat symétrique de `ConfigRepo`. `createConversation()` sans
   paramètre — la politique de contexte (sandbox + 1ᵉʳ agent côté web ; agent du YAML
   côté CLI) vit dans l'impl, jamais dans les composants. Blocage Phase 2 #1.
2. **Références inter-domaines = toujours des noms côté UI.** Le modèle canonique
   (`domain.rs`, wire RPC) est **déjà** 100 % name-keyed ; seule la couche REST de `app`
   a des PK `i32` + 2 FK numériques. `httpConfigRepo` traduit dans les deux sens ; l'impl
   RPC (F4) sera un pass-through. Blocage Phase 2 #2.
3. **`config-domain.ts` = forme `domain.rs` à la lettre** (pas la forme REST) : le wire
   RPC de F2 est la référence, `httpConfigRepo` absorbe l'écart. Types dans
   `@vanyline/protocol` (pas `@vanyline/ui`) car ce sont les payloads du RPC config.
4. **`useCrudResource` dupliqué** : le composable de `frontend/` reste `(client, basePath)`
   pour les dashboards ; les écrans config passent à un **nouveau** `@vanyline/ui`
   `(repo: ConfigRepo, domain)`. Pas de fusion — les dashboards ne sont pas des domaines
   `ConfigRepo`.
5. **MCP `sse` conservé.** Premier réflexe (moi) : le retirer car « pas implémenté côté
   sandbox ». **Correction du développeur** : les MCP se connectent au **moteur
   d'inférence** (qui gère SSE), pas à la sandbox. `McpTransport = 'sse' | 'http-streamable'`
   dans `config-domain.ts` ; `domain.rs::McpTransport` n'a que `HttpStreamable` → **`Sse` à
   ajouter en F2**. Divergence TS↔Rust assumée et documentée.
6. `AccountScreen` reste dans `frontend/` (F4 n'aura pas d'écran compte).

## Bilan de délégation

- **Tâches 01-05** (protocol scaffold, ts-rs, `RpcConnection`, `packages/ui` +
  `common/`, `ConfigShell`, extraction chat, `ConfigRepo` v1) : faites par **Cadence**
  (DeepSeek) avant cette session, commitées. Review Phase 3 : propres.
- **Tâche 06** (plumbing config : `config-domain.ts` + `ports` typé + `httpConfigRepo`
  bidirectionnel) : déléguée à **Qwen `implement`** via `llm-exec`. **Échec** — timeout
  (exit 124) à ~70 %, arbre laissé cassé (12 tests rouges). Bugs : caches en portée
  module (isolation de tests cassée), `toRest` async non attendu, `toRestProvider` qui
  perdait `type`, `list` sans préchargement du cache de référence, assertion Rust
  `toolset_wire_shape` fausse. **Fini par Claude** (réécriture propre de `httpConfigRepo`
  + sa spec).
  → Diagnostic : tâche à trop forte composante « jugement » (traduction bidirectionnelle
  subtile, isolation de tests) pour Qwen. Même famille que le constat
  `outillage-llm-exec` / [[git-integration]].
- **Tâches 07-10** (extraction des 6 écrans + specs + CI) : faites **directement par
  Claude** — choix explicite du développeur après le raté sur 06, les specs demandant
  une réécriture `fetch` mocké → `ConfigRepo` mocké (comprendre l'intention de chaque
  test, pas du remplissage de squelette).

## Review Phase 3 (Claude)

- Validation : `protocol` 19 · `ui` 82 · `frontend` 306 + build · `cargo test --workspace`
  (18 suites) · `fmt` · `clippy -D warnings` — **tout vert**.
- Risques du design vérifiés un par un : ts-rs OK (pas de repli) ; `@nuxt/ui` hors Nuxt
  dans un package OK (builds verts) ; poids `@vanyline/ui` moins lourd que craint
  (**pas** d'Element Plus dedans — reste frontend-only) ; course `name→id` acceptée
  (usage solo) ; sécurité : aucune surface nouvelle (les `name` passent par un lookup de
  cache, jamais interpolés dans une URL ; ids = nombres backend).
- Chat extraction (Cadence) : `chatEventsToUIStream` fidèle et bien testé (spec 202 l.),
  `cancel()` libère bien la source WS (`fix` 20384be). `MenuBar`/`useIdeSession`
  re-câblés proprement.
- **Nits non bloquants** : `chatBackend!`/`transport!` (assertions non-null) vs le pattern
  `useConfigRepo()` avec message d'erreur — incohérent, un `useChatBackend()` serait
  mieux ; pas de constantes exportées pour `'vanyline.chatBackend'`/`.chatTransport` (≠
  `CONFIG_REPO_KEY`) ; créer un `ModelProfile` sans provider → `VNL-CFG-404` de `idOf`
  plutôt qu'une validation de champ (message un peu sec) ; `list('skills')` sur-récupère
  le `body` (comportement REST pré-existant). Aucun ne justifiait un commit correctif.

## Pour F2 (`F2-vscode-ext-cli-rpc`)

- **Ajouter `Sse` à `lib/src/domain.rs::McpTransport`** (`#[serde(rename_all = "kebab-case")]
  Sse,`) + le `match` dans `lib/src/prefixed_mcp.rs:229` (impl réelle ou `Err(VNL-MCP-004)`).
- `config/<domain>/*` RPC doit sérialiser exactement les structs `domain.rs` — c'est ce
  que `config-domain.ts` copie, la conformance croisée est en place des deux côtés.
- Nom de domaine : `profiles` (UI) = `models` (CLI) = `model-profiles` (app REST).
- `AGENTS.md` : sa « Structure des répertoires » et sa table de stack ne mentionnent pas
  encore `packages/` — à compléter (signalé au développeur, pas fait sans accord).
