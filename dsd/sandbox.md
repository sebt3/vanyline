# Domain Definition: Sandbox (environnement de développement pour LLM)

## Résumé

Le domaine `sandbox` couvre le pod de travail où un développeur et un LLM lisent, modifient, compilent, testent et
vérifient du code : l'image et ses toolchains, l'accès authentifié (MCP, git, jobs, WebSocket de l'éditeur, LSP), la
**santé du code** et les bornes de sécurité de l'agent de code. C'est le point fort et le cœur de vanyline : sa
direction ne change pas avec la réorientation du harnais LLM (`app#DEC-1`).

Statut du document : **ébauche** (2026-11-03), écrite depuis les décisions déjà consignées (`vanyline.sdd`, `AGENTS.md`,
specs de `sandbox/` et `health/`). Aucune Solution n'est rédigée. Les points non sourçables sont des `TBD` ou des Open
Issues.

## Objectif et problème

Un LLM de code a besoin d'un environnement reproductible où l'on peut tout faire (compiler, lancer des scripts
d'installation), sans que cette liberté touche autre chose que le pod. Le développeur a besoin du même environnement,
dans un éditeur web, et d'un verdict de santé du code identique à celui du central.

## Objectifs métier

- **BO-1:** Un développeur ou un LLM travaille sur une branche d'un projet dans un pod isolé et prêt à l'emploi :
  toolchains, git, éditeur, terminal.
    - **Source:** `AGENTS.md`, composant sandbox

- **BO-2:** L'état de santé du code est toujours disponible dans la sandbox, et identique en local et au central.
    - **Source:** SpecDD /sandbox/src/health.sdd (contrat posé le 2026-10-04) ; SpecDD /health/src/lib.sdd

- **BO-3:** L'agent de code peut agir librement dans le pod, et les effets de ses actions sont bornés au niveau du pod.
    - **Source:** `app#DEC-3`

## Périmètre et non-objectifs

Le domaine couvre : l'image de base et les images de toolchains, le serveur de la sandbox (MCP, git, jobs, tickets,
WebSocket, LSP, ports), la santé du code (crate `vanyline-health` et son exposition), l'utilitaire de maintenance
`vanyline-maint`, et le modèle de menace de l'agent de code.

Hors périmètre, par décision :

- la création, la suspension et l'adressage réseau des pods : domaine `workspaces` ;
- les droits d'accès des utilisateurs : décidés en amont par `app` (lien utilisateur↔Owner) ;
- la conduite du LLM : domaine `agent-sessions` de lymaille, hébergé via `app`.

## Acteurs

- **Owner :** propriétaire unique de la sandbox ; seule identité acceptée.
- **Développeur :** utilise l'éditeur web, le terminal et les relais git de `app`.
- **Agent de code :** LLM qui agit par le MCP de la sandbox, avec les droits de l'Owner.
- **`app` :** obtient le jeton de l'Owner après avoir vérifié le lien utilisateur↔Owner, et relaie git et santé du code.
- **Opérateur :** obtient le jeton de l'Owner avec son contexte kubeconfig.
- **Controller :** crée le pod, injecte les toolchains et l'environnement, lance les Jobs de maintenance.
- **Supervision :** lit `/health` et `/metrics` sur le port d'exploitation.

## État actuel et état cible

- **Actuel (Observed in: `sandbox/src/`, `health/src/`) :** le serveur expose 17 outils MCP, 16 routes git, la santé du
  code, une API de jobs, un PTY et des ponts LSP ; l'auth par jeton d'Owner est contractée (`sandbox/src/auth.sdd`),
  son implémentation figure parmi les tâches ouvertes de `vanyline.sdd`.
- **Cible :** celle de la spec racine et de `docs/architecture.md` (« État cible, décisions du 2026-10-04 ») ; l'agent
  de code agit par les modules Rhai de lymaille, générés depuis le MCP de la sandbox (`app#DEC-1`), et l'outillage SpecDD
  y est disponible (`OI-4`).

## Hypothèses, contraintes et dépendances

- **CON-1:** Les toolchains sont des images OCI standard montées en `volumes[].image` (fonction native de Kubernetes,
  prérequis 1.31, stable depuis 1.36) ; l'image de base et les images de toolchains appartiennent à la même famille de
  distribution (trixie).
    - **Source:** `AGENTS.md`, composant sandbox

- **CON-2:** Le substrat natif commun de l'image de base inclut un éditeur de liens C (`cc`, `ld`, binutils, `libc-dev`,
  `make`, `pkg-config`), faute de quoi aucune compilation native ne lie.
    - **Source:** `AGENTS.md`, composant sandbox

- **CON-3:** Les outils de vérification de la santé du code (`cargo-llvm-cov`, `ruff`, `hadolint`, …) sont présents dans
  les images toolchain, à versions épinglées.
    - **Source:** `AGENTS.md`, composant sandbox

- **CON-4:** Le serveur MCP de la sandbox est sans état : aucune session MCP, chaque requête est autonome.
    - **Source:** SpecDD /sandbox/src/mcp.sdd

## Glossaire

- **Sandbox :** pod de travail d'un Owner, qui monte un worktree d'une branche d'un Project (voir `workspaces`).
- **Jeton d'Owner :** jeton JWT émis par l'apiserver Kubernetes pour le ServiceAccount `owner-<nom>`, par l'API
  `TokenRequest`, avec l'audience `vanyline-sandbox`.
- **Ticket :** jeton opaque, court-vécu et à usage unique, qui authentifie l'ouverture d'un WebSocket pour un navigateur.
- **Port principal / port d'exploitation :** le premier porte le MCP, git, la santé du code, les jobs et les tickets ;
  le second `/health` et `/metrics`.
- **Toolchain :** image OCI montée en volume et rendue utilisable par injection d'environnement au démarrage.
- **Santé du code :** verdict typé des vérifications d'un projet (compilation, lints, tests, couverture), restreint au
  code modifié, calculé par `vanyline-health`.
- **Job :** script shell durable de la sandbox, démarré de façon idempotente et interrogé par un workflow.
- **Confinement :** résolution de tout chemin sous la racine du workspace.

## Règles du domaine

- **BR-1 - MUST:** La sandbox appartient à un seul Owner et l'accès est tout ou rien : le jeton d'Owner est l'unique
  moyen d'authentification, sans niveau d'accès, sans groupe, sans mode de contournement.
    - **Source:** `vanyline.sdd`, section `Must` (décision du 2026-10-04) ; SpecDD /sandbox/src/auth.sdd
    - **Verified by:** `AC-1`

- **BR-2 - MUST:** Le ServiceAccount de l'Owner n'a aucun droit RBAC : c'est une identité, jamais un pouvoir.
    - **Source:** `vanyline.sdd`, section `Must not`
    - **Verified by:** `AC-1`

- **BR-3 - MUST:** Le navigateur ne voit jamais le jeton de l'Owner ni un JWT ; il n'utilise que des tickets.
    - **Source:** `vanyline.sdd`, section `Must not`
    - **Verified by:** `AC-2`

- **BR-4 - MUST:** `/health` et `/metrics` sont servis sur un port distinct, sans authentification, jamais publié.
    - **Source:** `vanyline.sdd`, section `Must`
    - **Verified by:** `AC-2`

- **BR-5 - MUST:** Une toolchain devient utilisable par injection d'environnement au démarrage (`PATH`,
  `LD_LIBRARY_PATH`, variables propres à la toolchain), jamais par magie ; ce qui doit être inscriptible est hors du
  volume, dans le PVC de l'Owner.
    - **Source:** `AGENTS.md`, composant sandbox
    - **Verified by:** `AC-3`

- **BR-6 - MUST:** Le confinement des chemins sous la racine du workspace s'applique à toute voie d'accès (outils MCP,
  WebSocket de fichiers, LSP). Il est une garde d'ergonomie : la frontière de sécurité réelle est le pod.
    - **Source:** `docs/architecture.md`, section « Serveur MCP » ; SpecDD /sandbox/src/tools_impl.sdd
    - **Verified by:** `AC-4`

- **BR-7 - MUST:** L'état de santé du code est toujours disponible ; il est recalculé après un délai de calme qui suit
  toute modification, et tous les consommateurs lisent le même état, calculé par le même code.
    - **Source:** BO-2
    - **Verified by:** `AC-5`

- **BR-8 - MUST:** Toute maintenance d'un workspace (clone, fetch, purge, worktree, détection de langages) s'exécute dans
  un pod à image sandbox par l'utilitaire `vanyline-maint`, avec des arguments en argv, sans interpolation d'un champ de
  CRD dans une commande shell.
    - **Source:** `vanyline.sdd`, section `Must not` ; `AGENTS.md`, règle de maintenance des projets
    - **Verified by:** `AC-6`

- **BR-9 - MUST:** Les effets de l'agent de code se bornent au niveau du pod, pas par l'approbation de ses actions : les
  toolsets accordés bornent son intention, pas les effets d'un script d'installation ou d'un `build.rs`.
    - **Source:** `app#DEC-3`
    - **Verified by:** `AC-7`

## Exigences fonctionnelles transverses

- **FR-1 - MUST:** La sandbox offre à l'agent de code un accès aux fichiers, à la recherche, à l'exécution de
  commandes, au LSP, à l'édition suivie d'un contrôle et à la santé du code, par un serveur MCP.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, SpecDD /sandbox/src/mcp.sdd

- **FR-2 - MUST:** La sandbox offre à l'éditeur web un système de fichiers temps réel, un terminal et des ponts LSP, par
  WebSocket authentifié par ticket.
    - **Source:** SpecDD /sandbox/src/ws/ticket.sdd

- **FR-3 - MUST:** La sandbox expose les opérations git du dépôt de travail (état, diff, indexation, commit,
  branches, fusion, envoi, clé SSH).
    - **Source:** SpecDD /sandbox/src/git.sdd
    - **Verified by:** `AC-8`

- **FR-4 - MUST:** La sandbox exécute des scripts shell de longue durée, démarrés de façon idempotente, interrogeables,
  annulables, avec leurs journaux.
    - **Source:** SpecDD /sandbox/src/jobs.sdd

- **FR-5 - MUST:** Les vérifications de santé du code couvrent au moins Rust, JavaScript/TypeScript/Vue, Python et
  Dockerfile.
    - **Source:** SpecDD /health/src/lib.sdd ; `AGENTS.md`, composant `health/`

- **FR-6 - MUST:** L'outillage SpecDD est disponible dans la boîte à outils JS de la sandbox, à une version épinglée.
    - **Source:** `app#FR-10` ; user instruction, 2026-11-03
    - **Verified by:** `AC-9`

## Qualité, sécurité et contraintes

- **NFR-1 - MUST:** Toute erreur de la sandbox et de la santé du code porte un identifiant unique (`VNL-SBX-*`,
  `VNL-HLT-*`).
    - **Source:** `AGENTS.md`, section Git (identifiants d'erreur uniques)

## Paysage des Solutions (proposition de découpage)

Huit Solutions sont écrites (reverse depuis les specs existantes, 2026-11-07) ; `sandbox/threat-model` reste à écrire avec le développeur.

- `sandbox/runtime` : image de base, toolchains, injection d'environnement, exécution des commandes, venv.
- `sandbox/access` : jeton d'Owner, tickets, ports principal et d'exploitation.
- `sandbox/mcp` : serveur MCP, outils, confinement, LSP, édition suivie d'un contrôle.
- `sandbox/editor` : WebSocket de fichiers, terminal, ponts LSP.
- `sandbox/git` : routes git et gestion de la clé SSH.
- `sandbox/code-health` : crate `vanyline-health`, cycle de vie de l'état, exposition HTTP et MCP.
- `sandbox/jobs` : API de jobs pour les workflows.
- `sandbox/maintenance` : `vanyline-maint`.
- `sandbox/threat-model` : liste de ce que l'agent de code ne doit jamais pouvoir faire, et bornes posées avec
  `workspaces`.

## Décisions du domaine

- **DEC-1 - Le jeton d'Owner est l'unique authentification**
    - **Context:** la version précédente prévoyait OIDC, groupes et niveaux d'accès.
    - **Decision:** un seul mécanisme (`BR-1`), décidé en amont par `app` ou par le RBAC Kubernetes pour un opérateur.
    - **Source:** `vanyline.sdd`, section `Must` (décision du 2026-10-04)

- **DEC-2 - Le `git push` n'est pas restreint par la sandbox**
    - **Decision:** la clé SSH de l'Owner, présente dans son PVC home monté dans la sandbox, tire et pousse ses dépôts ;
      la protection des branches relève de la forge.
    - **Consequences:** le rayon d'action de l'agent de code inclut tous les dépôts accessibles par cette clé.
    - **Source:** `app#DEC-4` ; `docs/architecture.md`, section « Clé SSH git » (2026-08-22)

- **DEC-3 - Les effets sont bornés au niveau du pod**
    - **Decision:** voir `BR-9` ; les moyens (réseau sortant, secrets, quotas, isolation noyau) sont décidés dans la
      Solution `sandbox/threat-model` avec `workspaces`.
    - **Source:** `app#DEC-3`

## Transition et migration

Les specs `.sdd` existantes de `sandbox/` et `health/` décrivent l'état actuel et restent valides tant qu'une Solution
ne les amende pas. L'auth par jeton d'Owner est contractée mais pas entièrement livrée (tâche ouverte de `vanyline.sdd`).

## Acceptation et vérification

- **AC-1:** Un appel à la sandbox avec un jeton d'un autre Owner, ou sans jeton, est refusé ; le ServiceAccount de
  l'Owner ne peut lire aucune ressource Kubernetes.
- **AC-2:** Un navigateur ouvre un WebSocket avec un ticket qui ne sert qu'une fois ; `/health` répond sur le port
  d'exploitation sans authentification et n'est pas joignable par le Service.
- **AC-3:** Dans une sandbox démarrée avec les toolchains `rust` et `node`, `cargo` et `node` s'exécutent et `cargo`
  écrit dans un répertoire du PVC de l'Owner.
- **AC-4:** Un chemin qui sort de la racine, y compris par lien symbolique ou par une URI LSP, est refusé.
- **AC-5:** Après une modification de fichier suivie du délai de calme, l'état de santé du code reflète la
  modification, et le MCP et le HTTP rendent le même état.
- **AC-6:** Un Job de maintenance rejoué n'échoue pas et ne duplique rien.
- **AC-7:** Un script d'installation qui tente un effet interdit par la liste de `sandbox/threat-model` est arrêté par
  une borne du pod, pas par un refus de l'agent.
- **AC-8:** L'état, le diff, le commit et l'envoi d'une branche fonctionnent depuis la sandbox.
- **AC-9:** Dans une sandbox, la commande SpecDD épinglée s'exécute et vérifie les specs du projet.

## Risques

- **Réseau sortant libre par défaut :** sans règle déclarée aux trois niveaux, aucune NetworkPolicy n'est produite
  (Observed in: `docs/architecture.md`, `build_sandbox_egress_netpol`).
- **Clé SSH dans le pod :** voir `DEC-2` ; elle contredit l'idée « aucun secret dans le pod » (`OI-1`).

## Open Issues

- **OI-1:** **Liste de ce que l'agent de code ne doit jamais pouvoir faire.** À écrire noir sur blanc (`BR-9`). Elle doit
  traiter la clé SSH de l'Owner présente dans le pod, le réseau sortant libre par défaut, les quotas et l'isolation
  noyau. **TBD :** à rédiger en session dédiée.
- **OI-2:** **Isolation noyau** (RuntimeClass gVisor ou Kata) : à examiner côté controller. **TBD.**
- **OI-3:** **Un workflow privilégié qui ouvre le même workspace lit des instructions écrites par un contexte de moindre
  confiance** (`app#BR-3`). **TBD :** règle à écrire.
- **OI-4:** **Outillage SpecDD (`FR-6`).** Quelle image toolchain l'embarque, à quelle version, et comment le MCP le
  présente à l'agent. **TBD.**
- **OI-5:** **Spec d'`auth` partiellement livrée.** Reste à implémenter ce que `vanyline.sdd` liste comme chantier ouvert
  (« auth Owner-SA seule »). **TBD :** ordonnancement.

## Références

- `vanyline.sdd`, `AGENTS.md`, `docs/architecture.md`.
- `app` (`app#DEC-1`, `app#DEC-3`, `app#DEC-4`, `app#BR-3`, `app#FR-10`).
