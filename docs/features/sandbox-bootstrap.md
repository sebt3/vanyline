# Feature — sandbox-bootstrap (WS-3)

## Ce que la feature fait

Donne vie à `vanyline-sandbox` : un serveur Rust qui expose les `vanyline-tools`
(filesystem, command, search) en **MCP HTTP streaming**, une image OCI de base conforme
à la recette validée (substrat natif + toolchains par image volumes), et un déploiement
de test sur le cluster.

**Point de départ : `kydah-mcp-template`** (`~/projets/kydah/kydah-mcp-template/`,
clonable depuis git.kydah.fr). Le template fournit déjà : transport MCP HTTP streamable
POST-only fait main sur axum (pas de dépendance à l'API server rmcp), auth JWT/JWKS
avec mode `--no-auth`, config clap/env, télémétrie (Prometheus + OTLP optionnel),
tests HTTP et CI. La sandbox est un fork adapté : on remplace les tools d'exemple par
la glue `vanyline-tools` et on intègre le code dans le crate `sandbox/` du monorepo.

**Indépendant de WS-0** : ne dépend que de `vanyline-tools` et du template — démarre
dès le jour 1. Bénéficie de WS-5 (tools-v2) : la glue consomme les schémas de
`tools/src/mcp.rs`, la surface d'outils suit automatiquement.

## Ce qu'elle ne fait pas

- Pas d'interface WebSocket éditeur (jalon ultérieur, après le MCP)
- Pas d'auth en phase 1 (voir phasage) — jamais exposé hors ClusterIP en attendant
- Pas de création de pods (c'est le controller) — le deploy de test est un YAML statique

## Architecture du binaire (héritée du template)

```
sandbox/src/
├── main.rs          # bootstrap : clap/env, tracing, axum
├── config.rs        # VNL_SANDBOX_PORT (défaut 8080), VNL_SANDBOX_ROOT (défaut /workspace), --no-auth
├── auth.rs          # JWT/JWKS du template — inactif en P1 (--no-auth), prêt pour P3
├── telemetry.rs     # tracing + métriques (du template)
├── mcp.rs           # JSON-RPC 2.0 + dispatch tools/list, tools/call (du template)
└── tools_impl.rs    # glue vanyline-tools → résultats MCP
```

- Transport : MCP HTTP streamable POST-only sur `/mcp` (+ `GET /health`) — le code du
  template, éprouvé. **Tâche de validation croisée** : le client rmcp de vanyline-lib
  doit dialoguer avec ce serveur (c'est le couple réellement déployé).
- Les définitions JSON de `tools/src/mcp.rs` sont la source des schémas ; la glue
  appelle `vanyline_tools::{filesystem, command, search}`.
- **Confinement** : tous les chemins sont résolus sous `VNL_SANDBOX_ROOT`
  (canonicalisation + préfixe vérifié, erreur `VNL-SBX-001` sinon). `execute_command`
  a `cwd = VNL_SANDBOX_ROOT`. C'est du garde-fou d'ergonomie, pas une frontière de
  sécurité — la frontière de sécurité, c'est le pod.

## Image et déploiement

- `sandbox/Dockerfile` : build multi-stage → `debian:trixie-slim` + substrat natif
  validé (cc/ld, binutils, libc-dev, make, pkg-config, git, curl, vim) + binaire.
- `deploy/sandbox/sandbox-test.yaml` : pod + service ClusterIP, PVC de travail monté sur
  `/workspace`, toolchains rust + node en `volumes[].image` avec l'injection d'env
  exacte (la recette image-volumes a été validée par des pods d'expérimentation,
  `deploy/sandbox-imagevol-*.yaml`, depuis supprimés).
- Test de bout en bout documenté : port-forward + client MCP (le CLI vanyline avec le
  serveur en config mcp) → `execute_command("cargo --version")` répond.

## Phasage auth (rappel des décisions AGENTS.md)

1. **P1 (ce chantier)** : `--no-auth`, ClusterIP uniquement.
2. **P2** : SA TokenReview + NetworkPolicy (chemin kydah-code/app).
3. **P3** : JWT (chemin frontend via ingress) + interface WS éditeur — le code
   JWT/JWKS du template est déjà dans la place, il n'attendra que sa config.

P2/P3 auront leurs propres design docs le moment venu.

## Risques et questions ouvertes

- **Interop client rmcp ↔ serveur template** : le template suit la spec streamable
  POST-only ; le client de vanyline-lib est rmcp. À valider dès la tâche 1 — si ça
  coince, c'est côté serveur qu'on ajuste (on maîtrise ce code).
- **Version de spec MCP** : le template cible 2024-11-05 ; vérifier l'écart avec ce
  que rmcp 1.7 négocie (protocolVersion) au moment du fork.
- **Bornes de sortie** : reprises de tools-v2 (constantes de la crate tools) — rien à
  redéfinir ici.
- **Streaming des commandes longues** : hors scope v1 (résultat à la fin) ; le
  transport le permettra plus tard.

## Découpage en tâches candidates

1. `fork-template` — import du squelette template dans `sandbox/` (config, telemetry, mcp.rs, un tool `ping`), adapté au workspace ; test d'interop avec le client rmcp de la lib en local. 
2. `tools-glue` — les tools de tools-v2 branchés, confinement des chemins. Tests unitaires du confinement.
3. `image` — Dockerfile + build local documenté.
4. `deploy-test` — YAML complet (PVC + image volumes + env) + procédure de validation cluster ; suppression des YAML d'expérimentation.
