CREATE TABLE safety_settings (
    guild_id BIGINT PRIMARY KEY,
    quarantine_role_id BIGINT,
    review_channel_id BIGINT,
    deny_action TEXT NOT NULL DEFAULT 'kick' CHECK (deny_action IN ('kick','ban')),
    verify_mode TEXT NOT NULL DEFAULT 'off' CHECK (verify_mode IN ('off','button','quiz')),
    verify_review TEXT NOT NULL DEFAULT 'staff' CHECK (verify_review IN ('staff','auto')),
    verify_channel_id BIGINT,
    verify_roles BIGINT[] NOT NULL DEFAULT '{}',
    verify_description TEXT NOT NULL DEFAULT 'Press the button below to unlock the server. It only takes a moment.',
    verify_message_id BIGINT,
    quiz_questions TEXT[] NOT NULL DEFAULT '{}',
    quiz_answers TEXT[] NOT NULL DEFAULT '{}',
    lockdown_channels BIGINT[] NOT NULL DEFAULT '{}',
    lockdown_message TEXT NOT NULL DEFAULT 'Channel locked while staff deal with a problem.'
);

CREATE TABLE quarantines (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    reason TEXT NOT NULL CHECK (reason IN ('verification','manual','raid','alt')),
    saved_roles BIGINT[] NOT NULL,
    answers TEXT[] NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    review_channel_id BIGINT,
    review_message_id BIGINT,
    PRIMARY KEY (guild_id, user_id)
);

CREATE TABLE lockdown_channels (
    guild_id BIGINT NOT NULL,
    channel_id BIGINT NOT NULL,
    posted_message BOOLEAN NOT NULL,
    PRIMARY KEY (guild_id, channel_id)
);

CREATE TABLE lockdown_overwrites (
    guild_id BIGINT NOT NULL,
    channel_id BIGINT NOT NULL,
    target_id BIGINT NOT NULL,
    is_member BOOLEAN NOT NULL,
    allow BIGINT NOT NULL,
    deny BIGINT NOT NULL,
    PRIMARY KEY (guild_id, channel_id, target_id, is_member),
    FOREIGN KEY (guild_id, channel_id) REFERENCES lockdown_channels (guild_id, channel_id) ON DELETE CASCADE
);
