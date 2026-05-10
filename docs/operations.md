# Operations

Runtime behavior reference for operators deploying and monitoring tiny-httpd.

## Request handling

| Method | Behavior |
| --- | --- |
| `GET` | Returns file body |
| `HEAD` | Returns headers without body |
| Any other | `405 Method Not Allowed` with `Allow: GET, HEAD` |

### Path resolution

| Request path | Candidate lookup |
| --- | --- |
| `/` | `index.html`, otherwise embedded welcome page |
| `/foo` | `foo`, then `foo/index.html` if `foo` is a directory |
| `/foo/` | `foo/index.html` |

Probe routes (`/livez`, `/readyz`) take precedence over static files.

### Response headers

Successful file responses include `Content-Type` (derived from file extension
via `mime_guess`) and `Content-Length`.
Probe responses include `Content-Type: text/plain; charset=utf-8`.

### Error responses

| Case | Status |
| --- | --- |
| Malformed request target or invalid percent encoding | `400 Bad Request` |
| Path traversal attempt or content-root escape | `400 Bad Request` |
| File not found | `404 Not Found` |
| Unsupported method | `405 Method Not Allowed` |
| I/O error while serving an existing file | `500 Internal Server Error` |

Error bodies are plain text and intentionally minimal.

### Default welcome page

When no usable `index.html` exists (content root missing or empty), `GET /`
and `HEAD /` return an embedded welcome page. All other paths return `404` as
usual. A user-provided `index.html` always takes precedence.

## Startup behavior

| Condition | Result |
| --- | --- |
| Content root missing or unavailable | Warning logged, server starts, welcome page fallback active |
| Content root exists but is not a directory | Startup fails |
| Listen socket cannot be bound | Startup fails |
| Telemetry initialization fails | Startup fails |

## Kubernetes probes

| Endpoint | Purpose |
| --- | --- |
| `/livez` | Liveness — process is running and handling HTTP |
| `/readyz` | Readiness — startup complete and able to serve; returns `503` during shutdown drain |

Probe routes accept any HTTP method.

## Graceful shutdown

On `SIGTERM`:

1. Readiness flips to `503`; non-probe requests are rejected with `503`.
2. Listener stays open for a **250 ms** readiness drain window so Kubernetes
   can observe the `503` from `/readyz` before the socket closes.
3. Listener closes; in-flight requests drain up to a **10 s** hard timeout.
4. Remaining connections are aborted after timeout.
5. `/livez` stays `200` until process exit.

This gives Kubernetes a readiness-failure signal before the listener closes.
Achieving zero-downtime rollouts also depends on probe frequency,
`terminationGracePeriodSeconds`, and endpoint propagation timing.

## Observability

Telemetry is initialized via the
[`telemetry-setup`](https://github.com/mattwend/telemetry-setup) crate (OTLP
export, structured tracing, Tokio runtime metrics).

The process sets the OpenTelemetry `service.name` resource attribute from
`TINY_HTTPD_SERVICE_NAME` / `--service-name` (default: `tiny-httpd`).

### Request spans

Each non-probe request creates a tracing span with:

- `http.request.method`
- `url.path`
- `http.response.status_code`
- `http.response.status_class`
- `network.peer.address` (when available)
- `http.server.request.duration_us`

Probe successes are logged at `debug` level; probe failures are always logged.

### Metric instruments

| Instrument | Type | Unit | Description |
| --- | --- | --- | --- |
| `http.server.request.count` | Counter | — | Completed HTTP requests |
| `http.server.request.duration` | Histogram | s | Request handling duration |
| `http.server.response.body.size` | Histogram | By | Response body size (from `Content-Length`) |
| `http.server.active_requests` | UpDownCounter | — | Requests currently in flight |

Completed-request metrics carry these attributes:

- `http.request.method`
- `http.response.status_class` (`2xx`, `4xx`, `5xx`, etc.)

## Filesystem safety

- Content root is canonicalized at startup.
- Path traversal components are rejected after percent-decoding.
- Each candidate path is canonicalized and verified to stay within the content
  root before opening.
- Only regular files are served; symlinks are followed only if their target
  remains inside the content root.
