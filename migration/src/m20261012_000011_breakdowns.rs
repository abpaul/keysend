use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const KEY: &str =
    "workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL";
const COUNTS: &str = "impressions INTEGER NOT NULL,clicks INTEGER NOT NULL,cost_micros INTEGER NOT NULL,conversions REAL NOT NULL,conversions_value REAL NOT NULL";
const ACCOUNT: &str = "FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE";
const TABLES: [&str; 9] = [
    "breakdown_history",
    "campaign_search_share_days",
    "conversion_action_days",
    "campaign_network_days",
    "hour_totals",
    "location_totals",
    "keyword_quality",
    "demographic_totals",
    "conversion_lag_totals",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            format!("CREATE TABLE breakdown_history (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,kind TEXT NOT NULL,first_day INTEGER,last_day INTEGER,error TEXT,PRIMARY KEY(workspace_id,ad_account_customer_id,kind),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE campaign_search_share_days ({KEY},campaign_id INTEGER NOT NULL,impressions INTEGER NOT NULL,shown REAL,shown_under INTEGER NOT NULL CHECK(shown_under IN (0,1)),lost_budget REAL,lost_budget_over INTEGER NOT NULL CHECK(lost_budget_over IN (0,1)),lost_rank REAL,lost_rank_over INTEGER NOT NULL CHECK(lost_rank_over IN (0,1)),CHECK(shown IS NULL OR shown_under=0),CHECK(lost_budget IS NULL OR lost_budget_over=0),CHECK(lost_rank IS NULL OR lost_rank_over=0),PRIMARY KEY(workspace_id,ad_account_customer_id,day,campaign_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE conversion_action_days ({KEY},campaign_id INTEGER NOT NULL,action TEXT NOT NULL,conversions REAL NOT NULL,conversions_value REAL NOT NULL,PRIMARY KEY(workspace_id,ad_account_customer_id,day,campaign_id,action),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE campaign_network_days ({KEY},campaign_id INTEGER NOT NULL,network TEXT NOT NULL,{COUNTS},PRIMARY KEY(workspace_id,ad_account_customer_id,day,campaign_id,network),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE hour_totals ({KEY},weekday INTEGER NOT NULL CHECK(weekday BETWEEN 0 AND 6),hour INTEGER NOT NULL CHECK(hour BETWEEN 0 AND 23),{COUNTS},PRIMARY KEY(workspace_id,ad_account_customer_id,day,weekday,hour),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE location_totals ({KEY},location_id INTEGER NOT NULL,name TEXT NOT NULL,{COUNTS},PRIMARY KEY(workspace_id,ad_account_customer_id,day,location_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE keyword_quality ({KEY},campaign_id INTEGER NOT NULL,ad_group_id INTEGER NOT NULL,criterion_id INTEGER NOT NULL,keyword TEXT NOT NULL,match_type TEXT NOT NULL,cost_micros INTEGER NOT NULL,quality_score INTEGER CHECK(quality_score BETWEEN 1 AND 10),ad_relevance TEXT,landing_page TEXT,expected_ctr TEXT,PRIMARY KEY(workspace_id,ad_account_customer_id,day,campaign_id,ad_group_id,criterion_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE demographic_totals ({KEY},kind TEXT NOT NULL CHECK(kind IN ('age','gender')),value TEXT NOT NULL,{COUNTS},PRIMARY KEY(workspace_id,ad_account_customer_id,day,kind,value),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE conversion_lag_totals ({KEY},bucket TEXT NOT NULL,conversions REAL NOT NULL,PRIMARY KEY(workspace_id,ad_account_customer_id,day,bucket),{ACCOUNT}) WITHOUT ROWID"),
        ] {
            manager.get_connection().execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in TABLES.iter().rev() {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
