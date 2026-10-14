# KeySend

KeySend watches your Google Ads every day, tells you in plain English how things stand, and brings what matters to your attention. It never changes the Ad Account, and every Recommendation shows its evidence.

## Language

**User**:
The person who signs in to KeySend. A user can be a business owner, a consultant, or an agency employee.
_Avoid_: Consultant, customer, member

**Workspace**:
The owner of Google Connections, Ad Accounts, and the Subscription. A Workspace has exactly one User.
_Avoid_: Organization, account, team, tenant

**Google Connection**:
One Google login that a User has authorized for a Workspace, which gives KeySend access to one or more Ad Accounts.
_Avoid_: Google account, integration, login

**Ad Account**:
One Google Ads customer ID. It has campaigns, an Account history, and Updates, and it holds the Branding.
_Avoid_: Account, client, advertiser, customer

**Campaign**:
One Google Ads campaign of an Ad Account. KeySend shows each campaign on its own page and never changes it.
_Avoid_: Ad set, ad group, project

**Branding**:
The name, logo, and accent colour of an Ad Account that appear in its Exports.
_Avoid_: Client, theme, template

**Window**:
The days that a page shows and the equal number of days before them that it compares with: the last 7 days or the last 30 days.
_Avoid_: Reporting Period, Comparison Period, date range, timeframe

**Metric**:
A number that KeySend calculates from Google Ads data. A Metric is never written or changed by AI.
_Avoid_: KPI, stat

**Update**:
A dated note about one Ad Account or one campaign, written by AI from the changes that KeySend detected. The user reads Updates as a list, newest first, and cannot edit them.
_Avoid_: Insight, notification, daily report, digest, Commentary, verdict

**Alert**:
An email that KeySend sends the morning after something breaks in an Ad Account: a campaign stops or spend doubles. For conversions that go to zero, the email comes two days later.
_Avoid_: Notification, warning, Update

**Finding**:
A problem or an opportunity that KeySend detects in the Ad Account with a rule, with the spend or conversions involved. AI never creates a Finding.
_Avoid_: Insight, issue, alert, audit item

**Contribution**:
The part of a change in a Metric that comes from one campaign, device, keyword, or location. It is a fact about the change and not a reason for it: an Update shows it with the label Contribution and a text never connects it to a result with a causal word.
_Avoid_: Finding, driver, cause

**Cause**:
The reason that an Update gives for a movement of a Metric, with a level (Confirmed or Likely) and the evidence that code wrote from the Account history. Code finds every Cause; AI only chooses one for a text and words it. A Confirmed Cause is a recorded fact that the numbers support (for a budget change, the spend reached the budget); a Likely Cause is a probable reason and the sentence that gives it says so. A sentence that gives a Cause also names it with a word that code lists for it, such as the date of the Account change.
_Avoid_: Reason, driver, root cause, insight, explanation

**Recommendation**:
An action that AI proposes for the Ad Account, with a Finding as evidence. The user cannot edit it; the user can mark it as not useful.
_Avoid_: Next step, tip, suggestion, insight

**Account history**:
The daily numbers of each campaign and the changes made in the Ad Account, which KeySend collects when the Ad Account is added and then each day.
_Avoid_: Cache, archive, raw data

**Account change**:
One change in the Ad Account that Google reports, such as a new budget, with its old and new value. It is part of the Account history. KeySend follows up on each change that it can read and that belongs to one campaign. Any other change of that campaign, or of no campaign, in the same 28 days makes the result impossible to separate.
_Avoid_: Event, edit, work done

**Follow-up**:
KeySend's check of how an Account change turned out: the 14 complete days after the change against the 14 days before it, for that campaign. A rule in code gives the verdict (it worked, it has not worked yet, no clear effect, or cannot be judged). When the rest of the Ad Account moved the same way in the same two periods, the verdict is no clear effect. KeySend computes it when it is needed and does not store it; the Update of the day on which it is due and the campaign page show it. It says what came after the change and never says that the change caused it.
_Avoid_: Impact analysis, result, attribution, cause

**Conversion tracking**:
The state of an Ad Account that tells KeySend if it can judge results: working, too few results, or not set up. The user can tell KeySend to judge the Ad Account by conversions or not.
_Avoid_: Traffic Report, Performance Report, report type, mode

**Onboarding**:
The guided first session of a new user: sign in with Google, connect Google Ads, choose Ad Accounts, and see the first numbers.
_Avoid_: Setup wizard, tutorial, tour

**Deep check**:
The weekly collection of search terms, keywords, landing pages, and ads, from which KeySend detects the Findings that daily numbers cannot show.
_Avoid_: Audit, scan, sync

**Ad Account profile**:
What the user tells KeySend about an Ad Account for a long time: its goal, its target cost per conversion, and a note. All parts are optional.
_Avoid_: Settings, preferences, brief

**Campaign activity**:
The months in which a campaign had impressions or spend.
_Avoid_: History, timeline, heatmap

**Weekly email**:
The email that KeySend sends once a week to the user, with the performance and the Updates of each Ad Account of the Workspace.
_Avoid_: Newsletter, digest, report

**Export**:
A PDF file of one Ad Account for the Window on the page. The user downloads it; KeySend never sends it to anyone.
_Avoid_: Report, delivery, download

**Trial**:
The 14 days in which a new Workspace has everything without a Subscription and without a payment card.
_Avoid_: Free plan, free tier, demo

**Subscription**:
The monthly payment that keeps KeySend open for a Workspace after the Trial, for up to 3 Ad Accounts.
_Avoid_: Plan, tier, membership, Unlock

**Access**:
What a Workspace has during its Trial and while its Subscription is active: the pages, the Updates, the emails, and the Export of its Ad Accounts.
_Avoid_: Unlock, entitlement, license, permission
