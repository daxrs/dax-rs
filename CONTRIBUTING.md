# Contributing to dax-rs

dax-rs is in early alpha with a single maintainer — issues and PRs are welcome.

## Getting Started

```sh
cargo run --bin generate-demodata   # generates a demo dataset
cargo run --bin dax-rs              # runs the server against it
```

See [`docs/RUNNING.md`](docs/RUNNING.md) for more.

## Before Starting Major Work

For anything beyond a small fix — a new feature, a significant refactor, a new storage backend, etc. — please open a Feature Request issue first and let the direction get validated before you invest a lot of time. This avoids PRs that don't fit the intended architecture or overlap with work already in progress.

## Before Submitting a PR

CI runs the following on every push — please check locally first:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

## Code Style

- Match `rustfmt.toml` (run `cargo fmt`).
- Enums must be matched exhaustively — no catch-all `_` arms on `match` unless a comment explains why.

## Reporting Issues

- Use the issue templates (DAX Query Issue / Other Bug / Feature Request).
- **Security vulnerabilities**: do not open a public issue — see [`SECURITY.md`](SECURITY.md).

## License

By contributing, you agree your contribution is licensed under the project's [AGPL-3.0-or-later](LICENSE).
