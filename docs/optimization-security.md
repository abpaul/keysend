# Optimize security

Optimize can change a customer's advertising spend, so every boundary of Investigations (`docs/agent.md`, "Authorization and security") holds here too, and more. This page lists the boundaries, the threats and the test that covers each, and the independent review of 21 October 2026.

## Boundaries

| Boundary | Where it holds |
|---|---|
| A change exists only as an `optimize::Action`: a closed enum with typed IDs and amounts, unknown fields refused | `src/optimize/mod.rs` |
| Code builds every Action from Signals. No model output, form text or search term becomes a kind, an ID or an amount | `src/optimize/propose.rs` |
| Only `optimize::execute::run` sends a change, and only through `Ads` | `src/optimize/execute.rs` |
| The REST client sends two kinds of change, budget amount and Campaign status, one operation each, with resource names built from the Ad Account that `Client::connect` resolved in the Workspace and from integer IDs | `src/google_ads/mutate.rs` |
| Applying is off unless `settings.optimize.live`; the kind must also be in `settings.optimize.actions` and in `execute::SENDABLE` | `execute::live` |
| Every SQL statement filters on the Workspace; inserts select the Ad Account within the Workspace | `src/workspace/optimization.rs` |
| `/ad-accounts/{id}/optimize*` use `OwnedAdAccount`; Proposal routes read the Proposal within the Workspace and answer not-found otherwise | `src/controllers/optimize.rs` |
| POST routes pass the same-origin check | `src/security.rs` |
| An Approval binds the fingerprint of the exact change; a trigger refuses any change of a Proposal's change or fingerprint | store and migration |
| The policy is checked when a Proposal is made, before the claim, against the live value, and right before each request that changes Google Ads | `policy::check`, `execute::still_allowed` |
| The day's changes and raises are counted, and a second change of an Ad Account is refused while one runs, in the claim's write transaction | `claim_execution` |
| The history and the policy versions are append-only (triggers) | migration |
| Logs hold the service, the status, the request ID and Google's error codes; no token or request body | `mutate.rs` |

## Threats and tests

Request tests are in `tests/requests/app/optimize.rs` unless stated.

| Threat | How it is prevented | Test |
|---|---|---|
| Cross-Workspace change | Workspace filter on every row; not-found routes | `another_workspace_cannot_see_approve_or_run_a_proposal` |
| Ad Account or resource substitution | IDs are typed; resource names come from the resolved Ad Account; the budget ID, amount and sharing are checked against the live Campaign | `a_budget_that_someone_changed_since_the_proposal_is_not_overwritten`, `mutate::tests::a_change_names_only_resources_of_its_ad_account_and_masks_only_its_field` |
| Approval replay or approval of another change | Approval moves only an open, unexpired Proposal with the submitted fingerprint; the claim requires it | `an_approval_binds_the_exact_proposal_and_a_proposal_cannot_change_after_it` |
| Duplicate delivery and retries | The claim moves Approved to Applying once; a lost answer is reconciled only by reading, never by resending | `an_approved_budget_cut_runs_once_and_is_read_back`, `a_lost_answer_is_settled_by_reading_the_budget_and_the_change_is_not_sent_twice`, `a_change_that_timed_out_is_not_sent_again_when_the_old_value_is_read`, `a_change_without_an_answer_from_google_ads_is_never_sent_again` |
| Uncertain results | Uncertain blocks the Ad Account until a read settles it; reconciliation never sends | `an_uncertain_change_is_reconciled_by_a_read_without_being_sent_again` |
| Concurrent workers past a limit | The claim counts in one write transaction | `two_workers_cannot_pass_the_daily_change_limit_together`, `raises_are_refused_without_a_current_account_wide_budget_total` |
| Shared budgets | Refused when proposed and against the live value | `a_shared_budget_is_never_changed`, `optimize::eval` (`shared_budget`) |
| Stale authorization, policy changed, stop pressed | Rechecked before the claim and before each change request | `a_change_the_autopilot_authorized_does_not_run_after_the_user_switches_it_off`, `the_workspace_stop_refuses_approved_changes_until_the_user_resumes`, `a_stop_pressed_while_a_change_is_read_prevents_the_change` |
| Revoked Google access | The change fails and the Autopilot is suspended | `a_revoked_google_connection_fails_the_change_and_suspends_the_autopilot` |
| Policy bypass through the form | Unknown modes, kinds and Campaigns refused; limits bounded; High risk kinds cannot be automatic; saving never resumes a suspended Autopilot | `policy::tests::a_policy_cannot_let_a_high_risk_kind_run_by_itself`, `protecting_a_campaign_keeps_a_suspended_autopilot_suspended` |
| Prompt injection through search terms or names | No model in Optimize; names are escaped text; a term is only the text of a negative keyword, which the REST client cannot send | `optimize::eval` (injected names and terms), `applying_is_refused_while_the_installation_is_not_live` |
| Refused or malformed responses | 4xx and `partialFailureError` are failures; 5xx and transport errors are Unknown and read back; only a read-back makes Applied | `a_refused_validation_changes_nothing`, `a_change_without_an_answer_from_google_ads_is_never_sent_again` |
| Indirect change through read-only paths | No GET handler queues a job; the Agent does not use Optimize | `requests::app::investigations` (no Google Ads request), review below |
| Tampering with the history | Append-only triggers | `the_optimization_history_is_append_only`, `requests::migration::optimization_history_is_append_only_and_goes_with_its_ad_account` |
| Applying while not live | Refused before any read or write | `applying_is_refused_while_the_installation_is_not_live` |

## Independent review, 21 October 2026

This historical review concerns commit `d628a60`. A separate reviewer reproduced findings with scratch tests outside the repository and found no Critical or High issue at that revision. The table records its tests and fixes, some since renamed or superseded. The hardening review below replaces its release judgment: raises are now disabled and unknown mutations are never resent.

| Finding | Severity | Test, before → after | Fix |
|---|---|---|---|
| An approved change that met a running change stayed Approved and never ran | Medium | `an_approved_change_runs_after_the_change_it_waited_for_is_settled`: `left: [] right: [2]` → ok | Hourly reconciliation queues approved Proposals that no execution claimed once nothing runs in their Ad Account (`execute::waiting`) |
| The envelope ignored raises made the same day | Medium | `raises_are_refused_without_a_current_account_wide_budget_total`: `left: Applied right: Refused(Envelope)` → ok | The envelope adds today's raises |
| A stop or switch-off after the claim did not stop the change | Medium | `a_stop_pressed_while_a_change_is_read_prevents_the_change`: `left: Applied right: Refused(Stopped)` → ok | `still_allowed` before the change and before a resend |
| Saving the policy re-activated a suspended Autopilot | Medium | `protecting_a_campaign_keeps_a_suspended_autopilot_suspended`: `left: Active right: Suspended` → ok | Saving keeps the standing; only the Autopilot control resumes |
| Any connection failure suspended the Autopilot as lost access | Low | not tested: the fixture cannot fail a token refresh without revoking it | Only a refused or disabled connection suspends; other failures leave the change approved for the next reconciliation |
| A protected Campaign not in the stored list was dropped silently | Low | in `protecting_a_campaign_keeps_a_suspended_autopilot_suspended`: 422 | The form refuses it |
| Two lost answers made a change Failed although Google might apply it later | Low | none | It stays Uncertain until a read settles it |
| The stop overwrote the decision time and left no line per Proposal | Low | none | It keeps the time and writes a line for each Proposal it fails |
| Settings could list kinds the client cannot send | Info | `applying_is_refused_while_the_installation_is_not_live` | `execute::SENDABLE` |

Historical accepted risks, reassessed below:

- **A manual change between KeySend's read and its write is overwritten.** Google Ads has no compare-and-set for a budget. The current implementation re-reads after validation to narrow the window; the read-back records the result, and the next evaluation sees the change as an Account change and starts the cooldown. The simulated Google Ads behaves the same way.
- **Superseded: execution did not recheck Access.** The current execution path checks Access before reservation and after validation.

The reviewer checked and found sound: Workspace scoping of every query, route authorization, the same-origin check, that no GET or Investigation path reaches a change, that no model output reaches a Proposal, escaping of names and terms, that GAQL and URLs are built only from integer IDs and stored customer IDs, budget validation against the live read, the Approval binding, replay and redelivery, the Autopilot limits, the policy form bounds, the live switch, error mapping and logs.

## Production hardening review

The review of `f48767a` through `d1287b0` found the following discrepancies before changes. The list is a source review, not evidence of a production exploit.

- The mutation serializer and sendable list also accepted `EnableCampaign`. Defaults excluded it, but configuration could expose a third mutation.
- Execution did not check paid monitoring Access again after Approval. The earlier accepted risk above is a release blocker for unattended execution.
- `still_allowed` checked mode and automatic authorization, but did not reapply changed protected Campaigns or limits for a manually approved Proposal after validation.
- An uncertain request could be sent again immediately when one read showed its old value. A stale read cannot prove the original request will never apply later.
- Read-back checked a budget amount without confirming the budget resource identity. Equal amounts on a replacement budget are insufficient verification.
- Missing AI model pricing allowed Investigation requests with only a token limit. This does not affect Optimize, which makes no model request.

The accepted code defects were covered by regression tests. Failing-first evidence exists for the execution races, retirement, observation, feedback, pricing and statistics fixes; the operator-recovery feature was implemented before its new request tests could run. Final results and residual risks are recorded in `docs/agent-production.md`.

The source review also identified stale envelope accounting across workers and midnight, omitted external changes in execution cooldowns, optimistic Outcome interpretation, and fractional conversion credit treated as an integer count. The earlier statement of no Critical or High issue applies only to that earlier review, not to the current release verdict. Applying stays disabled.

The hardened pipeline deliberately refuses all budget increases at execution while it lacks an authoritative current account-wide budget total. A cached sum plus locally recorded raises cannot establish an envelope after external changes or collection delays. It also keeps an old-value read after a sent request Uncertain rather than retrying. This can block an Ad Account until provider evidence resolves it. That is a visible operational limitation, not proof of failure or permission to resend.

Read-back establishes only that Google reports the intended value on the intended Campaign and budget. It cannot establish that KeySend alone caused that state. A manual write can still arrive between the final read and mutation, or after verification. There is no general compare-and-set. KeySend serializes its own account writes, re-reads after validation, rechecks access and policy, verifies resource identity, records conflicts and suspends Autopilot on conflict. It never sends a compensating write automatically.

The installation ceiling `optimize.max_mode` defaults to Shadow independently of `optimize.live: false`. `execute::live` requires at least Recommend, and automatic authorization and execution require Autopilot. A form cannot select a mode above the ceiling. Increasing a ceiling never converts Shadow history into executable Proposals. Both an operator's configuration and a User's current account authorization are required for writes.

Own-change provenance is conservative. An Outcome excludes at most one uniquely matching event with the exact target resource, operation, field, old/new values and an unambiguous timestamp inside the recorded execution interval in the Ad Account timezone. The execution must be verified and retain matching before/after state. Same-day changes are not discarded wholesale. Missing metadata, duplicate matches, ambiguous daylight-saving time and late timestamps remain confounders. An indistinguishable external event across every recorded dimension cannot be attributed conclusively; no provider request ID is present on the change event to prove exclusive provenance.

Explicit uncertain recovery records a provider-confirmed non-application as a User report, never as verified Google evidence. It cannot settle Applying, resend or compensate. It atomically changes the account to Investigate and Suspended, retires pending approvals and records who reported what. A wrong report can release the uncertainty lock after a later explicit settings save; this is a residual operator-evidence risk. There is no automatic time-based settlement.
