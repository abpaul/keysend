# Working in credseal

Read `README.md` for the workflow, the threat model and the file format.

Run before every commit:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo test --no-default-features --locked
```

- Library behavior: a test in `tests/load.rs` that calls the public API.
- Command line behavior: a test in `tests/cli.rs` that runs the binary.
- A change to the file format needs a new version and an updated README
  format reference. Keep fixed fixtures for existing versions.
- Error messages never contain the key, decrypted text, TOML excerpts or
  messages from `Deserialize` implementations. Assert the literal message.
