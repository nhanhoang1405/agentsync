-- Give every stored file a monotonic revision that clients can display and
-- compare. History privacy is enforced in Postgres as a final safety net.
ALTER TABLE agentsync_resources
    ADD COLUMN IF NOT EXISTS sync_version BIGINT NOT NULL DEFAULT 1;

UPDATE agentsync_resources
SET visibility = 'private'
WHERE kind = 'histories' AND visibility <> 'private';

DO $migration$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'agentsync_resources_history_privacy_check'
          AND conrelid = 'agentsync_resources'::regclass
    ) THEN
        ALTER TABLE agentsync_resources
            ADD CONSTRAINT agentsync_resources_history_privacy_check
            CHECK (kind <> 'histories' OR visibility = 'private');
    END IF;
END
$migration$;
