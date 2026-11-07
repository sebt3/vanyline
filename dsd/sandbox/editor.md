# Solution Design: Éditeur et langages

## Résumé

La Solution `sandbox/editor` décrit ce que la sandbox offre à l'éditeur web du navigateur : un système de fichiers en RPC JSON sur WebSocket (`/ws/fs`), un terminal PTY (`/ws/terminal`), un pont WebSocket vers les serveurs de langage (`/ws/lsp/{toolchain}`), ainsi que les mécanismes LSP qui les alimentent : le multiplexeur de sessions (`lsp.rs`), le client de session (`lsp_client.rs`) et les deux wrappers de diagnostics `vnl-ruff-lsp` et `vnl-hadolint-lsp`.

L'authentification par ticket relève de `sandbox/access`. Les outils MCP `lsp_*` et `edit_and_check` relèvent de `sandbox/mcp`, qui consomme le même multiplexeur.

Statut : reverse-engineering écrit depuis les specs `sandbox/src/` citées. Le code Rust n'a pas été relu. Les contradictions entre specs sont des Open Issues en **TBD**.

## Problème et objectif

Le développeur doit lire, éditer, exécuter des commandes et obtenir des diagnostics de langage dans la sandbox, depuis son navigateur, sans installer les outils localement. Plusieurs onglets ne doivent pas lancer plusieurs serveurs de langage pour une même toolchain. Les diagnostics ruff et hadolint sont exposés comme un serveur LSP par des shims maison.

Objectif : offrir ces accès à l'éditeur, confinés au pod, derrière le ticket de `sandbox#FR-2` (`sandbox#BO-1` pour l'usage).

## Périmètre et non-objectifs

Couvert :

- le handler `GET /ws/fs` (`SpecDD /sandbox/src/ws/fs.sdd`) ;
- le handler `GET /ws/terminal` et ses helpers PTY (`SpecDD /sandbox/src/ws/terminal.sdd`) ;
- le pont `GET /ws/lsp/{toolchain}` et la réécriture d'URI (`SpecDD /sandbox/src/ws/lsp.sdd`) ;
- le multiplexeur LSP, mode mono et composite (`SpecDD /sandbox/src/lsp.sdd`) et le client de session (`SpecDD /sandbox/src/lsp_client.sdd`) ;
- les wrappers `vnl-ruff-lsp` (`SpecDD /sandbox/src/bin/ruff_lsp.sdd`) et `vnl-hadolint-lsp` (`SpecDD /sandbox/src/bin/hadolint_lsp.sdd`).

Hors Solution :

- ticket, `POST /ws/ticket`, garde `ws_auth_middleware` : `sandbox/access` (`SpecDD /sandbox/src/ws/ticket.sdd`) ;
- outils MCP `lsp_*`, `edit_and_check`, `confine_path` : `sandbox/mcp` (`SpecDD /sandbox/src/tools_impl.sdd`) ;
- overlay `.venv` : `sandbox/runtime` (`SpecDD /sandbox/src/venv.sdd`) ;
- `VNL_LSP_TOOLCHAINS`, presets argv, volumes : `workspaces/sandboxes` (`SpecDD /controller/src/sandbox.sdd`) ;
- NetworkPolicy et Ingress : `workspaces/network`, `workspaces/sandboxes` ;
- santé du code (`vanyline-health`) : `sandbox/code-health` ;
- modèle de menace de l'agent de code : `sandbox#OI-1`, `sandbox/threat-model` ;
- code du frontend éditeur, lu seulement comme référence.

## Contexte applicable du domaine

- `sandbox#FR-2` : l'éditeur web dispose d'un système de fichiers temps réel, d'un terminal et de ponts LSP, par WebSocket authentifié par ticket. Cette Solution en est la réalisation.
- `sandbox#AC-2` : un ticket ne sert qu'une fois ; ce mécanisme appartient à `sandbox/access`.
- `sandbox#BR-1` : l'accès est tout ou rien ; l'Owner dispose de toutes les opérations, écritures et suppressions comprises.
- `sandbox#BR-3` : le navigateur ne voit jamais le jeton d'Owner.
- `sandbox#BR-5` : une toolchain est utilisable par injection d'environnement au démarrage (`PATH`, `LD_LIBRARY_PATH`).
- `sandbox#BR-6` : le confinement s'applique aux WebSocket de fichiers et au LSP ; il est une garde d'ergonomie, la frontière réelle est le pod.
- `sandbox#BR-9`, `sandbox#DEC-2` : le terminal exécute des commandes dans le pod, et la clé SSH de l'Owner y est présente.
- `sandbox#CON-3` : ruff et hadolint sont présents dans les images toolchain, à versions épinglées.
- `sandbox#NFR-1` : chaque erreur porte un identifiant `VNL-SBX-*`.
- `sandbox#AC-4` : un chemin qui sort de la racine, y compris par une URI LSP, est refusé (voir OI-2).
- `workspaces#CON-2` : le workspace est un volume local `RWO` ; les surveillances de fichiers ne traversent pas les systèmes réseau (voir DEC-4).
- `workspaces/sandboxes#FR-7`, `workspaces/sandboxes#FR-8` : le Service n'expose que le port `mcp` ; l'Ingress public n'existe que si l'Owner référence une Application.

## Acteurs

- **Développeur :** utilise l'éditeur web (fichiers, terminal, diagnostics, navigation).
- **Frontend :** ouvre les WebSocket directement sur la sandbox avec un ticket à usage unique (`AGENTS.md`, section Architecture ; `SpecDD /sandbox/src/ws/ticket.sdd` pour le ticket).
- **Agent de code :** utilise le même multiplexeur LSP par les outils MCP `lsp_*` (`SpecDD /sandbox/src/lsp.sdd`, Purpose).
- **Controller :** produit `VNL_LSP_TOOLCHAINS` et monte les volumes toolchain (`SpecDD /sandbox/src/lsp.sdd`, Depends on).
- **Serveurs primaires :** binaires des toolchains (exemples cités : `rust-analyzer`, `pyright-langserver`, `docker-langserver`, `vue-language-server`).
- **Auxiliaires :** `vnl-ruff-lsp` et `vnl-hadolint-lsp` (rôle `diagnostics-merge`), `typescript-language-server` (rôle `tsserver-forward`).

## État actuel et état cible

- **Actuel (Observed in: `sandbox/src/ws/`, `sandbox/src/lsp*.rs`, `sandbox/src/bin/`, selon les specs) :** les trois routes WebSocket, le multiplexeur composite et les deux wrappers existent. Les specs annoncent 25 tests pour `fs`, 7 pour `terminal`, 14 pour `ws/lsp`, environ 90 pour `lsp`, 11 unitaires pour `ruff_lsp`.
- **Écart :** les Scenarios de session WebSocket (upgrade réel, Close, Ping, multi-session) n'ont pas de harnais de test (OI-8).
- **Cible :** chaque Scenario est couvert par un test taggé de son titre, puis les specs sont relues. Toutes les specs de cette Solution portent `[ ] Relecture`, et aucune décision `[?]` n'est tranchée.

## Hypothèses, contraintes et dépendances

- **CON-1:** Le transport est axum 0.8 (feature `ws`) sur tokio, et le PTY passe par portable-pty. Une frame texte porte un message JSON-RPC pour le LSP ; une frame binaire porte des octets de PTY.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Platform, Depends on) ; `SpecDD /sandbox/src/ws/terminal.sdd` (Platform) ; `SpecDD /sandbox/src/ws/lsp.sdd` (Depends on)
- **CON-2:** Les serveurs LSP parlent JSON-RPC avec framing `Content-Length` sur stdio. Ce framing n'existe qu'entre la session et l'enfant, jamais côté WebSocket.
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Purpose, point 2) ; `SpecDD /sandbox/src/ws/lsp.sdd` (Purpose, point 1)
- **CON-3:** Les binaires sont des chemins absolus sous les volumes `/toolchains/…`. Le crate valide la forme JSON de `VNL_LSP_TOOLCHAINS`, mais ni l'existence des binaires ni la cohérence des presets.
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Depends on ; Accepts)
- **CON-4:** Les wrappers lancent `ruff check --output-format json --force-exclude -` et `hadolint --format json --no-color -` via `PATH`, sans shell. Les versions sont épinglées (ruff `0.16.6`, hadolint `v2.15.1`).
    - **Source:** `SpecDD /sandbox/src/bin/ruff_lsp.sdd` (Purpose, Must) ; `SpecDD /sandbox/src/bin/hadolint_lsp.sdd` (References) ; `sandbox#CON-3`
- **CON-5:** Aucun surveillant de fichiers n'existe dans ce périmètre (pas de `notify` ni d'inotify). Seules les frames publiées sur `fs_events` par `edit_and_check` et par la demande de flush sont relayées.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Purpose, Handles)
- **CON-6:** L'éditeur WebSocket n'est joignable depuis le navigateur que si l'Ingress existe, donc si l'Owner référence une Application.
    - **Source:** `workspaces/sandboxes#FR-8`, `workspaces/sandboxes#FR-7`
- **Dépendances :** `sandbox/access` (ticket, port principal) ; `sandbox/mcp` (`confine_path`, outils `lsp_*`) ; `sandbox/runtime` (venv, toolchains) ; `workspaces/sandboxes` (`VNL_LSP_TOOLCHAINS`, volumes) ; `vanyline-tools` (`tools/src/filesystem.rs`, pour les opérations de fichiers).

## Exigences fonctionnelles

- **FR-1 - MUST:** `/ws/fs` offre une RPC JSON (une requête = une frame texte = une réponse) pour `root`, `read`, `write`, `edit`, `delete`, `mkdir`, `rename`, `list` et `flush-ack`. Tout chemin est confiné dans la racine avant effet.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Accepts, Returns, Must)
    - **Verified by:** `AC-1`
- **FR-2 - MUST:** `read` est numéroté et borné (200 lignes, 16 KiB) par défaut ; `raw: true` rend le contenu brut. Chaque réponse reprend le `id` verbatim, y compris en erreur. `flush-ack` porte l'identifiant de flush dans `ackFor` seulement.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Must ; Scenarios `lecture_raw_rend_le_contenu_brut`, `flush_ack_malforme_rejete_sans_resoudre_le_waiter`)
    - **Verified by:** `AC-2`
- **FR-3 - MUST:** Les frames de `fs_events` sont relayées telles quelles à chaque session ouverte après l'upgrade. Il n'y a ni rattrapage des événements antérieurs, ni filtrage par chemin ou par identité.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Handles, Must not)
    - **Verified by:** `AC-3`
- **FR-4 - MUST:** Chaque connexion `/ws/terminal` ouvre un PTY réel relié octet pour octet. Seule la frame texte `{"type":"resize","cols":N,"rows":M}` contrôle la taille ; tout autre texte est ignoré sans fermeture.
    - **Source:** `SpecDD /sandbox/src/ws/terminal.sdd` (Must, Accepts)
    - **Verified by:** `AC-4`
- **FR-5 - MUST:** Le shell démarre dans `sandbox_root` avec une taille initiale de 24×80. Le client ne fournit jamais le cwd, la taille initiale ni le binaire du shell. L'overlay `.venv` est recalculé à chaque spawn. À la fermeture, le groupe du shell reçoit SIGKILL ; les jobs détachés survivent par conception.
    - **Source:** `SpecDD /sandbox/src/ws/terminal.sdd` (Must, Must not, Handles)
    - **Verified by:** `AC-5`
- **FR-6 - MUST:** `/ws/lsp/{toolchain}` relaie un message JSON-RPC par frame texte dans les deux sens, sans framing `Content-Length`. Les frames binaires entrantes et les Ping/Pong sont ignorés.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Purpose, Accepts, Must)
    - **Verified by:** `AC-6`
- **FR-7 - MUST:** Après upgrade, une toolchain inconnue ferme en `4004` (`no lsp for toolchain`) et un spawn en échec ferme en `4005` (`lsp spawn failed`) avec un log `error`. Un échec d'envoi produit une frame `-32600` sans rompre la session.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Must, Returns, Raises)
    - **Verified by:** `AC-7`
- **FR-8 - MUST:** Le processus LSP est spawné paresseusement. Une session par toolchain est partagée par tous les clients, WebSocket comme outils. Elle survit aux déconnexions et est remplacée par un processus neuf après sa mort.
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Handles) ; `SpecDD /sandbox/src/ws/lsp.sdd` (Handles)
    - **Verified by:** `AC-8`
- **FR-9 - MUST:** Les URI `file:///<relatif>` entrantes deviennent absolues sous la racine, et les URI absolues sortantes sous la racine redeviennent relatives. Cela vaut pour les clés `uri`, `*Uri` et les clés de `changes`. Les autres URI sont inchangées.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Must, Exposes : `rewrite_uri_string`, `rewrite_uris`)
    - **Verified by:** `AC-9`
- **FR-10 - MUST:** Le premier `initialize` à `id` entier part réellement. Son issue, succès ou échec, est mise en cache et rejouée aux clients suivants sous leur propre `id`, sans octet vers l'enfant. Sans issue après 30 s, la réponse est `-32603`.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Handles, Exposes) ; `SpecDD /sandbox/src/lsp.sdd` (Must ; `wait_for_initialize_outcome`)
    - **Verified by:** `AC-10`
- **FR-11 - MUST:** Une toolchain composite fusionne ses réponses en une seule, primary-first. Le rôle `diagnostics-merge` ne reçoit jamais de requête. La mort d'un auxiliaire dégrade la session sans la rompre. Les identifiants internes ne sortent jamais vers un client.
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Must, Handles, Returns)
    - **Verified by:** `AC-11`
- **FR-12 - MUST:** Les diagnostics sont mis en cache par URI et fusionnés par part d'enfant (remplacement, jamais ajout). Le cache et les abonnés reçoivent le même contenu. Les outils distinguent `Some(non vide)`, `Some(vide)` et `None`.
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Handles, Returns) ; `SpecDD /sandbox/src/lsp_client.sdd` (Returns, `diagnostics`)
    - **Verified by:** `AC-12`
- **FR-13 - MUST:** `vnl-ruff-lsp` publie les diagnostics de `ruff check` sur le contenu du buffer, transmis sur stdin. `didOpen` et `didSave` lintent immédiatement, `didChange` après un debounce de 500 ms. Tout résultat périmé est abandonné.
    - **Source:** `SpecDD /sandbox/src/bin/ruff_lsp.sdd` (Must, Accepts)
    - **Verified by:** `AC-13`
- **FR-14 - MUST:** `vnl-hadolint-lsp` applique le même patron à `hadolint --format json --no-color -`. La fin d'un range est la fin de la ligne, en unités UTF-16.
    - **Source:** `SpecDD /sandbox/src/bin/hadolint_lsp.sdd` (Must, Returns)
    - **Verified by:** `AC-14`
- **FR-15 - MUST:** Les wrappers répondent `-32601` à toute requête hors `initialize` et `shutdown`. En cas d'échec du spawn, de sortie non JSON ou d'E/S, ils publient une liste vide et ne meurent jamais à cause de l'outil.
    - **Source:** `SpecDD /sandbox/src/bin/ruff_lsp.sdd` (Must, Returns) ; `SpecDD /sandbox/src/bin/hadolint_lsp.sdd` (Must, Returns)
    - **Verified by:** `AC-15`

## Sécurité

- **Confinement FS :** toute opération sur `path` ou `to` passe par `confine_path` avant effet, avec les marqueurs `VNL-SBX-001/002/003`. Un symlink sortant est rejeté (`SpecDD /sandbox/src/ws/fs.sdd`, Must ; `sandbox#BR-6`). Le `read` `raw` n'a pas de plafond (OI-11).
- **Confinement LSP :** le pont réécrit les URI, mais la spec ne contracte aucune garde. Selon la règle décrite, `file:///../x` devient `file://{racine}/../x` sans normalisation (OI-2).
- **Terminal :** aucun contrôle d'accès propre. Le ticket de `sandbox/access` est la seule barrière, avec la NetworkPolicy et l'Ingress, hors contrat (`SpecDD /sandbox/src/ws/terminal.sdd`, Handles). Le terminal exécute des commandes et la clé SSH de l'Owner y est présente (`sandbox#DEC-2`). Le modèle de menace est traité ailleurs (`sandbox#OI-1`).
- **Identité :** les claims du ticket sont jetés. Aucun log d'identité n'est émis par `fs`, `terminal` ou `ws/lsp` (`SpecDD /sandbox/src/ws/fs.sdd`, Handles).
- **Racine :** le navigateur ne reçoit jamais le chemin de la racine ; la réécriture le garantit dans les cas traités (`SpecDD /sandbox/src/ws/lsp.sdd`, Handles).
- **Wrappers :** argv littéraux, pas de shell, URI et contenu jamais en argv, garde-fou sur `filename` (`SpecDD /sandbox/src/bin/ruff_lsp.sdd`, Must not).
- **Limites :** ni quota applicatif, ni limite de débit, ni taille de trame épinglée (`SpecDD /sandbox/src/ws/fs.sdd`, Tasks ; OI-11).

## Conception

```text
Navigateur (ticket, sandbox/access)
  |-- /ws/fs        fs_session : RPC JSON -> vanyline_tools::filesystem (confine_path) ; relais fs_events
  |-- /ws/terminal  terminal_session : PTY portable-pty -> shell ; binaire = octets, texte = resize
  `-- /ws/lsp/{tc}  lsp_session -> LspManager::get_or_spawn(tc) -> LspSession (primaire + aux)
                                      |-- stdio Content-Length -> serveur primaire
                                      `-- stdio Content-Length -> aux (ruff, hadolint, tsserver-forward)
Outils MCP lsp_* (sandbox/mcp) -> LspClient (lsp_client.rs) -> même LspSession
```

- **Mode mono-processus :** les octets sont transmis verbatim (notifications) ou avec identifiant restauré (réponses). Aucune table de fusion n'est consultée (`SpecDD /sandbox/src/lsp.sdd`, Must, invariant 8).
- **Mode composite :** les requêtes de la table fermée `MergeRoute::All` (`hover`, `definition`, `typeDefinition`, `implementation`, `references`, `codeAction`, `prepareRename`, `rename`, `completion`) passent par une barrière, puis sont fusionnées. Les `semanticTokens` restent au primaire seul. Les autres méthodes vont au primaire.
- **Forwarding tsserver :** `tsserver/request` du primaire est exécuté par `workspace/executeCommand` sur l'auxiliaire `tsserver-forward`, et la réponse revient au primaire en `tsserver/response`. Aucune trame n'atteint le navigateur.
- **Wrappers :** `didOpen` et `didSave` lintent sans debounce. `didChange` attend 500 ms. La génération `(version, génération)` est capturée avant le spawn. Le résultat est publié seulement s'il est encore courant.
- **Terminal :** argv du shell réduit au programme, sans flag (bash non-login). L'environnement hérité du serveur est complété par l'overlay `VIRTUAL_ENV`.

## Décisions

- **DEC-1 - Un processus LSP par toolchain et par pod, partagé**
    - **Context:** plusieurs onglets et les outils MCP sollicitent la même toolchain. Un serveur réel rejette un second `initialize`, observé selon la spec du pont.
    - **Decision:** une `LspSession` par toolchain, spawn paresseux, survie aux déconnexions, arrêt par kill seul (pas de `shutdown` ni `exit`).
    - **Consequences:** un seul `initialize` par processus vivant. Les caches sont partagés. Aucun arrêt par inactivité n'existe (OI-9).
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Purpose, Handles) ; `SpecDD /sandbox/src/ws/lsp.sdd` (Purpose)
- **DEC-2 - Le navigateur ne voit que des URI relatives**
    - **Context:** les serveurs exigent des URI absolues ; le navigateur ne doit pas connaître la racine.
    - **Decision:** réécriture bidirectionnelle par walker JSON récursif, sans allowlist de types de messages.
    - **Consequences:** toute nouvelle clé `*Uri` est couverte sans modification du code.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Purpose, point 2 ; Must)
- **DEC-3 - Le replay d'`initialize` rejoue l'issue réelle**
    - **Context:** un serveur peut échouer à l'initialisation (exemple cité : `typescript-language-server` sans `node_modules`).
    - **Decision:** l'issue, succès ou échec, est cachée et rejouée à l'identique aux clients suivants.
    - **Consequences:** un échec est propagé à tous les clients, sans attente ni erreur générique.
    - **Source:** `SpecDD /sandbox/src/ws/lsp.sdd` (Handles) ; `SpecDD /sandbox/src/lsp.sdd` (Scenario `issue_initialize_echec_cachee_aussi`)
- **DEC-4 - Pas de surveillant de fichiers, relais des seules notifications publiées**
    - **Context:** le workspace est un volume local `RWO` (`workspaces#CON-2`).
    - **Decision:** aucune notification pour les changements hors `edit_and_check` et hors demande de flush.
    - **Consequences:** une modification faite par le terminal ou par git n'est signalée à aucun client (CON-5).
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (Purpose, Handles, Must not)
- **DEC-5 - Terminal : binaire = données, texte = contrôle, jobs détachés conservés**
    - **Context:** le contrôle en bande et le keepalive sont des pièges documentés.
    - **Decision:** les frames binaires ne sont jamais interprétées. Le texte ne porte que `resize`. Les jobs `&` survivent à la fermeture.
    - **Consequences:** le service de développement en arrière-plan reste possible. Aucun job n'est tué hors du groupe foreground.
    - **Source:** `SpecDD /sandbox/src/ws/terminal.sdd` (Must, Must not)
- **DEC-6 - Wrappers : contenu sur stdin, argv littéraux**
    - **Context:** une URI ou un contenu en argv ouvrirait une injection.
    - **Decision:** le contenu du buffer passe sur stdin, les argv sont littéraux, aucun shell n'est utilisé.
    - **Consequences:** le wrapper ne lit jamais le fichier désigné par l'URI, et le résultat peut être plus ancien ou plus récent que le disque.
    - **Source:** `SpecDD /sandbox/src/bin/ruff_lsp.sdd` (Must not) ; `SpecDD /sandbox/src/bin/hadolint_lsp.sdd` (Must not)
- **DEC-7 - Sévérité ruff relayée**
    - **Context:** ruff 0.16.6 pose `error` sur des règles de style.
    - **Decision:** la sévérité ruff est relayée telle quelle, avec un repli à Warning.
    - **Consequences:** `F401` et `I001` remontent en erreur ; divergence avec hadolint (OI-5).
    - **Source:** `SpecDD /sandbox/src/bin/ruff_lsp.sdd` (Returns, décision citée du 2026-09-08)
- **DEC-8 - Fusion composite primary-first, sans timeout de barrière**
    - **Context:** un primaire qui ne répond jamais bloque la barrière.
    - **Decision:** la fusion est ordonnée par index d'enfant, et aucune barrière n'a de timeout.
    - **Consequences:** un primaire muet bloque son client (OI-9).
    - **Source:** `SpecDD /sandbox/src/lsp.sdd` (Handles, Barrière de fusion)

## Réalisation SpecDD

- `SpecDD /sandbox/src/ws/mod.sdd` : déclare `fs`, `lsp`, `terminal` et `ticket`. Le module `ticket` n'appartient pas à cette Solution.
- `SpecDD /sandbox/src/ws/fs.sdd` → `sandbox/src/ws/fs.rs` (FR-1 à FR-3).
- `SpecDD /sandbox/src/ws/terminal.sdd` → `sandbox/src/ws/terminal.rs` (FR-4, FR-5).
- `SpecDD /sandbox/src/ws/lsp.sdd` → `sandbox/src/ws/lsp.rs` (FR-6, FR-7, FR-9, FR-10, DEC-2, DEC-3).
- `SpecDD /sandbox/src/lsp.sdd` → `sandbox/src/lsp.rs` (FR-8, FR-10, FR-11, FR-12).
- `SpecDD /sandbox/src/lsp_client.sdd` → `sandbox/src/lsp_client.rs` (FR-8, FR-12 côté outils).
- `SpecDD /sandbox/src/bin/ruff_lsp.sdd` → `sandbox/src/bin/ruff_lsp.rs` (FR-13, FR-15).
- `SpecDD /sandbox/src/bin/hadolint_lsp.sdd` → `sandbox/src/bin/hadolint_lsp.rs` (FR-14, FR-15).
- Consommés hors Solution : `SpecDD /sandbox/src/ws/ticket.sdd` (`sandbox/access`), `SpecDD /sandbox/src/tools_impl.sdd` (`sandbox/mcp`), `SpecDD /sandbox/src/venv.sdd` (`sandbox/runtime`), `SpecDD /controller/src/sandbox.sdd` (`workspaces/sandboxes`).

## Acceptation et vérification

- **AC-1:** Un `read`, `write`, `list` ou `rename` vers un chemin sortant (`..`, absolu hors racine, symlink sortant) rend `VNL-SBX-001` sans effet ; une destination sortante ne déplace rien.
- **AC-2:** `read` sans option est numéroté et borné ; `raw:true` est brut. Un `id` est recopié verbatim, y compris en erreur. `flush-ack` avec `ackFor` non entier rend `flush-ack requires numeric ackFor` sans résoudre de waiter.
- **AC-3:** Une frame publiée sur `fs_events` parvient au client sans transformation. Une session ouverte après publication ne reçoit rien de ce qui précède.
- **AC-4:** Un octet binaire écrit sur le PTY revient en sortie sans transformation. `{"type":"resize","cols":100,"rows":40}` change la taille. Un texte hors gabarit est ignoré.
- **AC-5:** Un shell lancé dans un répertoire contenant `.venv/pyvenv.cfg` voit `VIRTUAL_ENV`. Un job au premier plan meurt à la fermeture, un job `&` survit.
- **AC-6:** Une frame texte est relayée au processus sans `Content-Length` côté WebSocket. Une frame binaire et un Ping sont ignorés sans fermeture.
- **AC-7:** Une toolchain inconnue ferme en `4004` avec la raison exacte ; un spawn en échec ferme en `4005` avec un log `error`. Un `id` non entier rend `-32600`, et la session continue.
- **AC-8:** Un client qui se déconnecte ne tue pas le processus. Après kill du processus, une nouvelle connexion spawne un processus neuf.
- **AC-9:** `file:///src/main.rs` devient `file://{racine}/src/main.rs` à l'entrée, et l'inverse en sortie. Une URI `http://` ne change pas. Les clés de `WorkspaceEdit.changes` sont réécrites.
- **AC-10:** Deux clients envoient `initialize`. Le processus n'en reçoit qu'un. Le second reçoit l'issue du premier, y compris une erreur.
- **AC-11:** Un `hover` composite donne une seule réponse fusionnée, primary-first. Une méthode hors table n'atteint que le primaire. Un auxiliaire `diagnostics-merge` ne reçoit aucune requête.
- **AC-12:** Le cache et les abonnés reçoivent le même fondu. Une publication d'enfant remplace sa part. `wait_for_diagnostics` distingue `Some([])` de `None`.
- **AC-13:** `vnl-ruff-lsp` publie sans debounce après `didOpen` et `didSave`, et après 500 ms pour `didChange`. Un résultat périmé n'est jamais publié. Si ruff est absent, la publication est vide.
- **AC-14:** `vnl-hadolint-lsp` suit le même comportement, et la fin de range est la fin de ligne en UTF-16.
- **AC-15:** Une requête hors `initialize` et `shutdown` rend `-32601`. Une sortie non JSON publie `[]`, et l'EOF de stdin termine avec le code `0`.

Les Scenarios de session WebSocket n'ont pas de harnais à ce jour (OI-8).

## Open Issues

- **OI-1 - Ping/Pong sur `/ws/fs`:** `fs.sdd` rend `unexpected binary frame` sur Ping et Pong, alors que `terminal.sdd` et `ws/lsp.sdd` les ignorent. Contradiction entre specs, reconnue en Tasks `[!]` de `fs.sdd`. **TBD.**
- **OI-2 - Confinement des URI LSP:** `sandbox#AC-4` exige le refus d'un chemin sortant par URI LSP. `ws/lsp.sdd` ne contient aucune garde, et `lsp.sdd` (Must not) renvoie le confinement vers `ws/lsp.rs` et `tools_impl`. **TBD :** contractualiser la garde, ou amender `AC-4`.
- **OI-3 - Référence obsolète à `tools_impl`:** `fs.sdd` (Depends on) dit que `tools_impl.sdd` « n'existe pas encore ». Le fichier existe et contracte `confine_path` sur `spawn_blocking`, alors que `fs.sdd` (Tasks `[?]`) appelle la fonction de façon synchrone. **TBD.**
- **OI-4 - Codes d'erreur:** `sandbox#NFR-1` exige un identifiant unique. Les littéraux de protocole de `fs` n'en ont pas, `-32600` masque `VNL-SBX-LSP-001/002/003`, et les messages de spawn de `lsp.sdd` n'en ont pas non plus. `lsp.sdd` attribue `VNL-SBX-LSP-012/013` aux deux wrappers, alors que `hadolint_lsp.sdd` n'émet que du texte en clair. **TBD.**
- **OI-5 - Divergences ruff et hadolint:** hadolint publie `code: null` là où ruff omet la clé ; le repli de sévérité est 2 chez ruff et 3 chez hadolint ; `style` vaut 4 chez hadolint seul ; les colonnes sont en scalaires Unicode chez ruff et en UTF-16 chez hadolint. Les deux specs demandent une convergence non tranchée. **TBD.**
- **OI-6 - Frontière avec `sandbox/mcp`:** `sandbox.md` place le LSP à la fois dans `sandbox/mcp` et dans `sandbox/editor`. Cette Solution prend le multiplexeur, le client et les wrappers ; les outils restent à `sandbox/mcp`. **TBD :** confirmer.
- **OI-7 - Accès navigateur sans Application:** `CON-6` implique qu'un Owner sans Application n'a pas d'éditeur navigateur. `AGENTS.md` décrit un accès direct. **TBD :** modèle pour un Owner sans Application.
- **OI-8 - Harnais de session WebSocket et test ruff:** les Scenarios d'upgrade réel (Close, Ping, multi-session) n'ont pas de harnais, comme indiqué dans `fs.sdd`, `terminal.sdd`, `ws/lsp.sdd` et `lsp.sdd`. `ruff_lsp.sdd` note que son harnais `../tests/ruff_lsp.rs` n'existe pas. **TBD :** choix du harnais.
- **OI-9 - Cycle de vie LSP:** kill seul, pas d'idle timeout, barrière sans timeout, et `wait_for_initialize_outcome` bloque 30 s la boucle du client perdant (`lsp.sdd`, `ws/lsp.sdd`, `lsp_client.sdd` Tasks). **TBD.**
- **OI-10 - Robustesse LSP:** `FrameReader` croît sans borne sur un en-tête invalide (`[!]`). Un `pending` fuit sur `-003` (`[!]`). `initialized` est consommé avant l'envoi (`[!]`). `initialize` jette l'erreur serveur et rend « neither result nor error » (`lsp_client.sdd`, `[!]`). Le percent-encodage de la racine n'est pas géré, et `file://{racine}` sans slash n'est pas réécrit. **TBD.**
- **OI-11 - Limites FS et terminal:** `raw:true` sans plafond ; détection `truncated` par sous-chaîne (`[!]`) ; `TicketQuery` mort ; `edit` renvoie le message sous `content` ; `TERM` non posé ; `SHELL` vide non géré ; cast `as u16` tronquant ; canal mpsc de 16 384 messages de 16 KiB ; aucun quota de sessions ; enfant jamais `wait()`. Sources : `fs.sdd` et `terminal.sdd` (Tasks). **TBD.**
- **OI-12 - Wrappers:** stderr du fils ruff et hadolint jeté ; aucun timeout sur le fils ; aucune machine d'état LSP ; `id: null` traité comme notification ; le `ruff` du `.venv` utilisateur est invisible au wrapper ; `eprintln!` sans exemption dédiée (`tooling.sdd`). Sources : `ruff_lsp.sdd` et `hadolint_lsp.sdd` (Tasks). **TBD.**

## Références

- `sandbox` : `FR-1`, `FR-2`, `BR-1`, `BR-3`, `BR-5`, `BR-6`, `BR-9`, `CON-3`, `NFR-1`, `AC-2`, `AC-4`, `DEC-2`, `OI-1`.
- `workspaces` : `CON-2`. `workspaces/sandboxes` : `FR-7`, `FR-8`.
- `AGENTS.md`, `vanyline.sdd`, `docs/architecture.md` (section « WebSocket éditeur », citée par les specs, non relue).
