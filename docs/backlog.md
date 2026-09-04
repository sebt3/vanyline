# Backlog

Idées et pistes pas encore scopées en feature — à piocher, détailler et trancher (y
compris le découpage en 1 ou plusieurs features) au moment de les sortir d'ici.

## Sélection des tools sandbox par toolset (pas géré aujourd'hui)

> **Partie 1 — refonte de l'interface `lsp_*` orientée agent — sortie du backlog le
> 2026-09-04** : design doc `docs/features/lsp-agent-interface.md`, branche
> `feat/lsp-agent-interface`. Décisions Phase 1 : périmètre = partie 1 seule ;
> `lsp_hover` retiré (absorbé par `lsp_definition`) ; positions par nom de symbole sur
> une ligne ; les 7 propositions dans la v1. La partie 2 ci-dessous **reste au
> backlog** (bloqueur UX non tranché).

Né d'un retour de test détaillé sur les tools `lsp_*` (agent DeepSeek, testé à la
main sur du Rust + TS réel), en creusant le point « je n'ai pas pu configurer les
tools, DeepSeek y avait quand même accès ».

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

---

## Support éditeur — autres langages

- **Vue** (CodeMirror + LSP) — recoupe la note « `.vue` non couvert » du design doc
  `docs/features/lsp-agent-interface.md` (Volar/vue-language-server) ; candidat naturel
  pour absorber ce point du backlog une fois attaqué.
- **Dockerfile** (coloration CodeMirror + toolchain à monter pour fournir linter/LSP —
  lequel reste à choisir, ex. hadolint côté lint, un langserver Dockerfile existe
  aussi côté LSP).
- **rhai et handlebars** — plugin CodeMirror à écrire (coloration syntaxique), pas de
  LSP en backend pour ces deux-là.

## Intégration Git dans l'IDE

- Explorer : colorer les fichiers modifiés et les nouveaux fichiers.
- Panel dédié (gauche) : état des changements en cours, commit avec message.
- Vue diff des fichiers dans une fenêtre type éditeur (groupe centre).
- Frontend graphique de "git log graph".

## Markdown viewer

## Auto-save

> Absorbé (2026-09-04) par la feature `docs/features/lsp-agent-interface.md` — l'édition
> LLM d'un fichier ouvert dans l'éditeur a besoin de l'autosave pour rafraîchir le
> buffer sans perte. Périmètre là-bas : écriture debouncée du buffer CodeMirror sur
> `/ws/fs`, rien de plus. Si un besoin autosave plus large émerge (historique, toggle
> global), rouvrir ici.

## Amélioration du chat LLM

- Fix du refresh en streaming.
- `tool_call` : affichage des paramètres et des résultats.
