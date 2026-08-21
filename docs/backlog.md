# Backlog — interface LSP orientée agent + sélection des tools sandbox

Née d'un retour de test détaillé sur les 5 tools `lsp_*` (agent DeepSeek, testés à la
main sur du Rust + TS réel) et d'une conversation de suivi sur l'évolution voulue.
**Deux fils distincts**, regroupés ici parce qu'ils sont nés de la même session, mais
séparables — à trancher (1 ou 2 features) au moment de les sortir du backlog.

Les correctifs déjà livrés le jour même (hors périmètre ici, pas à refaire) :
`lsp_hover` cassé (mauvais parsing de `Hover.contents`), `lsp_diagnostics` one-shot
(didOpen redondant + cache non partagé), trois états diagnostics (propre / pas encore
analysé / pas de LSP). Cf. `.claude/memory/lsp-integration.md` pour le détail.

---

## Partie 1 — Repenser l'interface `lsp_*` pour un agent, pas un IDE

### Ce que ça fait (une fois construit)

Remplace une collection de tools qui imitent des features d'IDE (hover, goto-def,
rename bruts) par une interface orientée boucle agent : après une édition, savoir vite
ce qui casse et qui est impacté — plutôt que des coordonnées à re-résoudre soi-même.

### Ce que ça ne fait pas

- Ne redesigne pas le protocole LSP lui-même ni la session partagée (process unique par
  toolchain, cache diagnostics/initialize) — cette couche reste telle quelle.
- Pas de support `.vue`/Volar ici — feature séparée, confirmée par le développeur
  (`.vue` non couvert est attendu, pas un défaut de cette feature).
- Ne touche pas à la sélection des tools par toolset (partie 2 ci-dessous).

### Faits vérifiés à concevoir avec (pas des suppositions)

- **`textDocument/diagnostic` (pull) n'est PAS uniforme** : rust-analyzer le supporte
  par fichier, mais annonce `"workspaceDiagnostics": false` dans ses capabilities —
  pas de pull workspace-wide en un appel. `typescript-language-server` ne le supporte
  pas du tout (`"Unhandled method"`, vérifié en direct sur le cluster). Toute
  proposition "diagnostics workspace en un appel via pull" doit être écartée ou
  reposer sur une itération côté sandbox par fichier suivi (cache déjà en place), pas
  sur une requête pull unique.
- `vanyline-tools` est framework-agnostic par règle de dépendance du monorepo (cf.
  `docs/architecture.md`, "Règles de dépendances" #3) — `write_file`/`edit_file`
  eux-mêmes ne doivent pas devenir LSP-aware. `edit_and_check` (ci-dessous) doit vivre
  côté sandbox (`tools_impl.rs`, qui connaît déjà et LSP et les tools filesystem), pas
  modifier les tools de base.

### Propositions retenues pour l'ébauche, par priorité décroissante

1. **`edit_and_check`** — applique une édition (réutilise `write_file`/`edit_file` de
   `vanyline-tools` en interne) puis attend/lit les diagnostics du fichier touché
   (cache déjà en place) et rend un **diff** : diagnostics apparus vs déjà présents
   avant l'édition — c'est ça qui répond à "est-ce que mon edit casse quelque chose",
   pas la liste brute du fichier. Le plus gros morceau : nécessite de capturer un état
   AVANT l'édition (diagnostics déjà en cache ou une attente courte si jamais
   analysés), appliquer, puis attendre une ré-analyse (latence rust-analyzer variable
   après un `didChange` — borner, et prévoir un état "pas encore ré-analysé, redemande
   dans Xms" plutôt que de bloquer indéfiniment).
2. **Carte du code — deux nouveaux tools** : `textDocument/documentSymbol` (outline
   d'un fichier : fonctions/structs/signatures en un appel, remplace N `read_file`) et
   `workspace/symbol` (recherche globale : "où est `AuthState` ?" en une requête).
   Standard LSP, vraisemblablement supporté par les deux serveurs déjà en place — à
   vérifier en implémentant, pas supposé.
3. **`lsp_references` enrichi** : pour chaque référence, la fonction englobante +
   sa signature, pas juste `fichier:ligne:colonne`. Faisable sans N+1 : un
   `documentSymbol` par FICHIER touché par les références (pas par référence),
   matché localement contre la position de chaque référence.
4. **`lsp_definition` absorbe le hover** : position + signature + doc courte en une
   réponse. `lsp_hover` retiré comme tool autonome. **Nécessite l'accord explicite du
   développeur avant implémentation** — `lsp_hover` vient d'être corrigé et livré ce
   jour même, le retirer n'est pas anodin.
5. **Rename preview + rapport de diff** : un mode qui calcule le `WorkspaceEdit` sans
   l'appliquer (rendre la liste des sites qui seraient touchés), et après application,
   un rapport ancien→nouveau par fichier — extension du code déjà là
   (`apply_workspace_edit` calcule déjà le WorkspaceEdit avant d'écrire).
6. **Ergonomie des positions — contre-proposition, pas la suggestion telle quelle** :
   plutôt que d'aligner sur le 1-based de `read_file` (ou accepter les deux, qui
   complexifie l'API), permettre de cibler un **nom de symbole sur une ligne** plutôt
   qu'un offset de caractère exact — le tool résout lui-même la position de
   l'identifiant sur cette ligne. Supprime à la fois le piège 0/1-based ET
   l'ambiguïté "position sur un espace résout vers le voisin" observée dans le
   rapport — une correction de base seule ne réglait que la première moitié du
   problème. Garder les coordonnées en option pour un usage précis. Snippet (texte de
   la ligne) inclus dans chaque résultat def/ref/rename, indépendamment du mode
   position choisi.
7. **`inspect_symbol`** : combine definition + references + signature en un seul
   appel — pure composition des tools ci-dessus, pas de nouvelle surface LSP.

### Risques et questions ouvertes

- `edit_and_check` est le seul morceau qui touche vraiment une autre couche
  (filesystem tools) — mérite sa propre validation de conception (comment borner
  l'attente de ré-analyse) avant une tâche d'implémentation.
- Vérifier `documentSymbol`/`workspace/symbol` sont bien supportés par
  typescript-language-server ET rust-analyzer avant de s'engager sur le point 2 —
  pas vérifié à ce jour, contrairement au pull diagnostics.
- Retirer `lsp_hover` change une interface tool déjà publiée (même si récente) —
  décision produit, pas juste technique.

---

## Partie 2 — Sélection des tools sandbox par toolset (pas géré aujourd'hui)

### Constat vérifié (pas une hypothèse)

Trouvé en creusant le retour "je n'ai pas pu configurer les tools, DeepSeek y avait
quand même accès" : **ce n'est pas spécifique à `lsp_*`**. `resolve_extra_mcp`
(`app/src/ws/chat.rs:307-313`) pose systématiquement :

```rust
McpSelection { server: "sandbox".to_string(), tools: vec![] }
```

`tools: vec![]` = wildcard, tous les tools passent (`tool_matches(&[], _) == true`,
vérifié). **Toute la surface de tools de la sandbox** (filesystem/search/command,
maintenant aussi `lsp_*`) contourne entièrement le mécanisme `Toolset.mcp[]` qui
filtre normalement les autres serveurs MCP (Grafana, playwright, etc.) par glob sur
`McpSelection.tools`. Ce n'est pas un bug introduit par `lsp-integration` — c'est un
choix (probablement involontaire, jamais revisité) de `chat-app-fonctionnel`
(2026-08-18). Les nouveaux tools `lsp_*` rendent juste la surface toujours-exposée
plus grande, donc le manque de contrôle plus visible.

### Pourquoi ce n'est pas un simple retrait de `tools: vec![]`

Le mécanisme `Toolset.mcp[].tools` existant suppose un serveur MCP **statique**,
enregistré en base (`mcp_servers`), avec un flux "tester la connexion" qui peuple
`available_tools` (`app/src/api/mcp_servers.rs::test_server`) pour construire une UI
de sélection. La sandbox n'est **pas** un tel serveur : son URL est résolue
dynamiquement par contexte de conversation (laquelle sandbox), il n'y a nulle part où
stocker un `available_tools` figé pour elle.

### Questions ouvertes pour la conception

- Réutiliser `Toolset.mcp[].tools` (même glob) pour la sandbox, ou un mécanisme
  dédié ? Si réutilisé : d'où vient la liste "tools disponibles" pour l'UI de
  sélection, vu qu'il n'y a pas de ligne `mcp_servers` à tester ? (Piste : la liste des
  tools sandbox est en réalité statique côté code — `filesystem_tools() +
  search_tools() + command_tools() + lsp_tools()` — pourrait être exposée par un
  endpoint dédié plutôt que le flux "test" générique.)
- Granularité voulue : tout ou rien (comme aujourd'hui) vs par catégorie
  (filesystem/search/command/lsp) vs par tool individuel ?
- Est-ce que ça doit bloquer sur une décision UX (où ça se configure : par Agent, par
  Toolset, par conversation ?) avant tout code.

### Ce que ça ne fait pas (pour l'instant)

Ne change rien au comportement actuel (tout exposé) tant que la conception n'est pas
tranchée — ce document ne fait qu'acter le constat et les questions, pas une décision
de design.
