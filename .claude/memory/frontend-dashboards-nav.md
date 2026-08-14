# frontend-dashboards-nav — navigation à trois niveaux + modales + dropdowns relationnels

Feature fermée le 2026-08-14 (task-01 → task-15, 15 tâches Qwen `implement`, toutes
commitées sur `main`). Design doc supprimé, contenu migré dans `docs/architecture.md`
(sections "Backend web — vanyline-app" et "Frontend — shell IDE Vue").

## Ce qui a changé

- **Routing** : `/` (accueil, liste projets) → `/p/:projectName` (dashboard projet,
  liste sandboxes) → `/p/:projectName/s/:sandboxName` (IDE, ex-`/ide/:sandboxName`,
  route supprimée sans redirect de compat). `App.vue` bascule entre le shell IDE
  (MenuBar+StatusBar) et un shell léger (`AppBreadcrumb.vue`) selon la route.
- **Dashboards** (`HomeDashboard.vue`/`ProjectDashboard.vue`) absorbent
  `ProjectsScreen.vue`/`SandboxesScreen.vue` (supprimés) — tableau + modale de création
  + bouton Paramètres, pas de bouton "gérer" séparé.
- **Settings réorganisé** : Projets/Sandboxes sortis (→ dashboards), les 6 écrans CRUD
  restants regroupés (Modèles / Outils / Agents / Skills / Compte) et convertis en
  modales reka-ui `DialogRoot` (création + édition).
- **Champs relationnels** : `ModelProfile.provider/model`, `Agent.model` (référence en
  réalité un `ModelProfile`, pas un provider/modèle brut — le libellé API est trompeur),
  `Agent.toolsets/skills`, `Toolset.mcp[].server/tools`, `Toolset.local_tools` — tous
  passés de texte libre à des dropdowns alimentés par les endpoints existants.
- **2 endpoints backend ajoutés** : `GET /api/local-tools` (lecture seule, expose le
  registre statique `tools::mcp::*`, déjà stable/testé — pas d'attente de tools-v2) et
  `POST /api/mcp-servers/{id}/test` (discovery de tools, réutilise
  `connect_domain_mcp_server_inner`/`list_all_tools()` de `lib/src/prefixed_mcp.rs`,
  persiste dans une nouvelle colonne `mcp_servers.available_tools`).

## Erreurs de compréhension corrigées en cours de route (pas des échecs, des infos)

1. **Portée de "modaux flottants" mal cadrée dans le design doc initial** : la première
   version ne mentionnait explicitement que les deux dashboards pour la conversion en
   modale ; Qwen a implémenté cette lecture restrictive et l'a signalée en revue plutôt
   que de deviner. Le développeur principal a confirmé que la demande initiale (point 2
   de la session de design) visait bien **tous** les écrans Settings à formulaire
   inline. Design doc corrigé, tâches complémentaires écrites (task-09 → task-14).
2. **Hypothèse fausse sur `local_tools`** : Claude avait d'abord affirmé qu'aucun
   registre de tools locaux n'existait ("attendre tools-v2"). En creusant
   `tools/src/mcp.rs`, le registre existait déjà (8 tools, testé) — correction faite
   en session, `GET /api/local-tools` ajouté au périmètre au lieu d'être différé.
3. **Régression trouvée en revue Phase 3** (task-15) : la conversion en modale avait
   déplacé `providersError`/`optionsError` à l'intérieur du `DialogContent` de
   création — invisibles au chargement de l'écran et dans la modale d'édition. Corrigé
   avant clôture (déplacé dans le corps principal de l'écran).
4. **Bug de test, pas de code** (task-15, dernier blocage avant clôture) :
   `ToolsetsScreen.spec.ts` mockait une erreur `/api/local-tools` en texte brut sans
   `Content-Type: application/json` — `client.ts` ne synthétise le message d'erreur que
   depuis un corps JSON `{"error": ...}` (comportement établi, cohérent avec le reste
   des routes `app`), un corps texte brut retombe toujours sur `HTTP {status}`. Le test
   voisin (même fichier) le faisait déjà correctement — corrigé pour être cohérent.

## Limite pré-existante découverte (pas introduite par cette feature)

`POST /api/mcp-servers/{id}/test` ne fonctionne que pour `server_type:
"http-streamable"` — `McpTransport` (`lib/src/prefixed_mcp.rs`) n'a qu'un seul variant,
un serveur `sse` n'a pas d'implémentation de transport. Documenté dans
`docs/architecture.md` ("Limites connues").

## Process

Tout commité directement sur `main` (pas de branche `feat/frontend-dashboards-nav`
séparée) — écart par rapport à la lecture stricte de `.claude/config.md` ("branche
feature"), pas relevé comme bloquant en session, à garder en tête si une prochaine
feature veut une vraie branche isolée. Un commit (`170dced`, task-09) a un message hors
format prescrit (`fix(...)` au lieu de `(feat: ...)`) — signalé par Qwen, le développeur
principal a explicitement choisi de ne rien corriger (pas poussé, pas grave pour un
usage solo).
