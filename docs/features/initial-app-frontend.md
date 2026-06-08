# Feature : initial-app-frontend (MVP)

## Ce que cette feature fait

MVP complet : auth OIDC, configuration LLM/MCP/agents via API REST, chat LLM avec tool calls MCP, image Podman déployable en K8s.

## Ce qu'elle ne fait PAS

- Pas d'UI de configuration (API + kuberest only)
- Pas d'éditeur de code (CodeMirror)
- Pas de sandbox, pas de controller, pas de Redis

## Interfaces clés et modules touchés

### Backend (Rust — `app/`)

```
app/src/
├── main.rs              # routeur axum, ServeDir, health
├── config.rs            # env vars, codes VNL-CFG-*
├── error.rs             # AppError, codes VNL-*
├── auth/
│   ├── mod.rs           # routes login/callback/logout
│   ├── oidc.rs          # OidcClientTrait (openidconnect 4)
│   ├── cookie.rs        # cookie stateless chiffré (pattern gramophone)
│   └── middleware.rs    # AuthUser + AdminAuth extractors
├── api/
│   ├── me.rs
│   ├── llm_providers.rs # CRUD + /test — admin
│   ├── mcp_servers.rs   # CRUD — admin
│   ├── agents.rs        # CRUD write admin, read OIDC
│   └── conversations.rs # CRUD + messages — OIDC
├── ws/chat.rs           # WebSocket : rig loop + MCP + streaming
├── llm/client.rs        # factory rig (Ollama / openai-compatible)
└── db/                  # PgPool + migrations sqlx
```

### Frontend (Svelte 5 — `frontend/src/`)

Routes hash-based : `#/` → Chat, `#/login` → Login, `#/chat/:id` → Chat.

Seules deux pages : Login.svelte + Chat.svelte.
Chat embarque ConversationList + AgentSelector + zone de messages + ChatInput.

### Auth

Stateless : cookie HttpOnly chiffré (64-byte COOKIE_SECRET).
Cookie stocke `{id_token}|{email}`, validé à chaque requête.

Admin : `Authorization: Bearer {ADMIN_SECRET}` — pour kuberest.

### Providers LLM supportés

- `ollama` : API native via `rig::providers::ollama`, discovery via `GET /api/tags`
- `openai-compatible` : `rig::providers::openai` avec base URL personnalisée (`/v1`), discovery via `GET /v1/models`

### MCP tool calls

`rig-core 0.38` feature `rmcp`. Connexion on-demand par session WebSocket.
Transports supportés : SSE (`http://…/sse`) et HTTP streamable (`http://…/mcp`).

### Historique messages (reprise à froid)

Colonne `payload JSONB` par message — format OpenAI complet (tool_calls, tool_results inclus).
À la connexion WS : chargement des messages en DB → désérialisation → reconstruction contexte rig.

## Modèle de données

6 tables : `users`, `llm_providers`, `mcp_servers`, `agents`, `agent_mcp_servers`, `conversations`, `messages`.

`llm_providers.available_models` : JSONB, populé par le endpoint `/test`.

## Variables d'environnement

| Variable | Obligatoire | Usage |
|----------|-------------|-------|
| `OIDC_ISSUER_URL` | oui | URL de l'issuer OIDC |
| `OIDC_CLIENT_ID` | oui | Client ID |
| `OIDC_CLIENT_SECRET` | oui | Client secret |
| `OIDC_REDIRECT_URL` | oui | URL de callback |
| `COOKIE_SECRET` | oui | 64 bytes, chiffrement cookie |
| `DATABASE_URL` | oui | PostgreSQL |
| `ADMIN_SECRET` | oui | Bearer token admin |
| `LISTEN_ADDR` | non | défaut `0.0.0.0:8080` |
| `STATIC_DIR` | non | défaut `./static` |
| `OIDC_SCOPES` | non | défaut `openid,email,profile` |
| `OIDC_CA_CERT` | non | CA custom pour l'issuer |

## Déploiement

Image : `docker.io/sebt3/vanyline-app:0.0.1-alpha.1`
Build : `podman build` multi-stage (node → rust → debian-slim)
Namespace K8s : `media-station`

Manifestes dans `deploy/` :
- `RestEndPoint_sso.yaml` — kuberest crée l'app OIDC dans Authentik (`media-system`)
- `secret.yaml`, `configmap.yaml`, `deployment.yaml`, `service.yaml`, `ingress.yaml`

## Risques identifiés

1. **openidconnect 4.0 vs 3.5** : API changée depuis gramophone — vérifier le portage auth en premier
2. **rig streaming + tool calls** : valider que `agent.stream_chat()` supporte le streaming pendant tool calls
3. **rmcp SSE vs HTTP streamable** : deux patterns distincts dans rmcp 1.7 — valider sur exemples rig 0.38
