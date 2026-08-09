# initial-app-frontend et sandbox-bootstrap (terminés, clôturés après coup le 2026-07-31)

Même anomalie que `controller-bootstrap.md`, découverte dans la foulée en vérifiant les
autres design docs restés en `docs/features/` : les deux étaient finis depuis
longtemps sans jamais avoir traversé la Phase 3.

- **initial-app-frontend (MVP)** : auth OIDC/cookie, config API, chat MCP, image
  déployable — tout fait, et depuis dépassé par `app-harness-parity` (tables/API
  name-keyed) sans que ça invalide la clôture (évolution attendue, pas une régression).
  `AdminAuth`/`ADMIN_SECRET` du MVP ont disparu en cours de route (retirés une fois
  l'API scopée par utilisateur) — les commentaires `// admin` encore présents dans
  `app/src/api/mod.rs` sont un résidu inoffensif, pas un vrai contrôle d'accès.
  Détails migrés : `docs/architecture.md` section "Backend web — vanyline-app".
- **sandbox-bootstrap (WS-3)** : les 4 tâches du design (fork-template, tools-glue,
  image, deploy-test) faites. Découverte notable en vérifiant le code réel : l'auth
  du serveur MCP (OIDC/JWKS + groupes, héritée telle quelle du template) est déjà
  active par défaut (refuse de démarrer sans `--no-auth`/`STATIC_TOKEN` explicite) —
  plus avancée que ce que la Phase P1 du design annonçait ("`--no-auth` uniquement").
  Reste un vrai point ouvert, pas un oubli de clôture : ce modèle OIDC/groupes est
  **distinct** des deux modes JWT-app/SA-TokenReview décrits dans `AGENTS.md` pour le
  frontend et kydah-code — personne ne les a encore câblés dessus (P2/P3 du design
  d'origine, jamais démarrés, pas de design doc dédié pour l'instant). Détails migrés :
  `docs/architecture.md` section "Serveur MCP — vanyline-sandbox".
- **Bonus trouvé en vérifiant le code contre `docs/architecture.md`** : la limite
  "Pas de streaming WS live côté app" (section Limites connues) était stale — la
  tâche `ws-chatevent` d'`app-harness-parity` l'a résolue (`ChannelSink` sur canal
  mpsc, streaming réel token-par-token, `CollectingSink` n'existe plus dans le code).
  Bullet retiré.

## app-harness-parity (WS-2) — backend clos, méthode frontend revue

Le backend (migrations, `PgConfigStore`, API REST, WS streaming — tâches 1-4 du design) est
fini et migré dans `docs/architecture.md`. Le frontend (tâches 5-6,
`front-crud`/`front-chat`) n'a jamais été commencé en code — `frontend/src/` n'a
que `Login.svelte`/`Chat.svelte` du MVP. Une session de maquettage Penpot
(2026-08-03, 5/8 écrans posés, patron liste+édition validé sur `LLM Providers`)
avait exploré cette suite, mais n'a pas convaincu à l'usage — un des déclencheurs de la
réorientation du 2026-08-09 (détails : `reorientation-2026-08-09.md`). **Le frontend/IDE
n'est pas abandonné** — il reste utile et voulu. Ce qui change : la méthode (Penpot +
Svelte à la main → assemblage Bits UI/Melt UI + shadcn-svelte, UI dense desktop) et la
priorité du webchat spécifiquement (très basse, contrairement au reste du frontend/IDE).
`docs/features/app-harness-parity.md` supprimé — à refaire avec la nouvelle méthode, pas
sur la base de ce design ni du maquettage Penpot associé.
