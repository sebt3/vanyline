# Domain Definition: Ligne de commande vanyline

## Résumé

Le domaine `cli` couvre le binaire `vanyline` : un chat en terminal (TUI), l'exécution ponctuelle d'une tâche, la gestion
de la configuration et des objets Kubernetes, et un serveur d'intégration JSON-RPC sur l'entrée et la sortie standard.

Statut du document : **ébauche** (2026-11-07). La direction est décidée (`DEC-2` à `DEC-4`) : la CLI devient un hôte
lymaille permanent et autonome. Les Solutions restent à écrire.

## Objectif et problème

Le développeur et les autres clients (intégrations, kydah-code) pilotent vanyline sans navigateur : discuter avec un LLM
dans un terminal, gérer les Owners, Projects et Sandboxes, et se brancher sur le MCP d'une sandbox.

## Objectifs métier

- **BO-1:** Piloter vanyline depuis un terminal : conversation, objets Kubernetes, configuration.
    - **Source:** `AGENTS.md`, composant cli

- **BO-2:** Offrir une interface d'intégration programmatique sans navigateur.
    - **Source:** `vanyline.sdd`, tâche ouverte (décision du 2026-10-04, serveur RPC conservé)

- **BO-3:** Automatiser en batch, sans LLM, des opérations sur une sandbox (scripts Rhai).
    - **Source:** décision du développeur, 2026-11-07

## Périmètre et non-objectifs

Le domaine couvre : le chat TUI, `run`, `script`, la gestion des sessions, la gestion des Owners, Projects et Sandboxes,
la configuration locale de l'hôte, le branchement sur la sandbox (`--sandbox`) et `serve --stdio`.

Hors périmètre : la conduite du LLM (domaine `agent-sessions` de lymaille), le contenu de la sandbox (domaine `sandbox`),
le fonctionnement des objets Kubernetes (domaine `workspaces`).

## Acteurs

- **Développeur :** utilisateur principal.
- **Client d'intégration :** consomme le serveur RPC (`serve --stdio`).
- **kydah-code :** client MCP de la sandbox (jeton d'Owner par kubeconfig).

## État actuel et état cible

- **Actuel (Observed in: `cli/src/`) :** TUI ratatui, REPL, `run` (texte ou JSONL), commandes `owner`, `project`,
  `sandbox`, `mcp`, `model`, `skill`, `toolset`, stockage de configuration YAML à deux couches, serveur RPC. Le chat
  s'appuie sur `vanyline-lib` et `vanyline-cfgstore`.
- **Cible :** hôte lymaille permanent et autonome (`DEC-2`), TUI ratatui conservé, `run` pour l'usage batch avec LLM,
  `script` pour l'usage batch sans LLM, stockage par fichiers dans la crate `cli` (`DEC-3`). Les commandes de
  configuration `model`, `skill`, `toolset` et `mcp` sont à réexaminer avec la Solution de configuration.

## Hypothèses, contraintes et dépendances

- **CON-1:** Le terminal est requis pour le TUI ; le chat n'a pas de mode sans terminal.
    - **Source:** `AGENTS.md`, composant cli

- **CON-2:** Les commandes de gestion d'objets Kubernetes agissent avec le contexte kubeconfig de l'utilisateur ; la
  sandbox n'accepte que le jeton d'Owner (`sandbox#BR-1`).
    - **Source:** `AGENTS.md`, tableau des clients de la sandbox

## Glossaire

- **TUI :** interface en terminal du chat.
- **Contexte (`--context`) :** le Project dans lequel la session s'ouvre. Il fixe l'agent principal et les skills
  (`DEC-5`).
- **Sandbox (`--sandbox`) :** instance existante dont le MCP est exposé à la session ou au script sous forme de modules
  Rhai. Remplace l'ancien mode `--toolbox`.
- **Serveur RPC :** `vanyline serve --stdio`, JSON-RPC 2.0, décrit par `docs/rpc-protocol.md`.

## Règles du domaine

- **BR-1 - MUST:** La CLI ne contourne pas l'authentification de la sandbox : elle présente le jeton d'Owner obtenu par le
  contexte kubeconfig de l'utilisateur.
    - **Source:** `sandbox#BR-1`
    - **Verified by:** `AC-1`

- **BR-2 - MUST:** La CLI applique elle-même les règles de nommage et d'immutabilité des objets (`<projet>-<nom>`,
  longueur limite), en réutilisant le nommage de `vanyline-crds` plutôt qu'une copie.
    - **Source:** `workspaces#BR-3`
    - **Verified by:** `AC-4`

- **BR-3 - MUST:** Un script exécuté par `script` ne dispose que des modules que le contexte accorde à une session ; il
  n'a pas plus de pouvoir qu'une session de ce contexte.
    - **Source:** décision du développeur, 2026-11-07 (proposition validée)
    - **Verified by:** `AC-3`

## Exigences fonctionnelles transverses

- **FR-1 - MUST:** `vanyline` crée, modifie, supprime, démarre et arrête les Projects et les Sandboxes, et gère les
  Owners, par les commandes `owner`, `project` et `sandbox`, avec le contexte kubeconfig de l'utilisateur. Une
  modification qui demande un redémarrage affiche le signalement porté par l'état de la sandbox.
    - **Source:** `AGENTS.md`, composant cli ; `workspaces#FR-3`, `workspaces#BR-5`

- **FR-2 - MUST:** `vanyline chat --context <projet> [--sandbox <nom>]` ouvre une conversation en TUI. Sans `--sandbox`,
  la session n'a aucune action sur le code.
    - **Source:** décision du développeur, 2026-11-07

- **FR-3 - MUST:** `vanyline run` exécute une tâche ponctuelle avec LLM, sortie texte ou JSONL, pour un usage batch.
    - **Source:** décision du développeur, 2026-11-07

- **FR-4 - MUST:** `vanyline script <fichier.rhai> --context <projet> --sandbox <nom>` exécute le script avec les
  modules du MCP de la sandbox, sans LLM. Le résultat du script va sur la sortie standard, les erreurs sur la sortie
  d'erreur, et le code de retour reflète le succès. Le délai d'exécution des commandes est configurable.
    - **Source:** décision du développeur, 2026-11-07

- **FR-5 - MUST:** La CLI permet de lister, reprendre et supprimer ses sessions : lymaille laisse ces opérations à l'hôte.
    - **Source:** `lymaille` `agent-sessions#CON-1`

## Décisions du domaine

- **DEC-1 - Le serveur RPC est conservé**
    - **Context:** son premier consommateur, l'extension VS Code, est supprimée.
    - **Decision:** il reste une interface d'intégration pour d'autres clients.
    - **Source:** `vanyline.sdd`, tâche ouverte (décision du 2026-10-04)

- **DEC-2 - La CLI est un hôte lymaille permanent et autonome**
    - **Context:** `app#DEC-1` retire `vanyline-lib`, dont le chat de la CLI dépend.
    - **Decision:** la CLI héberge elle-même ses sessions lymaille (hôte = le processus qui ouvre la session), sans
      dépendre d'`app` ni de l'avancement de l'intégration dans miryad-core. Les règles de lymaille s'appliquent : relais
      `fs` et `shell` désactivés, toute action sur le code passe par le MCP de la sandbox avec le jeton d'Owner. Le
      TUI ratatui est conservé ; son couplage à `ChatEvent` est remplacé par un adaptateur des événements lymaille, et
      l'approbation de commande en session est retirée.
    - **Alternatives:** client REST et SSE de l'hôte (écarté) ; abandon du chat (écarté).
    - **Source:** décision du développeur, 2026-11-07

- **DEC-3 - Le stockage est implémenté dans la crate `cli`**
    - **Decision:** la CLI implémente les traits de stockage de lymaille (`SessionStore`, `AgentStore`, `ProfileStore`,
      `ProviderStore`) sur des fichiers, dans un module de `cli`. La crate `vanyline-cfgstore` est retirée du
      workspace avec `vanyline-lib` ; la partie utile à la CLI est reportée dans `cli`.
    - **Source:** décision du développeur, 2026-11-07

- **DEC-4 - Seuls agents et skills sont transférables entre la CLI et `app`**
    - **Decision:** les définitions d'agents et de skills, au format lymaille, se transfèrent entre la CLI et `app`. Le
      transfert (import et export) est de la responsabilité d'`app`, dans les deux sens ; la CLI l'ignore. Providers,
      modèles, serveurs MCP, toolsets accordés et sessions ne voyagent pas.
    - **Source:** décision du développeur, 2026-11-07

- **DEC-5 - Le contexte est un Project**
    - **Decision:** `--context` désigne un Project, cohérent avec le modèle d'`app` ; la sandbox est un paramètre
      distinct et optionnel.
    - **Source:** décision du développeur, 2026-11-07

## Transition et migration

Les specs existantes de `cli/` décrivent l'état actuel (environ 16 700 lignes, surtout le chat et le TUI). Le TUI reste,
son backend est remplacé (`DEC-2`) ; `--toolbox` devient `--sandbox` ; l'approbation de commande en session et le
stockage `cfgstore` sont retirés. Inventaire précis des specs à réécrire : à faire en lecture seule.

## Acceptation et vérification

- **AC-1:** Une commande de gestion de sandbox échoue sans jeton valide et réussit avec le contexte kubeconfig de
  l'Owner.
- **AC-2:** Un projet ou une sandbox créé par la CLI reçoit sa ligne d'identité côté `app` (`workspaces#AC-2`).
- **AC-3:** Un script qui appelle un module non accordé par le contexte échoue.
- **AC-4:** Créer une sandbox dont le nom dépasse la limite est refusé par la CLI avant tout appel au cluster.

## Open Issues

- **OI-2:** **Que devient `serve --stdio` ?** Il garde un contrat sans consommateur connu : conservé en suspens
  (`DEC-1`) tant qu'aucun client n'existe. **TBD.**
- **OI-4:** **Contexte de configuration en CLI.** `--context` désigne un Project (`DEC-5`) ; comment ouvre-t-on une
  session de configuration, qui n'a pas de Project ? **TBD.**
- **OI-5:** **Sandbox éphémère pour `script`.** Écartée à ce stade : `script` ne se branche que sur une sandbox
  existante. À rouvrir si un besoin CI apparaît.

## Références

- `AGENTS.md`, `docs/rpc-protocol.md`, `vanyline.sdd`.
- `app` (`app#DEC-1`), `sandbox` (`sandbox#BR-1`), `workspaces`, `agent-sessions` de lymaille.
