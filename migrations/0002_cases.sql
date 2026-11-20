CREATE TABLE case_counters (
    guild_id BIGINT PRIMARY KEY,
    next INT NOT NULL
);

CREATE TABLE cases (
    guild_id BIGINT NOT NULL,
    case_number INT NOT NULL,
    kind TEXT NOT NULL,
    target_id BIGINT NOT NULL,
    actor TEXT NOT NULL CHECK (actor IN ('moderator', 'auto_punishment', 'system')),
    moderator_id BIGINT NULL,
    reason TEXT NULL,
    duration_secs BIGINT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NULL,
    dm_failed BOOLEAN NOT NULL DEFAULT false,
    removed_at TIMESTAMPTZ NULL,
    removed_by BIGINT NULL,
    PRIMARY KEY (guild_id, case_number),
    CHECK ((actor = 'moderator') = (moderator_id IS NOT NULL))
);

CREATE INDEX cases_target_idx ON cases (guild_id, target_id);
