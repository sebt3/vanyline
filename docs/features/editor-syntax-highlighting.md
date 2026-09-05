# Feature — Coloration syntaxique : Vue, Dockerfile, rhai, handlebars

## Ce que la feature fait (une phrase)

Ajoute la coloration syntaxique CodeMirror pour `.vue`, `Dockerfile`, `.rhai` et
`.hbs`/`.handlebars` dans l'éditeur web — coloration seule, aucun LSP.

## Ce qu'elle ne fait pas (périmètre explicite)

- **Aucun LSP, aucun backend, aucune image, aucune détection.** 100 % `frontend/`.
- Pas de repli intelligent (indentation contextuelle avancée, folding sur mesure) —
  ce que le mode CodeMirror fournit par défaut, rien de plus.
- Ne touche pas `lspToolchainForPath` ni le mapping sandbox `toolchain_for_path` —
  ces extensions restent en mode dégradé LSP (coloration + rien d'autre). Le
  branchement LSP de `.vue` et `Dockerfile` est le sujet des features `vue-lsp` et
  `docker-lsp`.

## Interfaces clés et modules touchés

### `frontend/src/components/panels/editorLanguage.ts`
- `byExtension` : entrées `vue`, `rhai`, `hbs`, `handlebars`.
  - `vue` → `@codemirror/lang-vue` (nouvelle dépendance ; tire `@codemirror/lang-html`
    comme base — `vue({ base: html() })`).
  - `rhai` → `StreamLanguage` maison (mots-clés `let fn if else while loop for in
    return true false`, commentaires `//` et `/* */`, chaînes `"` et backtick,
    nombres). Modèle : le bloc `toml` déjà présent (`StreamLanguage.define`).
  - `hbs`/`handlebars` → `StreamLanguage` maison : HTML + surcouche `{{ ... }}`,
    `{{#... }}`, `{{/... }}`, `{{! ... }}`. Simple overlay, pas d'AST.
- **`Dockerfile` = nom de fichier, pas extension.** Aujourd'hui `byExtension` et
  `languageExtensionForPath` passent tous par `path.split('.').pop()`. Ajouter en
  amont un test de nom de base : `Dockerfile`, `Dockerfile.*` (ex. `Dockerfile.dev`),
  `*.dockerfile`, `Containerfile` → `@codemirror/legacy-modes/mode/dockerfile`
  (**déjà installé**, `StreamLanguage.define(dockerFile)`).
- `languageExtensionForPath` : intègre le test de nom de base ci-dessus avant le
  découpage par extension.

### `frontend/src/components/panels/fileIcon.ts`
- Icônes pour `.vue`, `Dockerfile`/`Containerfile`, `.rhai`, `.hbs`. Même test de nom
  de base pour Dockerfile.

### `frontend/package.json`
- `@codemirror/lang-vue` (+ `@codemirror/lang-html` si pas déjà tiré transitivement —
  à vérifier au moment de la tâche).

### Modes rhai / handlebars
- Nouveaux fichiers sous `frontend/src/components/panels/` (ex. `langRhai.ts`,
  `langHandlebars.ts`), ou inline dans `editorLanguage.ts` s'ils restent courts.
  Conventions : mêmes que `fileIcon.ts` / `diffPatch.ts` (fonctions pures, testées).

## Contrainte de validation / échappement

Aucune. Pas d'entrée utilisateur passée dans un shell, une URL ou un chemin — la
feature ne fait que sélectionner une extension CodeMirror à partir du nom de fichier.

## Risques identifiés

1. **`@codemirror/lang-vue` v0.1.3, publié il y a ~3 ans.** Auteur = équipe CodeMirror,
   cible = template Vue (HTML + moustaches), périmètre stable → risque faible. Vérifier
   la compat avec les versions `@codemirror/*` du projet (state 6.7, view 6.x) au
   moment de la tâche ; repli = mode HTML simple si incompat.
2. **Modes rhai/handlebars maison** — coloration « suffisante », pas parfaite. Acté :
   on ne vise pas un vrai parseur (pas de Lezer grammar).
3. **Dockerfile par nom de fichier** — 3 endroits à toucher de façon cohérente
   (`byExtension`/`languageExtensionForPath`, `fileIcon.ts`). Un helper
   `dockerfileName(path): boolean` partagé évite la divergence.

## Questions ouvertes

- Modes rhai/handlebars écrits main (penchant) vs dépendance tierce
  (`@codemirror/legacy-modes` n'a ni l'un ni l'autre ; qualité des `codemirror-lang-*`
  tiers variable, souvent non maintenus).
- `Containerfile` inclus dans le jeu de noms Dockerfile (penchant : oui).
- Mode `.hbs` : overlay sur `htmlmixed` (legacy) ou sur `@codemirror/lang-html` ?
  (penchant : overlay `StreamLanguage` simple, pas de dépendance HTML si `lang-vue`
  ne la tire pas déjà).

## Migration `docs/architecture.md` (Phase 3)

- Tableau/section « Éditeur » : liste des langages colorés + note « coloration seule »
  pour rhai/handlebars.
- § stack frontend : `@codemirror/lang-vue`.
