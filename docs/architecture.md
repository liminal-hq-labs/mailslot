# Architecture

Mailslot is a library with thin shells on top. `mailslot-core` has no GTK or UI dependency, and every interface drives it through one `Engine`.

## Layers

```text
┌─────────────┐ ┌─────────────┐ ┌──────────────────┐ ┌───────────────────────┐
│ mailslot    │ │ mailslot-cli│ │ mailslotd        │ │ your Rust app         │
│ (GTK4 app)  │ │             │ │ (mailslot-ipc)   │ │ (Tauri, Iced, …)      │
└──────┬──────┘ └──────┬──────┘ └────────┬─────────┘ └──────────┬────────────┘
       │               │                 │ ▲ JSON-RPC           │
       │               │                 │ │ on a Unix socket   │
       │               │        ┌────────┴─┴───────┐            │
       │               │        │ Qt, WinUI, web,  │            │
       │               │        │ scripts (any     │            │
       │               │        │ language)        │            │
       │               │        └──────────────────┘            │
       │   commands ▼  │   events ▲                             │
       └───────────────┴─────────────────────────┬──────────────┘
                              ┌──────────────────┴─────────────────┐
                              │ Engine  (mailslot-core)            │
                              ├────────┬────────┬─────────┬────────┤
                              │ server │ client │ portmap │ config │
                              │ queue  │ watch  │ mdns    │        │
                              └────────┴────────┴─────────┴────────┘
                                 tokio · axum · reqwest · igd-next
```

| Crate | Kind | Contents |
|---|---|---|
| `mailslot-core` | Library, no GTK | `engine`, `proto` (HTTP contract types), `server`, `client`, send queue and folder watch, `config`, `portmap`, mDNS discovery, `netinfo`, `naming`, `token`, `snippets`, `events` |
| `mailslot-ipc` | Library and binary `mailslotd`, no GTK | JSON-RPC 2.0 over a Unix socket, the headless daemon |
| `mailslot-cli` | Binary `mailslot-cli` | `send`, `serve`, `watch`, `pair`, `status`, man page and shell completions |
| `mailslot-gtk` | Binary `mailslot` | Share page, Send page, Preferences, link card, history, notifications |

### Feature flags

`mailslot-core` has `server`, `client`, `portmap`, `tls` and `discovery` features, so an embedder pulls in only what it uses. A client-only program does not link axum. A receiver does not link the upload client.

## The Engine contract

The engine is everything a frontend needs.

```rust
let engine = Engine::builder()
    .config(config)                    // explicit config, no hidden global state
    .runtime(tokio::runtime::Handle::current())   // or .own_runtime() to build one
    .build()?;

let mut events = engine.events();      // Stream<Item = Event>
let snapshot = engine.state();         // current State, for first paint

engine.start().await?;                 // begin sharing
engine.rotate_token().await?;          // new link, old one dies at once
engine.send(SendRequest::new(files, link)).await?;
engine.pause().await;               // keep the link, answer 503
engine.resume().await;
engine.stop().await;
```

- **Commands** are async methods: `start`, `stop`, `pause`, `resume`, `rotate_token`, `send`, queue control, plus configuration updates.
- **Events** form a typed stream: upload started, progress, finished, failed, rejected, sharing state changes, port-mapping state, send-queue progress. Progress is throttled to 10 Hz per transfer. Terminal events are never dropped.
- **State** is a snapshot that always agrees with the event stream, so a frontend can render first and then apply events.
- The engine owns no global state and creates no hidden runtime. It takes a `tokio::runtime::Handle`, or builds its own only when asked.
- All I/O is async. The public API is async or returns streams.

The exact types are in the rustdoc of `mailslot-core`, and `examples/embed.rs` is a complete program that uses them.

## Threading in the GTK app

```text
GTK main thread                      tokio runtime
───────────────                      ─────────────
button click ── engine command ────▶ server / client / portmap tasks
glib::spawn_future_local ◀──────────  Engine event stream
  └─▶ update gio::ListStore, toasts, notifications
```

GObjects never leave the main thread. The app consumes the event stream with `glib::spawn_future_local` and turns events into list-store updates, never blocking the main loop.

## Writing a new frontend

1. Add `mailslot-core` as a dependency with only the features you need.
2. Create the engine with a config. Decide where the runtime comes from: pass your application's `Handle`, or ask the engine to build one.
3. Subscribe to `engine.events()` and render `engine.state()` first. Update your model from events on whichever thread your toolkit requires. Do not hold toolkit objects inside the engine's tasks.
4. Map user actions to engine commands. Never reach into the server or client directly.
5. Show errors from `start()` (port in use, destination not writable) in your own interface. The error type says what to suggest, such as the next free port.
6. On exit, call `stop()` so the UPnP mapping is removed and partials are cleaned up.
7. Add a CI check that your frontend builds against `mailslot-core` alone. The `core-embed` job does this for the engine itself.

### A frontend in another language

Run `mailslotd` (or embed `mailslot-ipc` in your own process) and talk to it over its Unix socket. The protocol is JSON-RPC 2.0, one JSON object per line, and the methods mirror the engine commands. The full method list is in the [specification](../SPEC.md#local-ipc-and-mailslotd).

1. Connect to `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/mailslotd.sock`. Only your user can.
2. Call `state` and render it.
3. Call `subscribe`, then apply each `event` notification to your model.
4. Map user actions to methods: `start`, `stop`, `pause`, `resume`, `rotate_token`, `send`, `settings.*`, `internet.set`.
5. Show engine errors (code `-32000`) with their message, and treat the other codes as bugs in your client.

A minimal client in Python:

```python
import json, os, socket

path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "ca.liminalhq.mailslot", "mailslotd.sock")
sock = socket.socket(socket.AF_UNIX)
sock.connect(path)
stream = sock.makefile("rw", encoding="utf-8", newline="\n")

def call(id, method, params=None):
    stream.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params or {}}) + "\n")
    stream.flush()

call(1, "state")
call(2, "subscribe")  # then events arrive as {"method": "event", "params": …}
for line in stream:
    message = json.loads(line)
    if message.get("method") == "event":
        print("event", message["params"])
    else:
        print("reply", message)
```

Run `mailslotd` as a service with `systemctl --user enable --now mailslotd` (the unit is `data/mailslotd.service`). A Qt, WinUI or web frontend then needs no Rust. UniFFI and C ABI bindings for in-process use are on the [roadmap](../SPEC.md#sync-roadmap).

## Embedding in Tauri

Wrap the engine in commands and forward events to the Tauri event channel.

```rust
struct MailslotState(mailslot_core::Engine);

#[tauri::command]
async fn start_sharing(state: tauri::State<'_, MailslotState>) -> Result<(), String> {
    state.0.start().await.map_err(|e| e.to_string())
}

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let engine = mailslot_core::Engine::builder()
        .runtime(tauri::async_runtime::handle().inner().clone())
        .build()?;
    let mut events = engine.events();
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            let _ = handle.emit("mailslot://event", &event);
        }
    });
    app.manage(MailslotState(engine));
    Ok(())
}
```

Reuse Tauri's runtime handle so there is one runtime in the process. Events are serializable, so the web view receives them as JSON. Call `stop()` from the `RunEvent::Exit` handler.

## Data and files

- Config `$XDG_CONFIG_HOME/ca.liminalhq.mailslot/config.toml`, mode `0600`.
- Data `$XDG_DATA_HOME/ca.liminalhq.mailslot/` with `history.jsonl` and `tls/`.
- The default destination is `~/Downloads/Mailslot`.

## Error handling

Core uses `thiserror` enums. Binaries use `anyhow` at their edges. The GUI shows start failures in an `adw::Banner` with an action, transient problems in an `adw::Toast` and destructive confirmations in an `adw::AlertDialog`. Aborted non-resumable uploads always delete their `.part` file through a guard that runs on drop.

## Boundaries enforced in CI

- `core-embed` builds `mailslot-core` and runs its embed example with no GTK installed.
- clippy `disallowed-methods` and `await_holding_lock` keep blocking calls out of async paths.
- `check-style.sh` keeps colour literals out of the GTK crate.
