# arrimage-fonctionnel-2026-08 — 7 features, web IDE réellement branché (terminé)

Famille de features enchaînées du 2026-08-10 au 2026-08-12, dans l'ordre :
`chat-todo-live` → `app-k8s-provisioning` → `sandbox-ws-runtime` →
`settings-real-config` → `controller-application-crd` → `sandbox-ingress-wiring` →
`explorer-editor-terminal-wiring`. Objectif atteint : le shell IDE Vue (posé visuellement
par `frontend-ui-shell`, fermée en même temps que cette famille) est maintenant
réellement branché sur une sandbox K8s — Explorer/Editor/Terminal ne sont plus des
mocks. Détails architecturaux migrés dans `docs/architecture.md` (sections "Backend
web", "Serveur MCP" — sous-section "WebSocket éditeur", "Opérateur Kubernetes" —
sous-sections "CRD Application"/"Ingress par Sandbox", "Frontend — shell IDE Vue").
Les 8 design docs (7 + `frontend-ui-shell.md`) sont supprimés.

## Processus qui a bien fonctionné : Claude conçoit, Cadence (DeepSeek) dispatche et
## écrit les tâches, Qwen implémente, Claude revoit avec build/test réels

Chaque feature : design doc écrit par Claude (avec le développeur pour les vraies
décisions d'architecture) → donné à Cadence, qui découpe en tâches et les fait
implémenter par Qwen, valide, commit → rapport de clôture à Claude → **revue
systématique par Claude avec `cargo test`/`clippy`/`fmt` réels, jamais de confiance
aveugle sur "tests verts"**. Ce dernier point a payé à chaque feature, pas
occasionnellement :
- `chat-todo-live` : le dernier commit du feature était en réalité **rouge**
  (`SettingsView.spec.ts` n'avait jamais reçu le fix d'une assertion périmée).
- `app-k8s-provisioning` : bug réel dans `sanitize_owner_name` (troncature à 63
  caractères pouvait couper pile sur un `-`, nom K8s invalide) — jamais détecté par
  les tests fournis (aucun cas ne tombait sur cette position).
- `sandbox-ws-runtime` : `clippy --all-targets -D warnings` (pas juste `clippy` seul)
  révélait 9 erreurs à chaque fois que ce n'était pas explicitement vérifié avant
  commit — motif récurrent, pas un accident isolé.
- `explorer-editor-terminal-wiring` : bug de timing réel dans `Terminal.vue`
  (`sendResize()` appelé avant l'event `'open'` du WebSocket, no-op silencieux) —
  masqué par un mock de test avec `readyState = OPEN` dès la construction, pas
  représentatif d'un vrai navigateur.

Cadence elle-même a montré une vraie rigueur, pas juste Qwen qui exécute : a détecté et
loggé le piège `cargo fmt --all` (interdit, dérive hors périmètre) avant de committer,
a découvert par elle-même que le protocole `read` numéroté/tronqué de `sandbox-ws-runtime`
casserait l'éditeur avant même que la tâche Editor ne soit écrite, a documenté
précisément l'adaptation provide/inject nécessaire pour dockview. Signaler
explicitement les incertitudes ("à valider en revue") plutôt que de trancher seule sur
tout — bon calibrage de ce qui relève de Cadence vs. de la revue Claude/développeur.

## Le motif le plus récurrent de toute la famille : la doc ment, vérifier avant d'agir

Trouvé et corrigé au moins quatre fois, jamais supposé :
1. `AGENTS.md` documentait "controller/ (déféré)" alors qu'implémenté et déployé
   depuis 2026-07-11 (`controller-bootstrap`) — trouvé en creusant l'écart
   code/contrat visuel du frontend, avant même le début de cette famille.
2. `app/Cargo.toml` n'activait pas la feature Cargo `k8s` de `vanyline-lib` —
   `VnlK8sClient` n'était pas compilé côté `app`, contrairement à ce que suggérait
   l'architecture documentée. Découvert en vérifiant, pas en lisant la doc.
3. `AGENTS.md` documentait deux mécanismes d'auth sandbox (JWT pour le frontend, SA
   TokenReview pour kydah-code/l'app) — seul JWT/JWKS existe réellement dans
   `sandbox/src/auth.rs`, SA TokenReview n'a jamais été construit. Découvert pendant
   `sandbox-ingress-wiring` en cherchant quel credential utiliser pour l'appel
   serveur-à-serveur `app` → sandbox.
4. Le protocole `read` de `/ws/fs` (`sandbox-ws-runtime`, déjà mergée) numérote et
   tronque — découvert seulement en implémentant l'Editor (`explorer-editor-terminal-wiring`),
   a nécessité de rouvrir une feature déjà close pour ajouter un mode `raw`.

Le point commun : une affirmation d'architecture (dans un doc, ou dans le code d'une
feature précédente) n'est vraie que jusqu'à preuve du contraire — la vérifier contre
le code réel avant de la prendre comme prémisse d'une tâche, pas seulement pour les
affirmations qui semblent surprenantes.

## Décisions d'architecture actées avec le développeur (pas supposées)

- **Auth WS navigateur → sandbox : ticket court-vécu à usage unique**, pas un JWT
  brut exposé au JS ni un nouveau mécanisme d'émission de token côté `app`. Réutilisé
  ensuite pour le relais `app` → sandbox du même ticket (`sandbox-ingress-wiring`),
  cohérent de bout en bout.
- **Owner K8s : provisioning paresseux restreint au seul `POST /api/projects`**, pas
  de création implicite sur une route de lecture — décision explicite préférée à
  l'auto-création systématique malgré la philosophie Kydah "tout ce qui peut être
  autoconfiguré doit l'être", parce que la création d'Owner a un effet de bord réel
  (ServiceAccount + PVC).
- **Postgres/DNS wildcard/certificat TLS wildcard : référencés, jamais provisionnés
  par le controller** — cohérent avec "on assemble sur étagère", pas une omission.
- **Routage sandbox : sous-domaine par sandbox** (`{name}.sandboxes.{host}`), pas de
  chemin sous l'host de l'app — implique un DNS/certificat wildcard externes, dette
  d'infra assumée et documentée, pas cachée.
- **Job backgroundé dans un terminal : doit survivre à la fermeture**, pas un bug à
  corriger. Fausse piste initiale (vouloir tuer tous les descendants, y compris
  backgroundés) corrigée après que le développeur a interrogé la prémisse — vérifié
  empiriquement (`/proc/<pid>/stat`, champ pgrp) que le mécanisme de kill ne pouvait de
  toute façon pas les atteindre, **et** confirmé nécessaire pour un cas d'usage déjà
  identifié (service de dev en arrière-plan, exposé plus tard via Service/Ingress).
  Leçon : quand le développeur pousse sur un diagnostic qui semblait acquis, revérifier
  depuis les faits plutôt que de défendre la conclusion initiale.
- **Cookie secret de la CR Application : auto-généré si absent**, jamais régénéré une
  fois créé (invaliderait les sessions actives) — seule pièce de la CR Application où
  l'autoconfiguration l'a emporté sur la référence explicite, car sans valeur métier à
  faire relire par un humain (contrairement à l'OIDC client secret ou Postgres).

## Pièges techniques trouvés en revue, à ne pas redécouvrir

- **`rustfmt` sur un fichier qui est une racine de crate (`lib.rs`/`main.rs`) traverse
  tout l'arbre de modules** — même en ne passant qu'un seul fichier en argument. Piège
  retombé dessus deux fois dans cette famille malgré la vigilance. Pour vérifier le
  formatage d'une racine de crate isolément : copier le fichier dans un répertoire à
  part avec des stubs vides pour chaque `mod` déclaré.
- **Process group kill sous contrôle de job bash** : un job foreground meurt via le
  hangup noyau déclenché par la mort du leader de session (pas via le `kill(-pgid)`
  lui-même, qui ne touche que le shell) ; un job backgroundé a son propre pgid, jamais
  atteint par aucun des deux mécanismes. Détail complet : `docs/architecture.md`
  section "WebSocket éditeur".
- **Test de synchronisation avec un marqueur `echo <texte>` auto-référent** : le pty
  réécho les octets tapés indépendamment de leur exécution par le shell (écho
  canonique du driver tty) — attendre l'écho de son propre marqueur ne prouve rien sur
  l'état réel du shell. Attendre le tout premier octet non sollicité (le prompt
  initial) n'a pas cette ambiguïté.
- **`ByteString` (k8s-openapi) encode/décode transparent en base64 à la sérialisation
  JSON** — poser une valeur déjà encodée en base64 dedans produit un double encodage
  dans l'appel API, mais le pod consommateur (`secretKeyRef`) ne voit que la couche
  décodée par K8s, donc le résultat net est correct. Vérifié en lisant l'implémentation
  `serde` du type, pas supposé — à revérifier de la même façon si le sujet revient.
