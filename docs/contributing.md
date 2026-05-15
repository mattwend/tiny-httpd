# Contributing

## Prerequisites

- Rust + Cargo (stable)
- Podman for local container builds
- Docker Buildx for GitHub Actions container validation (`docker/setup-buildx-action`)
- `musl-tools` and the Rust target `x86_64-unknown-linux-musl` for the reference container build

## Development workflow

```bash
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

Run `cargo fmt` and `cargo clippy` before every commit. CI enforces
formatting, clippy, tests, coverage generation, and container validation.

## Commit conventions

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): description

Body explaining *why*.
```

Commit in logical parts — one concern per commit.

## Code guidelines

- Use `tracing`, not `println!`.
- All fallible functions return `Result<T, E>` with crate-local `thiserror`
  enums.
- Propagate errors with `?`; no `unwrap()` / `expect()` in production code
  (`#[cfg(test)]` is exempt).
- No silent error swallowing: dropped `Err` must be logged or propagated.
- Document functions including arguments and return values.
- When behavior changes materially, update durable docs in the owning
  component.
- Do not ignore or adapt tests without explicit consent.
