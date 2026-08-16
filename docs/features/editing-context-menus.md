# Feature : menus contextuels & affordances d'édition

**Statut : implémentée, testée, revue (Phase 3 par Claude). Pas encore mergée ni
poussée.** Ce document garde trace du design initial et de ce qui a changé en
cours de route ; les parties pertinentes migreront vers `docs/architecture.md`
à la clôture, ce fichier sera alors supprimé.

## Ce que la feature fait

Rend le shell IDE utilisable au clic droit et via un menu Édition : recherche/
remplacement CodeMirror exposé dans un menu, menus contextuels (arbre, éditeur,
terminal, onglets) avec clipboard et utilitaires, icônes de fichier par extension
dans l'arbre, et CRUD complet sur l'arbre (nouveau fichier/dossier, renommer,
supprimer).

## Ce qu'elle ne fait pas

- Pas de drag & drop de fichiers dans l'arbre
- Pas de multi-sélection dans l'arbre (une action = un nœud)
- Pas de recherche globale multi-fichiers (grep sur le projet) — uniquement la
  recherche/remplacement dans le buffer actif (déjà fournie par CodeMirror)
- Pas de suppression récursive de dossier non vide (le backend refuse ce cas,
  cf. État de départ) — pas de confirmation en cascade à construire
- Pas de undo applicatif sur delete/rename — irréversible, comme le reste des
  opérations `/ws/fs` existantes (compensé côté UI par une confirmation avant
  Supprimer, cf. Corrections Phase 3)
- Terminal : pas de menu complet type shell, seulement Copier/Coller

## État de départ (vérifié dans le code, pas supposé)

- **Recherche/remplacement** : le `searchKeymap` de `basicSetup` était bien
  présent, mais **pas le `search({ top: true })` state field** — sans lui aucun
  panneau ne s'ouvrait (Mod-F ne faisait rien). Le vrai manque n'était donc pas
  que la découvrabilité du menu, mais une extension CodeMirror manquante. Ajoutée
  dans [Editor.vue](../../frontend/src/components/panels/Editor.vue).
- **Menus contextuels** : `reka-ui` fournit des primitives `ContextMenu*`
  distinctes de `Menubar*`, mais leur attribut de style partagé rendu dans le DOM
  est `[data-reka-menu-content]` — **pas** `[data-reka-context-menu-content]`
  comme supposé initialement (vérifié à l'exécution). Le forwarding de `class`
  est bien cassé comme pour `MenubarContent`, contournement identique appliqué
  dans [ContextMenu.vue](../../frontend/src/components/ContextMenu.vue).
- **Backend `/ws/fs`** ([sandbox/src/ws/fs.rs](../../sandbox/src/ws/fs.rs))
  exposait `read`, `write`, `edit`, `delete`, `list`. Complété par `mkdir`,
  `rename` et `root` (cf. Interfaces).
  - `write_file` crée déjà les dossiers parents manquants → "Nouveau fichier" ne
    demandait rien de neuf côté backend.
  - `delete_file` sur un dossier utilise `remove_dir` (non récursif) et renvoie
    `directory is not empty` si le dossier n'est pas vide — pas de suppression en
    cascade silencieuse.
- **Icônes** : premier set curé à la main dans
  [fileIcon.ts](../../frontend/src/components/panels/fileIcon.ts), même clés
  d'extension que `editorLanguage.ts`. Réutilise les icônes génériques
  `@element-plus/icons-vue` déjà présentes (Cpu, Files, Connection, DataLine...)
  plutôt qu'un pack d'icon-theme dédié — léger, pas de licence à gérer, mais sans
  correspondance visuelle réelle avec les langages (ex. Rust → icône
  "Connection"). Suffisant pour ce v1, discussion ouverte pour un set plus
  parlant plus tard.

## Interfaces clés et modules touchés

### Backend — `sandbox/src/ws/fs.rs` + `tools/src/filesystem.rs`
Trois nouveaux ops WS :
- `mkdir` : `tokio::fs::create_dir_all` (crée aussi les dossiers parents
  manquants, cohérent avec `write_file`). Idempotent sur un dossier déjà
  existant ; erreur `NotADirectory` si la cible est un fichier.
- `rename` : `{ "op": "rename", "path": "<source>", "to": "<dest>" }`. `to` est
  confiné par `confine_path` exactement comme `path` — même contrainte de
  sécurité que les ops existants.
- `root` (découverte en cours d'implémentation, absente du design initial) :
  `{ "op": "root" }`, sans `path`, renvoie la racine absolue confinée du
  sandbox. Ajoutée parce que le frontend n'a aucun autre moyen de connaître
  `sandbox_root` — nécessaire pour "Copier le chemin absolu".

### Frontend — composant `ContextMenu` générique
[ContextMenu.vue](../../frontend/src/components/ContextMenu.vue), `reka-ui`
(`ContextMenuRoot`/`Trigger`/`Content`/`Item`), paramétré par une liste
d'actions — même pattern que `menus`/`Item` dans `MenuBar.vue`.

| Cible | Actions livrées |
|---|---|
| Arbre ([Explorer.vue](../../frontend/src/components/panels/Explorer.vue)) | Nouveau fichier, Nouveau dossier, Copier le chemin relatif, Copier le chemin absolu, Renommer, Supprimer (avec confirmation) |
| Éditeur ([Editor.vue](../../frontend/src/components/panels/Editor.vue)) | Couper/Copier/Coller (sélection CodeMirror, Coller remplace la sélection active), Copier le chemin du fichier |
| Onglets (dockview, via `IdeShell.vue`) | **Menu natif dockview** (`getTabContextMenuItems`), pas le composant `ContextMenu` reka-ui prévu au design — Fermer/Fermer les autres/Fermer tout + Copier le chemin pour les onglets éditeur |
| Terminal ([Terminal.vue](../../frontend/src/components/panels/Terminal.vue)) | Copier (sélection xterm), Coller (écrit dans le PTY via le canal `/ws/terminal` déjà ouvert) |

### `MenuBar.vue`
Menu "Édition" : Rechercher, Remplacer — câblés via `registerIdeActions`/
`ideActions` (`useIdeSession`), clés `findInActiveFile`/`replaceInActiveFile`.
Les deux ouvrent le même panneau de recherche CodeMirror (`openSearchPanel`) —
il n'y a pas d'API publique pour ouvrir directement avec le champ remplacement
visible, donc "Remplacer" mène au même panneau que "Rechercher" plutôt qu'à une
vue dédiée.

### Icônes — `fileIcon.ts`
Mapping extension → composant icône, mêmes clés que `byExtension`
(`editorLanguage.ts`), consommé par `Explorer.vue` par nœud (+ icône dossier +
icône fichier générique en fallback).

## Décisions actées en cours d'implémentation

- **Rename d'un fichier ouvert** : ferme l'onglet concerné plutôt que de mettre
  à jour son path en place (pas de changement d'architecture d'`Editor.vue`,
  qui fige `filePath` à la création du panel).
- **Menu des onglets** : dockview fournit un menu contextuel natif
  (`getTabContextMenuItems`) plus simple que d'envelopper les tabs internes de
  dockview dans le composant `ContextMenu` reka-ui prévu au design (qui aurait
  demandé un rendu custom des tabs).
- **`mkdir`** : crée aussi les dossiers parents manquants (comme `write_file`),
  pas seulement le dossier final.

## Corrections apportées en revue Phase 3 (Claude)

- **Copie de chemin sur l'arbre manquante** : le design listait "Copier le
  chemin relatif/absolu" comme première action du menu contextuel de l'arbre ;
  l'implémentation initiale ne l'avait jamais câblée (reportée à "la tâche
  suivante" qui n'a jamais eu lieu), et l'op `root` ajoutée spécifiquement pour
  ça restait inutilisée côté frontend. Ajouté dans `Explorer.vue`
  (`copyRelativePath`/`copyAbsolutePath`, root mis en cache après premier
  appel).
- **Coller ne remplaçait pas la sélection** : `pasteClipboard` (menu contextuel
  éditeur) faisait une insertion pure à la tête de sélection sans supprimer le
  texte sélectionné — comportement différent du Ctrl+V natif du navigateur.
  Corrigé pour remplacer explicitement la plage sélectionnée.
- **Suppression sans confirmation** : `deleteNode` envoyait l'op `delete`
  immédiatement. Ajout d'un `window.confirm()` avant l'envoi, vu l'absence
  d'undo.
- **`cargo fmt` non lancé** avant les commits `mkdir`/`rename`/`root` — même
  angle mort que sur `ws10-language-support`, corrigé.
- Un commentaire expliquant `ws.binaryType = 'arraybuffer'`
  (`Terminal.vue`) avait été perdu pendant le refactor closures ; restauré.

## Point non vérifié — claim à corriger si retrouvé ailleurs

Le rapport d'implémentation mentionnait des tests de sélection CM6 "retirés"
faute de pouvoir dispatcher `{ selection: ... }` en jsdom ("crash fondamental"
dans `closeBrackets`/`bracketState.update`). **Non reproduit en Phase 3** :
`view.dispatch({ selection: EditorSelection.single(from, to) })` fonctionne
sans erreur avec le jeu d'extensions réel d'`Editor.vue` (`basicSetup` +
`oneDark` + `search({ top: true })`), y compris monté via `@vue/test-utils`
(seul du bruit `stderr` inoffensif de mesure de layout — même famille que les
`"Not implemented: HTMLCanvasElement's getContext()"` déjà vus ailleurs dans la
suite). Le test de remplacement de sélection au collage (cf. corrections
ci-dessus) le confirme : il dispatche une sélection puis vérifie le résultat,
sans contournement. Si ce claim ressurgit ailleurs, le retester avant de le
prendre pour acquis.

## Risques restants / discussion ouverte

- **Collapse de l'arbre au refresh** : `Explorer.vue` force un remount complet
  d'`el-tree` (`:key="refreshKey"`) après chaque création/renommage/suppression,
  ce qui replie tous les dossiers déplié. Acceptable pour ce v1, à revoir si ça
  gêne à l'usage.
- **Icônes génériques** : cf. État de départ — set fonctionnel mais sans
  correspondance visuelle par langage.
