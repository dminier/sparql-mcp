# sparql-mcp-viewer

A live 3D view of the knowledge graph an agent is building with `sparql-mcp`.
It connects to the server's viewer side-channel and grows the graph node by
node as the agent writes, instead of waiting for an export.

What it shows:

- **Classes** (anything typed `owl:Class`, `rdfs:Class`, `rdf:Property`,
  `owl:ObjectProperty` or `owl:DatatypeProperty`) as large glowing orange
  nodes, labelled; **instances** as small blue nodes. Literals are not nodes:
  they are attributes of their subject.
- Edges as lines, styled by kind: `rdf:type`, `rdfs:subClassOf`, and
  everything else.
- **What the agent just did**: the nodes touched by the latest write pulse
  green for two seconds, and the activity feed (bottom left) lists the last
  messages — `snapshot`, `insert`, `delete`, `changed`, `resync`,
  `other graph` — with their triple counts.
- A **resync banner** while the connection is not streaming (connecting,
  waiting for a snapshot, or retrying after a disconnect).

The layout is a force simulation (Barnes-Hut, multi-threaded). New nodes
appear next to a connected neighbor; existing nodes never jump, and graph
changes never move the camera.

## Enable the side-channel on the server

The channel is off by default. In `sparql-mcp.toml`:

```toml
[viewer]
enabled = true
bind    = "127.0.0.1:7878"
```

Then run the server as usual (`sparql-mcp serve`, or let your agent spawn it).
It adds one loopback TCP listener next to its STDIO loop; nothing else changes.

## Build (Windows)

With a Windows `rustup` toolchain (`x86_64-pc-windows-msvc`):

```powershell
cargo build --release -p sparql-mcp-viewer
```

The viewer depends on the `sparql-mcp` crate for its wire types, which pulls
in Oxigraph and RocksDB. RocksDB's bindings need `libclang` at build time:
install LLVM (`winget install LLVM.LLVM`) if the build stops on
`Unable to find libclang`.

For faster local iteration, link Bevy dynamically (never enabled by
`--all-features`, so CI is unaffected):

```powershell
cargo run -p sparql-mcp-viewer --features bevy/dynamic_linking
```

The Bevy Linux system packages installed in `.github/workflows/ci.yml` are
needed only by the CI's Linux lint-and-test job, not for a Windows build.

## Run

```text
sparql-mcp-viewer [--connect <ADDR>] [--config <PATH>] [--connect-timeout <SECS>]
                  [--headless] [--diagnostics]
```

| Flag | Meaning |
|---|---|
| `--connect <ADDR>` | Connect to this address, ignoring any config. |
| `--config <PATH>` | Read `[viewer] bind` from this file (default: `sparql-mcp.toml` if it exists; a missing explicit path is an error). Falls back to `127.0.0.1:7878`. |
| `--connect-timeout <SECS>` | TCP connect timeout, default 10. |
| `--headless` | No window: print `<kind> nodes=<n> edges=<m>` per message; exit with code 2 if the connection never reaches `Streaming` within the timeout. |
| `--diagnostics` | Log FPS and frame time once per second. |

To try it without an agent, the server crate ships a load generator that
serves the `smc:` ontology and then streams typed instances:

```powershell
cargo run --release -p sparql-mcp --example viewer_load -- --count 500 --batch 25
cargo run --release -p sparql-mcp-viewer -- --connect 127.0.0.1:7878
```

`--pattern-delete N` makes it end with a pattern-based `DELETE … WHERE`, which
the server can only report as a coarse `changed` event — the viewer then
resyncs from a fresh snapshot.

## Controls

| Input | Action |
|---|---|
| Right drag | Orbit |
| Left drag | Pan |
| Mouse wheel | Zoom |
| `F` | Frame the whole graph (this also stops any move the agent has under way) |
| Click a node | Select it: label, IRI (with a copy button), attributes, incoming and outgoing edges |
| Click an edge in the panel | Select the node at its other end |
| `Escape` | One step per press: clear the selection; else give back the view the agent changed; else clear the agent's highlight |
| *Agent view · Restore my view* (top left, shown while the agent has something up) | Clear the agent's highlight and give back the selection and class filter you had |
| Class panel (top left) | Toggle which classes' instances are shown; filtered-out nodes fade but keep their place |

## Controlled by the agent

With `[viewer] enabled` the server's `viewer_*` tools reach this viewer over the side-channel
(`docs/designs/sparql-mcp-viewer-agent-control.md`):

*   `viewer_show` makes nodes glow cyan. In *isolate* mode everything else fades, as do the edges that
    do not join two of them. It can also select a node, which opens its panel.
*   `viewer_filter` changes the same class filter as the panel on the right, so you can undo it there.
*   `viewer_clear` drops the highlight, and with `restore` gives your view back.
*   The activity feed lists each applied command as `agent highlight`, `agent isolate`,
    `agent filter`, `agent clear` or `agent restore`.

The agent never locks the interface. What you change is counted (`user.epoch`) and reported to the
agent, which is told to pass it back so that it does not overwrite your last move. The camera is moved by the
agent with an eased animation (`viewer_camera`, and `viewer_show` with `focus`); the moment you orbit,
pan, zoom or press `F`, the animation stops and the agent is told. Search and screenshots are not
built yet. A `--headless` viewer answers `status` and refuses the rest.

## Security

The side-channel listens on `127.0.0.1` only and has **no authentication**.
Any process on the same machine — including other local users' processes on a
shared machine — can connect and read the full contents of the active project
graph. That is acceptable for a single-operator workstation tool; do not
enable `[viewer]` on a machine with untrusted local users.

The viewer itself never listens on a port and writes nothing to disk. The
clipboard receives an IRI only when you click "copy IRI".

## Performance

Measured in release builds on the author's machine (NVIDIA GeForce RTX 4070,
Intel Core i7-14700K), driven by `viewer_load` with 5,000 instances:

- rendering: about 150 FPS (6.6 ms per frame; the target is 60 FPS);
- layout: 2.1 ms per step at 5,000 nodes and 10,000 edges (target: 4 ms),
  from `cargo test --release -p sparql-mcp-viewer -- --ignored layout_budget_5000`.
