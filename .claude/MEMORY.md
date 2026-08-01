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
- **Récidive du piège `/tmp` hors whitelist** (ws13-sandbox-runtime, tâche
  `image-cmds`, 2026-08-01) : un fichier de tâche qui demandait à Qwen
  d'écrire un Dockerfile de validation sous `/tmp/<nom>/` (au lieu de
  `/tmp/opencode/*`, seul préfixe `/tmp` whitelisté dans
  `external_directory` de `implement.md`) a fait échouer le `mkdir`
  (`auto-rejecting`) — la session ne plante pas cette fois (juste le step
  de validation qui ne s'exécute pas), mais rien n'est commité. Déjà
  documenté une fois pour de la *lecture* hors repo (cf. plus haut,
  ws08) ; ici c'est de l'*écriture* d'un fichier scratch, même cause
  racine. **Réflexe à appliquer systématiquement en rédigeant un fichier
  de tâche** : tout chemin scratch/temporaire donné à Qwen doit être soit
  `/tmp/opencode/*`, soit un chemin dans le repo (ex.
  `.tasks/<feature>/scratch/`) — jamais un `/tmp/<nom-libre>/` inventé au
  moment de la rédaction. Traité comme blocage d'outillage réel (pas un
  bug de code) : validé et committé directement par Claude plutôt que
  re-délégué.

### ws09-sandbox-maint-agent — vanyline-maint (terminé)

Binaire `vanyline-maint` dans l'image sandbox (`sandbox/src/maint.rs` + wrapper
clap `sandbox/src/bin/maint.rs`) : `init`/`fetch`/`purge`/`checkout`/`remove` +
stub `detect` (WS-10). Les 5 Jobs git du controller invoquent ce binaire en
argv — plus aucun `sh -c` dans `controller/` (R1 clos), presets toolchain deux
arches (R2). Erreurs `VNL-MAINT-001..005`. Détails : `docs/architecture.md`
section "Maintenance des workspaces". 4 tâches (3 Qwen + docs par Claude),
506 tests au total en fin de feature (474 au départ).

Leçons de délégation spécifiques (complètent la section Outillage ci-dessus) :

- **Les apostrophes françaises dans un message de commit donné verbatim font
  planter Qwen en boucle sur le quoting bash** (tâche 1 : 3 tentatives
  échouées puis crash de session sur un `Write /tmp/...` auto-rejeté — /tmp
  est hors whitelist `external_directory`). Parade qui marche, appliquée dès
  la tâche 2 : message **sans apostrophes/accents** + procédure imposée dans
  la section Commit du fichier de tâche : écrire le message dans
  `.tasks/commit-msg.txt` (chemin DANS le repo), `git commit -F`, supprimer
  le fichier. Zéro échec ensuite.
- **Vérifier le préfixe du message même avec la procédure -F** : tâche 3
  committée avec `(featur: ...)` au lieu de `(feat: ...)` — typo de Qwen dans
  le fichier de message, corrigée par `git commit --amend` (sûr : rien n'est
  poussé avant la clôture).
- Le compte de tests annoncé par Qwen était encore une fois faux/périmé
  (tâche 2 : "493" annoncés, 505 réels) — la règle "toujours recompter
  soi-même" reste valable.

### ws11-sandbox-git — endpoints `/git/status` et `/git/unpushed` (terminé)

`GET /git/status` (parse pur de `git status --porcelain=v2 --branch`) et `GET
/git/unpushed` (compare `HEAD` à `origin/<branche>` ou, sans upstream, à
`origin/<default>`) — `sandbox/src/git.rs`, mêmes middlewares que `/mcp`.
Détails architecturaux, schémas JSON, codes d'erreur `VNL-SBX-004..006` :
`docs/architecture.md` section "Endpoints git".

**Deux prérequis architecturaux découverts avant la première tâche** (pas
dans le design initial — trouvés en vérifiant l'hypothèse "les commandes
git tournent dans `VNL_SANDBOX_ROOT`" contre le mount réel du pod, avant
d'écrire la moindre ligne de code) :

1. **`repo.git` invisible dans le pod sandbox** : `git worktree add` écrit
   un pointeur `.git` **absolu** (`gitdir: /workspace/repo.git/worktrees/<sandbox>`,
   vérifié avec un vrai `git worktree add` local) ; le pod sandbox ne
   montait que le subPath `worktrees/<sandbox>` → toute commande git y
   échouait déjà, bug préexistant à `controller-bootstrap`, jamais débusqué
   faute de test e2e exerçant une vraie commande git. Fix : second
   `VolumeMount` du même volume `workspace` sur `repo.git`
   (`controller/src/sandbox.rs`).
2. **Refspec de fetch manquante, cible corrigée** : le point ouvert légué
   par WS-9 (ci-dessus, maintenant obsolète) proposait
   `+refs/heads/*:refs/heads/*` — **imprécis**, vérifié faux en local :
   ça aurait écrasé les branches locales des worktrees à chaque fetch. La
   bonne cible, confirmée par test réel (`git fetch` avant/après) et
   directement exigée par le design de `/git/unpushed`
   (`refs/remotes/origin/<branche>`) : `+refs/heads/*:refs/remotes/origin/*`,
   posée par `vanyline-maint init` (`git config --replace-all`, idempotent).

Les deux ont été traités comme tâches 00/01 (controller puis sandbox) avant
les tâches produit 02/03 (git-status, git-unpushed) — validés par le
développeur via `AskUserQuestion` avant la moindre implémentation, cf. règle
"jamais modifier les sources avant que le plan de la tâche courante soit
validé". 4 tâches Qwen, aucune ronde de correction nécessaire — chaque
fichier de tâche fournissait un contrat de code quasi complet (vérifié en
local au préalable avec de vrais dépôts git, pas des suppositions sur le
format porcelain v2 ou la topologie bare+worktree) plutôt que des
signatures seules ; a bien fonctionné pour de la logique de parsing/format
externe où l'ambiguïté coûte cher. Compte de tests revérifié par Claude
lui-même après chaque tâche (`cargo test --workspace`, jamais celui du
rapport Qwen) : 509 (tâche 00) → 512 (01) → 525 (02) → 532 (03), 0 échec à
chaque étape.

### controller-bootstrap — WS-4 (terminé, clôturé après coup le 2026-07-31)

`vanyline-controller` sorti du statut déféré : trois CRDs v1alpha1
(Owner/Project/Sandbox) et leurs reconcilers (`owner.rs`/`project.rs`/`sandbox.rs`),
7 tâches candidates du design toutes implémentées (crds, owner-reconciler,
project-jobs-builder, project-reconciler, sandbox-pod-builder, sandbox-reconciler,
deploy). Détails architecturaux : `docs/architecture.md` section "Opérateur
Kubernetes — vanyline-controller". `docs/features/controller-bootstrap.md` supprimé
à la clôture.

**Anomalie de process découverte en reprenant cette feature** : le design doc était
resté en `docs/features/` alors que le code était fini, testé (67 tests) et déployé
depuis le 2026-07-11 (image publiée `docker.io/sebt3/vanyline-controller:0.0.1-alpha.1`,
validée en e2e réel sur le cluster de dev — commit `ef0d3da`, qui a d'ailleurs débusqué
un vrai bug de `PatchParams::force` incompatible avec `Patch::Merge` sur le patch de
status). La Phase 3 (clôture : migration vers `architecture.md` + suppression du
design doc) n'avait jamais été faite alors que WS-9 (sandbox-maint-agent) et WS-8
(github-publication) ont ensuite modifié `controller/` sans jamais y toucher — signe
que la clôture peut se perdre silencieusement quand plusieurs features s'enchaînent
sans repasser explicitement par la Phase 3 de chacune. Réflexe à garder : après toute
tâche qui touche un composant dont le design doc est encore présent, vérifier s'il est
encore d'actualité plutôt que de supposer qu'il a déjà été clos.

### initial-app-frontend et sandbox-bootstrap (terminés, clôturés après coup le 2026-07-31)

Même anomalie que controller-bootstrap, découverte dans la foulée en vérifiant les
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

**app-harness-parity (WS-2) — partiellement clos** : le backend (migrations,
`PgConfigStore`, API REST, WS streaming — tâches 1-4 du design) est fini et migré
dans `docs/architecture.md`. Le frontend (tâches 5-6, `front-crud`/`front-chat`)
n'a jamais été commencé — `frontend/src/` n'a que `Login.svelte`/`Chat.svelte` du
MVP, aucun écran CRUD, `ChatMessage.svelte` ne rend que texte + tool calls à plat
(pas de repli par tool result, pas de badge usage, pas de sous-fil subagent).
`docs/features/app-harness-parity.md` **gardé**, réduit au seul périmètre restant —
ne pas le supprimer tant que le frontend n'est pas fait. Différence clé avec les
trois clôtures ci-dessus : toujours vérifier que TOUTES les tâches candidates du
design ont un équivalent dans le code avant de fermer, pas seulement le backend/la
partie la plus visible — une feature peut être "backend-complete" et rester ouverte.

### ws12-sandbox-clients — client K8s CLI + toolbox (terminé, stop-start différé sur WS-13)

Rend les Owners/Projects/Sandboxes pilotables hors du cluster-admin :
extraction `vanyline-crds` (types CRD, sans runtime kube), `VnlK8sClient`
(`lib/src/k8s.rs`, feature Cargo `k8s` désactivée par défaut), commandes
CLI `owner`/`project`/`sandbox` (`list`/`show`/`create`/`delete`),
méthodes JSON-RPC miroir, toolbox en inférence (`--toolbox`,
`SessionContext.extra_mcp`). Détails : `docs/architecture.md` section
"Client K8s CLI". 10 commits (crate-crds, lib-k8s, cli-owner/project/sandbox,
rpc-owner/project/sandbox, toolbox-lib/cli), 532 → 548 tests, 0 régression
à aucune étape. `docs/features/ws12-sandbox-clients.md` **gardé**, réduit
au seul périmètre restant (`stop`/`start`, bloqué sur WS-13 — champ
`suspended` absent de `SandboxSpec`, pas encore démarré).

**Décisions d'architecture prises en cours de route (pas dans le design
initial)** :

- **Point d'API toolbox** : le design proposait un décorateur `ConfigStore`
  côté CLI pour injecter la sandbox comme serveur MCP sans toucher `lib`.
  Après lecture du code réel (`SessionContext` a déjà `local_tools` comme
  précédent d'"input direct de l'hôte"), tranché pour un champ explicite
  symétrique `extra_mcp: Vec<(McpServer, McpSelection)>` sur
  `SessionContext` plutôt qu'un faux `ConfigStore` — plus petit changement
  lib, testable isolément, pas de mensonge sur le contenu de la config
  pour rejouer une mécanique existante. Bon exemple de "la proposition du
  design n'est qu'une proposition" : elle a changé après inspection du
  code, pas juste actée.
- **CLI `create` : flags complets, pas de `-f fichier.yaml`** — proposition
  initiale du design rejetée par le développeur ("si je dois utiliser un
  fichier, autant kubectl apply -f directement ; chaque commande n'a besoin
  que de peu d'arguments, clap gère ça bien"). Levée à retenir : ne pas
  supposer qu'imiter `kubectl apply -f` est un service rendu quand la
  vraie alternative (`kubectl` lui-même) existe déjà et fait ça mieux —
  la valeur d'un CLI dédié est dans les flags ciblés, pas dans la
  réplication d'un mécanisme générique.
- **`service_name`/`MCP_PORT` déplacés de `controller` vers `vanyline-crds`**
  (retouche de `controller/src/sandbox.rs` une deuxième fois après la tâche
  `crate-crds`) plutôt que dupliqués dans `lib` : `VnlK8sClient::sandbox_mcp_url`
  doit calculer la même URL que celle que le controller pose réellement — une
  seule source de vérité pour un nom de Service + un port, décidé avec le
  développeur plutôt que supposé.
- **Convention de test K8s vs MCP, à ne pas confondre** : les appels
  `Api<K>::list/get/create/delete` de `VnlK8sClient` ne sont JAMAIS
  unit-testés (pas de mock d'API server K8s dans ce projet, même principe
  que les reconcilers du controller) — mais les connexions **MCP**, elles,
  SONT testées avec un vrai serveur HTTP local
  (`lib/tests/mcp_connection_lifecycle.rs`, pattern réutilisé pour
  `extra_mcp` dans `lib/tests/toolbox_extra_mcp.rs`). La distinction tient
  à la légèreté de monter un serveur HTTP en local (trivial, `axum`+`rmcp`
  déjà en dev-deps) contre l'absence d'équivalent léger pour une vraie API
  Kubernetes. Un test qui déclencherait un vrai appel K8s contacterait un
  cluster réel sur la machine d'un développeur avec un kubeconfig valide —
  interdit, rappelé explicitement dans chaque tâche `rpc-*`/`cli-*`.

**Pièges techniques rencontrés (utiles pour tout futur code touchant les
mêmes types)** :

- **`#[derive(CustomResource)]` génère TOUJOURS `status: Option<S>`**,
  jamais `S` nu — même quand le design/le code partiel écrit
  `owner.status.pvc_name` sans `Option`. Bug trouvé dans mes propres
  specs de tâche (`cli-owner`, 2 rounds de fix), pas une erreur
  d'exécution de Qwen — leçon pour la rédaction des tâches futures :
  toujours écrire `match &x.status { Some(s) => ..., None => ... }`
  pour tout type CRD dérivé, jamais d'accès direct.
- **`k8s_openapi::Condition` a `type_: String` (pas `r#type`) et
  `message: String` (pas `Option<String>`, jamais vide via `as_deref`)**
  — vérifié dans les sources vendored, pas une supposition. Même piège
  que ci-dessus, trouvé dans mes propres specs, corrigé en 1 round.
- **`kube` avec `default-features = false, features = ["derive"]`
  n'embarque PAS `kube-client`/`kube-runtime`** (vérifié dans le
  `Cargo.toml` publié de `kube-4.0.0` : `derive = ["kube-derive",
  "kube-core/schema"]`, aucune dépendance sur `client`) — c'est ce qui
  permet à `vanyline-crds` de rester consommable par un CLI léger sans
  tirer la machinerie réseau de l'opérateur. Sans `default-features =
  false` explicite, le `default` de `kube` (`client` + `rustls-tls` +
  `ring`) s'ajoute silencieusement même si on ne déclare que `derive`.

**Découpage en sous-tâches, au-delà du design initial** : les tâches
candidates `cli-commands` (owner+project+sandbox) et `rpc-methods` (idem)
se sont révélées trop larges pour la règle des 30-45 min dès la
rédaction — scindées par type de ressource (03a/03b/03c, 04a/04b/04c) et
`toolbox` scindée en lib/cli (05a/05b) **avant** la première tentative,
pas après un échec — contrairement à `cli-harness` où le découpage
s'était fait pendant l'exécution. Fonctionne aussi bien en amont qu'en
cours de route : le signal ("cette tâche touche 3 types x 4 opérations
quasi identiques") est détectable à la lecture du design, pas seulement
à l'usage.

**Nouveaux modes d'échec Qwen observés (complètent la section Outillage
plus haut)** :

- **Qwen peut corrompre du code SANS RAPPORT avec la tâche en cours en
  éditant maladroitement autour** (`cli-owner`, tâche 03a) : en ajoutant
  du nouveau code en fin de `main.rs`, une édition antérieure dans le
  fichier a supprimé la fin d'une fonction préexistante (`run_agent`,
  branche `Show`) sans lien avec la tâche — `cargo check` a révélé un
  "unclosed delimiter" loin de la zone éditée. Contrairement aux échecs
  précédents (permission auto-rejetée, apostrophes) qui bloquent la
  session AVANT tout commit, celui-ci laisse un diff non commité mais
  DANGEREUX si on ne vérifiait pas `cargo check` avant de committer.
  Fix : tâche de correction chirurgicale (diff exact old/new fourni) —
  même principe que "corriger en déléguant", mais formulé comme
  continuation de la tâche interrompue (même message de commit final),
  pas comme un fix séparé, puisque rien n'avait encore été commité.
- **Qwen ignore parfois le format de commit même avec la consigne
  explicite ET la procédure `-F`** (`rpc-owner`, 04a : message
  `feat(cli): ...` au lieu de `(feat: ws12-sandbox-clients) rpc-owner`)
  — 3ème confirmation sur ce projet de la fiabilité limitée du
  formatting de commit délégué. Fix : `git reset --soft HEAD~1` +
  recommit avec le bon message (sûr, rien n'est poussé) — a aussi permis
  de corriger au passage un fichier non lié agrégé dans le même commit
  (`docs/features/ws15-quality-hygiene.md`, non tracké, sans rapport)
  malgré la consigne explicite "jamais `git add -A`".
- **Qwen peut recopier une valeur d'exemple sensible dans la doc générée**
  (04a : un prénom réel utilisé comme nom d'exemple `Owner` dans
  `docs/rpc-protocol.md`, alors que la règle "pas de prénom dans les
  fichiers commités" vit dans le CLAUDE.md global du développeur, invisible
  de Qwen). Depuis : rappel explicite dans chaque prompt de délégation
  suivant ("jamais de prénom réel dans les exemples, noms génériques
  type alice/demo-project") — Qwen n'a plus reproduit ensuite. Leçon
  généralisable : toute règle vivant dans un CLAUDE.md privé et invisible
  de Qwen doit être rappelée explicitement dans le prompt de délégation
  si elle peut s'appliquer au contenu généré (docs, exemples), pas
  seulement au code.

### ws15-quality-hygiene — gouvernance qualité CI (terminé)

Quatre jobs CI (`.github/workflows/test.yml`) : `doc-lint` (`missing_docs`, cliquet
bloquant en régression, 6 crates non-cli, baseline 621), `unwrap_used`/`expect_used`
(pas de job dédié — les 6 crates non-cli sont directement en `#![deny(...)]`, le job à
cliquet temporaire `unwrap-lint` supprimé une fois tous propres), `clippy-pedantic`
(non bloquant, baseline 585), `coverage` (première mesure sans seuil, 74,97 % lignes,
push `main` seulement). Détails techniques complets : `docs/architecture.md` section
"Gouvernance qualité — jobs CI (WS-15)". `docs/features/ws15-quality-hygiene.md`
supprimé à la clôture. 18 commits, 548 tests, 0 régression à chaque étape.

**Le design V1 (produit par un autre agent, "laguna s2.1") était truffé de chiffres
fabriqués** — pas juste imprécis, carrément faux et non vérifiables : "0 test" pour
`tools`/`controller` (mesurés à 73/65), "2131 tests" sur `sandbox` seul (total workspace
réel : 548), "~175 unwrap() en production" (mesuré à 35, et les 5 exemples cités à
l'appui pointaient vers du code de test, pas de la production), un ratio doc "lignes
`///` / items pub" qui dépasse 100% sur 3 crates sur 5. Le crate `crds` (7ᵉ membre du
workspace) n'apparaissait dans aucun relevé. Confirme la valeur de relire un design
produit par un autre agent commande par commande avant de l'utiliser comme base — cf.
`docs/architecture.md` pour les chiffres corrigés et reproductibles.

**Trois pièges techniques vérifiés empiriquement en cours de route** (détail complet et
commandes dans `docs/architecture.md`, section dédiée) :
- `#![warn(X)]` en source a une précédence absolue sur tout flag CLI `-A`/`-D` — a cassé
  le job `clippy` par défaut (`-D warnings`) pendant plusieurs commits sans être détecté,
  faute d'avoir rejoué la vraie commande CI après chaque tâche (`cargo check` seul ne
  suffit pas).
- `cargo check`/`cargo test` n'exécutent **jamais** les lints clippy — un
  `#![deny(clippy::X)]` peut être silencieusement cassé par du code de test sans que ni
  l'un ni l'autre ne le détecte. Seul `cargo clippy --workspace --all-targets -- -D
  warnings` (la commande CI réelle) fait foi.
- La compilation incrémentale de rustc/cargo sous-compte les diagnostics de façon non
  déterministe selon l'état du cache (`missing_docs` : 109 vs 169 réel sur `sandbox` ;
  pedantic+nursery : 583 vs 959 réel) — `CARGO_INCREMENTAL=0` obligatoire pour toute
  mesure de warnings, en local comme en CI.

**Nouveau mode d'échec Qwen, spécifique à cette feature** : le modèle sous-jacent
(context window 131K tokens) peut échouer par compaction de contexte sur une tâche qui
touche beaucoup de fichiers volumineux, même bien spécifiée — la session se compacte en
cours de route et finit par poser une question au lieu d'agir (pas un appel d'outil
bloqué par `question: deny`, juste du texte de fin de tour ; le résumé post-compaction
peut aussi halluciner des détails, ex. citer des crates "frontend"/"harness-core"
inexistants dans ce contexte). Une tâche combinant `sandbox`+`controller` (~6000 lignes
à lire) a échoué deux fois avant scindage par crate (task-05a/05b). **Quand le contrat
d'une tâche est déjà entièrement écrit et le risque de récidive élevé, appliquer
directement les modifications (Claude, via Edit) plutôt que de multiplier les tentatives
de délégation est plus efficace** — pas un problème de spécification qu'une réécriture
peut résoudre, une limite matérielle de l'outil. Seule feature du projet où Claude a
appliqué du code directement plutôt que de déléguer à Qwen, et le contexte explique
pourquoi (pas un contournement du workflow, une réponse à un blocage réel constaté deux
fois de suite).

---

### ws13-sandbox-runtime — socle CLI, egress trois niveaux, suspension manuelle (terminé)

Trois consolidations indépendantes du runtime sandbox, 5 tâches Qwen (image-cmds,
crds-egress, netpol-builder, netpol-sandbox-reconcile + netpol-cascade-bump en
04a/04b, suspended), 548 → 564 tests, 0 régression à chaque étape. Détails
architecturaux migrés dans `docs/architecture.md` (section "Serveur MCP" pour le
socle CLI, section "Opérateur Kubernetes" pour les sous-sections "NetworkPolicies
egress à trois niveaux" et "Suspension manuelle"). `docs/features/ws13-sandbox-runtime.md`
supprimé à la clôture.

**Deux erreurs du design initial corrigées avant la première tâche** (Phase 1,
via `AskUserQuestion` avec le développeur, avant tout code) :

- Le design affirmait qu'un mécanisme de watch inter-CRD (Sandbox watchant
  Owner/Project) existait déjà pour propager un changement d'egress —
  **vérifié faux** (`grep -rn ".watches("` sur tout `controller/src` : rien,
  chaque reconciler ne surveille que sa propre CRD). Toujours vérifier une
  affirmation d'architecture du design contre le code réel avant de l'utiliser
  comme prémisse d'une tâche — un design doc peut contenir des suppositions
  jamais vérifiées, pas seulement des décisions actées.
- Mécanisme retenu à la place (proposé par le développeur après une comparaison
  coût/latence objective demandée explicitement) : pas de nouveau watch
  permanent. Un changement sur `Sandbox.spec` se réconcilie déjà immédiatement
  (watch natif kube-runtime sur sa propre CRD) ; pour qu'un changement sur
  `Owner.spec.egress`/`Project.spec.egress` se propage aussi vite, `owner.rs`/
  `project.rs` patchent une annotation de bump sur leurs Sandboxes à **chaque**
  reconcile (inconditionnel, pas de détection de changement — décision
  explicite du développeur : cohérent avec le reste du controller qui ne diff
  jamais rien nulle part, le coût est borné par l'intervalle de requeue déjà
  en place). "Meilleur des deux mondes" dans les mots du développeur : réaction
  quasi immédiate, zéro coût d'API-server supplémentaire en régime stable.

**Décision de sécurité tranchée avec le développeur, pas supposée** : la règle
DNS toujours présente dans la netpol egress (indispensable — sans elle toute
white-list casse la résolution DNS) **n'a aucune restriction de destination**
(pas de `podSelector`/`namespaceSelector` ciblant kube-dns). Alternative
rejetée (cibler kube-dns précisément, `namespaceSelector: kube-system` +
`podSelector: k8s-app=kube-dns`) : plus restrictif mais suppose une convention
de labels du cluster jamais vérifiée — une erreur y aurait cassé silencieusement
la résolution DNS de toute sandbox à egress restreint. Le développeur a choisi
l'option robuste plutôt que l'option précise quand l'écart de risque était
asymétrique à ce point.

**Cadence de délégation à Qwen sur cette feature** : 6 lancements consécutifs
sans aucun échec de code (tous les diffs produits collaient exactement aux
contrats fournis, vérifiés par relecture systématique après chaque tâche —
`git show`, jamais de confiance aveugle même sur un run qui se dit "propre").
Deux frictions rencontrées, toutes deux des blocages d'outillage plutôt que
des bugs de code, traitées en validant/committant directement (règle
existante "sauf blocage réel d'outillage") plutôt qu'en re-déléguant :
- Récidive du piège `external_directory` hors whitelist, cette fois en
  **écriture** d'un scratch file (`/tmp/<nom-libre>/` au lieu de
  `/tmp/opencode/*`) — déjà documenté une fois pour de la lecture (ws08),
  reconfirme qu'il faut vérifier ce point à chaque rédaction de tâche, pas
  seulement s'y fier de mémoire.
- Compaction de contexte mi-session sur une tâche pourtant petite
  (`crds-egress`, 5 fichiers, diffs courts) — le fichier
  `.tasks/commit-msg.txt` créé plus tôt dans la même session n'était "plus
  trouvé" après coup et Qwen s'est arrêté en posant une question de
  confirmation (jamais répondue, session non interactive). Contrairement au
  mode d'échec déjà documenté pour ws15 (grosse tâche, contexte 131K
  saturé d'un coup), ici la compaction a eu lieu en cours de session normale
  — signe que ce mode d'échec n'est pas strictement corrélé à la taille de
  la tâche, à surveiller même sur des tâches courtes.

**Découpage `netpol-reconcile` en 04a/04b décidé avant la première tentative**
(pas après un échec, contrairement à `cli-harness`) : la tâche candidate du
design mélangeait deux fichiers/reconcilers distincts (Sandbox pour
l'application de la netpol, Owner+Project pour la cascade de propagation) —
signal détecté à la rédaction, même pattern que `ws12-sandbox-clients`.

---

## Contexte et philosophie

Projet né de deux tentatives précédentes (kydah-ai, kydah-code) qui ont montré les limites
des environnements LLM contraints en RAM/CPU. Philosophie : les langages interprétés vont
devoir réduire de voilure — Rust est un choix délibéré pour la maîtrise des ressources.
vanyline est une couche d'exécution gérée et K8s-native que plusieurs outils peuvent consommer.

---

## Scope — phase actuelle

harness-core, cli-harness, cli-rpc-stdio, ws07-review-fixes,
ws08-github-publication, ws09-sandbox-maint-agent, controller-bootstrap (WS-4)
et ws11-sandbox-git terminés (cli sur son vrai stockage YAML deux
couches, ancien `CliConfigStore` JSON supprimé ; serveur JSON-RPC stdio
complet, cf. section dédiée plus haut ; review sprint 1 R3-R16 corrigées —
R1/R2 closes par WS-9 (vanyline-maint, cf. section dédiée) ; repo publiable — Dockerfiles à
leur place, `deploy/` trié, CI de validation et de release GitHub Actions,
README étendu, cf. `docs/architecture.md` section "Limites connues" pour le
détail des deux dettes révélées en route ; `vanyline-controller` sorti du
statut déféré — trois CRDs Owner/Project/Sandbox réconciliées, déployé et
validé en e2e sur le cluster de dev, cf. `docs/architecture.md` section
"Opérateur Kubernetes" et section dédiée ci-dessus pour l'anomalie de clôture
tardive découverte en reprenant cette feature — détails dans
`docs/architecture.md`, `docs/features/ws07-review-fixes.md`,
`docs/features/ws08-github-publication.md` et
`docs/features/controller-bootstrap.md` supprimés après clôture) ;
ws11-sandbox-git — `GET /git/status`/`GET /git/unpushed` sur la sandbox, plus
deux fixes architecturaux prérequis (mount `repo.git` dans le pod sandbox,
refspec de fetch dans `vanyline-maint init`) découverts avant la première
tâche, cf. section dédiée ci-dessus, `docs/features/ws11-sandbox-git.md`
supprimé après clôture ; de même,
initial-app-frontend (MVP) et sandbox-bootstrap (WS-3, image podman +
confinement des chemins + glue MCP) étaient terminés sans jamais avoir été
clos — cf. section dédiée ci-dessus, détails migrés dans `docs/architecture.md`
sections "Backend web" et "Serveur MCP", les deux design docs supprimés.
app-harness-parity (WS-2) est **partiellement** clos : backend fini et migré
dans `docs/architecture.md`, frontend (`front-crud`/`front-chat`) jamais
commencé — `docs/features/app-harness-parity.md` gardé, réduit à ce périmètre
restant. tools-v2 (refonte SLM-friendly de `vanyline-tools`, 8 outils finaux)
avance en parallèle. Convergence CLI ↔ app : `vscode-ext-bootstrap.md`
(extension VS Code, consomme le RPC stdio) pas encore démarré — c'est la
prochaine étape naturelle maintenant que le RPC stdio est en place.

ws12-sandbox-clients (client K8s CLI : `vanyline-crds`, `VnlK8sClient`,
commandes owner/project/sandbox, méthodes RPC miroir, toolbox
`--toolbox`) **partiellement** clos, même statut qu'app-harness-parity :
tout sauf `stop`/`start` est fini et migré dans `docs/architecture.md`
(section "Client K8s CLI") — `docs/features/ws12-sandbox-clients.md`
gardé, réduit au seul périmètre `stop-start`. **Déblocage** : WS-13 a
ajouté `SandboxSpec.suspended` (cf. ci-dessous) — le seul reste est le
câblage CLI (`vanyline sandbox stop|start` = patch du champ), pas encore
fait, prochaine étape naturelle pour clore cette feature.

ws15-quality-hygiene (gouvernance qualité CI : doc-lint, deny unwrap/expect
sur les 6 crates non-cli, clippy-pedantic non bloquant, coverage baseline)
**terminée et close** — cf. section dédiée ci-dessus, détails migrés dans
`docs/architecture.md` section "Gouvernance qualité — jobs CI (WS-15)",
design doc supprimé. `cargo clippy --workspace --all-targets -- -D warnings`
vert, 548 tests au moment de sa clôture.

ws13-sandbox-runtime (socle CLI sandbox étendu, `NetworkPolicy` egress à
trois niveaux Owner/Project/Sandbox avec propagation par bump d'annotation,
suspension manuelle `SandboxSpec.suspended`) **terminée et close** — cf.
section dédiée ci-dessus, détails migrés dans `docs/architecture.md`
sections "Serveur MCP" et "Opérateur Kubernetes", design doc supprimé.
564 tests, `cargo clippy --workspace --all-targets` vert.

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
