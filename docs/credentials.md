# Encrypted credentials

KeySend reads secrets from ENV, or from an encrypted TOML file when
`CREDSEAL_MASTER_KEY` is set. Each environment has its own file and its own
unlocking key. The application name is `keysend`.

## Create or edit an environment

Install the standalone editor from the package included in this checkout:

```sh
cargo install --path vendor/credseal --features cli --locked
credseal edit -e development
credseal validate -e development
```

With `CREDSEAL_MASTER_KEY` unset, the editor uses
`config/credentials/development.key`. For a new environment it generates a
random key, saves it with owner-only permissions, and adds a rule to the
credentials directory's `.gitignore` before writing the key. Later edits
and validation reuse that file. A present ENV value takes precedence,
including an invalid or empty value, which causes an error.

If an encrypted file exists but its key is missing, restore the key from your
secret manager. The editor does not generate a replacement for existing
credentials.

`edit` opens `config/credentials/development.toml.enc` in `$VISUAL`, then
`$EDITOR`, or `vi`. It creates the file if it does not exist. It reads the
application name from `Cargo.toml`. For staging, run
`credseal edit -e staging`. `--editor 'code --wait'` selects another editor.

Commit the encrypted file. Keep its key outside Git. The `.gitattributes`
rule preserves the encrypted bytes when Git converts other files to CRLF.
No credentials editor is added to `keysend-cli`.

`validate` checks decryption and TOML syntax. It does not check KeySend's
field names or required values. Use the flat fields below, rather than
the generic editor template's nested example. Starting KeySend checks its
credentials schema and production requirements.

## Set the values

The TOML fields are optional. An absent value disables the corresponding
service, as it does in ENV mode. Production requires `keysend_master_key`.
Unknown fields are rejected.

| TOML field | Existing ENV setting |
|---|---|
| `google_client_id` | `GOOGLE_CLIENT_ID` |
| `google_client_secret` | `GOOGLE_CLIENT_SECRET` |
| `keysend_master_key` | `KEYSEND_MASTER_KEY` |
| `stripe_secret_key` | `STRIPE_SECRET_KEY` |
| `stripe_webhook_secret` | `STRIPE_WEBHOOK_SECRET` |
| `openai_api_key` | `OPENAI_API_KEY` |
| `cloudflare_email_token` | `CLOUDFLARE_EMAIL_TOKEN` |
| `report_email_recipient` | `REPORT_EMAIL_RECIPIENT` |
| `postdeploy_ingest_key` | `POSTDEPLOY_INGEST_KEY` |

An empty or missing recipient restriction permits normal User delivery. A configured
value permits only that exact email address.

The same flat file also accepts these operator credentials. They are not
application settings. The operator helper passes them only to the selected
command scope.

| TOML field | Operator setting | Scope |
|---|---|---|
| `kamal_registry_password` | `KAMAL_REGISTRY_PASSWORD` | Deploy |
| `cloudflare_cache_token` | `CLOUDFLARE_CACHE_TOKEN` | Deploy |
| `cloudflare_origin_certificate` | `CLOUDFLARE_ORIGIN_CERTIFICATE` | Deploy |
| `cloudflare_origin_key` | `CLOUDFLARE_ORIGIN_KEY` | Deploy |
| `ci_ssh_key` | `CI_SSH_KEY` | Deploy |
| `ci_known_hosts` | `CI_KNOWN_HOSTS` | Deploy |
| `r2_access_key_id` | `RCLONE_CONFIG_KEYSEND_R2_ACCESS_KEY_ID` | Backup |
| `r2_secret_access_key` | `RCLONE_CONFIG_KEYSEND_R2_SECRET_ACCESS_KEY` | Backup |
| `r2_endpoint` | `RCLONE_CONFIG_KEYSEND_R2_ENDPOINT` | Backup |
| `age_recipient` | `AGE_RECIPIENT` | Backup |

Use TOML multiline strings for PEM certificates, private keys and SSH host
entries. Keep their complete contents, including BEGIN and END lines.
`age_recipient` is the public encryption recipient. Store the private age
identity separately for restoration.

The application can decrypt every field in its credentials file. One file
and one unlocking key do not isolate deployment secrets from the application.
Do not publish the real encrypted file in a public source snapshot.

Keep the current `KEYSEND_MASTER_KEY` value as `keysend_master_key` when
moving to an encrypted file. It encrypts stored Google refresh tokens.
Changing it would make those tokens unreadable. It is separate from the
key that opens the credentials file.

For production, run `credseal edit -e production` to create or reuse the local
`config/credentials/production.key`. Store this unlocking key in your secret
manager separately from the existing KeySend data key. Supply its value as
`CREDSEAL_MASTER_KEY` to the deployed container. The application reads only
ENV for unlocking; local key files are an editor convenience and are excluded
from the Docker image.

Non-secret settings such as `HOST`, `DATABASE_URL` and `CLOUDFLARE_ACCOUNT_ID` keep their
existing configuration paths.

## Start the application

Select the same environment for the editor and the application:

```sh
credseal edit -e staging
CREDSEAL_MASTER_KEY="$(cat config/credentials/staging.key)" \
  cargo loco start --environment staging
```

A staging Loco configuration must also exist. The editor does not create it.

If `CREDSEAL_MASTER_KEY` is present, even if empty, KeySend requires a valid
key and the selected encrypted file. A wrong key, altered file, wrong
environment or invalid TOML stops startup. No field falls back to ENV.
If the unlocking variable is absent, KeySend uses its existing ENV settings.

Startup validates credentials before creating the application context.
The context holds a resolved snapshot for HTTP handlers, tasks and workers.
Requests and jobs do not reopen the encrypted file. Restart the application
after changing credentials. Plaintext is never stored in Loco's printable
configuration.

Run the fixture-based test suite without your application unlocking key:
`env -u CREDSEAL_MASTER_KEY cargo test --locked`. The credentials tests
supply their own public fixture keys.

The file directory defaults to `config/credentials`. Set
`settings.credentials_directory` in the Loco configuration and pass the same
directory with `credseal edit -e staging --directory /path/to/credentials`
when another location is needed.

## Check or use operator credentials

Build the application-specific helper:

```sh
cargo build --locked --bin keysend-credentials
CREDSEAL_MASTER_KEY="$(cat config/credentials/production.key)" \
  target/debug/keysend-credentials status --environment production
```

The helper prints only readiness booleans for payments, error reporting,
deployment and backups. It checks local configuration, not provider access.
It requires the unlocking key in ENV; it does not search for a local key file.
The editor remains the standalone `credseal` command.

`keysend-credentials exec --scope deploy|backup --environment NAME -- COMMAND`
runs the selected command directly. It removes inherited credential variables
before setting those of the selected scope. A backup child receives the age
recipient and the `keysend_r2` rclone settings, including region `auto`; it does
not receive the unlocking key. Set its non-secret destination as
`RCLONE_REMOTE=keysend_r2:BUCKET`. A deployment child keeps the unlocking key so
its deployment tool can supply it to the application container.

The deployment scope requires registry, cache, origin TLS and CI SSH fields.
The backup scope requires the three R2 fields and the age recipient. Missing
required fields, NUL bytes and an invalid key stop execution. The child keeps
its own exit status. These helper commands require sealed credentials; the
application and existing operator entry points retain their legacy ENV mode
when the unlocking variable is absent.

## Update the included crate

The package in `vendor/credseal` is a snapshot of the standalone crate.
This keeps a checkout buildable without a sibling directory or a published
registry package. Update the standalone source first, then run:

```sh
python3 scripts/vendor-credseal.py /path/to/credseal
python3 scripts/vendor-credseal.py --check /path/to/credseal
python3 scripts/vendor-credseal.py --verify
```

The sync command preserves the license and records source hashes in
`vendor/credseal/SOURCE.json`. It refuses to overwrite local edits to the
snapshot. CI verifies the hashes without access to the standalone checkout.
