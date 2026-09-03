---
name: F4-vscode-ext-config-ui
description: F4 (close 2026-09-03, mergée+poussée) — panneau de configuration de l'extension VS Code vanyline
metadata:
  type: project
---

# F4 — vscode-ext-config-ui

**Quatrième des 5 features [[vscode-ext-sequence]]. Close 2026-09-03, branche
`feat/F4-vscode-ext-config-ui` mergée `--no-ff` dans `main` (`92d6b2e`) et poussée.
Reste F5 (TreeView sandboxes).** Dépend de [[F2-vscode-ext-cli-rpc]] (RPC write-side)
et [[F3-vscode-ext-chat]] (host + pont + webview).

## Ce que la feature ajoute

Un onglet éditeur `vanyline.config` (commande « vanyline : Ouvrir les paramètres »)
qui monte `ConfigShell` + les 6 écrans de `@vanyline/ui`, branchés sur la CLI
`vanyline serve --stdio` par RPC via le pont postMessage de F3. Le chat reste dans
la sidebar. Un write config fait dans le panneau se reflète dans le sélecteur
d'agent du chat **sans reload**.

## Décisions structurantes

- **Un seul bundle webview**, monté en chat ou en config selon
  `<meta name="vanyline-view">` que `buildHtml` émet (une webview VS Code n'a pas de
  `location.search`). `ext/webview/src/router.ts` `resolveView()` — repli sécuritaire
  sur `chat` pour toute valeur absente/inconnue. `main.ts` monte `App.vue` ou
  `ConfigView.vue`. Retenu contre deux entrypoints Vite pour ne pas dupliquer la
  chaîne / le poids (le tree-shaking ne sépare pas chat/config — bundle ~863 kB,
  accepté).
- **Panel unique réutilisé** (`ext/src/panels/config.ts`) : `open()` crée au premier
  appel, `reveal()` ensuite. `onDidDispose` remet `panel = undefined` ; le `handle`
  superviseur vit dans la closure du module, pas dans le panel (survit à une
  fermeture/réouverture). Aucun abonnement notification (contrairement à `chat.ts`) :
  `config/changed` naît des writes vus par `bridge.ts`, pas d'une notification CLI.
- **`rpcConfigRepo`** (`ext/webview/src/rpcConfigRepo.ts`) : impl `ConfigRepo` quasi
  pass-through sur le pont → RPC. Name-keyed nativement côté CLI → **aucune** résolution
  `name↔id` (plus simple que `httpConfigRepo`). **Seule traduction** : domaine UI
  `profiles`↔RPC `models`, `mcp`↔`mcpServers` (un seul point, testé explicitement —
  un bug ici écrirait silencieusement le mauvais domaine). `get('skills')` →
  `config/skills/get` ; `get(<autre>)` relit la liste et filtre (pas de lecture unité
  RPC par nom) ; après `create`/`update` (réponse `null`) l'entrée est relue serveur.
  `setDefaultProvider` → `VNL-EXT-024` (concept web-only). `source` retiré des payloads
  d'écriture explicitement. Jamais de `layer` (hérite du défaut F2).
- **2 ajouts CLI/RPC tranchés au démarrage** (2026-09-03, développeur — cf. design doc
  supprimé) :
  - **`config/skills/get`** (`{name}` → `{name, description, body, source}`) : seule
    méthode exposant le `body` d'un skill. Sans elle, `SkillsScreen.vue` (figé F1)
    appelle `repo.get('skills', name)` pour préremplir le body avant édition → l'édition
    d'un skill existant l'écraserait avec un body vide (**perte de données**). Le store
    lit déjà le body (`cfgstore fs_store load_skill`) ; l'ajout = handler RPC + smoke +
    doc. Nom inconnu → `VNL-RPC-006` (comme les 6 lectures ; `VNL-RPC-012` reste aux
    écritures) ; `name` absent → `VNL-RPC-000`.
  - **Champ additif `"source": "workspace" | "global"`** sur chaque entrée des 6
    lectures liste + `config/skills/get` : couche dont l'entrée est résolue (workspace
    gagne les collisions), calc `config_entry_source`/`file_entry_source`/
    `skill_entry_source` de `vanyline_cfgstore::layers` (déjà utilisés par `config
    check`). Additif pur : jamais sur le wire d'écriture, ignoré par les clients qui ne
    le connaissent pas (`app` inclus, pas de `deny_unknown_fields` nulle part). Mémé
    valeur que la colonne `source` de `vnl … list`. Miroité par `source?` optionnel
    dans `config-domain.ts`, affiché par `SourceBadge.vue` (`@vanyline/ui`) sur les 6
    écrans — jamais présent côté web → pas de badge sur le frontend.
- **`config/changed`** : `extension.ts` diffuse `{type:'config/changed', domain}` à
  **toutes** les webviews après tout write `config/<domain>/{create,update,delete}`
  réussi (détecté par regex dans `bridge.ts`, `onWriteSucceeded` → closure late-bound
  dans `extension.ts`). `domain` = **nom RPC brut** (`models`, `mcpServers`), pas le nom
  UI. Le chat (`App.vue`) refetch `config/agents` à chaque `config/changed` (sans
  filtrer). `ConfigView` ne s'y abonne **pas** — ses écrans refetch à `onMounted` (pas
  de `<KeepAlive>` dans `ConfigShell`) et après leurs propres writes (`useCrudResource`).
- **`RELAY_WHITELIST`** passe de 4 à 31 entrées (toute la surface `config/*`
  lecture/écriture/actions). Jamais `initialize`/`shutdown`/`chat/send`/`chat/cancel`/
  `conversations/delete`. Ordre du tableau gelé par le test de `bridge.spec.ts`.
  **Partagée par les deux webviews** — le chat hérite de la surface config write ;
  accepté car la CSP nonce interdit toute exécution JS dans la webview.

## Périmètre exclu (v1)

Pas de sélecteur de couche actif (writes → défaut F2 : workspace si résolu, sinon
global) ; pas de synchro avec les `settings.json` VS Code natifs
(`serverPath`/`autoUpdateCli`/`defaultLogLevel` restent dans Extension Settings) ;
pas d'éditeur texte brut du `config.yaml` ; le bouton « provider par défaut » des
écrans partagés est rejeté (`VNL-EXT-024`).

## Bilan de délégation — Cadence, 2ᵉ livraison propre consécutive

Livré par **Cadence sans escalade, 0 bug bloquant en review Phase 3** — comme F3,
contraste net avec git-integration/miryad-core. `cargo fmt` lancé (comme LSP/F3).
Cadence + `implement` désormais tous deux sur `dgx/qwen3.8-flash-next`
(`reasoningEffort: xhigh`, temp 1.0/top_p 0.95) — le couplage « même modèle →
validation croisée faible » noté en F2 persiste, mais les 2 dernières livraisons
sont sorties propres. Tests : 129 ext (host+webview) + 22 rpc_stdio_smoke + 88 ui +
20 protocol + 306 frontend, clippy `-D warnings` clean, fmt clean, builds OK.

## Review Phase 3 — findings (tous mineurs, 1 seul corrigé en code)

1. **`App.vue` — `agentsUnavailable` ne se rétablissait jamais** (corrigé, commit
   `7e5cbc2`) : le `.then` de `refreshAgents` repeuplait `agents.value` mais laissait
   le `<select>` `disabled` si le serveur avait été absent au mount. Ajout de
   `agentsUnavailable.value = false` dans le `.then`. Préexistant F3, mais F4 ajoute un
   nouveau déclencheur (`config/changed`) donc chemin de récup plus plausible.
2. **`setDefaultProvider` — bouton mort dans l'extension** (noté, pas corrigé) :
   `LlmProvidersScreen` rend « Défaut » sans condition → clic = `VNL-EXT-024` →
   ErrorCard. Tradeoff assumé (écrans figés F1, « rejet » plutôt que « masquer »).
   Documenté dans `docs/ext-install.md` § Pannes connues.
3. **Whitelist partagée chat↔config** (noté) : érosion defense-in-depth, non
   exploitable (CSP). Pourrait être paramétrée par `view` si besoin.
4. **`config_list_sourced_response` relit `config.yaml` par entrée** (noté) : O(N) I/O
   par appel liste. OK aux tailles réelles.
5. **`config/changed` porte le nom RPC pas le nom UI** (noté, doc) : sans impact
   (personne ne filtre), documenté dans `architecture.md`.

## Pièges techniques

- `entry["source"] = json!(…)` sur un `serde_json::Value` : sûr ici (les 6 domaines +
  `SkillMeta` sérialisent tous en objet) ; `IndexMut<&str>` panique si non-objet/non-null.
- `config_entry_source` re-parse `config.yaml` workspace pour chaque nom, mais
  `list_x()` aurait déjà erré si le merge ne parsait pas → à ce stade c'est propre,
  juste redondant.
- `SkillsScreen` body round-trip : `load_skill` trime, `config/skills/create|update`
  trime aussi → idempotent, pas d'oscillation de newline finale. C'était une décision
  design réfléchie, pas un hasard.

## Pas testé sur cluster réel

Comme toute la famille — pas de backend dans l'env de dev. La checklist e2e
`docs/ext-install.md` §4 (créer agent → `vnl agents list` → sélecteur chat sans
reload, édition body de skill sans perte) reste à dérouler par le développeur sur
code-server.
