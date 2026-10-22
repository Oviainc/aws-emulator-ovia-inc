// SPDX-License-Identifier: CC0-1.0

//! @op IntrospectOAuth2TokenWithIAM
//! Reports the claims of a token this service issued (RFC 7662 token introspection).
//!
//! ## What a call does
//!
//! The token value is opaque and carries nothing, so every claim comes from the record
//! it keys in [`crate::state::ComAmazonawsSigninSigninState::oauth_tokens`] — the
//! record the operation that minted it wrote. A token with a record that is still
//! inside its lifetime, not revoked and owned by the caller's account is reported
//! `active` with its claims; anything else (a token no record names, one past its
//! `exp`, a revoked one, one of another account) is answered `{ "active": false }` with
//! every other claim omitted, which is the inactive-token response RFC 7662 §2.2 and
//! this operation's own documentation prescribe.
//!
//! This is a REPORTING operation (`@readonly`): it writes nothing, and a token's expiry
//! is decided against the clock when it is read rather than by a write here.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `IntrospectOAuth2TokenWithIAM`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-signin-signin/smithy-model/com-amazonaws-signin-signin.json")
//! ```
//!
//! Grants permission to inspect the metadata and state of an OAuth 2.0 access token or
//! refresh token Implements RFC 7662 OAuth 2.0 Token Introspection over a
//! SigV4-authenticated endpoint. Inspects the metadata of an access_token or refresh_token
//! issued by AWS Sign-In and returns the claims associated with it. Inactive token
//! semantics (RFC 7662 §2.2): when the supplied token is unknown, expired, revoked,
//! malformed, or owned by a different account, the response body is exactly { "active":
//! false } with all other claims omitted.
//!
//! ## Input
//!
//! * `token` (IntrospectionToken, len 1..4096, pattern ^ASO[AR][A-Za-z0-9+/=_\-]+$)
//!   **required** — The string value of the token to introspect. May be either an
//!   access_token or a refresh_token issued by AWS Sign-In.
//! * `tokenTypeHint` (TokenTypeHint, pattern ^(access_token|refresh_token)$) — Optional hint
//!   about the type of the token submitted for introspection. The server uses this hint to
//!   optimize lookup, but still falls back to the other token type on miss. Allowed values:
//!   access_token, refresh_token.
//!
//! ## Output
//!
//! * `accountId` (AccountId, len 12..12, pattern ^[0-9]{12}$) — 12-digit AWS account ID of
//!   the token's subject principal.
//! * `active` (Boolean) **required** — Indicates whether the token is currently active.
//!   `true` only when the token is valid, has not expired, has not been revoked, and belongs
//!   to the caller's account.
//! * `aud` (String) — Audience of the token: the OAuth resource the token is scoped to (for
//!   example, "aws-mcp.amazonaws.com"). Omitted for refresh tokens.
//! * `clientId` (String) — Client identifier for the OAuth 2.0 client that requested the
//!   token.
//! * `exp` (Long) — Token expiration time as a NumericDate (Unix epoch seconds).
//! * `iat` (Long) — Token issuance time as a NumericDate (Unix epoch seconds).
//! * `iss` (String) — Issuer of the token. Always "signin.amazonaws.com" for AWS Sign-In.
//! * `jti` (String) — Unique identifier for the token.
//! * `nbf` (Long) — Token "not before" time as a NumericDate (Unix epoch seconds).
//! * `resource` (String) — The OAuth resource the token is scoped to during Human OAuth flow.
//!   Only present for refresh token introspection.
//! * `signinSession` (String) — AWS Sign-In session ARN bound to the token, of the form
//!   arn:aws:signin:{region}:{account}:session/{uuid}.
//! * `sub` (String) — Subject of the token: the IAM principal ARN. For assumed-role sessions,
//!   this is the session ARN (matches sts:GetCallerIdentity's `Arn` field), e.g.
//!   arn:aws:sts::123456789012:assumed-role/MyRole/session-name.
//! * `tokenType` (IntrospectedTokenType, pattern ^(access_token|refresh_token)$) — Indicates
//!   which kind of token was introspected. One of "access_token" or "refresh_token".
//! * `userId` (String) — User identifier matching sts:GetCallerIdentity's `UserId` field for
//!   the token's subject principal (e.g. "AIDAEXAMPLE" for an IAM user, or
//!   "AROAEXAMPLE:session-name" for an assumed role).
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Error thrown for access denied scenarios with
//!   flexible HTTP status mapping Runtime HTTP Status Code Mapping: - HTTP 401
//!   (Unauthorized): TOKEN_EXPIRED, AUTHCODE_EXPIRED - HTTP 403 (Forbidden):
//!   USER_CREDENTIALS_CHANGED, INSUFFICIENT_PERMISSIONS The specific HTTP status code is
//!   determined at runtime based on the error enum value. Consumers should use the error
//!   field to determine the specific access denial reason.
//! * `InternalServerException` (HTTP 500) — Error thrown when an internal server error occurs
//!   HTTP Status Code: 500 Internal Server Error Used for unexpected server-side errors that
//!   prevent request processing.
//! * `TooManyRequestsError` (HTTP 429) — Error thrown when rate limit is exceeded HTTP Status
//!   Code: 429 Too Many Requests Possible OAuth2ErrorCode values: - INVALID_REQUEST: Rate
//!   limiting, too many requests, abuse prevention Possible causes: - Too many token requests
//!   from the same client - Rate limiting based on client_id or IP address - Abuse prevention
//!   mechanisms triggered - Service protection against excessive token generation
//! * `ValidationException` (HTTP 400) — Error thrown when request validation fails HTTP
//!   Status Code: 400 Bad Request Used for request validation errors such as malformed
//!   parameters, missing required fields, or invalid parameter values.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `IntrospectOAuth2TokenWithIAMError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-signin-signin.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `IntrospectOAuth2TokenWithIAMError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.signin#IntrospectOAuth2TokenWithIAM")]
#[derive(Debug)]
pub enum IntrospectOAuth2TokenWithIAMErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    TooManyRequestsError(TooManyRequestsError),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::introspect_o_auth2_token_with_iam::IntrospectOAuth2TokenWithIAMError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type IntrospectOAuth2TokenWithIAMError = crate::scaffolded::Scaffolded<IntrospectOAuth2TokenWithIAMErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<IntrospectOAuth2TokenWithIAMErrorModeled> for IntrospectOAuth2TokenWithIAMError {
    fn from(modeled: IntrospectOAuth2TokenWithIAMErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `IntrospectOAuth2TokenWithIAM` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct IntrospectOAuth2TokenWithIAM<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> IntrospectOAuth2TokenWithIAM<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for IntrospectOAuth2TokenWithIAM<Eng>
{
    type Input = IntrospectOAuth2TokenWithIAMInput;
    type Output = IntrospectOAuth2TokenWithIAMOutput;
    type Error = IntrospectOAuth2TokenWithIAMError;

    const NAME: &'static str = "IntrospectOAuth2TokenWithIAM";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INTROSPECT_O_AUTH2_TOKEN_WITH_IAM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INTROSPECT_O_AUTH2_TOKEN_WITH_IAM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                IntrospectOAuth2TokenWithIAMErrorModeled::InternalServerException(
                    InternalServerException {
                        error: crate::state::SERVER_ERROR_CODE.to_string(),
                        message: "the signin OAuth 2.0 token store is unavailable".to_string(),
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();
        // The account the caller speaks for; a token of any other account is
        // inactive to it. This emulator answers for one account, so this is the
        // account every token it issued belongs to.
        let caller_account = ::engine::emulator_account_id();

        // `tokenTypeHint` is advisory: the server "uses this hint to optimize
        // lookup, but still falls back to the other token type on miss", so a
        // lookup by the presented value — which is keyed by neither kind —
        // already answers for both, and the hint changes nothing this operation
        // reports.
        let _ = input.token_type_hint;

        // A read: the claims are computed from the stored record and nothing is
        // written, so introspecting a token leaves every later caller's answer
        // unchanged. An expired record stays resident until the next mint
        // reclaims it, and reads as inactive either way.
        let store = state.lock();
        let claims = store
            .token(&input.token)
            .filter(|token| token.is_active(now, caller_account))
            .map_or_else(crate::state::inactive_token, |token| token.to_claims());
        Ok(claims)
    }
}
