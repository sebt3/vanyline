// Tests structurels sur la migration `0002_harness_parity.sql`.
// Aucun serveur de base de données n'est requis : la migration est chargée
// comme texte binaire via `include_str!` et les assertions vérifient sa
// structure (présence des bonnes définitions, absence de deprecated,
// ordre des opérations).

const MIGRATION: &str = include_str!("../migrations/0002_harness_parity.sql");

// --- LLM providers ---

#[test]
fn llm_providers_scoped_by_user() {
    assert!(
        MIGRATION.contains("ADD COLUMN user_id UUID NOT NULL REFERENCES users(id)"),
        "llm_providers should add user_id FK to users"
    );
    assert!(
        MIGRATION.contains("uq_llm_providers_user_name UNIQUE (user_id, name)"),
        "llm_providers should have UNIQUE(user_id, name) constraint"
    );
}

#[test]
fn llm_providers_default_model_dropped() {
    assert!(
        MIGRATION.contains("ALTER TABLE llm_providers DROP COLUMN default_model"),
        "default_model column should be removed from llm_providers"
    );
}

// --- MCP servers ---

#[test]
fn mcp_servers_scoped_by_user() {
    assert!(MIGRATION.contains("ADD COLUMN user_id UUID NOT NULL REFERENCES users(id)"));
    assert!(
        MIGRATION.contains("uq_mcp_servers_user_name UNIQUE (user_id, name)"),
        "mcp_servers should have UNIQUE(user_id, name) constraint"
    );
}

// --- model_profiles ---

#[test]
fn model_profiles_table_shape() {
    assert!(MIGRATION.contains("CREATE TABLE model_profiles"));
    assert!(
        MIGRATION.contains("provider_id UUID NOT NULL REFERENCES llm_providers(id)"),
        "model_profiles should reference llm_providers via provider_id"
    );
    assert!(MIGRATION.contains("model TEXT NOT NULL"));
    assert!(
        MIGRATION.contains("UNIQUE (user_id, name)"),
        "model_profiles should have UNIQUE(user_id, name)"
    );
}

// --- toolsets ---

#[test]
fn toolsets_table_shape() {
    assert!(MIGRATION.contains("CREATE TABLE toolsets"));
    assert!(
        MIGRATION.contains("local_tools JSONB NOT NULL DEFAULT '[]'"),
        "toolsets should have local_tools JSONB default []"
    );
    assert!(
        MIGRATION.contains("mcp JSONB NOT NULL DEFAULT '[]'"),
        "toolsets should have mcp JSONB default []"
    );
    assert!(
        MIGRATION.contains("UNIQUE (user_id, name)"),
        "toolsets should have UNIQUE(user_id, name)"
    );
}

// --- skills ---

#[test]
fn skills_table_shape() {
    assert!(MIGRATION.contains("CREATE TABLE skills"));
    assert!(
        MIGRATION.contains("body TEXT NOT NULL DEFAULT ''"),
        "skills should have body TEXT default ''"
    );
    assert!(
        MIGRATION.contains("UNIQUE (user_id, name)"),
        "skills should have UNIQUE(user_id, name)"
    );
}

// --- agents ---

#[test]
fn agents_table_shape() {
    assert!(MIGRATION.contains("CREATE TABLE agents"));
    assert!(
        MIGRATION.contains("model_profile_id UUID NOT NULL REFERENCES model_profiles(id)"),
        "agents should reference model_profiles via model_profile_id"
    );
    assert!(
        MIGRATION.contains("CHECK (mode IN ('primary', 'subagent', 'all'))"),
        "agents should have CHECK on mode values"
    );
    assert!(
        MIGRATION.contains("toolsets JSONB NOT NULL DEFAULT '[]'"),
        "agents should have toolsets JSONB default []"
    );
    assert!(
        MIGRATION.contains("UNIQUE (user_id, name)"),
        "agents should have UNIQUE(user_id, name)"
    );
}

#[test]
fn agent_mcp_servers_dropped() {
    assert!(
        MIGRATION.contains("DROP TABLE IF EXISTS agent_mcp_servers"),
        "agent_mcp_servers table should be dropped"
    );
}

#[test]
fn agents_fk_constraint_reordering() {
    let drop_constraint = MIGRATION
        .find("DROP CONSTRAINT IF EXISTS conversations_agent_id_fkey")
        .expect("MIGRATION should contain DROP CONSTRAINT conversations_agent_id_fkey");

    let drop_table = MIGRATION
        .find("DROP TABLE IF EXISTS agents")
        .expect("MIGRATION should contain DROP TABLE IF EXISTS agents");

    let add_fk = MIGRATION
        .find("ADD CONSTRAINT conversations_agent_id_fkey")
        .expect("MIGRATION should contain ADD CONSTRAINT conversations_agent_id_fkey");

    assert!(
        drop_constraint < drop_table,
        "DROP CONSTRAINT must come before DROP TABLE agents"
    );
    assert!(
        drop_table < add_fk,
        "DROP TABLE agents must come before ADD CONSTRAINT conversations_agent_id_fkey"
    );
}

// --- Indexes ---

#[test]
fn indexes_present() {
    assert!(
        MIGRATION.contains("CREATE INDEX idx_model_profiles_user_id"),
        "idx_model_profiles_user_id should exist"
    );
    assert!(
        MIGRATION.contains("CREATE INDEX idx_toolsets_user_id"),
        "idx_toolsets_user_id should exist"
    );
    assert!(
        MIGRATION.contains("CREATE INDEX idx_skills_user_id"),
        "idx_skills_user_id should exist"
    );
    assert!(
        MIGRATION.contains("CREATE INDEX idx_agents_user_id"),
        "idx_agents_user_id should exist"
    );
}
