# Feature — ws12-sandbox-clients

## Statut

Terminé et clos dans `docs/architecture.md` (section "Client K8s CLI —
`vanyline-crds`, `VnlK8sClient`, toolbox") : extraction `vanyline-crds`,
`VnlK8sClient` (feature `k8s` de `vanyline-lib`), commandes CLI
`owner`/`project`/`sandbox` (`list`/`show`/`create`/`delete`), méthodes
JSON-RPC miroir (`owners/*`/`projects/*`/`sandboxes/*`, cf.
`docs/rpc-protocol.md`), toolbox en inférence (`--toolbox`,
`SessionContext.extra_mcp`). Ce fichier ne couvre plus que ce qui reste —
`stop`/`start`, jamais commencé.

## Ce qui reste à faire

**`stop-start`** — `vanyline sandbox stop|start <name>` (+ méthodes RPC
`sandboxes/stop|start`). **Bloqué** : nécessite un champ `suspended` (ou
équivalent) sur `SandboxSpec`, qui n'existe pas encore — ce champ et la
logique de suspension côté controller (arrêt/redémarrage du pod sans
supprimer la ressource) sont le périmètre d'une feature à part entière,
**WS-13**, pas encore démarrée. Cette tâche se séquence après WS-13.

Une fois WS-13 en place, le reste est mécanique et suit exactement le
même patron que les commandes `create`/`delete` déjà en place :
- `VnlK8sClient::set_sandbox_suspended(&self, name: &str, suspended: bool)`
  (patch du champ ajouté par WS-13)
- CLI : `vanyline sandbox stop <name>` / `start <name>`
- RPC : `sandboxes/stop`/`sandboxes/start`, params `{name}`, erreurs
  `VNL-RPC-010` (même code que le reste des méthodes K8s)

## Ce qu'elle ne fait pas

- Pas d'UI app/frontend pour ces objets (API/CLI d'abord ; l'app viendra
  quand l'intégration app ↔ sandbox sera à l'ordre du jour)
- Pas de gestion fine des droits : le kubeconfig/SA de l'appelant fait foi
- Pas de port-forward automatique : la toolbox suppose une URL MCP
  joignable (cas nominal : le CLI tourne dans le cluster)

## Risques et questions ouvertes

Aucun risque restant sur le périmètre clos. Pour `stop-start` : le
principal point à trancher au démarrage de WS-13 sera la sémantique exacte
de "suspendu" côté controller (scale-down du pod à 0 replica n'existe pas
ici — un Sandbox est un Pod nu, pas un Deployment ; probablement
suppression du Pod en gardant le reste de la ressource, à recréer au
`start`) — à documenter dans le design doc de WS-13, pas ici.
