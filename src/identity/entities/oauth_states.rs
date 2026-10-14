use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_states")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub state_hash: String,
    pub browser_hash: String,
    pub pkce_verifier: String,
    pub workspace_id: Option<String>,
    pub session_hash: Option<String>,
    pub sign_up_source: Option<String>,
    pub expires_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
