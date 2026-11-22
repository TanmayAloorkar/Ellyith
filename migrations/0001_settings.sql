CREATE TABLE guild_settings (
    guild_id BIGINT PRIMARY KEY,
    mod_roles BIGINT[] NOT NULL DEFAULT '{}',
    protected_roles BIGINT[] NOT NULL DEFAULT '{}',
    dm_show_moderator BOOLEAN NOT NULL DEFAULT false,
    warn_decay_enabled BOOLEAN NOT NULL DEFAULT false,
    warn_decay_days INT NOT NULL DEFAULT 30,
    filtered_words TEXT[] NOT NULL DEFAULT '{}',
    hoist_chars TEXT NOT NULL DEFAULT '!',
    nick_fallback TEXT NOT NULL DEFAULT 'Moderated Nickname',
    nick_filter_enabled BOOLEAN NOT NULL DEFAULT true
);

CREATE TABLE ladder_rules (
    guild_id BIGINT NOT NULL,
    warn_count INT NOT NULL CHECK (warn_count >= 1),
    action TEXT NOT NULL CHECK (action IN ('timeout', 'kick', 'tempban', 'ban')),
    duration_secs BIGINT NULL,
    PRIMARY KEY (guild_id, warn_count)
);
