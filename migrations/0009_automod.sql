CREATE TABLE automod_rules (
    guild_id BIGINT NOT NULL,
    kind TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    action TEXT NOT NULL,
    timeout_secs BIGINT NOT NULL,
    threshold INT NOT NULL,
    window_secs INT NOT NULL,
    percent INT NOT NULL,
    extra INT NOT NULL,
    ignored_roles BIGINT[] NOT NULL DEFAULT '{}',
    ignored_channels BIGINT[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (guild_id, kind)
);

CREATE TABLE automod_words (
    guild_id BIGINT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('whole','wildcard','regex')),
    pattern TEXT NOT NULL,
    PRIMARY KEY (guild_id, kind, pattern)
);

CREATE TABLE automod_sites (
    guild_id BIGINT NOT NULL,
    site TEXT NOT NULL,
    PRIMARY KEY (guild_id, site)
);

CREATE TABLE automod_scam_domains (
    domain TEXT PRIMARY KEY
);
