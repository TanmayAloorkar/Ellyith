CREATE TABLE dm_templates (
    guild_id BIGINT NOT NULL,
    kind TEXT NOT NULL,
    template TEXT NOT NULL,
    PRIMARY KEY (guild_id, kind)
);
