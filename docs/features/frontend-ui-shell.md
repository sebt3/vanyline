# Frontend — contrat visuel de l'IDE web

## Ce que la feature fait

Remplace le frontend Svelte existant par un nouveau frontend Vue 3 qui établit le
contrat visuel définitif de l'IDE web vanyline — coquille dockable, barre de menu,
panneaux (Explorer, Éditeur, Terminal, Workflow, Assistant) et vue Configuration —
issu d'un POC construit et validé itérativement avec le développeur principal.

## Ce qu'elle ne fait pas

Ce document couvre uniquement le contrat visuel. L'arrimage fonctionnel fera l'objet
d'une session de conception séparée, et notamment :

- Pas de connexion réelle à une sandbox (WebSocket éditeur/terminal/filesystem) —
  Explorer/Éditeur/Terminal affichent du contenu statique de démonstration
- Pas d'auth OIDC réelle — seule la décision est actée (pas d'écran de login dédié,
  redirection vers le provider en amont du shell si non authentifié)
- Pas de vraie intégration LLM — le panneau Assistant est un mock statique/local,
  aucun appel réseau
- Pas de provisioning réel de projet/sandbox depuis la vue Configuration — les champs
  n'écrivent rien, aucun appel à `app`
- Pas de moteur de workflow/DAG — le panneau Workflow est un mock visuel
- Pas d'optimisation du bundle (code-splitting, lazy-loading des panneaux)
- Pas de Storybook (existait côté Svelte, pas reconstruit ici)

## Interfaces clés et modules touchés

**Remplacement intégral** de `frontend/` (Svelte 5 + CodeMirror 6 + svelte-spa-router +
Tailwind 4, cf. `AGENTS.md`) par une nouvelle stack Vue 3 :

| Rôle | Choix | Notes |
|---|---|---|
| Coquille dockable | [dockview-vue](https://dockview.dev) | Thème `dockview-theme-abyss`, réutilisé comme design system transversal (`--dv-color-abyss-*`) |
| Éditeur | [CodeMirror 6](https://codemirror.net) | Déjà acté avant ce POC |
| Terminal | [xterm.js](https://xtermjs.org) (`@xterm/xterm` + `@xterm/addon-fit`) | Standard de facto (VS Code, code-server, Codespaces) |
| Arbre de fichiers | [Element Plus](https://element-plus.org) (`el-tree`) | Restylé via ses custom properties CSS, pas son thème par défaut |
| Menu / vue Configuration | [Reka UI](https://reka-ui.com) (`Menubar`, `Tabs`) | Headless, portage Vue de Radix — même famille que Bits UI/Melt UI évoqué côté Svelte initialement |
| Chat | [vue-advanced-chat](https://github.com/advanced-chat/vue-advanced-chat) | Voir risques — theming non résolu |

Composants livrés : `App.vue` (shell + bascule shell/Configuration), `MenuBar.vue`,
`StatusBar.vue`, `SettingsView.vue`, `panels/{Explorer,Editor,Terminal,Workflow,Chat}.vue`.

**Écarté en cours de route** : PrimeVue (licence payante découverte après intégration,
`node_modules/primevue/LICENSE.md` — clé requise, dégradation visuelle volontaire sans
clé) et `@nlux/vue` (n'existe pas : NLUX n'a qu'un package React).

## Risques identifiés et questions ouvertes

- **Theming `vue-advanced-chat` non résolu.** Cinq tentatives (prop `styles` en JSON,
  custom properties CSS, `!important`, écriture JS post-montage) sans effet visuel sur
  au moins une partie des couleurs (ex. `--chat-footer-bg-color-reply` reste à sa valeur
  par défaut). Cause racine non identifiée faute d'inspection réelle (DOM/computed
  styles) — reporté à une session dédiée avec le MCP playwright du développeur
  configuré. Repli déjà identifié si le theming reste bloqué : bulles de chat maison
  (déjà écrites et fonctionnelles dans une itération précédente du POC) + `@ai-sdk/vue`
  (`useChat`) pour la seule partie qui justifie un composant du marché — le streaming
  token par token.
- **`reka-ui` : forwarding `class`/`attrs` peu fiable sur certains composants.**
  Constaté sur `Menubar` (chaîne `MenubarContent` → `MenuContent` → … → `PopperContent` :
  au moins un maillon ne forwarde que ses props déclarées, pas `$attrs`). Contournement
  appliqué : styler via les attributs stables de la lib (`data-reka-*`, `role`, `data-state`)
  plutôt que via `class`, en règle globale non scopée avec commentaire explicatif. À
  appliquer par défaut pour tout nouveau composant reka-ui, plutôt qu'à re-découvrir au
  cas par cas.
- **Bundle ~560 Ko gzippé** (CodeMirror + xterm + Element Plus + vue-advanced-chat).
  Aucune tentative de code-splitting ; à surveiller si le nombre de panneaux augmente.
- **`SettingsView` est un premier brouillon**, qualifié comme tel par le développeur
  principal — 4 catégories (Projet, Sandbox, Agent & modèle, Compte) et leurs champs
  sont illustratifs, pas figés. À retravailler quand la session d'arrimage fonctionnel
  définira ce qui doit réellement y être configuré vs. autoconfiguré (cf. philosophie
  Kydah : « tout ce qui peut être autoconfiguré doit l'être »).
- **CI** (`test.yml`, job `frontend`) attend `build`/`check`/`test` — alignés sur le
  nouveau `package.json` (`vue-tsc --noEmit` pour `check`, `vitest run` pour `test`) et
  vérifiés en local. Un seul smoke test existe (`StatusBar.spec.ts`) : suffisant pour ne
  pas casser la CI, pas une couverture de la feature — le TDD réel démarre avec les
  premières tâches d'implémentation fonctionnelle.
- **`AGENTS.md`** documente encore l'ancienne stack Svelte dans son tableau technique.
  Volontairement non modifié ici : la mise à jour de `AGENTS.md`/`docs/architecture.md`
  est un geste de clôture de feature (cf. `.claude/config.md`, Phase 3), à faire quand
  la feature complète (visuel + arrimage) sera prête, pas à ce stade de brouillon.
- **Pod de démo `vanyline-ui-poc`** (namespace `media-station`, cluster du développeur
  principal) servait ce POC via un montage `subPath` pointant vers
  `projets/vanyline/frontend-poc-dockview/dist` — chemin qui n'existe plus après le
  renommage en `frontend/`. Le manifeste (nginx + Service + Ingress, hors contrôleur —
  outillage de review jetable) n'a pas été conservé dans `frontend/`. Le pod est
  probablement mort ou sert du contenu figé ; à détruire ou reconstruire séparément si
  encore utile.
