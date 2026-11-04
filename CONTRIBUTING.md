# Contributing to Ordinata

Bug reports, numerical test cases, documentation corrections and focused pull requests are welcome. Open an issue before starting a large change.

By submitting a contribution, you agree that it is licensed under the [Apache License 2.0](LICENSE), as described in section 5 of that license.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Numerical changes need an independently derived reference case in `fixtures/`. Include your Ordinata version, operating system and a synthetic example in bug reports; do not attach real measurement data.

Report security problems privately as described in [docs/SECURITY.md](docs/SECURITY.md).
