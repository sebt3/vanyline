# Mémoire du projet

Ce fichier est maintenu par Claude au fil des sessions.
Les développeurs peuvent le lire, le corriger ou le compléter à tout moment.

---

## Identité du projet

**Nom** : vanyline (de "vaniline" — addictif, universellement aimé, 'y' inséré pour l'inverse-SEO)
**But** : Environnement de développement cloud-native, multi-utilisateur, piloté par l'IA pour Kubernetes
**Licence** : BSD-3, public sur GitHub
**Gitea** : shuss/vanyline (privé, solo pour l'instant)
**Stack** : Rust (app, sandbox, controller) + TypeScript/Svelte 5/CodeMirror 6 (frontend)
**Monorepo** : Cargo workspace racine + package.json racine, chaque composant Rust a son sous-workspace

---

## Architecture — décisions et justifications

### Quatre composants

| Composant | Rôle | Décision |
|-----------|------|----------|
| frontend | Éditeur web + UI LLM | Svelte 5, CodeMirror 6 — framework de build TBD (SvelteKit vs Vite) |
| app | Backend, OIDC, Redis, PGVector | Rust — focus initial : interaction LLM, users, config API |
| sandbox | Pod K8s, serveur WS/MCP | Rust — image Debian slim + toolchains OCI volumes |
| controller | Opérateur K8s | Rust, kube-rs — **DÉFÉRÉ** |

### Design sandbox — élégance clé à retenir

La base image est stable (Debian slim + serveur + git/curl/make/vim). Les toolchains sont composées
à la création du pod via volumes OCI — pas de rebuild d'image quand on ajoute une toolchain.
PATH et LD_LIBRARY_PATH sont injectés pour rendre les binaires accessibles dans le shell.

### Contrôleur déféré

Le controller est la glue entre les deux axes de dev. Il ne sera pas construit immédiatement.
Pour tester la sandbox, un script shell/Python créera les pods directement.
Le CRD Owner aura des attributs quota (autres TBD).

---

## Scope — phase actuelle

**Deux axes en parallèle :**
1. **app + frontend** : interaction humain/LLM, gestion utilisateurs, API de configuration
2. **sandbox** : image de base + serveur + composition toolchains OCI

**Hors scope pour cette phase :**
- Controller Kubernetes
- Intégration des deux axes (nécessite le controller)
- Multi-utilisateur (arrive avec le controller)
- Ouverture aux autres contributeurs

**Déclencheur de convergence** : quand les deux axes sont assez matures pour s'assembler via le controller.

---

## Équipe et ouverture

- Solo pour l'instant (un seul développeur)
- Un second développeur rejoindra quand les deux axes se rencontreront (stade poc→mvp)
- Pas de définition formelle de MVP — construction incrémentale long terme

---

## Points ouverts (TBD)

- Framework frontend : SvelteKit vs Vite+Svelte (impact sur routing et commandes de build)
- Web framework Rust pour app : axum probable, pas encore décidé
- Providers LLM cibles : non définis
- Format des identifiants d'erreur
- Logger projet (app et sandbox)
