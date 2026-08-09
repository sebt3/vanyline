# ws12-sandbox-clients — client K8s CLI + toolbox (terminé et clos)

Rend les Owners/Projects/Sandboxes pilotables hors du cluster-admin :
extraction `vanyline-crds` (types CRD, sans runtime kube), `VnlK8sClient`
(`lib/src/k8s.rs`, feature Cargo `k8s` désactivée par défaut), commandes
CLI `owner`/`project`/`sandbox` (`list`/`show`/`create`/`delete`,
`sandbox stop`/`start`), méthodes JSON-RPC miroir (incl.
`sandboxes/stop`/`sandboxes/start`), toolbox en inférence (`--toolbox`,
`SessionContext.extra_mcp`). Détails : `docs/architecture.md` section
"Client K8s CLI". 11 commits (crate-crds, lib-k8s, cli-owner/project/sandbox,
rpc-owner/project/sandbox, toolbox-lib/cli, stop-start), 532 → 566 tests,
0 régression à aucune étape. `docs/features/ws12-sandbox-clients.md`
supprimé à la clôture (2026-08-01).

**Tâche `stop-start` (dernière du design, débloquée par
`ws13-sandbox-runtime.md`)** : `VnlK8sClient::set_sandbox_suspended(name,
suspended)` — patch merge JSON ciblé sur `spec.suspended`, pas de fonction
générique partagée avec le CRUD `list/get/create/delete` (un seul type
appelant, abstraction prématurée). Délégation Qwen sans round de
correction sur le code (diff conforme au contrat au premier essai), mais
**nouvelle occurrence du mode d'échec "compaction de contexte mi-session"**
déjà documenté sur ws15/ws13 : après avoir terminé l'implémentation, passé
tous les tests et lancé `cargo clippy`, la session a affiché une erreur de
dépassement de contexte du provider, puis un résumé post-compaction s'est
arrêté sur une question de confirmation avant de committer (jamais
répondue, session non interactive) — **aucun commit produit**, diff
correct mais non commité. Contrairement à ws15 (grosse tâche, échec
prévisible) et à ws13 (petite tâche, échec en cours d'édition), ici la
compaction a frappé **après** la fin du travail utile, au moment de
rédiger le rapport final — confirme que ce mode d'échec peut survenir à
n'importe quelle étape de la session, pas seulement pendant l'édition de
fichiers volumineux. Traité comme les précédents : diff vérifié
(`git diff`, conforme ligne à ligne au contrat de la tâche), tests
recomptés soi-même (566, +2 exact), commit fait directement par Claude
plutôt que re-délégué.

**Décisions d'architecture prises en cours de route (pas dans le design
initial)** :

- **Point d'API toolbox** : le design proposait un décorateur `ConfigStore`
  côté CLI pour injecter la sandbox comme serveur MCP sans toucher `lib`.
  Après lecture du code réel (`SessionContext` a déjà `local_tools` comme
  précédent d'"input direct de l'hôte"), tranché pour un champ explicite
  symétrique `extra_mcp: Vec<(McpServer, McpSelection)>` sur
  `SessionContext` plutôt qu'un faux `ConfigStore` — plus petit changement
  lib, testable isolément, pas de mensonge sur le contenu de la config
  pour rejouer une mécanique existante. Bon exemple de "la proposition du
  design n'est qu'une proposition" : elle a changé après inspection du
  code, pas juste actée.
- **CLI `create` : flags complets, pas de `-f fichier.yaml`** — proposition
  initiale du design rejetée par le développeur ("si je dois utiliser un
  fichier, autant kubectl apply -f directement ; chaque commande n'a besoin
  que de peu d'arguments, clap gère ça bien"). Levée à retenir : ne pas
  supposer qu'imiter `kubectl apply -f` est un service rendu quand la
  vraie alternative (`kubectl` lui-même) existe déjà et fait ça mieux —
  la valeur d'un CLI dédié est dans les flags ciblés, pas dans la
  réplication d'un mécanisme générique.
- **`service_name`/`MCP_PORT` déplacés de `controller` vers `vanyline-crds`**
  (retouche de `controller/src/sandbox.rs` une deuxième fois après la tâche
  `crate-crds`) plutôt que dupliqués dans `lib` : `VnlK8sClient::sandbox_mcp_url`
  doit calculer la même URL que celle que le controller pose réellement — une
  seule source de vérité pour un nom de Service + un port, décidé avec le
  développeur plutôt que supposé.
- **Convention de test K8s vs MCP, à ne pas confondre** : les appels
  `Api<K>::list/get/create/delete` de `VnlK8sClient` ne sont JAMAIS
  unit-testés (pas de mock d'API server K8s dans ce projet, même principe
  que les reconcilers du controller) — mais les connexions **MCP**, elles,
  SONT testées avec un vrai serveur HTTP local
  (`lib/tests/mcp_connection_lifecycle.rs`, pattern réutilisé pour
  `extra_mcp` dans `lib/tests/toolbox_extra_mcp.rs`). La distinction tient
  à la légèreté de monter un serveur HTTP en local (trivial, `axum`+`rmcp`
  déjà en dev-deps) contre l'absence d'équivalent léger pour une vraie API
  Kubernetes. Un test qui déclencherait un vrai appel K8s contacterait un
  cluster réel sur la machine d'un développeur avec un kubeconfig valide —
  interdit, rappelé explicitement dans chaque tâche `rpc-*`/`cli-*`.

**Pièges techniques rencontrés (utiles pour tout futur code touchant les
mêmes types)** :

- **`#[derive(CustomResource)]` génère TOUJOURS `status: Option<S>`**,
  jamais `S` nu — même quand le design/le code partiel écrit
  `owner.status.pvc_name` sans `Option`. Bug trouvé dans mes propres
  specs de tâche (`cli-owner`, 2 rounds de fix), pas une erreur
  d'exécution de Qwen — leçon pour la rédaction des tâches futures :
  toujours écrire `match &x.status { Some(s) => ..., None => ... }`
  pour tout type CRD dérivé, jamais d'accès direct.
- **`k8s_openapi::Condition` a `type_: String` (pas `r#type`) et
  `message: String` (pas `Option<String>`, jamais vide via `as_deref`)**
  — vérifié dans les sources vendored, pas une supposition. Même piège
  que ci-dessus, trouvé dans mes propres specs, corrigé en 1 round.
- **`kube` avec `default-features = false, features = ["derive"]`
  n'embarque PAS `kube-client`/`kube-runtime`** (vérifié dans le
  `Cargo.toml` publié de `kube-4.0.0` : `derive = ["kube-derive",
  "kube-core/schema"]`, aucune dépendance sur `client`) — c'est ce qui
  permet à `vanyline-crds` de rester consommable par un CLI léger sans
  tirer la machinerie réseau de l'opérateur. Sans `default-features =
  false` explicite, le `default` de `kube` (`client` + `rustls-tls` +
  `ring`) s'ajoute silencieusement même si on ne déclare que `derive`.

**Découpage en sous-tâches, au-delà du design initial** : les tâches
candidates `cli-commands` (owner+project+sandbox) et `rpc-methods` (idem)
se sont révélées trop larges pour la règle des 30-45 min dès la
rédaction — scindées par type de ressource (03a/03b/03c, 04a/04b/04c) et
`toolbox` scindée en lib/cli (05a/05b) **avant** la première tentative,
pas après un échec — contrairement à `cli-harness` où le découpage
s'était fait pendant l'exécution. Fonctionne aussi bien en amont qu'en
cours de route : le signal ("cette tâche touche 3 types x 4 opérations
quasi identiques") est détectable à la lecture du design, pas seulement
à l'usage.

**Nouveaux modes d'échec Qwen observés (complètent `outillage-llm-exec.md`)** :

- **Qwen peut corrompre du code SANS RAPPORT avec la tâche en cours en
  éditant maladroitement autour** (`cli-owner`, tâche 03a) : en ajoutant
  du nouveau code en fin de `main.rs`, une édition antérieure dans le
  fichier a supprimé la fin d'une fonction préexistante (`run_agent`,
  branche `Show`) sans lien avec la tâche — `cargo check` a révélé un
  "unclosed delimiter" loin de la zone éditée. Contrairement aux échecs
  précédents (permission auto-rejetée, apostrophes) qui bloquent la
  session AVANT tout commit, celui-ci laisse un diff non commité mais
  DANGEREUX si on ne vérifiait pas `cargo check` avant de committer.
  Fix : tâche de correction chirurgicale (diff exact old/new fourni) —
  même principe que "corriger en déléguant", mais formulé comme
  continuation de la tâche interrompue (même message de commit final),
  pas comme un fix séparé, puisque rien n'avait encore été commité.
- **Qwen ignore parfois le format de commit même avec la consigne
  explicite ET la procédure `-F`** (`rpc-owner`, 04a : message
  `feat(cli): ...` au lieu de `(feat: ws12-sandbox-clients) rpc-owner`)
  — 3ème confirmation sur ce projet de la fiabilité limitée du
  formatting de commit délégué. Fix : `git reset --soft HEAD~1` +
  recommit avec le bon message (sûr, rien n'est poussé) — a aussi permis
  de corriger au passage un fichier non lié agrégé dans le même commit
  (`docs/features/ws15-quality-hygiene.md`, non tracké, sans rapport)
  malgré la consigne explicite "jamais `git add -A`".
- **Qwen peut recopier une valeur d'exemple sensible dans la doc générée**
  (04a : un prénom réel utilisé comme nom d'exemple `Owner` dans
  `docs/rpc-protocol.md`, alors que la règle "pas de prénom dans les
  fichiers commités" vit dans le CLAUDE.md global du développeur, invisible
  de Qwen). Depuis : rappel explicite dans chaque prompt de délégation
  suivant ("jamais de prénom réel dans les exemples, noms génériques
  type alice/demo-project") — Qwen n'a plus reproduit ensuite. Leçon
  généralisable : toute règle vivant dans un CLAUDE.md privé et invisible
  de Qwen doit être rappelée explicitement dans le prompt de délégation
  si elle peut s'appliquer au contenu généré (docs, exemples), pas
  seulement au code.
