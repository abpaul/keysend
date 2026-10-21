# Investigations and Optimize in production

What is ready to run in production, what is switched off and why, what each costs, and how to switch each part on or off. `docs/agent.md` describes Investigations, `docs/optimization.md` Optimize and `docs/optimization-security.md` the security model.

## Readiness

| Part | State | Why |
|---|---|---|
| Investigations | Local safeguards verified; direct provider pending | Missing or invalid prices fail closed. Standard configured model rates are recorded below. Direct production-provider behavior has not been tested in this run. |
| Shadow Mode | Local implementation verified with seeded data; real-account validation pending | Uses the same deterministic decision path, stores non-executable records and feedback. The two independent fixtures have 4 unsafe cuts and 23 missed actions across 128 cases. Commercial gate B remains closed. |
| Approval-based changes | Not ready: switched off | Google's review of KeySend's management use is not done (`docs/optimization.md`, release gate). Tested against a simulated Google Ads and the REST stub only. |
| Autopilot | Not ready: switched off | The same, plus Google's review of unattended changes. |

## Provider integration

Investigations call the OpenAI Responses API directly (`services.ai_url`, default `https://api.openai.com/v1/responses`) with `store: false`, a strict JSON schema and the key from the credentials. The evaluation, by contrast, answers through `codex exec` behind a local proxy. No direct production request was made in this work: the evaluation measured the production instructions, schema, loop and checks, not the provider's latency, billing or caching. The first production investigations after a release are the verification: each stores its model calls and tokens, and each request logs its input, cached and output tokens.

## Cost

| Item | Measured | Limit in code |
|---|---|---|
| KeySend's own input per investigation | median 6,004 tokens, mean 6,572 (Round 3 requests, o200k tokenizer) | 80,000 tokens, checked before each request |
| Output per investigation | mean 573 tokens (Round 3) | 4,000 per round |
| Model requests per investigation | mean 2.2 | 3 rounds with operations, then one answer |
| Investigations per Workspace | | 20 in 24 hours |
| Estimated cost per investigation | needs `services.ai_prices` for the model | $0.25 when priced, checked before each request |
| Optimize evaluation | no model request | none needed |
| Optimize change | at least 5 Google Ads operations | the daily Google Ads reservation |

The earlier figure of 50,700 input tokens per investigation was the usage of the Codex harness and is about eight times KeySend's own (`docs/agent-evaluation.md`, "The size of KeySend's own requests"). Standard model prices are now configured for production. Missing or invalid prices refuse Investigation requests, alongside the token cap. With a price `P_in` and `P_out` in USD per million tokens, a typical investigation costs about `(6,000 × P_in + 573 × P_out) / 1,000,000`, and the worst case a Workspace can reach in a day is 20 investigations at the 80,000-token limit.

Estimated and actual cost are different numbers: KeySend estimates from the tokens the provider reports and the configured prices; the actual charge is the provider's invoice, which also prices cached tokens and other fees. KeySend does not read the invoice.

## Switches

| Setting | Default | Effect |
|---|---|---|
| `optimize.live` | `false` | Off: no change is sent; approving is refused and an approved change fails with "applying changes is switched off". |
| `optimize.max_mode` | `shadow` | Installation ceiling: Read-only, Shadow, approval-only (`recommend`) or Autopilot. A higher account mode cannot bypass it. |
| `optimize.actions` | `[adjust_campaign_budget, pause_campaign]` | The kinds that may run when live. Negative keywords, keyword pauses and targets are not implemented in the REST client and are refused whatever this says. |
| Workspace stop | off | The User's "Stop all changes"; no restart. |
| Policy mode | Investigate | Per Ad Account; set by the User. |

Settings are read at startup. More permissive applying requires an explicitly reviewed configuration change and all applicable release gates. Per-account policy changes cannot bypass the installation switch or supported mutation list.

## Google Ads compatibility

Research date: 2026-10-21. The [Google Ads research record](agent-research.md#google-ads-compatibility-and-compliance-research-2026-10-21) separates official documentation, local source inspection, and missing real-account evidence. It includes the primary source URLs. This work made no Google Ads calls, Cloud Console inspection, account modifications, or external submissions.

The client omits developer tokens in line with Google's [retirement instructions](https://developers.google.com/google-ads/api/docs/api-policy/developer-token). It uses REST v25, which the [version schedule](https://developers.google.com/google-ads/api/docs/sunset-dates) still supports. v26 is now latest. Neither fact proves the production project's access or the wire behavior of these mutations.

Source inspection found these compatibility limits:

| Check | Local evidence | Still needed |
|---|---|---|
| Project and OAuth | The client refreshes each Google Connection using the configured OAuth client and sends a bearer token and optional manager header. No matching Google/OAuth environment variable names were present in the research process. Credential stores were not opened. | Authorized confirmation of the owning project, API tier, OAuth publishing and verification status, and the User's Ad Account permissions. |
| Quota | Production config sets 2,880. The counter covers this database and resets by Los Angeles calendar date. Reads reserve each page. | Resolve the calendar-day versus Google's sliding-24-hour mismatch and account for other consumers of the project. The configured number does not establish the tier. |
| Mutation contract | One budget amount or Campaign pause per request, exact REST masks, validation first, partial failure disabled. Enable and other action kinds are refused by the current serializer. | Authorized validation of both permitted services against Google's actual schema and errors. |
| Mutation result | The adapter checks a result for the requested resource and treats malformed answers as Unknown. Execution separately reads resource identity and value before recording Applied. | Real response and read-back evidence, including malformed responses, failed reads, and timeout reconciliation. |
| Diagnostics | The adapter logs mutation status, codes, and request ID. Successful request IDs reach execution history. Failed request IDs are not returned by the error type. | Sanitized actual Google error envelopes and request IDs. Stub identifiers do not qualify. |

Existing tests exercise the simulator and local REST stub. This documentation review read those tests but did not rerun them. No real error, successful Google validation, mutation, or read-back was observed. API reads and `validateOnly` also require explicit account authorization before later verification. Applying remains off under the existing release gate.

### Operation evidence matrix

| Contract case | Deterministic local coverage | Real Google operation verified |
|---|---|---|
| Budget validation and amount mutation | Literal resource and amountMicros mask; REST stub; read-back identity and value | None |
| Campaign pause validation and mutation | Literal PAUSED/status mask; simulator; response parser | None |
| Invalid Campaign ID | Documented GoogleAdsFailure schema example and RESOURCE_NOT_FOUND classification | None |
| Shared budget | Proposal and live-state refusal, including a changed budget identity | None |
| Unsupported budget type | Non-DAILY budget refused; documented custom-period error schema example | None |
| Insufficient permissions | Provider-shaped authorization decoder and simulated revoked access | None |
| Stale resources | Two live reads; changed Campaign/budget/status conflict regressions | None |
| Quota errors | Provider-shaped quota decoder, HTTP 429 and local reservation tests | None |
| Authentication failure | Provider-shaped authentication decoder and revoked connection tests | None |
| Timeout or uncertain result | No resend; interruption, stale read-back, audit failure and reconciliation tests | None |

These schema examples are explicitly synthetic, not captured Google responses. They validate local serialization and parsing assumptions, not Google's acceptance. The ignored unit integration test `google_ads::mutate::tests::an_authorized_account_validates_the_exact_budget_and_pause_wire_requests` sends only validation requests through the production serializer and decoder to the real v25 endpoint. It requires `KEYSEND_CONTRACT_CUSTOMER`, `KEYSEND_CONTRACT_ACCESS_TOKEN`, `KEYSEND_CONTRACT_CAMPAIGN`, `KEYSEND_CONTRACT_BUDGET`, `KEYSEND_CONTRACT_BUDGET_MICROS`, and optionally `KEYSEND_CONTRACT_MANAGER`. Supply a short-lived token privately only after identifying the authorized account. Never include credentials or response bodies in tracked evidence. The test has not run. Test-account mutation and real read-back remain separate Gate A work; this harness never applies a change.

## Google Ads compliance and unsubmitted package

The controlling project release gate remains "Compliance and the release gate" in `docs/optimization.md`. This section records current research and prepares review text. It does not approve a release or certify compliance.

The [Developer Policies](https://support.google.com/adspolicy/answer/6169371) and the [RMF policy](https://developers.google.com/google-ads/api/docs/api-policy/rmf) raise separate questions about hosting, declared use, and tool classification. The research record explains their scope. An answer about one question does not establish the others.

The RMF notice rule allows material changes after the two-week notice period without a response. KeySend's gate additionally waits for Google's answer and review of unattended changes. That is the project's current release condition. There is no confirmed submission date or review outcome in this work.

### Draft Tool Change description

The following text is prepared for the change-description field of the [Tool Change Form](https://support.google.com/adspolicy/contact/tool_change). It has not been entered or submitted.

> KeySend is a hosted application used by Google Ads consultants, freelancers, and other Users outside our company. Today it reads their selected Ad Accounts to provide daily monitoring, Assessments, Investigations, and Exports. We propose adding Optimize, a limited management workflow. Applying changes is currently disabled.
>
> Optimize prepares a Proposal with a Campaign, its current value, the proposed value, evidence, risk, and observation period. Approval binds to that exact Proposal. The proposed initial release permits Campaign budget adjustments and Campaign pauses. The REST client does not implement keyword changes, negative keywords, or bidding-target changes. We do not create campaigns, ads, or ad groups through this workflow.
>
> A later Autopilot mode would let a User authorize a restricted policy per Ad Account. It bounds action kinds, budget steps and envelope, daily raises, cooldowns, and protected Campaigns. A Workspace stop blocks further changes. Optimize uses deterministic code, with no model in its decision or execution path. Investigations remain read-only.
>
> Users connect through their own Google OAuth consent. The hosted service uses KeySend's configured OAuth client and each Google Connection's stored refresh token. This is the architecture we ask you to assess. We are not asserting that per-User consent alone satisfies the current programmatic-use policy.
>
> Before execution, KeySend checks the Approval or policy authorization and reads the live Campaign and budget. It refuses stale or shared-budget changes, validates one exact update, checks authorization again, sends it, and reads it back. A Proposal becomes Applied only when the intended value is observed. Execution history records the authorization, before and after values, and available request ID. Uncertain results trigger read-back and reconciliation.
>
> Please assess approval-based changes separately from unattended Autopilot, confirm the permitted hosted credential model, and identify the applicable tool classification and any required access or policy changes. We will keep applying disabled while these questions remain unresolved under our release gate.

### Draft Compliance questions

These questions are prepared for the [Compliance contact form](https://support.google.com/adspolicy/contact/contact_ads_api), or an existing authorized review case. No case was opened.

1. Does this limited hosted Optimize workflow require each agency or end-advertiser to supply its own Cloud project and Ads API access? If so, what supported credential model should KeySend use?
2. Does individual Google OAuth consent plus a Proposal Approval satisfy the interactive-change requirement? What additional review or authentication is required for the proposed Autopilot policy?
3. For our actual project tier, does this use require a permissible-use update? At Standard, would you classify KeySend as a limited specialized tool, a full-service tool, or another category? Which RMF reports and management functions would apply?
4. Where should a project-based applicant submit the Tool Change Form when its required fields still reference a developer token and historical manager ID? What project identifier and contact confirmation should be supplied?
5. Does the attached proposed-change material meet the notice requirement, and does any separate review need to finish before approval-based changes or Autopilot can begin?

### Package fields and missing attachments

The owner must supply these facts privately before any authorized submission. Placeholders are intentional. No real account identifiers, secrets, or contact details belong in these tracked documents.

| Field or attachment | Prepared answer or remaining work |
|---|---|
| Outside-company access | Yes. |
| Internet access | Yes. |
| Tool URL | Use the published KeySend URL after owner confirmation. |
| Project, tier, and permissible use | Owner-provided project identifier and dated Console evidence. Not inspected. |
| Historical manager ID and API contact confirmation | Owner-provided facts if applicable. Ask Compliance how to complete legacy mandatory fields otherwise. |
| Contact email | Owner-provided monitored address. |
| Demo access | Pending an owner-approved review environment and access method. KeySend uses Google sign-in, so do not invent a password or share production credentials. Ask how Google wants to access the demo. |
| Screenshots or design document | Attach the current Proposal before approval, the applied or uncertain result history, Autopilot policy limits, Workspace stop, and authorization flow. Use seeded or sanitized data. Seeded Optimize, Shadow detail, policy and Proposal screenshots at 390 and 1280 pixels were captured and inspected on 21 October 2026. They show no horizontal overflow. Seeded uncertain-result and Workspace-stop captures are also prepared. Real-account and full authorization-flow attachments remain to be prepared. |
| Demonstration video | Optional form field. Not recorded. |
| Reporting and disclosure review | Attach a report-level RMF inventory if applicable, data-use and opt-out disclosures, and proposed replacement for any statement that KeySend never changes an Ad Account. Not audited here. |
| Verification evidence | Attach local test results only with their actual run date. Label simulations and REST stubs. Real validation, mutation, and read-back evidence are absent. |
| Intended effective date | Owner-chosen date after any applicable notice and unresolved review gates. No date is asserted. |

Preparing this package does not authorize its submission, account changes, real API checks, or enabling `optimize.live`.

## Deployment and rollback

The migration `m20261021_000021_optimization` only adds tables (`campaign_budgets`, `optimization_*`) and triggers; its `down` drops them. A rollback to a release without Optimize leaves these tables unread, which is harmless; rolling the migration back deletes the Proposals and their history, so do it only when that is intended. The `OptimizationWorker` must be registered in every release that the hourly scheduler can reach; a release without it leaves its jobs waiting in the queue.

## Release checklist

Migration `m20261021_000022_shadow` adds immutable Shadow records and append-only events, plus nullable change-resource identity on Account history. Rolling it back deletes Shadow evidence and removes that identity. Export required audit evidence before an intentional schema rollback.

1. Release with `optimize.live: false` (the default). Check that `/optimize` loads, that an Ad Account in Recommend mode shows Proposals or holds after its next daily run, and that no Google Ads mutate request appears in the logs.
2. Configure `services.ai_prices` for the production model so that the cost limit applies in money as well as tokens.
3. Follow "Compliance and the release gate" in `docs/optimization.md` before any `optimize.live: true`.

## Verified and not verified

Historical implementation evidence reported simulator and REST-stub execution tests, deterministic decision cases, the Investigation pre-request budget, seeded pages at 390 and 1280 pixels, and a seeded browser Approval. That earlier browser click is historical evidence, not an operation independently repeated in this hardening run. Current verification is recorded below with its actual scope.

Not verified: any request to the real Google Ads API, the production Cloud project's access level, Google's answer to the Tool Change Form, a direct OpenAI request, provider caching, and any Outcome on a real Ad Account.

### Current hardening verification prerequisites

The standard direct-provider `gpt-6.1-sol` rate is configured in `config/production.yaml` at USD 2 input and USD 10 output per million tokens. These rates were checked against the [official model page](https://developers.openai.com/api/docs/models/gpt-6.1-sol) on 21 October 2026. The investigation token cap is far below that page's long-context pricing threshold. Missing, zero, negative or non-finite prices now refuse the next Investigation request. A model override needs its own explicit price. Cached-input discounts are not subtracted from this conservative estimate. This is published rate verification, not a production invoice measurement. No direct provider request was made.

The real-contract harness `google_ads::mutate::tests::an_authorized_account_validates_the_exact_budget_and_pause_wire_requests` is ignored by default. It always sends `validateOnly: true`, calls the supported v25 REST endpoint, uses the production serializer and decoder, and never applies a change. It requires an explicitly authorized customer, Campaign, budget, amount and short-lived OAuth access token supplied privately in `KEYSEND_CONTRACT_*` environment variables. An optional manager ID uses `login-customer-id`. It does not add a developer token. Do not invoke it against an unspecified account or print its environment. No credentials were available for this run, so the harness is not integration evidence.

Deterministic decoder cases use the documented GoogleAdsFailure envelope and documented error enum values. They are schema examples, not captured provider responses. The adapter now treats malformed, mismatched-resource or partial-failure success responses as Unknown. It requires a result for the requested resource on a real mutation and accepts an empty validation response. The live-state query also requires an explicitly DAILY budget period. Unknown or campaign-total periods fail closed before validation. This conservative client restriction also excludes Campaign pauses when their budget period is unsupported.

Real verification still required for each of budget validation, budget mutation, pause validation, pause mutation, invalid IDs, shared budgets, campaign-total budgets, insufficient permissions, replaced resources, quota errors, revoked authentication and uncertain transports. Do not induce quotas or change production resources to collect an error. Capture sanitized evidence from authorized test accounts or existing incident records. A local schema example never closes that row. Production validation-only verification also needs account-specific authorization and remains unperformed.

## Controlled rollout gates

All gates start open. A passing local suite cannot close a real integration or commercial gate. No automatic transition or background job may advance an account's mode. The policy enum supports Investigate, Shadow, Recommend and Autopilot; `optimize.live` remains false by default and `optimize.actions` permits only budget adjustment and Campaign pause. The serializer independently refuses every other mutation.

Gate A requires authorized official test-account budget and pause validation, mutations and resource-aware read-back, with API version, sanitized request IDs and actual response evidence. No real operation has been verified in this run. Validation-only requests do not close the mutation part.

Gate B requires review of the independently authored 120 primary and 8 supplemental commercial cases, explicit adjudication of severe disagreements, and an agreed acceptable recommendation standard. The case labels are provisional judgments, not commercial ground truth. Rule compliance is reported separately. An unresolved severe decision defect keeps this gate open regardless of the aggregate score.

Gate C requires explicit consent for Shadow on real accounts, explainable recorded decisions, useful abstentions, consultant feedback, sufficient data coverage and evidence of zero mutations. Seeded browser and stub tests are necessary local evidence but do not close this real-account gate.

Gate D requires one operator-authorized production account, one exact approved budget reduction or Campaign pause, compliance clearance, reviewed legal and consent language, current account access and tracking, a known rollback procedure that does not write automatically, and verified read-back. Obtain explicit approval naming the account, resource and change before sending. This run is not that authorization.

Gate E requires compliance clearance for unattended changes, accepted commercial evidence, operational recovery drills and explicit bounded Autopilot authorization. Budget increases and Campaign pauses do not become automatic. Suspensions need a deliberate resume. Do not use consultant acceptance alone as an efficacy measure.

Before any gate transition, record the operator, scope, time, evidence, unresolved risks and the exact configuration revision. Retain existing Investigations and Assessments in Investigate. Starting Shadow requires its own policy choice and consent. Existing unexecuted shadow records stay non-executable after a later mode change. Stop, account removal, lost Access and policy revocation must be checked server-side. A stop does not recall a request Google already received.

The installation also has a typed `optimize.max_mode` ceiling, default Shadow. Turning on `live` alone cannot activate older write-capable policies. Approval-based release requires an explicit ceiling of `recommend`; Autopilot requires `autopilot` as well as a current account policy. The policy form refuses a mode above the ceiling and never silently substitutes a lower mode or enrolls an account into Shadow. More permissive ceilings are operator-reviewed configuration changes, not automatic gate transitions. Test fixtures explicitly allow the full ceiling only against local stubs and simulators.

### Browser verification on 21 October 2026

The signed-in seeded server rendered `/optimize`, `/optimize/shadows/1`, an Ad Account policy page and a budget-cut Proposal at 390 and 1280 pixels. All eight captures returned HTTP 200 with zero horizontal overflow. Four additional captures show the uncertain-recovery form and stopped Workspace at both widths, also with HTTP 200 and zero overflow. The mobile captures show stacked values and readable controls; desktop captures show the evidence and policy limits without clipping. Shadow decisions are collapsed except the first proposed decision. The non-executing notice and disabled Approval are visible.

Actual Chromium form submissions saved Useful feedback and returned to the open decision, refused Shadow without consent with HTTP 422, then saved Shadow after explicit consent. The installation ceiling disabled Autopilot and explained an older policy above the ceiling. These were local fixture operations, with fake provider services. They do not verify real consent, Google delivery or commercial results. The existing read-only footer remains unchanged pending approval of the replacement wording in `docs/optimization.md`.

### Repository verification on 21 October 2026

The independent baseline `scripts/check.sh` completed successfully with 384 library tests and 458 request tests. The hardening check completed successfully with 409 library tests and 504 request tests, zero failures, six ignored library tests and nine ignored request tests. The ignored real Google validation harness was not run. Formatting, Clippy with warnings denied, script checks and public-export guards passed. Migration and database-diagram checks passed; the diagram was regenerated. Legacy deterministic rule compliance remains 78/78 under the documented revised generator and disabled-raise expectations.

Regression evidence covers duplicate delivery, policy revocation during validation, access and stop controls, budget replacement, old-value read-back, lost answers, audit persistence failures, uncertain recovery, late reconciliation after recovery, feedback reversals, retained observations and actual data coverage. A recording HTTP server proves that the real adapter sends no validation or mutation request under Shadow; switching that same fixture to Recommend permits the positive-control validation request. These tests use local services and do not establish Google's actual behavior.

The uncertain-page screenshots use a manually constructed state in disposable seeded storage. They prove layout and wording only; request tests provide the timeout and reconciliation evidence. The browser Workspace-stop submission returned HTTP 200, showed the stop banner and left the uncertain change unresolved. No recovery report was submitted in that browser session.
