# Roadmap — sprint 2 : publication, sandbox utile, langages

Plan programme du sprint. Même modèle d'exécution que le sprint 1, désormais rodé :
**Fable planifie** (ce document + design docs + fichiers de tâches), **Qwen code**
(`llm-exec`, agent `implement`), **Sonnet valide et pilote** (review après chaque
délégation, `cargo check/test/clippy` systématique — Qwen ne peut pas s'auto-valider).
Les deux modes éprouvés au sprint 1 restent valides : deux features en parallèle
(code sur A pendant review sur B) ou pilotage autonome d'une feature de bout en bout.

## Objectif

Rendre le projet **publiable** (CI GitHub, README, images multi-arch, correctifs de
la review sprint 1) et rendre la sandbox **réellement utile** : langages détectés,
validation outillée, intégration git, pilotage depuis le CLI et l'app.

## Règle figée ce sprint

**L'image sandbox est l'outil de maintenance du controller.** Les jobs git
(init/fetch/purge/checkout) tournent déjà dans l'image sandbox — on fige la règle et
on la consolide : un petit utilitaire dédié dans l'image (`vanyline-maint`,
sous-commandes init/fetch/purge/checkout/detect) remplace les scripts shell inline.
Double bénéfice : ça corrige l'injection shell (anomalie R1 — les champs de CRD
passent en argv, plus jamais dans un `sh -c`) et ça porte la détection de langages
(WS-10) au bon endroit — là où le repo est cloné et rafraîchi.

## Correctifs issus de la review sprint 1 (WS-7)

Review complète du diff sprint 1 (~21 300 lignes, 98 fichiers) faite le 2026-07-12.
Qualité d'ensemble bonne (taxonomie d'erreurs appliquée, confinement sandbox solide,
couverture de tests réelle). Anomalies à corriger, par sévérité :

| # | Sévérité | Anomalie | Localisation |
|---|----------|----------|--------------|
| R1 | majeure | Injection shell : `spec.branch`, `spec.repo_url` interpolés bruts dans `sh -c` des jobs git | `controller/src/sandbox.rs:311`, `controller/src/project.rs:293` |
| R2 | majeure | Presets toolchain hardcodés `x86_64-linux-gnu` — casse arm64 (objectif CI du sprint) | `controller/src/sandbox.rs:56,61` |
| R3 | majeure | `McpServer.headers` jamais appliqués à la connexion — l'auth MCP configurée est ignorée silencieusement | `lib/src/prefixed_mcp.rs:226` |
| R4 | majeure | WS app : le tour LLM bloque la boucle de lecture du socket (pas de Close ni de verrou busy pendant un tour) | `app/src/ws/chat.rs:89-118` |
| R5 | moyenne | `ChatEvent::ToolResult.is_error` toujours `false` — les échecs d'outils ne sont jamais signalés aux consommateurs | `lib/src/event.rs:145` |
| R6 | moyenne | Le tool builtin `skill` charge n'importe quel skill du store, hors `SkillSelection` de l'agent | `lib/src/builtin/skill.rs:64` |
| R7 | moyenne | `provider.api_key` ignoré pour `ProviderType::Ollama` (proxy Ollama-compatible avec clé impossible) | `lib/src/model.rs:15` |
| R8 | moyenne | Timeout de commande : tue `sh` mais pas ses petits-enfants (pas de process group) | `tools/src/command.rs:68` |
| R9 | moyenne | Divergence de persistance sur échec de tour : l'app garde le message user, le RPC ne garde rien | `app/src/ws/chat.rs:163` vs `cli/src/rpc/handlers.rs:556` |
| R10 | mineure | `assemble_system_prompt` : system_prompt vide + autres sections → séparateur en tête | `lib/src/session.rs:85` |
| R11 | mineure | Tools locaux / serveurs MCP dupliqués si référencés par plusieurs toolsets (add_tool double, connexion MCP double) | `lib/src/session.rs:308` |
| R12 | mineure | Connexions MCP rouvertes à chaque tour, jamais fermées explicitement | `lib/src/prefixed_mcp.rs` |
| R13 | mineure | `state.seq` RPC jamais purgé pour les conversations supprimées (croissance non bornée) | `cli/src/rpc/handlers.rs:30` |
| R14 | mineure | `unwrap()` sur `oidc_issuer` dans `get_jwks_uri` — vérifier la garde de config | `sandbox/src/auth.rs:64` |
| R15 | mineure | `write_file`/`edit_file` non atomiques (pas de temp+rename) | `tools/src/filesystem.rs:286` |
| R16 | mineure | `GIT_SSH_COMMAND` avec `StrictHostKeyChecking=no` — à documenter avant publication | `controller/src/project.rs:255` |

R1 et R2 sont absorbées par WS-9 (utilitaire de maintenance). R3-R9 = tâches Qwen
fines en début de sprint. R10-R16 = au fil de l'eau, en tâches d'appoint.
Non couvert par cette review : le frontend (diff minime au sprint 1).

## Workstreams et dépendances

```
WS-6  vscode-ext-bootstrap (ext/)   ─── REPORTÉ du sprint 1, dépend de WS-1b (livré) → jour 1
WS-7  review-fixes (transverse)     ─── indépendant → jour 1, avant tout gros chantier sur les mêmes fichiers
WS-8  github-publication            ─── indépendant → jour 1 (CI d'abord : tout le sprint en profite)
WS-9  sandbox-maint-agent (ctrl+sbx)─── indépendant → jour 1 ; absorbe R1/R2 ; débloque WS-10
WS-10 language-support (ctrl+tools+sbx) ─ dépend de WS-9 (détection) ; le tool validate est parallélisable
WS-11 sandbox-git (sandbox)         ─── indépendant
WS-12 sandbox-clients (lib+cli)     ─── indépendant (CRDs livrés au sprint 1) ; la toolbox dépend du MCP sandbox (livré)
WS-13 sandbox-runtime (ctrl+sbx)    ─── netpols/stop-start : controller ; commandes : image sandbox
WS-14 cli-backend-llm-exec (étude)  ─── indépendant, fin de sprint
```

### WS-8 — github-publication
- `app/Dockerfile` créé (sa vraie place), pattern cargo-chef aligné sur
  `sandbox/Dockerfile` et `controller/Dockerfile` (3 images).
  Builder racine supprimé.
- CI GitHub complète, inspirée de `juke/.github/workflows/` : sur PR/push
  `cargo fmt --check` + `clippy` + `test --workspace` + build front ; sur tag
  binaire `vanyline` (CLI) arm64+amd64 (`taiki-e/upload-rust-binary-action`, cross)
  + 3 images multi-arch (`docker/build-push-action`, qemu+buildx, ghcr).
- Tri de `deploy/` en 3 sous-répertoires : `web/`, `controller/`, `sandbox/`.
- `README.md` : les axes du projet + comment déployer app et controller.
- R16 documenté au passage (limites de sécurité connues).

### WS-9 — sandbox-maint-agent
- Figer la règle dans `AGENTS.md`/`docs/architecture.md`.
- `vanyline-maint` (nouveau binaire du sous-workspace sandbox, embarqué dans
  l'image) : init/fetch/purge/checkout/worktree-remove en argv (corrige R1),
  détection d'arch à l'exécution ou presets par arch (corrige R2).
- Le controller invoque l'utilitaire au lieu des scripts inline.

### WS-10 — language-support (épique, périmètre volontairement réduit)
- **2 langages seulement : Rust et JavaScript/TypeScript** — et ça restera ce
  périmètre un bon moment.
- Détection au clonage initial et à chaque maintenance (`vanyline-maint detect`),
  stockée dans le **status du Project** ; en dérive la liste des toolchains à
  monter dans les sandbox du projet (plus besoin de les lister à la main).
- Nouveau tool `validate` dans la crate tools : lance test + lint + formatage pour
  les toolchains détectées, ne remonte au LLM **que les problèmes et les
  statistiques de succès** (sortie bornée, SLM-friendly).
- Derrière le tool, une API de résultats (coverage, problèmes, stats) ; la sandbox
  expose un endpoint dédié pour les remonter à l'utilisateur (frontend plus tard).
  Les résultats vivent **en mémoire de la sandbox** (décision 2026-07-12) — perdus
  au redémarrage du pod, il suffit de relancer `validate`.
- Coverage : **lcov comme format pivot** (décision 2026-07-12). Les deux
  écosystèmes l'exportent nativement (`cargo-llvm-cov --lcov`, vitest/istanbul) ;
  la sandbox le parse en un JSON compact (% global, % par fichier) servi par
  l'endpoint, et le `lcov.info` brut reste disponible — Codecov l'ingère
  nativement, ce qui ouvre l'upload Codecov depuis la CI (WS-8) sans conversion.

### WS-11 — sandbox-git
Endpoint(s) sandbox : liste de ce qui n'est pas commité ; liste des commits absents
du remote. (Le worktree et le bare sont déjà montés, git est dans l'image.)

### WS-12 — sandbox-clients
- La crate lib fournit tout pour manipuler Owners/Projects/Sandboxes (client K8s).
- Le CLI expose les commandes et les méthodes JSON-RPC correspondantes.
- En inférence, le CLI peut cibler une **toolbox** (une sandbox) : les tools locaux
  de la crate tools sont alors remplacés par ceux du MCP de la sandbox.

### WS-13 — sandbox-runtime
- Set de commandes élargi dans l'image de base (décision 2026-07-12, "socle +
  python3") : `ripgrep`, `fd-find`, `jq`, `procps`, `less`, `file`, `tree`,
  `patch`, `diffutils`, `unzip`, `openssh-client`, `ca-certificates`, `python3`
  (~70 Mo — python3 inclus car c'est l'outil réflexe des LLM pour tout script
  ad-hoc). Pas d'outils réseau/debug (dnsutils, netcat, strace) pour l'instant.
- NetworkPolicies 3 niveaux, **egress** (décision 2026-07-12 : ce que la sandbox
  peut atteindre) : Owner, Project et Sandbox peuvent chacun déclarer des
  ouvertures réseau (white-list). Aucune déclaration nulle part → aucune netpol
  egress produite ; au moins une → l'union des trois est appliquée au pod sandbox.
  La netpol ingress par-Owner existante reste inchangée.
- Arrêt/démarrage **manuel** (décision 2026-07-12, pas d'auto-arrêt sur inactivité) :
  une sandbox à conserver (MR pas encore validée) mais inactive peut être stoppée
  (pod supprimé, PVC/worktree conservés) et redémarrée — champ de spec dédié
  (ex. `suspended`), piloté par WS-12 côté CLI.

### WS-14 — cli-backend-llm-exec (étude)
Objectif ferme : le CLI remplace (avantageusement) llm-exec **au sprint 3**.
Livrable de ce sprint : doc d'écart fonctionnel opencode → vanyline CLI (étude
cas-par-cas, ensemble), qui devient le backlog du sprint 3.

Le référentiel est connu (`opencode/packages/core/src/tool`, relevé 2026-07-12) :
`bash, read, write, edit, apply-patch, glob, grep, skill, todowrite, question,
webfetch, websearch`. Couvert par vanyline : bash/read/write/edit/glob/grep
(execute_command, read_file, write_file, edit_file, find_files, search) + skill.
**À étudier cas-par-cas : `todowrite`, `question`, `webfetch`, `websearch`,
`apply-patch`** — plus l'écart hors-tools (injection de contexte, permissions
headless, format de sortie attendu par `llm-exec`).

## Jalons

### M0 — Assainissement
WS-7 : R3-R9 corrigées. WS-9 : règle figée, `vanyline-maint` en place (R1/R2
absorbées). WS-8 : CI de validation verte sur main (fmt/clippy/test).
**Critère de sortie** : plus aucune anomalie majeure ouverte ; toute PR passe la CI.

### M1 — Publication + fondations
WS-8 complet : Dockerfiles à leur place, deploy/ trié, README, release binaire +
3 images multi-arch sur tag. WS-10 : détection de langages visible dans le status
Project. WS-12 : lib + commandes CLI sur Owners/Projects/Sandboxes.
**Critère de sortie** : un `git tag` produit binaire et images publiables ; `vanyline
sandbox list/create/delete` fonctionne contre un cluster.

### M2 — Sandbox utile
WS-10 : `validate` de bout en bout (tool → API → endpoint sandbox). WS-11 :
endpoints git. WS-12 : toolbox en inférence. WS-13 : netpols 3 niveaux,
stop/start, set de commandes élargi.
**Critère de sortie** : depuis le CLI, un agent travaille dans une sandbox (toolbox),
lance `validate`, et l'état git de la sandbox est consultable par endpoint.

### M3 — Convergence
WS-6 : extension VS Code minimale (chat panel Svelte sur le RPC stdio — design doc
du sprint 1 toujours valable). WS-14 : doc d'écart llm-exec. R10-R16 soldées.

## Hors scope (rappel)

- Remplacement effectif de llm-exec (sprint 3 — ce sprint ne fait que l'étude)
- Autres langages que Rust et JS/TS
- Merge/push automatique des branches sandbox ; CRD Application
- Multi-utilisateur complet, quotas ; permissions/approbation des tools
- Compaction automatique du contexte
- Annulation de tour réelle et fidélité de l'historique (tool calls rejoués) —
  dette sprint 1 assumée, pas prioritaire ce sprint

## Décisions tranchées (2026-07-12)

- **Netpols** : egress — ce que la sandbox peut atteindre. L'ingress par-Owner
  existante ne bouge pas.
- **Stop/start** : manuel uniquement.
- **API validate** : résultats en mémoire de la sandbox ; coverage au format
  pivot lcov (compatible Codecov), synthèse JSON par l'endpoint.
- **Set de commandes sandbox** : socle (ripgrep, fd, jq, procps, less, file, tree,
  patch, diffutils, unzip, ssh-client, ca-certificates) + python3.
- **Référentiel WS-14** : les tools opencode de `packages/core/src/tool` (liste
  dans la section WS-14).

Plus aucune question ouverte — les design docs de feature peuvent démarrer.

## Ordre de travail suggéré

1. Jour 1, quatre fronts : WS-8 (CI de validation d'abord), WS-7 (R3-R9), WS-9, WS-6.
2. WS-10 dès que `vanyline-maint` existe ; le tool `validate` peut démarrer avant
   (crate tools, feuille pure).
3. WS-11, WS-12, WS-13 en parallèle au fil du sprint (composants disjoints).
4. WS-14 en fin de sprint, quand le CLI a montré ce qui lui manque en pratique.

## Design docs

Chaque workstream a son design doc (phase 1, véto réciproque développeur/Claude) ;
les fichiers de tâches Qwen en dérivent, une ou deux d'avance maximum.

| WS | Design doc |
|----|------------|
| WS-6 | `features/ws06-vscode-ext-bootstrap.md` (sprint 1, toujours valable) |
| WS-7 | `features/ws07-review-fixes.md` |
| WS-8 | `features/ws08-github-publication.md` |
| WS-9 | `features/ws09-sandbox-maint-agent.md` |
| WS-10 | `features/ws10-language-support.md` |
| WS-11 | `features/ws11-sandbox-git.md` |
| WS-12 | `features/ws12-sandbox-clients.md` |
| WS-13 | `features/ws13-sandbox-runtime.md` |
| WS-14 | `features/ws14-cli-backend-llm-exec.md` |
