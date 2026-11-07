# Solution Design: Synchronisation de `.vanyline/`

## Résumé

Cette Solution décrit `vanyline sync push` et `vanyline sync pull` : la copie explicite du dossier `.vanyline/` entre le
dépôt local du développeur et le worktree d'une sandbox. C'est le support de l'échange de configuration entre deux
développeurs qui ont chacun leur infrastructure et leur instance d'`app` (`cli#DEC-11`).

## Problème et objectif

`.vanyline/` existe en deux copies : le dépôt local, et le worktree de la sandbox où l'agent peut écrire des skills et des
définitions pendant une session. La sandbox est aussi l'endroit où git pousse vers la forge, donc par où passe le partage
avec l'autre développeur. Il faut une copie fidèle, sans écraser du travail sans le dire, et sans faire entrer en local, sans
revue, du contenu écrit par un contexte de moindre confiance.

## Périmètre et non-objectifs

Couvre : les deux commandes, la détection de divergence, la propagation des suppressions, le canal vers la sandbox.

Hors périmètre : le commit et le push git de `.vanyline/` (depuis la sandbox ou en local, par les outils git habituels),
l'activation des brouillons de toolsets (`hote#DEC-1`), et l'import dans `app` (`app#OI-7`).

## Contexte applicable du domaine

`cli#FR-6`, `cli#DEC-11`, `cli#DEC-4`, `cli#DEC-7`, `cli#DEC-10`, `cli#BR-1`, `cli#AC-5`. `sandbox#BR-1` : seul le jeton
d'Owner est accepté.

## Acteurs

- **Développeur :** lance `sync` après avoir lu le diff.
- **Agent (LLM) :** écrit dans le `.vanyline/` du worktree de la sandbox ; ne lance jamais `sync`.
- **Sandbox :** expose l'accès fichier de l'éditeur, confiné à sa racine.

## État actuel et état cible

- **Actuel :** aucune commande de synchronisation ; le seul accès fichier distant est le WebSocket `/ws/fs` de l'éditeur,
  ouvert par `app` avec un ticket.
- **Cible :** la CLI mine elle-même un ticket avec le jeton d'Owner (`POST /ws/ticket`) et utilise `/ws/fs`.

## Hypothèses, contraintes et dépendances

- **CON-1:** `/ws/fs` offre `read` (avec `raw: true` : contenu brut intégral, jamais numéroté ni tronqué), `write` (parents
  créés, écriture atomique), `delete` (fichiers et répertoires vides), `mkdir` et `list` ; tout est confiné à la racine de la
  sandbox. La lecture par défaut est numérotée et bornée : elle ne convient pas à une copie.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd`

- **CON-2:** `list` est borné à 200 entrées avec un marqueur de troncature.
    - **Source:** `SpecDD /tools/src/output.sdd`

- **CON-3:** Un ticket est opaque, à usage unique, valable 30 secondes, et s'obtient par `POST /ws/ticket` avec le jeton
  d'Owner.
    - **Source:** `SpecDD /sandbox/src/ws/ticket.sdd`

- **CON-4:** `read` brut n'a aucune limite de taille côté sandbox (fichier entier en mémoire).
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd`, tâche `[?]`

## Exigences fonctionnelles

- **FR-1 - MUST:** `vanyline sync push --context <projet> --sandbox <nom>` copie le `.vanyline/` local vers le `.vanyline/` de
  la racine de la sandbox ; `sync pull` copie dans l'autre sens. Le dossier local est celui du dépôt courant, ou celui que
  désigne l'option de `cli#DEC-10`.
    - **Source:** `cli#FR-6`, `cli#DEC-10`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** La CLI obtient un ticket par `POST /ws/ticket` avec le jeton d'Owner, ouvre `/ws/fs` et lit les fichiers en
  mode brut. Elle calcule les empreintes SHA-256 de son côté, sur les octets reçus.
    - **Source:** `cli#BR-1`, `CON-1`, `CON-3` ; décision du développeur, 2026-11-07
    - **Verified by:** `AC-1`

- **FR-3 - MUST:** La CLI garde, par couple dépôt et sandbox, la liste des empreintes à la dernière synchronisation réussie.
  Un fichier a divergé quand ses deux copies diffèrent et que la copie de destination n'est plus celle de cette liste.
    - **Source:** `cli#DEC-11`
    - **Verified by:** `AC-2`

- **FR-4 - MUST:** En cas de divergence, la commande n'écrit rien, ni d'un côté ni de l'autre : elle montre le diff de chaque
  fichier divergent et sort en échec. `--force-local` (pour `push`) ou `--force-remote` (pour `pull`) tranche en faveur
  de la source.
    - **Source:** `cli#FR-6`
    - **Verified by:** `AC-2`

- **FR-5 - MUST:** Une suppression depuis la dernière synchronisation se propage comme une modification, après le même
  contrôle de divergence. `--no-delete` la désactive.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-3`

- **FR-6 - MUST:** Avant toute écriture, la CLI refuse l'ensemble de la synchronisation si un fichier à copier contient une
  valeur que le détecteur de secrets de lymaille reconnaît.
    - **Source:** `cli#FR-6`, `cli/hote#CON-3`
    - **Verified by:** `AC-4`

- **FR-7 - MUST:** Un chemin rendu par la sandbox qui sort de `.vanyline/` (composante `..`, chemin absolu, lien) est refusé
  avant toute écriture locale. Un fichier qui n'est pas du texte UTF-8, ou qui dépasse la taille maximale, est refusé et
  nommé.
    - **Source:** `agent-sessions#BR-9` (le contenu du workspace est de moindre confiance)
    - **Verified by:** `AC-5`

- **FR-8 - MUST:** Si le listage distant est tronqué (`CON-2`), la commande échoue : elle ne synchronise jamais un arbre
  partiel.
    - **Source:** `CON-2`
    - **Verified by:** `AC-6`

- **FR-9 - MUST:** Une écriture interrompue en cours de route laisse chaque fichier entier, ancien ou nouveau (écriture
  atomique des deux côtés), et la liste de `FR-3` ne mentionne que les fichiers réellement copiés.
    - **Source:** `SpecDD /sandbox/src/ws/fs.sdd` (écriture atomique) ; `FR-3`
    - **Verified by:** `AC-7`

## Sécurité

`pull` fait entrer en local des fichiers qu'un agent a pu écrire dans un contexte de moindre confiance : c'est pourquoi la
commande est explicite, qu'elle montre le diff en cas de divergence, et que ses chemins sont contraints (`FR-7`). Le contenu
d'un `.vanyline/` n'est pas effectif sans promotion humaine (`cli#DEC-4`, `hote#DEC-1`) : le copier ne l'active pas. Le
ticket est mis dans l'URL de connexion : il ne doit jamais être journalisé.

## Conception

Un arbre local et un arbre distant sont chacun listés puis lus en brut, avec leurs empreintes ; la comparaison à trois (copie
locale, copie distante, liste de la dernière synchronisation) produit pour chaque chemin une action : rien, copier, supprimer
ou diverger. Les divergences arrêtent tout avant la première écriture ; sinon les actions s'exécutent, puis la liste de
`FR-3` est mise à jour. La liste vit dans le répertoire d'état de `cli/hote` (`$HOME/.local/state/vanyline/`), un fichier
par couple dépôt et sandbox (**ASSUMPTION** de disposition). Taille maximale d'un fichier : 256 Kio, comme la limite des
skills de lymaille (**ASSUMPTION**, à confirmer).

## Décisions

- **DEC-1 - La copie passe par le WebSocket fichier de l'éditeur en mode brut**
    - **Context:** `read_file` du MCP est numéroté et borné pour un LLM (200 lignes, 16 Kio, saut de ligne final perdu) ;
      `execute_command` ou un nouvel endpoint ne sont pas nécessaires.
    - **Decision:** la CLI réutilise `/ws/fs` de la sandbox (`read` brut, `write`, `delete`, `list`), avec un ticket qu'elle
      obtient elle-même du jeton d'Owner. Aucun changement de la sandbox.
    - **Alternatives:** `read_file` MCP avec normalisation (écartée : copie non fidèle) ; `execute_command` avec `base64`
      (écartée : inutile puisque le canal brut existe) ; nouvel endpoint de la sandbox (écartée : chantier de plus).
    - **Source:** décision du développeur, 2026-11-07

## Réalisation SpecDD

À écrire sous `cli/src/` (**ASSUMPTION** de découpage) : le client `/ws/fs` et l'obtention du ticket, la comparaison à trois,
la liste des empreintes, les commandes `sync push` et `sync pull`.

## Acceptation et vérification

- **AC-1:** Après `sync push` sur une sandbox sans `.vanyline/`, chaque fichier du dossier local existe dans le worktree, octet
  pour octet identique (y compris le saut de ligne final).
- **AC-2:** Si un fichier a changé des deux côtés depuis la dernière synchronisation, `push` n'écrit rien, montre le diff et
  sort en échec ; avec `--force-local` la copie de la sandbox est remplacée.
- **AC-3:** Un fichier supprimé en local est supprimé dans la sandbox au `push` suivant ; avec `--no-delete`, il y reste.
- **AC-4:** Un fichier contenant une valeur ressemblant à un secret fait refuser tout le `push`, sans aucune écriture.
- **AC-5:** Un chemin `../x` rendu par `list` ou un fichier non UTF-8 fait refuser le `pull` avant toute écriture locale.
- **AC-6:** Un `.vanyline/` de plus de 200 entrées fait échouer `sync` au lieu de n'en copier qu'une partie.
- **AC-7:** Une coupure pendant `push` laisse chaque fichier entier, et le `push` suivant reprend sans signaler de divergence
  sur les fichiers déjà copiés.

## Risques

- Les 200 entrées maximum d'un listage limitent la taille de `.vanyline/` : un dossier plus gros exigerait de lister par
  sous-répertoire. Peu probable pour de la configuration, mais à surveiller (`FR-8` en protège).
- Le canal repose sur l'éditeur web de la sandbox : un changement de son protocole casse `sync`. Sa spec et celle de la CLI
  doivent se référencer mutuellement.

## Open Issues

- **OI-1:** **Revue avant `pull`.** Faut-il un `--dry-run` qui montre ce que ferait la commande sans écrire, et/ou une
  confirmation avant d'écrire en local ? Sans divergence, `pull` écrit sans montrer de diff. **TBD.**
- **OI-2:** **Taille maximale d'un fichier** (`256 Kio` proposé) : à valider. **TBD.**

## Références

`cli` (`cli#FR-6`, `DEC-4`, `DEC-7`, `DEC-10`, `DEC-11`), `cli/hote`, `sandbox` (`SpecDD /sandbox/src/ws/fs.sdd`,
`SpecDD /sandbox/src/ws/ticket.sdd`).
