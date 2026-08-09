# ws11-sandbox-git — endpoints `/git/status` et `/git/unpushed` (terminé)

`GET /git/status` (parse pur de `git status --porcelain=v2 --branch`) et `GET
/git/unpushed` (compare `HEAD` à `origin/<branche>` ou, sans upstream, à
`origin/<default>`) — `sandbox/src/git.rs`, mêmes middlewares que `/mcp`.
Détails architecturaux, schémas JSON, codes d'erreur `VNL-SBX-004..006` :
`docs/architecture.md` section "Endpoints git".

**Deux prérequis architecturaux découverts avant la première tâche** (pas
dans le design initial — trouvés en vérifiant l'hypothèse "les commandes
git tournent dans `VNL_SANDBOX_ROOT`" contre le mount réel du pod, avant
d'écrire la moindre ligne de code) :

1. **`repo.git` invisible dans le pod sandbox** : `git worktree add` écrit
   un pointeur `.git` **absolu** (`gitdir: /workspace/repo.git/worktrees/<sandbox>`,
   vérifié avec un vrai `git worktree add` local) ; le pod sandbox ne
   montait que le subPath `worktrees/<sandbox>` → toute commande git y
   échouait déjà, bug préexistant à `controller-bootstrap`, jamais débusqué
   faute de test e2e exerçant une vraie commande git. Fix : second
   `VolumeMount` du même volume `workspace` sur `repo.git`
   (`controller/src/sandbox.rs`).
2. **Refspec de fetch manquante, cible corrigée** : le point ouvert légué
   par WS-9 (`ws09-sandbox-maint-agent.md`, maintenant obsolète) proposait
   `+refs/heads/*:refs/heads/*` — **imprécis**, vérifié faux en local :
   ça aurait écrasé les branches locales des worktrees à chaque fetch. La
   bonne cible, confirmée par test réel (`git fetch` avant/après) et
   directement exigée par le design de `/git/unpushed`
   (`refs/remotes/origin/<branche>`) : `+refs/heads/*:refs/remotes/origin/*`,
   posée par `vanyline-maint init` (`git config --replace-all`, idempotent).

Les deux ont été traités comme tâches 00/01 (controller puis sandbox) avant
les tâches produit 02/03 (git-status, git-unpushed) — validés par le
développeur via `AskUserQuestion` avant la moindre implémentation, cf. règle
"jamais modifier les sources avant que le plan de la tâche courante soit
validé". 4 tâches Qwen, aucune ronde de correction nécessaire — chaque
fichier de tâche fournissait un contrat de code quasi complet (vérifié en
local au préalable avec de vrais dépôts git, pas des suppositions sur le
format porcelain v2 ou la topologie bare+worktree) plutôt que des
signatures seules ; a bien fonctionné pour de la logique de parsing/format
externe où l'ambiguïté coûte cher. Compte de tests revérifié par Claude
lui-même après chaque tâche (`cargo test --workspace`, jamais celui du
rapport Qwen) : 509 (tâche 00) → 512 (01) → 525 (02) → 532 (03), 0 échec à
chaque étape.
