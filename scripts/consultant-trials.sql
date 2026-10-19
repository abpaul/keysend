-- Read-only report of the consultant offer experiment, one row per Trial.
-- sqlite3 -readonly -header -column keysend.sqlite < scripts/consultant-trials.sql
-- A qualified Trial connected at least 2 Ad Accounts.
WITH accounts AS (
  SELECT workspace_id, COUNT(*) AS ad_accounts FROM ad_accounts GROUP BY workspace_id
),
written AS (
  SELECT workspace_id, MIN(day) AS first_update_day, COUNT(DISTINCT day) AS update_days
  FROM updates GROUP BY workspace_id
),
weekly AS (
  SELECT workspace_id, COUNT(*) AS weekly_emails_sent FROM weekly_emails
  WHERE sent_at IS NOT NULL GROUP BY workspace_id
)
SELECT
  w.id AS workspace,
  COALESCE(w.sign_up_source, '') AS source,
  date(t.started_at, 'unixepoch') AS trial_started,
  COALESCE(a.ad_accounts, 0) AS ad_accounts,
  COALESCE(a.ad_accounts, 0) >= 2 AS qualified,
  date(u.first_update_day * 86400, 'unixepoch') AS first_update,
  COALESCE(u.update_days, 0) AS update_days,
  COALESCE(e.weekly_emails_sent, 0) AS weekly_emails_sent,
  COALESCE(s.status, '') AS subscription,
  date(s.created_at, 'unixepoch') AS subscribed
FROM workspaces w
JOIN workspace_trials t ON t.workspace_id = w.id AND t.started = 1
LEFT JOIN accounts a ON a.workspace_id = w.id
LEFT JOIN written u ON u.workspace_id = w.id
LEFT JOIN weekly e ON e.workspace_id = w.id
LEFT JOIN subscriptions s ON s.workspace_id = w.id
ORDER BY t.started_at;
