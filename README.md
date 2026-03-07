# Glebin

`glebin` is a small Rust workspace for experimenting with tick-based multiplayer servers.
The workspace now has three clear roles:

- `glebin-server`: async TCP game server with a fixed tick loop, server-owned world rules, public and private chat routing, shared snapshots, and a temporary chat audit log
- `glebin-client`: `ratatui` terminal client for moving around a shared 2D world, chatting, whispering, and seeing other players live
- `glebin-protocol` (stored in `glob/`): shared message, world, chat, and snapshot types used by both binaries

## World Model

The server owns a bounded tile grid and assigns each connected player a unique glyph and default callsign.
Clients send movement intents, display names, and chat messages; the server stays authoritative for positions, collisions, scoring, and world events.

The default world includes:

- solid ruins and tree clusters that block movement
- scenic props like lanterns and shallow pools
- collectible `star shard` entities that respawn and increase player score

## TUI Features

The client now renders:

- the shared world grid centered on the local player
- a live player roster with names, positions, and scores
- a chat window for player, system, and whisper messages
- a chat input panel
- a persistent per-player color theme for glyphs, roster entries, chat labels, and UI chrome
- an explicit opaque background for terminals using transparency
- lightweight client-side interpolation so player motion appears less jumpy between snapshots
- recent chat history replayed on connect so late joiners can catch up

Controls:

- move: arrow keys or `WASD`
- open chat: `Enter`, `/`, or `c`
- send chat: `Enter` while in chat mode
- whisper: `/w <name> <message>`
- reply to last whisper: `/r <message>`
- cancel chat: `Esc`
- quit: `q`

## Protocol

The server and client communicate with newline-delimited JSON.

Client messages:

- `move`: movement intent, such as `{"type":"move","dx":1,"dy":0}`
- `set_name`: update display name
- `send_chat`: send a public chat message or slash-command chat action such as whisper/reply

Server messages:

- `welcome`: assigned player id, glyph, display name, color, world config, and tick rate
- `snapshot`: current tick plus all players and collectibles
- `chat`: public chat, whispers, or system events such as joins, renames, and pickups
- `error`: protocol validation errors

## Running

Start the server:

```bash
cargo run -p glebin-server
```

The server prints the path to a temporary chat audit log on startup. Public chat, system events, and whispers are appended there for administrative review during that run.

Connect one or more TUI clients:

```bash
cargo run -p glebin-client -- --connect 127.0.0.1:9132 --name andrew
```

## Testing

```bash
cargo test
```
