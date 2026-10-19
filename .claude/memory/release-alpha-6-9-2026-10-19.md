# Releases alpha.6 → alpha.9 (2026-10-19) — premier déploiement cluster de l'état final

Branche `docs/specs-etat-final-2026-10` poussée (jamais mergée dans `main`, fast-forward possible).
`alpha.6` et `alpha.7` sont des releases GitHub **incomplètes** (images en échec) ; `alpha.8` complète
mais défectueuse en usage ; **`alpha.9` = première version testable** (CI verte, déployée sur
`media-test`, Restate enregistré).

## Défauts trouvés (tous inter-composants, invisibles aux tests unitaires)
- Dockerfiles : `app` ne copiait pas `packages/` (workspaces npm) ; `jsonnet-sys` (C++) exige g++ au
  build et libstdc++6 au runtime de `app` ; `controller`/`sandbox` doivent cuire `-p <crate>` seul.
- `lib` : `.timeout(300)` refusé par kube 4 (< 295) → watch en boucle, hub temps réel mort.
- NetworkPolicy sandbox : le pair `app` sans `namespaceSelector` (reste de « un seul namespace »)
  → 502 sur tout relais. Spec disait déjà juste, le code était resté en arrière.
- Restate : Service app sans port 9080 ni `VNL_WORKFLOW_DEPLOYMENT_URL` (le nom dérivé par l'app
  n'existait pas : le Service s'appelle `application-<nom>`).
- `ensure_owner` se fiait au lien en base sans vérifier l'Owner K8s ; premier Project d'un nouvel
  utilisateur en 403 (course : namespace/RoleBinding posés async par le controller).
- Interface : statut du Project lu une fois au montage ; PUT partiel → 422 `missing field name`
  (le PUT générique miryad-core veut le modèle COMPLET et écrit toutes les colonnes) ; menu des
  onglets via `getTabContextMenuItems` = module enterprise dockview (supprimé).
- Test de fournisseur LLM : endpoint = RACINE sans `/v1` (la lib l'ajoute) ; message opaque corrigé
  (`VNL-LLM-009`).

## Méthode / leçons
- Spec d'abord (j'écris), Haiku implémente (test rouge→vert), je vérifie par `git diff` + batterie.
  Rapports de Haiku fiables cette fois (tous vérifiés dans l'arbre).
- **OOM de code-server** : jamais plusieurs builds cargo/podman en parallèle ; `CARGO_BUILD_JOBS=2`,
  un seul agent cargo à la fois (frontend en parallèle OK).
- `docs/release-runbook.md` §8 (smoke test sans recharger la page, base vierge, utilisateur neuf)
  et §9 (pièges de build) ajoutés : c'est ce parcours qui révèle les défauts de frontière.
- Tag déjà poussé = on ne le déplace pas (bump suivant).
- Test `health::runner::timeout_tue_le_groupe` : rouge local seulement (pas de binaire `kill`).

## Restes ouverts
- `[?]` get_owner en erreur transitoire = « absent » (recréation → 409) ; listes de Projects/Sandboxes
  en 403 si lien périmé (ensure_owner ne répare qu'à la création).
- Scenario owners.sdd « ensure_owner avec un lien » sans test dédié ; checklist runbook §8 à rejouer.
- Étape 9 `llm-config` ; arbitrages `[?]` ; prompts des agents système à relire ; merge dans `main`.
