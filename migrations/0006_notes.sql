CREATE TABLE notes (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    target_id BIGINT NOT NULL,
    author_id BIGINT NOT NULL,
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX notes_target ON notes (guild_id, target_id, id DESC);
