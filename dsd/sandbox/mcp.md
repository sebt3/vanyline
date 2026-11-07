# Solution Design: MCP de la sandbox

## Résumé

La Solution `sandbox/mcp` est le serveur MCP HTTP de la sandbox : un point d'entrée `POST /mcp` en JSON-RPC 2.0, sans session, qui expose à l'agent de code 17 outils de `tools/list` (fichiers, recherche, exécution, édition contrôlée, santé du code et sept outils LSP). La sémantique et les descripteurs des outils fichiers et recherche viennent du crate `vanyline-tools`, partagé avec le WebSocket de l'éditeur ; la sandbox y ajoute le dispatch, la garde de confinement des chemins et l'enveloppe de résultat MCP. Les sessions LSP et le WebSocket éditeur relèvent de `sandbox/editor` (voir OI-1).

## Problème et objectif

L'agent de code doit lire, modifier, rechercher, exécuter et vérifier du code dans le pod, par un protocole standard, avec des sorties exploitables par un LLM (bornées, numérotées, marqueurs de reprise actionnables), sans que la garde des chemins dépende de la bonne conduite du modèle. Cette Solution réalise `sandbox#FR-1` et contribue à `sandbox#BO-3`.

## Périmètre et non-objectifs

Couvert : le protocole `POST /mcp`, la déclaration et le dispatch des outils de `tools/list`, le confinement des arguments de chemin, les formats de sortie des outils MCP et les erreurs de protocole et d'outil.

Hors périmètre :

- sessions LSP, cache de diagnostics, ponts LSP et WebSocket éditeur : `sandbox/editor` (les sept outils `lsp_*` et `inspect_symbol` sont inventoriés ici, leur contrat est renvoyé à `sandbox/editor`, OI-1) ;
- sémantique d'exécution de `execute_command` (processus, timeout, sortie) et overlay venv : `sandbox/runtime` (`SpecDD /tools/src/command.sdd`, `SpecDD /sandbox/src/venv.sdd`) ;
- cycle de vie de l'état de santé du code : `sandbox/code-health` ;
- jeton d'Owner : `sandbox/access` ; liste des effets interdits à l'agent : `sandbox/threat-model` (`sandbox#OI-1`) ;
- git, jobs, maintenance des workspaces.

Non-objectifs : pas de session MCP ni de pagination (`tools/list` ignore `cursor`) ; pas d'outil `create_directory` ; `mkdir` et `rename_file` existent dans le crate mais ne sont pas exposés en MCP ; pas de contrôle d'accès par outil (`sandbox#BR-1`).

## Contexte applicable du domaine

Items de `dsd/sandbox.md` appliqués sans redéfinition :

- `sandbox#BR-1` : jeton d'Owner unique, accès tout ou rien ; le MCP ne réévalue ni jeton ni niveau.
- `sandbox#BR-6` : confinement sur toute voie d'accès à un chemin, MCP compris ; garde d'ergonomie, frontière réelle = le pod.
- `sandbox#BR-7` : santé du code toujours disponible ; `code_health` lit l'état sans le recalculer.
- `sandbox#BR-9` : le MCP ne borne pas les effets d'un script exécuté par l'agent.
- `sandbox#CON-4` : serveur MCP sans état.
- `sandbox#FR-1` : offre d'outils de fichiers, recherche, commande, LSP, édition contrôlée et santé.
- `sandbox#NFR-1` : identifiants d'erreur `VNL-SBX-*` ; `sandbox#DEC-3` : effets bornés au niveau du pod.

## Acteurs

- **Agent de code :** LLM qui appelle `tools/list` et `tools/call` avec le jeton de l'Owner (`sandbox#Acteurs`).
- **Clients MCP :** kydah-code, `cli --toolbox`, opérateur (jeton obtenu avec son contexte kubeconfig), `app` pour les agents système (`AGENTS.md`, section « Clients de la sandbox »).
- **Owner :** identité unique de la sandbox (`sandbox#BR-1`).
- **Consommateurs des descripteurs :** CLI (`config/localTools`) et `app` (`GET /api/local-tools`), qui lisent les mêmes déclarations (`SpecDD /tools/src/mcp.sdd`, Purpose).
- **Éditeur web :** consomme les fonctions filesystem du crate par WebSocket, hors de cette Solution (`sandbox/editor`).

## État actuel et état cible

- **Actuel** (Observed in `sandbox/src/mcp.rs`, `sandbox/src/tools_impl.rs`, `tools/src/mcp.rs`) : `POST /mcp` répond à `initialize`, `tools/list` (17 outils) et `tools/call`. La vérification du jeton d'Owner est contractée (`SpecDD /sandbox/src/auth.sdd`) mais sa livraison reste partielle (`sandbox#OI-5`).
- **Actuel, contrat en cours de retrait :** le Tasks de `SpecDD /sandbox/src/mcp.sdd` prévoit de supprimer une route de découverte OAuth que son Purpose dit inexistante (OI-4).
- **Cible :** l'agent de code agit par les modules Rhai de lymaille générés depuis ce MCP (`dsd/sandbox.md`, « État actuel et état cible », renvoi à `app#DEC-1`). La génération de ces modules n'est pas traitée ici.

## Hypothèses, contraintes et dépendances

- **CON-1:** Le protocole est codé à la main sur axum, sans dépendance à l'API du crate `rmcp`.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Purpose
- **CON-2:** Descripteurs et sémantique des huit outils partagés sont ceux du crate `vanyline-tools` ; la sandbox ne duplique aucune logique filesystem, search ou command.
    - **Source:** SpecDD /tools/src/mcp.sdd, Purpose ; SpecDD /sandbox/src/tools_impl.sdd, Must not
- **CON-3:** Le préfixage `{serveur}/{outil}` est appliqué par le client (`SpecDD /lib/src/prefixed_mcp.sdd`) ; le serveur ne voit que des noms d'origine.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Purpose
- **CON-4:** `POST /mcp` est derrière `require_auth` ; les échecs d'authentification sont rendus par ce middleware, jamais en enveloppe JSON-RPC.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Depends on, Must not
- **CON-5:** La racine de confinement est `Config::sandbox_root` du processus, jamais une entrée de l'agent.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Depends on, Must not
- **CON-6:** Aucune sortie directe (`println!`, `eprintln!`, `dbg!`) ; les traces passent par `tracing`.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Must ; SpecDD /tooling.sdd
- **CON-7:** Dépendances vers d'autres Solutions : `sandbox/runtime` (exécution, venv), `sandbox/editor` (flush éditeur, diagnostics), `sandbox/code-health` (lecture de l'état, `notify_modified`).
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Depends on, Handles

## Exigences fonctionnelles

- **FR-1 - MUST:** `POST /mcp` accepte un corps JSON-RPC 2.0 unique (jamais un batch) et répond `initialize`, `tools/list` et `tools/call`. `initialize` rend `protocolVersion` `"2024-11-05"`, la capacité `tools` et `serverInfo` ; la version demandée par le client n'est pas comparée.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Accepts, Returns, Must
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** Une requête sans `id` exploitable (absent ou `null`), y compris `notifications/initialized` porteur d'un `id`, reçoit HTTP 202 avec corps vide.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Returns, Handles, Must
    - **Verified by:** `AC-1`
- **FR-3 - MUST:** Les erreurs de protocole sont rendues en HTTP 200 avec `error`, jamais `result`. Seuls `-32601` (méthode inconnue, avec `id`) et `-32602` (nom absent ou non chaîne, nom inconnu de toutes les familles) sont émis ; un corps invalide, un batch ou un `method` absent sont rejetés par l'extracteur axum, sans enveloppe.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Raises, Must, Handles
    - **Verified by:** `AC-1`, `AC-2`
- **FR-4 - MUST:** Un échec d'exécution d'un outil reconnu rend `{content, isError: true}` dont le texte porte un marqueur `VNL-SBX-*` ou `VNL-TLS-*` ; il n'est jamais converti en erreur JSON-RPC ni en `None`.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Returns, Must ; SpecDD /sandbox/src/tools_impl.sdd, Must ; SpecDD /tools/src/error.sdd, Must
    - **Verified by:** `AC-3`
- **FR-5 - MUST:** `tools/list` rend exactement 17 outils, sans préfixe, dans l'ordre : `read_file`, `write_file`, `edit_file`, `delete_file`, `list_directory`, `find_files`, `search`, `execute_command`, les huit outils de la famille LSP (`edit_and_check` en avant-dernier), puis `code_health` ; la liste est reconstruite à chaque appel.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Returns, Scenario `outils_listes_en_noms_origine_et_17_entrees` ; SpecDD /sandbox/src/health.sdd, Exposes (non relu ici)
    - **Verified by:** `AC-1`
- **FR-6 - MUST:** Chacun des huit descripteurs partagés a `name`, `description` non vide et `inputSchema` (`type` objet, `properties`, `required` non vide, tableaux fixés par la spec) ; chaque description contient un `Example: {…}` ; les défauts sont annoncés en prose, jamais dans le schéma.
    - **Source:** SpecDD /tools/src/mcp.sdd, Must, Scenarios `required_de_read_file_est_path`, `chaque_description_porte_un_exemple_json`, `defauts_annonces_en_prose_sans_cle_schema`
    - **Verified by:** `AC-5`
- **FR-7 - MUST:** `read_file` rend les lignes numérotées au format `{:>5}\t` à partir de `offset + 1` (0-based), avec `limit` par défaut 200 (`0` vaut défaut) ; une troncature ajoute `[truncated — N more lines, call again with offset=…]`. Le mode `raw` n'est pas dans le schéma MCP (OI-8).
    - **Source:** SpecDD /tools/src/filesystem.sdd, Returns, Must, Scenarios `read_file_numerote_aux_defauts`, `read_file_limite_declenche_marqueur_de_troncation`
    - **Verified by:** `AC-6`
- **FR-8 - MUST:** `write_file` crée les répertoires parents, écrit par fichier temporaire puis renommage, conserve le mode de la cible et rend `wrote {chemin résolu}`.
    - **Source:** SpecDD /tools/src/filesystem.sdd, Must, Scenarios `write_file_cree_repertoires_parents`, `write_file_preserve_permissions_de_la_cible` ; SpecDD /sandbox/src/tools_impl.sdd, Returns
    - **Verified by:** `AC-6`
- **FR-9 - MUST:** `edit_file` refuse un `old_string` vide avant tout accès disque ; sans `replace_all`, il échoue si l'occurrence est absente (`VNL-TLS-006`, avec la ligne la plus proche) ou multiple (`VNL-TLS-007`, avec le nombre) ; il rend `edited {path}: n replacement(s)`.
    - **Source:** SpecDD /tools/src/filesystem.sdd, Raises, Returns, Scenarios `edit_file_ambigu_sans_replace_all_refuse`, `edit_file_sans_correspondance_avec_ligne_la_plus_proche` ; SpecDD /tools/src/mcp.sdd, Must
    - **Verified by:** `AC-6`
- **FR-10 - MUST:** `delete_file` supprime un fichier ou un répertoire vide, jamais récursivement, et rend `deleted {chemin résolu}`.
    - **Source:** SpecDD /tools/src/filesystem.sdd, Must, Raises ; SpecDD /sandbox/src/tools_impl.sdd, Returns
    - **Verified by:** `AC-6`
- **FR-11 - MUST:** `list_directory` rend un arbre trié par nom binaire, deux espaces par niveau, `/` pour les répertoires, profondeur par défaut 1, plafonné à 200 entrées avec suffixe de troncature.
    - **Source:** SpecDD /tools/src/filesystem.sdd, Returns, Scenarios `list_directory_arbre_tri_avec_suffixe_repertoire`, `list_directory_plafonne_a_200_entrees`
    - **Verified by:** `AC-6`
- **FR-12 - MUST:** `find_files` rend des chemins relatifs au répertoire cherché (glob `globset`), plafonnés à 200 ; il exclut `.git`, `target` et `node_modules` à toute profondeur ; `.gitignore` ne s'applique que dans un dépôt git ; sans résultat, il rend `no files matching '…' under …`.
    - **Source:** SpecDD /tools/src/search.sdd, Purpose, Handles, Returns, Scenarios `find_files_gitignore_respecte_dans_depot_git`, `find_files_hors_depot_git_le_gitignore_ne_filtre_rien`, `find_files_tronque_a_200_entrees`
    - **Verified by:** `AC-7`
- **FR-13 - MUST:** `search` applique une regex ligne par ligne et rend `{rel}:{N}: {ligne}` (N 1-based), plafonné à 50 correspondances ; le filtre `glob` s'évalue sur le chemin relatif ; un fichier illisible ou non UTF-8 est sauté sans signal dans la sortie ; sans résultat, `no matches for '…' under …`.
    - **Source:** SpecDD /tools/src/search.sdd, Returns, Handles, Scenarios `search_format_de_ligne_exact`, `search_tronque_a_50_correspondances`, `search_fichier_illisible_ignore_silencieusement`
    - **Verified by:** `AC-7`
- **FR-14 - MUST:** `execute_command` confine son `cwd` avant exécution (vide vaut racine de la sandbox, jamais le cwd du processus), n'accepte aucun `envs` de l'agent et recalcule l'overlay venv à chaque appel ; sa sémantique d'exécution est dans `sandbox/runtime`.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Must, Handles, Scenarios `cwd_vide_de_execute_command_vaut_racine_sandbox`, `overlay_venv_non_injectable_et_recalcule` ; SpecDD /tools/src/mcp.sdd, Must
    - **Verified by:** `AC-8`
- **FR-15 - MUST:** `code_health` lit l'état courant sans jamais le recalculer ; son argument `detail` vaut `summary` (défaut), `issues` ou `coverage`, avec filtre `path` par préfixe et `limit` ; la sortie texte est bornée ; un rapport absent rend un texte de phase, pas une erreur.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Purpose, Returns ; SpecDD /sandbox/src/health.sdd (non relu ici)
    - **Realized by:** `sandbox/code-health`
    - **Verified by:** `AC-10`
- **FR-16 - MUST:** `edit_and_check` valide `mode` (`write` ou `edit`) et ses arguments avant tout I/O, applique l'édition sous confinement, puis rend l'en-tête de l'édition et le diff des diagnostics (apparus, disparus, inchangés) ; un dépassement de ré-analyse rend `VNL-SBX-LSP-011` en succès, l'édition étant appliquée.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Accepts, Returns, Handles, Scenarios `edit_and_check_union_valide_avant_io`, `edit_and_check_reanalyse_mute_rend_011_en_succes`
    - **Realized by:** `sandbox/editor` (flush et diagnostics)
    - **Verified by:** `AC-9`
- **FR-17 - MUST:** Tout argument de chemin des cinq familles (filesystem, search, command, LSP, edit_and_check) est confiné avant tout accès ; les URI issues des réponses LSP passent par la même garde.
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Must, Scenario `tout_dispatch_de_fichier_confine_son_chemin` ; `sandbox#BR-6`
    - **Verified by:** `AC-4`
- **FR-18 - MUST:** Chaque requête est autonome : un `tools/call` sans `initialize` préalable est servi, deux `initialize` successifs sont indépendants, et aucun en-tête de session n'est émis ni exigé.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Must, Must not, Scenario `outils_appelles_par_nom_origine_sans_initialisation` ; `sandbox#CON-4`
    - **Verified by:** `AC-1`
- **NFR-1 - MUST:** Les sorties sont bornées par des constantes centrales (`READ_MAX_LINES` 200, `READ_MAX_BYTES` 16 384, `LIST_MAX_ENTRIES` 200, `SEARCH_MAX_MATCHES` 50, `COMMAND_MAX_BYTES` 8 192) ; une troncature ne coupe jamais une ligne.
    - **Source:** SpecDD /tools/src/output.sdd, Must, Scenario `bornes_defaut_valeurs_centrales`
    - **Verified by:** `AC-6`

## Sécurité

- **Garde de confinement** (`confine_path`, SpecDD /sandbox/src/tools_impl.sdd, Handles) : racine canonifiée en premier (échec : `VNL-SBX-002`) ; chemin vide ou composé de `/` désigne la racine ; barres finales ignorées ; absolu pris tel quel, relatif joint à la racine.
- Les `..` sont résolus lexicalement avant le contrôle ; l'ancêtre existant le plus profond est canonifié, puis le suffixe inexistant est rajouté (écriture d'un fichier nouveau permise) ; le contrôle final compare des composants (`/work-evil` n'est pas sous `/work`) ; un lien symbolique sortant est rejeté ; `VNL-SBX-001` signale l'évasion, `VNL-SBX-003` un ancêtre non résoluble.
- **Appel systématique :** la garde précède chaque appel à `vanyline-tools`, `cwd` vide compris ; `envs` n'est pas désérialisable depuis les arguments (`#[serde(skip)]`). Le crate ne confine rien (DEC-2).
- **Réponses LSP :** les URI `file://` renvoyées par un serveur sont traitées comme entrées non fiables et passent par la même garde (détail dans `sandbox/editor`).
- **Limite assumée :** une substitution de lien entre la marche et la canonicalisation (course TOCTOU) est hors du modèle de menace retenu (DEC-7).
- **Hors de cette Solution :** les effets d'une commande ou d'un script (réseau sortant, clé SSH présente dans le pod, quotas) relèvent de `sandbox#BR-9`, `sandbox#DEC-2` et `sandbox#OI-1` ; le MCP ne les borne pas.

## Conception

### Vue d'ensemble

```text
Client MCP (kydah-code, cli --toolbox, opérateur, app)
  │ POST /mcp, Bearer = jeton du ServiceAccount de l'Owner (require_auth, en amont)
  ▼
mcp.rs : JSON-RPC 2.0 — initialize | tools/list | tools/call | notifications
  │ tools/call : chaîne de dispatch, premier Some gagnant, nom testé par nom
  ▼
tools_impl.rs
  ├─ dispatch_filesystem ──── read / write / edit / delete / list ─┐
  ├─ dispatch_search ──────── find_files, search ──────────────────┤ confine(chemin),
  ├─ dispatch_command ─────── execute_command + venv ──────────────┤ puis vanyline-tools
  ├─ dispatch_lsp ─────────── 7 outils LSP ───────► sandbox/editor
  ├─ dispatch_edit_and_check ► fichier confiné + sandbox/editor
  └─ dispatch_code_health ──► sandbox/code-health (lecture seule)
vanyline-tools : filesystem.rs, search.rs, output.rs, error.rs, mcp.rs (descripteurs)
```

### Protocole (`sandbox/src/mcp.rs`)

- Un corps JSON unique par requête ; une requête sans `id` prend la branche de notification (202), qui prime sur le dispatch.
- Métrique `mcp_requests_total` labellisée par méthode, incrémentée avant le dispatch (`SpecDD /sandbox/src/mcp.sdd`, Must).

### Dispatch et enveloppe (`sandbox/src/tools_impl.rs`)

- Ordre strict : filesystem, search, command, lsp, edit_and_check, code_health ; chaque famille répond `None` pour un nom étranger. Des arguments invalides rendent une erreur d'outil `invalid arguments for {name}: …`.
- Enveloppe `{content: [{type: "text", text}], isError}` produite par `ok_result` et `err_result`.
- Après une écriture réussie, le dispatch notifie la santé du code (`CodeHealth::notify_modified`, `SpecDD /sandbox/src/tools_impl.sdd`, Handles).

### Outils partagés (crate `vanyline-tools`)

| Outil | Module | Contrat de sortie observable |
| --- | --- | --- |
| `read_file` | filesystem | lignes `{:>5}\t…`, marqueur de reprise `offset=` |
| `write_file` | filesystem | `wrote {chemin résolu}` (message rendu par la sandbox) |
| `edit_file` | filesystem | `edited {path}: n replacement(s)` |
| `delete_file` | filesystem | `deleted {chemin résolu}` (message rendu par la sandbox) |
| `list_directory` | filesystem | arbre trié, `/` pour les répertoires, 200 entrées au plus |
| `find_files` | search | chemins relatifs triés, marqueur à 200 |
| `search` | search | `{rel}:{N}: {ligne}`, marqueur à 50 |
| `execute_command` | command (runtime) | sortie tête + queue bornée à 8 192 octets |

Erreurs `VNL-TLS-001` (introuvable, avec liste du parent), `002` (répertoire lu comme fichier), `003` (fichier lu comme répertoire), `004` (permission), `005` (argument), `006` et `007` (édition), `009` (E/S) ; `VNL-TLS-008` (timeout) appartient à la commande. Les descripteurs sont produits par des fonctions pures (`SpecDD /tools/src/mcp.sdd`).

### Format de sortie pour un LLM

- Bornes centralisées (NFR-1). Chaque marqueur indique l'action de reprise : `offset=` pour `read_file`, « narrow the path » pour listings et recherches.
- Chemins relatifs dans les recherches et les rendus LSP ; absolus et résolus dans `write_file` et `delete_file` (OI-7).

## Décisions

- **DEC-1 - Serveur MCP POST-only, sans session**
    - **Context:** la sandbox est appelée sans état partagé, et le domaine pose le contrat sans état.
    - **Decision:** JSON-RPC 2.0 sur `POST /mcp` seulement ; pas de `Mcp-Session-Id`, pas de flux SSE, pas de `DELETE`.
    - **Consequences:** toute session ou flux serveur exige une décision nouvelle (OI-5 sur l'identifiant de session facultatif).
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Purpose, Must not ; `sandbox#CON-4`
- **DEC-2 - Outils partagés dans `vanyline-tools`, confinement dans l'appelant**
    - **Context:** les mêmes outils servent le MCP, le WebSocket éditeur, `app` et la CLI.
    - **Decision:** le crate porte la sémantique et les descripteurs sans confinement ; chaque appelant confine avant d'appeler. Racine optionnelle dans le crate : écartée.
    - **Consequences:** un dispatch de fichiers qui oublie la garde expose le système de fichiers du pod ; le verrou est un test d'architecture (OI-3).
    - **Source:** SpecDD /tools/src/filesystem.sdd, Tasks (décidé 2026-10-18) ; SpecDD /tools/src/mcp.sdd, Purpose
- **DEC-3 - Descripteurs sans exécution**
    - **Context:** le LLM, la CLI et `app` lisent la même description des outils.
    - **Decision:** les trois fonctions de déclaration sont pures, sans argument ni I/O, et n'exécutent ni ne valident rien.
    - **Consequences:** la cohérence entre description et comportement repose sur les tests des modules frères (OI-2).
    - **Source:** SpecDD /tools/src/mcp.sdd, Purpose, Must not
- **DEC-4 - Échec d'outil en résultat, erreur JSON-RPC réservée au protocole**
    - **Context:** l'agent doit lire la raison d'un échec comme texte exploitable.
    - **Decision:** un outil trouvé qui échoue rend `isError: true` ; `-32601` et `-32602` ne couvrent que le protocole et la résolution du nom.
    - **Consequences:** le statut de `Unknown tool` reste discuté (OI-5).
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Returns, Must
- **DEC-5 - Sorties dimensionnées pour un LLM**
    - **Context:** une sortie non bornée sature le contexte ; une troncature muette empêche la reprise.
    - **Decision:** bornes centralisées, numérotation façon `cat -n`, troncature par lignes entières avec marqueur de reprise.
    - **Consequences:** formats contractuels ; le marqueur compte comme une ligne de plus (OI-8).
    - **Source:** SpecDD /tools/src/output.sdd, Purpose, Must ; SpecDD /tools/src/filesystem.sdd, Purpose
- **DEC-6 - Écriture atomique par temporaire et renommage**
    - **Context:** une écriture interrompue ne doit ni tronquer le fichier ni réinitialiser son mode.
    - **Decision:** temporaire dans le même répertoire, copie des permissions de la cible, puis `rename`.
    - **Consequences:** un arrêt entre écriture et `rename` laisse un résidu temporaire (OI-10).
    - **Source:** SpecDD /tools/src/filesystem.sdd, Purpose, Handles
- **DEC-7 - Garde de confinement d'ergonomie ; frontière réelle = pod**
    - **Context:** un attaquant capable de substituer des liens dans le workspace exécute déjà du code dans le pod.
    - **Decision:** la garde reste dans la sandbox ; la course TOCTOU est hors du modèle de menace retenu.
    - **Consequences:** la sécurité des effets repose sur les bornes du pod (`sandbox#BR-6`, `sandbox#DEC-3`).
    - **Source:** SpecDD /sandbox/src/tools_impl.sdd, Tasks (décidé 2026-10-18) ; `sandbox#BR-6`
- **DEC-8 - Pas de contrôle d'accès par outil**
    - **Context:** le modèle d'accès du domaine est tout ou rien.
    - **Decision:** le MCP n'évalue ni niveau, ni groupe, ni jeton ; l'authentification précède le routeur MCP.
    - **Consequences:** toute restriction par outil passe par un mécanisme de décision distinct, hors MCP.
    - **Source:** SpecDD /sandbox/src/mcp.sdd, Must not ; `sandbox#BR-1`, `sandbox#DEC-1`

## Réalisation SpecDD

- `SpecDD /sandbox/src/mcp.sdd` : protocole, FR-1 à FR-5, FR-18 (`sandbox/src/mcp.rs`).
- `SpecDD /sandbox/src/tools_impl.sdd` : dispatch, confinement, enveloppe, FR-4, FR-14 à FR-17 (`sandbox/src/tools_impl.rs`).
- `SpecDD /tools/src/mcp.sdd` : descripteurs, FR-6 (`tools/src/mcp.rs`) ; `SpecDD /tools/src/lib.sdd` : surface du crate.
- `SpecDD /tools/src/filesystem.sdd` : FR-7 à FR-10 ; `SpecDD /tools/src/search.sdd` : FR-12, FR-13 ; `SpecDD /tools/src/output.sdd` : NFR-1 ; `SpecDD /tools/src/error.sdd` : marqueurs `VNL-TLS-*`.
- Références seules, hors Solution : `SpecDD /sandbox/src/health.sdd` (FR-15), `SpecDD /tools/src/command.sdd` et `SpecDD /sandbox/src/venv.sdd` (`sandbox/runtime`, FR-14), `SpecDD /sandbox/src/lsp.sdd` et `SpecDD /sandbox/src/lsp_client.sdd` (`sandbox/editor`, FR-16).

## Acceptation et vérification

Les Scenarios cités existent dans les specs ; leur couverture par des tests est partielle (OI-12).

- **AC-1:** `initialize` rend la version fixe et l'écho d'`id` ; `tools/list` rend les 17 noms de FR-5 ; une notification rend 202 vide ; `-32601` et `-32602` suivent FR-3 ; un `tools/call` sans `initialize` est servi (scénarios `initialize_rend_version_protocole_et_infos_serveur`, `notification_sans_id_recoit_202_sans_corps`, `methode_inconnue_avec_id_rend_32601`, `outils_inconnu_de_toutes_familles_rend_32602`).
- **AC-2:** Un JSON tronqué ou un batch est rejeté en 4xx, sans membre `jsonrpc` ni `-32700` (scénario `corps_invalide_rejete_sans_enveloppe_rpc`).
- **AC-3:** Une évasion de chemin rend `isError: true` avec `VNL-SBX-001`, sans membre `error` (scénario `erreur_execution_outil_reste_enveloppe_is_error`).
- **AC-4:** Les chemins hors racine sont refusés, y compris via `..` sur segment inexistant et via lien symbolique (scénarios `tout_dispatch_de_fichier_confine_son_chemin`, `points_points_via_segment_inexistant_rejete`, `lien_symbolique_evadant_rejete`) ; `sandbox#AC-4` en vérification de domaine.
- **AC-5:** Les huit descripteurs ont les clés, `required`, exemples et défauts de FR-6 (scénarios de `SpecDD /tools/src/mcp.sdd`).
- **AC-6:** Lecture numérotée et marqueur exact, écriture atomique, édition ambiguë ou sans correspondance, suppression non récursive, listing trié et plafonné (scénarios de `SpecDD /tools/src/filesystem.sdd` et `SpecDD /tools/src/output.sdd`).
- **AC-7:** Format `{rel}:{N}: {ligne}`, marqueurs à 50 et 200, exclusions et gitignore conditionnel (scénarios de `SpecDD /tools/src/search.sdd`).
- **AC-8:** `cwd` vide résolu sur la racine, `envs` ignoré, venv recalculé (scénarios `cwd_vide_de_execute_command_vaut_racine_sandbox`, `overlay_venv_non_injectable_et_recalcule`).
- **AC-9:** Validation avant I/O, cas A et cas B, flush non acquitté, `VNL-SBX-LSP-011` en succès (scénarios `edit_and_check_*` de `SpecDD /sandbox/src/tools_impl.sdd`).
- **AC-10:** `code_health` ne recalcule pas et rend un texte borné ; `sandbox#AC-5` en vérification de domaine ; le Scenario de déclaration est dans `SpecDD /sandbox/src/health.sdd` (non relu ici).

## Open Issues

- **OI-1 - Frontière LSP entre `sandbox/mcp` et `sandbox/editor`:** `dsd/sandbox.md` (« Paysage des Solutions ») range les LSP dans les deux Solutions ; la demande de conception les place dans `sandbox/editor`, alors que `SpecDD /sandbox/src/tools_impl.sdd` déclare les sept outils LSP et `edit_and_check` dans le `tools/list` du MCP. **TBD :** propriétaire de la déclaration et de la sémantique.
- **OI-2 - Défaut de `timeout_secs` (conflit pré-existant, bloquant selon l'autorité):** `SpecDD /tools/src/mcp.sdd` impose à la description d'annoncer `default 30, 0 = no timeout` et le CLI applique 30 ; la sandbox mappe l'absence sur `0`, soit aucun timeout (`SpecDD /sandbox/src/tools_impl.sdd`, Handles et Tasks). Un `execute_command` sans champ peut ne jamais rendre la main. **TBD :** quel défaut fait foi ; la correction touche `sandbox/runtime` et `SpecDD /tools/src/mcp.sdd`.
- **OI-3 - Périmètre du verrou de confinement:** le Must de `SpecDD /sandbox/src/tools_impl.sdd` impose la garde aux cinq familles, mais le Scenario `tout_dispatch_de_fichier_confine_son_chemin` n'en couvre que filesystem et search ; sa Tasks `[?]` recommande d'étendre le test à LSP et `edit_and_check`, et la réponse du développeur reste à remplir. **TBD.**
- **OI-4 - Route de découverte OAuth:** le Purpose de `SpecDD /sandbox/src/mcp.sdd` affirme qu'aucun endpoint OAuth n'existe, alors que son Tasks prévoit de supprimer `oauth_metadata` et `GET /.well-known/oauth-protected-resource` (décision du 2026-10-04). **TBD :** état réel et ordre de retrait.
- **OI-5 - Contrat de protocole:** Tasks `[?]` de `SpecDD /sandbox/src/mcp.sdd` : `Unknown tool` en `-32602` alors que la spec MCP tend vers `isError` ; message trompeur pour un `name` non chaîne ; `tools/list` ne pagine jamais ; identifiant de session facultatif (DEC-1) ; label `method` non borné ; notifications comptées sous `"notification"`. **TBD :** arbitrage du développeur principal.
- **OI-6 - Textes périmés:** `SpecDD /sandbox/src/mcp.sdd` dit « les 16 noms sont distincts » pour 17 outils, et « 5 familles de dispatch » là où `SpecDD /sandbox/src/tools_impl.sdd` en porte six ; `SpecDD /tools/src/filesystem.sdd`, Purpose, dit « les sept tools exposés au LLM (MCP) » alors que son Handles en expose cinq. **TBD :** corriger les textes.
- **OI-7 - Rendu des chemins:** `write_file` et `delete_file` rendent le chemin absolu résolu, les rendus LSP sont relatifs ; une racine de config non canonifiée peut faire échouer l'affichage relatif (`SpecDD /sandbox/src/tools_impl.sdd`, Tasks). **TBD :** harmoniser ou assumer.
- **OI-8 - Lecture:** `raw` est accepté mais absent du schéma MCP et ignore `offset` (`SpecDD /tools/src/filesystem.sdd`, Tasks) ; le marqueur porte la sortie à 201 lignes pour `limit` 200 (`SpecDD /tools/src/output.sdd`, Tasks) ; une ligne unique de plus de 16 Kio sort sans marqueur. **TBD.**
- **OI-9 - Recherches:** aucune limite de taille ou de temps ; fichiers illisibles sautés sans signal ; `find_files` tronque avant tri ; `search` annonce une troncature sur une ligne sans correspondance (`SpecDD /tools/src/search.sdd`, Tasks). **TBD.**
- **OI-10 - Concurrence et atomicité:** nom de temporaire identique pour deux écritures concurrentes d'un même fichier ; `edit_file` est un cycle lecture-modification-écriture sans garde ; `rename` remplace un lien par un fichier (`SpecDD /tools/src/filesystem.sdd`, Tasks). **TBD.**
- **OI-11 - Champs ignorés:** `edit_and_check` ignore `old_string`/`new_string` en `write` et `content` en `edit`, sans refus (`SpecDD /sandbox/src/tools_impl.sdd`, Tasks). **TBD :** refus explicite ou silence assumé.
- **OI-12 - Couverture et dette de code:** plusieurs Scenarios des specs citées n'ont pas de test direct ; `SpecDD /sandbox/src/tools_impl.sdd` porte six `unreachable!` que le harnais interdit (Tasks). **TBD.**

## Références

- `dsd/sandbox.md` : `sandbox#BO-3`, `sandbox#FR-1`, `sandbox#BR-1`, `sandbox#BR-6`, `sandbox#BR-7`, `sandbox#BR-9`, `sandbox#CON-4`, `sandbox#NFR-1`, `sandbox#DEC-1`, `sandbox#DEC-2`, `sandbox#DEC-3`, `sandbox#AC-4`, `sandbox#AC-5`, `sandbox#OI-1`, `sandbox#OI-5`.
- `AGENTS.md`, section « Clients de la sandbox ».
- Specs sources : `SpecDD /sandbox/src/mcp.sdd`, `SpecDD /sandbox/src/tools_impl.sdd`, `SpecDD /tools/src/mcp.sdd`, `SpecDD /tools/src/filesystem.sdd`, `SpecDD /tools/src/search.sdd`, `SpecDD /tools/src/output.sdd`, `SpecDD /tools/src/error.sdd`, `SpecDD /tools/src/lib.sdd`.
- Specs référencées, non relues pour ce document : `SpecDD /sandbox/src/health.sdd`, `SpecDD /sandbox/src/auth.sdd`, `SpecDD /sandbox/src/venv.sdd`, `SpecDD /sandbox/src/lsp.sdd`, `SpecDD /sandbox/src/lsp_client.sdd`, `SpecDD /tools/src/command.sdd`.
