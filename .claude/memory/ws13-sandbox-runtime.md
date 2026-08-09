# ws13-sandbox-runtime — socle CLI, egress trois niveaux, suspension manuelle (terminé)

Trois consolidations indépendantes du runtime sandbox, 5 tâches Qwen (image-cmds,
crds-egress, netpol-builder, netpol-sandbox-reconcile + netpol-cascade-bump en
04a/04b, suspended), 548 → 564 tests, 0 régression à chaque étape. Détails
architecturaux migrés dans `docs/architecture.md` (section "Serveur MCP" pour le
socle CLI, section "Opérateur Kubernetes" pour les sous-sections "NetworkPolicies
egress à trois niveaux" et "Suspension manuelle"). `docs/features/ws13-sandbox-runtime.md`
supprimé à la clôture.

**Deux erreurs du design initial corrigées avant la première tâche** (Phase 1,
via `AskUserQuestion` avec le développeur, avant tout code) :

- Le design affirmait qu'un mécanisme de watch inter-CRD (Sandbox watchant
  Owner/Project) existait déjà pour propager un changement d'egress —
  **vérifié faux** (`grep -rn ".watches("` sur tout `controller/src` : rien,
  chaque reconciler ne surveille que sa propre CRD). Toujours vérifier une
  affirmation d'architecture du design contre le code réel avant de l'utiliser
  comme prémisse d'une tâche — un design doc peut contenir des suppositions
  jamais vérifiées, pas seulement des décisions actées.
- Mécanisme retenu à la place (proposé par le développeur après une comparaison
  coût/latence objective demandée explicitement) : pas de nouveau watch
  permanent. Un changement sur `Sandbox.spec` se réconcilie déjà immédiatement
  (watch natif kube-runtime sur sa propre CRD) ; pour qu'un changement sur
  `Owner.spec.egress`/`Project.spec.egress` se propage aussi vite, `owner.rs`/
  `project.rs` patchent une annotation de bump sur leurs Sandboxes à **chaque**
  reconcile (inconditionnel, pas de détection de changement — décision
  explicite du développeur : cohérent avec le reste du controller qui ne diff
  jamais rien nulle part, le coût est borné par l'intervalle de requeue déjà
  en place). "Meilleur des deux mondes" dans les mots du développeur : réaction
  quasi immédiate, zéro coût d'API-server supplémentaire en régime stable.

**Décision de sécurité tranchée avec le développeur, pas supposée** : la règle
DNS toujours présente dans la netpol egress (indispensable — sans elle toute
white-list casse la résolution DNS) **n'a aucune restriction de destination**
(pas de `podSelector`/`namespaceSelector` ciblant kube-dns). Alternative
rejetée (cibler kube-dns précisément, `namespaceSelector: kube-system` +
`podSelector: k8s-app=kube-dns`) : plus restrictif mais suppose une convention
de labels du cluster jamais vérifiée — une erreur y aurait cassé silencieusement
la résolution DNS de toute sandbox à egress restreint. Le développeur a choisi
l'option robuste plutôt que l'option précise quand l'écart de risque était
asymétrique à ce point.

**Cadence de délégation à Qwen sur cette feature** : 6 lancements consécutifs
sans aucun échec de code (tous les diffs produits collaient exactement aux
contrats fournis, vérifiés par relecture systématique après chaque tâche —
`git show`, jamais de confiance aveugle même sur un run qui se dit "propre").
Deux frictions rencontrées, toutes deux des blocages d'outillage plutôt que
des bugs de code, traitées en validant/committant directement (règle
existante "sauf blocage réel d'outillage") plutôt qu'en re-déléguant :
- Récidive du piège `external_directory` hors whitelist, cette fois en
  **écriture** d'un scratch file (`/tmp/<nom-libre>/` au lieu de
  `/tmp/opencode/*`) — déjà documenté une fois pour de la lecture (ws08),
  reconfirme qu'il faut vérifier ce point à chaque rédaction de tâche, pas
  seulement s'y fier de mémoire.
- Compaction de contexte mi-session sur une tâche pourtant petite
  (`crds-egress`, 5 fichiers, diffs courts) — le fichier
  `.tasks/commit-msg.txt` créé plus tôt dans la même session n'était "plus
  trouvé" après coup et Qwen s'est arrêté en posant une question de
  confirmation (jamais répondue, session non interactive). Contrairement au
  mode d'échec déjà documenté pour ws15 (grosse tâche, contexte 131K
  saturé d'un coup), ici la compaction a eu lieu en cours de session normale
  — signe que ce mode d'échec n'est pas strictement corrélé à la taille de
  la tâche, à surveiller même sur des tâches courtes.

**Découpage `netpol-reconcile` en 04a/04b décidé avant la première tentative**
(pas après un échec, contrairement à `cli-harness`) : la tâche candidate du
design mélangeait deux fichiers/reconcilers distincts (Sandbox pour
l'application de la netpol, Owner+Project pour la cascade de propagation) —
signal détecté à la rédaction, même pattern que `ws12-sandbox-clients`.
