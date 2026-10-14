# One shared SQLite database, with every row owned by a Workspace

KeySend keeps all Workspaces in one SQLite file, and every domain row carries its Workspace. We chose this because the hourly job that queues the run of each Ad Account must read across all Workspaces, and because KeySend has no need to export, restore, or delete one Workspace as a single file.

## Considered Options

- One SQLite database per Workspace, as in CrabGate: single-file export, restore, and deletion per tenant, but migrations run once per file, and each job that spans Workspaces must open every file.
- Postgres: better concurrent writes, but another service to operate on a shared server.

## Consequences

- A query without a Workspace filter can leak data. All reads and writes of domain rows go through one data-access layer that requires a Workspace.
- Deletion of one Workspace is a set of row deletions, not a file removal. Backups keep the deleted rows until the backups expire.
