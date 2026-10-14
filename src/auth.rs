use axum::{
    extract::{FromRequestParts, RawPathParams},
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use loco_rs::app::AppContext;

use crate::{
    identity::{self, SignedIn},
    workspace::{AdAccount, WorkspaceStore},
};

pub const SESSION_COOKIE: &str = "ks_session";
pub const OAUTH_COOKIE: &str = "ks_oauth";

pub struct Authenticated(pub SignedIn);

impl FromRequestParts<AppContext> for Authenticated {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, ctx)
            .await
            .map_err(IntoResponse::into_response)?;
        if let Some(cookie) = jar.get(SESSION_COOKIE) {
            if let Some(signed_in) = identity::authenticate(&ctx.db, cookie.value())
                .await
                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response())?
            {
                crate::language::set_current(signed_in.user.language)
                    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
                return Ok(Self(signed_in));
            }
        }
        Err(Redirect::to("/sign-in").into_response())
    }
}

pub struct OwnedAdAccount {
    pub signed: SignedIn,
    pub ad: AdAccount,
}

impl FromRequestParts<AppContext> for OwnedAdAccount {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let Authenticated(signed) = Authenticated::from_request_parts(parts, ctx).await?;
        let params = RawPathParams::from_request_parts(parts, ctx)
            .await
            .map_err(IntoResponse::into_response)?;
        let Some(id) = params
            .iter()
            .find_map(|(name, value)| (name == "id").then_some(value))
        else {
            return Err(crate::app::not_found());
        };
        let ad = WorkspaceStore::new(&ctx.db, &signed.workspace)
            .ad_accounts()
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response())?
            .into_iter()
            .find(|ad| ad.customer_id == id);
        match ad {
            Some(ad) => Ok(Self { signed, ad }),
            None => Err(crate::app::not_found()),
        }
    }
}
