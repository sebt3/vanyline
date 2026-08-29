# sandbox-state-ws — push temps réel des phases de sandbox (2026-08-29)

Feature courte, implémentée **directement par Claude** (pas Qwen/`.tasks/`, pas
Cadence) sur la branche `feat/sandbox-state-ws`, à la suite de `6948d23`
(« réactivité croisée FS éditeur/LLM → explorer/git », travail du développeur sur
`main`). Pas de design doc formel (menée hors Phase 1, comme
`chat-app-fonctionnel`).

## Ce que ça fait

`GET /api/ws/sandbox-state` (app, same-origin, cookie OIDC — pas de ticket) :
pousse au navigateur les changements de `status.phase` des sandboxes de
l'utilisateur, en temps réel.

- **lib** : `VnlK8sClient::watch_sandboxes()` → `Pin<Box<dyn Stream<WatchEvent<Sandbox>>>>`
  (kube-runtime `watcher`, timeout serveur 5 min). Feature `k8s` tire
  `kube-runtime` (dep directe, feature `runtime` de `kube` **pas** ajoutée —
  inutile, on importe `kube_runtime` directement).
- **app** (`ws/sandbox_state.rs`) : un **seul** watch partagé par toutes les
  connexions via `AppState.shared_sandbox_state` (`SharedState` : `parking_lot`,
  liste de subscribers, cache `project`→`owner` avec mémorisation des miss). Tâche
  `watch_loop` lancée à la 1ʳᵉ connexion (double-checked locking sur `watch_handle`),
  tourne ensuite pour la vie du process, se met en pause tant qu'aucun subscriber.
  Dispatch scopé par owner : namespace multi-tenant → on résout
  `Sandbox.spec.project` → `Project.spec.owner`.
- **controller** : le `Role` de `app` (`build_application_role`) gagne le verbe
  `watch` sur `sandboxes` — sans quoi `kube-runtime::watcher` boucle sur des 403.
  Pas de changement au `deploy/controller/controller.yaml` : la ClusterRole du
  controller détient déjà `watch` cluster-wide, la délégation namespacée n'est pas
  une escalade. Test de forme (`build_application_role_shape`) mis à jour.
- **frontend** : hub singleton `useSandboxState` (`composables/`), reconnexion
  avec back-off exponentiel 3s→30s (remis à zéro après une session stable) —
  évite la boucle serrée pour un utilisateur sans owner K8s, que le backend ferme
  aussitôt. `ProjectDashboard` s'y abonne. **Le payload `phase` n'est en fait pas
  consommé** : le seul consommateur se sert du WS comme *signal* et débounce un
  refetch du listing CRUD (300 ms). La map `sandboxPhases` attend un futur
  `SandboxDashboard` live.

## Review Phase 3

Faite par Claude sur le code que Claude venait d'écrire. Corrections de la 1ʳᵉ
passe (toutes committées avant clôture) :

1. **RBAC `watch` manquant** — bloquant : le watcher aurait bouclé sur des 403 en
   prod. Exactement le motif « capacité supposée mais non écrite dans le design »
   (cf. `git-integration`). Rappel : sans design doc, personne ne le devine.
2. **Course sur le démarrage de `watch_loop`** : `is_some()` puis `spawn` puis
   `= Some` avec le lock relâché entre les trois → deux connexions concurrentes
   pouvaient spawner deux loops. Corrigé en double-checked locking.
3. **`#![allow(clippy::unwrap_used)]` global** sur le module → régression WS-15.
   Retiré, passage à `parking_lot::Mutex` (plus aucun `.lock().unwrap()`).
4. **Zéro test** sur le nouveau module → 3 tests ajoutés (sérialisation camelCase
   + `phase: null`, filtrage du dispatch par owner, fan-out).
5. Churn cosmétique dans `lib/src/k8s.rs` (renommages `kube::` → bare, virgules
   traînantes, un test sans rapport) → annulé, diff réduit à l'ajout de la feature.
6. Front : boucle de reconnexion serrée pour un user sans owner → back-off.

Dette assumée laissée (documentée dans `docs/architecture.md`, section frontend) :
`watch_loop` continue à consommer le stream ~5 min après le départ du dernier
subscriber ; channel `unbounded` ; pas de ping serveur ; course mount/unmount/mount
possible sur le hub front.

Un commit `fix:` séparé (`a1133b6`) corrige au passage un `needless_borrow` clippy
**pré-existant** (rust 1.97, plus strict que la CI) dans les asserts openapi de
`main.rs` — il bloquait `cargo clippy --all-targets` en local.

## Migration architecture

Design migré dans `docs/architecture.md` : §WebSocket sandbox-state (section app),
verbe `watch` du Role (section controller), note hub dashboard (limites frontend).
`AGENTS.md` : ligne ajoutée à la table des interfaces. Pas de `docs/features/*.md`
à supprimer (jamais créé).

## Réglage DeepSeek-V4-Flash — trouvé pendant cette session

Le développeur a relevé que la fiche modèle **DeepSeek-V4-Flash-0731** prescrit
`temperature 1.0` / `top_p 0.95` (+ `reasoningEffort` élevé pour le Code Agent)
pour les charges agentiques locales — or `.opencode/agents/cadence.md` tournait à
`temperature: 0.2`, hors du point de fonctionnement d'un modèle de raisonnement
RL-tuné (même logique que l'avertissement « pas de temp 0 » sur R1). Corrigé
(`ad1cf49`) : cadence → `1.0` / `top_p 0.95` / `reasoningEffort high` /
`textVerbosity low` / `reasoningSummary auto` (= variant `high` du provider
`smart` dans `~/.opencode/opencode.json`). `release` (nouvel agent) : `temperature`
laissée à `0.8` — fiabilité prioritaire, choix assumé du développeur —, mêmes
`top_p`/`reasoning`.

Nuance vs `git-integration.md` : le diagnostic d'alors (« pas une baisse de
fiabilité du modèle, deux trous de config ») reste valide (les deux trous étaient
réels), mais cadence tournait **aussi** hors specs modèle. Les features déléguées
à cadence tournent désormais dans les conditions prévues — variable à garder en
tête pour juger les prochaines reviews. Cf. [[outillage-llm-exec]].

## Agent opencode `release` (créé cette session)

`.opencode/agents/release.md` — agent `primary` qui déroule
`docs/release-runbook.md` (validation locale → bump → tag → suivi CI →
redéploiement cluster de test). `model: smart/deepseek-v4-flash`. Garde-fous :
force-push de tag interdit (permissions), `kubectl delete` en `ask`, code source
et `docs/` non éditables, namespace cible obligatoire dans l'invocation. Le
développeur a ajusté les permissions (plusieurs `ask` → `allow` : `kubectl
apply/annotate/patch/scale`, `git push` non-force, `sed -i`) — accepté, c'est un
cluster de test. À ajouter éventuellement à la liste des agents de
`.claude/config.md` (décision développeur, fichier commité).

## Statut

Branche `feat/sandbox-state-ws` mergée dans `main` et poussée. Toujours pas testé
sur cluster réel (comme toutes les features depuis `ws10`).
