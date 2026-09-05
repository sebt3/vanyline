# Feature — Interface `lsp_*` orientée agent

Extraite du backlog (`docs/backlog.md`, section « LSP orientée agent + sélection des
tools sandbox », **partie 1 seulement** — la partie 2, sélection des tools sandbox par
toolset, reste au backlog : bloqueur UX non tranché, hors périmètre ici).

Décisions produit prises en Phase 1 avec le développeur (2026-09-04) :
- **Périmètre** : partie 1 seule.
- **`lsp_hover` retiré** comme tool autonome — absorbé par `lsp_definition`.
- **Positions** : cibler un nom de symbole sur une ligne (proposition 6), coordonnées
  gardées en option.
- **Découpage** : les 7 propositions du backlog dans la v1 (pas de coupe).
- **`edit_and_check` ne bloque jamais le LLM** même si un humain a le fichier ouvert
  dans l'éditeur ; l'éditeur est rafraîchi après l'édition, et **l'autosave de
  l'éditeur entre dans le périmètre de cette feature** (absorbe l'item `## Auto-save`
  du backlog) pour rendre ce rafraîchissement sans perte.

---

## Ce que la feature fait (une phrase)

Remplace la collection de tools `lsp_*` qui imitent des gestes d'IDE (hover,
goto-def, rename bruts, positions 0-based à re-résoudre) par une interface orientée
boucle agent : après une édition, savoir vite ce qui casse et qui est impacté, avec
la fonction englobante et sa signature plutôt que des coordonnées `fichier:ligne:col`.
Amène au passage l'autosave de l'éditeur, prérequis d'une édition LLM propre sur un
fichier ouvert.

## Ce qu'elle ne fait pas (périmètre explicite)

- **Ne touche pas au protocole LSP ni à la session partagée** (`sandbox/src/lsp.rs` :
  process unique par toolchain, multiplexage clients, cache `initialize` /
  `diagnostics`). Cette couche reste telle quelle — **sauf** l'ajout strictement
  additif de `didChange` / version par URI requis par `edit_and_check` (voir
  « Interfaces », le reste du manager est inchangé).
- **Ne touche pas à `vanyline-tools`** — `write_file` / `edit_file` restent
  framework-agnostic (règle de dépendance monorepo #3, `docs/architecture.md`). Toute
  logique LSP-aware vit côté sandbox (`sandbox/src/tools_impl.rs`), qui connaît déjà
  et le LSP et les tools filesystem.
- **Pas de support `.vue` / Volar** — feature séparée, confirmée par le développeur
  (`.vue` non couvert est attendu). `toolchain_for_path` reste rust / node.
- **Pas de diagnostics workspace-wide en un appel** — fait vérifié en direct sur le
  cluster : rust-analyzer annonce `workspaceDiagnostics: false`,
  typescript-language-server ne supporte pas `textDocument/diagnostic` du tout. Toute
  agrégation multi-fichiers passe par une itération côté sandbox sur les fichiers
  suivis (cache push déjà en place), jamais par une requête pull unique.
- **Ne change pas** le mécanisme de sélection des tools par toolset (partie 2 du
  backlog) — tous les tools `lsp_*` restent exposés en wildcard comme aujourd'hui.
- **Pas de nouvel endpoint sur `/ws/fs`** ni de contrat atomique/rollback — `rename`
  et `edit_and_check` restent best-effort séquentiel, même contrat que
  `apply_workspace_edit` existant. `edit_and_check` **ajoute** un message sortant sur
  `/ws/fs` (« fichier changé sur disque » → l'éditeur recharge), pas une nouvelle
  route.
- **Autosave** : l'éditeur gagne l'autosave (débounce → `/ws/fs`), mais **rien de
  plus** — pas d'historique de versions, pas de résolution de conflit à trois voies,
  pas de toggle par fichier au-delà de ce que R1 sous-question 4 tranche. C'est le
  strict nécessaire pour que le reload de buffer du cas B ne perde aucune frappe.
  Cette feature absorbe l'item `## Auto-save` du backlog.

---

## Inventaire des tools après la feature

| Tool | État | Change |
|---|---|---|
| `lsp_diagnostics` | conservé | inchangé (déjà 3 états : propre / pas encore analysé / pas de LSP) |
| `lsp_hover` | **retiré** | absorbé par `lsp_definition` |
| `lsp_definition` | modifié | rend position(s) **+ signature + doc courte + snippet de ligne** ; nouveau modèle de position |
| `lsp_references` | modifié | groupé par fichier, chaque réf avec **symbole englobant + signature** ; snippet ; nouveau modèle de position |
| `lsp_rename` | modifié | nouveau param `preview` (calcule sans appliquer) ; rapport ancien→nouveau par fichier ; nouveau modèle de position |
| `lsp_document_symbols` | **nouveau** | outline d'un fichier (`textDocument/documentSymbol`) |
| `lsp_workspace_symbols` | **nouveau** | recherche globale de symbole (`workspace/symbol`) |
| `inspect_symbol` | **nouveau** | composition definition + references + signature en un appel |
| `edit_and_check` | **nouveau** | applique une édition puis rend le **diff de diagnostics** (apparus vs déjà présents) |

Net : 5 tools → 8 tools (retrait de 1, ajout de 4).

---

## Interfaces clés et modules touchés

Modules touchés :
- `sandbox/src/tools_impl.rs` — dispatch et schémas des tools `lsp_*`, `resolve_position`,
  `dispatch_edit_and_check` (§1-7).
- `sandbox/src/lsp.rs` / `lsp_client.rs` — ajout additif `did_change` / `doc_versions` /
  `invalidate_diagnostics` + tracking des URIs ouvertes par un client navigateur (§7).
- `sandbox/src/ws/lsp.rs` — le tracking « ouverte par le navigateur » s'accroche au
  `subscribe`/`unsubscribe` du bridge (§7, R1 sous-q. 1).
- `sandbox/src/ws/fs.rs` (ou équivalent) — type de frame sortant « fichier changé sur
  disque » (§8).
- `frontend/` éditeur CodeMirror — autosave debouncé + reload de buffer sur
  notification (§8).
- `docs/architecture.md` — à la clôture (§ « Serveur LSP », § éditeur).

Aucune modification de `vanyline-tools`, du controller, de `app/`, des CRDs.

### 1. Modèle de position partagé (proposition 6)

Tous les tools qui prennent une position (`lsp_definition`, `lsp_references`,
`lsp_rename`, `inspect_symbol`) partagent une forme d'argument unique. Remplace
`LspPositionArgs { path, line: u64 /*0-based*/, character: u64 /*0-based*/ }`.

```rust
// sandbox/src/tools_impl.rs
#[derive(serde::Deserialize, Clone)]
pub struct LspSymbolTarget {
    pub path: String,
    /// Ligne 1-based (comme read_file). Requis.
    pub line: u64,
    /// Nom de l'identifiant à cibler sur cette ligne. Mode recommandé.
    /// Le tool résout lui-même la colonne du 1er match de ce nom sur la ligne.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Colonne 1-based, échappatoire pour un ciblage précis quand `symbol`
    /// est ambigu ou absent. Ignoré si `symbol` est fourni.
    #[serde(default)]
    pub character: Option<u64>,
}
```

Résolution (`resolve_position`, nouvelle fonction pure dans `tools_impl.rs`) :
- `symbol` fourni → lire la ligne `line` du fichier, trouver l'occurrence du nom en
  tant qu'**identifiant délimité** (pas une sous-chaîne — bordures `\b` sur
  `[A-Za-z0-9_]`), rendre `(line-1, col_start)` en 0-based LSP. Aucun match →
  `VNL-SBX-LSP-007`. Plusieurs matchs → premier, et le noter dans la réponse.
- `character` fourni (et pas `symbol`) → `(line-1, character-1)`.
- ni l'un ni l'autre → colonne 0 (début de ligne), comportement conservé.

Chaque résultat def/ref/rename inclut le **texte de la ligne** ciblée et de chaque
ligne résultat (snippet), indépendamment du mode.

> Point à valider avec le développeur : `line` passe en **1-based** (aligné
> `read_file`), `character` en **1-based** aussi pour cohérence — au prix d'une
> conversion interne. L'alternative (garder `character` 0-based LSP natif) est plus
> proche du protocole mais réintroduit le footgun mixte que la proposition 6 vise à
> supprimer. Recommandation : tout 1-based en entrée.

### 2. `lsp_definition` absorbe `lsp_hover`

`textDocument/definition` **+** `textDocument/hover` sur la même position, une seule
réponse. `lsp_hover` retiré de `lsp_tools()`, de `dispatch_lsp`, des tests.

Réponse (texte, pas JSON — cohérent avec l'existant) :
```
<path>:<line>: <snippet de la ligne ciblée>
signature: <hover.contents, 1re section de code>
doc: <hover.contents, prose, tronquée à ~3 lignes>
défini à:
  <uri relatif>:<line>: <snippet>
  ...
```
`hover_contents_to_text` (déjà là, gère `MarkedString | []  | MarkupContent`) réutilisé
tel quel. Pas de def trouvée → on rend quand même hover si présent.

### 3. `lsp_references` enrichi

Pour chaque référence : fichier, ligne, **symbole englobant + sa signature**, snippet.
Sans N+1 : **un** `textDocument/documentSymbol` par fichier distinct touché par les
références (pas par référence), puis matché localement — la position de chaque réf est
comparée aux `range` des symboles du fichier pour trouver l'englobant le plus profond.

```
<uri relatif>
  dans fn <nom>(<signature>)  —  L<line>
    L<line>: <snippet>
    L<line>: <snippet>
  dans impl <T> / <nom>  —  L<line>
    L<line>: <snippet>
```

`documentSymbol` rend soit `DocumentSymbol[]` (hiérarchique, avec `range`) soit
`SymbolInformation[]` (plat, avec `location`). Vérification R2 cluster (2026-09-04) : les
DEUX serveurs rendent la forme plate — son `range` ne couvre que le **nom** du symbole,
l'englobant « le plus profond » par containment est donc impossible sur la forme réelle
(l'hypothèse initiale « rust-analyzer rend le premier » est réfutée). Arbitrage
développeur 2026-09-04 : sur forme plate, englobant = dernier symbole du même fichier
démarrant à ou avant la réf (ordre document), signature = snippet de la ligne de
l'englobant (fichier déjà lu pour son `didOpen`) ; sur forme hiérarchique, algorithme
exact prévu ci-dessus (containment profond + `detail`). Toujours **un seul**
`documentSymbol` par fichier distinct, zéro requête LSP supplémentaire par référence.

### 4. Carte du code — deux tools (proposition 2)

- `lsp_document_symbols { path }` → `textDocument/documentSymbol`, rendu en arbre
  indenté : `kind` + nom + signature + `L<line>`. Remplace N `read_file` pour
  comprendre la structure d'un fichier.
- `lsp_workspace_symbols { query, path? }` → `workspace/symbol`, rendu plat :
  `<uri relatif>:<line>: <kind> <nom>`. « Où est `AuthState` ? » en une requête.
  `path` optionnel — arbitrage développeur 2026-09-04 : sert uniquement de **choix de
  toolchain** (`toolchain_for_path`, jamais ouvert/lisi) ; sans lui, première toolchain
  LSP configurée dans l'ordre `rust`, `node`.

> Risque : support `documentSymbol` / `workspace/symbol` par
> typescript-language-server **et** rust-analyzer **non vérifié à ce jour** (contrairement
> au pull diagnostics qui, lui, a été testé). Ce sont des méthodes LSP standard et de
> base — support attendu — mais à confirmer en implémentant (task dédiée), pas supposé.
> Si `workspace/symbol` manque quelque part : dégrader vers un message clair, pas un
> fallback ripgrep déguisé en LSP.

### 5. `lsp_rename` — mode preview + rapport de diff (proposition 5)

Nouveau param `#[serde(default)] preview: bool` sur `LspRenameArgs` (qui adopte aussi
`LspSymbolTarget` pour la position).
- `preview: true` → calcule le `WorkspaceEdit` (`textDocument/rename`), **n'applique
  pas**, rend la liste des sites : `<uri relatif>:<line>:<col>: <snippet>` groupés par
  fichier, plus un total.
- `preview: false` (défaut, comportement actuel) → applique via `apply_workspace_edit`
  (déjà là), puis rend un **rapport par fichier** : nb d'occurrences remplacées +
  snippet avant→après de chaque site. Le `WorkspaceEdit` est déjà calculé avant
  écriture dans `apply_workspace_edit` — le rapport est une extension de ce point, pas
  un second aller-retour LSP.

### 6. `inspect_symbol` (proposition 7)

Pure composition, **aucune nouvelle surface LSP** : `LspSymbolTarget` en entrée →
appelle en interne la logique de `lsp_definition` (def + hover) **et** de
`lsp_references` enrichi, agrège en une réponse. Une seule section « signature », puis
« défini à », puis « références (N) » groupées par fichier avec symbole englobant.

### 7. `edit_and_check` (proposition 1 — le gros morceau)

Vit dans `sandbox/src/tools_impl.rs` (nouveau `dispatch_edit_and_check` ou branche de
`dispatch_lsp`). **Réutilise `vanyline_tools::filesystem::{write_file, edit_file}` en
interne** — ne les modifie pas, ne les rend pas LSP-aware.

Argument : union discriminée entre un `write_file` complet et un `edit_file`
(remplacement de chaîne), plus le `path`. Sortie : **diff de diagnostics**.

```
édition appliquée: <path> (<write|edit>, <n> octets / <n> remplacements)

diagnostics APPARUS (<n>):
  <path>:<line>:<col>: error: <message>
diagnostics DISPARUS (<n>):
  <path>:<line>:<col>: warning: <message>
diagnostics INCHANGÉS (<n>) — non listés

[ou:]  ré-analyse pas encore stabilisée après <timeout> — relance edit_and_check
       ou lsp_diagnostics dans quelques secondes (VNL-SBX-LSP-008).
       L'édition EST appliquée sur le disque.
```

Séquence de messages requise (à dérouler concrètement — leçon `lsp-integration`) :

1. **Capturer l'état AVANT** : `cached_diagnostics(uri)` si présent ; sinon
   `wait_for_diagnostics(uri, court)` — borné court (fichier peut-être jamais analysé,
   ne pas bloquer). État `None` (jamais analysé) est légitime : le « avant » est alors
   l'ensemble vide, à signaler dans la réponse (« aucune analyse préalable — le diff
   est calculé contre vide »).
2. **Invalider le cache diagnostics** pour cette URI (`invalidate_diagnostics`) — pour
   que le `wait_for_diagnostics` de l'étape 5 bloque jusqu'au **prochain** push, pas
   sur le cached stale d'avant l'édition.
3. **Appliquer l'édition** sur le disque (`write_file` / `edit_file`).
4. **Déclencher la ré-analyse** — dépend de qui tient le fichier (cf. R1) :
   - **Cas A** (aucun éditeur navigateur sur l'URI) : le tool envoie
     `textDocument/didChange` (full sync, nouveau texte complet, version du compteur
     par URI de `LspSession`).
   - **Cas B** (un éditeur navigateur a l'URI ouverte) : le tool **n'envoie pas**
     `didChange` — il notifie le frontend « fichier changé sur disque » ; l'éditeur
     recharge son buffer et émet **son** `didChange`.
5. `wait_for_diagnostics(uri, borné)` — le prochain `publishDiagnostics` est la
   ré-analyse post-édition (peu importe qui a émis le `didChange`, il atterrit dans le
   même `diagnostics_cache` partagé). Timeout → `VNL-SBX-LSP-008` (état mou, pas une
   erreur : l'édition est faite).
6. **Diff** : `avant` vs `après` par identité `(line, col, severity, message)`.

**Ajout minimal au manager LSP** (`sandbox/src/lsp.rs` / `lsp_client.rs`) :
- `LspSessionInner` : `doc_versions: Mutex<HashMap<String, i32>>` (version par URI).
- `LspSession::next_doc_version(uri) -> i32` (incrémente, retourne).
- `LspSession::invalidate_diagnostics(uri)` (retire l'entrée du cache — pour que
  `wait_for_diagnostics` bloque jusqu'au **prochain** push, pas le cached stale).
- `LspClient::did_change(uri, version, full_text)` — notification
  `textDocument/didChange` avec `contentChanges: [{ text: full_text }]` (full sync).

Rien d'autre du manager ne bouge. `open_uris` reste write-once ; `edit_and_check`
suppose le fichier déjà ouvert (via `ensure_open`) et n'envoie **jamais** `didClose`.

### 8. Autosave éditeur + reload sur notification (frontend)

Requis par R1 cas B. Modules `frontend/` (éditeur CodeMirror — cf.
`docs/architecture.md` « Frontend — shell IDE Vue » / « WebSocket éditeur ») :

- **Autosave** : extension CodeMirror qui, sur `updateListener` (doc changé),
  planifie un `write` debouncé sur `/ws/fs` pour le fichier courant. Supprime l'état
  « modifications non enregistrées » (que l'éditeur ne matérialise pas aujourd'hui).
- **Reload sur notification** : à la réception d'un message `/ws/fs` « fichier changé
  sur disque : `<path>` », si un onglet tient ce fichier, relire le contenu et
  remplacer le buffer par une transaction CodeMirror (ce qui fait émettre le
  `didChange` par `@codemirror/lsp-client`). Avec autosave actif, aucune perte.
- **Flush avant édition LLM (cas B)** : optionnel — `edit_and_check` peut demander au
  frontend de flusher l'autosave du fichier avant d'écrire (cf. R1 sous-question 3).

Côté sandbox : le message sortant « fichier changé » sur `/ws/fs` (pas une nouvelle
route — un type de frame supplémentaire sur la connexion existante), émis par
`edit_and_check` après le `write_file`/`edit_file`.

---

## Risques identifiés et questions ouvertes

### R1 — Coexistence avec l'éditeur navigateur sur le MÊME fichier

**Décision développeur (2026-09-04) : on ne bloque jamais le LLM. `edit_and_check`
s'exécute quel que soit l'état de l'éditeur, et l'éditeur est rafraîchi après
l'édition. L'autosave de l'éditeur fait partie de cette feature (elle absorbe l'item
`## Auto-save` du backlog) — c'est ce qui rend le rafraîchissement sûr : plus de
modif humaine non enregistrée à perdre au reload du buffer.**

Le process LSP est **partagé** éditeur + tools (`docs/architecture.md`, « Serveur
LSP »). Quand l'éditeur navigateur a `foo.rs` ouvert, **le buffer de l'éditeur, pas
le disque, est la vérité du LSP pour ce fichier** — l'éditeur a envoyé son propre
`didOpen` (version 1) et des `didChange` au fil de la frappe. Deux clients LSP
indépendants (éditeur + `LspClient` du tool) qui envoient chacun des `didChange` avec
leur propre compteur de version pour la même URI = désync (rust-analyzer voit la
version reculer ou le contenu diverger, et LSP n'a **aucune** notification
serveur→client « ce document a été modifié par un autre »).

**Mécanisme retenu — l'éditeur reste seul émetteur de `didChange` quand il tient le
fichier :**

| Cas | Émetteur de `didChange` | Séquence |
|---|---|---|
| **A — aucun éditeur navigateur n'a l'URI ouverte** | le tool | `edit_and_check` écrit le disque → `LspClient::did_change` (compteur de version du tool, cf. §7) → `wait_for_diagnostics` |
| **B — un éditeur navigateur a l'URI ouverte** | l'éditeur | `edit_and_check` écrit le disque → notifie le frontend « fichier changé sur disque » → l'éditeur recharge le buffer depuis le disque → `@codemirror/lsp-client` émet **son** `didChange` (version de l'éditeur) → `publishDiagnostics` atterrit dans le `diagnostics_cache` partagé → `wait_for_diagnostics` du tool le récupère |

Dans le cas B, `edit_and_check` **n'envoie jamais son propre `didChange`** — il attend
que le round-trip navigateur produise la ré-analyse. Round-trip lent / WS déconnectée
→ timeout → `VNL-SBX-LSP-008` (édition faite sur disque, retry) ; à la reconnexion
l'éditeur recharge de toute façon.

**Autosave (dans le périmètre de cette feature).** Écriture debouncée du buffer
CodeMirror vers `/ws/fs` à chaque changement (l'éditeur n'a aujourd'hui **ni**
autosave **ni** indicateur « non enregistré », cf. `docs/architecture.md`
« Limites connues »). Effet sur R1 : quand un humain a le fichier ouvert, ses frappes
sont déjà sur le disque, donc (i) le `read_file` du LLM voit toujours l'état courant
et (ii) le reload de buffer du cas B ne détruit aucune modif locale. La fenêtre de
course résiduelle = le délai de debounce (frappe non encore flushée au moment où le
LLM écrit) — bornée à quelques centaines de ms au lieu d'une session d'édition
entière. Édition concurrente stricte humain+LLM sur le même fichier reste
intrinsèquement racy ; l'autosave la rend acceptable, pas nulle.

**Sous-questions à trancher à la task `edit_and_check` (pas avant) :**

1. **Détecter le cas B** : `LspSession` doit savoir si une URI est ouverte par un
   client *navigateur*. `open_uris` (write-once, tous clients confondus) ne le dit
   pas. Piste : `Set<String>` des URIs ouvertes par un `ClientId` dont le
   `subscribe` vient du bridge `ws/lsp.rs`, nettoyé sur `unsubscribe`.
2. **Canal de notification « fichier changé sur disque » vers le frontend** : sur
   `/ws/fs` (l'éditeur y est déjà connecté) ou un événement dédié ? Forme du message,
   et l'éditeur ne recharge que s'il tient ce fichier.
   **Tranché 2026-09-04** : push sur `/ws/fs` existant — `fs_session` passe à une boucle
   select (lecture socket + canal interne broadcast par path), le client gagne un dispatch
   permanent (frames réponse aux handlers one-shot, frames événement aux abonnés).
   Diffusion à toutes les sessions `/ws/fs` authentifiées ; l'éditeur ne recharge que s'il
   tient le fichier. Note d'implémentation : le loop actuel est strictement
   requête→réponse (`ws/fs.rs`) et le client consomme par listener one-shot
   (`sandboxWs.ts`) — les deux sont à retoucher, c'est assumé.
3. **Autosave : debounce et flush** — valeur du debounce (200-500 ms ?) ; et est-ce
   que `edit_and_check` en cas B demande un **flush immédiat** au frontend *avant*
   d'écrire (ferme la fenêtre de course au prix d'un round-trip de plus), ou accepte
   la fenêtre de debounce ? Recommandation : flush explicite avant écriture en cas B,
   debounce simple sinon.
   **Tranché 2026-09-04** : debounce **300 ms** (livré tâche 07a) ; en cas B,
   `edit_and_check` envoie une requête flush au frontend et attend l'ack avant d'écrire ;
   timeout de flush court → repli sur la fenêtre de debounce, mention dans le rapport.
4. **Autosave : périmètre de déclenchement** — tous les fichiers ouverts, ou
   opt-out possible ? Interaction avec un fichier en lecture seule / hors workspace.
5. **Cas B sans que le round-trip aboutisse** (autre onglet, pas le fichier au
   premier plan) : le timeout `VNL-SBX-LSP-008` couvre, mais valider que le message
   dit clairement « édition appliquée, ré-analyse en attente de l'éditeur ».

Les tools 1 à 6 ne dépendent pas de R1 — implémentables d'abord. La task
`edit_and_check` (+ autosave) porte ces sous-questions.

### R2 — `documentSymbol` / `workspace/symbol` non vérifiés sur les deux serveurs

Cf. §4. Méthodes standard, support attendu, mais le backlog impose de vérifier avant
de s'engager (contrairement au pull diagnostics, déjà testé). Task carte-du-code à
faire précéder d'une vérification cluster ou d'un test contre un serveur réel.

### R3 — Latence de ré-analyse rust-analyzer variable après `didChange`

`edit_and_check` borne l'attente (§7 étape 5). Le bon comportement à la limite est
`VNL-SBX-LSP-008` (« relance dans quelques secondes », édition appliquée), **jamais**
un blocage indéfini ni un « propre » par défaut (même piège que les 3 états de
`lsp_diagnostics`). Valeur de timeout à caler à l'implémentation ; commencer par
`DIAGNOSTICS_TIMEOUT` existant × 2.

### R4 — Retrait de `lsp_hover` = changement d'interface tool publiée

Décision produit prise (2026-09-04). `lsp_hover` livré le 2026-08-20, retiré 15 jours
après. Impact limité : les tools `lsp_*` sont LLM-only, l'éditeur navigateur consomme
le LSP en direct (pas les tools MCP) — le menu contextuel « Aller à la définition » de
l'éditeur n'est pas touché. Mise à jour requise à la clôture : `docs/architecture.md`
§ « Serveur LSP » (liste des 5 tools).

### R5 — Surface d'entrée utilisateur/réseau → shell / URL / chemin

Contrainte `.claude/config.md` (Phase 1) — revue explicite, pas seulement la forme des
endpoints :

- **`path`** (tous les tools) : passe par `confine` / `confine_path` existant
  (`tools_impl.rs:59`) — anti-traversal déjà en place, réutilisé tel quel, **aucun
  nouveau chemin ne contourne `confine`**. `edit_and_check` confine `path` **avant**
  d'appeler `write_file` / `edit_file`.
- **`symbol`, `query`, `new_name`** : injectés dans des **params JSON-RPC LSP**
  (`workspace/symbol.query`, `rename.newName`, ou une recherche de sous-chaîne locale
  pour `symbol`). **Jamais** dans une commande shell, une URL, ni un chemin. `symbol`
  sert à indexer une ligne de fichier déjà lue en mémoire — recherche de sous-chaîne
  Rust, pas de regex compilée depuis l'entrée (pas de ReDoS), pas d'accès disque
  supplémentaire.
- **`edit_and_check` contenu d'édition** : c'est le rôle de `write_file` / `edit_file`
  de `vanyline-tools`, inchangés — le contenu écrit n'est pas interprété.
- **Aucun** de ces tools ne construit d'argv, de commande shell, d'URL réseau ou de
  chemin filesystem à partir d'une entrée non-`path`. Le seul I/O est : lecture du
  fichier confiné, écriture du fichier confiné (`edit_and_check` uniquement), messages
  JSON-RPC vers le process LSP local via stdio (pas de réseau).
- **Autosave / notification « fichier changé »** : le `path` du message sortant
  `/ws/fs` et celui du `write` d'autosave passent par le même confinement que les
  `write`/`read` `/ws/fs` existants — l'autosave n'écrit que le fichier actuellement
  ouvert dans un onglet (chemin déjà validé à l'ouverture), le message « fichier
  changé » porte un chemin relatif au workspace produit côté sandbox (jamais un
  chemin absolu, cohérent avec la convention `/ws/fs`), l'éditeur ne l'utilise que
  pour un lookup d'onglet, pas pour un accès disque direct.

### R6 — `resolve_position` : ambiguïté de symbole sur une ligne

`symbol: "x"` sur `let x = f(x);` — 2 matchs. Décision : **premier match**, et le
signaler dans la réponse (`(symbole "x" trouvé 2× sur la ligne, 1re occurrence
utilisée — précise character: N pour l'autre)`). Ne pas deviner l'intention.

---

## Codes d'erreur alloués

| Code | Sens |
|---|---|
| `VNL-SBX-LSP-007` | `resolve_position` : `symbol` introuvable comme identifiant sur la ligne indiquée |
| `VNL-SBX-LSP-008` | `edit_and_check` : édition appliquée sur le disque, ré-analyse LSP pas stabilisée avant le timeout (état mou, retry) |

Existants réutilisés : `VNL-SBX-LSP-004` (process fermé), `-005` (erreur serveur LSP),
`-006` (pas de LSP pour l'extension / la toolchain).

---

## Ordre d'implémentation proposé (tasks just-in-time)

1. **Modèle de position** (`LspSymbolTarget` + `resolve_position` + snippets) —
   refactor des 3 tools position existants, comportement inchangé hors ergonomie.
   Base pour tout le reste.
2. **`lsp_definition` absorbe hover** + retrait `lsp_hover`.
3. **Carte du code** (`lsp_document_symbols`, `lsp_workspace_symbols`) — précédée de
   la vérif de support (R2).
4. **`lsp_references` enrichi** (dépend de `documentSymbol` de la task 3).
5. **`lsp_rename` preview + rapport**.
6. **`inspect_symbol`** (composition pure de 2 + 4).
7. **Autosave éditeur** (frontend) — extension CodeMirror débounce → `write` `/ws/fs`,
   + réception d'un message « fichier changé sur disque » → reload de buffer. Livrable
   seul (utile indépendamment), prérequis du cas B de la task 8.
8. **`edit_and_check`** — côté sandbox : `did_change` / `doc_versions` /
   `invalidate_diagnostics` au manager, tracking des URIs ouvertes par un client
   navigateur, émission du message « fichier changé » sur `/ws/fs`, dispatch du tool
   lui-même (diff de diagnostics, cas A/B). Porte les sous-questions de R1. **La plus
   grosse — à re-découper** (p. ex. 8a manager + tracking, 8b le tool + le diff).

Tasks 1-6 et 7 indépendantes de R1 ; 8 dépend de 7. Chaque task : un commit, tests
d'abord (TDD), fakes LSP existants (`lsp_test_fakes::FAKE_LSP_PY` /
`FAKE_LSP_NODIAG_PY`) étendus au besoin.

---

## À faire à la clôture (Phase 3)

- Migrer la description de l'interface `lsp_*` de v2 dans `docs/architecture.md`
  § « Serveur LSP » (remplacer la liste des 5 tools, ajouter le modèle de position,
  `edit_and_check`, l'ajout `didChange` au manager).
- Documenter l'autosave dans `docs/architecture.md` § « Frontend — shell IDE Vue » /
  « WebSocket éditeur » et **retirer** la mention « aucun indicateur modifications non
  enregistrées » des « Limites connues ».
- Supprimer ce fichier.
- Retirer l'item `## Auto-save` de `docs/backlog.md` (absorbé) et vérifier que la
  partie 1 en a bien été retirée à l'extraction, que « Support éditeur — autres
  langages » ne référence plus une partie disparue.
