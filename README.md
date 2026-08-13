# vanyline

Environnement de développement cloud-native, multi-utilisateur et piloté par l'IA, construit pour Kubernetes.

## À quoi ça sert

vanyline offre des espaces de travail isolés dans des pods Kubernetes. Chaque développeur dispose d'un éditeur web complet et d'un assistant LLM qui a accès aux mêmes outils, fichiers et commandes que lui — pas une simulation, le vrai shell.

Les toolchains (Rust, Node, Go, Python…) sont composées à la volée à partir de définitions déclaratives : pas de rebuild d'image, pas de configuration manuelle. Tu déclares les outils dont tu as besoin, le pod les monte au démarrage.

## Pour qui

Pour les développeurs qui font tourner un cluster Kubernetes et veulent :
- des environnements de dev reproductibles et isolés par projet
- un assistant IA qui opère dans le même contexte qu'eux (accès réel au code, aux commandes, aux fichiers)
- une gestion multi-utilisateur sans friction

## Composants

| Composant | Rôle |
|-----------|------|
| **frontend** | Éditeur de code web + interface de conversation LLM |
| **app** | Backend : authentification OIDC, sessions, orchestration LLM, API de configuration |
| **sandbox** | Pod Kubernetes embarquant un serveur WebSocket/MCP — accès réel au code et aux commandes |
| **controller** | Opérateur K8s gérant les ressources Application, Owner et Sandbox |

## Déploiement

Manifestes Kubernetes dans `deploy/` :

- `deploy/web/` : l'app (déploiement, service, ingress, configmap, secret,
  RestEndPoint_sso pour l'auth OIDC via kuberest)
- `deploy/controller/` : l'opérateur (CRDs, RBAC, déploiement) — régénérer
  `crds.yaml` après tout changement de schéma via `deploy/controller/generate-crds.sh`
- `deploy/sandbox/` : manifeste de test pour une sandbox (pas géré par le
  controller pour l'instant, usage développement)

Déployer l'app :

```bash
kubectl apply -f deploy/web/
```

Déployer le controller (CRDs d'abord, une fois) :

```bash
kubectl apply -f deploy/controller/crds.yaml
kubectl apply -f deploy/controller/controller.yaml
```

Le controller lit `SANDBOX_IMAGE_TAG` et `APP_IMAGE_TAG` (args `--sandbox-image-tag`/
`--app-image-tag` ou variables d'env de son déploiement) pour savoir quel tag utiliser
sur `ghcr.io/sebt3/vanyline-{sandbox,app}` — sans ces variables, il retombe sur sa
propre version (`CARGO_PKG_VERSION`).

## Build local

Images (depuis la racine du repo, un Dockerfile par composant) :

```bash
podman build -f app/Dockerfile -t vanyline-app:dev .
podman build -f sandbox/Dockerfile -t vanyline-sandbox:dev .
podman build -f controller/Dockerfile -t vanyline-controller:dev .
```

Binaires, hors image :

```bash
cargo build --workspace
npm run build   # frontend
```

## Limites de sécurité connues

- **`StrictHostKeyChecking=no`** sur les jobs git du controller
  (`GIT_SSH_COMMAND`, `controller/src/project.rs`) : la clé hôte du serveur
  git n'est pas vérifiée lors du clone/fetch. Acceptable pour l'instant
  (repos internes, réseau de confiance) — à durcir avant un usage avec des
  remotes non maîtrisés.
- **Mode `--no-auth` de la sandbox** (`sandbox/src/config.rs`) désactive
  l'authentification JWT/TokenReview — usage développement uniquement (log de
  warning au démarrage). La frontière de sécurité devient alors le pod
  lui-même et sa NetworkPolicy, pas un token applicatif : ne jamais exposer
  une sandbox `--no-auth` au-delà du réseau interne du namespace.

## État du projet

En développement actif. Pas encore utilisable en production.
