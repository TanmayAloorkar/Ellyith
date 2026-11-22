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
5. Set the startup command to `chmod +x ./aio-bot && ./aio-bot`. A binary uploaded from Windows has no executable bit, so this sets it on every start. Alternatively, run `chmod +x ./aio-bot` once in the panel console if it has a shell, and use `./aio-bot` as the startup command. Wispbyte's exact panel behaviour for this is unverified.

Never commit or paste real values of these variables into the repo.

## 3. First start

Start the server and watch the console. Migrations run automatically on startup. Look for the log line containing "connected as"; that means the bot is logged in to Discord. Startup failures show as panics with `failed to connect to the database` (check `DATABASE_URL`), `failed to run migrations`, or `failed to build the Discord client` (check `DISCORD_TOKEN`). `Error in user data setup` means command registration failed (usually a wrong `GUILD_ID`, or the bot was invited without the `applications.commands` scope). `client stopped` means the gateway connection failed (check the token and intents). In both cases the process exits with code 1, so Wispbyte's panel shows the server as stopped.

If the console shows `Exec format error`, first confirm the uploaded file is the musl ELF built in step 1 (about 19 MB; `file aio-bot` should say "x86-64, statically linked"). If it is, the host is probably ARM. This path is untested. Add the target and rebuild:

    rustup target add aarch64-unknown-linux-musl
    cargo zigbuild --release --target aarch64-unknown-linux-musl

Upload `target/aarch64-unknown-linux-musl/release/aio-bot` in place of the old binary and restart.

## 4. Invite and permissions

In the Discord developer portal, Bot tab, keep "Public Bot" OFF so the bot cannot be added to other servers, and enable the privileged intents Server Members and Message Content.

Invite the bot with the permission integer 1099666910342. It covers: View Channels, Send Messages, Embed Links, Read Message History, Manage Messages, View Audit Log, Manage Nicknames, Kick Members, Ban Members, Moderate Members, Move Members, Mute Members. Invite URL form:

    https://discord.com/oauth2/authorize?client_id=<APPLICATION_ID>&scope=bot%20applications.commands&permissions=1099666910342

After the invite, open Server Settings, Roles, and drag the bot's role above the highest role of each member it must moderate. Discord blocks kicks, bans, timeouts and nickname changes on members whose top role is at or above the bot's.

## Command names

Config commands (Manage Server only):

- `/config modroles add|remove|list`, `/config protected add|remove|list`
- `/config logs set|disable|list`
- `/config dm set|reset|view|showmod`
- `/config ladder set|remove|list`
- `/config decay on|off`
- `/config nickwords add|remove|list`
- `/config nickfilter on|off|hoist|fallback`
- `/config panel <channel>` posts the moderation panel (replaces the previous one)

Moderation commands (mods):

- `/purge` deletes recent messages by count or time

## 5. Smoke test

Run these in the live server:

1. `/ping` replies.
2. `/config logs set mod_actions #mod-log` confirms.
3. `/warn` an alt account. A case appears and an embed lands in `#mod-log`.
4. `/case remove` that case. It is marked removed and no longer counts.
5. `/config panel #channel`, then kick an alt through the panel (Members, Kick). A case appears and the DM and log entry match the slash command.

## 6. Memory check

After about an hour of running, open the panel's resource graph. RAM should sit well under 512 MB. If it climbs steadily, report it, since all caches are meant to be bounded.
