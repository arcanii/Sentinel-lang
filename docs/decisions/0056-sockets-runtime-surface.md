# ADR 0056: A TCP sockets runtime surface (the networking gap)

Status: **PROPOSED — layer 1 (the runtime primitives) IMPLEMENTED** (`b90a889`). The six
`extern "C"` socket primitives (+ an ephemeral-port query) are in `sentinel-runtime`
with a Rust loopback-echo test; the compiler-builtin wiring + the self-hosted-`scg`
builtin-table mirror (which the FnId shift makes mandatory — see "Touch-points") are the
remaining, separable step. Scopes the runtime + ABI surface that real network services
(an `sshd`, an HTTPS server) need but Sentinel lacks today.

## Context

The examples-as-tests track has shipped a comprehensive constant-time crypto stdlib —
X25519 / Ed25519 / X448 / Ed448, ChaCha20-Poly1305, AES-GCM, the SHA-2/3 families,
HMAC, HKDF — i.e. essentially a complete SSH/TLS *cipher suite*. The obvious flagship
demo is a real network service that exercises it end to end. But Sentinel's runtime
exposes only file I/O (`read_file` / `write_file` / `print_bytes`, ADR 0035), `alloc` /
`free` / `realloc`, and the structured-concurrency primitives (`scope` / `spawn` /
`await`, ADR 0024). There are **no sockets** — no `socket` / `bind` / `listen` /
`accept` / `connect` / `read` / `write`. Both an `sshd` and a webserver are blocked on
exactly this gap, so it is the prerequisite for either.

The companion `std/net/ssh` transport library is being built **loopback-first** (over
in-memory buffers), so the crypto/handshake showcase does not wait on this ADR; this ADR
is what lets that handshake later run over a real connection.

## Decision

Add a minimal **TCP** sockets surface as runtime builtins, mirroring the file-I/O
builtins (ADR 0035): `extern "C"` functions in `sentinel-runtime`, registered as
builtin `FnId`s, taking/returning the standard `[u8]` ABI (`{i64 len, ptr data}`) plus
an opaque `i64` handle for the descriptor. The model is **blocking sockets + one task
per connection** — the structured-concurrency model already in the language maps onto
the canonical accept-loop directly.

### The builtins (the proposed signatures, Sentinel-side)

```
// --- server ---
tcp_listen(port: i64) -> i64            // bind 127.0.0.1:port + listen; returns a
                                        //   listener handle ≥ 0, or < 0 on error.
tcp_accept(listener: i64) -> i64        // block for a connection; returns a conn
                                        //   handle ≥ 0, or < 0 on error/shutdown.
// --- client ---
tcp_connect(host: [u8], port: i64) -> i64   // host = dotted-quad IPv4 bytes (no DNS);
                                            //   returns a conn handle ≥ 0 or < 0.
// --- stream I/O (both roles) ---
tcp_read(conn: i64, max: i64) -> [u8]   // block for up to `max` bytes; returns the
                                        //   bytes read (len 0 = peer closed / EOF).
tcp_write(conn: i64, data: [u8]) -> i64 // write all of `data`; returns bytes written,
                                        //   or < 0 on error.
tcp_close(conn: i64) -> i64             // close a listener or connection; 0 / < 0.
```

`tcp_read` returns a fresh `sentinel_alloc`'d `[u8]` (scope-exit drop frees it, exactly
like `read_file`); `tcp_write` borrows its `data` (the ADR 0033 A3 runtime-builtin
rule). The handle is an opaque `i64` (the OS file descriptor), never dereferenced by
Sentinel code.

### Error handling: RETURN errors, do NOT panic (the deviation from file I/O)

File I/O **aborts** on failure (ADR 0035 D5) because a missing input file is a program
bug. A network daemon is the opposite: a dropped connection, a refused connect, or a
peer reset is *normal* and must not take the process down. So the socket builtins
**return** a negative `i64` on failure (or an empty `[u8]` from `tcp_read` for a clean
EOF) and never abort. A future increment can layer a richer `?T` / `Result` over this;
the sign-of-the-return convention is the MVP, matching `write`/`read`'s POSIX shape.

### Concurrency: blocking + one OS-thread-backed task per connection

The idiomatic server is an accept loop that hands each connection to a worker — which is
exactly `scope` + `spawn`:

```
fn serve(port: i64) -> i64 {
    let l: i64 = tcp_listen(port);
    scope {
        while true {
            let c: i64 = tcp_accept(l);
            if c < 0 { break; 0 } else { 0 };
            spawn handle_conn(c);     // one task per connection
        }
    }
    0
}
```

For this to work, a `spawn`ed task that *blocks* in `tcp_accept` / `tcp_read` must not
stall the others. Two options:

- **(v1, proposed) OS-thread-backed tasks** — `sentinel_task_spawn` runs each task on
  its own OS thread, so a blocking syscall only blocks that thread. Simplest; correct;
  scales to hundreds–low-thousands of connections. The runtime already hands tasks a
  packed-args wrapper, so this is a backing-strategy choice, not a language change.
- **(future) a non-blocking reactor** — `O_NONBLOCK` sockets + an `epoll`/`kqueue` poll
  loop driving cooperative tasks. Higher connection ceiling (the "high speed" story),
  but a much larger runtime change (a reactor, readiness wakeups, `await`-on-fd). Out of
  scope for v1; the blocking model is forward-compatible (the Sentinel-side API is
  unchanged if the backing strategy later changes).

### Constant-time / information flow: the socket is a PUBLIC boundary

The wire is observable by definition, so the socket builtins carry **public `[u8]`**:
`tcp_read` yields `[u8]`, `tcp_write` takes `[u8]`. Secret data (keys, plaintext) is
encrypted *first* — the ciphertext is a declassified `[u8]` — and only then written. The
constant-time guarantee lives entirely **upstream** in the crypto (a key bit never
reaches a branch / index / divisor); the socket is the sanctioned declassify sink for
already-public bytes. This keeps the model clean: nothing secret can be `tcp_write`n
without an explicit `declassify`, and the type system enforces it (a `tcp_write(conn,
secret_bytes)` is a type error — you must declassify, which is the audit point).

### Scope / non-goals (v1)

- **TCP only** — no UDP, no raw sockets.
- **IPv4 only** — `tcp_connect` takes 4 dotted-quad bytes; IPv6 deferred.
- **No DNS** — the caller passes an IP; a resolver is a later library/builtin.
- **Localhost-bound `listen`** in v1 (`127.0.0.1`) so example servers are not
  internet-exposed by default; a bind-address argument is a small follow-up.
- **No TLS** — TLS/SSH termination is a *library* on top of this surface (the whole
  point); sockets carry bytes only.
- **No `select`/`poll` exposed** — the blocking + per-connection-task model hides it.

### Touch-points (when implemented)

`sentinel-runtime` (the six `extern "C"` builtins + the OS-thread task backing);
`sentinel-codegen` + `sentinel-resolve` (register the builtin `FnId`s + their call
shape, exactly like `read_file`/`write_file`/`print_bytes`); `sentinel-types` (the
builtin signatures); a `std/net` library wrapping them. ⚠ **FnId shift**: these are new
builtins, so they extend the builtin `FnId` table (currently `0..=13`) and shift user-fn
`FnId`s in dumps — the same concern as the file-I/O builtins. Either mirror them into the
self-hosted `scg` (so the selfhost differentials stay byte-identical) or land them
snc-side with the demos in `examples/` (not `tests/pass`), like ADR 0055's `u128`. The
latter is the lower-risk first step.

### Security posture (for the eventual daemon)

The daemon holds long-lived secret key material and faces untrusted input. The sockets
surface itself adds no crypto — it is a dumb byte pipe — so the security burden sits in
the protocol library (SSH/TLS), which is exactly where Sentinel's `secret`/constant-time
guarantees apply. Resource limits (max concurrent connections, read-size caps) belong in
the `std/net` wrapper, not the raw builtins. Examples bind localhost.

## Alternatives considered

- **Algebraic effects for I/O** (an `Net` effect handled by the runtime) — more
  principled and composable, but ADR 0035 already chose runtime builtins over effects
  for file I/O (D2) for exactly the same reasons (no handler plumbing for a leaf
  syscall); sockets follow that precedent for consistency.
- **A non-blocking reactor in v1** — the higher-performance design, but a large runtime
  change; deferred (the blocking API is forward-compatible).
- **Exposing raw `socket`/`bind`/`listen`/`accept` separately** — more POSIX-faithful
  but more surface; the fused `tcp_listen` (socket+bind+listen) is enough for v1.

## Status / next

PROPOSED. The loopback `std/net/ssh` transport library is being built first (no sockets
needed); this surface is the milestone that lets that handshake run over a real
connection. Recommended first implementation step: the six builtins snc-side + a
localhost echo-server example, deferring the `scg` mirror (mirroring the `u128`
precedent), then the reactor as a separate performance ADR.
