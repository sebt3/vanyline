-- app/migrations/0002_harness_parity.sql
-- WS-2 app-harness-parity : ModelProfile / Toolset / Skill + Agent v2

-- llm_providers : scoping utilisateur, default_model retiré du chemin agent
-- (ModelProfile porte désormais le modèle ; available_models reste pour
-- l'UI de création de profil). Pas de backfill : aucune installation
-- existante à préserver.
ALTER TABLE llm_providers ADD COLUMN user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE llm_providers ADD CONSTRAINT uq_llm_providers_user_name UNIQUE (user_id, name);
ALTER TABLE llm_providers DROP COLUMN default_model;

-- mcp_servers : scoping utilisateur
ALTER TABLE mcp_servers ADD COLUMN user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE mcp_servers ADD CONSTRAINT uq_mcp_servers_user_name UNIQUE (user_id, name);

-- model_profiles : seul objet qu'un agent référence pour son modèle
CREATE TABLE model_profiles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    provider_id UUID NOT NULL REFERENCES llm_providers(id) ON DELETE RESTRICT,
    model TEXT NOT NULL,
    temperature DOUBLE PRECISION,
    max_tokens BIGINT,
    options JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, name)
);

-- toolsets : groupe d'outils MCP + fragment de prompt
CREATE TABLE toolsets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    prompt TEXT,
    local_tools JSONB NOT NULL DEFAULT '[]',
    mcp JSONB NOT NULL DEFAULT '[]', -- [{server, tools[]}]
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, name)
);

-- skills : index léger (name+description) + body lazy-loadé séparément
CREATE TABLE skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, name)
);

-- agents v2 : refonte complète (pas de données existantes à préserver)
ALTER TABLE conversations DROP CONSTRAINT IF EXISTS conversations_agent_id_fkey;
DROP TABLE IF EXISTS agent_mcp_servers;
DROP TABLE IF EXISTS agents;

CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    mode TEXT NOT NULL DEFAULT 'primary' CHECK (mode IN ('primary', 'subagent', 'all')),
    model_profile_id UUID NOT NULL REFERENCES model_profiles(id) ON DELETE RESTRICT,
    toolsets JSONB NOT NULL DEFAULT '[]', -- noms de toolsets
    skills JSONB NOT NULL DEFAULT '"auto"', -- "auto" | "none" | [noms]
    system_prompt TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, name)
);

ALTER TABLE conversations
    ADD CONSTRAINT conversations_agent_id_fkey
    FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE SET NULL;

CREATE INDEX idx_model_profiles_user_id ON model_profiles(user_id);
CREATE INDEX idx_toolsets_user_id ON toolsets(user_id);
CREATE INDEX idx_skills_user_id ON skills(user_id);
CREATE INDEX idx_agents_user_id ON agents(user_id);
