# ws09-sandbox-maint-agent — vanyline-maint (terminé)

Binaire `vanyline-maint` dans l'image sandbox (`sandbox/src/maint.rs` + wrapper
clap `sandbox/src/bin/maint.rs`) : `init`/`fetch`/`purge`/`checkout`/`remove` +
stub `detect` (WS-10). Les 5 Jobs git du controller invoquent ce binaire en
argv — plus aucun `sh -c` dans `controller/` (R1 clos), presets toolchain deux
arches (R2). Erreurs `VNL-MAINT-001..005`. Détails : `docs/architecture.md`
section "Maintenance des workspaces". 4 tâches (3 Qwen + docs par Claude),
506 tests au total en fin de feature (474 au départ).

Leçons de délégation spécifiques (complètent `outillage-llm-exec.md`) :

- **Les apostrophes françaises dans un message de commit donné verbatim font
  planter Qwen en boucle sur le quoting bash** (tâche 1 : 3 tentatives
  échouées puis crash de session sur un `Write /tmp/...` auto-rejeté — /tmp
  est hors whitelist `external_directory`). Parade qui marche, appliquée dès
  la tâche 2 : message **sans apostrophes/accents** + procédure imposée dans
  la section Commit du fichier de tâche : écrire le message dans
  `.tasks/commit-msg.txt` (chemin DANS le repo), `git commit -F`, supprimer
  le fichier. Zéro échec ensuite.
- **Vérifier le préfixe du message même avec la procédure -F** : tâche 3
  committée avec `(featur: ...)` au lieu de `(feat: ...)` — typo de Qwen dans
  le fichier de message, corrigée par `git commit --amend` (sûr : rien n'est
  poussé avant la clôture).
- Le compte de tests annoncé par Qwen était encore une fois faux/périmé
  (tâche 2 : "493" annoncés, 505 réels) — la règle "toujours recompter
  soi-même" reste valable.
