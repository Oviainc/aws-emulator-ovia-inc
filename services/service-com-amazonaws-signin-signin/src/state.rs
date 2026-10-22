// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-signin-signin service.
//!
//! Three resources live here, and every implemented handler of this service
//! reads and writes exactly these records:
//!
//! * the **resource-permission statement** — `PutResourcePermissionStatement`
//!   mints one, `ListResourcePermissionStatements` enumerates them,
//!   `GetResourcePolicy` renders them as the account's SignIn resource-based
//!   policy document and `DeleteResourcePermissionStatement` removes one;
//! * the **console-authorization configuration** —
//!   `PutConsoleAuthorizationConfiguration` persists one per target,
//!   `GetConsoleAuthorizationConfiguration` reports it back and
//!   `DeleteConsoleAuthorizationConfiguration` releases it;
//! * the **OAuth 2.0 token** — `CreateOAuth2TokenWithIAM` mints an access
//!   token, `IntrospectOAuth2TokenWithIAM` reports the claims of an access or
//!   refresh token the service issued and `RevokeOAuth2TokenWithIAM` revokes a
//!   refresh token together with the whole chain minted from it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    IntrospectOAuth2TokenWithIAMOutput, PermissionStatementSummary, PolicyStatement,
    ResourceNotFoundException, SigninResourceBasedPolicy, TooManyRequestsError, ValidationException,
};

/// The `Version` of the rendered `SigninResourceBasedPolicy`.
///
/// `2012-10-17` is the current IAM policy-language version every AWS
/// resource-based policy document declares; the model types `version` as a plain
/// string and no operation of this service accepts one, so the service's own
/// policy language version is the only value it can report.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The largest page `ListResourcePermissionStatements` serves, and the page size
/// it uses when the request names no `maxResults`:
/// `ListResourcePermissionStatementsInput$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const STATEMENT_PAGE_LIMIT: i32 = 100;

/// How many permission statements the store holds before
/// `PutResourcePermissionStatement` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any
/// account's limit: AWS bounds a resource-based policy by its document size and
/// publishes no statement count, and this emulator models neither Service Quotas
/// nor an account's adjustments. It is what stops the state growing without end —
/// every record it holds is released by `DeleteResourcePermissionStatement`,
/// which also forgets that statement's client tokens, so the token maps cannot
/// outgrow this bound either.
pub const PERMISSION_STATEMENT_QUOTA: usize = 100;

/// How many DISTINCT targets the console-authorization store holds before
/// `PutConsoleAuthorizationConfiguration` refuses to take on another one.
///
/// Like [`PERMISSION_STATEMENT_QUOTA`] this is the emulator's OWN bound on the
/// store, not a claim about any account's limit: AWS publishes no maximum number
/// of console-authorization targets and this emulator models neither Service
/// Quotas nor an account's adjustments. It is what stops the map growing without
/// end, since `TargetId`'s key space is effectively unlimited: a put naming a
/// target the store already holds is an update and always allowed, while a put
/// naming a NEW target past this bound is refused. Every record it holds is
/// released by `DeleteConsoleAuthorizationConfiguration`.
pub const CONSOLE_AUTHORIZATION_TARGET_QUOTA: usize = 100;

/// The `error` code the model documents for `ValidationException`: "Will be
/// INVALID_REQUEST for validation errors".
pub const VALIDATION_ERROR_CODE: &str = "INVALID_REQUEST";

/// The `error` code the model documents for `ResourceNotFoundException`: "Will be
/// RESOURCE_NOT_FOUND".
pub const NOT_FOUND_ERROR_CODE: &str = "RESOURCE_NOT_FOUND";

/// The `error` code the model documents for `ServiceQuotaExceededException`:
/// "Will be SERVICE_QUOTA_EXCEEDED".
pub const QUOTA_ERROR_CODE: &str = "SERVICE_QUOTA_EXCEEDED";

/// The `error` code the model documents for `TooManyRequestsError`: "Will be
/// INVALID_REQUEST for rate limiting scenarios".
pub const RATE_LIMIT_ERROR_CODE: &str = "INVALID_REQUEST";

/// The `error` code the model documents for `InternalServerException`: "Will be
/// SERVER_ERROR for internal server errors" — the enum member `SERVER_ERROR`
/// carries the wire value `server_error`.
pub const SERVER_ERROR_CODE: &str = "server_error";

/// The description `ValidationException` carries when an OAuth 2.0 request is
/// malformed.
///
/// Not chosen wording: it is what the model documents for the `INVALID_REQUEST`
/// member of `OAuth2ErrorCode` — "The request is missing a required parameter,
/// includes an invalid parameter value, or is otherwise malformed" — which is
/// also RFC 6749 §5.2's description of the `invalid_request` error.
pub const INVALID_REQUEST_MESSAGE: &str = "The request is missing a required parameter, \
     includes an invalid parameter value, or is otherwise malformed.";

/// `IntrospectOAuth2TokenWithIAM`'s own refusal of a request that carries no
/// token value at all.
pub const TOKEN_REQUIRED_MESSAGE: &str = "token is required";

/// `IntrospectOAuth2TokenWithIAM`'s own refusal of a token value that is not
/// shaped like a Sign-In token.
pub const TOKEN_INVALID_FORMAT_MESSAGE: &str = "token has invalid format";

/// The longest token value any Sign-In token member admits:
/// `IntrospectionToken` and `RevocationToken` are both `@length(min: 1, max:
/// 4096)`.
pub const TOKEN_MAX_LENGTH: usize = 4096;

/// Whether `value` is shaped like a token `IntrospectOAuth2TokenWithIAM`
/// accepts: `IntrospectionToken`'s `^ASO[AR][A-Za-z0-9+/=_\-]+$` within its
/// `@length(min: 1, max: 4096)`.
///
/// Spelled out here because this operation owns its own acceptance: the
/// handler, not the generic constraint walk, is what answers a value outside
/// it, so the handler needs the predicate.
pub fn is_introspection_token(value: &str) -> bool {
    let Some(rest) = value
        .strip_prefix(ACCESS_TOKEN_PREFIX)
        .or_else(|| value.strip_prefix(REFRESH_TOKEN_PREFIX))
    else {
        return false;
    };
    value.len() <= TOKEN_MAX_LENGTH && !rest.is_empty() && rest.chars().all(is_token_character)
}

/// Whether `value` is shaped like a token `RevokeOAuth2TokenWithIAM` revokes:
/// `RevocationToken`'s `^ASOR[A-Za-z0-9+/=_\-]+$` within its `@length(min: 1,
/// max: 4096)`. Anything else is a value revocation leaves alone — including an
/// access-token-shaped `ASOA…` value, which the model documents as not accepted
/// for revocation.
pub fn is_revocation_token(value: &str) -> bool {
    let Some(rest) = value.strip_prefix(REFRESH_TOKEN_PREFIX) else {
        return false;
    };
    value.len() <= TOKEN_MAX_LENGTH && !rest.is_empty() && rest.chars().all(is_token_character)
}

/// One character of the token alphabet both token shapes share:
/// `[A-Za-z0-9+/=_\-]`.
fn is_token_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
}

/// The region a request is attributed to when it carries no SigV4 credential
/// scope to read one from. `engine::emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `iss` claim of every token this service issues:
/// `IntrospectOAuth2TokenWithIAMResponse$iss` — "Always `signin.amazonaws.com`
/// for AWS Sign-In".
pub const TOKEN_ISSUER: &str = "signin.amazonaws.com";

/// The ARN namespace of this service (`aws.api#service` `arnNamespace`), used
/// for the sign-in session ARN a token is bound to.
pub const ARN_SERVICE: &str = "signin";

/// The `access_token` alternative of `IntrospectedTokenType` /`TokenTypeHint`,
/// both modelled `^(access_token|refresh_token)$`.
pub const TOKEN_TYPE_ACCESS: &str = "access_token";

/// The `refresh_token` alternative of the same two shapes.
pub const TOKEN_TYPE_REFRESH: &str = "refresh_token";

/// The prefix an access token carries.
///
/// `IntrospectionToken` is modelled `^ASO[AR][A-Za-z0-9+/=_\-]+$` — two token
/// families — and `RevocationToken`, which takes only a refresh token ("Only
/// the refresh_token type is accepted", `RevokeOAuth2TokenWithIAM`), narrows
/// that to `^ASOR[A-Za-z0-9+/=_\-]+$`. So `ASOR` is the refresh-token family
/// and the other alternative, `ASOA`, is the access-token one.
pub const ACCESS_TOKEN_PREFIX: &str = "ASOA";

/// The prefix a refresh token carries — `RevocationToken`'s own pattern.
pub const REFRESH_TOKEN_PREFIX: &str = "ASOR";

/// The `tokenType` `CreateOAuth2TokenWithIAM` answers with: `BearerTokenType`
/// is modelled `^Bearer$` — "Always "Bearer" per OAuth 2.1 specification".
pub const BEARER_TOKEN_TYPE: &str = "Bearer";

/// The only grant `CreateOAuth2TokenWithIAMRequest$grantType` admits:
/// `ClientCredentialsGrantType` is modelled `^client_credentials$`, documented
/// 'OAuth 2.0 grant type. Must be "client_credentials"'.
///
/// `CreateOAuth2TokenWithIAM` judges this member itself (see its handler), so
/// the value it accepts is named here rather than left to the generic
/// constraint walk.
pub const GRANT_TYPE_CLIENT_CREDENTIALS: &str = "client_credentials";

/// `CreateOAuth2TokenWithIAMRequest$resource`'s modelled `@length(min: 1, max:
/// 2048)` ceiling, likewise judged by that operation's own handler. Smithy bounds a
/// string's length in CHARACTERS, so the handler counts characters, not bytes.
pub const MAX_RESOURCE_LENGTH: usize = 2048;

/// The lifetime, in seconds, of an access token minted by
/// `CreateOAuth2TokenWithIAM`.
///
/// `CreateOAuth2TokenWithIAMResponse$expiresIn` documents "Token lifetime in
/// seconds. Value is the minimum of session validity and 1 hour", and
/// `TokenExpiresIn` is `@range(min: 1, max: 3600)`. This emulator's sign-in
/// sessions do not expire sooner than an hour, so the minimum is the hour.
pub const ACCESS_TOKEN_EXPIRES_IN: i64 = 3600;

/// How many OAuth 2.0 tokens the store holds before
/// `CreateOAuth2TokenWithIAM` refuses to mint another one.
///
/// Like [`PERMISSION_STATEMENT_QUOTA`] this is the emulator's OWN bound on the
/// store, not a claim about any account's limit: it is what stops the token map
/// growing without end. Every expired record is reclaimed before the bound is
/// tested, so the bound is only ever reached by that many tokens that are all
/// still live.
pub const OAUTH_TOKEN_QUOTA: usize = 100;

/// The IAM user name of the ambient caller — the identity this emulator
/// attributes a request to.
///
/// The same name the emulator's STS service reports from `GetCallerIdentity`
/// for a request whose access key names no session, so the `sub` and `userId`
/// claims of a token agree with `sts:GetCallerIdentity` as
/// `IntrospectOAuth2TokenWithIAMResponse$sub`/`$userId` document.
const AMBIENT_USER_NAME: &str = "emulator";

/// The alphabet an AWS unique id (`AIDAEXAMPLE`-style) is drawn from, and the
/// number of characters after its prefix — as the emulator's STS service mints
/// them, so a `userId` claim has the shape `sts:GetCallerIdentity` reports.
const UNIQUE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// How many characters follow a unique id's four-character prefix.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// `ConditionBlock`: the condition operator to the condition keys it tests, each
/// key holding the values it is tested against.
///
/// The model types it `map<ConditionType, map<String, list<String>>>`, which is
/// the IAM policy-document condition grammar:
/// `"Condition": { "<operator>": { "<condition key>": [values] } }`.
pub type ConditionBlock = HashMap<String, HashMap<String, Vec<String>>>;

/// The IAM condition operator an exact-match condition key is tested under.
const STRING_EQUALS: &str = "StringEquals";

/// The IAM condition operator an address or CIDR condition key is tested under.
const IP_ADDRESS: &str = "IpAddress";

/// The IAM condition operator the principal a statement does NOT apply to is
/// tested under.
const ARN_NOT_EQUALS: &str = "ArnNotEquals";

/// The AWS global condition key a VPC identifier is tested against.
const SOURCE_VPC_KEY: &str = "aws:SourceVpc";

/// The AWS global condition key a VPC endpoint identifier is tested against.
/// Both the SignIn and the console endpoint of one statement are values of this
/// one key.
const SOURCE_VPCE_KEY: &str = "aws:SourceVpce";

/// The condition key an address inside the VPC is tested against.
const VPC_SOURCE_IP_KEY: &str = "aws:VpcSourceIp";

/// The AWS global condition key the requester's address is tested against.
const SOURCE_IP_KEY: &str = "aws:SourceIp";

/// The AWS global condition key the region of the request is tested against.
const REQUESTED_REGION_KEY: &str = "aws:RequestedRegion";

/// The AWS global condition key the calling principal's ARN is tested against.
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";

/// The authorization scope of a 12-digit account id target.
pub const ACCOUNT_SCOPE: &str = "ACCOUNT";

/// The authorization scope of an `o-` AWS Organizations organization target.
pub const ORGANIZATION_SCOPE: &str = "ORGANIZATION";

/// The authorization scope of an `r-` AWS Organizations root target.
pub const ROOT_SCOPE: &str = "ROOT";

/// One permission statement of the account's SignIn resource-based policy, as
/// this emulator persists it: the id it was minted with and the condition-key
/// members the request supplied.
#[derive(Debug, Clone)]
pub struct PermissionStatement {
    /// `StatementId`'s `^[A-Za-z0-9+/]{64}=?$`, minted by
    /// [`ComAmazonawsSigninSigninState::mint_statement_id`] and stored here: it
    /// is what `PutResourcePermissionStatement` returns and what
    /// `DeleteResourcePermissionStatement` names.
    pub statement_id: String,
    pub source_vpc: Option<String>,
    pub signin_source_vpce: Option<String>,
    pub console_source_vpce: Option<String>,
    pub vpc_source_ip: Option<String>,
    pub source_ip: Option<String>,
    pub requested_region: Option<String>,
    pub excluded_principal: Option<String>,
}

impl PermissionStatement {
    /// The populated condition members, projected into the `ConditionBlock` the
    /// list summary and the policy document both carry.
    ///
    /// Each member becomes a value of the AWS global condition key that names
    /// it, under the operator its type is tested with: identifiers and the
    /// region by string equality, addresses by `IpAddress`, and the excluded
    /// principal by `ArnNotEquals` — the statement is the one that does NOT
    /// apply to it. A member the request did not carry contributes nothing, so
    /// a statement created with no condition member at all has no condition
    /// block.
    pub fn conditions(&self) -> Option<ConditionBlock> {
        let mut block = ConditionBlock::new();
        let mut add = |operator: &str, key: &str, value: &Option<String>| {
            if let Some(value) = value {
                block
                    .entry(operator.to_string())
                    .or_default()
                    .entry(key.to_string())
                    .or_default()
                    .push(value.clone());
            }
        };

        add(STRING_EQUALS, SOURCE_VPC_KEY, &self.source_vpc);
        add(STRING_EQUALS, SOURCE_VPCE_KEY, &self.signin_source_vpce);
        add(STRING_EQUALS, SOURCE_VPCE_KEY, &self.console_source_vpce);
        add(STRING_EQUALS, REQUESTED_REGION_KEY, &self.requested_region);
        add(IP_ADDRESS, VPC_SOURCE_IP_KEY, &self.vpc_source_ip);
        add(IP_ADDRESS, SOURCE_IP_KEY, &self.source_ip);
        add(ARN_NOT_EQUALS, PRINCIPAL_ARN_KEY, &self.excluded_principal);

        (!block.is_empty()).then_some(block)
    }

    /// The `PermissionStatementSummary` projection
    /// `ListResourcePermissionStatements` returns.
    pub fn to_summary(&self) -> PermissionStatementSummary {
        PermissionStatementSummary {
            sid: self.statement_id.clone(),
            condition: self.conditions(),
        }
    }

    /// This statement as one `PolicyStatement` of the rendered policy document.
    ///
    /// Only `Condition` is reported: the model's `PolicyStatement` carries
    /// `Effect`, `Principal`, `Action` and `Resource` as OPTIONAL members and
    /// this service's surface has no input for any of them, so the emulator
    /// reports the conditions it stored rather than inventing a frame around
    /// them. `PolicyStatement` models no `Sid` member either — the statement id
    /// is reported by `PutResourcePermissionStatement` and by
    /// `ListResourcePermissionStatements`, which is where the model puts it.
    pub fn to_policy_statement(&self) -> PolicyStatement {
        PolicyStatement {
            effect: None,
            principal: None,
            action: None,
            resource: None,
            condition: self.conditions(),
        }
    }
}

/// The console-authorization configuration of one target, as
/// `PutConsoleAuthorizationConfiguration` persisted it.
#[derive(Debug, Clone)]
pub struct ConsoleAuthorizationConfiguration {
    /// `TargetId`: a 12-digit account id, an `o-` organization id or an `r-`
    /// root id.
    pub target_id: String,
    /// The scope the target's form implies — see [`scope_for_target`].
    pub scope: String,
    /// Whether console authorization is configured for this target.
    pub console_authorization_enabled: bool,
}

impl ConsoleAuthorizationConfiguration {
    /// The three members `Put`/`Get`/`DeleteConsoleAuthorizationConfiguration`
    /// all report, read off the stored record.
    pub fn to_members(&self) -> (String, String, bool) {
        (
            self.target_id.clone(),
            self.scope.clone(),
            self.console_authorization_enabled,
        )
    }
}

/// One OAuth 2.0 token this service issued, as it is persisted: the opaque
/// value it was handed out as, the claims `IntrospectOAuth2TokenWithIAM`
/// reports for it, and whether `RevokeOAuth2TokenWithIAM` has revoked it.
#[derive(Clone)]
pub struct OAuth2Token {
    /// The opaque token value the caller presents. SENSITIVE: `token` is
    /// `@sensitive` on every operation that takes one, so [`std::fmt::Debug`]
    /// below leaves it out.
    pub token: String,
    /// [`TOKEN_TYPE_ACCESS`] or [`TOKEN_TYPE_REFRESH`] — which kind of token
    /// this is, reported as the `tokenType` claim.
    pub token_type: String,
    /// The OAuth 2.0 client that requested the token, for the `clientId` claim.
    /// `None` for a token minted through the IAM flow
    /// (`CreateOAuth2TokenWithIAM`), where an IAM principal authenticated with
    /// SigV4 and no OAuth client took part, so there is no client identifier to
    /// report.
    pub client_id: Option<String>,
    /// The OAuth resource the token is scoped to: the `aud` claim of an access
    /// token and the `resource` claim of a refresh token.
    pub resource: String,
    /// The account of the token's subject principal, for the `accountId` claim.
    /// A token belonging to another account is INACTIVE to this caller
    /// (RFC 7662 §2.2).
    pub account_id: String,
    /// The IAM principal ARN the token speaks for, for the `sub` claim.
    pub subject: String,
    /// The subject principal's AWS unique id, for the `userId` claim.
    pub user_id: String,
    /// The sign-in session ARN the token is bound to, for the `signinSession`
    /// claim.
    pub session_arn: String,
    /// This token's own unique identifier, for the `jti` claim.
    pub jti: String,
    /// When the token was issued, for the `iat` and `nbf` claims.
    pub issued_at: SystemTime,
    /// The token's lifetime in seconds: `iat + expires_in` is the `exp` claim.
    pub expires_in: i64,
    /// The token chain this token belongs to: the `jti` of the refresh token it
    /// was minted from, or its own when it heads its chain. Revoking a refresh
    /// token invalidates "the entire token chain"
    /// (`RevokeOAuth2TokenWithIAM`), which is every token sharing this value.
    pub chain: String,
    /// Whether a revocation has invalidated this token.
    pub revoked: bool,
}

/// Prints the token's claims but never the token value itself.
impl std::fmt::Debug for OAuth2Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuth2Token")
            .field("token_type", &self.token_type)
            .field("client_id", &self.client_id)
            .field("resource", &self.resource)
            .field("account_id", &self.account_id)
            .field("subject", &self.subject)
            .field("jti", &self.jti)
            .field("issued_at", &self.issued_at)
            .field("expires_in", &self.expires_in)
            .field("revoked", &self.revoked)
            .finish_non_exhaustive()
    }
}

impl OAuth2Token {
    /// The instant the token stops being usable.
    pub fn expires_at(&self) -> SystemTime {
        self.issued_at + Duration::from_secs(u64::try_from(self.expires_in.max(0)).unwrap_or(0))
    }

    /// Whether the token's lifetime has run out at `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at()
    }

    /// Whether this is a refresh token.
    pub fn is_refresh_token(&self) -> bool {
        self.token_type == TOKEN_TYPE_REFRESH
    }

    /// Whether the token is active for a caller in `caller_account`:
    /// `IntrospectOAuth2TokenWithIAMResponse$active` is "`true` only when the
    /// token is valid, has not expired, has not been revoked, and belongs to
    /// the caller's account".
    pub fn is_active(&self, now: SystemTime, caller_account: &str) -> bool {
        !self.revoked && !self.is_expired(now) && self.account_id == caller_account
    }

    /// The introspection response for this token: every claim the model
    /// declares, read off the stored record.
    ///
    /// `aud` and `resource` carry the same stored value but belong to different
    /// token kinds: `aud` is "Omitted for refresh tokens" and `resource` is
    /// "Only present for refresh token introspection".
    pub fn to_claims(&self) -> IntrospectOAuth2TokenWithIAMOutput {
        let issued_at = epoch_seconds(self.issued_at);
        let refresh = self.is_refresh_token();
        IntrospectOAuth2TokenWithIAMOutput {
            active: true,
            client_id: self.client_id.clone(),
            user_id: Some(self.user_id.clone()),
            token_type: Some(self.token_type.clone()),
            exp: Some(epoch_seconds(self.expires_at())),
            iat: Some(issued_at),
            // The token is usable from the moment it was issued, so "not
            // before" is its issuance time.
            nbf: Some(issued_at),
            sub: Some(self.subject.clone()),
            aud: (!refresh).then(|| self.resource.clone()),
            iss: Some(TOKEN_ISSUER.to_string()),
            jti: Some(self.jti.clone()),
            account_id: Some(self.account_id.clone()),
            signin_session: Some(self.session_arn.clone()),
            resource: refresh.then(|| self.resource.clone()),
        }
    }
}

/// What a token is minted with — everything about a new token that is not
/// drawn from the random source or read off the clock.
#[derive(Debug, Clone)]
pub struct NewToken {
    /// [`TOKEN_TYPE_ACCESS`] or [`TOKEN_TYPE_REFRESH`].
    pub token_type: &'static str,
    /// The OAuth resource the token is scoped to.
    pub resource: String,
    /// The OAuth 2.0 client that requested it, if any.
    pub client_id: Option<String>,
    /// The region whose sign-in session the token is bound to.
    pub region: String,
    /// The token's lifetime in seconds.
    pub expires_in: i64,
    /// The chain to join — the `jti` of the refresh token this token was minted
    /// from. `None` heads a new chain, which is what a token minted straight
    /// from an IAM principal does.
    pub chain: Option<String>,
    /// The sign-in session ARN to bind to, when the token joins the session of
    /// the chain it was minted from. `None` mints a fresh session.
    pub session_arn: Option<String>,
}

/// The inactive-token response, which RFC 7662 §2.2 — quoted by
/// `IntrospectOAuth2TokenWithIAM` — fixes exactly: "the response body is
/// exactly { "active": false } with all other claims omitted".
pub fn inactive_token() -> IntrospectOAuth2TokenWithIAMOutput {
    IntrospectOAuth2TokenWithIAMOutput {
        active: false,
        ..IntrospectOAuth2TokenWithIAMOutput::default()
    }
}

/// Per-service state for com-amazonaws-signin-signin. A handler reaches it through [`service_state`].
#[derive(Default)]
pub struct ComAmazonawsSigninSigninState {
    /// Every permission statement, keyed by its id. Ids are minted in a
    /// lexicographically increasing sequence, so a `BTreeMap` holds them in
    /// mint order — which is what gives `ListResourcePermissionStatements` the
    /// stable total order its pagination token indexes into, and
    /// `GetResourcePolicy` a deterministic document.
    pub permission_statements: BTreeMap<String, PermissionStatement>,
    /// A `PutResourcePermissionStatement` `clientToken` to the statement that
    /// token created, so a repeated put answers with the first statement
    /// instead of minting a second one. Bounded by the store it points into: an
    /// entry is only written beside a newly minted statement (itself capped by
    /// [`PERMISSION_STATEMENT_QUOTA`]) and is dropped when that statement is
    /// deleted.
    pub statement_idempotency: HashMap<String, String>,
    /// A `DeleteResourcePermissionStatement` `clientToken` to the statement it
    /// removed, so a retried delete answers the same success rather than a 404.
    /// Written only through [`ComAmazonawsSigninSigninState::remember_deletion`],
    /// which keeps it to the [`PERMISSION_STATEMENT_QUOTA`] most recent deletes.
    pub deletion_idempotency: HashMap<String, String>,
    /// The tokens of [`Self::deletion_idempotency`] in the order they were
    /// recorded, so the oldest is the one evicted when the bound is reached.
    pub deletion_token_order: VecDeque<String>,
    /// The console-authorization configuration of each configured target.
    /// Bounded by [`CONSOLE_AUTHORIZATION_TARGET_QUOTA`]: a put naming a new
    /// target past that count is refused rather than stored.
    pub console_auth_configs: HashMap<String, ConsoleAuthorizationConfiguration>,
    /// Every OAuth 2.0 token this service issued, keyed by the opaque token
    /// value the caller presents — which is what both
    /// `IntrospectOAuth2TokenWithIAM` and `RevokeOAuth2TokenWithIAM` look a
    /// token up by. Bounded by [`OAUTH_TOKEN_QUOTA`], with every expired record
    /// reclaimed before a new token is minted.
    pub oauth_tokens: HashMap<String, OAuth2Token>,
    /// The counter statement ids are minted from.
    pub statement_id_seq: u64,
}

/// Prints the store, but reports the token map by SIZE only: its keys are the
/// opaque token values themselves, which are `@sensitive` on the wire and must
/// not reach a log through a formatted store.
impl std::fmt::Debug for ComAmazonawsSigninSigninState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComAmazonawsSigninSigninState")
            .field("permission_statements", &self.permission_statements)
            .field("statement_idempotency", &self.statement_idempotency)
            .field("deletion_idempotency", &self.deletion_idempotency)
            .field("deletion_token_order", &self.deletion_token_order)
            .field("console_auth_configs", &self.console_auth_configs)
            .field("oauth_tokens", &self.oauth_tokens.len())
            .field("statement_id_seq", &self.statement_id_seq)
            .finish()
    }
}

impl ComAmazonawsSigninSigninState {
    /// Mint an unused statement id matching `StatementId`'s
    /// `^[A-Za-z0-9+/]{64}=?$`.
    ///
    /// The counter is rendered as 64 lowercase hex digits — an alphabet that
    /// pattern admits and whose ASCII order is its numeric order, so ids sort in
    /// mint order and one of them is usable as a `nextToken` (whose own pattern
    /// `^[-a-zA-Z0-9+=/_]*$` admits it too). The loop skips an id already taken,
    /// which is what makes the counter's wrap safe.
    pub fn mint_statement_id(&mut self) -> String {
        loop {
            self.statement_id_seq = self.statement_id_seq.wrapping_add(1);
            let id = format!("{:064x}", self.statement_id_seq);
            if !self.permission_statements.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record that `token` deleted `statement_id`, keeping at most
    /// [`PERMISSION_STATEMENT_QUOTA`] records: once that many deletes are
    /// remembered the OLDEST is forgotten to make room, so the map stays bounded
    /// without the idempotency guarantee decaying as the store ages — a retry of
    /// a recent delete still answers its original success.
    pub fn remember_deletion(&mut self, token: String, statement_id: String) {
        if self
            .deletion_idempotency
            .insert(token.clone(), statement_id)
            .is_none()
        {
            self.deletion_token_order.push_back(token);
        }
        while self.deletion_token_order.len() > PERMISSION_STATEMENT_QUOTA {
            if let Some(evicted) = self.deletion_token_order.pop_front() {
                self.deletion_idempotency.remove(&evicted);
            }
        }
    }

    /// True when `token` is a pagination token this service could have issued:
    /// one of the statement ids [`Self::mint_statement_id`] has already minted.
    ///
    /// `ListResourcePermissionStatements`' `nextToken` is modelled
    /// `^[-a-zA-Z0-9+=/_]*$`, far wider than the ids this service mints, so this
    /// is what separates a token the caller got from a page of this operation
    /// from one it invented. A minted id is 64 lowercase hex digits rendering a
    /// counter value the mint has reached, so a token is issuable exactly when it
    /// reads as such a value; the id of a statement deleted meanwhile stays
    /// issuable, which is what lets a page still be served after the statement
    /// its token named is gone.
    pub fn is_issued_pagination_token(&self, token: &str) -> bool {
        token.len() == 64
            && u128::from_str_radix(token, 16)
                .is_ok_and(|value| (1..=u128::from(self.statement_id_seq)).contains(&value))
    }

    /// Render the account's SignIn resource-based policy from the stored
    /// statements: one policy statement per stored permission statement, in
    /// mint order. With nothing stored the policy carries an empty statement
    /// list — the account has a policy, it just permits nothing extra.
    pub fn to_policy(&self) -> SigninResourceBasedPolicy {
        SigninResourceBasedPolicy {
            version: Some(POLICY_VERSION.to_string()),
            statement: Some(
                self.permission_statements
                    .values()
                    .map(PermissionStatement::to_policy_statement)
                    .collect(),
            ),
        }
    }

    /// The token record `value` names, whatever its kind: both operations that
    /// take a token resolve it by the value the caller presented, so a
    /// `tokenTypeHint` only ever saves a lookup — "the server uses this hint to
    /// optimize lookup, but still falls back to the other token type on miss"
    /// (`IntrospectOAuth2TokenWithIAMRequest$tokenTypeHint`).
    pub fn token(&self, value: &str) -> Option<&OAuth2Token> {
        self.oauth_tokens.get(value)
    }

    /// Reclaims every token past its own lifetime. An expired token is
    /// INACTIVE to `IntrospectOAuth2TokenWithIAM` and is no longer usable for
    /// anything else either, so dropping it changes no answer — which is what
    /// lets the clock, rather than a caller, bound this store.
    pub fn purge_expired_tokens(&mut self, now: SystemTime) {
        self.oauth_tokens.retain(|_, token| !token.is_expired(now));
    }

    /// Whether another token fits under [`OAUTH_TOKEN_QUOTA`].
    pub fn has_room_for_token(&self) -> bool {
        self.oauth_tokens.len() < OAUTH_TOKEN_QUOTA
    }

    /// Mints and persists a token, and hands back the record as stored.
    ///
    /// The token value and the identifiers are drawn from the system random
    /// source, never from the request, so a value is unguessable and no two
    /// tokens collide; the subject principal is the identity this emulator
    /// attributes the request to ([`ambient_subject`]).
    pub fn issue_token(&mut self, new: NewToken, now: SystemTime) -> OAuth2Token {
        let prefix = if new.token_type == TOKEN_TYPE_REFRESH {
            REFRESH_TOKEN_PREFIX
        } else {
            ACCESS_TOKEN_PREFIX
        };
        let jti = random_uuid();
        let token = OAuth2Token {
            token: self.fresh_token_value(prefix),
            token_type: new.token_type.to_string(),
            client_id: new.client_id,
            resource: new.resource,
            account_id: ::engine::emulator_account_id().to_string(),
            user_id: ambient_user_id(),
            subject: ambient_subject(),
            session_arn: new
                .session_arn
                .unwrap_or_else(|| signin_session_arn(&new.region, &random_uuid())),
            chain: new.chain.unwrap_or_else(|| jti.clone()),
            jti,
            issued_at: now,
            expires_in: new.expires_in,
            revoked: false,
        };
        self.oauth_tokens.insert(token.token.clone(), token.clone());
        token
    }

    /// Revokes the refresh token `value` names and the whole chain minted from
    /// it, and answers whether any record was revoked.
    ///
    /// `RevokeOAuth2TokenWithIAM` "invalidat[es] the entire token chain so that
    /// the refresh_token can no longer be used to mint new access_tokens": the
    /// access tokens already minted from it share its chain, so they are
    /// revoked with it. The records are KEPT and flagged rather than dropped,
    /// so a revoked token stays distinguishable until its lifetime runs out.
    pub fn revoke_token_chain(&mut self, value: &str) -> bool {
        let Some(chain) = self
            .oauth_tokens
            .get(value)
            .filter(|token| token.is_refresh_token())
            .map(|token| token.chain.clone())
        else {
            return false;
        };
        let mut revoked = false;
        for token in self.oauth_tokens.values_mut() {
            if token.chain == chain && !token.revoked {
                token.revoked = true;
                revoked = true;
            }
        }
        revoked
    }

    /// A token value of `prefix` the store does not already hold.
    fn fresh_token_value(&self, prefix: &str) -> String {
        loop {
            let candidate = format!("{prefix}{}", mint_opaque_value());
            if !self.oauth_tokens.contains_key(&candidate) {
                return candidate;
            }
        }
    }
}

/// The authorization scope `TargetId`'s form implies.
///
/// `TargetId` is modelled `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$` — an
/// account id, an AWS Organizations organization id or an organizational root
/// id — so the scope is a function of which alternative the value matches.
/// `scope` is modelled as a plain string with no enum, so these three names are
/// this emulator's, named after the Organizations entity each target form
/// identifies. `None` is unreachable through a request the boundary validator
/// admitted, and the handlers refuse it as a validation failure rather than
/// guessing a scope.
pub fn scope_for_target(target_id: &str) -> Option<&'static str> {
    if target_id.len() == 12 && target_id.bytes().all(|b| b.is_ascii_digit()) {
        return Some(ACCOUNT_SCOPE);
    }
    if target_id.starts_with("o-") {
        return Some(ORGANIZATION_SCOPE);
    }
    if target_id.starts_with("r-") {
        return Some(ROOT_SCOPE);
    }
    None
}

/// The `ResourceNotFoundException` payload for a statement id that names no
/// stored statement.
pub fn statement_not_found(statement_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        error: NOT_FOUND_ERROR_CODE.to_string(),
        message: format!("no permission statement with statementId {statement_id}"),
    }
}

/// The `ResourceNotFoundException` payload for a target whose console
/// authorization was never configured.
pub fn configuration_not_found(target_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        error: NOT_FOUND_ERROR_CODE.to_string(),
        message: format!("no console authorization configuration for targetId {target_id}"),
    }
}

/// The `TooManyRequestsError` payload for a put that would take the
/// console-authorization store past [`CONSOLE_AUTHORIZATION_TARGET_QUOTA`].
///
/// `PutConsoleAuthorizationConfiguration` does not declare
/// `ServiceQuotaExceededException`, so the store's bound is reported with the
/// modelled refusal it does declare for exactly this shape of request: the one
/// the model documents for "abuse prevention" and "service protection against
/// excessive" use.
pub fn console_target_quota_exceeded(target_id: &str) -> TooManyRequestsError {
    TooManyRequestsError {
        error: RATE_LIMIT_ERROR_CODE.to_string(),
        message: format!(
            "console authorization is already configured for the maximum of {} targets; \
             configuring targetId {target_id} as well would exceed it",
            CONSOLE_AUTHORIZATION_TARGET_QUOTA
        ),
    }
}

/// The modelled `ValidationException` payload carrying `message`.
pub fn validation_failed(message: impl Into<String>) -> ValidationException {
    ValidationException {
        error: VALIDATION_ERROR_CODE.to_string(),
        message: message.into(),
    }
}

/// The target a console-authorization call addresses: the `targetId` it carries,
/// or the caller's own account when it carries none.
pub fn requested_target(target_id: Option<String>) -> String {
    target_id.unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSigninSigninState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSigninSigninState>)
}

/// The region this request was signed for, read from its SigV4 credential
/// scope — the only region a request carries — or [`DEFAULT_REGION`] when it
/// carries none.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A sign-in session ARN:
/// `IntrospectOAuth2TokenWithIAMResponse$signinSession` documents the form
/// `arn:aws:signin:{region}:{account}:session/{uuid}`. The account segment
/// comes from the engine, never from a literal.
pub fn signin_session_arn(region: &str, session_uuid: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("session/{session_uuid}"))
}

/// The IAM principal ARN this emulator attributes a request to.
///
/// A request carries no identity beyond its SigV4 access key id, and this
/// service holds no session registry to resolve one through, so the subject is
/// the ambient identity the emulator's STS service reports for exactly that
/// case: the IAM user of the emulator's own account. Both are built from
/// `engine`'s account contract, so a token's `sub` claim is the `Arn`
/// `sts:GetCallerIdentity` answers with, as the model says it must be.
pub fn ambient_subject() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"))
}

/// The subject principal's AWS unique id, for the `userId` claim.
pub fn ambient_user_id() -> String {
    unique_id("AIDA", &ambient_subject())
}

/// An AWS unique id: `prefix` plus [`UNIQUE_ID_SUFFIX_LEN`] characters derived
/// from `seed`.
///
/// The characters are a SHA-256 digest of `seed` mapped onto the id alphabet,
/// so the id is a function of the principal it names — the same principal
/// always yields the same id, two different ones practically never collide, and
/// nothing about it depends on when it was asked for. This is the derivation
/// the emulator's STS service mints `AIDA`/`AROA` ids with, so a `userId` claim
/// matches the `UserId` `sts:GetCallerIdentity` reports. It is NOT base32: the
/// mapping only has to be stable and well distributed, never decodable.
pub fn unique_id(prefix: &str, seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let suffix = digest
        .iter()
        .take(UNIQUE_ID_SUFFIX_LEN)
        .map(|byte| char::from(UNIQUE_ID_ALPHABET[usize::from(*byte) % UNIQUE_ID_ALPHABET.len()]));
    prefix.chars().chain(suffix).collect()
}

/// `at` as whole seconds since the Unix epoch — the NumericDate the `exp`,
/// `iat` and `nbf` claims are modelled as (`Long`, "Unix epoch seconds").
pub fn epoch_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A fresh random (version 4) UUID in its canonical hyphenated form, from the
/// system random source.
///
/// Drawn through `emulator_telemetry`'s own generator, a crate this service
/// already depends on, so the opaque values below add no dependency.
fn random_uuid() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// A fresh opaque token body: 244 bits from the system random source, rendered
/// as the hex digits of two UUIDs, so a token value can be resolved only by the
/// caller it was handed to. Every character is one `IntrospectionToken`'s
/// `^ASO[AR][A-Za-z0-9+/=_\-]+$` admits.
pub fn mint_opaque_value() -> String {
    let mut value = String::with_capacity(64);
    for _ in 0..2 {
        value.extend(random_uuid().chars().filter(|c| *c != '-'));
    }
    value
}

/// The `TooManyRequestsError` payload for a mint that would take the token
/// store past [`OAUTH_TOKEN_QUOTA`].
///
/// `CreateOAuth2TokenWithIAM` declares no quota error; this is the refusal its
/// model documents for exactly this shape of request — "abuse prevention
/// mechanisms triggered", "service protection against excessive token
/// generation".
pub fn token_quota_exceeded() -> TooManyRequestsError {
    TooManyRequestsError {
        error: RATE_LIMIT_ERROR_CODE.to_string(),
        message: format!(
            "this account already holds the maximum of {OAUTH_TOKEN_QUOTA} live OAuth 2.0 \
             tokens; revoke or let one expire before requesting another"
        ),
    }
}

