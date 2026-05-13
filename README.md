# tiny-httpd

Minimal, single-binary HTTP server for serving static websites in Kubernetes.
No TLS — let your ingress or service mesh handle that.

## Features

- Serves static files from a configurable content root (`/app/public` by
  default)
- Embedded default welcome page when no `index.html` is present
- `GET` and `HEAD` only; blocks path traversal and symlink escapes
- Kubernetes liveness (`/livez`) and readiness (`/readyz`) probes
- Graceful shutdown with readiness drain to support zero-downtime rollouts
- Configurable HTTP/1 header-read, idle-connection, and graceful-close timeouts
- Structured tracing and HTTP request metrics
- Static musl binary in a `scratch` container image

## Prerequisites

- Rust + Cargo
- Podman for local container builds
- Docker Buildx for GitHub Actions container validation and publishing

The reference container build targets `x86_64-unknown-linux-musl` and
produces a static binary for a `scratch` image.

## Quick start

Run locally:

```bash
cargo run -- --listen-addr 127.0.0.1:8080 --content-root ./public
```

Build and run the container:

```bash
podman build -f Containerfile -t tiny-httpd:dev .
podman run --rm -p 8080:8080 localhost/tiny-httpd:dev
```

Without a content root on disk, `/` serves the embedded welcome page.

## Configuration

Environment variables can be overridden by CLI flags.

| Environment variable | CLI flag | Default |
| --- | --- | --- |
| `TINY_HTTPD_LISTEN_ADDR` | `--listen-addr` | `0.0.0.0:8080` |
| `TINY_HTTPD_CONTENT_ROOT` | `--content-root` | `/app/public` |
| `TINY_HTTPD_SERVICE_NAME` | `--service-name` | `tiny-httpd` |
| `TINY_HTTPD_HEADER_READ_TIMEOUT_SECS` | `--header-read-timeout-secs` | `30` |
| `TINY_HTTPD_IDLE_CONNECTION_TIMEOUT_SECS` | `--idle-connection-timeout-secs` | `60` |
| `TINY_HTTPD_GRACEFUL_CLOSE_TIMEOUT_SECS` | `--graceful-close-timeout-secs` | `5` |

## Path resolution

| Path | Lookup |
| --- | --- |
| `/` | `index.html`, otherwise embedded welcome page |
| `/foo` | `foo`, then `foo/index.html` if directory |
| `/foo/` | `foo/index.html` |

Probe routes (`/livez`, `/readyz`) take precedence over static files.

## Kubernetes deployment

### Probes

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

### Serving content

The base image ships the binary only. Provide site files via a derived image,
volume mount, or init-container pattern:

```yaml
initContainers:
  - name: content
    image: site-content:<tag>
    command: ["/bin/cp", "-a", "/public/.", "/app/public/"]
    volumeMounts:
      - name: public
        mountPath: /app/public
containers:
  - name: tiny-httpd
    image: tiny-httpd:<tag>
    volumeMounts:
      - name: public
        mountPath: /app/public
        readOnly: true
volumes:
  - name: public
    emptyDir: {}
```

## Documentation

- [Operations reference](docs/operations.md) — request handling, shutdown,
  probes, metrics, error responses
- [ADR-001](docs/adr-001-tiny-static-http-server.md) — architectural
  decisions and design rationale
- [Contributing](docs/contributing.md) — development workflow and code
  guidelines
