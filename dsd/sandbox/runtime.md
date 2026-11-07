# Solution Design: Exécution de la sandbox

## Résumé

La Solution `sandbox/runtime` rend le pod sandbox exécutable : image de base (`sandbox/Dockerfile`), images toolchain
montées en `volumes[].image` (`toolchains/`), environnement qui les rend utilisables (presets et agrégation calculés par
le controller, `controller/src/sandbox.sdd`), overlay `.venv/` des processus enfants (`sandbox/src/venv.rs`), outil
`execute_command` (`tools/src/command.rs`), puis configuration et démarrage du binaire `vanyline-sandbox`
(`sandbox/src/config.rs`, `main.rs`, `lib.rs`).

Hors Solution : authentification (`sandbox/access`), MCP, git, LSP et éditeur (`sandbox/mcp`, `sandbox/git`,
`sandbox/editor`), santé du code (`sandbox/code-health`), modèle de menace de l'agent (`sandbox#OI-1`, `sandbox#BR-9`).

Document en REVERSE : il décrit ce que les specs et fichiers cités contractent. Les écarts entre sources sont des
Open Issues (TBD), sans arbitrage.

## Problème et objectif

Un agent de code et un développeur doivent compiler, tester et lancer des outils dans le pod, avec les bons binaires
et les bonnes variables, sans rien installer au runtime : le volume toolchain est en lecture seule (`sandbox#BR-5`).
L'objectif est que la chaîne image → pod → processus tienne ce contrat, et que chaque maillon soit sourcé.

## Périmètre et non-objectifs

Couvert : image de base et utilisateur d'exécution ; images toolchain (contenu, montage, publication) ; agrégation de
`PATH`, `LD_LIBRARY_PATH` et des clés propres ; overlay `.venv/` ; `execute_command` (exécution, timeout, sortie
bornée) ; configuration et ordre de démarrage du serveur.

Non-objectifs : installation de paquets au runtime (interdite par construction) ; cycle de vie du pod, Service et
Ingress (`workspaces/sandboxes`) ; validation du jeton d'Owner (`sandbox/access`) ; lancement du LSP (`VNL_LSP_TOOLCHAINS`
n'est lu ici qu'au démarrage) ; isolation noyau et quotas (`sandbox#OI-2`, `workspaces#OI-3`).

## Contexte applicable du domaine

- `sandbox#BR-5` : une toolchain devient utilisable par injection d'environnement ; l'inscriptible est hors volume. Détaillé en FR-2 à FR-7.
- `sandbox#BR-6` : confinement des chemins, appliqué au `cwd` de `execute_command` (FR-11).
- `sandbox#BR-4` : `/health` et `/metrics` sur un port distinct, sans authentification, jamais publié (FR-15).
- `sandbox#BR-9`, `sandbox#DEC-3` : les effets de l'agent sont bornés au pod.
- `sandbox#CON-1` à `sandbox#CON-3` : images OCI en `volumes[].image`, substrat C commun, outils de santé épinglés.
- `sandbox#NFR-1` : identifiants `VNL-SBX-*` (voir OI-7).
- `workspaces#BR-1` : un Owner = un namespace contenant son PVC home, monté dans le pod.

## Acteurs

- **Agent de code :** exécute des commandes par `execute_command` du MCP de la sandbox.
- **Développeur :** terminal PTY de l'éditeur web, qui reçoit le même overlay `.venv/` (`venv.sdd`).
- **Controller :** compose le pod, monte les toolchains, pose leur environnement (`controller/src/sandbox.sdd`).
- **Mainteneur des images :** Dockerfiles et matrice de publication (`.github/workflows/release.yml`).

## État actuel et état cible

- **Actuel (Observed in: `sandbox/src/`, `tools/src/`, `toolchains/`, `controller/src/sandbox.sdd`) :**
  - `venv.sdd` et `command.sdd` décrivent un comportement implémenté, avec tests par Scenario pour une partie d'entre eux ;
  - `config.sdd` demande de remplacer le code existant (`oidc_*`, `no_auth`, `static_token`) : `Tasks` non cochées ; les images `rust`, `node`, `python`, `docker` existent, une seule est spécifiée.
- **Cible :** le contrat de `sandbox/src/config.sdd` et l'ordre de démarrage de `sandbox/src/main.sdd`. Aucun autre état cible n'est déclaré dans les sources de cette Solution.

## Hypothèses, contraintes et dépendances

- **CON-1:** Les toolchains sont montées en `volumes[].image` (Kubernetes ≥ 1.31, GA en 1.36).
    - **Source:** SpecDD /controller/src/sandbox.sdd (Supposer à l'exécution) ; `sandbox#CON-1`
- **CON-2:** Le volume toolchain est en lecture seule et le process est non-root : toute installation au runtime échoue en `Read-only file system`.
    - **Source:** `toolchains/rust/Dockerfile` (commentaire d'en-tête) ; SpecDD /toolchains/toolchains.sdd (Must not)
- **CON-3:** Image de base et images toolchain sont sur la même famille de distribution (trixie) ; le loader du base résout la glibc du volume.
    - **Source:** `sandbox#CON-1` ; `sandbox/Dockerfile` ; `toolchains/python/Dockerfile` (commentaire)
- **CON-4:** L'UID d'exécution est 1000 ; le home Linux correspond au chemin de montage du PVC home (`/home/vanyline`).
    - **Source:** `sandbox/Dockerfile` (commentaires) ; `controller/src/project.rs` (`SANDBOX_UID`) ; SpecDD /controller/src/owner.sdd (`HOME_MOUNT_PATH`)

## Exigences fonctionnelles

- **FR-1 - MUST:** L'image de base contient le substrat C commun (`cc`, `ld`, binutils, `libc-dev`, `make`, `pkg-config`), `git`, `curl`, `vim`, et s'exécute sous `vanyline` (UID 1000, home `/home/vanyline`, sans création de home).
    - **Source:** `sandbox/Dockerfile` ; `AGENTS.md`, section `sandbox/` ; `sandbox#CON-2`
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** Chaque toolchain est une image OCI montée en lecture seule sous `/toolchains/<nom>` ; `{root}` dans ses valeurs d'environnement vaut ce chemin.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, `{root}`, `@toolchain_root`)
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** Les outils d'une toolchain (rust-analyzer, `cargo-llvm-cov` avec `llvm-tools-preview`, `ruff`, `hadolint`, serveurs LSP) sont installés au build, jamais au runtime ; les outils de vérification de santé sont épinglés à une version exacte.
    - **Source:** SpecDD /toolchains/toolchains.sdd (Must, Must not) ; `toolchains/rust/Dockerfile` (`--version 0.9.1`) ; `toolchains/python/Dockerfile` (ruff 0.16.6, SHA256) ; `toolchains/docker/Dockerfile` (hadolint v2.15.1) ; `sandbox#CON-3`
    - **Verified by:** `AC-2`
- **FR-4 - MUST:** `PATH` = segments `PATH` des toolchains dans l'ordre de la liste, suivis du `BASE_PATH` standard, toujours émis. `LD_LIBRARY_PATH` = segments concaténés sans suffixe, émis s'il existe au moins un segment.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, `@aggregate_toolchain_env`)
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** Un preset est choisi par nom exact (`rust`, `node`, `docker`, `python`) quand `Toolchain.env` est vide ; un `env` explicite remplace le preset entièrement ; un nom inconnu ne pose aucune variable ; les autres clés sont posées telles quelles, la dernière toolchain gagnant.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, `@toolchain_preset`, `@resolve_toolchain_env`)
    - **Verified by:** `AC-4`
- **FR-6 - MUST:** Le preset `python` place le user-site dans le home de l'Owner : `PYTHONUSERBASE=/home/vanyline/.local`, `PIP_USER=1`, et le segment `/home/vanyline/.local/bin` littéral dans `PATH`, sans `{root}`, car le volume est en lecture seule.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must, preset `python`) ; `docs/architecture.md`, section « Presets toolchain »
    - **Verified by:** `AC-5`
- **FR-7 - MUST:** Le serveur ne calcule ni ne pose aucune variable de toolchain : il hérite de l'environnement du pod. Hors variables de configuration, il ne lit au démarrage que `VNL_LSP_TOOLCHAINS`.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Purpose : l'injection d'env est portée par le controller) ; SpecDD /sandbox/src/main.sdd (étape 7, Accepts)
    - **Verified by:** `AC-3`
- **FR-8 - MUST:** Si `<sandbox_root>/.venv/pyvenv.cfg` est un fichier régulier (lien symbolique vers un fichier accepté), l'overlay vaut exactement `VIRTUAL_ENV=<sandbox_root>/.venv` puis `PATH=<sandbox_root>/.venv/bin:<PATH du process>` ; sinon il est vide. Le contenu du marqueur n'est pas lu.
    - **Source:** SpecDD /sandbox/src/venv.sdd (Returns, Handles, Must)
    - **Verified by:** `AC-6`
- **FR-9 - MUST:** L'overlay est recalculé à chaque appel, sans cache, et seul `<sandbox_root>/.venv` est reconnu (ni remontée ni recherche récursive).
    - **Source:** SpecDD /sandbox/src/venv.sdd (Must, Must not, Handles)
    - **Verified by:** `AC-7`
- **FR-10 - MUST:** L'overlay ne s'applique qu'aux processus enfants de `execute_command` et du terminal PTY, jamais au LSP ; le processus serveur ne modifie pas son propre environnement.
    - **Source:** SpecDD /sandbox/src/venv.sdd (Purpose, Must, Must not)
    - **Verified by:** `AC-8`
- **FR-11 - MUST:** `execute_command` lance `sh -c <command>` dans un groupe de processus dédié ; un `cwd` absent vaut `sandbox_root` et tout `cwd` passe par le confinement ; `timeout_secs` > 0 tue le groupe entier à l'expiration.
    - **Source:** SpecDD /tools/src/command.sdd (Handles, Must) ; SpecDD /sandbox/src/tools_impl.sdd (`confine`) ; `sandbox#BR-6`
    - **Verified by:** `AC-9`, `AC-10`
- **FR-12 - MUST:** Le champ `envs` n'est jamais désérialisé depuis les arguments du LLM ; une clé `envs` reçue est ignorée sans erreur.
    - **Source:** SpecDD /tools/src/command.sdd (Must, Must not, Scenario `envs_jamais_deserialisees`)
    - **Verified by:** `AC-11`
- **FR-13 - MUST:** Le résultat est une chaîne en quatre parties (`exit code`, `duration`, `stdout`, `stderr`) ; chaque flux est borné à 8 KiB en tête et queue ; un code de sortie non nul est un succès (`Ok`).
    - **Source:** SpecDD /tools/src/command.sdd (Returns)
    - **Verified by:** `AC-12`
- **FR-14 - MUST:** Le démarrage suit l'ordre : parse, initialisation de la télémétrie, validation, liaison du port d'exploitation, état partagé, routeur, liaison du port principal, TLS éventuel.
    - **Source:** SpecDD /sandbox/src/main.sdd (Must, étapes 1 à 13)
    - **Verified by:** `AC-13`
- **FR-15 - MUST:** Le port d'exploitation (défaut `0.0.0.0:9090`) est lié avant le principal (défaut `0.0.0.0:3000`) ; un échec de liaison, une adresse invalide ou deux adresses égales sont fatals (sortie 1).
    - **Source:** SpecDD /sandbox/src/main.sdd (Must, Handles) ; SpecDD /sandbox/src/config.sdd (Exposes) ; SpecDD /sandbox/src/lib.sdd (`spawn_ops_server`)
    - **Verified by:** `AC-14`, `AC-15`
- **FR-16 - MUST:** Chaque drapeau a un nom long, une variable d'environnement et un défaut compilé ; priorité drapeau > variable > défaut ; une erreur clap sort avec le code 2 ; la configuration est lue une seule fois, au parse, depuis argv et l'environnement.
    - **Source:** SpecDD /sandbox/src/config.sdd (Purpose, Raises, Must)
    - **Verified by:** `AC-15`
- **FR-17 - MUST:** `validate` refuse le démarrage si `owner_subject` (forme `system:serviceaccount:<ns>:<nom>`) ou `token_issuer` est absent ou vide, si `token_audience` est vide, si les deux sources de clés sont posées, ou si une source est vide. Aucun drapeau ne désactive la vérification du jeton.
    - **Source:** SpecDD /sandbox/src/config.sdd (Raises, Must not, Scenario `aucun_drapeau_de_contournement`) ; `sandbox#BR-1`
    - **Verified by:** `AC-16`
- **FR-18 - MUST:** Le TLS est actif si et seulement si la paire `--tls-cert` / `--tls-key` est fournie ; une seule des deux est une erreur fatale.
    - **Source:** SpecDD /sandbox/src/main.sdd (Must, Raises)
    - **Verified by:** `AC-17`
- **FR-19 - MUST:** Chaque image toolchain (`rust`, `node`, `docker`, `python`) est construite par la matrice de publication, avec le même tag que `app`, `sandbox` et `controller`.
    - **Source:** `.github/workflows/release.yml` (matrice `toolchains-*`) ; commentaires des `toolchains/*/Dockerfile`
    - **Verified by:** `AC-18`

## Sécurité

- `execute_command` est un shell : métacaractères et injection de commande sont possibles par conception (SpecDD /tools/src/command.sdd, Accepts). Les bornes sont celles du pod (`sandbox#BR-9`, DEC-5) ; la liste de ce que l'agent ne doit jamais faire relève de `sandbox#OI-1`, non traitée ici.
- `envs` n'est pas injectable par le LLM (FR-12) : l'overlay vient donc de la configuration du process uniquement.
- Le port d'exploitation n'est jamais publié (`sandbox#BR-4`), et le serveur refuse de démarrer si ce port ne peut pas être lié (FR-15).

## Conception

```text
sandbox/Dockerfile ──────────────► image vanyline-sandbox (UID 1000, cc/ld, git, curl, vim, python3, ...)
toolchains/<nom>/Dockerfile ─────► image toolchain (outils au build, lecture seule au runtime)
                                      │ volumes[].image → /toolchains/<nom>
controller (sandbox.sdd) ────────► pod : PATH, LD_LIBRARY_PATH, clés de preset, VNL_SANDBOX_ROOT, VNL_LSP_TOOLCHAINS
                                      │
vanyline-sandbox ────────────────► parse → telemetry → validate → port ops → état → routeur → port principal (TLS ?)
                                      │
execute_command ─────────────────► cwd confiné, overlay .venv (recalculé), envs ignoré → sh -c (groupe) → sortie bornée
```

- **Pas d'installation au runtime.** Les outils vivent dans les images ; ce qui s'écrit vit dans le PVC de l'Owner ou dans un cache du Project (`/project-cache/<nom>`, `controller/src/sandbox.sdd`).
- **L'environnement est un contrat de pod.** Le controller écrit `PATH` et `LD_LIBRARY_PATH` dans la spec ; le serveur hérite. Les presets sont donc dans `controller/src/sandbox.rs` (`toolchain_preset`).
- **Démarrage sans demi-état.** La validation précède toute écoute ; le port d'exploitation est lié avant le principal.

## Décisions

- **DEC-1 - Toolchains en images OCI lues seules, outils cuits au build**
    - **Context:** `rustup component add` échoue dans le pod (`Read-only file system`) : le volume est en lecture seule par propriété Kubernetes.
    - **Decision:** chaque toolchain est une image publiée avec le monorepo ; les outils sont ajoutés à son `Dockerfile`.
    - **Source:** `toolchains/rust/Dockerfile` (commentaire) ; `docs/architecture.md`, section « Images toolchain = images LSP, décidé après coup (2026-08-20) »

- **DEC-2 - L'environnement des toolchains est posé par le controller**
    - **Context:** la liste des toolchains effectives est dérivée ou explicite dans la CR Sandbox, donc connue du controller.
    - **Decision:** le controller calcule `PATH`, `LD_LIBRARY_PATH` et les clés de preset dans la spec du pod ; le serveur hérite.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Purpose, toolchains effectives) ; `sandbox#BR-5`

- **DEC-3 - Presets nommés, remplacement intégral**
    - **Context:** une toolchain avec un `env` explicite ne doit pas hériter d'un preset partiel.
    - **Decision:** `Toolchain.env` non vide remplace le preset ; jamais de fusion.
    - **Source:** SpecDD /controller/src/sandbox.sdd (Must)

- **DEC-4 - Overlay `.venv/` sans cache ni remontée**
    - **Decision:** recalcul à chaque appel, racine seule, marqueur = fichier `pyvenv.cfg` ; un `.venv` créé en session est vu à la commande suivante.
    - **Source:** SpecDD /sandbox/src/venv.sdd (Purpose, Handles)

- **DEC-5 - `execute_command` est un shell, pas un exec argv**
    - **Context:** l'agent doit pouvoir enchaîner des commandes et lancer des scripts d'installation.
    - **Decision:** `sh -c` sans validation de la commande ; les effets sont bornés au pod.
    - **Consequences:** toute restriction de l'agent passe par le modèle de menace (`sandbox#OI-1`, `sandbox#BR-9`).
    - **Source:** SpecDD /tools/src/command.sdd (Accepts, « PAR DESIGN ») ; `sandbox#DEC-3`

- **DEC-6 - Utilisateur non-root sans création de home**
    - **Context:** le home Linux doit correspondre au montage du PVC de l'Owner.
    - **Decision:** utilisateur `vanyline` UID 1000, `--home-dir /home/vanyline`, sans `--create-home`.
    - **Source:** `sandbox/Dockerfile` (commentaires) ; SpecDD /controller/src/owner.sdd (`HOME_MOUNT_PATH`)

## Réalisation SpecDD

- `SpecDD /sandbox/src/venv.sdd` → `sandbox/src/venv.rs` (FR-8 à FR-10).
- `SpecDD /tools/src/command.sdd` → `tools/src/command.rs` (FR-11 à FR-13).
- `SpecDD /sandbox/src/config.sdd` → `sandbox/src/config.rs` (FR-15 à FR-17).
- `SpecDD /sandbox/src/main.sdd` → `sandbox/src/main.rs` (FR-14, FR-15, FR-18).
- `SpecDD /sandbox/src/lib.sdd` → `sandbox/src/lib.rs` (FR-15 pour les deux ports seulement).
- `SpecDD /controller/src/sandbox.sdd` → `controller/src/sandbox.rs` (FR-2, FR-4 à FR-7 ; Solution `workspaces/sandboxes`, citée pour l'env).
- `SpecDD /toolchains/toolchains.sdd` → `toolchains/rust/` (FR-2, FR-3 pour la seule image rust).
- Sans spec : `sandbox/Dockerfile`, `toolchains/{node,python,docker}/`, `.github/workflows/release.yml` (FR-1, FR-19) ; `deploy/sandbox/sandbox-test.yaml` est une fixture (OI-2, OI-4).

## Acceptation et vérification

- **AC-1:** Une image issue de `sandbox/Dockerfile` démarre sous l'UID 1000 `vanyline`, home `/home/vanyline`, avec `cc`, `make`, `pkg-config`, `git`, `vim`.
- **AC-2:** Toolchain `rust` montée en lecture seule : `cargo llvm-cov --version` réussit, `llvm-tools` est installé, l'alias `stable-<host>` existe (`toolchains/rust/test-toolchain.sh`, `test-toolchain-health.sh` ; Scenarios `cargo_llvm_cov_utilisable_en_lecture_seule`, `llvm_tools_presents`).
- **AC-3:** Un pod `rust` + `node` reçoit un `PATH` dont les segments précèdent le `BASE_PATH` (Scenario `agregat_path_concatene_dans_ordre_des_toolchains`) ; le serveur ne pose pas ces variables lui-même.
- **AC-4:** Un nom inconnu ne produit aucune variable ; un `env` explicite remplace le preset (Scenarios `agregat_toolchain_inconnue_path_egal_base_path`, `agregat_env_explicite_remplace_le_preset_entierement`).
- **AC-5:** Le preset `python` pose `PYTHONUSERBASE=/home/vanyline/.local`, `PIP_USER=1` et le segment littéral (Scenario `preset_python_user_site_et_pip`).
- **AC-6:** Avec `.venv/pyvenv.cfg` fichier, l'overlay vaut exactement `[VIRTUAL_ENV, PATH]` (Scenario `overlay_present_exactement_virtual_env_puis_path`).
- **AC-7:** Un `.venv` créé entre deux appels est vu au second ; un lien symbolique vers un fichier est un marqueur ; deux appels rendent le même `PATH` (Scenarios `venv_cree_entre_deux_appels_vu_des_le_second`, `pyvenv_cfg_symlink_vers_fichier_est_un_marqueur`, `deux_appels_successifs_path_identique`).
- **AC-8:** L'exclusion du LSP et la pureté de l'environnement serveur sont des `Must` / `Must not` sans Scenario dédié dans `venv.sdd`.
- **AC-9:** Un `cwd` vide hérite de `sandbox_root` ; un `cwd` inexistant ou fichier régulier rend une erreur (Scenarios `cwd_applique_au_process`, `cwd_inexistant_file_not_found_avec_hint`, `cwd_fichier_regulier_not_a_directory`).
- **AC-10:** Un timeout tue le groupe, descendants compris (Scenario `timeout_tue_groupe_et_descendants`).
- **AC-11:** Une clé `envs` dans le JSON est ignorée (Scenario `envs_jamais_deserialisees`) ; `tools_call_execute_command_envs_non_injectables` est référencé par `venv.sdd`.
- **AC-12:** Format à quatre parties et borne de 8 KiB par flux (Scenarios `stdout_borne_tete_et_queue_8kib`, `sortie_formatee_quatre_sections`, partiel selon `command.sdd`).
- **AC-13:** L'ordre de démarrage est celui de `main.sdd`. Vérification : TBD, le harnais de `main.rs` n'est pas écrit (`main.sdd`, Tasks).
- **AC-14:** Port d'exploitation occupé : erreur (Scenario `spawn_ops_server_sur_port_occupe_rend_erreur`, `sandbox/tests/http.rs`) ; la preuve binaire (exit 1) est reportée au harnais.
- **AC-15:** Défauts `ports_par_defaut`, `sandbox_root_par_defaut`, `cooldown_par_defaut` et erreurs clap (`config.sdd`) ; conversion en tests non cochée.
- **AC-16:** Scenarios `owner_subject_requis`, `issuer_requis`, `sources_de_cles_exclusives`, `source_vide_refusee`, `aucun_drapeau_de_contournement` (`config.sdd`).
- **AC-17:** `tls_cert_sans_cle_exit_1` est listé parmi les tests à écrire (`main.sdd`, Tasks) : TBD.
- **AC-18:** La matrice `release.yml` construit les quatre images toolchain ; aucun test de publication dans les sources.

Vérification actuelle : tests de `venv.rs`, des presets (`controller/src/sandbox.rs`, ex. `toolchain_preset_python_env`) et de
`command.rs` ; `sandbox/tests/http.rs` cité par les specs. Aucun test du démarrage binaire ni de l'image de base n'est référencé.

## Open Issues

- **OI-1 - Emplacement de `CARGO_HOME` (TBD) :** `AGENTS.md` (sandbox) et `sandbox#BR-5` / `sandbox#AC-3` placent cargo dans le PVC de l'Owner ; `controller/src/sandbox.sdd` pose `CARGO_HOME=/project-cache/cargo` (cache du Project, sous-chemin du PVC workspace) ; `deploy/sandbox/sandbox-test.yaml` pose `/workspace/.cargo`. Le preset `rust` ne pose pas la variable. Voisin de `workspaces/sandboxes#OI-14`.
- **OI-2 - Racine de confinement (TBD) :** `config.sdd` et le controller (`VNL_SANDBOX_ROOT=/home/vanyline/workspace`) divergent de `deploy/sandbox/sandbox-test.yaml` (`/workspace`) ; la mise à jour du manifeste est une Tâche non cochée de `config.sdd`.
- **OI-3 - Timeout par défaut de `execute_command` (TBD) :** `command.sdd` fixe le défaut serde à 0 (illimité) ; le schéma MCP annonce « default 30 » ; le harnais CLI force 30 ; le dispatch sandbox ne force rien. Sans `timeout_secs`, aucune limite (question ouverte dans `command.sdd`).
- **OI-4 - Images du manifeste de test (TBD) :** `deploy/sandbox/sandbox-test.yaml` monte `docker.io/library/rust:slim-trixie` et `node:trixie-slim`, et non les images `vanyline-toolchains-*` publiées. Fixture obsolète ou contrat voulu ?
- **OI-5 - Version de l'image python (TBD) :** `toolchains/python/Dockerfile` utilise `python:3.14-slim-trixie` ; son commentaire et `docs/architecture.md` disent `3.13`.
- **OI-6 - Couverture des specs toolchain (TBD) :** `toolchains/toolchains.sdd` ne possède que `./rust/` ; node, python et docker n'ont pas de spec (Tasks non cochée). `sandbox#OI-4` reste ouvert pour l'outillage JS.
- **OI-7 - Préfixe d'identifiant d'erreur (TBD) :** `sandbox#NFR-1` et `AGENTS.md` exigent `VNL-SBX-*` ; `command.sdd` utilise `VNL-TLS-008` et `VNL-TLS-009` ; les messages de validation de `config.sdd` n'ont aucun identifiant.
- **OI-8 - Épinglage des outils LSP (TBD) :** `sandbox#CON-3` exige des versions épinglées pour les outils de santé ; `typescript`, `typescript-language-server` et `pyright` sont installés sans version (`toolchains/node`, `toolchains/python`). Périmètre voulu ?
- **OI-9 - Architecture du `LD_LIBRARY_PATH` (TBD) :** `AGENTS.md` décrit `…/usr/lib/<arch>-linux-gnu` ; le preset ajoute `aarch64`, alors que les images sont amd64 seulement (commentaires des Dockerfiles, `release.yml`).
- **OI-10 - Overlay `.venv/` (TBD) :** `venv.sdd` ouvre six points : `PATH` vide produit `<bin>:` ; `bin/` non vérifié ; erreurs I/O avalées sans log ; marqueur vide accepté ; chemin non-UTF-8 rendu en U+FFFD ; paramètre `cwd` de `spawn_shell`. Ils attendent une décision du développeur principal avant d'être un contrat.
- **OI-11 - Sortie et annulation de `execute_command` (TBD) :** message « partial output kept » faux (sortie jetée) ; annulation sans kill de groupe ; stdin hérité (blocage possible) ; code `-1` pour un signal ; permission sur `cwd` remontée en `VNL-TLS-009` (`command.sdd`).
- **OI-12 - Démarrage (TBD) :** `main.sdd` ouvre : arrêt gracieux absent (flush OTel perdu) ; appairage TLS vérifié après le lancement du port d'exploitation ; adresses loguées avant bind (port 0 illisible) ; `MCP_LISTEN=""` accepté par clap puis refusé ; exemption `cfg(test)` attribuée à `main.rs` par `tooling.sdd` mais absente du fichier.
- **OI-13 - Décompte des drapeaux (TBD) :** `main.sdd` parle de seize drapeaux longs, `config.sdd` de dix-sept champs (dont `health_base_ref`). Lequel fait foi ?
- **OI-14 - Code de configuration non aligné (TBD) :** `config.sdd` demande de supprimer `oidc_*`, `no_auth`, `static_token` du code existant ; Tâche non cochée. FR-16 et FR-17 ne sont donc pas encore tenus par le code. Dette voisine : `sandbox#OI-5`.

## Références

- `dsd/sandbox.md` : `sandbox#BO-1`, `BR-1`, `BR-4`, `BR-5`, `BR-6`, `BR-9`, `CON-1` à `CON-3`, `NFR-1`, `DEC-3`, `OI-1`, `OI-2`, `OI-4`, `OI-5`, `AC-3`.
- `dsd/workspaces.md` : `workspaces#BR-1`, `workspaces#OI-3` ; `dsd/workspaces/sandboxes.md` : `workspaces/sandboxes#OI-14`.
- SpecDD /sandbox/src/venv.sdd, /sandbox/src/config.sdd, /sandbox/src/main.sdd, /sandbox/src/lib.sdd, /sandbox/src/tools_impl.sdd, /tools/src/command.sdd, /controller/src/sandbox.sdd, /controller/src/owner.sdd, /toolchains/toolchains.sdd.
- `sandbox/Dockerfile`, `toolchains/{rust,node,python,docker}/Dockerfile`, `.github/workflows/release.yml`, `deploy/sandbox/sandbox-test.yaml`, `docs/architecture.md`, `AGENTS.md`.
