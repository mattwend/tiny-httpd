# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-05-17

### Added
- Static file serving from a configurable content root with safe path resolution.
- Embedded default welcome page when no `index.html` is present.
- Support for `GET` and `HEAD` requests.
- Kubernetes liveness and readiness probes.
- Graceful shutdown with readiness drain for rolling updates.
- Configurable HTTP/1 header-read, idle-connection, graceful-close, and drain timeouts.
- Structured tracing and OpenTelemetry HTTP metrics.
- Container build for a static musl binary in a `scratch` image.
- Unit, integration, and doctest coverage for core behavior and edge cases.
