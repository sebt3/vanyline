# miryad-core-integration

## Ce que ça fait

Remplace la couche auth/session + accès BDD + API CRUD de `app` par `miryad-core` : OIDC/session
cookie/tokens API, users/groupes/RBAC, routeurs REST/GraphQL/MCP génériques construits sur le
trait `MiryadResource`. Bascule complète en une fois (décision développeur, 2026-08-25) : pas de
cohabitation ancien/nouveau, pas de chemin de migration progressif.

## Ce que ça ne fait pas

- **Ne touche pas** `sandbox/` ni `controller/` — aucune dépendance miryad-core de ce côté, les
  CRDs `Owner`/`Project`/`Sandbox`/`Application` restent gérées telles quelles par le controller.
- **Ne résout pas le workflow/DAG** — la feature 9 de miryad-core (reprise du moteur workflow)
  reste en standby ; vanyline en hérite quand elle sera livrée, pas avant.
- **Ne rescaffold pas le frontend automatiquement** — le générateur TypeScript du template
  `miryad` (IR → écrans Vue) reste hors scope. Le frontend Vue existant de vanyline est adapté à
  la main aux nouvelles routes/formats de réponse, pas régénéré.
- **Ne change pas** le mécanisme de ticket WS court-vécu frontend→sandbox (`POST
  /api/sandboxes/{name}/ws-ticket`) — il continue d'exister à côté, cf. question ouverte plus bas
  sur ce qui l'alimente une fois l'auth portée par miryad-core.

## Entités concernées

Mapping vers `MiryadResource`, RBAC `OwnerOnly` (propriétaire = utilisateur courant) :
`ModelProfile`, `Toolset`, `Skill`, `AgentRecord`, `Conversation`.

Mapping `read_policy() = Public`, `write_policy() = AdminOnly`, `owner_column() = None` (référentiel
partagé, pas de notion de propriétaire) : `LlmProvider`, `McpServer` — tout utilisateur voit la
liste pour choisir un modèle/toolset, seul un admin configure endpoint/clé API.

Cas particuliers :
- **`User`** — remplacé par le `User` de miryad-core (`miryad_users` : id, subject, email,
  display_name, created_at — **schéma fixe**, vérifié dans `src/users/user.rs`, aucun mécanisme
  d'extension). L'actuel `users.k8s_owner_name` (lien vers l'Owner CRD K8s, résolu par
  `api/owners.rs::ensure_owner`) vit donc dans une **table séparée côté `app`**
  (ex: `owner_links`, FK vers `miryad_users.id`), pas une colonne ajoutée au schéma miryad-core.
- **`ChatContext`** — polymorphe (`kind`/`data`), pas un `OwnerOnly` simple : pas de propriétaire
  direct, la portée owner vient de la ressource référencée (ex: sandbox). À modéliser à part.
- **`Message`** — pas de propriété dérivée de `conversation_id` : `owner_id` posé directement
  depuis le principal courant à la création, comme n'importe quel `OwnerOnly` (`before_create` de
  miryad-core n'a pas accès à la BDD, cf. `src/resource.rs` — pas de vérification possible que
  `conversation_id` appartient au même propriétaire). Limite acceptée sciemment, même choix que
  celui documenté côté `miryad` sur `RecipeIngredient.owner_id` (décision développeur, 2026-08-25).
  `ChatContext` suit le même principe si un `owner_id` s'avère nécessaire pour lui.

**Hors `MiryadResource`** : `Owner`/`Project`/`Sandbox` restent des CRDs K8s (pas des tables
Postgres) — les endpoints qui les manipulent (`api/owners.rs`, `api/projects.rs`,
`api/sandboxes.rs`) restent des handlers axum custom, en dehors du routeur générique.

## Interfaces clés / modules touchés

- `app/src/auth/*` → remplacé par `miryad_core::auth` (routeur `auth_router()`, extracteur
  principal, dual-auth cookie/token).
- `app/src/db/*` → remplacé par des entités SeaORM (une par table ci-dessus) + migrations
  dédiées (table de suivi `seaql_migrations_app`, cf. précédent côté `miryad` pour éviter la
  collision avec la table de miryad-core).
- `app/src/api/{llm_providers,model_profiles,mcp_servers,toolsets,skills,agents,conversations}.rs`
  → remplacés par des implémentations `MiryadResource` + montage `resource_router()`.
- `app/src/api/{owners,projects,sandboxes}.rs` → conservés tels quels (handlers custom), mais
  rebranchés sur l'extracteur de principal de miryad-core au lieu du middleware auth maison.
- Routes REST versionnées (`/api/v1/{resource}`, préfixes figés par miryad-core depuis 0.1.1) —
  le frontend doit s'adapter aux nouveaux chemins et à la forme de réponse générique.

## Risques identifiés et décisions (résolus le 2026-08-25)

1. **Extension du schéma `User`** — schéma fixe confirmé (`src/users/user.rs`). `k8s_owner_name`
   vit dans une table séparée côté `app`, FK vers `miryad_users.id` (cf. "Entités concernées").
2. **`Message`/`ChatContext`, ownership imbriquée** — limite acceptée sciemment (même choix que
   `RecipeIngredient.owner_id` côté `miryad`) : `owner_id` posé directement à la création, pas de
   vérification croisée via `conversation_id` (`before_create` n'a pas accès DB, de toute façon).
3. **Données existantes** — aucune instance vanyline en prod ; le déploiement `media-test` cité
   (`.claude/memory/v0.1.1-first-live-deploy.md`) est l'environnement de test, 100% jetable. Pas de
   contrainte de migration de données.
4. **RBAC `LlmProvider`/`McpServer`** — lecture `Public`, écriture `AdminOnly`, `owner_column() =
   None` (référentiel partagé, cf. "Entités concernées").
5. **Logout RP-initiated** — pas un besoin ici, contrairement à `miryad`. Rien à traiter.

## Question ouverte restante

- **Auth du ticket WS frontend→sandbox** — `app` mine aujourd'hui ce ticket en présentant le
  `id_token` OIDC de l'utilisateur. Confirmé dans le code : `AuthPrincipal::PrincipalSource::
  Session { id_token }` (`src/auth/principal.rs`) porte exactement cette valeur pour le flow
  session cookie — à utiliser telle quelle une fois `app` basé sur le principal miryad-core. Pas
  de vérification supplémentaire jugée nécessaire à ce stade (à confirmer en écrivant la tâche
  concernée, si un détail de câblage surprend).

## Décisions complémentaires (blocage Cadence en Phase 2, 2026-08-25)

Remontées après les 4 premières tâches (socle, rebranchement, Skill, Toolset) — le pattern
`vanyline_<entité>` + `MiryadResource` + `resource_router` est posé et validé, mais les entités
restantes soulèvent des cas que ce pattern seul ne couvre pas.

**a) Endpoints métier non-CRUD** (`POST /llm-providers/{id}/test`, `PUT
/llm-providers/{id}/default`, `POST /mcp-servers/{id}/test`) — conservés en **handlers axum
custom**, à côté de `resource_router`, même traitement que `owners`/`projects`/`sandboxes`. RBAC
vérifié à la main dans le handler via les helpers publics de miryad-core
(`miryad_core::rbac::can_write::<Entity>(db, &user)`/`can_read`, `src/rbac.rs`) — pas de logique
d'accès réinventée côté `app`.

**b) Ordre de remplacement et modèle de référence inter-ressources** — ids `i32` partout (précédent
déjà posé par Skill/Toolset), pas de validation croisée de FK à la création (`before_create` n'a
pas accès DB, cf. décision existante sur `Message`/`RecipeIngredient`) :

1. `LlmProvider` — read `Public`, write `AdminOnly`, `owner_column() = None` + handlers custom
   `test_provider`/`set_default_provider`.
2. `McpServer` — même politique + handler custom `test_server`.
3. `ModelProfile` — `OwnerOnly`, FK `provider_id: i32` → `LlmProvider`, pas de validation croisée.
4. `AgentRecord` — `OwnerOnly`, FK `model_profile_id: i32` → `ModelProfile`. `toolsets`/`skills`
   restent des tableaux JSON de **noms** (pas d'id) — `vanyline_lib`/`PgConfigStore` les résolvait
   déjà par nom, aucune migration nécessaire sur ces deux champs.
5. `ChatContext`/`Conversation`/`Message` — cf. (c).
6. `config_store.rs` (`PgConfigStore`) — réécrit contre les entités SeaORM une fois 1-5 livrées :
   lit `LlmProvider`/`ModelProfile`/`McpServer`/`Toolset`/`Skill`/`AgentRecord` ensemble via
   `ConfigStore` (trait `vanyline_lib`), ne peut pas être fait avant qu'elles existent toutes.
   Découverte non listée dans le design initial — corrigée ici.
7. Retrait final de `app/src/db/models.rs` et des tables sqlx devenues mortes.

**c) `ChatContext`/`Conversation`/`Message` : pas de `MiryadResource`** — en relisant
`conversations.rs`, ces trois entités n'entrent pas dans le moule CRUD générique :
`create_conversation` a un effet de bord (crée un `ChatContext` avant le `Conversation`), résout
`agent_name` → id côté serveur, `list_conversations` filtre par jointure JSON (au-delà de
`filter_column()`, égalité simple sur une seule colonne), `get_messages` est une sous-route dédiée
(`GET /conversations/{id}/messages`). Décision : **entités SeaORM, mais pas de `MiryadResource` ni
`resource_router`** — handlers axum custom réécrits contre SeaORM (même construction que
`owners.rs`), RBAC vérifié à la main (`conv.owner_id != principal.id` → 403, comme aujourd'hui).
Schéma inchangé : `ChatContext` sans colonne owner (jamais vérifiée directement, la portée vient de
la ressource référencée), `Conversation`/`Message` avec `owner_id: i32` posé à la création.

## Revue Phase 3 (2026-08-25/27) — bugs trouvés et corrigés par Claude

Phase 2 (Cadence) livrée : 17 commits, `cargo test`/`clippy`/`fmt` et `npm run build`/`test` tous
verts. La revue de clôture (`/code-review`, 8 angles + vérification manuelle) a montré que
"tout vert" ne suffisait pas — aucun test ne faisait un vrai aller-retour HTTP+DB avec les payloads
réels du frontend. Deux bugs rendaient la feature non fonctionnelle malgré des tests au vert ;
un troisième, plus profond, aurait cassé le déploiement même sur Postgres réel.

**Critiques (corrigés)** :
- Inserts manuels (`ws/chat.rs::persist_message`, `conversations.rs::create_conversation`,
  `owners.rs::ensure_owner`) posaient `id: Set(0)` au lieu de `NotSet` — la 2ᵉ conversation/le 2ᵉ
  message créés système-wide percutaient la contrainte de clé primaire. Chat cassé dès le 2ᵉ tour.
- `resource_router` désérialise le body en `Model` tel quel ; `id`/`owner_id` sans
  `#[serde(default)]` alors que le frontend ne les envoie jamais → 400 sur toute
  création/modification via le CRUD généré (Agents/ModelProfiles/Skills/Toolsets/
  LlmProviders/McpServers). Vérifié empiriquement (désérialisation du body réel envoyé par
  `AgentsScreen.vue`).
- **Découvert en creusant le premier bug** (pas dans la liste initiale) : plusieurs migrations
  chaînaient un index non-UNIQUE en `.index(...)` directement sur `Table::create()` — valide
  seulement avec `.unique()` (contrainte de table réelle) ; sans lui, sea-query génère un fragment
  `(...)` orphelin, syntaxiquement invalide **aussi bien contre Postgres que SQLite** (vérifié dans
  les deux dialectes). `chat_contexts`/`conversations`/`messages` corrigées : index simple via
  `manager.create_index(...)`, instruction séparée.

**Hauts (corrigés)** : validation `provider_type`/`server_type`/`mode` (CHECK + validation
handler perdues à la bascule) restaurée via `before_create` (pur, sans accès DB) sur les 3
entités concernées. Gap distinct accepté : existence des noms `toolsets`/`skills` sur `Agent`
(nécessite un accès DB que `before_create` n'a pas — même limite que la validation croisée par id
déjà actée plus haut).

**Moyens (corrigés)** : préfixe incohérent des endpoints custom `llm-providers`/`mcp-servers`
(`test`/`default`) — déplacés de `/api` vers `/api/v1`, au même préfixe que leur CRUD généré ;
pagination frontend (`useCrudResource.fetchAll`) qui tronquait silencieusement au-delà de la
première page de `PagedResult` — corrigé pour boucler sur `total_pages` ; index composite
`messages(conversation_id, created_at)` restauré (perdu à la bascule, requis par `get_messages`).

**Bas (corrigés)** : `set_default_provider` remplacé (fetch-all + boucle par ligne + 2 re-fetches
morts) par deux `UPDATE` bulk ; helper `fn db_err` dupliqué dans 9 fichiers remplacé par un seul
`impl From<sea_orm::DbErr> for AppError` (`error.rs`) — même comportement (`VNL-DB-006`), un seul
endroit à maintenir.

**Faux positifs écartés** (remontés par plusieurs angles indépendamment, vérifiés et neutralisés) :
l'absence de filtre `owner_id` sur `LlmProvider`/`McpServer` dans `config_store.rs` est voulue
(ressources `Public`/`AdminOnly` sans propriétaire, décidée en Phase 1) ; les invariants auth
(expiration JWT, CSRF state, `HttpOnly`/`SameSite=Lax`) sont bien à parité côté miryad-core
(vérifié dans son code source).

Nouveaux tests de régression permanents : `db::test_support::real_db()` (sqlite en mémoire, les
deux migrateurs appliqués — nécessite `sqlx-sqlite` en `[dev-dependencies]` de `app/Cargo.toml`,
jamais en production) ; `migration::migrations_apply_cleanly` (le jeu complet de migrations
s'applique contre un vrai schéma) ; régressions ciblées sur `agents`/`conversations`/
`llm_providers` couvrant chacun des bugs critiques ci-dessus.

## Statut

Phase 1 close (2026-08-25) — accord explicite des deux parties (développeur + Claude) sur le
périmètre, le mapping d'entités et les risques ci-dessus. Décisions complémentaires tranchées le
même jour suite au blocage remonté par Cadence en Phase 2. Phase 2 livrée par Cadence, Phase 3
(revue + corrections) faite par Claude directement à la demande du développeur — `cargo
check/test/clippy/fmt` et `npm run build/test` tous verts après corrections. Prêt pour merge, sous
réserve de validation développeur.
