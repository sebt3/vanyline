# F5 — `F5-vscode-ext-sandboxes` (close 2026-09-04)

Dernière des 5 features « extension VS Code `vanyline` ». **La séquence VS Code est
terminée.** Séquence et procédure de reprise : `.claude/memory/vscode-ext-sequence.md`.

## Ce qui est livré

`TreeView` native `vanyline.resources` (sidebar `vanyline`, sous Chat) : Owners ›
Projects › Sandboxes, chaque nœud portant sa phase (icône) en description. 6 commandes
avec menus contextuels par type de nœud :

- `vanyline.project.{create,delete}` → RPC `projects/{create,delete}`
- `vanyline.sandbox.{create,delete}` → RPC `sandboxes/{create,delete}`
- `vanyline.sandbox.{stop,start}` → RPC `sandboxes/{stop,start}` (= patch `spec.suspended`)
- `vanyline.resources.refresh` (bouton titre)

Specs saisies en `InputBox`/`QuickPick` natifs. `sandboxes/create` n'envoie que
`{name, project, branch}` — les toolchains sont auto-dérivées des langages détectés
(cf. `ws10-language-support`). Suppression = **modale de confirmation** ; stop/start =
pas de modale (transition réversible). Procédure e2e manuelle : `docs/ext-install.md` §5.

## Architecture (2 fichiers, motif « pur + colle » systématisé)

| Fichier | Rôle |
|---|---|
| `ext/src/resources.ts` | **Logique pure, testée sans `vscode`** : `createResourcesModel` (aucun cache, filtrage owner→project→sandbox, nœuds `error`/`info` au lieu de rejets), `run*` des 6 commandes (`PromptApi` injecté), `validate*` de surface. |
| `ext/src/panels/resources.ts` | Colle vscode : `createTreeView`, mapping `ResourceNode`→`TreeItem`, `EventEmitter`/`onDidChangeTreeData`, `registerRun`, titre/description. **Sans test** (comme tous les `panels/*`). |

**100 % host** : la `TreeView` parle au `RpcConnection` du handle superviseur **en
direct** (`resources.attachServer(h.conn)`) — pas de webview, donc **rien à ajouter à
`RELAY_WHITELIST`** (contrairement à l'anticipation du rappel transverse de la séquence).

Namespace affiché = `metadata.namespace` du premier objet listé (la CLI RPC le résout
une fois par session). Figé : changer de contexte kubeconfig impose `Reload Window`.

## Risque design n° 1 — levé sans code

« `sandboxes/stop|start` RPC patchent-ils réellement `spec.suspended` ? » (MEMORY listait
`vanyline sandbox stop|start` **CLI** comme jamais câblé). Vérifié en lecture :
`handlers.rs` (`handle_sandboxes_{stop,start}`) → `VnlK8sClient::set_sandbox_suspended`
(`lib/src/k8s.rs:90`) fait un merge-patch `{ "spec": { "suspended": bool } }` et retourne
le CR patché ; le controller l'honore (`controller/src/sandbox.rs:1023` →
`phase = "Suspended"`). **Le trou CLI d'origine ne concernait pas le chemin RPC.** Aucune
modif Rust dans F5.

## Review Phase 3 — 1 bug bloquant, 1 mineur (corrigés par Claude)

**Bloquant — `treeView.refresh()` n'existe pas.** `panels/resources.ts` rafraîchissait
l'arbre via `(treeView as unknown as { refresh(): Promise<void> }).refresh()`, avec un
commentaire affirmant que c'est « une API publique depuis 1.67 absente des types ». **Faux
— il n'existe aucun `TreeView.refresh()`** ; le seul mécanisme est un `EventEmitter`
exposé en `TreeDataProvider.onDidChangeTreeData`. Conséquences : `refreshView()` lançait un
`TypeError` synchrone → bouton refresh en échec, refresh post-action silencieusement
inopérant (arbre périmé jusqu'au reload), et `attachServer` (`void refreshAndView()`)
propageait l'exception à chaque `ready` du superviseur. **Les 168 tests passaient** car ils
ne couvrent que `resources.ts` pur — `panels/resources.ts` n'a pas de test et le faux Rpc
des tests bypasse `validateInput`/l'API vscode.

→ **L'API hallucinée venait du fichier de tâche `task-01` lui-même** (rédigé par Claude
dans une session antérieure : « Commande `vanyline.resources.refresh` :
`await treeView.refresh(); updateTitle();` »). Cadence a implémenté fidèlement et a
*ajouté* la justification inventée du cast. Leçon : une API nommée dans un fichier de
tâche doit être vérifiée contre `@types/*` avant d'être écrite — Qwen/Cadence ne
recoupe pas, il exécute.

Fix : `EventEmitter<ResourceNode | undefined>` + `onDidChangeTreeData` ; `refreshView()`
= `changeEmitter.fire(undefined)` ; le namespace n'étant résolu qu'après la 1ʳᵉ liste RPC,
`updateTitle()` est aussi rappelé par le provider à la fin de chaque `getChildren`.

**Mineur — « branche par défaut (optionnel) » non omissible.** L'`InputBox` câblait
`validate: validateGitRef`, qui rejette `''` → impossible de vider le champ pré-rempli
`main` pour omettre `defaultBranch` ; la branche `if (defaultBranch !== '')` de
`runProjectCreate` était donc morte via l'UI réelle (seul le test scripté, qui bypasse
`validateInput`, l'atteignait). Fix : `validate: (v) => v === '' ? undefined :
validateGitRef(v)` pour ce champ facultatif uniquement.

## Bilan de délégation

3 tâches Cadence, chacune avec un commit d'implémentation + un commit `fix:` « écarts au
contrat » (auto-review Cadence). **Pas d'escalade.** Mais le seul filet réel a été la
Phase 3 : le bug bloquant a traversé l'implémentation ET l'auto-review parce que sa
source était le fichier de tâche. 3ᵉ feature `ext/` livrée par Cadence (après F3, F4) —
les deux précédentes étaient « 0 bug bloquant » ; celle-ci en avait un, hérité de la
spec, pas du modèle.

`cargo fmt` : sans objet (aucun code Rust touché). Validation lancée : `npm run
check/test/build --workspace=ext` (+ `check` des packages) — tout vert.
