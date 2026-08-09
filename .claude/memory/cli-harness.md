# cli-harness — configuration YAML deux couches (terminé)

Le CLI a son vrai stockage natif : `FsConfigStore` (`cli/src/fs_store.rs`) implémente
`ConfigStore` sur deux couches YAML — globale (`~/.config/vanyline/`) et workspace
(`<racine>/.vanyline/`, découverte en remontant jusqu'à `.vanyline/` ou `.git/`).
`config.yaml` (providers/models/mcp/defaults) fusionne clé par clé ; `agents/<name>.md`
(frontmatter + corps = system prompt), `toolsets/<name>.yaml`, `skills/<name>/SKILL.md`
fusionnent par nom de fichier (workspace remplace intégralement l'homonyme global).
Toutes les commandes CLI (`run`/REPL, `agents|models|toolsets|skills|providers|mcp list`,
`agents show`, `config check`) tournent dessus ; l'ancien `CliConfigStore` (JSON) est
supprimé — rupture assumée, pas de migration automatique. Les conversations ont quitté
`~/.config` pour `~/.local/share/vanyline/` (XDG data) et se référencent par index de
liste ou préfixe d'UUID, plus par UUID complet obligatoire. Détails : `docs/architecture.md`
(section "Configuration CLI").

Dépendance `yaml_serde` (fork maintenu de `serde_yaml`, devenu archivé/non maintenu —
vérifié activement avant de choisir, ne pas repartir de `serde_yaml` par réflexe).

Stratégie : même pattern que harness-core (additif jusqu'à un cutover mécanique final),
mais découpé plus finement que prévu par le design initial — chaque tâche candidate du
design (`fs-store`, `commands`) s'est révélée trop large pour la règle des 30-45 min et a
été éclatée en sous-tâches (02a/02b/02c, 04a/04b/04c/04d) au fil de l'implémentation, pas
anticipées à l'avance. Fonctionne bien : découper *pendant* l'exécution dès qu'une tâche
candidate touche plusieurs formats/fichiers indépendants, plutôt que de figer le découpage
dans le design doc.
