---
name: lsp-integration
description: LSP par toolchain dans la sandbox (process partagé éditeur+LLM, URIs, rename cross-file) — décisions, blocages cadence, review de clôture (2026-08-20)
metadata:
  type: project
---

# LSP par toolchain dans la sandbox (2026-08-19 → 2026-08-20)

## Ce qui a été livré

Design initial (`docs/features/lsp-integration.md`, supprimé à la clôture — contenu
migré dans `docs/architecture.md`, sections "Serveur LSP", "Détection de langages et
toolchains automatiques" et "Frontend — shell IDE Vue") : un process LSP par
toolchain dans le pod sandbox (`LspManager`/`LspSession`, multiplexage multi-clients),
**unique et partagé** entre l'éditeur navigateur (`@codemirror/lsp-client` via une
route WS `/ws/lsp/:toolchain`) et le LLM (tools MCP `lsp_diagnostics`/`lsp_hover`/
`lsp_definition`/`lsp_references`/`lsp_rename`, additifs aux tools existants).
`Toolchain.lsp: Option<LspSpec>` côté CRD, résolu par preset name-keyed (rust/node)
ou explicite (LSP custom possible). Rename cross-file côté UI par flux custom
(`textDocument/rename` direct), pas le helper du package.

## Process — délégation à cadence, mais avec un vrai signal d'escalade cette fois

Comme `ws10-language-support` et `editing-context-menus`, cette feature a été
déléguée à l'agent opencode `cadence` après le design Claude. Différence notable par
rapport aux deux précédentes : **cadence s'est arrêté et a escaladé trois fois**
plutôt que de livrer silencieusement un code qui compile avec des lacunes trouvées
seulement en review Phase 3. Chaque blocage a été résolu par une décision
développeur+Claude documentée dans le design doc avant que cadence reprenne — le
design doc a servi de journal de décision vivant, pas juste un plan figé en amont.
`cargo fmt --check` a aussi été lancé et passait dès la review de clôture — première
fois sur cette lignée de features déléguées (motif répété 4 fois avant celle-ci, cf.
`ws10-language-support`, `editing-context-menus`).

### Les trois blocages

1. **Forme de `Toolchain.lsp`** — le design initial ne précisait pas si l'image LSP
   venait du CRD (explicite) ou d'un preset controller (implicite). Résolu par
   précédent de code : `Toolchain.image` n'a jamais de fallback preset invisible
   (toujours explicite dans le CRD), mais `Toolchain.env` en a un, appliqué
   uniformément. Décision : `LspSpec{image,bin,args}` explicite + fallback name-keyed
   pour `image` (via `Context`/env `LSP_IMAGE_*`, configurable au déploiement) et pour
   `bin`/`args` (hardcodés, ce sont des faits protocolaires, pas de la config
   d'infra). Ni un pur A ni un pur B des options posées par cadence — un hybride qui
   suit deux précédents différents pour deux champs différents du même struct.
2. **URIs absolues côté navigateur** — LSP exige des URIs absolues, le navigateur ne
   connaît jamais `VNL_SANDBOX_ROOT` (cohérent avec `/ws/fs`). Résolu par
   normalisation bidirectionnelle dans le bridge WS sandbox (walker JSON générique
   sur les clés `*Uri`), pas par exposition du root au navigateur — cohérence avec la
   convention relative déjà en place partout ailleurs dans l'app, et un seul point de
   traduction réutilisable pour le rename plus tard. Cas spécial anticipé et
   documenté à l'avance pour `WorkspaceEdit.changes` (URI en clé d'objet) — n'a donc
   pas généré un 4ᵉ blocage quand la tâche rename l'a atteint.
3. **Rename cross-file, helper du package limité** — `renameSymbol` de
   `@codemirror/lsp-client` ignore silencieusement les fichiers non ouverts (confirmé
   en lisant `rename.ts`). C'est le risque "maturité du package" déjà noté dans le
   design initial qui s'est matérialisé, pas un nouveau trou. Résolu par flux rename
   custom (`textDocument/rename` direct + application manuelle du `WorkspaceEdit`),
   réutilisant le même contrat non-atomique/best-effort que le tool MCP `lsp_rename`
   déjà livré côté sandbox (`apply_workspace_edit`) — pas de nouvel endpoint batch sur
   `/ws/fs`.

**Leçon pour le prochain design doc qui fait entrer un protocole/lib externe** :
dérouler 2-3 formes de message concrètes (pas seulement les interfaces à l'altitude
architecture) suffit souvent à intercepter ce genre de collision de contrat avant
l'implémentation plutôt que pendant.

## Piège trouvé en review de clôture (corrigé)

Message de statut du rename trompeur : un fichier déjà ouvert n'est modifié que dans
le buffer CodeMirror (pas d'autosave dans cet éditeur), un fichier fermé est écrit
sur disque immédiatement — mais le message annonçait les deux comme "Renommé" sans
distinction. Aggravé par un gap plus large, pré-existant et non traité ici (hors
périmètre) : cet éditeur n'a **aucun** indicateur "modifications non enregistrées"
sur les onglets, nulle part. Corrigé : message qui distingue `savedToDisk`/
`pendingSave`/`failed`. Au passage, un test ("demande nom et renomme") avait un mock
`getView()` sans `dispatch` — le chemin heureux échouait silencieusement et
l'ancienne formulation vague du message masquait l'échec en contenant quand même le
mot "Renommé". Corrigé aussi — **leçon** : un message de statut trop vague pour
distinguer succès/échec peut aussi maquiller un test qui ne teste pas ce qu'il croit
tester.

## Limite connue assumée, pas un bug caché

`LSP_IMAGE_RUST`/`LSP_IMAGE_NODE` pointent par défaut sur les images toolchain
(pas de rust-analyzer/typescript-language-server dedans) — documenté en commentaire
par cadence dès l'implémentation. Code complet et testé, **pas fonctionnel sur un
cluster réel** tant que ces images ne sont pas construites et publiées. Même motif
que plusieurs features précédentes (web IDE, détection de langages) jamais validées
en conditions réelles — pas une régression spécifique à cette feature.
