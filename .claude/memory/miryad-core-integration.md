---
name: miryad-core-integration
description: Bascule de app sur miryad-core (auth/RBAC/CRUD générique) — décisions Phase 1, blocage Cadence Phase 2, bugs critiques trouvés et corrigés en revue Phase 3, merge avec git-integration (2026-08-25 → 08-27)
metadata:
  type: project
---

# Bascule de `app` sur miryad-core (2026-08-25 → 08-27)

## Ce qui a été livré

Design initial (`docs/features/miryad-core-integration.md`, supprimé à la clôture —
contenu migré dans `docs/architecture.md`, section "Backend web — `vanyline-app`") :
remplacement complet, en une fois, de la couche auth/session/BDD/CRUD maison de `app`
par `miryad-core` (crate publique crates.io, moteur générique derrière le template
`miryad`, cf. `$HOME/projets/miryad-core`). sqlx → SeaORM, `Uuid` → `i32`
auto-incrémenté, auth OIDC/cookie maison → `miryad_core::auth`, six entités
(`LlmProvider`/`McpServer`/`ModelProfile`/`Toolset`/`Skill`/`AgentRecord`) sur le CRUD
générique `resource_router`/`MiryadResource`, le reste (`Owner`/`Project`/`Sandbox` —
CRDs K8s — et `Conversation`/`Message`/`ChatContext` — logique custom) en handlers
axum maison rebranchés sur SeaORM.

## Origine et décision

Projet personnel séparé du développeur (`miryad-core`, distinct de vanyline),
initialement écarté comme "pas encore assez mûr, vanyline serait le premier vrai
consommateur" — objection levée quand le développeur a fini le template `miryad` en
parallèle (10aine de bugs trouvés/corrigés en conditions réelles : OIDC Authentik,
CNPG, RBAC testé avec de vrais 403, GraphQL avec traversée de relations, 0.1.0→0.1.3).
Discussion Phase 1 (développeur + Claude) a tranché plusieurs points avant tout code :
schéma `User` de miryad-core fixe (pas d'extension) → table séparée côté `app`
(`vanyline_owner_links`) plutôt qu'une colonne ajoutée ; ownership imbriquée
(`Message`→`Conversation`) acceptée sans validation croisée (`before_create` n'a pas
accès BDD) ; `LlmProvider`/`McpServer` requalifiés en ressources globales
`Public`/`AdminOnly` (rupture avec le MVP initial, par-utilisateur) après une question
directe du développeur, pas une hypothèse de Claude.

## Blocage Cadence en Phase 2 — ambiguïtés non couvertes par le design initial

Après 4 tâches livrées (socle, rebranchement, Skill, Toolset), Cadence s'est arrêté
sur trois questions que le design ne tranchait pas : (a) devenir des endpoints
non-CRUD (`test`/`default` sur LlmProvider/McpServer) — résolu : handlers custom à
côté de `resource_router`, RBAC via les helpers publics `miryad_core::rbac::can_read`/
`can_write` plutôt que réinventé ; (b) ordre de remplacement et modèle de référence
inter-ressources — résolu : LlmProvider/McpServer → ModelProfile → AgentRecord →
Conversation/Message/ChatContext → `config_store.rs` en dernier (découverte non listée
au design initial : lit les 6 tables ensemble, ne peut être réécrit qu'une fois toutes
migrées) ; (c) modèle de `ChatContext`/`Conversation`/`Message` — résolu après lecture
du code réel (`create_conversation` a un effet de bord, résout un nom côté serveur,
filtre par jointure JSON, sous-route dédiée) : entités SeaORM mais **pas**
`MiryadResource`, handlers custom comme `owners.rs`.

## Revue Phase 3 (Claude) — "tout vert" ne suffisait pas

Cadence a livré 17 commits, `cargo test`/`clippy`/`fmt` et `npm run build`/`test` tous
verts. Une revue de clôture (`/code-review`, 8 angles + vérification manuelle) a montré
qu'aucun test ne faisait un vrai aller-retour HTTP+DB avec les payloads réels du
frontend — deux bugs rendaient la feature non fonctionnelle malgré ça, et un
troisième, plus profond, aurait cassé le déploiement même sur Postgres réel :

1. **Chat cassé dès le 2ᵉ message** : inserts manuels (`persist_message`,
   `create_conversation`, `ensure_owner`) posaient `id: Set(0)` au lieu de `NotSet` —
   la 2ᵉ conversation/le 2ᵉ message créés système-wide percutaient la contrainte de
   clé primaire.
2. **CRUD générique cassé en 400** : `id`/`owner_id` sans `#[serde(default)]` sur les
   6 entités, alors que le frontend ne les envoie jamais (`resource_router`
   désérialise le body en `Model` tel quel). Vérifié empiriquement en désérialisant le
   body réel de `AgentsScreen.vue`.
3. **Trouvé en creusant le bug 1, pas dans la liste initiale** : plusieurs migrations
   chaînaient un index non-UNIQUE en `.index(...)` directement sur `Table::create()` —
   valide seulement avec `.unique()` (contrainte de table réelle) ; sans lui, sea-query
   génère un fragment `(...)` orphelin, syntaxiquement invalide, **vérifié aussi bien
   contre Postgres que SQLite**. Aurait cassé le déploiement même en conditions réelles
   — personne n'avait fait tourner ces migrations contre une vraie base avant cette
   revue (décision Phase 1 : environnement 100% jetable, pas de test live prévu).

Tous corrigés par Claude directement (le développeur a demandé "corriges toi-même"
plutôt qu'un renvoi à Cadence), avec tests de régression permanents :
`db::test_support::real_db()` (sqlite en mémoire, les deux migrateurs appliqués —
`sqlx-sqlite` ajouté en `[dev-dependencies]` de `app/Cargo.toml`, jamais en
production) + régressions ciblées sur `agents`/`conversations`/`llm_providers`
couvrant chacun des trois bugs. **Leçon** : un jeu de tests entièrement vert ne
garantit rien sur un aller-retour body réel → désérialisation → insert si aucun test
ne l'exerce réellement — le mock côté frontend et les tests de contrat côté Rust
(politique RBAC, pas le body JSON) avaient chacun un angle mort que seul l'autre
aurait pu couvrir, et ni l'un ni l'autre ne le faisait.

Corrections secondaires (hauts/moyens/bas, toutes appliquées) : validation
`provider_type`/`server_type`/`mode` restaurée via `before_create` (pur, sans accès
DB) ; préfixe `/api` vs `/api/v1` incohérent sur les endpoints custom llm-providers/
mcp-servers, aligné ; pagination frontend (`useCrudResource.fetchAll`) qui tronquait
silencieusement au-delà de la première page, corrigée ; index composite
`messages(conversation_id, created_at)` restauré ; `set_default_provider` réécrit en 2
`UPDATE` bulk (au lieu d'un fetch-all + boucle + re-fetches morts) ; `db_err` dupliqué
dans 9 fichiers consolidé en un seul `impl From<sea_orm::DbErr> for AppError`.

**Faux positifs écartés** (remontés indépendamment par plusieurs angles de revue,
vérifiés avant d'être classés non-bugs) : l'absence de filtre `owner_id` sur
`LlmProvider`/`McpServer` dans `config_store.rs` est voulue (ressources globales,
décidée en Phase 1) ; les invariants auth (expiration JWT, CSRF state,
`HttpOnly`/`SameSite=Lax`) sont à parité avec miryad-core (vérifié dans son code
source, pas supposé).

## Merge avec `git-integration` — conflits réels, pas cosmétiques

Les deux branches avaient touché les mêmes fichiers (`Cargo.toml`, `sandboxes.rs`,
`error.rs`). Résolution : `Cargo.toml` — additions non conflictuelles combinées
(dépendances git-proxy de `git-integration` + `[dev-dependencies]` sqlite de cette
feature) ; `sandboxes.rs` — conservé les tests `raw_git_tail` de `git-integration`,
adapté `git_proxy` pour utiliser `resolve_user`/`miryad_core::auth` (l'ancien
`get_or_create_user` qu'il appelait n'existe plus après la bascule) ; `error.rs` —
auto-mergé proprement (les deux variantes/impls coexistaient sans collision réelle).
Validation complète post-merge : tous verts (87 tests app, 337 tests frontend).

## Statut

Mergé dans `main` (`d34950b`), poussé. `docs/features/miryad-core-integration.md`
supprimé après migration du contenu pertinent dans `docs/architecture.md`. Pas encore
testé sur cluster réel (comme toutes les features récentes de cette lignée).
