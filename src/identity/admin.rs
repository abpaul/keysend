use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use loco_rs::Result;
use rand::{rngs::OsRng, Rng};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::Sha256;

use super::{hash_token, random_token};
use crate::{
    database::begin_write,
    settings::{wall_clock, Settings},
};

pub const CHALLENGE_SECONDS: i64 = 600;
pub const SESSION_SECONDS: i64 = 14400;

pub fn allowed(settings: &Settings, email: &str) -> bool {
    settings.master_key.is_some() && settings.admin_emails.iter().any(|entry| entry == email)
}
fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, sql, values)
}
fn mac(settings: &Settings, hash: &str, email: &str, code: &str) -> Hmac<Sha256> {
    let key = settings
        .master_key
        .as_ref()
        .expect("admin requires a master key")
        .derive(b"keysend:admin:code:v1");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).expect("HMAC accepts a 32-byte key");
    mac.update(hash.as_bytes());
    mac.update(&[0]);
    mac.update(email.as_bytes());
    mac.update(&[0]);
    mac.update(code.as_bytes());
    mac
}
const HOUR: i64 = 3600;
const DAY: i64 = 86400;
const FAILED_CODES_PER_ADDRESS_PER_DAY: i64 = 10;

async fn throttle(
    db: &impl ConnectionTrait,
    key: String,
    now: i64,
    window: i64,
    limit: i64,
) -> Result<bool> {
    let row = db.query_one_raw(statement(
        "INSERT INTO admin_throttle(key,window_start,count) VALUES (?,?,1) ON CONFLICT(key) DO UPDATE SET window_start=CASE WHEN window_start<=? THEN excluded.window_start ELSE window_start END,count=CASE WHEN window_start<=? THEN 1 ELSE count+1 END RETURNING count",
        vec![key.into(), now.into(), (now-window).into(), (now-window).into()],
    )).await?.expect("throttle upsert returns a row");
    Ok(row.try_get::<i64>("", "count")? <= limit)
}
async fn exhausted(
    db: &impl ConnectionTrait,
    key: &str,
    now: i64,
    window: i64,
    limit: i64,
) -> Result<bool> {
    Ok(db
        .query_one_raw(statement(
            "SELECT 1 FROM admin_throttle WHERE key=? AND window_start>? AND count>=?",
            vec![key.into(), (now - window).into(), limit.into()],
        ))
        .await?
        .is_some())
}

pub async fn challenge(
    db: &DatabaseConnection,
    settings: &Settings,
    email: &str,
    ip: &str,
    token: &str,
) -> Result<Option<String>> {
    let now = wall_clock().timestamp();
    let tx = begin_write(db).await?;
    let ip_ok = throttle(&tx, format!("ip:{}", hash_token(ip)), now, HOUR, 10).await?;
    let email_ok = !allowed(settings, email)
        || throttle(&tx, format!("email:{}", hash_token(email)), now, HOUR, 5).await?;
    let result = if email_ok && ip_ok && allowed(settings, email) {
        let code = format!("{:06}", OsRng.gen_range(0..1_000_000u32));
        let hash = hash_token(token);
        let tag =
            STANDARD_NO_PAD.encode(mac(settings, &hash, email, &code).finalize().into_bytes());
        tx.execute_raw(statement(
            "DELETE FROM admin_challenges WHERE email=?",
            vec![email.into()],
        ))
        .await?;
        tx.execute_raw(statement(
            "INSERT INTO admin_challenges(token_hash,email,code_mac,expires_at) VALUES (?,?,?,?)",
            vec![
                hash.into(),
                email.into(),
                tag.into(),
                (now + CHALLENGE_SECONDS).into(),
            ],
        ))
        .await?;
        Some(code)
    } else {
        if !email_ok || !ip_ok {
            tracing::info!(event = "admin_throttled");
        }
        None
    };
    tx.commit().await?;
    Ok(result)
}

pub enum Verification {
    SignedIn(String),
    Refused,
}

pub async fn verify(
    db: &DatabaseConnection,
    settings: &Settings,
    token: &str,
    code: &str,
    ip: &str,
) -> Result<Verification> {
    let now = wall_clock().timestamp();
    let hash = hash_token(token);
    let tx = begin_write(db).await?;
    // Successful verification does not consume the failed-verification budget.
    let bucket = format!("verify-ip:{}", hash_token(ip));
    if exhausted(&tx, &bucket, now, HOUR, 20).await? {
        tracing::info!(event = "admin_throttled");
        tx.commit().await?;
        return Ok(Verification::Refused);
    }
    let row = tx.query_one_raw(statement("UPDATE admin_challenges SET attempts=attempts+1 WHERE token_hash=? AND expires_at>? AND attempts<5 RETURNING email,code_mac",vec![hash.clone().into(),now.into()])).await?;
    let mut result = Verification::Refused;
    if let Some(row) = row {
        let email: String = row.try_get("", "email")?;
        let tag: String = row.try_get("", "code_mac")?;
        let failures = format!("verify-email:{}", hash_token(&email));
        if exhausted(&tx, &failures, now, DAY, FAILED_CODES_PER_ADDRESS_PER_DAY).await? {
            tracing::warn!(event = "admin_code_guessing", email);
        } else if allowed(settings, &email)
            && STANDARD_NO_PAD.decode(tag).is_ok_and(|tag| {
                mac(settings, &hash, &email, code)
                    .verify_slice(&tag)
                    .is_ok()
            })
        {
            let deleted = tx
                .execute_raw(statement(
                    "DELETE FROM admin_challenges WHERE token_hash=?",
                    vec![hash.into()],
                ))
                .await?
                .rows_affected();
            if deleted == 1 {
                let session = random_token();
                tx.execute_raw(statement("INSERT INTO admin_sessions(token_hash,email,created_at,expires_at) VALUES (?,?,?,?)",vec![hash_token(&session).into(),email.clone().into(),now.into(),(now+SESSION_SECONDS).into()])).await?;
                tracing::info!(event = "admin_signed_in", email);
                result = Verification::SignedIn(session);
            }
        } else if !throttle(&tx, failures, now, DAY, FAILED_CODES_PER_ADDRESS_PER_DAY).await? {
            tracing::warn!(event = "admin_code_guessing", email);
        }
    }
    if !matches!(result, Verification::SignedIn(_)) {
        throttle(&tx, bucket, now, HOUR, 20).await?;
        tracing::info!(event = "admin_code_rejected");
    }
    tx.commit().await?;
    Ok(result)
}
pub async fn authenticate(
    db: &DatabaseConnection,
    settings: &Settings,
    token: &str,
) -> Result<Option<String>> {
    let row = db
        .query_one_raw(statement(
            "SELECT email FROM admin_sessions WHERE token_hash=? AND expires_at>?",
            vec![hash_token(token).into(), wall_clock().timestamp().into()],
        ))
        .await?;
    let email = row
        .map(|row| row.try_get::<String>("", "email"))
        .transpose()?;
    Ok(email.filter(|email| allowed(settings, email)))
}
pub async fn sign_out(db: &DatabaseConnection, token: &str) -> Result<()> {
    db.execute_raw(statement(
        "DELETE FROM admin_sessions WHERE token_hash=?",
        vec![hash_token(token).into()],
    ))
    .await?;
    tracing::info!(event = "admin_signed_out");
    Ok(())
}
pub async fn retain(db: &DatabaseConnection) -> Result<()> {
    let now = wall_clock().timestamp();
    for (sql, cutoff) in [
        ("DELETE FROM admin_challenges WHERE expires_at<=?", now),
        ("DELETE FROM admin_sessions WHERE expires_at<=?", now),
        ("DELETE FROM admin_throttle WHERE window_start<?", now - DAY),
    ] {
        db.execute_raw(statement(sql, vec![cutoff.into()])).await?;
    }
    Ok(())
}
