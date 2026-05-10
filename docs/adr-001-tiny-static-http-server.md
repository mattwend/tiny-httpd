# ADR-001: Tiny Static HTTP Server

## Status

Accepted

## Date

2026-05-10

## History

- 2026-04-24 — proposed as draft RFC (commit `91b7af4`).
- 2026-05-10 — promoted to ADR-001; RFC content folded into this document
  and the original file removed.

## Context

We need a lightweight HTTP server to serve static websites inside Kubernetes
clusters. The server sits behind an ingress or service mesh that handles TLS,
authentication, caching, and compression. Existing general-purpose web servers
carry unnecessary complexity, configuration surface, and container image size
for this use case.

## Decision

Build `tiny-httpd` as a minimal Rust static-file HTTP server with the following
design boundaries:

### In scope

- Serve static files from a configurable content root (`/app/public` by
  default).
- Support `GET` and `HEAD` only.
- Prevent path traversal and symlink escape outside the content root.
- Expose Kubernetes-specific liveness (`/livez`) and readiness (`/readyz`)
  probes.
- Graceful shutdown with readiness drain for zero-downtime rollouts.
- Structured telemetry via the shared
  [`telemetry-setup`](https://github.com/mattwend/telemetry-setup) crate.
- Ship as a single statically linked musl binary in a `scratch` container
  image.
- Serve an embedded default welcome page when no content is available, so the
  base image is self-contained.

### Out of scope

These concerns belong to the ingress/proxy, the build pipeline, or a future
ADR:

- TLS termination
- Authentication, authorization, sessions, cookies
- Uploads or runtime content mutation
- Directory listings
- Reverse proxying
- Compression or precompressed asset negotiation
- Range requests, ETags, conditional requests
- Dynamic content or server-side rendering
- Runtime content fetching (Git, object storage, OCI artifacts)

### System context

```text
Client → Ingress (TLS, routing) → Kubernetes Service → tiny-httpd Pod
  → filesystem content root
```

| Component | Responsibility |
| --- | --- |
| Ingress / service mesh | TLS, public routing, cache/compression policy |
| Kubernetes | Scheduling, restarts, readiness/liveness probing |
| tiny-httpd | Static file serving, probe responses, telemetry |

### Kubernetes health model

Separate `/livez` and `/readyz` endpoints instead of a single `/healthz`:

- Kubernetes distinguishes liveness from readiness; a combined endpoint hides
  that distinction.
- Telemetry export failures must not affect readiness — they are observability
  signals, not health checks.
- `exec` probes are unsuitable for a `scratch` image with no shell.
- TCP probes only prove socket acceptance; HTTP probes validate the server loop
  and content access.

Probe routes are reserved and take precedence over static files at the same
path.

### Content packaging

The server does not build, fetch, or mutate content. Content must be immutable
for the lifetime of a running pod.

Supported deployment patterns:

1. **Derived image** — `COPY` site files into a downstream image based on
   `tiny-httpd`.
2. **Volume mount** — Mount content at the content root path.
3. **Init-container** — Copy content from a separate immutable image into a
   shared `emptyDir` volume before `tiny-httpd` starts. This pattern supports
   independent release cadence for server and content.

### Filesystem safety

All file lookups canonicalize paths and verify they remain inside the content
root before opening. Symlinks are followed only when their canonical target
stays within bounds. Files are opened shortly after canonicalization; a
residual resolve/open TOCTOU window remains, but this is accepted because the
content root is expected to be read-only at runtime.

### Graceful shutdown

On `SIGTERM`, readiness flips to `503` while the listener stays open briefly so
Kubernetes can observe the failure. Then the listener closes and in-flight
requests drain with a bounded hard timeout. `/livez` remains `200` until
process exit.

### Technology choices

- **tokio** — async runtime and signal handling.
- **hyper** + **hyper-util** / **http-body-util** — HTTP serving without a
  full web framework.
- **mime_guess** — content type lookup from file extensions.
- **tracing** — structured instrumentation.
- **telemetry-setup** — OTLP export wiring, shared across services; OTLP is
  the standard telemetry transport used across services in this environment.
- **thiserror** — crate-local error types.

A full web framework is avoided unless routing or middleware requirements grow
beyond this scope.

## Conformance requirements

Automated tests must cover:

- `GET` and `HEAD` behavior.
- `405` handling and `Allow` header.
- index resolution for `/`, `/dir`, and `/dir/`.
- `404` for missing files.
- invalid percent encoding and traversal attempts.
- symlink escape rejection.
- content type and content length headers.
- `/livez` and `/readyz` behavior, including readiness failure after
  content-root access failure where testable.
- telemetry initialization errors being propagated during startup.

## Consequences

### Benefits

- The server is auditable: small codebase, explicit behavior, no configuration
  language.
- The `scratch` image has minimal attack surface — no shell, no package
  manager, no libc.
- Features like compression, caching headers, or TLS require upstream
  infrastructure, not server changes.
- Adding HTTP features (range requests, ETags, etc.) requires a new ADR to
  avoid scope creep.
- Content workflows that need runtime fetching must solve that outside the
  server process.

### Costs and trade-offs

- No range requests means clients retrying large assets must re-download the
  full object.
- No ETag or conditional request support means cache validation must be handled
  upstream or not at all.
- The embedded welcome page can mask a missing or mis-mounted content volume
  — operators must verify volume mounts independently of a `200` at `/`.
- Reliance on ingress or service-mesh features pushes caching, compression, and
  TLS policy outside the binary, increasing dependency on platform
  configuration.
