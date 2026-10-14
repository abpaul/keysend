use std::collections::HashMap;

use loco_rs::Result;
use sea_orm::{ConnectionTrait, TransactionTrait};

use super::{sql, WorkspaceStore};
use crate::language::Language;

impl WorkspaceStore<'_> {
    pub async fn translations(
        &self,
        language: Language,
        hashes: &[String],
    ) -> Result<HashMap<String, String>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        self.db
            .query_all_raw(sql(
                "SELECT source_hash,text FROM translations WHERE workspace_id=?1 AND language=?2 AND source_hash IN (SELECT value FROM json_each(?3))",
                vec![
                    self.workspace.id().into(),
                    language.code().into(),
                    serde_json::to_string(hashes)?.into(),
                ],
            ))
            .await?
            .into_iter()
            .map(|row| Ok((row.try_get("", "source_hash")?, row.try_get("", "text")?)))
            .collect()
    }

    pub async fn save_translations(
        &self,
        id: &str,
        language: Language,
        translations: &[(String, String)],
        now: i64,
    ) -> Result<()> {
        let txn = self.db.begin().await?;
        for (hash, text) in translations {
            let mut values = self.monitoring_account(id);
            values.extend([
                language.code().into(),
                hash.clone().into(),
                text.clone().into(),
                now.into(),
            ]);
            txn.execute_raw(sql("INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) SELECT workspace_id,customer_id,?3,?4,?5,?6 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT DO NOTHING",values)).await?;
        }
        txn.commit().await?;
        Ok(())
    }
}
