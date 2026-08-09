# Réorientation (2026-08-09)

Session de brainstorm (développeur + Claude, aucune implémentation en cours de session
sauf trois actions explicitement demandées en fin de session — cf. "Actions déjà faites"
plus bas) déclenchée par deux constats indépendants : (1) les 11 commits `front-crud`
avaient été pilotés par une instance DeepSeek mal configurée, sans confiance possible dans
le travail produit ; (2) un doute de fond sur la direction du projet, indépendant du point (1).

**Correction importante (même jour, fin de session)** : une première synthèse de cette
session avait surestimé la portée du changement — écrite comme un abandon du web IDE au
profit d'un "substrat d'exécution d'agents autonomes". C'est faux, corrigé par le
développeur avant clôture. Le web IDE **reste utile et n'est pas abandonné**. Ce qui suit
est la version corrigée.

## Diagnostic

- **La prémisse "se passer de Claude" ne tenait pas.** `AGENTS.md` ne listait jamais
  Claude Code comme client de la sandbox (frontend/kydah-code/app seulement) — pas un
  oubli, un angle mort du design initial. Un routeur MCP dédié pour connecter Claude à une
  sandbox a été envisagé puis écarté : si Claude dispose déjà d'un accès kubectl,
  `kubectl exec` fait exactement ce qu'un routeur ferait, sans plomberie à maintenir.
- **Le maquettage Penpot n'a pas convaincu à l'usage** — pas l'idée d'avoir une UI web
  (elle reste voulue), mais la méthode : concevoir visuellement à la main sans compétence
  design dans l'équipe (LLM y compris) ne produit pas un résultat satisfaisant.
- **La vision "hub pour tout"** (workflows de traitement génériques, extraction de traces
  pour post-training) est un cul-de-sac — conclusion héritée d'une session sur un autre
  projet, non rouverte ici.
- **Un besoin réel, non couvert** par le duo claude-code/opencode actuel : des workflows
  automatisés, batch, avec de l'intelligence LLM, agents autonomes sur tâches délimitées.
  Ceci s'ajoute au projet, ça ne le remplace pas.

## Ce qui change réellement

### 1. Méthode : on n'invente plus, on assemble sur étagère

Principe transversal, pas limité à l'UI : la seule chose que ce projet doit "inventer"
reste la gestion des pods Kubernetes (sandbox, controller — déjà faite). Tout le reste se
construit par assemblage de pièces existantes, choisies objectivement, plutôt que par
conception ad hoc. Concrètement pour l'UI (web IDE compris, pas seulement un hypothétique
nouvel écran) :

- **Bits UI / Melt UI** — primitives Svelte headless (comportement clavier/focus/ARIA
  correct, zéro CSS imposé), même auteur/base que shadcn-svelte.
- **shadcn-svelte** — style visuel déjà pris par-dessus Bits UI, réputé pour les outils
  pro denses (dashboards denses, tables, command menus) — pas un template marketing
  responsive. Code copié dans le repo, possédé, pas loué (pas de risque plateforme).
- **Coquille desktop en CSS Grid fixe**, écrite à la main — pas de breakpoint mobile.
- **Svelte Flow (xyflow)** réservé si un éditeur de DAG visuel devient un vrai besoin —
  encore en alpha, à revalider au moment venu, pas une dépendance centrale d'entrée de jeu.
- Kits admin "mobile-first" classiques (Flowbite Svelte Admin, SvelteForge Admin) écartés
  pour cette raison précise — pas parce que "sur étagère" est rejeté en soi.

### 2. UI dense, esthétique desktop

Le web IDE (et tout écran de config/CRUD) vise une densité et une logique d'interaction
d'application desktop (type VSCode/Figma) — pas un site adaptatif qui se replie en
hamburger sur mobile. C'est un critère de sélection des outils ci-dessus, pas un nouveau
composant à construire.

### 3. Fonctionnel — webchat en priorité très basse

L'ambition "hub de chat web" (interface web pour piloter des conversations LLM) passe en
priorité très basse — pas éliminée, mais pas un chantier prioritaire. Le web IDE
(édition/revue de code, écrans de config) reste prioritaire et utile.

### 4. Fonctionnel — ajout : workflows/batch/DAG

Nouvelle capacité, additive :

- État des workflows en **Postgres** (`app/`, sqlx déjà câblé) — **modèle de DAG réel dès
  le départ** (nœuds + arêtes de dépendance, pas une approximation séquentielle/parallèle
  à enrichir plus tard) : position explicite du développeur, un workflow *est* un DAG. Le
  DAG reste un problème borné (tri topologique + Postgres) — ce qui rend Argo/Tekton
  lourds, c'est la généricité autour (templating, retries/backoff, artefacts, RBAC, UI),
  pas le concept de DAG en soi.
- **Sandboxes existantes = surface d'exécution**, pas de nouveau Job K8s par étape — les
  pods sandbox (déjà gérés par le controller, pré-câblés MCP/toolchains) reçoivent le
  travail via leur interface déjà existante (MCP HTTP / RPC stdio du CLI).
- **Worker séparé de l'UI** : un Deployment (+HPA au besoin), pas dans le même pod que
  l'UI — lit Postgres, pilote les sandboxes pour les nœuds prêts du DAG, écrit le résultat
  en retour.
- Orchestrateurs du marché écartés, raisons vécues et vérifiées, pas juste des préférences :
  - **Argo Workflows** — confiance envers l'éditeur (méfiance de longue date du
    développeur envers les stratégies commerciales agressives type "pied dans la porte" —
    vécu et vérifié une fois avec Bitnami/changement de licence).
  - **Tekton** — opérationnel (~80k `TaskRun` dans un namespace vécus, api-server en
    souffrance ; leçon retenue : ne jamais faire porter de l'état applicatif à fort volume
    par etcd/l'API K8s — d'où aussi l'abandon d'une éventuelle CRD `Workflow`/
    `WorkflowRun`, même corrigée en cours de session avant d'être proposée comme design
    final).
  - **n8n** — pas code-first.
  - **Prefect** — déjà opéré en prod dans le homelab du développeur
    (`$HOME/projets/kydah/box/apps/prefect`, Postgres+Grafana), mais UI web et Python
    jugés peu adaptés, concepts (flows/tasks/deployments/work-pools/blocks) jugés trop
    riches pour le besoin.
  - **Grafana comme UI applicative** — outil de monitoring pour le développeur, pas une UI
    qu'il apprécie pour piloter une appli (déjà utilisé pour Prefect, sans en garder une
    bonne impression pour cet usage).

## Rôle de Claude

Reste **interactif uniquement** (Claude Code, VS Code et/ou autre) — pas de rôle
headless/automatisé dans les workflows eux-mêmes (ça, c'est Qwen/opencode et/ou DeepSeek).
Accès au code dans les sandboxes via `kubectl exec` — suffisant, pas besoin de MCP dédié.
Vérifié en session (agent `claude-code-guide`) : la facturation Claude Code dépend du mode
d'authentification (`CLAUDE_CODE_OAUTH_TOKEN`/login = tarif abonnement même en headless ;
`ANTHROPIC_API_KEY` = tarif API à l'usage), pas du mode interactif/headless en soi —
information gardée au cas où un futur besoin headless réapparaîtrait, mais sans impact sur
le design actuel puisque Claude reste interactif par choix du développeur. Réserve non
vérifiée : clauses d'usage raisonnable Anthropic pour du batch automatisé intensif, à
clarifier avant toute dépendance de production si le sujet revient.

## Actions déjà faites en clôture de cette session

- Les 11 commits `front-crud` (pilotés par la session DeepSeek mal configurée, de
  confiance nulle) retirés de l'historique : `main` reset sur `419103c` (merge
  `feat/ws14-cli-backend-llm-exec`), pas de remote tracking donc aucun risque de
  divergence avec `origin`.
- MCP `penpot` retiré de `~/.claude.json` (déjà fait côté opencode par ailleurs).
- `docs/features/app-harness-parity.md` supprimé — la méthode (maquettage Penpot puis
  code à la main) qui le sous-tendait est abandonnée ; le besoin qu'il décrivait (CRUD de
  config, chat) n'est pas abandonné, juste à reconstruire avec la nouvelle méthode et avec
  le webchat en priorité basse.
- `.claude/MEMORY.md` restructuré en index + fichiers dédiés sous `.claude/memory/`
  (demande séparée du développeur, cf. absence de contenu dupliqué inutilement).

## Reste à faire

Pas de design doc formel écrit pendant cette session (brainstorm assumé, seules les
actions ci-dessus ont été exécutées). Prochaine étape naturelle : un
`docs/features/<nom>.md` phase 1 pour la partie workflow/DAG (schéma Postgres, contrat du
worker Deployment) et, séparément, un point sur la méthode de construction UI (choix
précis Bits UI/Melt UI/shadcn-svelte, périmètre IDE vs CRUD vs webchat) — accord explicite
du développeur requis avant de lancer l'un ou l'autre.
