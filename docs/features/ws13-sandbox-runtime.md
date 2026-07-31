# Feature — ws13-sandbox-runtime

## Ce que la feature fait

Trois consolidations du runtime sandbox : un set de commandes élargi dans
l'image de base, des NetworkPolicies egress déclaratives à trois niveaux
(Owner/Project/Sandbox), et l'arrêt/démarrage manuel d'une sandbox.

## Ce qu'elle ne fait pas

- Pas d'auto-arrêt sur inactivité (décision 2026-07-12 : manuel uniquement)
- Pas de netpol ingress nouvelle (celle par-Owner existante ne bouge pas)
- Pas de résolution DNS dans les règles egress (limite K8s : ipBlock/selectors,
  pas de FQDN — assumé, voir risques)

## 1. Commandes de l'image de base

Ajout au Dockerfile sandbox (décision 2026-07-12, "socle + python3") :
`ripgrep`, `fd-find`, `jq`, `procps`, `less`, `file`, `tree`, `patch`,
`diffutils`, `unzip`, `openssh-client`, `ca-certificates`, `python3` (~70 Mo).
Pas d'outils réseau/debug (dnsutils, netcat, strace) tant qu'un besoin réel ne
les réclame pas. Debian nomme le binaire fd `fdfind` et rg `rg` — symlink
`fd` → `fdfind` pour coller au réflexe des LLM.

## 2. NetworkPolicies egress à trois niveaux

Nouveau champ, même forme aux trois niveaux (`vanyline-crds`) :

```yaml
spec:
  egress:                      # white-list, absent = ne déclare rien
    - description: "registre npm interne"
      cidr: "10.42.7.0/24"     # ou
      podSelector: {...}       #    (exclusifs)
      namespaceSelector: {...} # optionnel, combine avec podSelector
      ports: [{port: 443, protocol: TCP}]
```

Règles de production (reconciler Sandbox) :

- **Aucun des trois niveaux ne déclare d'egress → aucune netpol egress produite**
  (la sandbox garde l'egress libre du namespace).
- Au moins une déclaration → une netpol egress sur le pod sandbox, avec
  l'**union** des règles Owner + Project + Sandbox, plus **toujours** :
  - DNS vers kube-dns (UDP/TCP 53) — sans quoi toute white-list est inutilisable
  - l'API server si le pod en a besoin (à confirmer : v1 non, le serveur sandbox
    n'appelle pas l'API K8s tant que TokenReview n'est pas actif)
- La netpol est reconciliée : modifier l'egress d'un Owner met à jour les
  netpols de toutes ses sandboxes (le reconciler Sandbox watch/requeue sur
  Owner et Project — mécanique déjà en place pour la résolution des refs).

Fonctions pures de construction (même style que `build_sandbox_netpol`
existant), testées sans cluster.

## 3. Arrêt/démarrage manuel

- `SandboxSpec.suspended: bool` (défaut false).
- `suspended: true` → le reconciler supprime le **pod** (worktree, PVC, service,
  netpols conservés) ; `status.phase: Suspended`.
- `suspended: false` → le pod est recréé (chemin nominal existant) ; le
  worktree étant conservé, aucun job checkout n'est relancé s'il existe déjà
  (idempotence déjà en place).
- Piloté par WS-12 (`vanyline sandbox stop|start` = patch du champ).
- Le coût conservé d'une sandbox suspendue = son worktree sur le PVC — c'est le
  compromis voulu (MR pas encore validée).

## Risques et questions ouvertes

- **Pas de FQDN dans les egress K8s** : ouvrir "git.kydah.fr" demande son IP/CIDR.
  Assumé v1 (les cibles internes ont des CIDR stables) ; si le besoin FQDN
  devient réel, ce sera un chantier CNI (CiliumNetworkPolicy…) — hors scope.
- L'ordre d'application : une netpol egress qui apparaît coupe tout le reste de
  l'egress du pod — la règle DNS systématique est LE point à ne pas rater
  (test dédié).
- `suspended` et le tour LLM en cours : supprimer le pod tue la session — v1
  assume (c'est un arrêt volontaire) ; documenter.

## Découpage en tâches candidates

1. `image-cmds` — paquets + symlink fd + build validé
2. `crds-egress` — champ `egress` aux trois niveaux + régénération CRDs
3. `netpol-builder` — fonctions pures union + DNS + tests
4. `netpol-reconcile` — application/mise à jour/suppression dans le reconciler,
   requeue sur Owner/Project
5. `suspended` — champ + logique reconciler + status + tests
