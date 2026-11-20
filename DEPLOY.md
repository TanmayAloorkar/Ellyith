# Deploying to Wispbyte

The bot runs on Wispbyte's Rust "compiled binaries" runtime (512 MB RAM, 1 GB disk) with the host-managed PostgreSQL database. All state lives in PostgreSQL.

## 1. Build

Install the toolchain once: Rust stable, `rustup target add x86_64-unknown-linux-musl`, `cargo install cargo-zigbuild`, and zig on your PATH. Then, from the repo root:

    cargo zigbuild --release --target x86_64-unknown-linux-musl

The output is `target/x86_64-unknown-linux-musl/release/aio-bot`, a static, stripped Linux x86-64 ELF of about 19 MB. It uses rustls only, so it needs no OpenSSL on the host.

## 2. Wispbyte setup

1. Create a server with the Rust runtime.
2. Create the PostgreSQL database in the panel and copy its connection URL.
3. Set these environment variables in the panel:
   - `DISCORD_TOKEN`: the bot token from the Discord developer portal.
   - `DATABASE_URL`: the Wispbyte PostgreSQL URL from step 2.
   - `GUILD_ID`: the ID of the live server (enable Developer Mode in Discord, right-click the server, Copy Server ID).
   - `RUST_LOG=info`
4. Upload `aio-bot` to the server's file manager.
5. Set the startup command to `./aio-bot`. If the panel has no executable bit option and the console says permission denied, run `chmod +x aio-bot` from the console, if offered, or re-upload with execute permission.

Never commit or paste real values of these variables into the repo.

## 3. First start

Start the server and watch the console. Migrations run automatically on startup. Look for the log line containing "connected as"; that means the bot is logged in to Discord.

If the console shows `Exec format error`, the host is ARM. Add the target and rebuild:

    rustup target add aarch64-unknown-linux-musl
    cargo zigbuild --release --target aarch64-unknown-linux-musl

Upload `target/aarch64-unknown-linux-musl/release/aio-bot` in place of the old binary and restart.

## 4. Invite and permissions

In the Discord developer portal, Bot tab, enable the privileged intents Server Members and Message Content.

Invite the bot with the permission integer 1099666910342. It covers: View Channels, Send Messages, Embed Links, Read Message History, Manage Messages, View Audit Log, Manage Nicknames, Kick Members, Ban Members, Moderate Members, Move Members, Mute Members. Invite URL form:

    https://discord.com/oauth2/authorize?client_id=<APPLICATION_ID>&scope=bot%20applications.commands&permissions=1099666910342

After the invite, open Server Settings, Roles, and drag the bot's role above every role of the members it must moderate. Discord blocks kicks, bans, timeouts and nickname changes on members whose top role is at or above the bot's.

## 5. Smoke test

Run these in the live server:

1. `/ping` replies.
2. `/config logs set mod_actions #mod-log` confirms.
3. `/warn` an alt account. A case appears and an embed lands in `#mod-log`.
4. `/case remove` that case. It is marked removed and no longer counts.

## 6. Memory check

After about an hour of running, open the panel's resource graph. RAM should sit well under 512 MB. If it climbs steadily, report it, since all caches are meant to be bounded.
