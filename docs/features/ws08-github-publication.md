# Feature — ws08-github-publication

## Ce que la feature fait

Rend le repo publiable sur GitHub : Dockerfiles à leur place, CI complète
(validation + release binaire/images multi-arch), `deploy/` trié, README.
Source d'inspiration CI : `~/projets/juke/.github/workflows/` (patterns validés).

## Ce qu'elle ne fait pas

- Pas de publication effective (le "go" public est une décision du développeur)
- Pas de chart Helm ni d'operator lifecycle (les YAML statiques suffisent)
- Pas de versioning sémantique outillé (le tag manuel déclenche la release)

## État des lieux

- `sandbox/Dockerfile` et `controller/Dockerfile` existent déjà ; `app/Dockerfile`
  a été créé à partir du builder racine supprimé (adapter les chemins
  de contexte : le build reste lancé depuis la racine du monorepo,
  `docker build -f app/Dockerfile .`).
- Stack TLS 100 % rustls (`openssl-probe` seul dans le lock) : la
  cross-compilation ne traîne aucune dépendance C TLS. `libssl-dev` (stage build)
  et `libssl3` (runtime) du builder supprimé étaient des vestiges et ont été
  retirés au profit de `ca-certificates` + `rustls-native-certs`.

## CI GitHub

### `.github/workflows/test.yml` — sur push/PR

- `cargo fmt --all --check`
- `cargo clippy --workspace` (deny warnings)
- `cargo test --workspace`
- frontend : `npm ci && npm run build && npx svelte-check && npm run test`
- cache : `Swatinem/rust-cache` + cache npm
- optionnel (peut arriver en fin de sprint avec WS-10) : `cargo llvm-cov --lcov`
  + upload Codecov (le format pivot lcov est déjà la décision WS-10)

### `.github/workflows/release.yml` — sur tag

- **Binaire `vanyline` (CLI)** x86_64 + aarch64 linux :
  `taiki-e/create-gh-release-action` puis `taiki-e/upload-rust-binary-action`
  (matrix targets, `cross` pour aarch64, `manifest-path: cli/Cargo.toml` —
  même piège de workspace que juke : scoper le package explicitement)
- **3 images multi-arch** (linux/amd64 + linux/arm64) vers ghcr.io :
  qemu + buildx + `docker/build-push-action`, une par Dockerfile
  (`app/`, `sandbox/`, `controller/`), tags `latest` + `${ref_name}`

## Tri de `deploy/`

```
deploy/
├── web/          # deployment.yaml, service.yaml, ingress.yaml, configmap.yaml,
│                 # secret.yaml, RestEndPoint_sso.yaml
├── controller/   # controller.yaml, crds.yaml, generate-crds.sh
└── sandbox/      # sandbox-test.yaml
```

Les YAML d'expérimentation `sandbox-imagevol-*.yaml` (pods de validation de la
recette image volumes, périmés depuis que la recette est dans AGENTS.md et le
controller) sont **supprimés**. Mettre à jour toute référence aux chemins
(README, docs, éventuels scripts).

## README.md

- Ce qu'est vanyline (les axes : harness lib, CLI + RPC stdio, app web,
  sandbox, controller) — reprendre le schéma d'AGENTS.md
- Comment déployer **l'app** (`deploy/web/`) et **le controller**
  (`deploy/controller/`, CRDs, `SANDBOX_IMAGE`)
- Build local (cargo, npm, images)
- Section "limites de sécurité connues" : R16 (`StrictHostKeyChecking=no` sur
  les jobs git), mode `--no-auth` des pods sandbox (frontière = pod + netpol)

## Risques et questions ouvertes

- **`gh` indisponible en local et CI intestable hors GitHub** : la mise au point
  des workflows se fera par itérations de push une fois le repo GitHub actif —
  prévoir la tâche en conséquence (petits commits, pas de validation locale
  possible au-delà du lint YAML).
- **Nom des images** : `ghcr.io/<owner>/vanyline-{app,sandbox,controller}` —
  à confirmer avec le nom du repo GitHub final.
- Le build app fait `npm run build` du frontend dans l'image : vérifier que le
  cache buildx couvre correctement les deux écosystèmes (sinon temps de build
  multi-arch douloureux).

## Découpage en tâches candidates

- [x] `dockerfiles` — `app/Dockerfile` créé (builder racine supprimé), retrait libssl si
   vestige, vérif build local des 3 images
- [ ] `deploy-tri` — arborescence web/controller/sandbox, suppression des YAML
   d'expérimentation, mise à jour des références
- [ ] `ci-test` — workflow de validation (rust + frontend + caches)
- [ ] `ci-release` — workflow de release (binaire matrix + 3 images multi-arch)
- [ ] `readme` — README complet, section limites de sécurité (R16)
