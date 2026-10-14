# Contributing

## Check a change

`AGENTS.md` says which test to write. Run these before you open a pull request; CI runs the same commands.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

## How a change arrives

The canonical source is maintained in a private repository. This repository receives snapshots of it, so its history is a series of exports.

A pull request is not merged here. The maintainer applies it to the canonical source and the change arrives with the next snapshot. A direct edit in this repository is lost at the next snapshot.

The production content, the deployment files and the release scripts are not part of this repository.
