-- app/migrations/0006_chat_contexts.sql
-- Modèle de contexte de conversation (WS chat-app-fonctionnel, axe 1),
-- extensible au-delà de la sandbox (cf. docs/features/chat-app-fonctionnel.md).
-- Pas de backfill : aucune installation existante à préserver.

CREATE TABLE chat_contexts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind TEXT NOT NULL,
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE conversations ADD COLUMN context_id UUID NOT NULL REFERENCES chat_contexts(id);

CREATE INDEX idx_chat_contexts_kind ON chat_contexts(kind);
CREATE INDEX idx_conversations_context_id ON conversations(context_id);
