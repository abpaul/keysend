# credseal

Encrypted, per-environment TOML credentials for Rust applications, with the
workflow of Rails credentials: commit the encrypted file, keep the key out of
the repository, and change secrets with `credseal edit`.

credseal copies the Rails workflow, not its file format. It cannot read
`credentials.yml.enc` files.

- One file per environment: `config/credentials/<env>.toml.enc`.
- One 256-bit key per environment. Runtime loading uses `CREDSEAL_MASTER_KEY`;
  the CLI can create and reuse an ignored local key file.
- AES-256-GCM. The application and environment names are authenticated, so a
  file decrypts only for the application and environment it was written for.
- Typed loading with `serde`. `SecretString` redacts its value in debug output.

Version 0.1.0. Not published to crates.io. Not independently audited.

## Install

Rust 1.94 or newer. Use a local path dependency:

```toml
[dependencies]
credseal = { path = "../credseal" }
serde = { version = "1", features = ["derive"] }
```

Install the command line tool, which needs the `cli` feature:

```sh
cargo install --path ../credseal --features cli --locked
```

## Workflow

In the directory of a Rust package, credseal takes the application name from
`[package] name` in `Cargo.toml`:

```sh
credseal edit -e staging             # creates the local key and encrypted file
credseal validate -e staging
git add config/credentials/staging.toml.enc
```

Outside a Rust package, or to use another name, pass `--app`:

```sh
credseal edit --app myapp -e staging
```

When `CREDSEAL_MASTER_KEY` is unset, `init` and `edit` create a random
`config/credentials/<env>.key` for a new credentials file. The CLI adds
`/<env>.key` to that directory's `.gitignore` before writing the private key.
Subsequent commands reuse the key without changing `.gitignore`; keep the
generated rule in place. An existing encrypted file without its
key fails; it never receives a replacement key. `validate` never generates keys.

Commit `<env>.toml.enc` and the generated `.gitignore`. Never commit a key.
Back up the local key securely: losing it makes the encrypted credentials
unreadable. Add `*.toml.enc.lock` and `.env` to the project `.gitignore`.

For deployment, put the local key's Base64 value into your secret manager and
supply it as `CREDSEAL_MASTER_KEY`. Runtime library loading does not search
for local key files. A set ENV key always takes precedence; an empty or invalid
ENV key fails without falling back to a local file.

Add this rule to `.gitattributes` so Git preserves the encrypted bytes even
when a checkout converts text files to CRLF:

```gitattributes
*.toml.enc -text
```

## Load credentials

```rust
use credseal::{Context, CredentialsFile, MasterKey, SecretString};
use serde::Deserialize;

#[derive(Deserialize)]
struct Credentials {
    stripe: Stripe,
}

#[derive(Deserialize)]
struct Stripe {
    publishable_key: String,
    secret_key: SecretString,
}

fn load() -> Result<Credentials, credseal::Error> {
    let key = MasterKey::from_env()?;
    let context = Context::new("myapp", "production")?;
    CredentialsFile::new("config/credentials/production.toml.enc", context).load(&key)
}
```

Call `credentials.stripe.secret_key.expose_secret()` where the value is used.
`examples/typed.rs` is a complete program.

The application decides which environment it runs in and which fields are
required. credseal has no environment fallback.

## Commands

All commands except `keygen` require the environment: `-e`, `--environment`
or `--env`. `--app` defaults to the package name in `Cargo.toml` in the
current directory. Without `--app`, a command fails when that file is missing,
is a symbolic link, is not TOML, or has no `[package] name`, as in a virtual
workspace manifest. The error never shows the content of `Cargo.toml`.
`--directory` defaults to `config/credentials`. Names are 1 to 64 characters
from `a-z`, `0-9`, `_` and `-`.

| Command | Does |
|---|---|
| `keygen` | Prints a new random key and a newline to standard output. |
| `init` | Creates `<env>.toml.enc` with a commented, empty TOML document. Refuses to overwrite. Does not open an editor. |
| `edit` | Decrypts into a private temporary file, opens the editor, checks the TOML and encrypts the result. When `<env>.toml.enc` does not exist, starts from the template of `init` and creates the file. Also named `credentials:edit`. |
| `validate` | Decrypts and checks that the text is TOML. Prints no secret. |

There is no command that prints the decrypted text and no rekey command.

`edit` runs `--editor`, else `$VISUAL`, else `$EDITOR`, else `vi`. The command
is split into words like a shell does, but no shell runs it, so `$(...)`, `;`
and `|` stay literal arguments. The file path is the last argument.
`CREDSEAL_MASTER_KEY` is removed from the editor's environment. Use an editor
command that waits until the file is closed, such as `code --wait`.

On Unix, the parent keeps running when you type Ctrl-C or Ctrl-\ in the
editor. The editor handles those signals itself. If the editor stops with
an error or a signal, credseal discards the edit and removes its temporary
plaintext file.

`edit` keeps the encrypted file unchanged, byte for byte, when:

- the editor exits with a non-zero status;
- the edited text is not TOML, or is larger than 1 MiB;
- the edited file was replaced by a symbolic link or another non-regular file;
- the text did not change.

When the file did not exist, the same failures create no file. If you exit the
editor without changes, `edit` creates the file with the template.

In the first three cases the edits are discarded. Copy them out of the editor
before you exit if you want to keep them.

### Concurrent edits

`init` and `edit` hold an exclusive lock on `<env>.toml.enc.lock` for the whole
command. A second `init` or `edit` of the same file fails at once. Different
environments use different locks. Updates to the shared `.gitignore` also
hold an exclusive lock on that file. The lock file stays after the command; do
not delete it while a command runs.

Before it writes, `edit` reads the encrypted file again. If another process
changed it, `edit` keeps that change and discards your edits. The lock is
advisory: a program that does not use credseal can still write between this
check and the replacement.

### Writes

The new encrypted file is written to a temporary file in the same directory,
synced, and renamed over the old file. The directory is then synced on Unix.
`init`, and `edit` of a new file, use a rename that fails when the file
exists. `edit` then keeps the other file and discards your edits.
If syncing the directory fails after replacement, the command exits with an
error that says the credentials were saved but may not survive a crash.

## Keys

Use one key per environment. A development key must not decrypt production
credentials.

The library reads the key from `CREDSEAL_MASTER_KEY` or from a string that
the application passes to `MasterKey::from_base64`. It does not search for key
files. The CLI first checks ENV, then `<directory>/<env>.key`, and only creates
a key when `init` or `edit` creates a new environment. Key files must be regular
files, not symbolic links. New key files have mode `0600` on Unix and are synced
before the encrypted file is created. A new key remains if the editor fails.

Within a Git worktree, the CLI refuses a local key path that Git tracks, even
if the file was deleted from disk. Before generating a key, it checks that Git
ignores the path. Without Git, the generated `.gitignore` protects future
checkouts. Do not force-add key files. Store deployment keys in a secret manager
or in the deployment platform's secret settings.

### Rotate a key

There is no rekey command. Rotate by hand:

1. `credseal keygen` and store the new key.
2. `mkdir old && mv config/credentials/production.toml.enc old/`
3. `CREDSEAL_MASTER_KEY=<old> credseal edit -e production --directory old`
   to view the values. Exit without changes.
4. `CREDSEAL_MASTER_KEY=<new> credseal edit -e production` and enter the
   values.
5. Deploy the new key with the new file. Delete `old/` and the old key.

The old file stays in Git history. If the old key leaked, also rotate every
secret that the file contained.

## Threat model

credseal protects the confidentiality and integrity of a committed file
against anyone who does not have the key. A reader with the repository cannot
read the values. A writer cannot change values, swap files between
environments or applications, or downgrade the version, without failing
decryption.

credseal does not protect against:

- anyone who has the key, or who can read the environment of the process;
- a compromised machine, editor, editor plugin or shell;
- copies that the editor makes outside the temporary directory, such as
  backups, undo files or swap files in another location;
- memory copies made by the TOML parser and `serde`, and swapped memory;
- plaintext left on disk if `credseal edit` crashes or is killed with
  SIGTERM or SIGKILL. The temporary
  directory is removed on every normal exit. Removal is not secure deletion;
- a program that writes the credentials file without the lock;
- replay of an older file encrypted with the same key and names.

Error messages never contain the key, decrypted text, TOML excerpts or
messages from `Deserialize` implementations. They can contain file paths and
line numbers.

## Limits

| What | Limit |
|---|---|
| Encrypted file | 2 MiB |
| Decrypted TOML | 1 MiB |
| Application and environment names | 1 to 64 bytes |

The credentials file, the edited file and the lock file must be regular files,
not symbolic links. Symbolic links in the directories above them are followed.

## File format, version 1

```text
credseal-v1\n
<standard padded Base64 of: nonce (12 bytes) || ciphertext || tag (16 bytes)>
```

The second line may end with one `\n`. No other whitespace is allowed.

- Cipher: AES-256-GCM, 96-bit nonce from the operating system random number
  generator, new for every encryption.
- Associated data: `credseal-v1\n || len(app) || app || len(env) || env`. Each
  length is one byte. The names come from the caller, not from the file.
- Plaintext: UTF-8 TOML.

A file with another `credseal-v<n>` first line fails with
`Error::UnsupportedVersion`. The tests load generated malformed and valid
files, plus a fixed file encrypted with Ruby/OpenSSL. The latter checks the
format against a separate AES-GCM implementation. Its generator and public
test key are in `tests/fixtures/generate.rb`. Regeneration requires Ruby 3
or newer linked against OpenSSL. The generator uses a fixed public test key
and nonce. The Rust tests need no Ruby.

## Platforms

The tests run on macOS. Linux uses the same POSIX calls, but the tests have
not run on it yet. Windows is not tested: file modes are not set there, and
the command line tests do not run.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
