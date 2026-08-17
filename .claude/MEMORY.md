# Mémoire du projet

Ce fichier est un **index synthétique**, pas le détail — à la manière d'un `llm.txt` :
une vue d'ensemble courte de là où va le projet, plus des pointeurs vers des fichiers
dédiés sous `.claude/memory/` où vit le contenu complet (décisions, leçons, pièges
techniques, chiffres). Maintenu par Claude au fil des sessions ; les développeurs
peuvent le lire, le corriger ou le compléter à tout moment.

---

## Identité du projet

**Nom** : vanyline (de "vaniline" — addictif, universellement aimé, 'y' inséré pour l'inverse-SEO)
**But** : Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes
**Licence** : BSD-3, public sur GitHub
**Gitea** : shuss/vanyline (privé, solo pour l'instant)
**Stack** : Rust (app, sandbox, controller) + TypeScript/Vue 3/CodeMirror 6 (frontend)
**Monorepo** : Cargo workspace racine + package.json racine, chaque composant Rust a son sous-workspace

---

## Direction actuelle (synthèse)

Depuis le **2026-08-09**, le projet ajuste sa méthode et étend son périmètre — **ce n'est
pas un abandon du web IDE**, qui reste utile et voulu.

- **Ce qui change en méthode** : on n'invente plus, on assemble sur étagère (principe
  transversal — la seule vraie invention du projet reste la gestion des pods K8s, déjà
  faite) ; l'UI (IDE compris) vise une densité et une esthétique **desktop** (type
  VSCode/Figma), pas un site adaptatif mobile-first. Concrètement pour l'UI : Bits
  UI/Melt UI (primitives headless) + shadcn-svelte (style visuel dense déjà pris) +
  coquille CSS Grid desktop écrite à la main, plutôt que le maquettage Penpot + Svelte
  brut d'avant.
- **Ce qui change en fonctionnel, de façon ciblée** : le webchat (hub de conversation
  LLM web) passe en **priorité très basse** ; une capacité **workflow/batch/DAG** est
  **ajoutée** — état en Postgres (`app/`), vrai modèle de DAG dès le départ, sandboxes
  existantes comme seule surface d'exécution (pas de Job K8s par étape), worker
  Deployment(+HPA) séparé de l'UI. Orchestrateurs du marché écartés après examen (Argo,
  Tekton, n8n, Prefect, Grafana comme UI) — raisons vécues, pas des préférences vagues.
- **Ce qui ne change pas** : le web IDE, gardé comme composant utile ; l'architecture
  quatre composants ; le rôle interactif-only de Claude (`kubectl exec` dans les
  sandboxes, pas de rôle headless dans les workflows).

Raisons détaillées de chaque choix/rejet, correction d'une première synthèse trop large
écrite en cours de session, actions déjà exécutées (reset des 11 commits `front-crud`
pilotés par une session DeepSeek mal configurée, retrait du MCP Penpot, suppression du
design doc `app-harness-parity`) et travail restant :
**`.claude/memory/reorientation-2026-08-09.md`**.

**Mise à jour (2026-08-12)** : le web IDE mentionné ci-dessus comme "gardé, pas
abandonné" est maintenant réellement branché sur une sandbox K8s (Explorer/Editor/
Terminal, plus CRD Application/Ingress par sandbox côté controller) — cf.
`.claude/memory/arrimage-fonctionnel-2026-08.md`. Le choix UI concret qui a émergé du
POC diffère de la piste envisagée ici le 2026-08-09 (Bits UI/Melt UI + shadcn-svelte) :
Vue 3 + dockview-vue + Element Plus + Reka UI, cf. `docs/architecture.md` section
"Frontend — shell IDE Vue" pour le détail et les raisons. Le webchat (priorité basse) et
le workflow/DAG (ajouté) restent non démarrés — cette famille de features n'a touché
que le web IDE et ses prérequis d'infra.

---

## Index — fondations et features livrées

Chaque entrée ci-dessous est **terminée et close** (tests verts, design doc supprimé
le cas échéant) sauf mention contraire. Le détail (décisions, chiffres, pièges
techniques, leçons de délégation Qwen) vit dans le fichier pointé, pas ici.

| Fichier | Contenu |
|---|---|
| `.claude/memory/architecture-fondations.md` | Quatre composants, auth sandbox (JWT/SA TokenReview), toolchains K8s image volumes, kydah-code comme client |
| `.claude/memory/harness-core.md` | Cœur LLM/MCP name-keyed de `vanyline-lib` (`ConfigStore`, `ChatEvent`, `SessionContext`) |
| `.claude/memory/cli-harness.md` | Stockage CLI natif YAML deux couches (global + workspace) |
| `.claude/memory/cli-rpc-stdio.md` | `vanyline serve --stdio`, JSON-RPC 2.0 |
| `.claude/memory/outillage-llm-exec.md` | Délégation à Qwen via `llm-exec` — règles stabilisées, modes d'échec connus, agent opencode `cadence` |
| `.claude/memory/ws09-sandbox-maint-agent.md` | Binaire `vanyline-maint` (init/fetch/purge/checkout/remove) |
| `.claude/memory/ws11-sandbox-git.md` | Endpoints `/git/status`, `/git/unpushed` sur la sandbox |
| `.claude/memory/controller-bootstrap.md` | `vanyline-controller` (WS-4) — CRDs Owner/Project/Sandbox |
| `.claude/memory/initial-app-frontend-sandbox-bootstrap.md` | MVP app, sandbox-bootstrap (WS-3), et clôture d'app-harness-parity (WS-2, méthode revue — cf. réorientation) |
| `.claude/memory/ws12-sandbox-clients.md` | Client K8s CLI (`vanyline-crds`, `VnlK8sClient`), toolbox `--toolbox` |
| `.claude/memory/ws15-quality-hygiene.md` | Gouvernance qualité CI (doc-lint, deny unwrap/expect, clippy-pedantic, coverage) |
| `.claude/memory/ws13-sandbox-runtime.md` | Socle CLI sandbox, egress NetworkPolicy trois niveaux, suspension manuelle |
| `.claude/memory/ws14-cli-backend-llm-exec.md` | Flags `run` `-m/-t/-j`, builtin todo, mapping agents ; correction svelte-check/storybook |
| `.claude/memory/reorientation-2026-08-09.md` | Pivot stratégique complet (cf. "Direction actuelle" ci-dessus) |
| `.claude/memory/arrimage-fonctionnel-2026-08.md` | 7 features (2026-08-10 → 08-12) : web IDE Vue réellement branché sur une sandbox K8s — ticket WS, CRD Application, Ingress par sandbox, todo persistant, config réelle. Process Claude/Cadence/Qwen qui a fonctionné, motif récurrent de doc drift trouvé 4 fois, décisions actées, pièges techniques |
| `.claude/memory/v0.1.1-first-live-deploy.md` | Premier déploiement réel de la CRD Application (media-test, 2026-08-13) : 5 bugs trouvés en live (caCert PEM inline, TLS cert-manager absent, clé DB `uri` vs `databaseUrl`, dependabot npm mal ciblé sous workspaces, frontend sans redirect 401→login) → release v0.1.1, v0.1.2 en attente (tests en cours avant publication) |
| `.claude/memory/frontend-dashboards-nav.md` | Navigation à 3 niveaux (`/` → `/p/:project` → `/p/:project/s/:sandbox`), dashboards Projets/Sandboxes sortis des Settings, Settings restant regroupé (Modèles/Outils/Agents/Skills/Compte) + converti en modales reka-ui, champs relationnels (provider→profil→agent, mcp discovery, local-tools) remplacent le texte libre, 2 endpoints backend ajoutés |
| `.claude/memory/ws10-language-support.md` | Détection Rust/JS-TS (présence seulement, pas de version) → `Project.status.languages` (patch dédié, pas le reconciler) → toolchains Sandbox auto-dérivées si `spec.toolchains` vide (tout ou rien). Livré par l'agent opencode `cadence` plutôt que Qwen/`.tasks/` ; review Claude a trouvé RBAC trop large + bug de chemin relatif + `fmt` non lancé, tous corrigés avant clôture. Tool `validate` (scope originel plus large) non démarré, laissé de côté |
| `.claude/memory/editing-context-menus.md` | Menu Édition (Rechercher/Remplacer), menus contextuels (arbre/éditeur/terminal/onglets), icônes de fichier, CRUD complet sur l'arbre (mkdir/rename/root ajoutés côté sandbox). Livré par Cadence (2ᵉ feature sur ce mode, même motif que WS-10) ; review Claude a trouvé un pan du design jamais câblé (copier chemin sur l'arbre, malgré l'op backend construit pour ça), Coller qui ne remplaçait pas la sélection, suppression sans confirmation, `fmt` non lancé (3ᵉ occurrence) — et un claim de Cadence sur un "crash" de test jsdom non reproduit en review |

---

## Contexte et philosophie

Projet né de deux tentatives précédentes (kydah-ai, kydah-code) qui ont montré les limites
des environnements LLM contraints en RAM/CPU. Philosophie : les langages interprétés vont
devoir réduire de voilure — Rust est un choix délibéré pour la maîtrise des ressources.
vanyline est une couche d'exécution gérée et K8s-native que plusieurs outils peuvent consommer.

---

## Statut actuel

- **tools-v2** (refonte SLM-friendly de `vanyline-tools`, 8 outils finaux) avance en parallèle.
- **vscode-ext-bootstrap** (extension VS Code consommant le RPC stdio) pas encore démarré.
- **Web IDE réellement branché** (2026-08-12) : famille de 7 features
  (`.claude/memory/arrimage-fonctionnel-2026-08.md`) — Explorer/Editor/Terminal
  connectés à une vraie sandbox K8s, CRD Application + Ingress par sandbox côté
  controller, config réelle (`SettingsView`).
- **Navigation et Settings restructurés** (2026-08-14,
  `.claude/memory/frontend-dashboards-nav.md`) : dashboards Projets/Sandboxes (`/`,
  `/p/:project`) sortis des Settings, Settings restant réorganisé et converti en
  modales, champs relationnels (dropdowns) remplaçant le texte libre partout où un
  champ référençait une autre entité. Toutes les features de l'index ci-dessus sont
  terminées et closes ; leurs design docs (`docs/features/*.md`) sont supprimés. Pas
  de design doc formel écrit pour la nouvelle direction (réorientation) pour l'instant.
- **Détection de langages + toolchains automatiques** (2026-08-15,
  `.claude/memory/ws10-language-support.md`) : `vanyline-maint detect` (Rust/JS-TS,
  présence seulement) → `Project.status.languages` → Sandbox monte automatiquement
  les toolchains correspondantes si `spec.toolchains` est vide. Validation sur
  cluster réel pas encore faite (code review + tests unitaires/intégration
  seulement à ce stade).
- **Menus contextuels & affordances d'édition** (2026-08-17,
  `.claude/memory/editing-context-menus.md`) : arbre/éditeur/terminal/onglets
  cliquables droit (clipboard, copie de chemin, CRUD arbre), menu Édition
  Rechercher/Remplacer, icônes de fichier par extension. Branche
  `feat/editing-context-menus` mergée localement dans `main`, pas encore poussée.
  Web IDE toujours pas testé sur cluster réel (idem détection de langages
  ci-dessus).

**Reste ouvert / pas démarré** (pas "hors scope" par nécessité, juste pas encore
attaqué) :
- Auth kydah-code → sandbox (NetworkPolicy en place, aucun mécanisme applicatif) et
  orchestration MCP par `app` (le relais de ticket WS existe, pas d'appel MCP par `app`
  à la sandbox).
- `vanyline sandbox stop|start` (CLI) — champ `suspended` posé côté CRD depuis
  `ws13-sandbox-runtime`, jamais câblé côté CLI.
- Tool `validate` (test/lint/format par toolchain détectée, scope original plus
  large de `ws10-language-support`) — jamais démarré, pas de design doc actif.
- Workflow/DAG (capacité "ajoutée" par la réorientation du 2026-08-09) et webchat
  (priorité très basse) — le panneau Workflow/Chat du shell IDE reste mock.
- Multi-utilisateur complet, quotas
- Permissions/approbation des tools ; compaction automatique du contexte
- Ouverture aux autres contributeurs

**Déclencheur de convergence** : quand les axes sont assez matures pour s'assembler via le controller.

---

## Équipe et ouverture

- Solo pour l'instant (un seul développeur)
- Un second développeur rejoindra quand les deux axes se rencontreront (stade poc→mvp)
- Pas de définition formelle de MVP — construction incrémentale long terme

---

## Points ouverts (TBD)

- ~~Framework frontend~~ — décidé, révisé le 2026-08-12 : Vite + Vue 3 + `vue-router`
  (abandon de svelte-spa-router — tout `frontend/` réécrit en Vue par `frontend-ui-shell`,
  cf. `docs/architecture.md` section "Frontend — shell IDE Vue")
- ~~Web framework Rust pour app~~ — décidé : axum
- ~~Auth app→sandbox~~ — **corrigé le 2026-08-12** : l'entrée précédente ("SA TokenReview,
  même mécanisme que kydah-code") était fausse — jamais implémenté. Le mécanisme
  réellement construit (`sandbox-ingress-wiring`) : `app` relaie un ticket WS en
  présentant le `id_token` OIDC de l'utilisateur authentifié, pas un compte de service.
  kydah-code ne consomme toujours pas la sandbox (pas démarré, mécanisme d'auth pour ce
  client encore à concevoir).
- ~~Providers LLM~~ — décidé : Ollama/llama.cpp/vllm auto-hébergés dans le cluster ou via un proxy avec API Ollama-compatible. Aucun provider cloud. Endpoint configurable.
- Format des identifiants d'erreur
- Logger projet (app et sandbox)
