CREATE TABLE log_channels (
    guild_id BIGINT NOT NULL,
    category TEXT NOT NULL,
    channel_id BIGINT NOT NULL,
    PRIMARY KEY (guild_id, category)
);
