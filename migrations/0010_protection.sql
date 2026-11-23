CREATE TABLE protection_settings (
    guild_id BIGINT PRIMARY KEY,
    raid_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    raid_joins INT NOT NULL DEFAULT 10,
    raid_secs INT NOT NULL DEFAULT 10,
    raid_responses TEXT[] NOT NULL DEFAULT '{alert,quarantine}',
    raid_filter TEXT[] NOT NULL DEFAULT '{}',
    raid_filter_days INT NOT NULL DEFAULT 7,
    raid_end_mins INT NOT NULL DEFAULT 10,
    alt_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    alt_days INT NOT NULL DEFAULT 7,
    alt_pts_age INT NOT NULL DEFAULT 40,
    alt_pts_avatar INT NOT NULL DEFAULT 20,
    alt_pts_name INT NOT NULL DEFAULT 40,
    alt_pts_invite INT NOT NULL DEFAULT 30,
    alt_threshold INT NOT NULL DEFAULT 60,
    alt_response TEXT NOT NULL DEFAULT 'flag' CHECK (alt_response IN ('flag','quarantine')),
    gate_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    gate_days INT NOT NULL DEFAULT 7,
    gate_action TEXT NOT NULL DEFAULT 'quarantine' CHECK (gate_action IN ('flag','quarantine','kick','ban')),
    nuke_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    nuke_window_secs INT NOT NULL DEFAULT 10,
    nuke_bans INT NOT NULL DEFAULT 3,
    nuke_kicks INT NOT NULL DEFAULT 3,
    nuke_chan_del INT NOT NULL DEFAULT 2,
    nuke_role_del INT NOT NULL DEFAULT 2,
    nuke_webhooks INT NOT NULL DEFAULT 3,
    nuke_perms INT NOT NULL DEFAULT 1
);

CREATE TABLE protection_raids (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ends_at TIMESTAMPTZ NOT NULL,
    ended_at TIMESTAMPTZ,
    alert_channel BIGINT,
    alert_message BIGINT,
    started_lockdown BOOLEAN NOT NULL DEFAULT FALSE
);
-- One active raid per guild.
CREATE UNIQUE INDEX protection_raids_active ON protection_raids (guild_id) WHERE ended_at IS NULL;

CREATE TABLE protection_raiders (
    raid_id BIGINT NOT NULL REFERENCES protection_raids (id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL,
    account_created TIMESTAMPTZ NOT NULL,
    default_avatar BOOLEAN NOT NULL,
    PRIMARY KEY (raid_id, user_id)
);

CREATE TABLE protection_invites (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    invite_code TEXT,
    inviter_id BIGINT,
    PRIMARY KEY (guild_id, user_id)
);

CREATE TABLE protection_bans (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    username TEXT NOT NULL,
    display_name TEXT NOT NULL,
    invite_code TEXT,
    banned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX protection_bans_banned_at ON protection_bans (banned_at);

CREATE TABLE protection_trusted (
    guild_id BIGINT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('user','role')),
    target_id BIGINT NOT NULL,
    PRIMARY KEY (guild_id, kind, target_id)
);

CREATE TABLE protection_strips (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    roles BIGINT[] NOT NULL,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    restored BOOLEAN NOT NULL DEFAULT FALSE
);
