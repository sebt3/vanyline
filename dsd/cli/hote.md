# Solution Design: Hôte lymaille de la CLI

## Résumé

Cette Solution fait de la CLI un hôte de la librairie lymaille : elle implémente les traits de stockage sur des fichiers,
résout les références vers les secrets et les connexions, dérive le contexte de travail depuis le cluster, et tient le
journal d'audit des scripts. Elle ne contient ni le TUI, ni les commandes `run` et `script`, ni la synchronisation de
`.vanyline/` : ce sont d'autres Solutions du domaine `cli`, qui la consomment.

## Problème et objectif

Lymaille ne fait aucune entrée-sortie : elle définit des traits que l'hôte implémente (`agent-sessions#CON-1`). La CLI,
hôte permanent et autonome (`cli#DEC-2`), doit fournir ces implémentations pour qu'une session s'ouvre, se persiste et se
reprenne sans `app`. Le développeur utilise sa propre instance d'`app` ; un second développeur utilise la sienne ; les deux
partagent leurs configurations de projet par le dépôt git, dans `.vanyline/` (`cli#DEC-4`, `cli#DEC-11`). Les fichiers de
cette Solution sont donc le support de cet échange et doivent rester lisibles et éditables à la main.

## Périmètre et non-objectifs

La Solution couvre : les stores de session, d'agents, de profils, de providers, de toolsets, de types de contexte, de
contextes et de mémoires ; la résolution des secrets ; la dérivation du ToolsetContext ; la connexion MCP à la sandbox
avec le jeton d'Owner ; le journal d'audit et l'index des sessions.

Hors périmètre : l'affichage (Solution `cli/chat`), `run` et `script` (Solution `cli/batch`), la synchronisation
(Solution `cli/sync`), les commandes de gestion d'objets Kubernetes (Solution `cli/objets`), la création et la
modification interactives des définitions par un agent de configuration (`cli#OI-4`), et le transfert avec `app`
(`app#OI-7`).

## Contexte applicable du domaine

`cli#BR-1` : la CLI présente le jeton d'Owner obtenu par le kubeconfig. `cli#DEC-2` à `DEC-7`, `DEC-9`, `DEC-10` :
hôte permanent, stockage dans la crate `cli`, ToolsetContext dérivé, deux niveaux, secrets hors couches versionnées,
dépôt local. `agent-sessions#BR-9` : la session démarre sur un contexte de travail. `agent-sessions#CON-1` : le stockage
passe par des traits que l'hôte implémente. `sandbox#BR-1` : seul le jeton d'Owner est accepté.

## Acteurs

- **Développeur :** ouvre des sessions, édite ses fichiers de configuration, partage `.vanyline/` par git.
- **Agent (LLM) :** lit et peut écrire le niveau partagé quand sa définition le lui accorde ; ne choisit jamais son
  stockage.
- **Autre développeur :** consomme le même `.vanyline/` depuis sa propre instance d'`app` ou sa propre CLI.

## État actuel et état cible

- **Actuel (Observed in: `cli/src/store.rs`, `cli/src/config.rs`, `cfgstore/`) :** stockage YAML à deux couches de
  `vanyline-cfgstore`, conversations sauvegardées par `store.rs`.
- **Cible :** implémentations des traits de lymaille dans un module `host` de la crate `cli` ; `cfgstore` retiré.

## Hypothèses, contraintes et dépendances

- **CON-1:** Les futures des traits de lymaille sont `Send` et les traits s'utilisent par paramètre générique, jamais en
  objet de trait.
    - **Source:** `agent-sessions/session#FR-1`

- **CON-2:** Les traits de stockage de lymaille n'ont ni listage ni suppression pour les agents, les sessions, les
  providers et les profils : ces opérations relèvent de l'hôte.
    - **Source:** `agent-sessions/agents`, `agent-sessions/session#DEC-1`

- **CON-3:** Un fichier de configuration ne contient jamais de valeur secrète ; un provider ne référence son
  authentification que par un nom.
    - **Source:** `cli#DEC-9` ; `agent-sessions/agents#FR-6`

## Exigences fonctionnelles

- **FR-1 - MUST:** La CLI implémente `SessionStore` sur des fichiers, un état de session par fichier, et écrit
  atomiquement : un état n'est jamais visible à moitié écrit. La dernière écriture gagne, comme le prévoit lymaille.
    - **Source:** `agent-sessions/session#FR-1`, `agent-sessions/session#DEC-1`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** La CLI implémente `AgentStore`, `ProfileStore`, `ProviderStore`, `ToolsetStore`, `ContextTypeStore`,
  `ContextStore` et `MemoryStore` sur des fichiers lisibles à la main, répartis en deux niveaux : utilisateur
  (`$HOME/.config/vanyline/`) et partagé (`.vanyline/` du dépôt courant). Une lecture consulte le niveau partagé d'abord
  puis le niveau utilisateur (`Layered`) ; une écriture désigne explicitement son niveau.
    - **Source:** `cli#DEC-7` ; `agent-sessions/agents#FR-5`
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** Sans dépôt courant ni option de répertoire (`cli#DEC-10`), seul le niveau utilisateur existe, sans
  erreur.
    - **Source:** `cli#DEC-10`

- **FR-4 - MUST:** Une définition invalide est refusée à l'écriture par la validation de lymaille ; un fichier illisible à
  la lecture est signalé avec son chemin et sa cause, sans interrompre les autres lectures.
    - **Source:** `agent-sessions/agents#FR-1`

- **FR-5 - MUST:** La CLI résout un nom de secret dans `$HOME/.config/vanyline/secrets/secrets.yaml`. Elle refuse de lire
  ce fichier si lui-même n'est pas en mode 600 ou son répertoire en mode 700, ou si l'un des deux a des droits plus larges.
    - **Source:** `cli#DEC-9`
    - **Verified by:** `AC-3`

- **FR-6 - MUST:** La CLI ne persiste, ne journalise et n'affiche jamais la valeur d'un secret résolu.
    - **Source:** `agent-sessions#CON-3`
    - **Verified by:** `AC-3`

- **FR-7 - MUST:** À l'ouverture d'une session sur `--context <projet>` et `--sandbox <nom>`, la CLI lit les CR Project et
  Sandbox avec le kubeconfig de l'utilisateur et construit le ToolsetContext éphémère décrit par `cli#DEC-6` ; le
  ContextType correspondant est livré avec la CLI. Elle ouvre ensuite la session sur le point de départ que lymaille
  résout depuis ce contexte (`resolve_start`).
    - **Source:** `cli#DEC-6` ; `agent-sessions/toolsets#FR-21`
    - **Verified by:** `AC-4`

- **FR-8 - MUST:** Le rôle MCP « sandbox » du contexte est lié au Service de la sandbox ; chaque appel présente le jeton
  du ServiceAccount de l'Owner (TokenRequest, audience `vanyline-sandbox`). Le jeton est renouvelé avant son expiration
  pendant une session longue, sans interrompre le tour en cours.
    - **Source:** `cli#BR-1`, `sandbox#BR-1`
    - **Verified by:** `AC-5`

- **FR-9 - MUST:** La CLI écrit un journal d'audit des scripts exécutés (un enregistrement par script : session, agent,
  texte complet, résultat) et implémente `SessionIndex` : le listage des sessions pour `cli#FR-5` et les erreurs de script
  d'une session, lues dans ce journal.
    - **Source:** `agent-sessions/session#FR-8`, `agent-sessions/toolsets` (`SessionIndex`) ; `cli#FR-5`

- **FR-10 - SHOULD:** La CLI refuse d'ouvrir une session déjà ouverte par un autre processus de la CLI.
    - **Source:** `agent-sessions/session#FR-4` (un tour actif par session, au sein d'un processus seulement)
    - **Rationale:** lymaille ne garantit pas l'exclusion entre processus ; la dernière écriture gagnerait sinon en silence.

## Sécurité

Les fichiers du niveau partagé sont écrits par des agents de moindre confiance que ceux du niveau utilisateur et circulent
par git : lire un fichier du niveau partagé ne l'active pas au-delà de ce que la définition de l'agent accorde déjà, et le
contexte de la session décide des toolsets offerts (`agent-sessions#BR-9`). Le niveau partagé ne contient aucun secret
(`CON-3`) ; un fichier qui en contiendrait un est détecté par le détecteur de lymaille à l'écriture et à la
synchronisation (`cli#FR-6`).

## Structure et disposition des fichiers

Disposition validée par le développeur le 2026-11-07 (basée sur les catégories du manifeste de projet de lymaille) : un
répertoire par catégorie sous chaque niveau.

```text
<niveau>/                 $HOME/.config/vanyline/  ou  .vanyline/
|-- agents/               une définition par fichier (frontmatter + prompt)
|-- profiles/  providers/ un fichier par entité nommée
|-- toolsets/             actifs et brouillons
|-- context-types/  contexts/
`-- memories/             par portée
```

Hors des niveaux de configuration : `$HOME/.config/vanyline/secrets/secrets.yaml` (`FR-5`), et le répertoire d'état local
`$HOME/.local/state/vanyline/` pour les sessions et le journal d'audit (validé le 2026-11-07).

Formats, lus dans lymaille (0.2.0) :

- **Agent :** `AgentDefinition::to_text` et `from_text`, frontmatter YAML puis prompt en corps ; un fichier `.md` par
  agent (`agent-sessions/agents#FR-3`).
- **Skill :** texte `SKILL.md` (frontmatter YAML puis corps, compatible Agent Skills), lu par `Skill::parse`
  (`agent-sessions/toolsets#FR-7`).
- **Provider, profil, ContextType, ToolsetContext, mémoire :** types `serde`, sans format texte de lymaille ; la CLI les
  écrit en YAML. Un provider ne porte que des références (`CON-3`).
- **Toolset :** un répertoire par toolset (`DEC-2`), car l'enveloppe `serde` enferme le module Rhai dans une chaîne,
  illisible à la main et mal adaptée à la revue de diff.

```text
toolsets/<nom>/
|-- toolset.yaml          métadonnées, version, exigences
|-- module.rhai           le code du module
`-- skills/<skill>/SKILL.md   un skill et ses documents annexes
```

Un brouillon (`DEC-1`) vit dans un répertoire frère, `toolsets-drafts/<nom>/`, de même forme (**ASSUMPTION**, à confirmer
à l'écriture de la spec du store de toolsets).

## Décisions

- **DEC-1 - Le brouillon d'un toolset suit le modèle brouillon et actif de lymaille**
    - **Context:** `ToolsetStore` distingue `get_active`, `get_draft` et `put_draft` ; seule la promotion d'un brouillon
      le rend actif.
    - **Decision:** un toolset arrivé par synchronisation ou écrit par un agent est un brouillon. L'activer est une action
      humaine hors session.
    - **Consequences:** la promotion exige une commande de la CLI (nom à fixer dans la spec).
    - **Source:** `agent-sessions/toolsets#FR-8` ; `app#DEC-3`

- **DEC-2 - Un toolset s'écrit en répertoire, pas en enveloppe `serde`**
    - **Context:** le format d'échange entre développeurs est lu, édité et revu dans git.
    - **Decision:** `toolset.yaml` pour les métadonnées, `module.rhai` pour le code, `skills/<skill>/SKILL.md` pour les
      skills ; la CLI convertit vers et depuis le type de lymaille.
    - **Alternatives:** un fichier YAML par toolset avec le Rhai en bloc littéral (écarté : diff et édition moins
      confortables) ; le JSON `serde` tel quel (écarté : illisible).
    - **Source:** décision du développeur, 2026-11-07

## Réalisation SpecDD

Les specs seront créées sous `cli/src/host/` (**ASSUMPTION** de découpage, un fichier source par contrat) : stores de
session et d'index, stores de catalogue, store de toolsets et de contextes, store de mémoires, secrets, contexte dérivé,
jeton d'Owner et journal d'audit. À retirer à parité : `SpecDD /cli/src/store.sdd`, `SpecDD /cli/src/config.sdd`,
`SpecDD /cli/src/config_check.sdd` et les specs de `cfgstore/`.

## Compatibilité et migration

Les conversations existantes de `store.rs` ne sont pas reprises : leur format dépend de `ChatEvent` de `vanyline-lib`
(`app#DEC-1`). Les fichiers YAML de `cfgstore` sont lus une fois, par une commande de migration ou à la main : **TBD**,
à trancher avec le développeur avant le retrait de `cfgstore`.

## Acceptation et vérification

- **AC-1:** Une session sauvegardée puis rechargée par un nouveau processus rend le même état ; une écriture interrompue
  laisse l'état précédent intact.
- **AC-2:** Un agent défini aux deux niveaux est lu au niveau partagé ; supprimé du niveau partagé, il redevient celui du
  niveau utilisateur.
- **AC-3:** `secrets.yaml` en mode 644, ou son répertoire en 755, est refusé avec une erreur explicite ; la valeur d'un
  secret résolu n'apparaît ni dans un état de session, ni dans le journal d'audit, ni dans un message d'erreur.
- **AC-4:** `--context <projet> --sandbox <nom>` produit un ToolsetContext dont les paramètres viennent du projet, de la
  branche et du namespace, et dont le rôle « sandbox » est lié au bon Service.
- **AC-5:** Une session plus longue que la validité du jeton d'Owner continue de joindre la sandbox.

## Risques

- Le format de fichier est le contrat d'échange entre développeurs : le changer casse les dépôts partagés. Il doit être
  versionné et validé avant la première utilisation (voir Open Issues).

## Open Issues

- **OI-1:** **Disposition d'une mémoire sur disque.** Une mémoire a une portée (niveau et référence) puis un identifiant ;
  proposition : `memories/<référence>/<id>.md` sous chaque niveau, corps en markdown. **TBD.**
- **OI-2:** **Format de `secrets.yaml`.** Association nom vers valeur ; création et édition par une commande de la CLI, ou
  seulement à la main. **TBD.**
- **OI-3:** **Migration de `cfgstore`** (voir Compatibilité et migration). **TBD.**
- **OI-4:** **Versionnement du format de `.vanyline/`.** Un champ de version, ou le seul format de lymaille. **TBD.**

## Références

`cli` (`cli#DEC-2` à `DEC-11`), `agent-sessions` de lymaille (`session`, `agents`, `toolsets`, `modules`),
`sandbox#BR-1`, `workspaces`, `app#OI-7`.
