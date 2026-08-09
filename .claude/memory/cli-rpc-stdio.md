# cli-rpc-stdio — serveur JSON-RPC 2.0 sur stdio (terminé)

`vanyline serve --stdio` (`cli/src/rpc/`) : `initialize`/`shutdown`,
`config/agents|models|toolsets|skills`, `conversations/list|get|create|
delete`, `chat/send` (asynchrone, spawné en tokio pour un vrai parallélisme
inter-conversations — un seul tour actif par conversation, `VNL-RPC-002`
sinon), `chat/cancel` (no-op v1). 110 tests cli au total. Détails
architecturaux : `docs/architecture.md` section "RPC stdio". Spec complète
du protocole (trames, codes d'erreur, piège camelCase/snake_case) :
`docs/rpc-protocol.md`.

**Piège de test découvert (ws07-review-fixes, 2026-07-31)** : `data_dir()`
(`cli/src/config.rs`, via `dirs::data_dir()`) résout `XDG_DATA_HOME` — un
état **global au process**, pas thread-local. `cli/src/rpc/handlers.rs`
a un mécanisme d'isolation dédié (`DATA_DIR_ENV_LOCK` + `isolated_data_dir()`,
juste avant `conversations_list_empty` dans le module `tests`) : **tout
test qui touche `store::` (get/save/delete_conversation) doit appeler
`let (_tmp, _guard) = isolated_data_dir();` en tout premier**, sinon il
est flaky sous `cargo test` parallèle (déterministe à l'échec en run
complet, systématiquement vert isolé ou en `--test-threads=1` — ne pas se
fier à un test lancé seul par son nom pour valider ce genre de fix).
Piège rencontré concrètement : un test préexistant qui ne vérifiait que
`busy`/codes d'erreur (jamais l'état persisté) n'avait jamais eu besoin de
ce mécanisme ; lui ajouter une assertion `store::get_conversation(...)`
l'a rendu flaky sans toucher à sa logique.

Bug réel trouvé en cours de route (pas dans le design, dans
l'implémentation) : `state` détenait un clone du sender mpsc et n'était
droppé qu'après avoir attendu la tâche writer — le process ne sortait
JAMAIS après `shutdown`/EOF (deadlock silencieux, seulement visible via un
test d'intégration qui spawn le vrai binaire, pas via des tests unitaires
sur `handle_line`). Confirme l'utilité d'au moins un test de bout en bout
par le process réel en plus des tests unitaires pour ce genre de code
(cycle de vie / shutdown), qui ne se voit pas en testant la logique pure.
