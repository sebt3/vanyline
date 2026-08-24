# Git intégration — statut, diff, commit, branches, merge, push, SSH (2026-08-21 → 08-24)

Feature demandée par le développeur en priorité explicite ("git reste le cœur
articulaire du processus de production de code"), sortie de `docs/backlog.md`. Rouvre
volontairement les décisions "pas d'action git, pas de diff de contenu" de WS-11 —
c'était un arbitrage de scope pour livrer vite, pas une contrainte permanente
(correction actée par le développeur en session, pas une redécouverte de Claude).

## Ce qui a été livré

12+ endpoints `sandbox/src/git.rs` (statut, diff, staging, commit, branches CRUD,
checkout, merge/abort, push, log, clé SSH), un relais REST `app` → `/git/*`
(`ANY /api/sandboxes/{name}/git/{*path}`, réutilise le pattern JWT du relais de ticket
WS existant), `GitPanel.vue`/`DiffView.vue`/décoration `Explorer.vue` côté frontend.
Libs sur étagère retenues (directive explicite du développeur, deux fois : diff puis
graphe) : `@codemirror/merge` (`unifiedMergeView`), `@gitgraph/js`. Détail technique
complet migré dans `docs/architecture.md` (section "Serveur MCP" + sous-sections
dédiées) — ce fichier ne redit pas ce qui y est déjà écrit correctement.

## Décision structurante : clé SSH dans le PVC Owner, pas un Secret K8s

Premier jet de Claude proposait un Secret K8s dédié + un nouveau client K8s pour
`app` (RBAC nouvelle). **Rejeté par le développeur** : le PVC Owner
(`/home/vanyline`) existe précisément pour ce cas d'usage et est déjà monté au même
chemin dans le Job d'init du Project et le pod Sandbox — vérifié en code avant
d'accepter la correction, pas pris pour argent comptant. Argument du développeur qui
généralise au-delà de ce cas : généraliser les Secrets dédiés par type de credential
(SSH, GPG, kubeconfig...) multiplierait les objets à gérer sans raison — le PVC Owner
est le bon endroit pour ce type de données pour tout futur besoin similaire.
Conséquence : zéro nouvelle capacité pour `app`, `Project.spec.git_secret` déprécié
(champ CRD conservé, plus consommé).

## Escalades Cadence pendant la Phase 2 — le process a fonctionné

Deux blocages remontés par Cadence (DeepSeek v4 flash) avant d'écrire la tâche
concernée, conformément à son mandat ("ne complète pas le design toi-même") :
1. Comportement HTTP de `POST /git/merge` en conflit (200 structuré vs erreur HTTP) —
   le design doc avait deux indications contradictoires. Résolu : 200
   `{ conflicted, sha }`, le champ `ok` retiré (il faisait doublon, source de
   l'ambiguïté).
2. Canal d'auth frontend → `/git/*` — le design assumait que le frontend "consomme
   les endpoints" sans jamais spécifier le transport, alors que `/git/*` est sous
   JWT OIDC et le navigateur n'en détient jamais. Résolu : relais REST `app`,
   vérifié comme une extension d'un pattern déjà existant (pas une nouvelle
   capacité) avant de trancher.

Aucune des deux n'était devinable sans revenir au développeur/Claude — Cadence a bien
fait de s'arrêter plutôt que de choisir seule.

## Review Phase 3 — l'écart de gravité le plus important du projet à ce jour

Contrairement aux features Cadence précédentes (RBAC trop large, chemin relatif
faux, `fmt` non lancé, éléments UI non câblés — cf. `ws10-language-support.md`,
`editing-context-menus.md`), cette review a trouvé des bugs de sécurité réels, pas
des coquilles :

- **Traversal de chemin** dans le relais `app` (`/git/../mcp` atteignait un endpoint
  hors périmètre — la normalisation d'URL du client HTTP collapsait les `..` que
  `encode_git_path` laissait passer).
- **Injection d'argument** sur `/git/merge`/`/git/checkout`/`/git/branches`
  (`branch: "--abort"` interprété comme un flag git, pas une ref — `merge_args` et
  consorts n'avaient pas de séparateur `--`, contrairement à `diff_args`/`stage_args`).
- **Fail-open** sur le check dirty-tree de `/git/checkout` (`.unwrap_or(false)` sur un
  échec de spawn git → tree jugé propre par défaut, défaisait la garantie de sécurité
  actée en conception).
- Perte des `/` dans les noms de branche à travers le relais (axum décode `%2F` avant
  le handler, jamais ré-encodé — casse `feature/x`, convention courante).
- Contrat de passthrough du proxy rompu sur une réponse non-JSON (404/405 texte brut
  → 502 générique).

Tous corrigés en Phase 3 par Claude directement (fixes petits, bien spécifiés, pas de
nouvelle décision de design — accord explicite du développeur avant de le faire),
avec tests TDD ajoutés (`raw_git_tail`, `reject_leading_dash`, `is_dirty`). Un commit
portait aussi un trailer `Signed-off-by` avec le nom/email personnel du développeur
(branche non poussée, corrigé par réécriture d'historique `filter-branch` — leçon :
même Cadence peut faire fuiter des identifiants personnels dans un commit, à vérifier
en review comme le reste).

## Diagnostic de l'écart de gravité — pas une baisse de fiabilité de Cadence

Discuté explicitement avec le développeur, qui a raison sur le fond : ce n'est **pas**
un signal de limitation du modèle (DeepSeek v4 flash via Cadence reste jugé fiable,
capable de travail que Qwen ne ferait pas). C'est un trou de configuration des deux
côtés, vérifié en code, pas supposé :

- `cargo fmt` était **permis** pour Cadence (`.opencode/agents/cadence.md`,
  `bash: cargo fmt*: allow`) mais **absent** des "Commandes de validation" d'`AGENTS.md`
  que Cadence suit explicitement à l'étape 4 de sa boucle — l'instruction manquait,
  pas la permission. Corrigé (`fix/cadence-validation-gaps`, mergé 2026-08-24).
- Le mandat d'escalade de Cadence ("ne tranche pas une ambiguïté d'architecture")
  ne nommait que les ambiguïtés de comportement visibles, jamais les contraintes de
  sécurité implicites (validation d'argv/URL/chemin construits depuis une entrée
  utilisateur) — Cadence vérifie la conformité *fonctionnelle* au contrat de tâche,
  pas la sûreté de l'implémentation, et rien ne lui disait que c'était aussi son
  travail. Étendu explicitement dans `.opencode/agents/cadence.md`.
- Symétrique côté Claude : le design doc de cette feature spécifiait des contrats de
  comportement ("cet endpoint fait X") mais aucune contrainte d'implémentation
  sécurité — `.claude/config.md` (Phase 1) exige désormais cette contrainte
  explicitement dès qu'une interface fait transiter de l'entrée utilisateur vers une
  commande shell, une URL ou un chemin.

Root cause réelle : cette feature est la première branche Cadence-directe à
construire un mécanisme cross-composant inventé de zéro (le relais d'auth) et à
shell-out avec de l'input utilisateur en argv — les features précédentes (LSP, menus
contextuels, détection de langages) n'avaient rien de comparable comme surface
d'attaque. "Conforme au contrat" et "sûr" divergent précisément sur ces deux
familles de risque, et rien dans la chaîne design doc → tâche → implémentation →
review n'avait de garde-fou explicite avant cette session.

## Dette assumée, pas bloquante (détail : `docs/architecture.md`, "Limites connues")

`CreateProjectBody.git_secret` toujours accepté silencieusement côté API/CLI malgré
la dépréciation ; duplication non résolue (boilerplate `-C`+`run_git` ×15,
scoping owner dupliqué ×5 dans `app/src/api/sandboxes.rs`) ; bouton Diff sur fichier
supprimé toujours en échec ; `diffPatch.ts` reconstruit le diff inversé côté client
plutôt que de le demander à la sandbox (fragile). Staging par hunk, résolution de
conflit 3-way, graphe multi-branches complet : différés explicitement, pas exclus.
Pas de validation sur cluster réel (comme la plupart des features de cette liste).
