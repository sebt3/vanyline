# Menus contextuels & affordances d'édition (2026-08-17)

## Ce qui a été livré

Design initial (`docs/features/editing-context-menus.md`, supprimé à la clôture —
contenu migré dans `docs/architecture.md` section "Frontend — shell IDE Vue" et
section "Serveur MCP") : menu Édition (Rechercher/Remplacer), menus contextuels
(arbre, éditeur, terminal, onglets), icônes de fichier par extension, CRUD complet
sur l'arbre (nouveau fichier/dossier, renommer, supprimer, copier chemin
relatif/absolu). Trois nouveaux ops WS côté sandbox (`mkdir`, `rename`, `root`).

## Process — délégation à Cadence, même motif que WS-10

Comme `ws10-language-support`, cette feature a été entièrement déléguée à l'agent
opencode `cadence` après le design Claude (docs/features/*.md), sans repasser par
le format `.tasks/<feature>/task-XX.md`. Deuxième point de données sur le même
motif : code fonctionnel et testé (tests verts, build propre), mais **plusieurs
lacunes trouvées uniquement en review Claude post-implémentation** — cf. ci-dessous.
Confirme la conclusion de WS-10 : la review Phase 3 reste nécessaire quel que soit
l'agent d'exécution, elle n'est pas optionnelle même quand la suite de tests passe.

## Pièges trouvés en review (corrigés avant clôture)

- **Action du design jamais livrée malgré un backend construit pour elle** : "copier
  le chemin relatif/absolu" était la toute première action demandée pour le menu de
  l'arbre (design doc **et** note initiale du développeur principal) — l'implémentation
  a bien ajouté l'op WS `root` spécifiquement pour permettre le chemin absolu, mais
  n'a jamais câblé les deux entrées de menu correspondantes (reportées à "la tâche
  suivante", jamais faite). Résultat : un op backend testé et fonctionnel, mort côté
  frontend. **Leçon générale** : une review de clôture doit vérifier qu'un morceau
  d'infra ajouté "pour X" a bien un appelant qui fait X — pas juste que l'infra
  compile et passe ses propres tests.
- **Coller (menu contextuel éditeur) ne remplaçait pas la sélection** :
  `changes: {from: selection.head, insert}` est une insertion pure, jamais testée
  avec une sélection active (seul le cas "sans sélection" avait un test). Diverge du
  Ctrl+V natif du navigateur. Trouvé en lisant le code, pas par un test qui aurait dû
  le couvrir.
- **Suppression sans confirmation** sur l'arbre — combiné à l'absence d'undo côté
  `/ws/fs` (`remove_dir`/`remove_file` directs), un clic dans le menu contextuel
  détruisait définitivement. Le design doc actait explicitement "pas d'undo" sans
  qu'une confirmation UI ait été ajoutée en compensation.
- **`cargo fmt --check`** non lancé avant les commits `mkdir`/`rename`/`root` — même
  angle mort que WS-10, troisième occurrence toutes features Cadence confondues.
  Motif suffisamment répété pour mériter un hook pre-commit dédié si ça se reproduit
  une quatrième fois.
- Un commentaire WHY (`ws.binaryType = 'arraybuffer'`, `Terminal.vue`) perdu comme
  collatéral d'un refactor (déplacement de `TerminalActions.ts` vers des closures
  in-place pour corriger un vrai bug d'état partagé multi-terminal) — restauré.

## Claim de Cadence corrigé (pas juste un piège de code, un piège de diagnostic)

Le rapport de Cadence affirmait que dispatcher une sélection CodeMirror
(`view.dispatch({selection: ...})`) "crashe fondamentalement" en jsdom
(`closeBrackets`/`bracketState.update`), et avait retiré les tests correspondants
sur cette base. **Non reproduit en Phase 3** : testé directement (EditorView nu, puis
avec le jeu d'extensions réel d'`Editor.vue`, puis monté via `@vue/test-utils` comme
en production) — ça fonctionne à chaque fois, seul du bruit `stderr` inoffensif
(mesure de layout non implémentée par jsdom, même famille que les warnings
`HTMLCanvasElement.getContext()` déjà vus ailleurs dans la suite). Le fix du bug
Coller a servi de test de régression : il dispatche une vraie sélection puis vérifie
le résultat, sans contournement. **Leçon** : un claim d'échec d'infra de test
("impossible de tester X ici") mérite une vérification indépendante en review avant
de le prendre pour acquis et de le laisser figer dans la doc — surtout s'il justifie
l'absence d'un test sur un chemin par ailleurs bugué.

## Décisions actées cette session (developer + Claude, avant implémentation)

- CRUD complet sur l'arbre dès ce lot (pas de scope réduit à "copier chemin
  seulement") — implique `mkdir`/`rename` backend, séquencés avant le CRUD frontend
  qui en dépend.
- Terminal : menu contextuel applicatif dédié (Copier/Coller) plutôt que le menu
  natif du navigateur, qui n'a aucun sens sur un PTY.
- Renommer un fichier ouvert ferme son onglet plutôt que de mettre à jour son `path`
  en place — évite de casser l'invariant "`path` fixe par instance" d'`Editor.vue`
  pour un seul cas d'usage.
