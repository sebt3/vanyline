# Runbook — release + validation live

Procédure suivie et stabilisée le 2026-08-14 pendant la première validation
bout-en-bout de la CRD `Application` sur un environnement de test réel
(`media-test`). But de ce fichier : rendre la procédure directement rejouable
en session, sans redécouvrir les pièges au fil de l'eau.

Contexte des commandes : `kubectl` déjà pointé sur le bon contexte/cluster,
répertoire de travail = racine du repo. Remplacer `media-test` par le
namespace cible.

## 1. Valider avant de couper une release

Toujours avant de bump la version — la CI (`Tests` workflow) peut être rouge
pour des raisons pré-existantes sans rapport avec le changement en cours ; ne
pas s'y fier seul, valider en local d'abord :

```bash
cargo test --workspace
cargo clippy --workspace          # pas --all-targets -D warnings : plus strict que la
                                   # validation locale standard, peut avoir des faux
                                   # positifs pré-existants sans rapport (vérifier par
                                   # `git stash` + re-run si un doute existe)
npm run build                     # vue-tsc -b && vite build
npm run test
```

Zéro régression sur les quatre avant de continuer.

## 2. Bump de version

```bash
# Cargo.toml — [workspace.package] version
sed -i 's/version = "X.Y.Z"/version = "X.Y.Z+1"/' Cargo.toml

# deploy/controller/controller.yaml — 3 occurrences (image + 2 env vars)
sed -i 's/vX\.Y\.Z/vX.Y.Z+1/g' deploy/controller/controller.yaml

cargo check --workspace           # régénère Cargo.lock avec la nouvelle version
```

**Si le schéma d'une CRD a changé** (nouveau champ, struct, etc.) — toujours
avant de commiter le bump :

```bash
deploy/controller/generate-crds.sh   # régénère deploy/controller/crds.yaml
```

Commit dédié :

```bash
git add Cargo.toml Cargo.lock deploy/controller/controller.yaml [deploy/controller/crds.yaml]
git commit -m "chore: bump version X.Y.Z+1"
```

## 3. Push + tag

```bash
git push origin main
git tag -a vX.Y.Z+1 -m "vX.Y.Z+1"
git push origin vX.Y.Z+1
```

**Piège vécu — préfixe `v` obligatoire.** `.github/workflows/release.yml` tague
les images sur `github.ref_name` (le nom du tag git tel quel). Un tag sans `v`
publie des images `X.Y.Z` que rien dans les manifests ne référence (ils
attendent `vX.Y.Z`) — `deploy/controller/controller.yaml` a eu ce bug depuis la
toute première release avant d'être corrigé.

**Si le tag doit être redéplacé** (fix poussé après un premier tag cassé,
ex. lockfile npm désynchronisé bloquant le build de l'image `app`) :

```bash
git tag -f -a vX.Y.Z+1 -m "vX.Y.Z+1"
git push origin vX.Y.Z+1 --force
```

Ne jamais faire ça sans que le développeur l'ait explicitement demandé —
c'est un force-push de tag, action destructive sur un repo public.

## 4. Suivre le build CI

```bash
curl -s "https://api.github.com/repos/sebt3/vanyline/actions/runs?event=push&per_page=3" \
  | python3 -c "
import sys,json
d=json.load(sys.stdin)
for r in d.get('workflow_runs',[]):
    print(r['id'], r['name'], r['head_sha'][:8], r['status'], r.get('conclusion'), r['created_at'])
"
```

Puis poller les jobs du run `Release` trouvé (`create-release`,
`image (app|sandbox|controller)`, `upload-cli (...)`) jusqu'à ce qu'aucun ne
soit `queued`/`in_progress`. Compter ~3-5 min pour les trois images.

**Piège vécu — rate limit API GitHub anonyme.** `api.github.com` limite les
requêtes non authentifiées à 60/heure. Une boucle de poll à 15-20s peut le
taper en quelques minutes (silencieusement : la réponse 403 fait juste
paraître le run "jamais fini" côté script de poll). Si ça arrive, attendre le
reset (`x-ratelimit-reset` dans les headers, ou juste patienter) et/ou espacer
le polling à 25-30s.

**Contournement qui n'est pas concerné par ce rate limit** — vérifier
directement la présence des images sur le registre, sans passer par
`api.github.com` :

```bash
for pkg in vanyline-controller vanyline-app vanyline-sandbox; do
  TOKEN=$(curl -s "https://ghcr.io/token?scope=repository:sebt3/$pkg:pull" \
    | python3 -c "import sys,json;print(json.load(sys.stdin)['token'])")
  curl -s -H "Authorization: Bearer $TOKEN" "https://ghcr.io/v2/sebt3/$pkg/tags/list"
done
```

## 5. Redéployer sur le cluster de test

```bash
kubectl apply -f deploy/controller/crds.yaml

sed 's/media-station/media-test/g' deploy/controller/controller.yaml > /tmp/controller-media-test.yaml
kubectl apply -f /tmp/controller-media-test.yaml
kubectl -n media-test rollout status deploy/vanyline-controller --timeout=90s
```

**Piège vécu — race avec l'ancien pod controller en terminaison.** Ne pas
enchaîner tout de suite sur le trigger du reconcile de l'Application : le
`rollout status` ci-dessus confirme que le *nouveau* pod tourne, mais
l'*ancien* peut rester en `Terminating` encore plusieurs dizaines de secondes
et effectuer un dernier reconcile avec son ancienne image/config avant de
mourir — ce dernier reconcile peut écraser ce que le nouveau pod vient de
poser. Attendre explicitement qu'il ne reste plus qu'un seul pod :

```bash
until [ "$(kubectl -n media-test get pods -l app=vanyline-controller --no-headers | wc -l)" = "1" ]; do sleep 3; done
```

Puis seulement, forcer le reconcile de l'Application (elle ne se reconcile pas
toute seule juste parce que le controller a redémarré avec une nouvelle image
par défaut — il faut un événement sur l'objet lui-même) :

```bash
kubectl -n media-test annotate application.vanyline.solidite.fr <name> reconcile-trigger="$(date +%s)" --overwrite
kubectl -n media-test rollout status deploy/application-<name> --timeout=90s
kubectl -n media-test get deploy application-<name> -o jsonpath='{.spec.template.spec.containers[0].image}{"\n"}'
```

Le même motif `kubectl annotate <resource> reconcile-trigger="$(date +%s)" --overwrite`
marche pour forcer le reconcile de n'importe quelle CR vanyline (Owner,
Project, Sandbox, Application) sans toucher à son spec — utile chaque fois
qu'un changement de *configuration du controller* (image, env) doit se
répercuter sur des objets existants sans attendre le prochain cycle naturel.

## 6. Limite structurelle à connaître : la création lazy ne se met jamais à jour

`ensure_owner` (côté `app`, `app/src/api/owners.rs`) ne crée un Owner
qu'une seule fois — si l'Owner existe déjà, ses champs ne sont **jamais**
resynchronisés, même après une release qui change les valeurs par défaut
posées à la création (`homeAccessMode`, `applicationRef`, etc.). Après une
release qui touche ces défauts, les Owner déjà créés dans l'environnement de
test doivent être patchés à la main :

```bash
kubectl -n media-test patch owner.vanyline.solidite.fr <name> --type merge \
  -p '{"spec":{"<champ>":"<valeur>"}}'
kubectl -n media-test annotate owner.vanyline.solidite.fr <name> reconcile-trigger="$(date +%s)" --overwrite
```

Même limite pour tout PVC déjà provisionné avec un `accessModes` devenu
incompatible (le champ est immuable une fois le PVC créé) : supprimer le pod
qui le référence, puis le PVC lui-même, avant de retrigger le reconcile du
parent (Owner/Project) qui le recrée avec la bonne valeur.

## 7. Champ requis à la création d'une Application : `ingressController`

Piège vécu (2026-08-14) : sans ce champ, l'ingress public de **toute**
sandbox de cette Application timeout en 504 sur **toute** requête (pas
spécifique au WebSocket) — le terminal et l'explorateur de fichiers
semblent "ne pas marcher" alors que le code frontend/sandbox est correct.

Cause : `build_sandbox_netpol` (`controller/src/sandbox.rs`) n'autorise
l'ingress vers le pod sandbox que depuis (1) les pods du même Owner et (2)
le pod `app` — jamais depuis le controller d'Ingress lui-même (Traefik),
sauf si `Application.spec.ingressController` est renseigné. Ce champ n'est
**pas auto-détecté** : il se règle manuellement à la création de
l'Application, exactement comme `ingressClassName`/`tlsIssuerName` — donc
à poser en même temps que ces deux-là, jamais après coup en réaction à un
symptôme :

```yaml
spec:
  ingressClassName: traefik
  tlsIssuerName: self-sign
  ingressController:
    namespace: kydah-core        # namespace des pods Traefik
    podLabels:
      app.kubernetes.io/name: traefik
```

Si le champ manque sur une Application déjà créée, le patcher et forcer le
reconcile de chaque Sandbox concernée (la NetworkPolicy sandbox n'est
recalculée qu'au reconcile de la Sandbox, pas automatiquement au patch de
l'Application) :

```bash
kubectl -n media-test patch application.vanyline.solidite.fr <name> --type merge \
  -p '{"spec":{"ingressController":{"namespace":"kydah-core","podLabels":{"app.kubernetes.io/name":"traefik"}}}}'
kubectl -n media-test annotate sandbox.vanyline.solidite.fr <name> reconcile-trigger="$(date +%s)" --overwrite
```
