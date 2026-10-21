# Research behind the Agent

The Agent answers a question about the Ad Accounts of a Workspace by calling a few read-only operations and then writing an answer that code checks against what those operations returned. This page records the published work that shaped that design, what it showed, where it stops applying, and how KeySend decides whether a technique stays. `docs/agent.md` describes the Agent itself.

Each technique is kept only while an evaluation in `src/agent/eval.rs` shows that it helps. A technique that does not change a measured result is removed.

## Google Ads compatibility and compliance research, 2026-10-21

This review opened current public Google documentation and read the local REST client, settings, execution, and tests. It made no Google Ads API calls, including reads or `validateOnly` requests. It did not open Cloud Console, change accounts, decrypt credentials, or submit forms. Findings about Google describe published documentation, not verified behavior of KeySend's production project.

### Developer token retirement and project access

The claim is confirmed by the body of Google's [developer token migration guide](https://developers.google.com/google-ads/api/docs/api-policy/developer-token). It dates retirement to September 9, 2026, says the header is optional and ignored, and locates access on the project that owns the OAuth credentials. Historical access transferred using the preceding 90 days of API activity. The guide also records an ongoing rollout issue affecting some post-retirement access upgrades and returning authorization errors. This is a documented issue, not an error observed here.

Some official text is inconsistent. The migration page's summary still requires a token, as do the [REST authorization guide](https://developers.google.com/google-ads/api/rest/auth?hl=en) and token-bearing examples in the [REST mutate guide](https://developers.google.com/google-ads/api/rest/common/mutate). The explicit migration instructions and current access-policy body support KeySend's omission of the header. They do not prove that its configured project can access production.

Google's [access levels and permissible use](https://developers.google.com/google-ads/api/docs/access-levels) gives these limits per project over a sliding 24-hour period:

| Access level | Accounts | Daily operations |
|---|---|---|
| Test | Test only | 15,000 |
| Explorer | Test and production | 2,880 production, 15,000 test |
| Basic | Test and production | 15,000 |
| Standard | Test and production | Unlimited daily operations, with other rate limits retained |

Explorer's restricted-service list does not include Campaign or CampaignBudget mutations. Standard alone has a separate permissible-use allocation. Reporting allocation permits reads, while Ad creation / management covers management services. A larger quota is not evidence of an approved change in use. Applications go through the project's [Google Ads API Overview](https://console.cloud.google.com/apis/api/googleads.googleapis.com/overview). New Basic and Standard applications need [brand verification](https://developers.google.com/google-ads/api/docs/api-policy/brand-verification). No project, access level, allocation, or verification status was inspected here.

The [quota guide](https://developers.google.com/google-ads/api/docs/best-practices/quotas) counts Search requests, individual mutate operations, and other service requests. Valid pagination requests are exempt, while invalid page tokens and `GoogleAdsFailure` responses count. One mutate request can contain at most 10,000 operations. Standard does not remove service rate limits.

Local finding: `config/production.yaml` sets `daily_limit: 2880`, while `GoogleSettings::default` is 15,000. `src/workspace/google_ads.rs` sums usage across Workspaces in this database, resets on the Los Angeles calendar date, and reserves daily-history capacity. This differs from Google's sliding window and cannot include another deployment or tool using the same project. Reads count each page locally, so this part is conservative. A normal Optimize attempt reserves at least five local operations: pre-read, validation, immediate re-read, mutation, and read-back. Reconciliation and retries can use more. These are source findings, not quota measurements.

### OAuth and the hosted tool's declared use

The [multi-user OAuth workflow](https://developers.google.com/google-ads/api/docs/oauth/multi-user-authentication?hl=en) specifies the `https://www.googleapis.com/auth/adwords` scope, offline access for background work, and OAuth app verification. `src/controllers/auth.rs` requests this scope only when connecting Google Ads, with offline access, consent, state, and PKCE. `src/google.rs` refreshes the stored Google Connection token using the configured OAuth client. An OAuth grant is separate from Cloud project access and from the User's permissions on an Ad Account. The [call structure guide](https://developers.google.com/google-ads/api/docs/concepts/call-structure) explains bearer authentication and manager access using `login-customer-id` without hyphens.

The [common errors guide](https://developers.google.com/google-ads/api/docs/get-started/common-errors?hl=en) documents seven-day refresh tokens for external OAuth apps in Testing, `invalid_grant`, `USER_PERMISSION_DENIED`, and `CUSTOMER_NOT_ENABLED`. It also documents `CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION` for Test-only projects in v25 and later, with `ACTION_NOT_PERMITTED` in v24 and earlier. None of these errors was received from Google during this review. Publishing an OAuth app, verifying its brand, verifying scopes, obtaining API access, and retaining Ad Account permissions are separate checks.

The current [Google Ads Developer Policies](https://support.google.com/adspolicy/answer/6169371) require use to match the access application and a Tool Change Form when reporting gains management functionality. Their Programmatic use section also restricts shared proxies and headless changes without review or direct per-entity authentication. It says agencies and end-advertisers need their own Cloud account and Ads access. Inference: per-User OAuth alone does not establish that KeySend's hosted Autopilot meets those rules. The proposed hosting and credential model needs an explicit Compliance answer. The same policy requires visible data-use disclosures, opt-out, secure data handling, and disclosure of reporting delays beyond 24 hours. This review is not a complete policy audit.

Google's [RMF policy](https://developers.google.com/google-ads/api/docs/api-policy/rmf) applies only to Standard projects. Full-service tools have creation, management, and reporting requirements. Limited specialized tools may qualify differently, but Compliance decides. Reporting requirements depend on the hierarchy levels and reports displayed, with required fields and download requirements. This review did not audit each KeySend report against that list.

The RMF page requires screenshots or mockups of material proposed changes at least two weeks before they take effect. It explicitly says a response is not required after that notice period. That does not settle a separate permissible-use request or headless-access review. KeySend's existing release gate in `docs/optimization.md` waits for Google's answer and review of unattended changes. This research does not relax that gate.

The public [Tool Change Form](https://support.google.com/adspolicy/contact/tool_change) still asks for a historical manager ID associated with a developer token and API Center contact confirmation. It also asks for the change, screenshots or design documentation, outside-company and Internet access, the tool URL, demo access or an explanation, an optional video, and a contact email. These legacy fields conflict with the new onboarding model. No identifier or checkbox should be invented to bypass them. The [Compliance contact form](https://support.google.com/adspolicy/contact/contact_ads_api) is the published route to clarify the correct project-based submission. Reading these forms made no submission.

### REST version, masks, errors, and read-back

The [version schedule](https://developers.google.com/google-ads/api/docs/sunset-dates) lists v26 as latest, released October 14, 2026. v25, released July 22, 2026, remains supported with a tentative August 2027 sunset. KeySend's settings and environment configurations use `/v25`. Supported does not mean tested against Google, and this review does not propose a version change.

`src/google_ads/mutate.rs` builds one operation under the Workspace's resolved Ad Account. It calls `campaignBudgets:mutate` for `amountMicros`, and `campaigns:mutate` for `status`. Budget amounts are decimal strings. The current serializer and default allowlist in `src/settings.rs` permit budget adjustment and Pause only. Enable, negative keywords, keyword pauses, and bidding targets are refused by this REST serializer.

The [REST mutate guide](https://developers.google.com/google-ads/api/rest/common/mutate) defines sparse updates with an `updateMask`. The [CampaignBudget operation](https://developers.google.com/google-ads/api/reference/rpc/v25/CampaignBudgetOperation) uses a protobuf FieldMask. Its [JSON encoding](https://protobuf.dev/reference/protobuf/google.protobuf/#fieldmask) converts paths to lower camel case. Thus REST `"amountMicros"` and `"status"` match the documented encoding. GAQL and protobuf source names such as `amount_micros` are a different representation.

Both [MutateCampaignBudgetsRequest](https://developers.google.com/google-ads/api/reference/rpc/v25/MutateCampaignBudgetsRequest) and [MutateCampaignsRequest](https://developers.google.com/google-ads/api/reference/rpc/v25/MutateCampaignsRequest) define `validate_only`, `partial_failure`, and `response_content_type`. Validation checks without execution and returns errors rather than results. With partial failure disabled, operations form a transaction within that request. This does not make the pre-read, validation, mutation, and read-back atomic together. `MUTABLE_RESOURCE` requests the changed resource in the response.

Baseline finding, corrected during this hardening: the client asked for `MUTABLE_RESOURCE` but only checked status and error codes. It now requires the requested resource in one mutation result. Non-JSON, malformed and partial-failure success bodies remain Unknown. Validation-only may return an empty object. The execution layer in `src/optimize/execute.rs` separately queries the live Campaign and marks the Proposal Applied only when the intended value matches. This reduces false success but is not proof of exclusive ownership of the change or protection from a concurrent external edit.

The mutation adapter extracts code values from `error.details[].errors[].errorCode` and `partialFailureError.details`. HTTP 429 or the two recognized resource-exhaustion codes become Quota. HTTP 401 or 403 and recognized access codes become Access. HTTP 5xx and transport failures after a connection may have opened become Unknown. Other errors preserve the first extracted code or an HTTP fallback. A connection failure becomes Unavailable. It logs service, validation flag, status, request ID, and codes. Google's [logging guide](https://developers.google.com/google-ads/api/docs/productionize/logging) recommends retaining the response `request-id` for support. The adapter logs failed request IDs but does not return them in its error type.

Baseline execution read before another mutation after Unknown; hardening removes automatic resend because one old-value read cannot prove the first request is finished. An original-value read remains Uncertain. A matching intended value and resource can verify the observed state. Neither proves exclusive ownership of the change. Real errors, result shapes, propagation timing, and timeout reconciliation still need authorized account verification. A validation success alone cannot prove mutation rights or read-back behavior.

Existing evidence is local and synthetic. `a_change_names_only_resources_of_its_ad_account_and_masks_only_its_field` in `src/google_ads/mutate.rs` checks literal request bodies. In `tests/requests/app/optimize.rs`, `a_budget_change_reaches_google_ads_as_one_validated_operation_with_an_exact_field_mask` checks the REST stub, and `a_change_without_an_answer_from_google_ads_is_never_sent_again` checks a stub 503 before any application. The simulator additionally models a lost answer after application. These tests were read, not rerun for this documentation change. The REST stub does not validate the request against Google's schema and does not return a mutable budget resource. Its fabricated request ID is not Google evidence.

### Credential availability and evidence boundary

Only environment variable names were inspected, with case-insensitive matching for `GOOGLE`, `GADS`, `ADWORDS`, and `OAUTH`. None were present in this process. The source credential schema names `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`. No values were printed or read from credential stores. No refresh token, master key, encrypted credential file, or production storage was opened. This does not prove credentials are absent from other stores or deployments.

No account has been explicitly authorized for real calls in this task. Before any later real verification, the owner must identify the permitted Cloud project, Ad Account, credential source, and allowed operations. A read-only grant does not authorize `validateOnly` or a mutation. A mutation grant must bound the resource and change. Any later evidence must distinguish test accounts from production and record sanitized request IDs, actual error codes, validation results, and read-back separately from the local stub results.

## Evidence to design

| Technique | Failure it addresses | What KeySend does | Added complexity | What keeps it |
|---|---|---|---|---|
| ReAct, bounded | The answer guesses where it could have looked | A loop of at most 3 rounds and 8 operations, then one answer | A loop and a step table | Essential conclusions found with the loop that a single pass without operations misses |
| Specialized tools (Toolformer) | The model computes or invents a number | Operations are business questions; Rust writes the queries, checks access, computes every Metric | One typed enum of operations | Zero answers with a number that is not in the evidence |
| Short, ranked context (Lost in the Middle) | A relevant number is buried in a long input | Each operation returns at most 20 rows, ranked by spend or by contribution, with an evidence ID | Row limits and ranking in each operation | Same essential conclusions with fewer input tokens than a full dump |
| Deterministic verification (Chain-of-Verification, Self-Refine) | Unsupported claims, wrong numbers, wrong names | Every claim cites evidence IDs; code checks each number and name against only the cited evidence, and causal words against the hedging rules | One validator that reuses `src/ai.rs` | Rejected claims in the evaluation that a reader would also reject; no second model call |
| Evidence strength from data (calibration, selective prediction) | Confident conclusions from noisy data | Code labels each piece of evidence Strong, Limited or Weak from conversions, days and Conversion tracking; the answer cannot state a stronger level | One function of counts | Low-volume and lag scenarios answered with the right level |
| Trust boundaries (OWASP) | A campaign name or a search term gives orders | Authorization is resolved from the session, never from the model; there is no write operation; untrusted text arrives only as quoted JSON values | None beyond the typed operations | The injection scenario and the cross-Workspace request tests |

Rejected: a vector database, a planner model, a second model that reviews the answer, full replay of earlier conversations, and a generic query operation. Each section says why.

## ReAct: reasoning and acting

Yao et al., ReAct: Synergizing Reasoning and Acting in Language Models, 2022. https://arxiv.org/abs/2210.03629

Problem. Chain-of-thought answers come only from the model's memory, and tool-only agents act without a plan.

Findings. Interleaving a reasoning step with an action and its observation beat chain-of-thought alone on HotpotQA fact verification (FEVER) by reducing hallucinated facts, and beat imitation and reinforcement learning baselines on the ALFWorld and WebShop decision tasks.

Limits. The tasks are open-domain and the tools are search and lookup. Many of the errors the paper reports are repeated actions and loops, which an unbounded loop does not prevent. The written reasoning trace is not needed when the model API calls tools natively.

For KeySend. The model sees the result of one operation before it chooses the next one: a comparison that shows a fall in conversion rate leads to campaigns and Conversion tracking, not to search terms. The loop is bounded by code (rounds, operations, seconds, tokens), an identical repeated call returns the stored result, and the hidden reasoning is never stored or shown. The interface shows the operations that ran.

Measured: the bounded loop and a dump of every operation reach the same essential credit (88.9% and 89.6% in Round 2, within trial noise); the loop reads 3 operations instead of 28 and scored 94.4% against 80.6% on the three new held-out cases. Kept for its smaller reads and payload; its quality advantage is not established.

## Toolformer: specialized tools

Schick et al., Toolformer: Language Models Can Teach Themselves to Use Tools, 2023. https://arxiv.org/abs/2302.04761

Problem. Language models are poor at arithmetic and at facts that change, which simple tools do well.

Findings. A model that learned to call a calculator, a search engine, a calendar and others improved on arithmetic and factual benchmarks over the same model without tools and over much larger models.

Limits. The model was fine-tuned to insert the calls. KeySend uses an API model that chooses typed operations through a strict JSON schema, one request per round, so the training method does not apply, only the principle. The schema gives the same typed choice as native function calling and works with the existing request function and evaluation proxy; the two were not compared.

For KeySend. The Metric is never computed by the model. Each operation returns numbers that code computed and formatted, and the answer may only repeat them. The operations are business questions (compare campaigns, investigate conversions), not database or GAQL access, so the model cannot ask for data outside the Ad Accounts of the investigation and cannot write a query.

Worth it: yes. Measured by the share of answers whose numbers all occur in the cited evidence.

## Lost in the Middle: context length and position

Liu et al., Lost in the Middle: How Language Models Use Long Contexts, 2023. https://arxiv.org/abs/2307.03172

Problem. Whether models use all of a long input.

Findings. On multi-document question answering and key-value retrieval, accuracy was highest when the relevant information was at the start or end of the input and fell when it was in the middle, also for models built for long contexts.

Limits. The tasks retrieve one fact. Newer models are better at long inputs. The result says nothing about analysis across many numbers.

For KeySend. Operations rank their rows (largest spend or largest contribution first), cut them at a row limit, and state the number of rows left out. A follow-up receives the earlier question, the earlier answer and the evidence it cited, not the earlier transcript. The evaluation compares this with one request that holds the whole account summary.

Measured: the Agent sends 77% fewer KeySend characters than the dump but, through the evaluation proxy, only 11% fewer input tokens, because each round repeats the instructions and schema. The short-context claim is not established for cost.

## Self-Refine, Reflexion and Chain-of-Verification

Madaan et al., Self-Refine: Iterative Refinement with Self-Feedback, 2023. https://arxiv.org/abs/2303.17651
Shinn et al., Reflexion: Language Agents with Verbal Reinforcement Learning, 2023. https://arxiv.org/abs/2303.11366
Dhuliawala et al., Chain-of-Verification Reduces Hallucination in Large Language Models, 2023. https://arxiv.org/abs/2309.11495

Problem. A first answer has errors that the same model can find when asked to check it.

Findings. Self-Refine improved outputs on several generation tasks by feedback and rewriting with the same model. Reflexion improved success on coding and decision tasks by retrying with remembered feedback from a test or an environment signal. Chain-of-Verification reduced hallucinated list items and facts by planning verification questions and answering them independently of the draft.

Limits. Every round costs a full model call. The gains depend on a reliable feedback signal; without an external signal, a model checking itself often keeps its errors. Reflexion's strongest results use unit tests as the signal.

For KeySend. The feedback signal already exists in code: `src/ai.rs` checks that every number, percentage, amount and name of a text occurs in the data, and that a causal word appears only with an allowed cause or a hedge. The Agent applies these checks per claim, against only the evidence that the claim cites, which is the verification step of Chain-of-Verification without a second call. A claim that fails is dropped, and the answer says how many were dropped. A second model pass is not used.

Measured: the check keeps every number of a kept text in its cited evidence and refuses changes on Weak evidence, but its net effect on essential credit is inconclusive (Round 3: 87.8% checked against 90.0% for the unchecked draft) and it still removes some useful texts, such as summaries that use because about uncertainty. It is kept as a guarantee of where numbers come from, not as a measured quality gain. A second model pass is not used. A repair request that sent the check's refusals back to the model, Reflexion with an external signal, was built and measured: its answer was kept in 6 of 32 cases, it restored no summary of the Agent, and it used 25.9% of the input tokens (`docs/agent-evaluation.md`, Round 2). It was removed. The losses it targeted came from evidence IDs written into the text, which code now removes.

## Retrieval-augmented generation

Lewis et al., Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks, 2020. https://arxiv.org/abs/2005.11401

Problem. Facts stored only in model weights are hard to update and to cite.

Findings. Conditioning generation on retrieved passages improved open-domain question answering and gave more specific and factual text than a model without retrieval.

Limits. The retrieval is dense search over unstructured text. KeySend's data is structured, keyed by Ad Account, Campaign and date.

For KeySend. Retrieval is the store: Account history, Deep check data, Findings, Assessments, Recommendation history, and the verified Business context. A vector database would add infrastructure and an unscoped similarity search where a keyed query by Workspace and Ad Account is exact.

Worth it: retrieval yes, vector search no.

## Calibration and selective prediction

Guo et al., On Calibration of Modern Neural Networks, 2017. https://arxiv.org/abs/1706.04599
Geifman and El-Yaniv, Selective Classification for Deep Neural Networks, 2017. https://arxiv.org/abs/1705.08500
Kadavath et al., Language Models (Mostly) Know What They Know, 2022. https://arxiv.org/abs/2207.05221
Xiong et al., Can LLMs Express Their Uncertainty? An Empirical Evaluation of Confidence Elicitation in LLMs, 2023. https://arxiv.org/abs/2306.13063

Problem. Whether a model's confidence matches its accuracy, and when a system should abstain.

Findings. Modern networks are often overconfident. A system that abstains on low-confidence inputs can trade coverage for accuracy. Large models can be fairly calibrated on their token probabilities for some formats, but confidence that a model states in words is often overconfident.

Limits. The studies measure classification and question answering, not analysis of an account.

For KeySend. A stated confidence is not used. Code gives each piece of evidence a strength from things that can be counted: conversions in each period, days of data, Conversion tracking, and whether the latest days are still within the conversion delay. The answer shows that strength, and a recommendation on Weak evidence becomes an investigation, not an action. When the evidence is insufficient the answer says so; that is the abstention.

Worth it: yes. Measured by the low-volume, lag and tracking scenarios.

## Prompt injection and agent security

OWASP, LLM Prompt Injection Prevention Cheat Sheet. https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html
OWASP Top 10 for LLM Applications 2025, LLM01 Prompt Injection and LLM06 Excessive Agency. https://genai.owasp.org/llm-top-10/

Guidance. Separate instructions from data, but do not rely on the separation; validate every tool call against the permissions of the user and the session in code; give tools least privilege; validate output; log security-relevant decisions; limit repeated attempts.

For KeySend. Campaign names, search terms, landing pages, ad text and research notes are data. They arrive in the model input only as JSON string values inside operation results. The defense does not depend on the model obeying: the Agent has no write operation, the Google Ads client sends only `googleAds:search`, each operation reads only the Ad Accounts that the session's Workspace owns, the model refers to an Ad Account by a short handle that code resolves within the investigation, and every operation's arguments are parsed into Rust types with limits. A follow-up can only continue an investigation of the same Workspace. A daily limit per Workspace bounds repeated attempts and cost.

Worth it: required.

# Research behind Optimize

Optimize (`docs/optimization.md`) changes Ad Accounts, so its methods were chosen for being checkable at KeySend's scale: a few Proposals per Ad Account per month, 0 to 100 conversions per Campaign per month, and conversion delays of days. Each entry gives the source and what it found, why it applies, the hypothesis, what KeySend did and the decision. Sources were opened on 2026-10-21; abstracts were read where the full text was not available, as noted.

| Technique | Source | Decision | Where |
|---|---|---|---|
| Exact count test for "beyond chance" | Gu et al. 2008 | Kept as a rule | `optimize::stats`, every Signal |
| Settled days before judging conversions | Chapelle 2014 | Kept as a rule, simplified | `signals::settled` |
| Entry and clearing thresholds, cooldowns | Google Ads Help on Smart Bidding learning; pacing papers | Kept as rules | `signals::ENTER`, `CLEAR`, the policy's cooldown and observation |
| Descriptive before/after with a comparison group | Brodersen et al. 2015; Lewis and Rao 2015 | Kept as a description; causal claims rejected | `optimize::outcome` |
| Code-only plans, model never in the control path | Greshake et al. 2023; OWASP LLM06; Debenedetti et al. 2025 | Kept: stricter, no model at all | the whole module |
| Human-AI interaction guidelines | Amershi et al. 2019 | Kept as a checklist for the Proposal page | `assets/views/optimize/` |
| CUSUM or Bayesian change-point detection | Lucas 1985; Adams and MacKay 2007 | Deferred | |
| Contextual bandits, safe RL, off-policy evaluation | Li et al. 2010; Thomas et al. 2015; García and Fernández 2015 | Rejected for now | |
| Model cascades | Chen et al. 2023 (FrugalGPT) | Deferred; no model in Optimize | |
| LLM judges for decisions | Zheng et al. 2023 | Rejected for decisions | `optimize::eval` uses expected decisions |

## Exact test for two counts

Gu, Ng, Tang and Schucany, Testing the Ratio of Two Poisson Rates, Biometrical Journal 2008. https://doi.org/10.1002/bimj.200710403

Finding. The paper compares asymptotic, approximate, exact conditional and likelihood ratio tests of a ratio of Poisson rates and gives sample sizes. At small counts the exact tests are the safe choice.

For KeySend. A Campaign that spent S at a target T would record about S/T conversions. `stats::poisson_at_most` gives the chance of recording x or fewer; a Signal needs it below 5%, and the cost Signals also need 10 conversions. With 5 conversions against an expected 8, the chance is 19% and nothing is proposed, however high the cost per conversion looks. Spend without conversions reduces to `e^(-S/T) < 0.05`, which is spend above three target costs. Outcomes instead use an exact conditional comparison of two integer counts with their measured exposure, accounting for uncertainty in both periods. Fractional attribution credits are not rounded into exact counts.

Limit. A nominal 5% tail probability assumes independent Poisson integer counts, stable exposure and a fixed commercial target. Repeated overlapping evaluations and multiple Campaigns do not provide a calibrated 5% action error rate. Conversion lag, changing attribution, tracking failures and commercial targets require separate safeguards. Statistical significance alone does not establish useful profit or lead quality. No real commercial error rate or uplift has been measured.

Decision. Kept. Tests with literal values: `optimize::stats::tests`.

## Conversion delay

Chapelle, Modeling Delayed Feedback in Display Advertising, KDD 2014 (abstract). https://doi.org/10.1145/2623330.2623634

Finding. Conversions arrive up to a month after the click; a model of the delay decides whether a click without a conversion is negative or "too early to tell".

For KeySend. The model needs click-level logs, which KeySend does not have. The idea applies directly: recent days are incomplete, not bad. Signals leave out the last day, or the last week when a quarter or more of the Ad Account's conversions arrive two days or more after the click (`conversion_lag_totals`), and each Proposal says how many days it left out. An evaluation case with 70% late conversions expects no cut.

Decision. Kept as a rule; the delay model was rejected.

## Hysteresis, cooldowns and learning

Agarwal et al., Budget Pacing for Targeted Online Advertisements at LinkedIn, KDD 2014 (abstract). https://doi.org/10.1145/2623330.2623366 · Xu et al., Smart Pacing for Effective Online Ad Campaign Optimization, 2015. https://arxiv.org/abs/1506.05851 · Google Ads Help, bid strategy status. https://support.google.com/google-ads/answer/13020501

Finding. Pacing inside the auction is hard because supply and prices move quickly. Google's own Smart Bidding re-converges over one to two conversion cycles, and advises against several target changes within one cycle.

For KeySend. KeySend does not pace; Google does. What carries over is the risk that a daily rule chases its own effect. A cost Signal enters at 1.3 times the target and an open Proposal clears only below 1.1; a Campaign that anyone changed within 7 days gets no Proposal; a change by KeySend blocks the opposite change for 14 days. The evaluation has cases for each.

Decision. Kept as rules. Learned pacing rejected.

## Measuring what followed

Brodersen et al., Inferring Causal Impact Using Bayesian Structural Time-Series Models, 2015. https://arxiv.org/abs/1506.00356 · Lewis and Rao, The Unfavorable Economics of Measuring the Returns to Advertising, QJE 2015. https://doi.org/10.1093/qje/qjv023

Finding. Lewis and Rao found median confidence intervals on return over 100 percentage points wide across 25 large experiments, and called selection bias "a crippling concern" for observational methods. CausalImpact needs control series that the change did not touch.

For KeySend. One Campaign with tens of conversions cannot support a causal claim, and other Campaigns of the same Ad Account share auctions and budgets, so they are weak controls. `outcome::evaluate` compares the 14 settled days after with the 14 before, beyond chance, and calls it no clear change when the rest of the Ad Account moved the same way. The page says what followed, not what caused it.

Decision. Kept as a description. CausalImpact and synthetic control deferred; causal claims rejected.

## Keeping the model out of the control path

Greshake et al., Not What You've Signed Up For: Compromising Real-World LLM-Integrated Applications with Indirect Prompt Injection, 2023. https://arxiv.org/abs/2302.12173 · OWASP, LLM06:2025 Excessive Agency. https://genai.owasp.org/llmrisk/llm062025-excessive-agency/ · Debenedetti et al., Defeating Prompt Injections by Design (CaMeL), 2025. https://arxiv.org/abs/2503.18813

Finding. Retrieved text can act as instructions and steer which APIs are called. OWASP's mitigations include authorization in downstream systems rather than in the model, and human approval of high-impact actions. CaMeL derives control flow only from trusted input, so untrusted data cannot change it.

For KeySend. Search terms and Campaign names are written by searchers and advertisers. Optimize has no model at all: Signals, Proposals and policy are code, a Campaign name is only displayed (escaped) and a search term becomes the text of a negative keyword, never an instruction or a resource name. Evaluation cases with injected names and terms expect the same decisions as without them.

Decision. Kept, in its strictest form. A CaMeL interpreter is unnecessary without model-written plans.

## Approval page

Amershi et al., Guidelines for Human-AI Interaction, CHI 2019. https://doi.org/10.1145/3290605.3300233

Finding. 18 guidelines, validated with 49 design practitioners against 20 products.

For KeySend. The Proposal page applies "make clear how well the system can do" (the count, the chance, the days left out), "make clear why" (the Signal and the target), "convey the consequences" (now and proposed value, the direction, the budget that is not a ceiling), "support efficient dismissal" (Reject, which stops the same Proposal for 30 days), "provide global controls" (the policy, Stop all changes), "notify users about changes" (the history), and "scope services when in doubt" (holds with their reasons).

Hypothesis. Users who see the consequence line and the evidence count approve fewer Proposals that they later reverse. Not measured.

Decision. Kept as a checklist.

## Rejected or deferred

- **Change-point detection** (Lucas, Counted Data CUSUMs, Technometrics 1985, https://doi.org/10.1080/00401706.1985.10488030; Adams and MacKay 2007, https://arxiv.org/abs/0710.3742). A Poisson CUSUM fits counts and is cheap, but the existing Findings and Alerts already detect tracking stops; Optimize uses a fixed 30-day window with an exact test, which a User can check. Deferred until it can be compared with the current detection on replayed history.
- **Bandits, safe RL and off-policy evaluation** (Li et al. 2010, https://arxiv.org/abs/1003.0146; Thomas et al., AAAI 2015; García and Fernández, JMLR 2015). They needed millions of logged events with random exploration; KeySend has a few non-random Proposals per Ad Account per month, and high-confidence bounds at that size would always refuse. Rejected for now. The history records every Proposal, the decision and the Outcome, so the data such a method needs accumulates.
- **A model that chooses among Proposals or words them.** With four kinds and one change of spend per Campaign, the choice is a rule; wording from the language tables is exact in seven languages. Rejected: it adds cost, a failure mode and an injection path without a decision to make.
- **Model cascades** (Chen et al., FrugalGPT, 2023, https://arxiv.org/abs/2305.05176). Deferred: Optimize makes no model call, and Investigations are small (`docs/agent-production.md`).
- **LLM judges for decisions** (Zheng et al. 2023, https://arxiv.org/abs/2306.05685: about 80% agreement with humans, with position and verbosity bias). A decision is checked against an expected decision, not judged.

Not opened: the full texts of Page 1954, Vaver and Koehler's geo experiments (the page refused access), and Thomas et al.'s High Confidence Policy Improvement. They are not cited as findings.
