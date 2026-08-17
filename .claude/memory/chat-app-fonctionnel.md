# Feature : chat app fonctionnel (2026-08-17 → 2026-08-18)

Trois axes livrés ensemble sur `feat/chat-app-fonctionnel` (pas encore mergée ni
poussée). Détail technique complet migré dans `docs/architecture.md` (section
"Frontend — shell IDE Vue", sous-section "Chat — contexte de conversation, tools
sandbox, Nuxt UI" + section "Backend web", paragraphe "WebSocket chat"). Ce fichier
garde ce qui ne va pas dans une doc d'architecture : le déroulé, les décisions prises
en cours de route, et ce que la revue Phase 3 a trouvé.

## Origine

Trois problèmes distincts remontés par le développeur sur le chat de l'app :
1. Les agents référençaient des tools sandbox dans leur toolset sans jamais pouvoir
   les utiliser en pratique (même en CLI, sans droits).
2. Aucun moyen de régler top_p/top_k/min_p/repeat_penalty/thinking_mode/etc. côté web
   (le profil de modèle n'exposait que temperature/max_tokens dans l'UI).
3. `vue-advanced-chat` est un composant de chat humain-humain (bulles, statuts lu/
   distribué) — pas adapté à une conversation avec un LLM (pas de rendu markdown/code,
   `tool_result` jamais géré dans le switch du composant, pas de concept de reasoning).

Cadrage explicite du développeur : pas la CLI dans cette session, uniquement l'app.
Les trois problèmes traités comme une seule feature à trois axes (pas trois features
séparées) — "on rend le chat de l'app fonctionnelle, c'est une feature, 3 axes."

## Process

Design doc classique (Phase 1) puis le développeur a demandé à Claude d'implémenter
lui-même les trois axes plutôt que de découper en tâches `.tasks/` pour Qwen — écart
assumé au mode habituel (Qwen implémente), adapté pour cette session. TDD respecté
(tests écrits avec ou juste avant chaque changement), commits atomiques par axe,
`cargo fmt`/`clippy -D warnings`/tests lancés avant chaque commit — sauf un oubli de
`cargo fmt` sur les 3 premiers commits d'axe, rattrapé en Phase 3 (cf. plus bas).

**Interruption machine** : crash PC (NVMe défaillant) en plein milieu, juste après
avoir flagué le problème Tailwind CSS de l'axe 3 (cf. ci-dessous) et avant que le
développeur ait répondu. Rien perdu — tout le travail jusque-là était commité. Reprise
propre à la question en attente.

## Décisions actées en cours de route

- **Modèle de contexte polymorphe, pas une FK directe vers une sandbox** : demande
  explicite du développeur. "Pour l'instant, toute les conversations auront en
  contexte une sandbox dans l'app. Mais à terme, le contexte pourra être différent :
  une fenêtre de chat dans les écrans de paramétrage pour aider le paramétrage par
  ex. Du coup une clé directe dans le modèle vers une sandbox ne scalera pas."
  → table `chat_contexts` (`kind`/`data` JSONB), un seul `kind` implémenté
  (`"sandbox"`) mais le schéma n'a pas besoin de migration pour en accueillir un
  second.
- **Composant Chat sur étagère, pas construit à la main** : demande explicite ("je
  préfèrerais vraiment prendre un composant sur étagère"). Recherche de l'existant
  Vue+LLM (markstream-vue/streamdown-vue = juste des renderers markdown, pas des
  composants de chat complets ; Nuxt UI Chat = seul vrai composant complet
  tool-calls/reasoning/streaming pour ce cas d'usage).
- **Correction en cours de route, actée avec le développeur** : l'annonce initiale
  ("Nuxt UI n'exige pas Tailwind d'après la doc consultée") était fausse — vérifiée en
  installant réellement le paquet et en lisant la doc d'installation Vue exacte, pas
  le résumé d'une page. Tailwind CSS + `@nuxt/ui/vite` + wrapper `<UApp>` global sont
  en réalité obligatoires. Le développeur a tranché explicitement : **accepter
  Tailwind global** plutôt que scoper ou abandonner Nuxt UI. Pas de conflit visuel
  constaté avec Element Plus/Reka UI après coup, mais pas testé en conditions réelles
  (pas de backend disponible dans l'environnement de dev).

## Revue Phase 3 — ce qui a été trouvé

- **Faille de scoping owner** (trouvée par Claude en review, pas signalée par le
  développeur) : `resolve_extra_mcp` (`app/src/ws/chat.rs`) résolvait
  `sandbox_mcp_url` à partir de `context.data.sandbox_name` — un champ posé
  **librement par le client** à la création de la conversation — sans vérifier que
  cette sandbox appartient à l'Owner de l'utilisateur authentifié. Tous les autres
  endpoints touchant une sandbox (`api::sandboxes::*`) appliquent ce scoping
  systématiquement ; celui-ci avait été oublié à l'écriture initiale. Corrigé :
  même vérification (`project.spec.owner == owner`) ajoutée avant résolution de
  l'URL MCP.
- **`cargo fmt` non lancé** sur les 3 premiers commits d'axe — 4ᵉ occurrence du même
  motif déjà noté sur `ws10-language-support` et `editing-context-menus`
  (`.claude/memory/ws10-language-support.md`, `.claude/memory/editing-context-menus.md`).
  Rattrapé en Phase 3, mais le motif est maintenant récurrent — à surveiller de plus
  près en cours d'implémentation la prochaine fois, pas seulement à la clôture.
- **Fuite de connexion WS à l'unmount** : `useChat` (AI SDK) n'a pas de nettoyage
  automatique — fermer la session ou changer de conversation en plein streaming
  laissait le WS ouvert côté navigateur jusqu'au `done`/`error` naturel du tour, plus
  personne pour le lire. Corrigé par `stop()` sur `onUnmounted` dans `ChatSession.vue`.

## Limites connues assumées

- `skill_loaded`/`subagent_*`/`usage` (`ChatEvent`) n'ont pas d'équivalent dans le
  modèle `UIMessage` de l'AI SDK (pensé mono-agent) — ignorés dans l'adaptateur
  `VanylineChatTransport`, pas de rendu dans l'UI. Pas tranché pour la suite
  (mono-agent en pratique aujourd'hui, donc pas encore gênant).
- Pas de validation stricte du contenu de `data` dans `chat_contexts` côté backend —
  cohérent avec le reste du codebase (`options` JSONB de `ModelProfile`, `mcp_servers`
  déjà des passthrough non validés).
- **Pas de test en conditions réelles** : pas de Postgres/K8s dans l'environnement de
  dev de cette session — seuls les tests unitaires/intégration (Rust + Vitest) et le
  build ont été vérifiés, plus un boot du serveur Vite dev sans backend. Le
  développeur doit encore valider sur un vrai cluster avant de pousser/merger.

## Reste ouvert (hors scope explicite de cette session)

- CLI non touchée (cadrage explicite du développeur) — les mêmes trois axes n'existent
  pas côté CLI (paramètres avancés déjà possibles via `options` en YAML, mais pas de
  contexte de conversation ni de composant de chat côté CLI).
- Contexte `"settings"` (chat d'aide au paramétrage) : modèle de données prêt à
  l'accueillir, rien d'implémenté.
