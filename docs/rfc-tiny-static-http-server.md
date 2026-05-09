# RFC: Tiny Static HTTP Server

## Status

Draft

## Owner

Matthias Wende

## Date

2026-04-24

## Summary

Build a small Rust HTTP server that serves immutable static files from a configured filesystem content root. The service is intended to run in Kubernetes behind an ingress/proxy such as Envoy. It handles only static file delivery, Kubernetes health/readiness probes, graceful shutdown, and telemetry initialization.

## Goals

- Serve static files from a configured content root, defaulting to `/app/public`.
- Support `GET` and `HEAD` only.
- Prevent path traversal and content-root escape, including symlink escape.
- Run as a single statically linked binary in a minimal container image.
- Integrate with Kubernetes readiness/liveness probes.
- Emit structured logs, traces, and metrics through the shared `telemetry-setup` crate.
- Keep behavior explicit and small enough to audit.

## Non-goals

The server will not implement:

- TLS termination.
- Authentication, authorization, sessions, or cookies.
- Uploads or any runtime content mutation.
- Directory listings.
- Reverse proxying.
- Compression or precompressed asset negotiation.
- Range requests.
- ETag or conditional request handling.
- A general web-server configuration language.

These concerns belong to the ingress/proxy, the build pipeline, or a future RFC.

## System context

```text
Client
  -> Envoy / Kubernetes ingress
  -> Kubernetes Service
  -> tiny-httpd Pod
  -> immutable filesystem content root
```

Responsibilities:

| Component | Responsibility |
| --- | --- |
| Envoy / ingress | TLS, public routing, optional cache/compression policy |
| Kubernetes | scheduling, restarts, readiness/liveness probing |
| tiny-httpd | static file serving, probe responses, telemetry |

## Content packaging and deployment

`tiny-httpd` does not build, fetch, or mutate site content. Content must be immutable for the lifetime of a running Pod.

The base server image ships binary only:

```Dockerfile
FROM tiny-httpd:<version> AS server
FROM scratch
COPY --from=server /app/tiny-httpd /app/tiny-httpd
ENTRYPOINT ["/app/tiny-httpd"]
```

If no content is present at startup, the server still starts and serves an embedded default welcome page at `/`. This keeps the base image self-contained and useful before downstream content injection.

A supported alternate deployment may package content in a separate immutable content image and copy it into a shared volume with an init container before `tiny-httpd` starts. The main server container still runs the same binary image and reads from `content_root`, usually mounted read-only:

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

This alternate shape is useful when server and content versions need independent release cadence. The copy tool belongs to the init container or content image, not to `tiny-httpd`; the release server image remains minimal.

Runtime content fetching from object storage, OCI artifacts, Git, or other network sources is out of scope for the server. If such workflows are needed, they must complete before `tiny-httpd` starts and must present a local immutable `content_root`.

## Runtime configuration

Configuration is supplied by command-line flags and/or environment variables. Defaults must be suitable for the container image.

| Setting | Default | Description |
| --- | --- | --- |
| `listen_addr` | `0.0.0.0:8080` | Socket address to bind. |
| `content_root` | `/app/public` | Directory containing generated static files. |
| `service_name` | `tiny-httpd` | Telemetry service name. |

Startup fails if:

- the content root exists but is not a directory,
- the content root exists but cannot be canonicalized,
- the configured listen socket cannot be bound,
- telemetry initialization returns an error.

If the content root is missing or otherwise unavailable during startup inspection, startup logs a warning, continues, and enables the embedded default welcome page fallback.

## HTTP behavior

### Methods

| Method | Behavior |
| --- | --- |
| `GET` | Return the selected file body. |
| `HEAD` | Return the same headers as `GET` without a body. |
| Any other method | Return `405 Method Not Allowed` with `Allow: GET, HEAD`. |

### Path resolution

The request target path is percent-decoded and normalized before file lookup.

Resolution rules:

| Request path | Candidate lookup |
| --- | --- |
| `/` | `index.html` |
| `/foo` | `foo`, then `foo/index.html` if `foo` is a directory |
| `/foo/` | `foo/index.html` |
| `/foo.html` | `foo.html` |

If no candidate resolves to a regular file, return `404 Not Found`.

### Default welcome page

The binary embeds a default HTML welcome page at compile time.

Fallback rules:

- If no usable content root is available, `GET /` and `HEAD /` return the embedded page.
- If a content root exists but `/index.html` is absent, `GET /` and `HEAD /` return the embedded page.
- All non-root paths still return normal `404 Not Found` when no file resolves.
- If a user-provided `/index.html` exists, it always takes precedence over the embedded page.

The server does not perform implicit redirects for missing or extra trailing slashes.

### Response headers

Successful file responses include:

- `Content-Type`, derived from file extension using a maintained MIME mapping crate.
- `Content-Length`.

Probe responses include `Content-Type: text/plain; charset=utf-8`.

Cache headers are not added by this server in the initial implementation. They may be added at the ingress/proxy layer or by a future RFC.

### Error responses

| Case | Status |
| --- | --- |
| Malformed request target or invalid percent encoding | `400 Bad Request` |
| Path traversal attempt or content-root escape | `400 Bad Request` |
| File not found | `404 Not Found` |
| Unsupported method | `405 Method Not Allowed` |
| I/O error while serving an existing file | `500 Internal Server Error` |

Error bodies are plain text and intentionally minimal.

## Filesystem safety

The implementation must ensure that no request can read outside `content_root`.

Required checks:

1. Canonicalize `content_root` during startup.
2. Reject request paths containing parent-directory components after decoding.
3. For each selected candidate, canonicalize the candidate path before opening it.
4. Verify the canonical candidate starts with the canonical content root.
5. Serve regular files only.

Symlinks inside the content root are allowed only if their canonical target remains inside the content root. Symlinks escaping the content root are rejected.

## Kubernetes health model

Do not implement a generic `/healthz` endpoint.

The service exposes Kubernetes-specific probe endpoints:

| Endpoint | Purpose | Success criteria |
| --- | --- | --- |
| `/livez` | Liveness probe | Process is running and can handle an HTTP request. |
| `/readyz` | Readiness probe | Startup validation completed and the server can serve requests; if a content root exists, it must remain readable. |

Probe routes are reserved and take precedence over static files with the same path. They should be used by Kubernetes probes and should not be routed publicly by ingress.

Rationale:

- Kubernetes distinguishes liveness from readiness; a single `/healthz` endpoint usually hides that distinction.
- OTLP export, metrics, and logs are observability signals, not health checks. A telemetry collector outage must not make this static server unready.
- `exec` probes are unsuitable for a `scratch` image.
- TCP probes only prove that a socket accepts connections; HTTP readiness can also validate the server loop and content-root access.

Recommended Kubernetes probe configuration:

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

## Observability

Telemetry setup is delegated to the shared opinionated OTLP setup crate published from `https://github.com/mattwend/telemetry-setup`.

The HTTP server depends on the package from Git:

```toml
telemetry-setup = { git = "https://github.com/mattwend/telemetry-setup", tag = "v0.1.0", features = ["otlp", "tokio-metrics"] }
```

The server is responsible for creating meaningful spans, events, and metrics. The `telemetry-setup` crate is responsible for subscriber/provider setup, OTLP exporter configuration, resource attributes, shutdown flushing, and optional Tokio runtime metrics.

### Request logging and tracing

Each non-probe request creates one tracing span with at least:

- `http.request.method`
- `url.path`
- `http.response.status_code`
- `network.peer.address` when available
- elapsed time
- bytes sent

Probe requests may be logged at `debug` or omitted from access logs to avoid noise, but failures must be logged.

### Metrics

Initial metrics:

- request count by method and status class,
- request duration histogram,
- response body size histogram,
- in-flight request gauge.

Metric instruments should be created through OpenTelemetry APIs compatible with `telemetry-setup`; exporter details remain outside this crate.

## Graceful shutdown

The process handles `SIGTERM` by first marking readiness false. While the listener is still running, `/readyz` returns `503 Service Unavailable` so Kubernetes can remove the Pod from Service endpoints. The server then performs a bounded graceful shutdown: in-flight requests may complete, and new non-probe requests are rejected with `503 Service Unavailable` or the listener is closed. `/livez` remains `200 OK` until process exit.

Before exiting, call the telemetry guard shutdown path so OTLP data can flush.

## Implementation choices

Preferred dependency set:

- `tokio` for async runtime and signal handling.
- `hyper` plus `hyper-util`/`http-body-util` for HTTP serving.
- `mime_guess` or equivalent maintained crate for content type lookup.
- `tracing` for instrumentation.
- `telemetry-setup` for telemetry initialization and OTLP export wiring.
- `thiserror` for crate-local error types.

Avoid adding a full web framework unless routing or middleware requirements grow beyond this RFC.

## Container image

The reference release image contains only:

```text
/app/tiny-httpd
```

Both image shapes run the binary directly, with no shell or package manager in the server container. The filesystem and content mount should be read-only at runtime where the platform allows it.

## Test requirements

Automated tests must cover:

- `GET` and `HEAD` behavior.
- `405` handling and `Allow` header.
- index resolution for `/`, `/dir`, and `/dir/`.
- `404` for missing files.
- invalid percent encoding and traversal attempts.
- symlink escape rejection.
- content type and content length headers.
- `/livez` and `/readyz` behavior, including readiness failure after content-root access failure where testable.
- telemetry initialization errors being propagated during startup.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Path traversal or symlink escape | Canonicalize and verify candidates before opening; test malicious paths. |
| Probe endpoints conflict with content | Reserve `/livez` and `/readyz`; document route precedence. |
| Telemetry backend outage affects serving | Treat exporter failures as telemetry failures, not readiness failures after startup. |
| Over-scoping into a web server | Keep non-goals explicit and require future RFCs for new HTTP features. |

## Decision

Implement `tiny-httpd` as a minimal Rust static-file server with explicit Kubernetes `/livez` and `/readyz` probes, safe canonicalized file lookup, graceful shutdown, and telemetry wired through the shared `telemetry-setup` Git dependency. Use a single final image with `/app/public` baked in as the reference deployment, and explicitly support init-container population from a separate immutable content image. Keep TLS, caching policy, compression, authentication, and runtime content fetching outside this service.
