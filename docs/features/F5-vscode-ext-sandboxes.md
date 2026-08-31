# Feature — F5-vscode-ext-sandboxes

Dernière des cinq features « extension VS Code `vanyline` ». Séquence et état :
`.claude/memory/vscode-ext-sequence.md`. Dépend de **F3** (host + `RpcConnection`).
Indépendante de F2/F4.

## Ce que la feature fait

Expose dans l'extension la gestion des ressources Kubernetes de `vanyline` (Owners,
Projects, Sandboxes) déjà disponibles en RPC (`owners/*`, `projects/*`, `sandboxes/*`,
`sandboxes/stop|start` — cf. `cli/src/rpc/handlers.rs`), via une `TreeView` native VS
Code avec actions contextuelles.

## Ce qu'elle ne fait pas

- **Pas d'éditeur / terminal / FS distant** dans l'extension — VS Code a son éditeur
  natif, et on ne réimplémente pas le shell dockview du frontend web (décision session).
- **Pas de connexion MCP** de l'extension vers la sandbox (kydah-code → sandbox reste
  non câblé, hors scope de toute la famille).
- Pas de provisioning d'Owner automatique — l'extension lit / crée via RPC, le lazy
  provisioning reste côté `app` (`POST /api/projects`) et CLI.
- **Pas de watch temps réel des phases** — `app` a `/api/ws/sandbox-state`, la CLI RPC
  n'a pas d'équivalent. v1 = refresh manuel + refresh après action. Un watch RPC est une
  question ouverte (éventuelle F6).
- Pas de réutilisation de `@vanyline/ui` (les dashboards web sont couplés dockview /
  vue-router).

## Architecture

```
ext/
└── src/
    ├── resources.ts        TreeDataProvider : Owners > Projects > Sandboxes
    └── panels/resources.ts enregistrement TreeView + commandes + menus contextuels
```

- `TreeView` `vanyline.resources` dans la sidebar `vanyline` (sous le chat).
- `TreeDataProvider` : racine → `owners/list` ; enfant d'un Owner → `projects/list`
  filtré ; enfant d'un Project → `sandboxes/list` filtré. Chaque nœud porte la phase
  (`sandboxes/get` → `status.phase`) en description + icône par phase.
- Actions (InputBox / QuickPick natifs pour les specs) :
  - `vanyline.project.create` / `.delete` → `projects/create` / `projects/delete`
  - `vanyline.sandbox.create` / `.delete` → `sandboxes/create` / `sandboxes/delete`
  - `vanyline.sandbox.stop` / `.start` → `sandboxes/stop` / `sandboxes/start`
  - `vanyline.resources.refresh`
- Namespace : la CLI RPC le résout **une fois par session** (`defaults.namespace` du
  `config.yaml` fusionné, sinon contexte kubeconfig courant) — l'extension **affiche
  lequel** dans le titre de la vue, pas de sélecteur par appel (limite RPC documentée
  dans `docs/architecture.md`).

### `contributes` (extrait)

- `views` : `vanyline.resources` (tree) dans `viewsContainers` `vanyline`
- `commands` : `vanyline.project.{create,delete}`, `vanyline.sandbox.{create,delete,stop,start}`, `vanyline.resources.refresh`
- `menus/view/item/context` : actions par type de nœud (`when: viewItem == owner|project|sandbox`)
- `menus/view/title` : refresh

## Modules touchés

| Module | Changement |
|---|---|
| `ext/src/resources.ts` | **nouveau** — `TreeDataProvider` |
| `ext/src/panels/resources.ts` | **nouveau** — enregistrement + commandes |
| `ext/src/extension.ts` | `registerResources(context, rpc)` dans `activate` |
| `ext/package.json` | `views` + `commands` + `menus` ci-dessus |
| `cli/src/rpc/handlers.rs` | **seulement si** `sandboxes/stop\|start` s'avèrent non fonctionnels (voir risques) |

## Sécurité (argv / URL / chemin)

- `owners/projects/sandboxes create` prennent des specs (`OwnerSpec` / `ProjectSpec` /
  `SandboxSpec`) fournies par l'utilisateur → deviennent des **noms de ressources K8s**
  et des champs de CR. La validation RFC1123 des noms est **côté CLI / controller**
  (déjà le cas, cf. `sanitize_owner_name` de `app`). L'extension valide en surface
  (feedback formulaire) ; la source de vérité reste le backend.
- `ProjectSpec` porte une **URL de dépôt git** → clonée par un Job `vanyline-maint`
  (arguments en **argv**, jamais un script shell assemblé — cf. `AGENTS.md` section
  controller). Risque porté par le controller, pas l'extension.
- Aucune interpolation shell côté extension. La CLI RPC utilise le **kubeconfig local**
  de l'utilisateur (pas d'OIDC) — l'extension hérite de ce contexte, cohérent avec
  « harness local ».

## Tests

- `ext` (Vitest, RPC mocké) : `TreeDataProvider` — arbre à 3 niveaux, filtrage
  owner→project→sandbox correct, phase affichée, refresh après action.
- Commandes create/delete/stop/start → bon appel RPC avec les bons params, arbre
  rafraîchi.
- Test e2e manuel (`docs/ext-install.md`) : sur un cluster réel accessible via
  kubeconfig, créer un projet + une sandbox depuis l'extension, la voir passer
  `Pending → Running`, stop → `Suspended`, start, delete.

## Risques et questions ouvertes

- **`sandboxes/stop|start` sont-ils fonctionnels ?** Les méthodes RPC existent
  (`handlers.rs:239-240`) mais `MEMORY.md` liste `vanyline sandbox stop|start` (CLI)
  comme « champ `suspended` posé côté CRD, jamais câblé ». **Tâche 1 : vérifier que ces
  méthodes RPC patchent réellement `spec.suspended`** — sinon, les câbler fait partie de
  F5 (petit ajout côté `cli` + `VnlK8sClient`).
- **Pas de watch** : UX de fraîcheur = bouton refresh + refresh auto après action.
  Acceptable v1 ? Si non → F6 (watch RPC : nouvelle méthode `sandboxes/watch` +
  notifications, calquée sur `/api/ws/sandbox-state`).
- **`TreeView` native vs dashboards web** : divergence visuelle assumée (idiome VS Code,
  léger, actions contextuelles gratuites).
- **Namespace figé par session** : si l'utilisateur change de contexte kubeconfig, il
  doit recharger la fenêtre. Documenté, pas mitigé en v1.
- **Formulaires de spec via InputBox/QuickPick natifs** : les specs `SandboxSpec`
  (toolchains, branche, project ref) sont riches — une suite d'InputBox peut être
  pénible. Alternative : un mini-webview form. Retenu InputBox pour v1, à réévaluer si
  l'ergonomie est mauvaise.

## Découpage en tâches candidates

1. Vérifier `sandboxes/stop|start` RPC (patch réel de `spec.suspended`) ; les câbler si besoin.
2. `TreeDataProvider` owners/projects/sandboxes (lecture) + affichage du namespace + icônes de phase.
3. Commandes create/delete (InputBox/QuickPick) + refresh post-action.
4. Commandes stop/start + menus contextuels + test e2e manuel.
