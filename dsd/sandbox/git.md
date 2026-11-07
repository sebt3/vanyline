# Solution Design: Git dans la sandbox

## Résumé

La Solution `sandbox/git` réalise la surface git HTTP du serveur de la sandbox : seize routes `/git/*` qui exposent le worktree de l'Owner (état, diff, indexation, commit, branches, fusion, log, push) et la provision de la clé SSH ed25519 de l'Owner. Elle est réalisée par `sandbox/src/git.rs`, dont le comportement est contracté par SpecDD `/sandbox/src/git.sdd`.

Cette Solution n'ajoute aucun contrôle d'accès : tout appelant authentifié par le jeton de l'Owner est l'Owner (`sandbox#BR-1`, DEC-2). Le `git push` n'est pas restreint par la sandbox (`sandbox#DEC-2`). Statut : document de reverse-engineering. Le contrat de `git.sdd` n'est opposable qu'après sa relecture humaine (tâche ouverte, `git.sdd`, Tasks).

## Problème et objectif

L'éditeur web et l'app ont besoin des opérations git courantes sur le worktree de l'Owner sans shell ni accès direct au dépôt ; la sandbox les offre sous forme de routes HTTP, dont les effets restent confinés au pod (`sandbox#BR-9`, `sandbox#DEC-3`). Objectif : fournir ces routes avec des formes JSON stables, des erreurs typées `VNL-SBX-0XX` (`sandbox#NFR-1`), et provisionner la clé SSH que le push utilise. Contribue à `sandbox#FR-3`.

## Périmètre et non-objectifs

Couvert : les seize routes et leurs handlers, les helpers de parsing et de construction d'argv, l'enveloppe `GitError` vers HTTP, la lecture de la clé publique et la génération idempotente de la clé ed25519 dans `$HOME/.ssh`.

Hors périmètre :

- le relais REST `ANY /sandboxes/{name}/git/{*path}` de l'app (`@git_proxy`, SpecDD `/app/src/api/sandboxes.sdd`) : consommateur, décrit ailleurs ;
- le câblage des routes et `require_auth` (SpecDD `/sandbox/src/lib.sdd`, `/sandbox/src/auth.sdd`) et le confinement `confine_path` (SpecDD `/sandbox/src/tools_impl.sdd`) ;
- l'injection de `HOME` et de `GIT_SSH_COMMAND` dans les pods (SpecDD `/controller/src/sandbox.sdd`, SpecDD `/controller/src/project.sdd`) ;
- le modèle de menace de l'agent de code : traité par `sandbox#OI-1`, non rédigé ici ;
- les opérations git non exposées : rebase interactif, cherry-pick, stash, résolution de conflit 3-way, staging par hunk, force-push, graphe multi-branches (`docs/architecture.md`, « Endpoints git »).

## Contexte applicable du domaine

- `sandbox#BR-1` : accès tout ou rien par jeton d'Owner, sans niveau ni groupe. Applicable tel quel.
- `sandbox#BR-2` : le ServiceAccount de l'Owner n'a aucun droit RBAC ; son jeton n'ouvre que la sandbox.
- `sandbox#BR-3` : le navigateur ne voit jamais le jeton ; les routes sont appelées par l'app.
- `sandbox#BR-6` : le confinement de chemin est une garde d'ergonomie, la frontière réelle est le pod.
- `sandbox#BR-9`, `sandbox#DEC-3` : les effets d'un script d'installation ou d'un `build.rs` ne sont pas bornés par cette Solution.
- `sandbox#DEC-2` : le `git push` n'est pas restreint ; la protection des branches relève de la forge.
- `sandbox#FR-3` : exigence domaine couverte par cette Solution (`sandbox#AC-8`).
- `sandbox#NFR-1` : identifiants d'erreur uniques `VNL-SBX-*`.

## Acteurs

- **Owner :** seul appelant accepté ; son jeton est présenté par l'app ou par un opérateur (section Acteurs de `sandbox`).
- **Développeur :** agit depuis l'éditeur web et les relais git de l'app.
- **Agent de code :** agit par `execute_command` et le `git` du shell, sans route ni outil MCP dédiés (`docs/architecture.md`).
- **app :** relaie les routes sans les modifier (`SpecDD /app/src/api/sandboxes.sdd`, `@git_proxy`).
- **Controller :** fournit `HOME` et `GIT_SSH_COMMAND` aux Jobs git (SpecDD `/controller/src/sandbox.sdd`).

## État actuel et état cible

- **Actuel (Observed in: `sandbox/src/git.rs`) :** les seize routes et leurs handlers sont implémentés et contractés par `git.sdd`. Les tests HTTP existent pour `status` et `unpushed` seulement (`sandbox/tests/http.rs` ; `git.sdd`, Tasks).
- **Cible :** diff, commit, branches, fusion et push sont rouverts (`docs/architecture.md`, « Endpoints git », décision `git-integration` du 2026-08-22). Les opérations listées au périmètre restent hors cible.
- **Transitionnel :** l'état de la garde d'Owner est contradictoire entre sources (voir OI-8).

## Hypothèses, contraintes et dépendances

- **CON-1:** Le serveur ne lit ni identité ni niveau d'accès : tout appelant ayant passé `require_auth` est l'Owner.
    - **Source:** `git.sdd`, section Handles (« Absence de garde par niveau »)
- **CON-2:** Les exécutables `git` (porcelain v2, git ≥ 2.11, `for-each-ref` avec `%(upstream:short)`) et `ssh-keygen` sont fournis par l'image sandbox.
    - **Source:** `git.sdd`, Depends on ; présence de `ssh-keygen` dans l'image : TBD (OI-12)
- **CON-3:** Les sous-processus héritent de l'environnement du serveur ; le serveur ne pose aucune variable `GIT_*`. Le répertoire SSH est lu depuis `HOME` à chaque appel, avec repli sur `/home/vanyline`.
    - **Source:** `git.sdd`, Purpose et Accepts (`GET /git/ssh-key`)
- **CON-4:** Le worktree est `config.sandbox_root` (`VNL_SANDBOX_ROOT`, défaut `/workspace`).
    - **Source:** `git.sdd`, Purpose et Depends on
- **CON-5:** Les Jobs git du controller reçoivent `HOME=/home/vanyline` et `GIT_SSH_COMMAND` pointant en dur sur `/home/vanyline/.ssh/id_ed25519`.
    - **Source:** SpecDD `/controller/src/sandbox.sdd` (template des Jobs git, `HOME/GIT_SSH_COMMAND`) ; SpecDD `/controller/src/project.sdd` (`@git_pod_template`) ; `docs/architecture.md`, « Clé SSH git — PVC Owner »
    - **Note:** le Pod sandbox lui-même n'est pas établi par les sources lues (OI-11)

## Exigences fonctionnelles

- **FR-1 - MUST:** Le serveur expose les seize routes `/git/*` (status, unpushed, diff, stage, unstage, commit, branches GET/POST, checkout, DELETE branches, push, log, ssh-key GET/POST, merge, merge/abort), chacune sur sa méthode, derrière `require_auth`.
    - **Source:** `git.sdd`, Must (table de `./lib.sdd`) ; `git.sdd`, Accepts
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** `GET /git/status` rend la branche, les entrées de fichiers (état, `staged`) et `clean` ; `GET /git/unpushed` rend le delta contre `origin/<branche>` ou, sans amont, contre `origin/<défaut>`, tronqué à 200 commits avec `truncated`.
    - **Source:** `git.sdd`, Returns (`/git/status`, `/git/unpushed`) ; `git.sdd`, Must (troncature à 200)
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** `GET /git/diff?path=` rend le patch brut du working tree, ou de l'index si `staged=true`, pour un chemin confiné ; un diff vide est une réponse `200`.
    - **Source:** `git.sdd`, Accepts et Returns (`/git/diff`) ; `sandbox#FR-3`
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** `POST /git/stage` et `POST /git/unstage` n'agissent que sur la liste de chemins fournie (`git add -- …`, `git restore --staged -- …`) ; une liste vide est un no-op `200`.
    - **Source:** `git.sdd`, Handles (stage, unstage) et Must not (`git add -A` interdit)
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** `POST /git/commit` ne commite que l'index (sans `-a`) et rend le SHA complet à 40 caractères hexadécimaux et la première ligne du message ; un index vide est refusé.
    - **Source:** `git.sdd`, Returns (`/git/commit`), Raises (`EmptyCommit`), Must not (commit hors index)
    - **Verified by:** `AC-4`
- **FR-6 - MUST:** `GET`/`POST /git/branches`, `POST /git/checkout` et `DELETE /git/branches/{name}` listent, créent, basculent et suppriment les branches. Le checkout est refusé si l'arbre ou l'index est sale ; les fichiers non suivis ne bloquent pas. La suppression est forcée (`git branch -D`).
    - **Source:** `git.sdd`, Accepts, Returns et Raises (`CheckoutRefused`)
    - **Verified by:** `AC-5`
- **FR-7 - MUST:** `POST /git/merge` rend `200 {conflicted:false, sha}` en cas de fusion automatique, et `200 {conflicted:true, sha:null}` en cas de conflit. `POST /git/merge/abort` annule une fusion engagée.
    - **Source:** `git.sdd`, Returns (`/git/merge`), Must (exit 1 = résultat `200`) ; `docs/architecture.md`, « Endpoints git » (conflit = résultat normal)
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** `GET /git/log` rend au plus `limit` commits (défaut 100) de la branche courante, ou de toutes les refs avec `all=true`, avec `truncated` calculé par une sonde à `limit+1`.
    - **Source:** `git.sdd`, Accepts et Returns (`/git/log`)
    - **Verified by:** `AC-2`
- **FR-9 - MUST:** `POST /git/push` pousse la branche courante vers `origin` (ou la branche par défaut si la branche n'a pas d'homologue distant, OI-2), sans `--force` ni refspec, et rend `{ok:true, pushed}` où `pushed` est compté avant le push.
    - **Source:** `git.sdd`, Handles (push), Must (compte avant push, pas de `--force`)
    - **Verified by:** `AC-6`
- **FR-10 - MUST:** `GET /git/ssh-key` rend `exists` et la clé publique (trimée) sans erreur quand la clé est absente ; `POST /git/ssh-key` génère une clé ed25519 sans passphrase si la clé privée est absente, et rend la clé publique existante sinon.
    - **Source:** `git.sdd`, Returns (ssh-key), Must (idempotence sur le fichier privé) ; `docs/architecture.md`, « Clé SSH git — PVC Owner »
    - **Verified by:** `AC-8`
- **FR-11 - MUST:** Toute erreur de cette Solution répond en JSON `{"error": "<VNL-SBX-0XX: message>"}` ; le statut est `400` pour `InvalidRef` et `500` pour les autres variantes.
    - **Source:** `git.sdd`, Returns (mapping des erreurs), Must ; `sandbox#NFR-1`
    - **Verified by:** `AC-9`
- **FR-12 - MUST:** Tout chemin entrant de diff, stage ou unstage est confiné à la racine du worktree ; un chemin évadant répond `500 VNL-SBX-015`.
    - **Source:** `git.sdd`, Must (`@confine_path`) ; `sandbox#BR-6`
    - **Verified by:** `AC-3`
- **NFR-1 - MUST:** Chaque commande est lancée par argv structuré, sans shell ; les positions de refs (branche, merge, checkout, delete) sont protégées contre les flags par rejet des valeurs commençant par `-` ; diff, stage et unstage portent `--`.
    - **Source:** `git.sdd`, Must et Handles (stratégie anti-injection de flags) ; `docs/architecture.md`, « Endpoints git » (garde `VNL-SBX-014`)
    - **Verified by:** `AC-5`
- **NFR-2 - MUST:** Chaque sous-processus (`git`, `ssh-keygen`) s'exécute dans `spawn_blocking`, jamais sur l'executor asynchrone.
    - **Source:** `git.sdd`, Purpose et Must
    - **Verified by:** TBD (aucun Scenario ; vérification par revue)

## Sécurité

- La clé privée n'est jamais renvoyée, seule la clé publique trimée sort ; le jeton d'Owner et les JWT n'apparaissent dans aucune réponse (`git.sdd`, Must not). Aucune valeur de clé n'est reproduite ici.
- La clé privée est générée sans passphrase (`ssh-keygen -N ""`, `git.sdd`, Must). Cette clé est présente dans le PVC de l'Owner, visible de la sandbox, donc de l'agent de code : conséquence de `sandbox#DEC-2`, et objet de `sandbox#OI-1`.
- L'authentification du push repose sur la clé SSH provisionnée et sur un `known_hosts` pré-rempli hors de ce fichier, ou sur un credential helper de l'image (`git.sdd`, Handles).
- Les permissions de `.ssh` ne sont pas fixées explicitement (`git.sdd`, Tasks `[?]`, OI-9).
- Aucun timeout ne borne un sous-processus bloqué (`git.sdd`, Tasks `[!]`, OI-1).
- Les formes d'opération sont contraintes : jamais de `git add -A`, de `push --force`, de `commit -a` ni de `branch -D` sur une liste implicite (`git.sdd`, Must not). Ces contraintes ne protègent pas les branches de la forge ; elles restent de forme (`sandbox#DEC-2`).

## Conception

Structure : un module `sandbox/src/git.rs` (un seul fichier, un seul contrat). Il contient les handlers, les parseurs de sortie (porcelain v2, log au format `%x1f`, for-each-ref, branches), les constructeurs d'argv, l'enveloppe `GitError`, et la lecture et génération de la clé SSH.

```text
handler axum (git.rs)
  |-- extracteurs (query, JSON)  -> rejet 400/422 avant tout git
  |-- garde de valeur            -> reject_leading_dash, confine_path
  |-- spawn_blocking
  |     `-- std::process::Command (argv structuré, -C <sandbox_root>)
  |-- parseur de sortie          -> ParseFailed (VNL-SBX-005) si inattendu
  `-- GitError -> IntoResponse   -> {"error": "VNL-SBX-0XX: ..."}
```

Runtime, push : `rev-parse --abbrev-ref HEAD` ; `show-ref` de `origin/<branche>` ; si absent, résolution de la branche par défaut ; compte `rev-list --count` ; `git push origin <cible>`. Le compte est renvoyé même si le push échoue ensuite (ordre de `git.sdd`, Must : compte avant push). L'échec est classé par `classify_push_stderr` : `Rejected` avant `AuthFailed`.

Runtime, commit : sonde `git diff --cached --quiet` ; `git commit -m <message>` ; `git log -1` pour le SHA complet.

Interfaces :

- Entrée : routes `/git/*` appelées avec le jeton d'Owner par `@git_proxy` de l'app (`SpecDD /app/src/api/sandboxes.sdd`), passthrough du statut et du JSON. Sortie : sous-processus `git` et `ssh-keygen`, `$HOME/.ssh/`. Aucune persistance propre.

## Décisions

- **DEC-1 - Routes HTTP, pas d'outil MCP git**
    - **Context:** les LLM disposent déjà de `execute_command` et de `git` dans l'image.
    - **Decision:** les opérations git sont exposées par routes HTTP servant l'app et l'éditeur, sans outil MCP dédié.
    - **Alternatives:** un outil MCP git (non retenu, `docs/architecture.md`).
    - **Source:** `docs/architecture.md`, « Endpoints git »
- **DEC-2 - Pas de garde par niveau**
    - **Context:** une garde par niveau créerait un second mécanisme d'autorisation.
    - **Decision:** aucune lecture d'identité ni de niveau dans `git.rs` ; le jeton d'Owner suffit (`sandbox#BR-1`).
    - **Consequences:** toute opération autorisée à l'Owner l'est aussi à quiconque détient son jeton, commit et push inclus.
    - **Source:** `sandbox#DEC-1` ; `git.sdd`, Handles (contrat voulu, décision du 2026-10-04)
- **DEC-3 - Clé SSH dans le PVC de l'Owner, générée à la demande**
    - **Context:** le push doit s'authentifier sans Secret Kubernetes dédié par type de credential.
    - **Decision:** la clé est provisionnée par `POST /git/ssh-key` dans le PVC home ; le POST ne régénère jamais une clé existante.
    - **Alternatives:** Secret Kubernetes monté conditionnellement, déprécié par `Project.spec.git_secret`.
    - **Consequences:** la clé est atteignable par le pod (`sandbox#DEC-2`) ; la régénération casserait les deploy keys déjà enregistrées côté forge.
    - **Source:** `docs/architecture.md`, « Clé SSH git — PVC Owner » (2026-08-22)
- **DEC-4 - Forme du push contrainte, cible non restreinte**
    - **Context:** un push avec refspec ou `--force` peut détruire l'historique distant.
    - **Decision:** `git push origin <cible>` uniquement, sans `--force` ni refspec explicite ; la cible n'est pas restreinte par la sandbox.
    - **Consequences:** la protection de `main` relève de la forge (`sandbox#DEC-2`, `app#DEC-4`).
    - **Source:** `git.sdd`, Accepts (`POST /git/push`) et Must ; `app#DEC-4`
- **DEC-5 - Conflit de fusion = résultat, pas erreur**
    - **Context:** un conflit est un état normal à résoudre dans l'éditeur.
    - **Decision:** exit 1 de `git merge` rend `200 {conflicted:true, sha:null}` ; les autres codes sont `VNL-SBX-012`.
    - **Consequences:** le client distingue conflit et échec par le corps, pas par le statut.
    - **Source:** `git.sdd`, Must ; `docs/architecture.md`, « Endpoints git »
- **DEC-6 - Erreurs-utilisateur rendues en 500, statu quo assumé**
    - **Context:** `EmptyCommit`, `CheckoutRefused`, `DetachedHead`, `NoMergeInProgress` et `InvalidPath` sont des erreurs de l'utilisateur.
    - **Decision:** seul `InvalidRef` produit `400` ; les autres restent `500` jusqu'à arbitrage (OI-4).
    - **Consequences:** le statut ne distingue pas les erreurs de requête des pannes ; le corps `VNL-SBX-0XX` porte la distinction.
    - **Source:** `git.sdd`, Returns et Tasks `[?]` (« statu quo partiel », commentaire de `IntoResponse`)

## Réalisation SpecDD

- `SpecDD /sandbox/src/git.sdd` : contrat complet de cette Solution (FR-1 à FR-12, NFR-1, NFR-2, DEC-1 à DEC-6).
- `sandbox/src/git.rs` : implémentation, `Owns` de `git.sdd`.
- `sandbox/tests/http.rs` : tests d'intégration des routes, dont seuls `status` et `unpushed` sont couverts à ce jour (`git.sdd`, Tasks).
- Dépendances réalisées ailleurs : `SpecDD /sandbox/src/lib.sdd` (câblage des routes, FR-1), `SpecDD /sandbox/src/auth.sdd` (CON-1), `SpecDD /sandbox/src/tools_impl.sdd` (`confine_path`, FR-12), `SpecDD /sandbox/src/config.sdd` (CON-4).
- `SpecDD /app/src/api/sandboxes.sdd` : relais `@git_proxy`, consommateur de cette Solution, non réalisé ici.

## Acceptation et vérification

- **AC-1:** Les seize routes répondent sur leur méthode ; sans jeton la réponse est `401` (garde amont), avec le jeton d'un autre Owner `403`, avec le jeton de l'Owner la requête est acceptée. Scenario `routes_git_sous_require_auth`.
- **AC-2:** `status` rend branche et fichiers ; `unpushed` sur HEAD détaché répond `500 VNL-SBX-006` ; `unpushed` tronqué à 200 avec `truncated`; `log` sans paramètre rend 100 commits. Scenarios `status_route_*`, `unpushed_*`, `log_par_defaut_cent_commits_tri_git_et_branche`.
- **AC-3:** `diff` rend le patch brut ; un chemin évadant répond `500 VNL-SBX-015` ; `stage` d'une liste vide est un no-op ; `stage`/`unstage` produisent `add -- <chemin>` et `restore --staged -- <chemin>`. Scenarios `diff_*`, `stage_*`.
- **AC-4:** `commit` sans index répond `500 VNL-SBX-008` ; `commit` de l'index rend un SHA de 40 caractères et le titre. Scenarios `commit_*`.
- **AC-5:** Une valeur commençant par `-` est refusée en `400 VNL-SBX-014` sans effet git ; `checkout` refusé sur arbre sale en `500 VNL-SBX-011` ; `branches`, `create` et `delete` conformes. Scenarios `refs_avec_tiret_initial_*`, `checkout_refuse_*`, `creation_branche_*`, `suppression_de_branche_*`, `branches_*`.
- **AC-6:** `push` rend `{ok:true, pushed}` avec le compte pris avant le push ; un rejet non fast-forward répond `500 VNL-SBX-009` ; un échec d'identification répond `500 VNL-SBX-010`. Scenarios `push_*`, `classification_stderr_*`.
- **AC-7:** `merge` automatique rend le SHA ; un conflit rend `200 {conflicted:true}` ; une ref inconnue `500 VNL-SBX-012` ; `merge/abort` sans fusion `500 VNL-SBX-013`, avec fusion la termine. Scenarios `merge_*`.
- **AC-8:** `GET ssh-key` sans clé rend `exists:false` sans erreur ; deux `POST` successifs rendent la même clé publique, commençant par `ssh-ed25519 ` ; la réponse ne contient jamais la clé privée. Scenarios `ssh_key_*`.
- **AC-9:** Toute erreur est un JSON `{"error": "VNL-SBX-0XX: …"}` ; `400` uniquement pour `InvalidRef`. Scenario `toute_erreur_git_est_enveloppee_en_json_error`.

État de la vérification : les Scenarios existent dans `git.sdd` ; les tests HTTP ne couvrent que `status` et `unpushed`. Les autres AC restent sans test de route (`git.sdd`, Tasks). Un AC sans test ne vaut pas preuve.

## Open Issues

- **OI-1 - Blocage sans timeout:** aucun timeout ni `GIT_TERMINAL_PROMPT=0` ; un push qui attend une saisie bloque un thread `spawn_blocking` indéfiniment (`git.sdd`, Tasks `[!]`). **TBD :** timeout applicatif et désactivation des prompts.
- **OI-2 - Cible du push sans homologue:** sans `origin/<branche>`, `push` pousse la branche par défaut et non la branche courante, alors que le compte est calculé sur la branche courante (`git.sdd`, Tasks `[?]`, Scenario `push_sans_homologue_remote_pousse_le_nom_par_defaut`). **TBD :** comportement voulu ou bogue.
- **OI-3 - Résolution de la branche par défaut:** le second appel `git --git-dir` n'a pas de `-C` ; un chemin relatif de `--git-common-dir` dépend du cwd du processus (`git.sdd`, Tasks `[?]`). **TBD.**
- **OI-4 - Statut des erreurs-utilisateur:** `EmptyCommit`, `CheckoutRefused`, `DetachedHead`, `NoMergeInProgress`, `InvalidPath` en 500 (DEC-6, `git.sdd`, Tasks `[?]`). **TBD :** migration 4xx ou contrats figés.
- **OI-5 - Conflit fallacieux:** `git merge` sort en 1 sans fusion engagée (ex. modifications locales écrasées) ; la réponse `conflicted:true` est alors fausse (`git.sdd`, Tasks `[?]`). **TBD :** vérifier `MERGE_HEAD`.
- **OI-6 - Abort effondré:** tout échec de `merge --abort` devient `VNL-SBX-013 no merge in progress` (`git.sdd`, Tasks `[?]`). **TBD.**
- **OI-7 - Chemin vide et tailles:** `diff?path=` vide répond `500 VNL-SBX-007` ; aucune limite de taille de sortie ni de `limit` (`git.sdd`, Tasks `[?]`). **TBD :** borner ou assumer.
- **OI-8 - Garde d'Owner contradictoire:** `git.sdd` (Tasks `[x]`) indique l'alignement sur le contrat d'auth achevé et les tests présentant le jeton d'Owner ; `sandbox#OI-5` indique que l'auth par jeton d'Owner reste partiellement livrée. **TBD :** état réel de `require_auth` avant de tenir AC-1 pour vérifié.
- **OI-9 - Permissions de `.ssh`:** `.ssh` est créé à 0755 + umask, sans `chmod` explicite (`git.sdd`, Tasks `[?]`). **TBD :** poser 0700 ou assumer.
- **OI-10 - Chemin de la clé, deux sources:** `GIT_SSH_COMMAND` est en dur sur `/home/vanyline/.ssh/id_ed25519` (`docs/architecture.md`, SpecDD `/controller/src/project.sdd`, `[?]` l.395), tandis que la clé est écrite sous `$HOME/.ssh` (`git.sdd`). Les deux coïncident tant que `HOME=/home/vanyline`. **TBD :** source unique du chemin.
- **OI-11 - Env du Pod sandbox:** les sources lues prouvent `HOME` et `GIT_SSH_COMMAND` sur les Jobs git (`controller/src/sandbox.sdd`, `project.sdd`), pas sur le Pod sandbox lui-même. **TBD :** vérifier le template du Pod.
- **OI-12 - `ssh-keygen` dans l'image:** la présence de l'exécutable n'est pas établie par les sources lues (CON-2). **TBD.**
- **OI-13 - Consommateur CLI non trouvé:** l'usage de `/git/status` par `cli/chat` n'a pas été trouvé dans le dépôt (`cli/`, hors `dsd/`, `docs/`, specs). **TBD :** le consommateur existe-t-il ?
- **OI-14 - Duplication et porcelain:** `@run_git` duplique la logique de `@run_status` (`git.sdd`, Tasks `[?]`) ; le tag `!` et `origPath` sont jetés sans contrat frontend (`git.sdd`, Tasks `[?]`). **TBD.**
- **OI-15 - Relecture:** `git.sdd` n'est pas relu ; le comportement accidentel n'est pas contrat tant que la relecture humaine n'est pas faite (`git.sdd`, Tasks `[ ]`). **TBD.**

## Références

- `sandbox` : `BR-1`, `BR-2`, `BR-3`, `BR-6`, `BR-9`, `DEC-2`, `DEC-3`, `FR-3`, `NFR-1`, `AC-8`, `OI-1`, `OI-5`.
- `app` : `DEC-4` (`git push` depuis la sandbox est normal ; la protection relève de la forge).
- SpecDD `/sandbox/src/git.sdd`, `/sandbox/src/lib.sdd`, `/sandbox/src/auth.sdd`, `/sandbox/src/tools_impl.sdd`, `/sandbox/src/config.sdd`, `/app/src/api/sandboxes.sdd`, `/controller/src/sandbox.sdd`, `/controller/src/project.sdd`.
- `docs/architecture.md`, sections « Endpoints git » et « Clé SSH git — PVC Owner ».
