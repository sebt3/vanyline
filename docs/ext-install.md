# Installation de l'extension VS Code `vanyline` + test e2e

Procédure d'installation et de validation manuelle de l'extension
`sebt3.vanyline` : chat dans la sidebar secondaire, et un panneau de
configuration (onglet éditeur) pour éditer la config de la CLI — F4.
L'extension n'est pas publiée sur une marketplace : le `.vsix` est attaché à
chaque release GitHub de `sebt3/vanyline` (job `ext` de
`.github/workflows/release.yml`) et s'installe à la main. Cible :
code-server (et VS Code desktop) sur Linux.

Contexte des commandes : répertoire de travail = racine du repo pour
`npm run install:local` ; sinon là où se trouve le `.vsix` téléchargé.

## 1. Objectif

L'extension `sebt3.vanyline` ajoute une vue Chat dans la sidebar secondaire
de code-server/VS Code. Elle est propulsée par un binaire local `vanyline`
qu'elle télécharge et met à jour automatiquement dans
`~/.local/bin/vanyline`, SHA256 vérifié contre l'asset `.sha256` de la
release `sebt3/vanyline` ; à défaut, `vanyline.serverPath` permet de pointer
un binaire fourni manuellement (auto-update alors désactivée).

## 2. Installation manuelle

**Depuis une release GitHub** — télécharger `vanyline-<ver>.vsix` sur la page
des releases de `sebt3/vanyline`, puis :

```bash
code-server --install-extension vanyline-<ver>.vsix    # code-server
code --install-extension vanyline-<ver>.vsix           # VS Code desktop
```

**Depuis le dépôt** (checkout de la branche, ou main) :

```bash
npm ci
npm run install:local
```

`install:local` enchaîne `npm run build --workspace=vanyline` (host esbuild +
webview vite), `npm run package --workspace=vanyline` (`vsce package
--no-dependencies` dans `ext/`, produit `ext/vanyline-<ver>.vsix`) puis
`code-server --install-extension`. Prérequis : build local du workspace
(`npm ci` à la racine suffit — les packages `@vanyline/*` sont des workspaces)
et `code-server` dans le `PATH`.

Après installation, recharger la fenêtre (Command Palette → « Reload
Window ») pour activer l'extension.

## 3. Procédure de test e2e (à faire sur un code-server réel)

Pas de job CI automatisé pour cette partie — le design documente la
procédure, pas un pipeline. Ouvrir un code-server réel, fenêtre sur un
workspace quelconque, et dérouler la checklist :

1. **Première ouverture** → conteneur secondaire « vanyline » avec la vue
   Chat visible ; la status bar affiche « vanyline: démarrage » puis passe à
   « vanyline: prêt ».
2. **Premier lancement sans binaire** (faire `rm ~/.local/bin/vanyline`
   avant) → le téléchargement est visible dans l'OutputChannel « vanyline »
   ; le binaire est posé en `~/.local/bin/vanyline`, mode 755
   (`ls -l ~/.local/bin/vanyline` → `-rwxr-xr-x`).
3. **Réseau coupé + binaire absent** → activation dégradée : status « erreur »,
   message citant la commande `vanyline.restartServer` pour réessayer ;
   l'éditeur reste utilisable, l'activation ne plante pas.
4. **Envoyer un message** → les tokens s'affichent progressivement dans la
   webview, fin propre sur `done` (pas de spinner bloqué, pas de trou dans
   le texte).
5. **Bouton stop pendant un tour** → `chat/cancel` part, le tour s'arrête,
   le statut de la webview revient en état « prêt à renvoyer ».
6. **Menu view/title** : « Nouvelle session » fonctionne (QuickPick non
   requis) ; « Choisir une session » ouvre un QuickPick listant les sessions,
   la reprise affiche l'historique correct.
7. **Sélecteur d'agent** → peuplé depuis `config/agents` (les agents de la
   config CLI apparaissent) ; choisir un agent explicite et envoyer un
   message → l'agent choisi passe sur le message envoyé (visible côté
   OutputChannel / réponse).
8. **`vanyline.openSettings`** → ouvre l'onglet éditeur « vanyline —
   Configuration » (panneau de config F4, pas les paramètres VS Code —
   `serverPath`/`autoUpdateCli`/`defaultLogLevel` restent accessibles via le
   menu Extensions → Extension Settings). Rouvrir la commande alors que
   l'onglet est déjà ouvert doit le révéler, pas en créer un second.
9. **`vanyline.serverPath` pointant un binaire divergent** → le binaire est
   utilisé tel quel, l'OutputChannel loggue explicitement « auto-update
   désactivée » + le chemin utilisé, et **aucun** téléchargement n'a lieu
   (même si `~/.local/bin/vanyline` est absent ou divergent).
10. **Tuer le process CLI** (`pkill -f 'vanyline serve'`) → redémarrage
    automatique avec backoff exponentiel visible (« vanyline: redémarrage
    (n) » dans la status bar), puis retour « vanyline: prêt ».

Tout point rouge est une régression de F3 : la remonter telle quelle
(point n° + symptôme + versions extension/binaire), pas la corriger en
silence pendant le test.

## 4. Test e2e manuel — panneau de configuration (F4)

Pas de job CI pour cette partie non plus — à dérouler sur le code-server réel,
**après la checklist §3**, dans la même fenêtre. Le workspace doit porter un
marqueur `.vanyline/` ou `.git` : sinon les écritures tombent en couche
globale et les badges `workspace` n'apparaîtront pas — ce n'est pas un bug.

1. **Ouverture** : commande `vanyline: Ouvrir les paramètres` → onglet éditeur
   « vanyline — Configuration », nav gauche à 4 groupes (Modèles, Outils,
   Agents, Skills) et écran « Fournisseurs LLM » par défaut. Relancer la
   commande → même onglet révélé (panel unique).
2. **Créer un provider** : « Créer un fournisseur », nom `e2e-prov`, type
   `ollama`, endpoint `http://localhost:11434` → l'entrée apparaît dans la
   liste avec le badge `workspace`.
3. **Chaînage Modèles** : créer un profil de modèle `e2e-model` rattaché au
   provider `e2e-prov` (le sélecteur de provider du formulaire liste
   `e2e-prov`) → la liste « Profils de modèle » le montre, badge `workspace`.
4. **Créer un agent** (le cœur de la procédure design) : onglet Agents, créer
   `e2e-agent` (modèle `e2e-model`, un mode, un prompt) → apparaît dans la
   liste, badge `workspace`.
5. **Côté CLI** : dans un terminal du workspace,

   ```bash
   vanyline agents list
   ```

   doit montrer la ligne `e2e-agent | workspace | …` — la source affichée est
   la même que le badge (même calcul `file_entry_source`). Vérifier aussi sur
   place que le fichier `agents/e2e-agent.md` existe bien sous
   `.vanyline/agents/` et nulle part dans la config globale.
6. **Chat sans rechargement** : retourner dans la vue Chat (sidebar) → le
   sélecteur d'agent contient `e2e-agent` SENS AUCUN rechargement de fenêtre
   ni clic (broadcast `config/changed` après le write de l'étape 4). C'est LE
   point qui valide la tâche 07 ; rouge = régression à remonter telle quelle.
7. **Édition sans perte du body d'un skill** : onglet Skills, créer un skill
   `e2e-skill` avec un body non vide (`# test\nligne`), fermer/réouvrir
   l'onglet si voulu, puis Modifier → le champ Body doit montrer le corps
   existant (lecture `config/skills/get`) ; le sauvegarder sans le toucher
   laisse le fichier `SKILL.md` inchangé
   (`cat .vanyline/skills/e2e-skill/SKILL.md`). Un body vidé ici EST une perte
   de données : c'est le cas que `config/skills/get` protège.
8. **Tester un provider** : sur `e2e-prov`, bouton « Tester » → si Ollama
   tourne sur 11434, liste de modèles dans l'UI ; sinon carte d'erreur citant
   `VNL-RPC-006`. Les deux issues sont vertes, l'erreur doit être lisible
   (timeout 10 s, jamais un spinner infini).
9. **Persistance** : laisser un formulaire ouvert avec du texte, passer sur un
   autre onglet éditeur puis revenir → l'état de la webview est conservé
   (`retainContextWhenHidden`).
10. **Nettoyage** : supprimer les entrées `e2e-*` depuis le panneau (boutons
    Supprimer) puis revérifier `vanyline agents list` (disparu) et le chat
    (disparu du sélecteur, sans reload). Suppression en couche workspace
    uniquement ; un `name` présent aussi en global laisse la global intacte
    (`VNL-RPC-012` sinon sur double suppression — attendu).

Tout point rouge de cette section est une régression de F4 : la remonter
telle quelle (point n° + symptôme + versions extension/binaire), pas la
corriger en silence pendant le test.

## 5. Pannes connues

- **Mismatch `PROTOCOL_VERSION`** (extension et CLI trop éloignées) :
  l'`initialize` échoue proprement avec `VNL-RPC-003` — message actionnable
  « mettez à jour l'extension et la CLI ensemble ». Il n'y a pas de
  négociation de version : c'est voulu, le contrat RPC est un seul numéro
  (`docs/rpc-protocol.md`).
- **Asset `.sha256` absent sur la release** : le téléchargement est
  **refusé** (`VNL-EXT-005`), sans fallback « on fait confiance ». Vérifier
  que la release concernée embarque bien `vanyline-<target>.tar.gz.sha256`
  (produit par le job `upload-cli`, `checksum: sha256`).
- **Offline au premier lancement** : pas de binaire, pas de réseau → mode
  dégradé (voir checklist point 3), pas de crash d'activation. Relancer avec
  `vanyline.restartServer` une fois le réseau revenu.
- **`VNL-EXT-023` dans un écran de config** : l'entrée a disparu entre
  l'affichage et l'action (`get` après relecture introuvable — message
  `VNL-EXT-023: <domain>/<name> introuvable` ; ex. supprimée entre-temps en
  CLI). L'écran affiche l'erreur ; recharger la liste suffit.
- **`VNL-EXT-024` sur « Défaut »** : définir un provider par défaut est un
  concept du web (`app`), pas de la config CLI — le bouton est rejeté par
  conception dans l'extension. Attendu, pas un bug.
