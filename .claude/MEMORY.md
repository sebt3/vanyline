# Mémoire du projet

Ce fichier est maintenu par Claude au fil des sessions.
Les développeurs peuvent le lire, le corriger ou le compléter à tout moment.

---

## Identité du projet

**Nom** : vanyline (de "vaniline" — addictif, universellement aimé, 'y' inséré pour l'inverse-SEO)
**But** : Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes
**Licence** : BSD-3, public sur GitHub
**Gitea** : shuss/vanyline (privé, solo pour l'instant)
**Stack** : Rust (app, sandbox, controller) + TypeScript/Svelte 5/CodeMirror 6 (frontend)
**Monorepo** : Cargo workspace racine + package.json racine, chaque composant Rust a son sous-workspace

---

## Architecture — décisions et justifications

### Quatre composants

| Composant | Rôle | Décision |
|-----------|------|----------|
| frontend | Éditeur web + UI LLM | Vite + Svelte 5, CodeMirror 6, svelte-spa-router, Tailwind CSS 4, Vitest, Storybook |
| app | Backend, OIDC, Redis, PGVector | Rust — focus initial : interaction LLM, users, config API |
| sandbox | Pod K8s, serveur WS/MCP | Rust — image Debian slim + toolchains OCI image volumes |
| controller | Opérateur K8s | Rust, kube-rs — **DÉFÉRÉ** |

### L'app n'est pas sur le chemin chaud

Le frontend se connecte **directement** à la sandbox en WebSocket (JWT validé par la sandbox).
kydah-code se connecte directement au MCP de la sandbox via service K8s interne.
L'app ne proxifie rien — elle gère l'auth, le LLM, la config.

### Deux modes d'auth sur la sandbox

- **JWT** (frontend via ingress) : token OIDC émis par l'app, validé par la sandbox
- **SA TokenReview + NetworkPolicy** (kydah-code et app via service interne) : la sandbox appelle
  le K8s TokenReview API pour valider le SA token du pod appelant ; NetworkPolicy par sandbox
  restreint l'accès aux pods du même namespace avec les bons labels

Mécanisme uniforme pour kydah-code ET l'app : tous deux utilisent le ServiceAccount du Owner
concerné. L'app, pour orchestrer le LLM d'un utilisateur donné, utilise le SA de son Owner.

Décision : Option A (JWT émis par l'app pour kydah-code) rejetée car elle crée une dépendance
à l'app avant que l'app existe — incompatible avec le développement des deux axes en parallèle.

### Design sandbox — toolchains via K8s image volumes

Les toolchains utilisent des images Docker standard (ex: `rust:slim-trixie`, `node:trixie-slim`)
montées via `volumes[].image` — feature K8s native GA depuis v1.36, prérequis v1.31+.
Pas de registre propriétaire, pas de build custom.

**Validé en conditions réelles sur cluster 1.36 (2026-07-01, cri-o 1.36.1)** — deux pods de test
(`deploy/sandbox-imagevol-*.yaml`) ont éprouvé node et rust. Recette d'assemblage confirmée :

- **Répartition base / volume / PVC** :
  - *base* = substrat natif commun installé proprement (apt) : **linker C `cc`/`ld` + binutils,
    `libc-dev`, make, pkg-config**, git, curl, vim. Le linker est obligatoire : `rust:slim`
    ne l'embarque pas → `cargo build` échoue sur `error: linker cc not found`. Vrai pour toute
    compilation native (node-gyp, cgo…).
  - *volumes* = toolchains langage, read-only, utilisables par **injection d'env** :
    - `PATH` → `…/bin` du volume
    - `LD_LIBRARY_PATH` → `…/usr/lib/<arch>-linux-gnu` du volume (sinon le loader du base ne
      trouve pas les libs du volume — ex: `libatomic.so.1` manquant pour node)
    - **env par toolchain** : rust → `RUSTUP_HOME` sur le volume (les binaires de `cargo/bin`
      sont des symlinks vers `rustup`, ça suffit — rustup n'est PAS un obstacle)
  - *PVC du Owner* = homes writable **hors volume read-only** : `CARGO_HOME`, `~/.npm`, etc.
- **Contrainte distro** : base et images toolchain sur la **même famille** (trixie). Le loader
  du base résout la glibc ; un mismatch de distro marche par compat ascendante = hasard, pas design.
- **Correction doc** : l'ancienne formule « PATH/LD_LIBRARY_PATH injectés » sous-estimait le sujet
  (linker manquant dans le base + env par toolchain). AGENTS.md corrigé en conséquence.

### kydah-code est un client de la sandbox

kydah-code (extension VS Code pour code-server) consomme le MCP de la sandbox pour donner
à Qwen l'accès aux vrais outils (builds, filesystem, terminal) sans saturer le pod code-server.
Le Owner dans ce cas référence le PVC existant de code-server — pas de nouveau stockage.
Fonctionne uniquement quand kydah-code tourne dans un code-server K8s (service interne).

### Contrôleur — bootstrap engagé

CRDs Owner/Project/Sandbox v1alpha1. Owner : identité + PVC home RWX + ServiceAccount
(identité utilisée par kydah-code ET l'app pour accéder aux sandboxes du Owner). Project :
repo git + PVC workspace RWO bloc local (rust-analyzer/openvscode-server ont besoin
d'inotify, ne traverse pas les FS réseau). Sandbox : projection d'une branche (git worktree)
+ toolchains en image volumes. Reconciler Owner (PVC home/SA/status) déjà implémenté.

### harness-core — cœur LLM/MCP name-keyed (terminé)

Refonte complète de `vanyline-lib` : domaine name-keyed (`Provider`, `ModelProfile`,
`McpServer`, `Toolset`, `Agent`, `SkillMeta` — plus d'UUID exposé), `ConfigStore` (trait de
résolution par nom), `ChatEvent`/`EventSink` (un seul type d'événement pour REPL/WS/futur
JSON-RPC), `SessionContext`/`run_agent_turn` (point d'entrée unique), tools builtin
`skill`/`task` (subagents avec garde de profondeur). `cli/` et `app/` migrés dessus
(`CliConfigStore` adapte les fichiers JSON existants, `PgConfigStore` adapte le schéma PG
existant — aucun des deux n'a introduit de nouveau stockage, adaptation mécanique
uniquement). Ancien cœur (`ChatSink`/`run_chat_turn`/types UUID-keyed) supprimé. Détails :
`docs/architecture.md` (section "Session engine"). Stratégie qui a bien fonctionné : tâches
additives strictes (nouveaux modules, jamais toucher l'existant) jusqu'à une tâche finale de
bascule mécanique — le workspace est resté vert après chaque tâche, permettant une revue
incrémentale fiable. Dette assumée et documentée (pas streaming WS live, pas
d'annulation, historique appauvri) plutôt que du scope creep pour "bien faire tout de suite".

### cli-harness — configuration YAML deux couches (terminé)

Le CLI a son vrai stockage natif : `FsConfigStore` (`cli/src/fs_store.rs`) implémente
`ConfigStore` sur deux couches YAML — globale (`~/.config/vanyline/`) et workspace
(`<racine>/.vanyline/`, découverte en remontant jusqu'à `.vanyline/` ou `.git/`).
`config.yaml` (providers/models/mcp/defaults) fusionne clé par clé ; `agents/<name>.md`
(frontmatter + corps = system prompt), `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`
fusionnent par nom de fichier (workspace remplace intégralement l'homonyme global).
Toutes les commandes CLI (`run`/REPL, `agents|models|toolsets|skills|providers|mcp list`,
`agents show`, `config check`) tournent dessus ; l'ancien `CliConfigStore` (JSON) est
supprimé — rupture assumée, pas de migration automatique. Les conversations ont quitté
`~/.config` pour `~/.local/share/vanyline/` (XDG data) et se référencent par index de
liste ou préfixe d'UUID, plus par UUID complet obligatoire. Détails : `docs/architecture.md`
(section "Configuration CLI").

Dépendance `yaml_serde` (fork maintenu de `serde_yaml`, devenu archivé/non maintenu —
vérifié activement avant de choisir, ne pas repartir de `serde_yaml` par réflexe).

Stratégie : même pattern que harness-core (additif jusqu'à un cutover mécanique final),
mais découpé plus finement que prévu par le design initial — chaque tâche candidate du
design (`fs-store`, `commands`) s'est révélée trop large pour la règle des 30-45 min et a
été éclatée en sous-tâches (02a/02b/02c, 04a/04b/04c/04d) au fil de l'implémentation, pas
anticipées à l'avance. Fonctionne bien : découper *pendant* l'exécution dès qu'une tâche
candidate touche plusieurs formats/fichiers indépendants, plutôt que de figer le découpage
dans le design doc.

### Outillage — délégation à Qwen via `llm-exec`

Depuis la feature cli-rpc-stdio (2026-07-12), Claude délègue directement à
Qwen via `llm-exec` (plus de passe-plat humain) : fichier de tâche écrit
dans `.tasks/<feature>/`, lancé en arrière-plan, revu puis committé par
Claude après coup (l'agent opencode `implement` a `git commit*: ask` dans
ses permissions — bloquant en exécution non-interactive, donc Qwen
implémente + valide, ne commit jamais).

Deux pièges d'environnement rencontrés au premier essai :

- **`opencode run` échoue avec `Session not found`** si lancé depuis le Bash
  tool de Claude Code sur cette machine — l'environnement hérite
  `OPENCODE_SERVER_PASSWORD`/`OPENCODE_BINARY` du pod code-server, et leur
  présence fait échouer la création de session (mécanisme de contrôle
  serveur sans rapport avec un `run` ponctuel). Un terminal interactif
  classique n'a pas ce problème (confirmé : même commande, résultat
  différent). **Fix** : préfixer `env -u OPENCODE_SERVER_PASSWORD -u
  OPENCODE_BINARY` devant `llm-exec`/`opencode run` quand l'appel vient du
  Bash tool.
- **Modèle** : malgré la consigne globale d'utiliser `strix/qwen3.6:27b`
  (dense), le développeur préfère `strix/qwen3.6:35b-a3b` (MoE) pour ces
  délégations — le dense est trop lent en pratique sur le Strix. Toujours
  passer `-m` explicitement (l'auto-découverte reste à éviter), juste avec
  ce tag-là par défaut sur ce projet.

---

## Contexte et philosophie

Projet né de deux tentatives précédentes (kydah-ai, kydah-code) qui ont montré les limites
des environnements LLM contraints en RAM/CPU. Philosophie : les langages interprétés vont
devoir réduire de voilure — Rust est un choix délibéré pour la maîtrise des ressources.
vanyline est une couche d'exécution gérée et K8s-native que plusieurs outils peuvent consommer.

---

## Scope — phase actuelle

harness-core et cli-harness terminés (cli sur son vrai stockage YAML deux couches,
ancien `CliConfigStore` JSON supprimé). Plusieurs workstreams avancent en parallèle :
app-harness-parity (stockage PG natif côté app, en cours — migrations et `PgConfigStore`
avancés, statut exact à vérifier avant de s'appuyer dessus), tools-v2 (refonte SLM-friendly
de `vanyline-tools`, 8 outils finaux), sandbox-bootstrap (image podman + confinement des
chemins + glue MCP), controller-bootstrap (reconcilers Owner/Project/Sandbox avancés).
Convergence CLI ↔ app : cli-rpc-stdio (JSON-RPC stdio) démarré le 2026-07-12
sur la branche `feature/cli-rpc-stdio` (design doc commité, tâche 1/4
`rpc-skeleton` en cours via Qwen) ; `vscode-ext-bootstrap.md` pas encore
démarré, dépend de la stabilisation des commandes CLI.

**Hors scope pour cette phase :**
- Intégration app ↔ sandbox (nécessite le controller à maturité)
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
