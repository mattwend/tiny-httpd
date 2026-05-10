# Contributing

## Prerequisites

- Rust + Cargo (stable)
- Podman for local container builds
- Docker Buildx for GitHub Actions container validation and publishing

## Development workflow

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Run `cargo fmt` and `cargo clippy` before every commit. The CI pipeline
enforces both.

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
- Do not ignore or adapt tests without explicit consent.
