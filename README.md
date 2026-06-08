# vanyline

Environnement de développement cloud-native, multi-utilisateur et piloté par l'IA, construit pour Kubernetes.

## À quoi ça sert

vanyline offre des espaces de travail isolés dans des pods Kubernetes. Chaque développeur dispose d'un éditeur web complet et d'un assistant LLM qui a accès aux mêmes outils, fichiers et commandes que lui — pas une simulation, le vrai shell.

Les toolchains (Rust, Node, Go, Python…) sont composées à la volée à partir de définitions déclaratives : pas de rebuild d'image, pas de configuration manuelle. Tu déclares les outils dont tu as besoin, le pod les monte au démarrage.

## Pour qui

Pour les développeurs qui font tourner un cluster Kubernetes et veulent :
- des environnements de dev reproductibles et isolés par projet
- un assistant IA qui opère dans le même contexte qu'eux (accès réel au code, aux commandes, aux fichiers)
- une gestion multi-utilisateur sans friction

## Composants

| Composant | Rôle |
|-----------|------|
| **frontend** | Éditeur de code web + interface de conversation LLM |
| **app** | Backend : authentification OIDC, sessions, orchestration LLM, API de configuration |
| **sandbox** | Pod Kubernetes embarquant un serveur WebSocket/MCP — accès réel au code et aux commandes |
| **controller** | Opérateur K8s gérant les ressources Application, Owner et Sandbox |

## État du projet

En développement actif. Pas encore utilisable en production.
