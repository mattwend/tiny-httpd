# ADR-002: Idle Connection Timeout via ActivityIo Wrapper

## Status

Accepted

## Date

2026-05-14

## Context

HTTP/1.1 keep-alive connections can remain open indefinitely after the last
request completes. In a Kubernetes environment with limited connection budgets,
idle connections waste file descriptors and memory without serving traffic. The
server needs a per-connection idle timeout that resets on real byte activity so
that:

- Long-lived but **active** connections (slow downloads, pipelining) are not
  killed prematurely.
- Genuinely **idle** keep-alive connections are closed promptly.

### Problem

hyper does not expose an "on activity" callback, idle-timeout hook, or
keep-alive timeout configuration. Once a TCP stream is handed to
`hyper_util::server::conn::auto::Builder::serve_connection()`, hyper owns the
read/write lifecycle and the caller cannot observe whether the connection is
idle or actively transferring data.

A simple overall-duration timeout from connection open to close would penalise
active connections. Detecting idleness requires observing byte-level I/O
activity.

## Decision

Implement `ActivityIo<T>`, a transparent `AsyncRead + AsyncWrite` wrapper that
intercepts every `poll_read`, `poll_write`, and `poll_write_vectored` call.
When a non-zero number of bytes flow, it signals an `Arc<Notify>`. The
per-connection task consumes these notifications in its `tokio::select!` loop
and resets the idle deadline on each signal.

### Design details

- **Lossy edge trigger** — `Notify::notify_one()` is used intentionally.
  Multiple rapid reads/writes may collapse into a single pending notification.
  This is acceptable because the consumer only needs to know that *some*
  activity happened since the last reset.
- **Zero-byte reads are not signalled** — EOF (zero-byte read) does not reset
  the idle timer, so a half-closed connection progresses toward timeout.
- **`poll_flush` and `poll_shutdown` are pure pass-through** — these are
  control operations, not data flow, and do not indicate user activity.

## Alternatives considered

### 1. hyper built-in `header_read_timeout`

Already configured and used for the initial header read phase. This only covers
the time to receive request headers, not idle gaps between keep-alive requests.
Not a replacement.

### 2. hyper-util keep-alive timeout

`hyper_util::server::conn::auto::Builder` does not expose a per-connection
idle or keep-alive timeout. The underlying `hyper::server::conn::http1::Builder`
also lacks this. Not available.

### 3. Tower service-layer middleware

A Tower `Service` layer could track time since the last request completed. This
would be simpler (no `Pin` gymnastics, no raw `poll_*` implementations) but
coarser: it only observes request boundaries, not byte-level activity. A
connection performing a slow streaming response would appear "idle" between
service call start and finish. `ActivityIo` is more accurate.

### 4. `tokio_util::io::InspectReader` + `InspectWriter`

`tokio-util` (already a dependency) provides `InspectReader` and
`InspectWriter` that call a closure on every non-empty read/write. This would
eliminate the entire `ActivityIo` struct and all five `poll_*` implementations
(~80 lines), replacing them with ~5 lines at the call site.

**Trade-off:** `InspectWriter` does not intercept `poll_write_vectored`, so
vectored writes would fall through to the default `poll_write` implementation
(still correct and still signalled, but with a minor efficiency cost if hyper
uses vectored writes). The current `ActivityIo` handles `poll_write_vectored`
explicitly.

**Verdict:** A viable simplification if the codebase needs to shrink. Rejected
for now because the explicit implementation already exists with tests, handles
vectored writes, and carries no additional conceptual overhead beyond what is
documented here.

### 5. TCP `SO_KEEPALIVE` / socket-level idle detection

OS-level TCP keepalives detect dead peers, not application-level idleness. A
client can hold a keep-alive connection open with TCP keepalives flowing while
sending zero HTTP requests. Not a replacement.

## Consequences

### Benefits

- Idle connections are reclaimed without penalising active transfers.
- The wrapper is zero-cost when no bytes flow (no heap allocation, no timer
  manipulation on `Poll::Pending`).
- The `Notify`-based edge trigger avoids per-byte timer resets; the idle
  deadline is only touched once per wakeup cycle.

### Costs

- ~80 lines of manual `AsyncRead`/`AsyncWrite` boilerplate that must be
  maintained if upstream trait signatures change.
- The wrapper must be kept in sync with any new `AsyncWrite` trait methods
  (e.g., future Tokio additions).
- A simpler alternative (`InspectReader`/`InspectWriter`) exists and may be
  preferred if maintenance cost becomes a concern.
