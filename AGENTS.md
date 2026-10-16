# Working in KeySend

Read `README.md` for the system, `docs/features.md` to find a feature, and `docs/architecture.md` for the modules and the rules. `CONTEXT.md` defines the terms; use them in code, text and tests.

## Check a change

Run `scripts/check.sh` before every commit. Manual CI and the pre-push hook
use the same command. Set `git config core.hooksPath .githooks` to enable the
hook. The command runs formatting, Clippy, Rust tests and script checks with
private disposable SQLite storage. It keeps failure evidence and never uses a
shared development database.

When `README.md` has a section "Public repository", follow it: it defines which
files leave this repository.

| Change | How to show that it works |
|---|---|
| Logic, a route, a job, an email, the Export | A request test in `tests/requests/` that calls the route or runs the job, or a unit test beside the code. Give the test name and the result line |
| A public page | `cargo loco start`, then the screenshots below with `http://localhost:5150` |
| A signed-in page | The seeded server and the screenshots below |
| The schema | A test in `tests/requests/migration.rs`. Then write `docs/database.md` again: `KEYSEND_WRITE_DIAGRAMS=1 cargo test --locked --test mod database_diagram` |

A passing test shows the HTML. Only a screenshot shows the layout. For a change that a user can see, take both screenshots, look at them, and state what each one shows.

## Which test to write

- A route, a job, an email or the Export changes what a User or operator gets: write a request test in `tests/requests/`. Call the route or run the job. Assert the status, the redirect, the words and numbers that the User reads, or the call to Google, Stripe, the AI or email.
- A rule with many cases (detection, findings, masks, plurals, formats, chart geometry): write a unit test beside the code. Add one request test only when the wiring is new.
- A bug: first write the test that fails for the reported behavior. Then fix. Give the failing line and the passing line.
- A new rule or threshold: write the test first, with a literal expected value.
- Copy, CSS, layout, or a template with no new logic: no new test. The two screenshots are the evidence.
- JavaScript, or a form that a browser submits: run the seeded server and drive the page with `scripts/chromium.mjs`. State what you saw.
- Name a test as a sentence about the User, the Ad Account or the operator, in the terms of `CONTEXT.md`: `a_subscription_ends_when_its_period_ends`. No ticket numbers, no function names.
- Assert through the interface. Read `WorkspaceStore` in an assertion only when no page, email or outgoing call shows the result.
- Do not assert CSS classes, element nesting, marketing sentences, the number of elements, SQL text, or what Loco, Axum or SeaORM do. Page budgets are in `tests/requests/app/perf.rs`.
- On a public page, do assert the links and their sign-up sources, the price and the Trial length, the structured data, and the security and cache headers.
- A job that the queue runs in production needs one test with the real queue: `Fixture::queued_at`.

## Signed-in pages in a browser

`cargo loco start` cannot sign in without Google credentials. The seeded server has a signed-in User with Ad Accounts and Updates:

```sh
DIRECTORY=$(mktemp -d)
scripts/serve-seeded.sh "$DIRECTORY"
KEYSEND_SESSION=$(cat "$DIRECTORY/cookie") node scripts/screenshot.mjs "$DIRECTORY/shots" \
  "http://$(cat "$DIRECTORY/address")" /ad-accounts /settings
touch "$DIRECTORY/stop"
```

`scripts/screenshot.mjs` writes one PNG at 390 px and one at 1280 px for each path, and one line with the status and the overflow in pixels. Give `scripts/serve-seeded.sh` the test `requests::app::demo::serve_harbour_dental_to_a_browser` as its second argument for the Ad Account of the home page examples. Set `KEYSEND_CHROMIUM` when no Chromium is found.

## Report

State what changed, which features of `docs/features.md` it affects, which checks ran with their results, and what was not verified.
