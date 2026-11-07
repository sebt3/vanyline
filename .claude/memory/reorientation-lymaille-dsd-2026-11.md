# Réorientation du harnais LLM vers lymaille + DSD, et spike lymaille × sandbox (2026-11-03)

Session de réflexion avec le développeur principal. **Aucune ligne de code de production n'a été
modifiée, aucune spec n'a été écrite** : cette session a posé la direction, mesuré une hypothèse
par un spike jetable, et laissé le vrai chantier (installer DSD et écrire le DSD de
réorientation) à la session suivante.

## 1. Direction décidée

**Le constat.** Le tooling LLM construit dans vanyline (état-final 2026-10 : `lib`, agents
système, `llm-config` prévu) ne répond pas à la base de harness voulue, et le plan « extraire
vers miryad-core » est **caduc**.

**La thèse.**
- Pas de mécanisme d'approbation **dans** une session (le « yolo » des docs), mais le LLM ne peut
  pas tout faire : les limites sont posées **avant**, par le contexte.
- Le sélecteur de début de session est le **contexte de travail** (Project/Sandbox, l'écran d'où
  l'on ouvre le chat), **pas la personnalité d'un agent principal** (comme Claude Code : on choisit
  le répertoire, pas l'agent). Une « session de configuration » est avant tout un **contexte de
  configuration** ; le vocabulaire technique (agent `Config`, etc.) peut être masqué par l'UX.
- **lymaille** (`~/projets/lymaille`, publiée sur crates.io en 0.1.0) est
  le cœur générique réutilisable (sessions durables, scratchpad Rhai, toolsets découverts à la
  demande). miryad-core l'intègre (en cours) ; vanyline la consomme. **Vanyline reste le harnais
  dédié au code** : sandbox, controller, crds, health et shell frontend sont **inchangés** (la
  sandbox est le point fort et le cœur du projet).
- DSD (extension de SpecDD : Domain Definition puis Solution Design, au-dessus des `.sdd`) est
  adopté dans vanyline, comme dans lymaille (`dsd/agent-sessions.md` + `dsd/agent-sessions/*.md`).

**Décisions techniques prises en cours de discussion.**
- **Hôte de la session lymaille = `app`** (pas la sandbox). Les relais `fs`/`shell` de lymaille
  sont désactivés par défaut et miryad-core y veille ; toute action sur le code passe par le MCP de
  la sandbox (fonctions Rhai générées par `mcp_module`).
- **Modèle de menace de l'agent de code.** Les toolsets accordés bornent l'*intention* du LLM
  (un wrapper Rhai de politique sur l'exec, module sous-jacent inaccessible au LLM, est
  légitime), pas les *effets* (`build.rs`, scripts postinstall exécutent du code arbitraire).
  Les effets se bornent au niveau du pod : NetworkPolicy en egress, secrets absents du pod, SA sans
  RBAC, quotas, isolation noyau (RuntimeClass gVisor/Kata : à regarder côté controller). Une
  approbation humaine par commande ne protège pas mieux contre la malveillance. À écrire noir sur
  blanc dans la Domain Definition : la liste de ce que l'agent de code ne doit jamais pouvoir faire.
- **`git push` depuis la sandbox est normal et non restreint par vanyline** : l'Owner porte une clé SSH
  qui tire/pousse tous ses dépôts (c'est aussi le cas pour le travail de dev ici). Protection de `main` et revue
  des MR = la forge ; un credential limité au projet reste possible côté forge, vanyline ne l'exige pas.
- **Séparation stricte des rôles.** En contexte productif, le LLM (et ses sous-agents) peut modifier
  `.skills/` et `AGENTS.md` ; il ne remonte dans la configuration que via une session en contexte
  de configuration, et la promotion est une action humaine hors session (DEC-13 de lymaille). Les
  *droits* ne viennent jamais du workspace (un manifeste ne contient que des noms, stockés par
  l'hôte). Trou à écrire : un workflow privilégié qui ouvre le même workspace lit des instructions
  écrites par un contexte de moindre confiance.
- BO-3 de lymaille (« l'utilisateur choisit l'agent et l'environnement ») contredit la thèse : à
  reformuler dans le DSD de lymaille (l'agent principal découle du contexte).

**Périmètre probablement touché (à confirmer par inventaire, non mesuré)** : `lib` (~6k lignes),
`cfgstore` (~9k), `cli` (~16,7k, surtout chat/TUI), une partie d'`app` (~29k : agents, config LLM,
chat, workflows `agent`, MCP de domaine), côté frontend `ChatEvent`/`@vanyline/protocol`/
`@vanyline/ui`. Inchangés : sandbox, controller, crds, health, `tools` (côté sandbox).

**À traiter** : les ~32 commits non poussés de la branche `docs/specs-etat-final-2026-10` (étapes
8 « agents système » et 9 « llm-config » incluses) sont à trier avant tout push. Les passages
« philosophie yolo, `permission` sans objet » de `docs/architecture.md` (l. 233 et 278) et de
`docs/llm-exec-gap.md` (l. 84, 117) sont mal formulés : ce n'est pas « pas de permissions » mais
« permissions fixées par le contexte, pas par une approbation en session ».

## 2. Le spike lymaille × sandbox (conclusions ; données brutes non conservées)

Spike jetable (`spikes/lymaille-sandbox/`, hors workspace, non suivi, ~25 essais Qwen3.6-35B-A3B et
Qwen3.8-flash sur une sandbox réelle, sandbox supprimée le 2026-11-03). Il a servi à nos yeux avant le DSD ;
le développeur ne garde pas ces données comme référence. **Verdict : le setup marche** (un LLM réussit une
tâche de code quand la sandbox est présentée comme des modules Rhai).

Conclusions retenues :
- Le goulot n'est pas le pad indirect mais l'échappement de code dans un littéral Rhai.
- Qwen3.6 et Qwen3.8 connaissent mal Rhai ; un prompt enrichi (~1k tokens) suffit : règles `##"…"##`, pas de
  backtick dans une chaîne à backticks, `r#"…"#` n'existe pas en Rhai, une `fn` ne voit pas les variables du pad,
  paramètres en `#{…}`. Avec elles : 9–10 appels, 0 erreur de script.
- Un tool `stash` n'a pas aidé : ne pas l'ajouter à lymaille.
- Persistance du pad : non mesurée, jugée sans effet sur la direction.
- Issues remontées à lymaille le 2026-11-03 : #1 (réponse vide `Valid`), #2 (raisonnement perdu si coupure),
  #3 (message d'erreur `#{…}` + règles du prompt dans la skill de départ), #4 (reformuler BO-3).
- Infra : `Sandbox.spec.egress` vide laisse crates.io atteignable (à vérifier si voulu) ; `execute_command`
  a un timeout par défaut de 30 s ; les jetons d'Owner de `kubectl create token` expirent.

## 3. Avancement et reste à faire

**Fait le 2026-11-03** (`dsd/app.md` relu une première fois le même jour : BR-4/BR-5 entérinées, IDE + vue du diff hors modèle de tableaux de bord, OI-4/OI-5 d'origine tranchées en DEC-7/DEC-8) : plugin DSD installé (`specdd plugin add git@github.com:specdd/specdd.git dsd`),
`vanyline.sdd` `Forbids: /spikes`, **ébauche de `dsd/app.md`** (Domain Definition du domaine `app` seulement,
sans Solution), note de cadrage pour miryad-core (`~/projets/miryad-core/NOTE-vanyline-orientation-lymaille.md`,
non commitée, à reprendre en session là-bas), issues lymaille, sandbox du spike supprimée.

**Ajouté ensuite (même jour)** : `dsd/sandbox.md` (santé du code incluse, le développeur l'a voulu), `dsd/workspaces.md`
(Owner/Project/Sandbox/Application, controller, CRDs, identité en base), `dsd/cli.md` (avenir en TBD). Pas de domaine
`workflows` : c'est une fonctionnalité de miryad-core que vanyline enrichit (Solution `app/workflows` proposée). Règle
ajoutée au CLAUDE.md global : ne plus évoquer le coût en tokens de SpecDD/DSD. **Rien n'est commité** : présenter les
quatre domaines au développeur, puis 2 commits (plugin + `app`, puis les autres), sans `.claude/` ni `spikes/`. Avant de
supprimer cette mémoire, vérifier que tout son « reste à faire » est dans les Transition/Open Issues des domaines.

À faire, dans l'ordre :
1. Discuter `dsd/app.md` avec le développeur (la vision UX en est la matière : contextes, tableaux de bord,
   widgets scriptés, actions d'interface, artefacts, SpecDD+DSD) ; trancher les Open Issues, puis écrire les
   Solutions une à une (paysage proposé : sessions, dashboards, widgets, ui-actions, artifacts, planning,
   configuration). Le développeur ne veut pas tout le DSD d'un coup.
2. **Session miryad-core** : amender son DSD (conditions B1–B4 de la note : serveurs MCP fournis par l'hôte,
   ouverture par contexte, projet résolu par l'hôte, durée des runs d'agent).
3. **Domaines à écrire plus tard, beaucoup plus mécaniques** : sandbox (avec le modèle de menace, la liste de ce que
   l'agent de code ne doit jamais pouvoir faire, **et l'outillage SpecDD dans la boîte à outils JS : modification
   demandée par le développeur, à traiter avec ce domaine, noté ici pour la session suivante**), controller,
   health.
4. Inventaire en lecture seule de ce que vanyline duplique ou contredit par rapport à lymaille (`lib`, `tools`,
   `cfgstore`, `llm-config`, agents système, MCP de domaine, workflows `agent`, TUI, `ChatEvent`), puis tranche
   verticale mince et retrait de l'ancien chemin à parité.
5. Trier les ~32 commits non poussés de la branche `docs/specs-etat-final-2026-10` (étapes 8 et 9) ; corriger les
   passages « yolo » de `docs/architecture.md` (l. 233, 278) et `docs/llm-exec-gap.md` (l. 84, 117) en
   « permissions fixées par le contexte, pas par une approbation en session ».

**Mis à jour le 2026-11-07 — avenir de la CLI (dsd/cli.md, décidé avec le développeur)** : la CLI devient un **hôte
lymaille permanent et autonome** (pas de phase transitoire, indépendante de l'avancement de miryad-core), TUI ratatui
conservé (backend remplacé, adaptateur d'événements, approbation en session retirée). Stockage : traits de lymaille
(`SessionStore`, `AgentStore`, `ProfileStore`, `ProviderStore`) implémentés sur fichiers **dans la crate `cli`** ;
`vanyline-cfgstore` retiré du workspace avec `lib`. Tout le niveau partagé `.vanyline/` (agents, skills, profils, providers, contextes) se transfère CLI↔`app`
(responsabilité d'`app`, `app#OI-7`), sans secrets, par promotion humaine ; `vanyline sync push|pull` explicite, refus si
divergence (`cli#DEC-11`). `--context` = un Project, `--sandbox` = instance existante (remplace `--toolbox`). `run` reste
(batch avec LLM) ; nouveau `vanyline script` (Rhai sans LLM sur une sandbox, mêmes modules que le contexte accorde).
Gestion complète Projects/Sandboxes (créer/modifier/supprimer/démarrer/arrêter) directement sur le cluster, adoptée par
`app`. `serve --stdio` reste en suspens (`cli#OI-2`, sans client). Reste ouvert : session de configuration en CLI
(`cli#OI-4`), sandbox éphémère pour `script` (`cli#OI-5`). `cli.md` est non commité, comme le reste.

**Bilan du 2026-11-07 (suite).** Le domaine `cli` est complet : `dsd/cli.md` (DEC-1 à DEC-11) et cinq Solutions
`dsd/cli/{hote,chat,batch,sync,objets}.md`, commités sur `main` (non poussés). Points tranchés : contexte dérivé du Project
et de la Sandbox, niveaux utilisateur et `.vanyline/`, secrets dans `$HOME/.config/vanyline/secrets/secrets.yaml`, TUI
conservé (script replié, message injecté pendant un tour, `/agent` retiré), `run` à un tour, `script --param`, codes de sortie
0/1/2/3/130, `sync` par `/ws/fs` brut (ticket miné par la CLI), `apply -f`/`edit` pour modifier, Owner par `--owner` puis
`defaults.owner`. Ouverts : `cli#OI-2/4/5`, `hote#OI-1..4`, `objets#OI-1`. Inventaire : `inventaire-vanyline-lymaille-2026-11.md`.
Reste à faire du plan : domaines `controller`/`health`/`sandbox` (plus tard), session miryad-core, tri de la branche
état-final et correction des passages « yolo » des docs.

## 4. Rappels

- `spikes/` est non suivi par git, désormais interdit par la spec racine. Binaire du spike : cache cargo déjà nettoyé.
- Vision détaillée du développeur (2026-11-03) : consignée dans `dsd/app.md` (BO-1 à BO-4, FR-1 à FR-10).
