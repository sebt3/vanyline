# Feature — ws12-sandbox-clients

## Ce que la feature fait

Rend les Owners/Projects/Sandboxes pilotables hors du cluster-admin : la crate
lib fournit le client K8s typé, le CLI expose les commandes et les méthodes
JSON-RPC, et l'inférence CLI peut cibler une **toolbox** (une sandbox) dont les
tools MCP remplacent les tools locaux.

## Ce qu'elle ne fait pas

- Pas d'UI app/frontend pour ces objets (API/CLI d'abord ; l'app viendra quand
  l'intégration app ↔ sandbox sera à l'ordre du jour)
- Pas de gestion fine des droits : le kubeconfig/SA de l'appelant fait foi
- Pas de port-forward automatique : la toolbox suppose une URL MCP joignable
  (cas nominal : le CLI tourne dans le cluster, ex. pod code-server)

## Architecture — où vivent les types CRD

Les types Owner/Project/Sandbox vivent dans `controller/src/crds.rs` et le CLI
ne doit pas dépendre du crate controller (il embarquerait le runtime opérateur).
**Nouvelle crate feuille `crds/` (`vanyline-crds`)** : les structs de spec/status
+ derives kube, consommée par `controller` (qui ne garde que les reconcilers) et
par `lib`. C'est le même pattern additif que d'habitude : déplacement mécanique,
aucun changement de sémantique.

Dans `lib`, module `k8s` derrière une **feature `k8s`** (le CLI l'active, l'app
ne la paie pas tant qu'elle n'en a pas besoin) :

```rust
pub struct VnlK8sClient { /* kube::Client + namespace */ }
impl VnlK8sClient {
    pub async fn list_owners(&self) -> Result<Vec<Owner>, VnyError>;
    pub async fn get/create/delete_owner(...);
    // idem projects, sandboxes
    pub async fn set_sandbox_suspended(&self, name: &str, suspended: bool) -> ...;  // WS-13
    pub async fn sandbox_mcp_url(&self, name: &str) -> Result<String, VnyError>;
    // → http://sandbox-<name>.<ns>.svc:3000/mcp (service posé par le controller)
}
```

## CLI

Commandes (mêmes conventions que l'existant, sortie tabulaire) :

```
vanyline owner    list|show <name>|create ...|delete <name>
vanyline project  list|show <name>|create ...|delete <name>
vanyline sandbox  list|show <name>|create ...|delete <name>|stop <name>|start <name>
```

Méthodes JSON-RPC miroir : `owners/list|get|create|delete`,
`projects/...`, `sandboxes/...` (+ `sandboxes/stop|start`) — mêmes règles que
les méthodes existantes (erreurs `VNL-RPC-xxx`, camelCase sur le fil, cf.
`docs/rpc-protocol.md` à étendre).

Le namespace vient du contexte kubeconfig, surchargable (`--namespace`, et
`defaults.namespace` dans config.yaml). Sans kubeconfig joignable : erreur
propre `VNL-K8S-001`, les commandes non-K8s continuent de fonctionner.

## Toolbox en inférence

`vanyline run --toolbox <sandbox> ...` (+ `defaults.toolbox`) :

- Résout l'URL MCP de la sandbox via `VnlK8sClient::sandbox_mcp_url`
- Construit le `SessionContext` avec `local_tools` **vide** et la sandbox
  injectée comme serveur MCP du tour — réutilise la mécanique
  `connect_mcp_servers_selected` existante (la sandbox est un serveur MCP
  http-streamable comme un autre, sans passer par la config `mcp:` de
  l'utilisateur)
- Les toolsets de l'agent continuent de s'appliquer pour les serveurs MCP
  *additionnels* ; seule la partie `local_tools` est remplacée
- Les builtins `skill`/`task` ne changent pas

## Risques et questions ouvertes

- **Point d'API à figer tôt** : comment "remplacer les local tools par la
  sandbox" s'exprime dans `SessionContext` sans câblage spécial — proposition :
  l'hôte (CLI) le fait en amont (local_tools vide + serveur MCP forcé), la lib
  ne change pas. À valider à la première tâche.
- Joignabilité de l'URL MCP depuis un CLI hors cluster : hors scope v1,
  documenté (un port-forward manuel fonctionne déjà).
- Auth : les sandboxes tournent en `--no-auth` derrière netpol — le CLI dans le
  cluster passe si ses labels/namespace le permettent. L'activation du mode SA
  TokenReview reste un chantier sandbox ultérieur (P2 du phasage).
- `create` en CLI : quels champs en flags vs fichier YAML ? Proposition v1 :
  `create -f fichier.yaml` (apply-like) + flags minimaux pour sandbox
  (`--project`, `--branch`).

## Découpage en tâches candidates

1. `crate-crds` — extraction `vanyline-crds`, controller migré (mécanique)
2. `lib-k8s` — `VnlK8sClient` + feature `k8s` + tests (mock/enregistrements)
3. `cli-commands` — owner/project/sandbox list/show/create/delete
4. `rpc-methods` — méthodes JSON-RPC miroir + doc protocole
5. `toolbox` — `--toolbox` en inférence (résolution URL + câblage session)
6. `stop-start` — commandes + RPC (le champ de spec arrive par WS-13 ;
   cette tâche se séquence après)
