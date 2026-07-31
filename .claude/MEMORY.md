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
(`deploy/sandbox-imagevol-*.yaml`, supprimés depuis par WS-8 une fois la recette absorbée par
`sandbox/Dockerfile` et le controller) ont éprouvé node et rust. Recette d'assemblage confirmée :

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

### cli-rpc-stdio — serveur JSON-RPC 2.0 sur stdio (terminé)

`vanyline serve --stdio` (`cli/src/rpc/`) : `initialize`/`shutdown`,
`config/agents|models|toolsets|skills`, `conversations/list|get|create|
delete`, `chat/send` (asynchrone, spawné en tokio pour un vrai parallélisme
inter-conversations — un seul tour actif par conversation, `VNL-RPC-002`
sinon), `chat/cancel` (no-op v1). 110 tests cli au total. Détails
architecturaux : `docs/architecture.md` section "RPC stdio". Spec complète
du protocole (trames, codes d'erreur, piège camelCase/snake_case) :
`docs/rpc-protocol.md`.

**Piège de test découvert (ws07-review-fixes, 2026-07-31)** : `data_dir()`
(`cli/src/config.rs`, via `dirs::data_dir()`) résout `XDG_DATA_HOME` — un
état **global au process**, pas thread-local. `cli/src/rpc/handlers.rs`
a un mécanisme d'isolation dédié (`DATA_DIR_ENV_LOCK` + `isolated_data_dir()`,
juste avant `conversations_list_empty` dans le module `tests`) : **tout
test qui touche `store::` (get/save/delete_conversation) doit appeler
`let (_tmp, _guard) = isolated_data_dir();` en tout premier**, sinon il
est flaky sous `cargo test` parallèle (déterministe à l'échec en run
complet, systématiquement vert isolé ou en `--test-threads=1` — ne pas se
fier à un test lancé seul par son nom pour valider ce genre de fix).
Piège rencontré concrètement : un test préexistant qui ne vérifiait que
`busy`/codes d'erreur (jamais l'état persisté) n'avait jamais eu besoin de
ce mécanisme ; lui ajouter une assertion `store::get_conversation(...)`
l'a rendu flaky sans toucher à sa logique.

Bug réel trouvé en cours de route (pas dans le design, dans
l'implémentation) : `state` détenait un clone du sender mpsc et n'était
droppé qu'après avoir attendu la tâche writer — le process ne sortait
JAMAIS après `shutdown`/EOF (deadlock silencieux, seulement visible via un
test d'intégration qui spawn le vrai binaire, pas via des tests unitaires
sur `handle_line`). Confirme l'utilité d'au moins un test de bout en bout
par le process réel en plus des tests unitaires pour ce genre de code
(cycle de vie / shutdown), qui ne se voit pas en testant la logique pure.

### Outillage — délégation à Qwen via `llm-exec`

Depuis la feature cli-rpc-stdio (2026-07-12, 6 tâches + plusieurs rondes
de correction), Claude délègue directement à Qwen via `llm-exec` (plus de
passe-plat humain) : fichier de tâche écrit dans `.tasks/<feature>/`,
lancé en arrière-plan, revu et validé par Claude après coup, committé par
Claude. Règles stabilisées :

- **Toujours préfixer `env -u OPENCODE_SERVER_PASSWORD -u
  OPENCODE_BINARY`** devant `llm-exec`/`opencode run` lancé depuis le Bash
  tool de Claude Code — sans ça, `opencode run` échoue avec `Session not
  found` (l'environnement hérite ces deux variables du pod code-server ;
  un terminal interactif classique n'a pas le problème). Non lié à un
  process `opencode serve` particulier — c'est la présence des variables
  elles-mêmes qui casse la création de session.
- **Modèle** : `-m "strix/qwen3.6:35b-a3b"` (MoE) par défaut sur ce
  projet, pas le dense `27b` de la consigne globale — trop lent en
  pratique sur le Strix (préférence du développeur). Toujours passer `-m`
  explicitement, ne jamais compter sur l'auto-découverte.
- **Cause racine trouvée et corrigée (2026-07-31)** : les permissions
  `bash: ... allow` de `.opencode/agents/implement.md` n'avaient aucun
  effet parce que le catch-all `"*": ask` était placé en **dernière**
  position du mapping YAML — or la résolution de permission d'opencode
  applique la règle **la dernière qui matche gagne** (confirmé par la doc
  officielle : "Rules are evaluated by pattern match, with the last
  matching rule winning" ; pattern recommandé : catch-all en premier,
  règles spécifiques après). Un `"*": ask` en fin de liste matche tout et
  écrase silencieusement tous les `allow` déclarés au-dessus. Le bloc
  `external_directory` du même fichier avait la bonne structure
  (catch-all d'abord) — c'était une erreur d'ordre isolée au bloc `bash`,
  pas une limitation d'`opencode run` headless. Décision du développeur
  (2026-07-31) : basculer `implement.md` en modèle blacklist — `bash:
  "*": allow` en tête, puis `git push*: deny`, `git rebase*: deny`,
  `sudo*: deny` après. Qwen peut désormais committer et faire tout ce que
  le développeur ferait en bash, sauf push/rebase/sudo — **changement de
  workflow assumé** : l'ancienne règle "Claude committe systématiquement
  après relecture" ne s'applique plus par défaut à `implement`, seule la
  relecture (`cargo check/test/clippy` après chaque délégation) reste
  systématique. `diagnose.md` avait le même bug d'ordre sur son bloc
  `bash` (`"*": deny` en dernier) — corrigé en gardant le modèle whitelist
  (catch-all `deny` remis en premier) car `diagnose` a `edit: deny` et ne
  doit jamais pouvoir écrire de fichier, y compris via un détour bash
  (`sed -i`, redirection `>`) — un blacklist y créerait une fuite de la
  garantie "aucune modification".
- **`external_directory: /home/coder/.config/opencode/*: allow`** ajouté
  aux deux agents (2026-07-31) : Qwen plantait en boucle sur un `ask`
  auto-rejeté en tentant de lire `~/.config/opencode/AGENTS.md` (contexte
  global qu'il connaît par sa propre config opencode). Décision du
  développeur : autoriser quand même, malgré ce fichier contenant un prénom
  en clair (que la règle globale du développeur garde hors des fichiers
  commités) — Qwen est un outil d'exécution locale, pas un tiers externe ;
  le risque réel est qu'il reproduise ce prénom dans un fichier commité ou
  un message généré, à surveiller en review plutôt qu'à bloquer en amont.
- **Une permission auto-rejetée (bash OU external_directory) a une chance
  non négligeable de faire planter toute la session `opencode run`**
  plutôt que de laisser Qwen s'adapter et continuer — observé à 3 reprises
  sur cette feature (deux fois sur des `bash find` avant le fix d'ordre,
  une fois sur un `external_directory` légitimement `ask`). Le fix d'ordre
  bash + l'ajout d'`external_directory` réduisent la fréquence des
  rejets, mais ne garantissent pas qu'un rejet restant (ex: un chemin
  externe vraiment hors périmètre) laisse la session continuer proprement.
  Réflexe : en cas de session qui se termine avec un diff vide ou quasi
  vide après un `! permission requested ... auto-rejecting` dans le log,
  relancer directement la même tâche plutôt que de chercher un bug dans le
  fichier de tâche — c'est souvent juste ce plantage.
- **Corriger en déléguant, pas en patchant direct** (retour du
  développeur, 2026-07-12) : un bug/oubli remonté par la validation de
  Claude devient un fichier de tâche de correction (`<tâche>-fix-NN.md`)
  délégué à Qwen, jamais un `Edit` direct de Claude — sauf blocage réel
  d'outillage où Qwen ne peut de toute façon rien faire de plus (rare : la
  plupart des bugs de code s'y prêtent très bien, vu sur 4 rondes de
  correction consécutives sur `chat/send` sans encombre).
- **Qwen oublie régulièrement d'écrire les tests prévus par la tâche**
  même quand l'implémentation de production est correcte (vu sur 2 tâches
  sur 6) — vérifier le compte de tests avant/après (`cargo test
  --workspace`) plutôt que de faire confiance au rapport final de Qwen ;
  si des tests manquent, fichier de tâche de correction dédié plutôt que
  de les écrire soi-même.
- **Qwen ne respecte pas fiablement le format de message de commit** donné
  verbatim dans la section "Commit" du fichier de tâche (vu sur
  ws08-github-publication, 2026-07-31 : 4 commits sur 7 dans le mauvais
  format — `feat(nom): titre` ou `feat: nom — titre` au lieu de
  `(feat: nom) titre — description`, parfois même sans le nom de la feature
  du tout). Sans conséquence fonctionnelle mais casse la cohérence de
  l'historique. Comme rien n'est poussé avant la fin de la feature (`origin/main`
  très en retard sur ce projet solo), corriger via `git commit --amend`
  (commit de tête) ou `git reset --soft` + recommit (commits plus anciens)
  est sûr — **toujours vérifier le format après chaque délégation**, ne pas
  supposer que la consigne verbatim suffit.
- **Qwen peut stager plus large que le périmètre de la tâche** (vu sur
  ws08-github-publication, tâche `dockerfiles` : le premier commit a
  embarqué `docs/roadmap-sprint2.md`, un fichier non tracké sans rapport,
  probablement via un `git add` trop large côté Qwen). Corrigé une fois
  (reset + recommit ciblé) puis **prévenu en ajoutant une consigne
  explicite dans chaque tâche suivante** ("stager précisément les fichiers
  listés, jamais `git add -A`/`git add .`") — n'a plus reproduit sur les 6
  tâches suivantes. Instruction à inclure systématiquement dans la section
  "Commit" de tout fichier de tâche sur ce projet, tant que le repo contient
  des design docs non trackés en attente (`docs/features/*.md`,
  `docs/roadmap*.md`).
- **`external_directory` en dehors du repo courant peut planter toute la
  session même avec la whitelist déjà élargie** (confirmé à nouveau sur
  ws08-github-publication, tâche `ci-test` v1, 2026-07-31) : un fichier de
  tâche qui demandait à Qwen de lire `~/projets/juke/.github/workflows/`
  (repo voisin, hors périmètre `implement.md`) a fait planter la session sur
  un rejet auto (diff vide, aucun commit). Fix : **ne jamais référencer un
  chemin hors du repo courant dans un fichier de tâche** — si du contenu
  d'un autre projet sert de modèle, le lire soi-même (Claude) et le
  reproduire intégralement dans la section "Code partiel" du fichier de
  tâche plutôt que de renvoyer Qwen le lire.
- **Le compte de tests annoncé par Qwen dans son propre rapport peut être
  faux même quand l'exécution réelle est correcte** (vu sur
  ws08-github-publication, tâche `fmt-repo` : Qwen a annoncé "363 tests
  passés", le vrai chiffre — revérifié par Claude — était 474, identique à
  avant la tâche, aucune régression). Ne jamais citer le chiffre du rapport
  Qwen sans le revérifier soi-même via `cargo test --workspace`.
- **`AGENTS.md` documente des commandes de validation moins strictes que ce
  qu'il faudrait pour une CI propre** : `cargo clippy --workspace` (sans
  `--all-targets`) ne vérifie pas le code de test, jamais remarqué avant
  d'écrire une vraie CI (ws08-github-publication, 2026-07-31) — a révélé
  d'un coup ~20 erreurs préexistantes (surtout un faux positif
  `await_holding_lock` répété sur le pattern `isolated_data_dir()`, cf.
  section cli-rpc-stdio ci-dessus). Idem `cargo fmt --all --check` : jamais
  lancé sur tout le workspace, 53 fichiers non conformes découverts d'un
  coup. Les deux ont nécessité une tâche de nettoyage dédiée avant que la CI
  parte verte. Leçon : **une commande de validation documentée mais jamais
  réellement exécutée sur tout le périmètre n'est pas une garantie** — le
  premier run réel révèle souvent de la dette accumulée silencieusement.

---

## Contexte et philosophie

Projet né de deux tentatives précédentes (kydah-ai, kydah-code) qui ont montré les limites
des environnements LLM contraints en RAM/CPU. Philosophie : les langages interprétés vont
devoir réduire de voilure — Rust est un choix délibéré pour la maîtrise des ressources.
vanyline est une couche d'exécution gérée et K8s-native que plusieurs outils peuvent consommer.

---

## Scope — phase actuelle

harness-core, cli-harness, cli-rpc-stdio, ws07-review-fixes et
ws08-github-publication terminés (cli sur son vrai stockage YAML deux
couches, ancien `CliConfigStore` JSON supprimé ; serveur JSON-RPC stdio
complet, cf. section dédiée plus haut ; review sprint 1 R3-R16 corrigées —
R1/R2 absorbées par WS-9, hors périmètre ; repo publiable — Dockerfiles à
leur place, `deploy/` trié, CI de validation et de release GitHub Actions,
README étendu, cf. `docs/architecture.md` section "Limites connues" pour le
détail des deux dettes révélées en route — détails dans `docs/architecture.md`,
`docs/features/ws07-review-fixes.md` et `docs/features/ws08-github-publication.md`
supprimés après clôture). Plusieurs workstreams avancent
en parallèle : app-harness-parity (stockage PG natif côté app, en cours —
migrations et `PgConfigStore` avancés, statut exact à vérifier avant de
s'appuyer dessus), tools-v2 (refonte SLM-friendly de `vanyline-tools`, 8
outils finaux), sandbox-bootstrap (image podman + confinement des chemins +
glue MCP), controller-bootstrap (reconcilers Owner/Project/Sandbox avancés).
Convergence CLI ↔ app : `vscode-ext-bootstrap.md` (extension VS Code,
consomme le RPC stdio) pas encore démarré — c'est la prochaine étape
naturelle maintenant que le RPC stdio est en place.

**Point ouvert issu de ws08, résolu** : `svelte-check`/storybook (2026-07-31,
branche `fix/svelte-check-skiplibcheck`). Root cause : les fichiers
`*.stories.svelte` importent `@storybook/addon-svelte-csf`, dont les types
importent `storybook/internal/types` (générique multi-renderer, touche
React/Node même en usage Svelte pur) — `frontend/tsconfig.json` ne
définissait pas `skipLibCheck`, donc TypeScript type-checkait aussi ces
`.d.ts` tiers. Fix : `skipLibCheck: true` (réglage standard pour ce cas, pas
un compromis) — 70 erreurs → 0. Étape `check` réactivée dans
`.github/workflows/test.yml`, bullet retiré de `docs/architecture.md`.

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
