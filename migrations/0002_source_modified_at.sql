-- Preserve the source machine's filesystem timestamp across synchronization.
-- Existing rows remain NULL and use agent-specific content metadata as a
-- best-effort fallback during pull.
ALTER TABLE agentsync_resources
    ADD COLUMN IF NOT EXISTS source_modified_at TIMESTAMPTZ;
