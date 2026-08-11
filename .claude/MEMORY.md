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
**Stack** : Rust (app, sandbox, controller) + TypeScript/Svelte 5/CodeMirror 6 (frontend)
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
- Toutes les features de l'index ci-dessus sont terminées et closes ; leurs design docs
  (`docs/features/*.md`) sont supprimés. Pas de design doc formel écrit pour la nouvelle
  direction (réorientation) pour l'instant.

**Hors scope pour cette phase :**
- Intégration app ↔ sandbox — **correction du 2026-08-09** : la raison invoquée jusqu'ici
  ("nécessite le controller à maturité") était fausse, le controller est implémenté et
  déployé depuis `controller-bootstrap` (2026-07-11, cf. `.claude/memory/controller-bootstrap.md`).
  L'écart trouvé en creusant l'écart code/contrat visuel du frontend (2026-08-09) : `app`
  ne branche même pas la feature Cargo `k8s` de `vanyline-lib`, donc `VnlK8sClient`
  n'est pas compilé côté `app` — c'est ça le vrai blocage, pas la maturité du controller.
  Reste hors scope pour l'instant par choix de séquencement (famille de features
  "arrimage fonctionnel", cf. `docs/features/frontend-ui-shell.md`), pas par nécessité.
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

- ~~Framework frontend~~ — décidé : Vite + svelte-spa-router (même stack que gramophone/frontend dans vynil)
- Web framework Rust pour app : axum probable, pas encore décidé
- ~~Auth app→sandbox~~ — décidé : SA TokenReview (SA du Owner concerné), même mécanisme que kydah-code
- ~~Providers LLM~~ — décidé : Ollama/llama.cpp/vllm auto-hébergés dans le cluster ou via un proxy avec API Ollama-compatible. Aucun provider cloud. Endpoint configurable.
- Format des identifiants d'erreur
- Logger projet (app et sandbox)
