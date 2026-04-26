# tiny-httpd

Minimal Rust HTTP server for static files in Kubernetes.

## What it does

- Serves static files from `/app/public` by default
- Supports `GET` and `HEAD`
- Resolves `index.html` for `/`, `/dir`, and `/dir/`
- Blocks path traversal and symlink escapes outside the content root
- Opens resolved files during path resolution to avoid a resolve/open TOCTOU gap
- Exposes Kubernetes probes:
  - `/livez`
  - `/readyz`
- Shuts down gracefully during rollouts
- Emits structured telemetry via `telemetry-setup`
- Records request metrics for count, duration, response body size, and active requests

## What it does not do

No TLS, auth, uploads, directory listings, reverse proxying, compression, range requests, ETags, or dynamic content.

## Prerequisites

- Rust + Cargo
- Podman for container builds

The reference container build targets `x86_64-unknown-linux-musl` and produces a static binary for a `scratch` image.

## Run locally

```bash
mkdir -p public
printf 'hello\n' > public/index.html
cargo run -- --listen-addr 127.0.0.1:8080 --content-root ./public
```

## Configuration

Environment variables can be overridden by CLI flags.

| Setting | Environment variable | CLI flag | Default |
| --- | --- | --- | --- |
| `listen_addr` | `TINY_HTTPD_LISTEN_ADDR` | `--listen-addr` | `0.0.0.0:8080` |
| `content_root` | `TINY_HTTPD_CONTENT_ROOT` | `--content-root` | `/app/public` |
| `service_name` | `TINY_HTTPD_SERVICE_NAME` | `--service-name` | `tiny-httpd` |

Startup fails if the content root is missing or not a directory, the socket cannot be bound, or telemetry setup fails.

## Request behavior

| Request | Result |
| --- | --- |
| `GET /path` | Returns the file body |
| `HEAD /path` | Returns the same headers without the body |
| Other methods | `405 Method Not Allowed` with `Allow: GET, HEAD` |
| Any method on `/livez` | Liveness status |
| Any method on `/readyz` | Readiness status |

### Response headers

Successful file responses include `Content-Type` (derived from the file extension via `mime_guess`) and `Content-Length`. Probe responses include `Content-Type: text/plain; charset=utf-8`.

### Error responses

| Case | Status |
| --- | --- |
| Malformed request target or invalid percent encoding | `400 Bad Request` |
| Path traversal attempt or content-root escape | `400 Bad Request` |
| File not found | `404 Not Found` |
| Unsupported method | `405 Method Not Allowed` |
| I/O error while serving an existing file | `500 Internal Server Error` |

Error bodies are plain text and intentionally minimal.

### Path resolution

| Path | Lookup |
| --- | --- |
| `/` | `index.html` |
| `/foo` | `foo`, then `foo/index.html` if `foo` is a directory |
| `/foo/` | `foo/index.html` |
| `/foo.html` | `foo.html` |

Probe routes take precedence over static files.

## Kubernetes probes

```yaml
livenessProbe:
  httpGet:
    path: /livez
    port: http
readinessProbe:
  httpGet:
    path: /readyz
    port: http
```

During graceful shutdown the server:

1. Marks readiness false and rejects non-probe requests with `503`.
2. Keeps the listener open for a 250 ms readiness drain window so `/readyz` can return `503` before the socket closes.
3. Stops accepting new connections and drains in-flight requests up to a 10 s hard timeout, after which remaining tasks are aborted.
4. `/livez` stays `200` until process exit.

This gives Kubernetes an HTTP readiness failure signal before connection refusal.

## Container build

A `.containerignore` excludes `target/`, `.git/`, and `.gitignore` from the build context to keep image builds smaller and preserve layer caching.

Build from the repository root:

```bash
podman build -f Containerfile -t tiny-httpd:dev .
```

The final image contains:

```text
/app/tiny-httpd
/app/public/...
```

The `public/` directory must exist at the repository root before building; the `COPY` step fails otherwise. Add site files there, or use the init-container pattern below.

### Init-container deployment

When server and content versions need independent release cadence, package content in a separate immutable image and copy it into a shared volume before `tiny-httpd` starts:

```yaml
initContainers:
  - name: content
    image: site-content:<content-digest-or-tag>
    command: ["/bin/cp", "-a", "/public/.", "/app/public/"]
    volumeMounts:
      - name: public
        mountPath: /app/public
containers:
  - name: tiny-httpd
    image: tiny-httpd:<server-version>
    volumeMounts:
      - name: public
        mountPath: /app/public
        readOnly: true
volumes:
  - name: public
    emptyDir: {}
```

The copy tool belongs to the init container or content image, not to `tiny-httpd`; the server image stays minimal. Runtime content fetching from object storage, Git, or other network sources is out of scope; such workflows must complete before `tiny-httpd` starts and must present a local immutable `content_root`.

## Observability

The server emits per-request spans and HTTP server metrics.

### Metric instruments

| Instrument | Type | Unit | Description |
| --- | --- | --- | --- |
| `http.server.request.count` | Counter | — | Completed HTTP requests |
| `http.server.request.duration` | Histogram | s | HTTP request duration |
| `http.server.response.body.size` | Histogram | By | HTTP response body size |
| `http.server.active_requests` | UpDownCounter | — | HTTP requests currently in flight |

Completed-request metrics include these attributes:

- `http.request.method`
- `http.response.status_class`

Response body size is recorded from the HTTP `Content-Length` header when present. In this server, file and text responses set a fixed content length, so the recorded metric reflects the declared response body size for both `GET` and `HEAD` handling.

## Development

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```

## Design

See [`docs/rfc-tiny-static-http-server.md`](docs/rfc-tiny-static-http-server.md).
