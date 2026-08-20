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

## Suivi same-day — images LSP réelles (`fix/lsp-toolchain-images`, 2026-08-20)

La limite notée à la clôture (`LSP_IMAGE_RUST`/`LSP_IMAGE_NODE` = images toolchain
génériques, sans LSP dedans) a été corrigée le jour même, sur retour développeur.
Piste initiale envisagée (`mcr.microsoft.com/devcontainers/typescript-node`,
supposée tout inclure) écartée après lecture du Dockerfile réel — ne contient pas le
LSP. Piste rust-analyzer envisagée par le développeur (`rustup component add` en pod)
confirmée impossible : `volumes[].image` monte toujours en lecture seule côté K8s,
propriété de l'API, pas une contrainte vanyline — reproduit avec l'erreur exacte
(`Read-only file system`) avant de conclure. Résolu par deux nouvelles images
publiées avec le monorepo, LSP baké **au build** (pas au runtime) :
`toolchains/rust/Dockerfile` (`rust:slim-trixie` + `rustup component add
rust-analyzer` + symlink), `toolchains/node/Dockerfile` (`node:trixie-slim` + `npm
install -g typescript-language-server`), publiées avec le même tag que
app/sandbox/controller (`.github/workflows/release.yml`, matrix étendue).
`TOOLCHAIN_IMAGE_*` et `LSP_IMAGE_*` pointent désormais par défaut sur la même
image par langage — décision assumée de garder le double montage existant
(`/toolchains/<name>` et `/toolchains/<name>-lsp`, même image deux fois) plutôt que
de rouvrir `LspSpec`/le mécanisme de montage pour un gain marginal.

Toujours pas testé sur un cluster réel (pas d'environnement K8s dans ces sessions de
dev) — mais le code n'est plus structurellement mort comme à la clôture initiale.
