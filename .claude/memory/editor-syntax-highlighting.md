# Feature — Coloration syntaxique éditeur (Vue / Dockerfile / rhai / handlebars)

**Close le 2026-09-06. Branche `feat/editor-syntax-highlighting` mergée dans `main`
et poussée, branche supprimée.**

## Périmètre

100 % `frontend/`. Coloration seule, **aucun LSP, aucun backend, aucune image,
aucune détection**. Le branchement LSP de `.vue` et `Dockerfile` est le sujet des
features `vue-lsp` / `docker-lsp` (séparées).

Langages ajoutés à `editorLanguage.ts::byExtension` :
- `.vue` → `@codemirror/lang-vue` v0.1.3 sur base `@codemirror/lang-html` (nouvelle
  dép directe — `vue({ base: html() })`, `base` **doit** être le retour de `html()`).
  `@codemirror/lang-html` déclaré en dép directe (importé explicitement, pas
  seulement transitif).
- `.rhai` → mode `StreamLanguage` maison `langRhai.ts` (mots-clés fermés, `//` +
  `/* */`, chaînes `"` et backtick multi-ligne, nombres).
- `.hbs` / `.handlebars` → mode `StreamLanguage` maison `langHandlebars.ts` :
  HTML-lite + surcouche moustaches, **pas d'overlay sur un mode LR** (un mode stream
  ne se superpose pas proprement à un mode LR). Conséquences verrouillées en tests :
  `{{else}}` → `variableName` (pas `keyword`, hors liste `# ^ /`), un mot hors balise
  n'est jamais `attributeName`, `{{!--` se ferme au premier `}}` (pas de recherche
  `--}}`).
- Dockerfile/Containerfile : **par nom de base, pas par extension** — helper partagé
  `dockerfileName.ts` (`Dockerfile`, `Dockerfile.*`, `*.dockerfile`, `Containerfile*`,
  insensible casse), testé en amont du découpage d'extension dans
  `languageExtensionForPath` ET `fileIcon.ts::iconForPath`. →
  `@codemirror/legacy-modes/mode/dockerfile` (déjà installé).

Icônes Element Plus (`fileIcon.ts`) : vue → MagicStick, rhai → Reading,
hbs/handlebars → Film, Dockerfile → Goods. Choix curé main, ajustable sans risque.

## Delivery

Livré par **Cadence** (`cadence` + `implement` sur `qwen3.8-flash-next`), mode
feature `.tasks/` (5 tâches). Design doc Phase 1 avait des penchants explicites sur
les 3 questions ouvertes (modes maison, `Containerfile` inclus, overlay stream
simple) — Cadence les a tranchés dans le sens du penchant et signalé le choix pour
validation Phase 3. Tous validés en review.

**Review Phase 3 : 0 bug bloquant.** J'ai vérifié la robustesse des deux modes
maison contre le garde-fou `readToken` (« failed to advance ») — tous les chemins
consomment ≥ 1 caractère. Noms de tokens tous résolus par `@lezer/highlight` (pas de
`Unknown highlighting tag`). Un seul finding mineur : docstring d'en-tête de
`byExtension` pas à jour (ne listait pas les nouveaux langages) — **motif doc-drift
récurrent, mais ici attendu : `.claude/config.md` demande explicitement de garder la
migration doc pour la Phase 3**, ce n'est pas un oubli d'implémentation. Corrigé à la
clôture avec la migration `docs/architecture.md` (nouvelle sous-section « Coloration
syntaxique » près de `lspToolchainForPath`).

Contraste : 3ᵉ feature livrée par le binôme `qwen3.8-flash-next` (après F4/F5 côté
`ext/`), confirme que ce binôme est le plus fiable à date — cf.
[[sandbox-state-ws]] pour l'historique des réglages Cadence.

## Suite

`feat/vue-lsp` (design doc Phase 1 + spike Volar v3 déjà écrits, rebasé sur ce
`main` le 2026-09-06) : LSP `.vue` réel via multiplexeur LSP côté sandbox (Volar v3
mode hybride, `vue-language-server` + `typescript-language-server(+@vue/typescript-plugin)`).
Décision développeur (2026-09-05) : v3 multiplexeur, pas v1.8 takeover.
