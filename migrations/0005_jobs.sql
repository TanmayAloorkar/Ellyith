CREATE TABLE scheduled_jobs (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    kind TEXT NOT NULL,
    target_id BIGINT NOT NULL,
    run_at TIMESTAMPTZ NOT NULL,
    done BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX scheduled_jobs_pending ON scheduled_jobs (run_at) WHERE NOT done;
