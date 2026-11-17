# idea-chat

An exploration toward an open-source Discord alternative built on the idealyst
framework. Chat, voice, video — cross-platform, centralized server, no P2P.

Start with **[docs/framework-gap-analysis.md](docs/framework-gap-analysis.md)**.
It audits what the SDK does and does not provide, and §9 is the current plan.

## Where things stand

Text chat is fully served by existing SDKs. Voice needs one thing the framework
does not have — a **native `AudioStream` sink** — and the whole point of this
repo right now is to prove that by building around it.

Everything here lives outside the `idealyst-native` tree on purpose. What earns
promotion into the SDK gets decided after it works, not before.

## Hearth — the app

`crates/hearth` is one crate compiled two ways. By default it builds the
client: the UI from `Hearth.dc.html`, plus HTTP stubs for every `#[server]`
function. Under `--features server` it builds the other half — the real
function bodies over `crates/hearth-db` — and `src/bin/server.rs` serves the
API and the wasm bundle from one origin, which the session cookie requires.

```bash
docker compose up -d postgres redis
docker exec idea-chat-postgres-1 psql -U postgres -c 'CREATE DATABASE hearth_dev'

cargo build -p hearth --bin server --features server
(cd crates/hearth && idealyst build --web)
bash tools/reset-dev.sh          # fresh instance on :8090, prints a claim link
```

Open <http://127.0.0.1:8090>. A fresh instance has no accounts and no default
password: the server prints a one-time claim link, and the first person to
paste it becomes the instance admin. After that it is invite-only.

What works end to end, against Postgres: claiming, sign-in and sign-out,
invites, creating spaces, channels, posting, editing, deleting, threads,
paging, read state and unread pips, direct messages, and the roster. Voice
rooms render and the controls are local-only — the audio path is
`crates/voice-*`, and the two tiers are not joined yet.

### Verification

```bash
# the data layer, against a real Postgres (skips without the variable)
HEARTH_TEST_DATABASE_URL=postgres://postgres:dev@localhost:55432/hearth \
  cargo test -p hearth-db                # 41

# Idea Chat: the shared rules and the server's gates (no database needed), then
# its data layer. That one takes a server URL (each test makes its own database
# on it) and skips without the variable.
cargo test -p chat-model -p chat-api
CHAT_TEST_DATABASE_URL=postgres://postgres:dev@localhost:55432/postgres \
  cargo test -p chat-db

bash tools/reset-dev.sh
bash tools/auth-flows.sh                 # 34 — session, CSRF, timing, invites
bash tools/message-flows.sh              # 17 — posting, threads, authorization

bash tools/reset-dev.sh
node tools/hearth-e2e.js                 # 27 — the whole app, two browsers
```

`hearth-e2e.js` is the one that matters: it claims an instance, makes a space,
posts, reloads, invites a second person in another browser, watches them talk,
signs out and back in — all through the UI.

### Design notes

- **[docs/data-model.md](docs/data-model.md)** — the schema, the auth flows,
  and §13's record of where the plan turned out to be wrong.
- **[docs/hearth-framework-findings.md](docs/hearth-framework-findings.md)** —
  what building this taught us about idealyst, including two entries that are
  corrections to my own misuse rather than defects.

## Step 1 — the voice loop (current)

Proves the out-and-in audio path with no codec, no QUIC, and no new framework
code, so every later decision is made against a pipeline that actually runs.

```text
OUT  microphone.open_stream() -> AudioStream
       .subscribe(|frame|)              interleaved f32 + rate + pts
       Packet::from_f32().encode()      raw i16, no codec yet
       WsSender::send(Binary)

IN   WebSocket::recv() -> Binary
       Packet::decode()
       AudioStream::new() -> (stream, AudioWriter)    one pair per peer
       writer.write_pcm_f32(rate, channels, &pcm)
       playback::play(&stream)                        <-- the only real gap
```

### Crates

| crate | what it is |
| --- | --- |
| `voice-proto` | The wire format. Zero deps, no framework types, fully unit-tested — the most likely candidate for promotion, so it stays portable. |
| `voice-server` | Binary-WebSocket room relay. Validates headers, fans out, never decodes audio. |
| `voice-client` | The idealyst app. `session.rs` is both transport directions; `playback.rs` is the stand-in for the missing `audio-output` SDK. |

### Running it

```bash
cargo run -p voice-server                  # relay on :8787
cd crates/voice-client && idealyst dev --web --local
```

Open **two tabs** and hit Join in both. Nothing to configure — each tab picks a
random ID. **Use headphones**; there is no echo cancellation on the native path
at all, and the browser's is off by default here (see below).

### Known-and-intended limitations

These are deliberate for step 1, not oversights:

- **Web only.** `playback::play` returns `NotSupported` on native, naming the
  gap. That error *is* the finding.
- **No codec.** Raw i16. Mono at 48 kHz is ~768 kbps. A codec slots in later as
  a pure `&[f32] -> Vec<u8> -> Vec<f32>` transform and changes nothing structural.
- **No jitter buffer.** The framework's WebAudio bridge ring buffer (~1 s cap,
  drops oldest) is standing in. Real pacing and loss concealment come later.
- **No signalling.** Peers are discovered by hearing audio from them. Roster,
  names, and mute belong on `#[channel]`, where JSON is the right shape.
- **Echo cancellation defaults OFF.** For a real call it should be on, but in a
  two-tab self-test the audio you are listening for is your own voice from the
  speakers — exactly what AEC removes. The toggle switches it.

## Next

2. `audio-output` (native) — the one real framework deliverable.
3. WebSocket → QUIC/WebTransport.
4. Jitter buffer + PLC.
5. Opus, benchmarked against this pipeline rather than a synthetic one.
6. AEC, then video on platform hardware codecs.

## Verification

`cargo test` — 12 wire-format tests (`voice-proto`) and 6 relay integration
tests (`voice-server`) against a real server on an ephemeral port, covering
no-self-echo, room isolation, and malformed-frame rejection.

`node tools/gateway-test.js` — three tabs through the single gateway origin,
plus an assertion that a redirect never leaks an internal address. Set
`ORIGIN=https://xxx.ngrok.app` to run it against a live tunnel.

`node tools/three-way.js` — three real tabs through the director onto the
containerized fleet: everyone hears both peers, one node owns the room, and the
assignment is released when the last tab leaves. 20/20 checks.

`node tools/loop-test.js` — two real Chromium tabs, real `getUserMedia`
capture, through the relay and back out an `<audio>` element. Last run:

```
tab1(id=1): sent 149 · recv 116 · dropped 0 · peers 1 · 48000 Hz · mic 100% · remote 100%
tab2(id=2): sent 117 · recv 118 · dropped 0 · peers 1 · 48000 Hz · mic  99% · remote 100%
all checks passed
```

![the loop running](docs/voice-loop.png)

Both directions carry a real signal end to end, with zero drops — on web, with
no new framework code.

## Devcontainer

`.devcontainer/` brings up the dev environment plus Postgres, Redis, and a
headless Chromium for the browser harnesses. Open the folder in VS Code and
"Reopen in Container".

Two things it gets right that are easy to get wrong:

- **It mounts `../..`, not this repo.** The workspace path-depends on
  `../idealyst-native`, so both checkouts must sit side by side inside the
  container exactly as they do on the host. Mounting only `idea-chat` breaks
  every framework dependency at manifest-parse time.
- **`wasm-bindgen-cli` is pinned from the framework's lockfile.** A version skew
  against the `wasm-bindgen` crate fails with an error that reads like a corrupt
  build rather than a version mismatch.

The coordination sidecars are reachable inside the container as
`postgres://postgres:dev@postgres:5432/voice` and `redis://redis:6379`, and are
published to the host on `55432` and `6399` for `psql`/`redis-cli`.

`deploy/Dockerfile` builds the fleet image. It uses `deploy/Cargo.server.toml`,
a server-only workspace, so the build context stays inside this repo — the
server tier depends on none of the framework, and copying an entire framework
checkout just so cargo can parse manifests it will never build is pure waste.
