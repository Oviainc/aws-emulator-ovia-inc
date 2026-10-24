// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice service.
//!
//! One [`UserPool`] per created pool, each owning its app clients and its users,
//! beside the auth sessions and the tokens the sign-in operations issue. Every
//! operation computes its response from this state — a create stores a resource
//! and a later read renders the STORED resource, never the request that made it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::ring::signature::KeyPair;
use ::sha2::Digest;

use crate::ops::types::{
    AccountRecoverySettingType, AcrLevelConfigType, AdminCreateUserConfigType,
    AnalyticsConfigurationType, AttributeType, AuthenticationResultType, CustomDomainConfigType,
    DeviceConfigurationType, DomainDescriptionType, EmailConfigurationType, GroupType,
    IssuerConfigurationType, KeyConfigurationType, LambdaConfigType, MFAOptionType,
    NumberAttributeConstraintsType, PasswordPolicyType, RefreshTokenRotationType,
    ResourceServerScopeType, ResourceServerType, RoutingType,
    SchemaAttributeType, SignInPolicyType, SmsConfigurationType, StringAttributeConstraintsType,
    TokenValidityUnitsType, UserAttributeUpdateSettingsType, UserPoolAddOnsType,
    UserPoolClientType, UserPoolPolicyType, UserPoolType, UserType, UsernameConfigurationType,
    VerificationMessageTemplateType,
};

/// The region a request that carries no SigV4 credential scope falls back to, so
/// a minted pool id and ARN always name one region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service's SigV4 / endpoint / ARN namespace (OPERATIONS.md "Wire protocol").
pub const SIGNING_NAME: &str = "cognito-idp";

/// Amazon Cognito's default password policy for a new user pool: at least 8
/// characters, with an uppercase letter, a lowercase letter, a number and a
/// symbol required (Amazon Cognito Developer Guide, "Password policies"). The
/// model's `PasswordPolicyType.MinimumLength` range starts at 6, so 8 is a
/// default, not a bound.
const DEFAULT_PASSWORD_MIN_LENGTH: i32 = 8;

/// `PasswordPolicyType.TemporaryPasswordValidityDays`: "Defaults to `7`. If you
/// submit a value of `0`, Amazon Cognito treats it as a null value and sets
/// `TemporaryPasswordValidityDays` to its default value." (this service's model).
const DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS: i32 = 7;

/// `AdminCreateUserConfigType.UnusedAccountValidityDays`: "The default value for
/// this parameter is 7." (this service's model).
const DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS: i32 = 7;

/// `CreateUserPoolClient.AccessTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your access tokens are valid for one
/// hour." (this service's model). `IdTokenValidity` is documented the same way.
const DEFAULT_ACCESS_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// `CreateUserPoolClient.RefreshTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your refresh tokens are valid for 30
/// days", and a submitted `0` is overridden "with the default value of 30 days"
/// (this service's model).
const DEFAULT_REFRESH_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// `CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes, of
/// that session token" and its modelled range is 3..15, so an app client that
/// sets no value gets the range's minimum.
const DEFAULT_AUTH_SESSION_VALIDITY_MINUTES: u64 = 3;

/// `CreateUserPoolClient.ExplicitAuthFlows`: "If you don't specify a value for
/// `ExplicitAuthFlows`, your app client supports `ALLOW_REFRESH_TOKEN_AUTH`,
/// `ALLOW_USER_SRP_AUTH`, and `ALLOW_CUSTOM_AUTH`." (this service's model).
pub const DEFAULT_EXPLICIT_AUTH_FLOWS: [&str; 3] = [
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_USER_SRP_AUTH",
    "ALLOW_CUSTOM_AUTH",
];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=ADMIN_USER_PASSWORD_AUTH`:
/// the current one and the legacy one it "replaces"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const ADMIN_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ADMIN_NO_SRP_AUTH"];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=USER_PASSWORD_AUTH`:
/// "`ALLOW_USER_PASSWORD_AUTH`: Enable user password-based authentication", beside
/// the legacy `USER_PASSWORD_AUTH` the same member documents
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_USER_PASSWORD_AUTH", "USER_PASSWORD_AUTH"];

/// The `ExplicitAuthFlows` value that enables `AuthFlow=REFRESH_TOKEN_AUTH` and its
/// `REFRESH_TOKEN` alias: "`ALLOW_REFRESH_TOKEN_AUTH`: Enable authflow to refresh
/// tokens" (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model), which
/// is also one of the three a client that sets no `ExplicitAuthFlows` gets.
pub const REFRESH_TOKEN_AUTH_FLOWS: [&str; 1] = ["ALLOW_REFRESH_TOKEN_AUTH"];

/// `UserPoolTierType` defaults to `ESSENTIALS` (`CreateUserPool.UserPoolTier`).
const DEFAULT_USER_POOL_TIER: &str = "ESSENTIALS";

/// `UserPoolMfaType` is `OFF` for a pool that configures no MFA
/// (`MfaConfiguration`: "Sets multi-factor authentication (MFA) to be on, off, or
/// optional" in this service's model).
const DEFAULT_MFA_CONFIGURATION: &str = "OFF";

/// Deletion protection is opt-in: "When active, `DeletionProtection` prevents
/// accidental deletion of your user pool" (this service's model), so a pool that
/// asks for none can be deleted.
const DEFAULT_DELETION_PROTECTION: &str = "INACTIVE";

/// `PreventUserExistenceErrors` "Defaults to `LEGACY`"
/// (`CreateUserPoolClient.PreventUserExistenceErrors` in this service's model),
/// which is the mode that answers `UserNotFoundException` for an absent user.
const DEFAULT_PREVENT_USER_EXISTENCE_ERRORS: &str = "LEGACY";

/// The message every `UserNotFoundException` carries.
///
/// `UserNotFoundException` is "the message returned when a user isn't found"
/// (this service's model) and the answer is the same sentence whatever was asked
/// for: it names neither the username nor the pool, so the error reports that
/// the lookup found nothing without telling the caller which identity it looked
/// for. A message built from the requested username would answer a different
/// sentence per username, which is what makes the refusal an existence oracle.
pub const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

// ---------------------------------------------------------------------------
// Password policy rejection wording
//
// A rejection says that the password missed the pool's policy and which
// requirement it missed, in a fixed clause per requirement: the clause is the
// same whatever the pool configured, so the refusal does not report the pool's
// policy values to a caller that has not read them.
// ---------------------------------------------------------------------------

/// The prefix every password-policy rejection message carries, ahead of the
/// clause for the requirement the password missed.
const PASSWORD_POLICY_PREFIX: &str = "Password did not conform with password policy: ";

/// The clause for a password shorter than `PasswordPolicyType.MinimumLength`.
const PASSWORD_TOO_SHORT: &str = "Password not long enough";

/// The clause for `PasswordPolicyType.RequireUppercase`.
const PASSWORD_NEEDS_UPPERCASE: &str = "Password must have uppercase characters";

/// The clause for `PasswordPolicyType.RequireLowercase`.
const PASSWORD_NEEDS_LOWERCASE: &str = "Password must have lowercase characters";

/// The clause for `PasswordPolicyType.RequireNumbers`.
const PASSWORD_NEEDS_NUMBERS: &str = "Password must have numeric characters";

/// The clause for `PasswordPolicyType.RequireSymbols`.
const PASSWORD_NEEDS_SYMBOLS: &str = "Password must have symbol characters";

/// The message a password-policy rejection carries for the requirement
/// `requirement` names.
fn password_policy_message(requirement: &str) -> String {
    format!("{PASSWORD_POLICY_PREFIX}{requirement}")
}

// ---------------------------------------------------------------------------
// User pool domains
// ---------------------------------------------------------------------------

/// The managed login branding version for hosted UI (classic): "A value of `1`
/// indicates hosted UI (classic) and a version of `2` indicates managed login"
/// (`CreateUserPoolDomain.ManagedLoginVersion` in this service's model).
pub const HOSTED_UI_CLASSIC_BRANDING: i32 = 1;

/// The managed login branding version, which the same member documents as
/// needing "your user pool be configured for any feature plan other than
/// `Lite`".
pub const MANAGED_LOGIN_BRANDING: i32 = 2;

/// The only branding versions `ManagedLoginVersion` names, so a request asking
/// for any other value asks for a version that does not exist.
pub const MANAGED_LOGIN_VERSIONS: [i32; 2] = [HOSTED_UI_CLASSIC_BRANDING, MANAGED_LOGIN_BRANDING];

/// The branding version a domain created without one carries: the model names
/// only the two above, and hosted UI (classic) is the one that needs no feature
/// plan.
pub const DEFAULT_MANAGED_LOGIN_VERSION: i32 = HOSTED_UI_CLASSIC_BRANDING;

/// The feature plan managed login is NOT included in
/// (`CreateUserPoolDomain.ManagedLoginVersion`); one of the modelled
/// `UserPoolTierType` values.
pub const LITE_USER_POOL_TIER: &str = "LITE";

/// The message a `FeatureUnavailableInTierException` carries: a tier refusal
/// reports the pool's configured feature plan and the feature that plan does not
/// include, in one template, so a caller can parse the feature out of it and
/// tell its operator what to upgrade.
///
/// `tier` is the pool's stored `UserPoolTier` (a `UserPoolTierType` value) and
/// `feature` names the feature the request asked for.
pub fn feature_unavailable_in_tier_message(tier: &str, feature: &str) -> String {
    format!("The following feature is not available for the {tier} pricing tier configured: {feature}")
}

/// The feature name a tier refusal gives for a `ManagedLoginVersion` of
/// `version`: the branding version is part of the feature asked for, since only
/// managed login (version 2) needs a feature plan other than
/// [`LITE_USER_POOL_TIER`].
pub fn managed_login_feature_name(version: i32) -> String {
    format!("Managed Login Version {version}")
}

/// The `DomainStatusType` a freshly created domain is in, until the clock makes
/// its activation due: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";

/// The `DomainStatusType` a domain reaches once [`DOMAIN_ACTIVATION_SPAN`] has
/// elapsed: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a created domain stays `CREATING` before the clock owes it `ACTIVE`.
///
/// A chosen span, not a documented duration: `CreateUserPoolDomain` only says
/// "Your prefix domain might take up to one minute to take effect" and "Your
/// custom domain is online within five minutes", so a domain is NOT ready when
/// the call returns, and this emulator makes the transition due shortly after
/// instead of waiting minutes. Never zero, so a caller reading the domain right
/// after creating it still sees `CREATING`.
pub const DOMAIN_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Store bounds
//
// This state lives in the emulator's memory, so every store a caller can grow
// is bounded: a repeated request must not grow the process without limit.
// These caps are the EMULATOR's, not an AWS quota, and are deliberately far
// above anything a test fixture builds. The two credential stores are swept of
// what the clock has already expired before the cap is applied, so the cap is
// reached only by live state.
// ---------------------------------------------------------------------------

/// The most user pools this emulator holds.
pub const MAX_USER_POOLS: usize = 1_000;

/// The most app clients one user pool holds.
pub const MAX_CLIENTS_PER_POOL: usize = 1_000;

/// The most users one user pool holds.
pub const MAX_USERS_PER_POOL: usize = 10_000;

/// The most groups one user pool holds.
pub const MAX_GROUPS_PER_POOL: usize = 1_000;

/// The most resource servers one user pool holds.
pub const MAX_RESOURCE_SERVERS_PER_POOL: usize = 1_000;

/// The most unexpired challenge sessions this emulator holds; the oldest are
/// evicted past it.
const MAX_AUTH_SESSIONS: usize = 10_000;

/// The most unexpired issued tokens this emulator holds; the oldest are evicted
/// past it. Three are recorded per sign-in, two per refresh.
const MAX_ISSUED_TOKENS: usize = 30_000;

/// Per-service state. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// Every created user pool, keyed by its minted `UserPoolId`.
    pub user_pools: HashMap<String, UserPool>,
    /// Challenge sessions, keyed by the opaque `Session` token handed to the caller.
    pub auth_sessions: HashMap<String, AuthSession>,
    /// Every token this emulator issued, keyed by the token string, so a later
    /// operation can resolve a token back to the stored user.
    pub issued_tokens: HashMap<String, TokenRecord>,
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The stored pool, or `None` when no pool carries that id.
    pub fn pool(&self, user_pool_id: &str) -> Option<&UserPool> {
        self.user_pools.get(user_pool_id)
    }

    /// The stored pool, mutably.
    pub fn pool_mut(&mut self, user_pool_id: &str) -> Option<&mut UserPool> {
        self.user_pools.get_mut(user_pool_id)
    }

    /// The stored pool that holds the app client `client_id`, or `None` when no pool
    /// does.
    ///
    /// The unauthenticated half of this API — `InitiateAuth`, `SignUp`,
    /// `ForgotPassword`, `RespondToAuthChallenge` — identifies its user pool by
    /// `ClientId` ALONE and models no `UserPoolId` member, so the stored app client is
    /// what names the pool. An id is minted per client and lives in exactly one pool,
    /// so at most one pool can answer.
    pub fn pool_of_client(&self, client_id: &str) -> Option<&UserPool> {
        self.user_pools
            .values()
            .find(|pool| pool.clients.contains_key(client_id))
    }

    /// The stored user pool domain `domain` names, prefix or custom, or `None`
    /// when no pool in this emulator holds it.
    ///
    /// `DescribeUserPoolDomain` identifies a domain by `Domain` ALONE and models
    /// no `UserPoolId` member, so one domain string names at most one pool: the
    /// lookup spans every pool rather than one.
    pub fn domain(&self, domain: &str) -> Option<&UserPoolDomain> {
        self.user_pools
            .values()
            .find_map(|pool| pool.domain_named(domain))
    }

    /// Store a challenge session and return its token.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — and the oldest are
    /// evicted if the store is still at [`MAX_AUTH_SESSIONS`], so repeating the
    /// challenge cannot grow this map without limit.
    pub fn start_session(&mut self, now: SystemTime, session: AuthSession) -> String {
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        evict_oldest_until(
            &mut self.auth_sessions,
            MAX_AUTH_SESSIONS.saturating_sub(1),
            |stored| stored.expires_at,
        );
        let token = new_session_token();
        self.auth_sessions.insert(token.clone(), session);
        token
    }

    /// Resolve the `Session` token a challenge response carries.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — which is what tells
    /// an expired session apart from one this emulator never handed out. The
    /// session stays stored: answering its challenge is what ends it
    /// ([`Self::end_session`]), so a caller whose response was refused can send
    /// a corrected one while the session still lives.
    pub fn session(&mut self, token: &str, now: SystemTime) -> SessionLookup {
        let expired = self
            .auth_sessions
            .get(token)
            .is_some_and(|stored| stored.expires_at <= now);
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        match self.auth_sessions.get(token) {
            Some(stored) => SessionLookup::Found(stored.clone()),
            None if expired => SessionLookup::Expired,
            None => SessionLookup::Unknown,
        }
    }

    /// Forget the challenge session `token` names: a session answers its
    /// challenge once.
    pub fn end_session(&mut self, token: &str) {
        self.auth_sessions.remove(token);
    }

    /// The token record for `token`, or `None` when this emulator never issued it.
    pub fn token(&self, token: &str) -> Option<&TokenRecord> {
        self.issued_tokens.get(token)
    }

    /// Issue the access / id / refresh token triple for `username` on
    /// `client_id`, record all three, and return the modelled result.
    ///
    /// Every claim comes from stored state: the `sub` and `username` from the
    /// stored user, its `cognito:groups` from the groups the stored user belongs
    /// to, `client_id` from the stored app client, the issuer from the stored
    /// pool, and each lifetime from the app client's validity settings. Each of
    /// the three is signed with the POOL's key pair, so a verifier can check it
    /// against the key set that pool publishes. `None` when the pool, the client
    /// or the user is gone, or when the signature could not be computed — never
    /// a token nothing signed.
    ///
    /// Tokens the clock has already expired are swept first, and the oldest are
    /// evicted if the store is still at [`MAX_ISSUED_TOKENS`], so repeated
    /// sign-ins cannot grow this map without limit.
    pub fn issue_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
    ) -> Option<AuthenticationResultType> {
        self.mint_tokens(user_pool_id, client_id, username, now, true)
    }

    /// Issue the access / id token pair for a sign-in that REUSES the refresh token the
    /// caller presented, record both, and return the modelled result.
    ///
    /// `InitiateAuth`'s `AuthFlow`: "REFRESH_TOKEN_AUTH and REFRESH_TOKEN — Receive new
    /// ID and access tokens when you pass a `REFRESH_TOKEN` parameter with a valid
    /// refresh token as the value" (this service's model). The refresh token in force
    /// stays the one the caller already holds, so none is minted and the result carries
    /// no `RefreshToken` member. Otherwise identical to [`issue_tokens`], whose claim
    /// building it shares.
    pub fn refresh_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
    ) -> Option<AuthenticationResultType> {
        self.mint_tokens(user_pool_id, client_id, username, now, false)
    }

    /// The one token-minting path behind [`issue_tokens`] and [`refresh_tokens`]:
    /// `with_refresh_token` decides whether a refresh token is minted beside the access
    /// and id tokens, so the two entry points cannot disagree about a claim.
    ///
    /// Every token it mints is signed with the POOL's key pair and names that key by
    /// `kid`, so a verifier can check it against the key set the pool publishes; a token
    /// whose signature could not be computed is not issued at all ([`signed_jws`]).
    fn mint_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        with_refresh_token: bool,
    ) -> Option<AuthenticationResultType> {
        let TokenInputs {
            issuer,
            sub,
            stored_username,
            email,
            groups,
            signing_key,
            access_ttl,
            id_ttl,
            refresh_ttl,
        } = {
            let pool = self.user_pools.get(user_pool_id)?;
            let client = pool.clients.get(client_id)?;
            let user = pool.user(username)?;
            TokenInputs {
                issuer: pool.issuer(),
                sub: user.sub.clone(),
                stored_username: user.username.clone(),
                email: user.attributes.get("email").cloned(),
                groups: pool.user_group_names(user),
                signing_key: pool.signing_key.clone(),
                access_ttl: client.access_token_ttl(),
                id_ttl: client.id_token_ttl(),
                refresh_ttl: client.refresh_token_ttl(),
            }
        };

        let issued_at = unix_seconds(now);
        let origin_jti = new_uuid();

        let mut access_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "client_id": client_id,
            "username": stored_username,
            "token_use": "access",
            "scope": "aws.cognito.signin.user.admin",
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": issued_at + access_ttl.as_secs() as i64,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        let mut id_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "aud": client_id,
            "cognito:username": stored_username,
            "token_use": "id",
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": issued_at + id_ttl.as_secs() as i64,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        if let (Some(email), Some(members)) = (email, id_claims.as_object_mut()) {
            members.insert("email".to_string(), serde_json::Value::String(email));
        }

        // `AdminAddUserToGroup`: "A user who is in a group ... populates a
        // `cognito:groups` claim to their access and identity tokens" (this
        // service's model), so the claim is carried by both of those two tokens
        // and by neither the refresh token. A user in no group carries no claim
        // rather than an empty list. The model documents no order for the list,
        // so it is rendered in the stored order, which is stable across reads.
        if !groups.is_empty() {
            let claim = serde_json::Value::Array(
                groups
                    .iter()
                    .map(|name| serde_json::Value::String(name.clone()))
                    .collect(),
            );
            for claims in [&mut access_claims, &mut id_claims] {
                if let Some(members) = claims.as_object_mut() {
                    members.insert("cognito:groups".to_string(), claim.clone());
                }
            }
        }

        let access_token = signed_jws(&access_claims, &signing_key)?;
        let id_token = signed_jws(&id_claims, &signing_key)?;
        // A refresh sign-in reuses the token the caller presented, so no refresh claims
        // are built on that path at all.
        let refresh_token = if with_refresh_token {
            let refresh_claims = serde_json::json!({
                "sub": sub,
                "iss": issuer,
                "client_id": client_id,
                "username": stored_username,
                "token_use": "refresh",
                "iat": issued_at,
                "exp": issued_at + refresh_ttl.as_secs() as i64,
                "jti": new_uuid(),
                "origin_jti": origin_jti,
            });
            Some(signed_jws(&refresh_claims, &signing_key)?)
        } else {
            None
        };

        let minted = 2 + usize::from(with_refresh_token);
        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(minted),
            |record| record.expires_at,
        );

        for (token, kind, ttl) in [
            (Some(&access_token), TokenKind::Access, access_ttl),
            (Some(&id_token), TokenKind::Id, id_ttl),
            (refresh_token.as_ref(), TokenKind::Refresh, refresh_ttl),
        ]
        .into_iter()
        .filter_map(|(token, kind, ttl)| token.map(|token| (token, kind, ttl)))
        {
            self.issued_tokens.insert(
                token.clone(),
                TokenRecord {
                    user_pool_id: user_pool_id.to_string(),
                    client_id: client_id.to_string(),
                    username: stored_username.clone(),
                    kind,
                    issued_at: now,
                    expires_at: now + ttl,
                    revoked: false,
                },
            );
        }

        Some(AuthenticationResultType {
            access_token: Some(access_token),
            // `ExpiresIn` is the access token's lifetime, in seconds.
            expires_in: i32::try_from(access_ttl.as_secs()).unwrap_or(i32::MAX),
            token_type: Some("Bearer".to_string()),
            refresh_token,
            id_token: Some(id_token),
            new_device_metadata: None,
        })
    }

    /// Forget every session and token issued for `username` in `user_pool_id`.
    /// Called when the user it authenticates is deleted.
    pub fn forget_user_credentials(&mut self, user_pool_id: &str, username: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.username == username));
        self.issued_tokens
            .retain(|_, t| !(t.user_pool_id == user_pool_id && t.username == username));
    }

    /// Forget every session and token issued in `user_pool_id`. Called when the
    /// pool they belong to is deleted.
    pub fn forget_pool_credentials(&mut self, user_pool_id: &str) {
        self.auth_sessions
            .retain(|_, s| s.user_pool_id != user_pool_id);
        self.issued_tokens
            .retain(|_, t| t.user_pool_id != user_pool_id);
    }
}

/// One stored user pool, with the app clients and users it owns.
#[derive(Debug, Clone)]
pub struct UserPool {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub region: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub policies: UserPoolPolicyType,
    pub lambda_config: Option<LambdaConfigType>,
    pub mfa_configuration: String,
    pub deletion_protection: String,
    pub user_pool_tier: String,
    pub username_attributes: Vec<String>,
    pub alias_attributes: Vec<String>,
    pub auto_verified_attributes: Vec<String>,
    pub schema: Vec<SchemaAttributeType>,
    pub admin_create_user_config: AdminCreateUserConfigType,
    pub username_configuration: Option<UsernameConfigurationType>,
    pub device_configuration: Option<DeviceConfigurationType>,
    pub email_configuration: Option<EmailConfigurationType>,
    pub sms_configuration: Option<SmsConfigurationType>,
    pub verification_message_template: Option<VerificationMessageTemplateType>,
    pub account_recovery_setting: Option<AccountRecoverySettingType>,
    pub user_attribute_update_settings: Option<UserAttributeUpdateSettingsType>,
    pub user_pool_add_ons: Option<UserPoolAddOnsType>,
    pub key_configuration: Option<KeyConfigurationType>,
    pub issuer_configuration: Option<IssuerConfigurationType>,
    pub acr_configuration: Option<HashMap<String, AcrLevelConfigType>>,
    pub sms_authentication_message: Option<String>,
    pub tags: HashMap<String, String>,
    /// This pool's Amazon Cognito prefix domain, once one is created:
    /// `UserPoolType.Domain` is "The domain prefix, if the user pool has a
    /// domain associated with it" — a single value, so a pool holds at most one.
    pub domain: Option<UserPoolDomain>,
    /// This pool's custom domain, once one is created: `UserPoolType.CustomDomain`
    /// is a single value too, so a pool holds at most one of those as well.
    pub custom_domain: Option<UserPoolDomain>,
    pub clients: HashMap<String, UserPoolClient>,
    /// Users keyed by [`UserPool::user_key`], so a pool configured
    /// case-insensitive treats `Alice` and `alice` as one user.
    pub users: HashMap<String, User>,
    /// This pool's groups keyed by `GroupName`, ordered so a read of several is
    /// stable. `GroupNameType` carries no case-insensitivity trait, so a name is
    /// matched exactly as it was created.
    pub groups: BTreeMap<String, Group>,
    /// This pool's OAuth 2.0 resource servers keyed by their `Identifier`,
    /// ordered so a read of several is stable. `ResourceServerIdentifierType`
    /// carries no case-insensitivity trait, so an identifier is matched exactly
    /// as it was created.
    pub resource_servers: BTreeMap<String, ResourceServer>,
    /// The key pair this pool's tokens are signed with and whose public half
    /// its key set publishes, minted with the pool and kept for its lifetime,
    /// so every token the pool ever issued stays verifiable against the key set
    /// it publishes now.
    pub signing_key: PoolSigningKey,
}

impl UserPool {
    /// The token issuer for this pool: the regional user-pool endpoint.
    pub fn issuer(&self) -> String {
        format!(
            "https://{SIGNING_NAME}.{}.amazonaws.com/{}",
            self.region, self.id
        )
    }

    /// Where this pool publishes the key set its tokens are verified against:
    /// `<issuer>/.well-known/jwks.json`, the location the token's `iss` claim
    /// leads a verifier to (Amazon Cognito Developer Guide, "Verifying a JSON
    /// web token").
    pub fn jwks_uri(&self) -> String {
        format!("{}/.well-known/jwks.json", self.issuer())
    }

    /// This pool's token-signing key pair, minted when the pool was created.
    pub fn signing_key(&self) -> &PoolSigningKey {
        &self.signing_key
    }

    /// This pool's JSON Web Key Set (RFC 7517 §5): the public half of every key
    /// its tokens are signed with, each entry found by the `kid` the token
    /// header carries. This is the document a verifier reads from
    /// [`UserPool::jwks_uri`], and it holds only PUBLIC key parameters.
    ///
    /// A pure read: it renders the stored key and changes nothing.
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({ "keys": [self.signing_key.to_jwk()] })
    }

    /// This pool with a freshly minted id, and the ARN that id implies. Used
    /// when a minted id turns out to name a pool that already exists.
    #[must_use]
    pub fn with_new_id(mut self, region: &str) -> Self {
        self.id = new_pool_id(region);
        self.arn = ::engine::emulator_arn(SIGNING_NAME, region, &format!("userpool/{}", self.id));
        self
    }

    /// Whether usernames in this pool are compared case-sensitively.
    ///
    /// `UsernameConfiguration.CaseSensitive` is the pool's setting; a pool that
    /// sets none compares case-sensitively.
    pub fn case_sensitive(&self) -> bool {
        self.username_configuration
            .as_ref()
            .is_none_or(|configuration| configuration.case_sensitive)
    }

    /// The key `username` is stored under in [`UserPool::users`].
    pub fn user_key(&self, username: &str) -> String {
        if self.case_sensitive() {
            username.to_string()
        } else {
            username.to_lowercase()
        }
    }

    /// The stored user `username` names, by username or by `sub`.
    pub fn user(&self, username: &str) -> Option<&User> {
        self.users
            .get(&self.user_key(username))
            .or_else(|| self.users.values().find(|user| user.sub == username))
    }

    /// The key the stored user `username` names is held under, or `None` when no
    /// such user exists. Resolves a `sub` the same way [`UserPool::user`] does.
    pub fn user_storage_key(&self, username: &str) -> Option<String> {
        let key = self.user_key(username);
        if self.users.contains_key(&key) {
            return Some(key);
        }
        self.users
            .iter()
            .find(|(_, user)| user.sub == username)
            .map(|(stored_key, _)| stored_key.clone())
    }

    /// The names of this pool's groups `user` belongs to, in stored (name) order.
    ///
    /// Only groups the pool still HOLDS are reported: `DeleteGroup` documents that
    /// a deleted group "no longer contributes to users' `cognito:preferred_group`
    /// or `cognito:groups` claims", so a membership of a group that is gone is not
    /// rendered.
    pub fn user_group_names(&self, user: &User) -> Vec<String> {
        user.groups
            .iter()
            .filter(|name| self.groups.contains_key(*name))
            .cloned()
            .collect()
    }

    /// The stored user `username` names, mutably.
    pub fn user_mut(&mut self, username: &str) -> Option<&mut User> {
        let key = self.user_storage_key(username)?;
        self.users.get_mut(&key)
    }

    /// The effective password policy: the pool's own, with the documented
    /// defaults filled in for what it leaves unset.
    pub fn password_policy(&self) -> PasswordPolicyType {
        let mut policy = self
            .policies
            .password_policy
            .clone()
            .unwrap_or_else(default_password_policy);
        if policy.minimum_length.is_none() {
            policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
        }
        if policy.temporary_password_validity_days == 0 {
            policy.temporary_password_validity_days = DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
        }
        policy
    }

    /// `None` when `password` satisfies this pool's password policy, else the
    /// requirement it misses — the message a client has to fix the request from.
    ///
    /// The message is the fixed clause for the requirement missed, behind the
    /// fixed policy-rejection prefix: it reports WHICH requirement the password
    /// failed and never the value this pool configured for it.
    pub fn check_password(&self, password: &str) -> Option<String> {
        let policy = self.password_policy();
        let minimum = policy.minimum_length.unwrap_or(DEFAULT_PASSWORD_MIN_LENGTH);
        if i64::try_from(password.chars().count()).unwrap_or(i64::MAX) < i64::from(minimum) {
            return Some(password_policy_message(PASSWORD_TOO_SHORT));
        }
        if policy.require_uppercase && !password.chars().any(char::is_uppercase) {
            return Some(password_policy_message(PASSWORD_NEEDS_UPPERCASE));
        }
        if policy.require_lowercase && !password.chars().any(char::is_lowercase) {
            return Some(password_policy_message(PASSWORD_NEEDS_LOWERCASE));
        }
        if policy.require_numbers && !password.chars().any(|c| c.is_ascii_digit()) {
            return Some(password_policy_message(PASSWORD_NEEDS_NUMBERS));
        }
        if policy.require_symbols && !password.chars().any(is_symbol) {
            return Some(password_policy_message(PASSWORD_NEEDS_SYMBOLS));
        }
        None
    }

    /// How long a temporary password this pool issues stays usable.
    pub fn temporary_password_validity(&self) -> Duration {
        let days = self.password_policy().temporary_password_validity_days;
        Duration::from_secs(u64::try_from(days).unwrap_or(0) * 24 * 60 * 60)
    }

    /// `None` when every name in `names` is an attribute this pool's schema
    /// declares, else the first one that is not: an attribute a pool does not
    /// declare cannot hold a value.
    pub fn unknown_attribute(&self, names: &[String]) -> Option<String> {
        names
            .iter()
            .find(|name| {
                !is_standard_attribute(name)
                    && !self
                        .schema
                        .iter()
                        .any(|entry| entry.name.as_deref() == Some(name.as_str()))
            })
            .cloned()
    }

    /// Whether this pool's schema lets `name`'s stored value be CHANGED:
    /// `SchemaAttributeType.Mutable` — "Specifies whether the value of the
    /// attribute can be changed" (this service's model). An attribute the
    /// schema does not declare has no entry to read, so it is reported
    /// unchangeable.
    pub fn attribute_mutable(&self, name: &str) -> bool {
        self.schema
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
            .and_then(|entry| entry.mutable)
            .unwrap_or(false)
    }

    /// Whether this pool's schema marks `name` a required attribute.
    pub fn attribute_required(&self, name: &str) -> bool {
        self.schema
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
            .and_then(|entry| entry.required)
            .unwrap_or(false)
    }

    /// The names of the attributes this pool's schema marks required that `user`
    /// has no value for: what a `NEW_PASSWORD_REQUIRED` challenge asks the user
    /// to supply. `sub` is never among them — the pool mints it.
    pub fn missing_required_attributes(&self, user: &User) -> Vec<String> {
        self.unset_required_attributes(&user.attributes)
    }

    /// The names of the attributes this pool's schema marks required that
    /// `attributes` holds no value for — the same rule as
    /// [`UserPool::missing_required_attributes`], asked of an attribute set that
    /// is not stored yet.
    pub fn unset_required_attributes(&self, attributes: &BTreeMap<String, String>) -> Vec<String> {
        self.schema
            .iter()
            .filter(|entry| entry.required == Some(true))
            .filter_map(|entry| entry.name.clone())
            .filter(|name| name != "sub" && !attributes.contains_key(name))
            .collect()
    }

    /// This pool's domain under the name `domain` — its prefix domain or its
    /// custom one — or `None` when this pool holds no domain of that name.
    pub fn domain_named(&self, domain: &str) -> Option<&UserPoolDomain> {
        [self.domain.as_ref(), self.custom_domain.as_ref()]
            .into_iter()
            .flatten()
            .find(|stored| stored.domain == domain)
    }

    /// The modelled view of this pool, rendered from what is stored.
    pub fn to_user_pool_type(&self) -> UserPoolType {
        UserPoolType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            policies: Some(self.policies.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            lambda_config: self.lambda_config.clone(),
            // `UserPoolType.Status` is `@deprecated` ("This property is no longer
            // available."), so it is left out rather than invented.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            schema_attributes: Some(self.schema.clone()),
            auto_verified_attributes: non_empty(&self.auto_verified_attributes),
            alias_attributes: non_empty(&self.alias_attributes),
            username_attributes: non_empty(&self.username_attributes),
            sms_verification_message: None,
            email_verification_message: None,
            email_verification_subject: None,
            verification_message_template: self.verification_message_template.clone(),
            sms_authentication_message: self.sms_authentication_message.clone(),
            user_attribute_update_settings: self.user_attribute_update_settings.clone(),
            mfa_configuration: Some(self.mfa_configuration.clone()),
            device_configuration: self.device_configuration.clone(),
            // Computed from what the pool holds, never from a request.
            estimated_number_of_users: i32::try_from(self.users.len()).unwrap_or(i32::MAX),
            email_configuration: self.email_configuration.clone(),
            sms_configuration: self.sms_configuration.clone(),
            user_pool_tags: Some(self.tags.clone()),
            sms_configuration_failure: None,
            email_configuration_failure: None,
            domain: self.domain.as_ref().map(|domain| domain.domain.clone()),
            custom_domain: self.custom_domain.as_ref().map(|domain| domain.domain.clone()),
            admin_create_user_config: Some(self.admin_create_user_config.clone()),
            user_pool_add_ons: self.user_pool_add_ons.clone(),
            username_configuration: self.username_configuration.clone(),
            arn: Some(self.arn.clone()),
            account_recovery_setting: self.account_recovery_setting.clone(),
            user_pool_tier: Some(self.user_pool_tier.clone()),
            key_configuration: self.key_configuration.clone(),
            issuer_configuration: self.issuer_configuration.clone(),
            acr_configuration: self.acr_configuration.clone(),
        }
    }
}

/// One stored user pool domain: what `CreateUserPoolDomain` persisted and every
/// later read of that domain renders.
///
/// A domain is a PREFIX domain when it carries no `CustomDomainConfig` and a
/// CUSTOM domain when it does (`CreateUserPoolDomain.CustomDomainConfig`:
/// "Provide this parameter only if you want to use a custom domain for your user
/// pool. Otherwise, you can omit this parameter and use a prefix domain
/// instead.").
#[derive(Debug, Clone)]
pub struct UserPoolDomain {
    /// The domain string the request named, which is this resource's key.
    pub domain: String,
    /// The pool the domain is attached to.
    pub user_pool_id: String,
    /// The custom-domain configuration as submitted; `None` for a prefix domain.
    pub custom_domain_config: Option<CustomDomainConfigType>,
    /// The managed login branding version in force for this domain.
    pub managed_login_version: i32,
    /// The routing configuration as submitted, which only a custom domain may
    /// carry.
    pub routing: Option<RoutingType>,
    /// The CloudFront endpoint that hosts a CUSTOM domain, minted with it;
    /// `None` for a prefix domain, for which `CreateUserPoolDomain` documents
    /// "If you set an Amazon Cognito prefix domain, this parameter returns
    /// null."
    pub cloud_front_distribution: Option<String>,
    /// When the domain was created.
    pub creation_date: SystemTime,
    /// The instant the clock owes this domain's `CREATING` -> `ACTIVE` move.
    /// Stored, never slept on: a read computes the status from it.
    pub active_at: SystemTime,
}

impl UserPoolDomain {
    /// A new domain for `user_pool_id`, `CREATING` until the clock makes its
    /// activation due one [`DOMAIN_ACTIVATION_SPAN`] from `now`.
    pub fn new(
        domain: String,
        user_pool_id: String,
        custom_domain_config: Option<CustomDomainConfigType>,
        managed_login_version: i32,
        routing: Option<RoutingType>,
        now: SystemTime,
    ) -> Self {
        // A custom domain is served through CloudFront, and the caller has to
        // point an alias record at that endpoint, so one is minted with the
        // domain; a prefix domain has none.
        let cloud_front_distribution = custom_domain_config
            .is_some()
            .then(new_cloud_front_domain);
        Self {
            domain,
            user_pool_id,
            custom_domain_config,
            managed_login_version,
            routing,
            cloud_front_distribution,
            creation_date: now,
            active_at: now + DOMAIN_ACTIVATION_SPAN,
        }
    }

    /// Whether this is a custom domain rather than an Amazon Cognito prefix one.
    pub fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }

    /// This domain's `DomainStatusType` as of `now`: `CREATING` until the clock
    /// has made its activation due, `ACTIVE` from then on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            DOMAIN_STATUS_ACTIVE
        } else {
            DOMAIN_STATUS_CREATING
        }
    }

    /// The modelled view of this domain, rendered from what is stored.
    pub fn to_domain_description(&self, now: SystemTime) -> DomainDescriptionType {
        DomainDescriptionType {
            user_pool_id: Some(self.user_pool_id.clone()),
            aws_account_id: Some(::engine::emulator_account_id().to_string()),
            domain: Some(self.domain.clone()),
            // `S3Bucket` is "The Amazon S3 bucket where the static files for
            // this domain are stored" and `Version` "The app version": both
            // name AWS-internal hosted-UI infrastructure this emulator does not
            // deploy, so neither is invented.
            s3_bucket: None,
            cloud_front_distribution: self.cloud_front_distribution.clone(),
            version: None,
            status: Some(self.status(now).to_string()),
            custom_domain_config: self.custom_domain_config.clone(),
            managed_login_version: Some(self.managed_login_version),
            routing: self.routing.clone(),
        }
    }
}

/// One stored app client of a user pool.
#[derive(Debug, Clone)]
pub struct UserPoolClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub user_pool_id: String,
    pub client_name: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub refresh_token_validity: i32,
    pub access_token_validity: Option<i32>,
    pub id_token_validity: Option<i32>,
    pub token_validity_units: Option<TokenValidityUnitsType>,
    pub read_attributes: Option<Vec<String>>,
    pub write_attributes: Option<Vec<String>>,
    pub explicit_auth_flows: Vec<String>,
    pub supported_identity_providers: Option<Vec<String>>,
    pub callback_urls: Option<Vec<String>>,
    pub logout_urls: Option<Vec<String>>,
    pub default_redirect_uri: Option<String>,
    pub allowed_oauth_flows: Option<Vec<String>>,
    pub allowed_oauth_scopes: Option<Vec<String>>,
    pub allowed_oauth_flows_user_pool_client: bool,
    pub analytics_configuration: Option<AnalyticsConfigurationType>,
    pub prevent_user_existence_errors: String,
    pub enable_token_revocation: bool,
    pub enable_propagate_additional_user_context_data: bool,
    pub auth_session_validity: Option<i32>,
    pub refresh_token_rotation: Option<RefreshTokenRotationType>,
}

impl UserPoolClient {
    /// Whether `AuthFlow=ADMIN_USER_PASSWORD_AUTH` is enabled for this client.
    pub fn allows_admin_password_auth(&self) -> bool {
        self.allows_any_of(&ADMIN_PASSWORD_AUTH_FLOWS)
    }

    /// Whether `AuthFlow=USER_PASSWORD_AUTH` is enabled for this client.
    pub fn allows_user_password_auth(&self) -> bool {
        self.allows_any_of(&USER_PASSWORD_AUTH_FLOWS)
    }

    /// Whether `AuthFlow=REFRESH_TOKEN_AUTH` / `REFRESH_TOKEN` is enabled for this
    /// client.
    pub fn allows_refresh_token_auth(&self) -> bool {
        self.allows_any_of(&REFRESH_TOKEN_AUTH_FLOWS)
    }

    /// Whether this client's stored `ExplicitAuthFlows` carry any of `flows`.
    fn allows_any_of(&self, flows: &[&str]) -> bool {
        self.explicit_auth_flows
            .iter()
            .any(|flow| flows.contains(&flow.as_str()))
    }

    /// Whether this client suppresses "no such user": `PreventUserExistenceErrors
    /// = ENABLED` answers a generic authentication error instead.
    pub fn hides_user_existence(&self) -> bool {
        self.prevent_user_existence_errors == "ENABLED"
    }

    /// The access token lifetime this client issues.
    pub fn access_token_ttl(&self) -> Duration {
        token_ttl(
            self.access_token_validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.access_token.as_deref()),
            // "The default time unit for AccessTokenValidity in an API request is hours."
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The id token lifetime this client issues.
    pub fn id_token_ttl(&self) -> Duration {
        token_ttl(
            self.id_token_validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.id_token.as_deref()),
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The refresh token lifetime this client issues. A stored `0` is the
    /// model's "Amazon Cognito overrides the value with the default value of 30
    /// days".
    pub fn refresh_token_ttl(&self) -> Duration {
        let validity = (self.refresh_token_validity != 0).then_some(self.refresh_token_validity);
        token_ttl(
            validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.refresh_token.as_deref()),
            // "The default time unit for RefreshTokenValidity in an API request is days."
            TokenValidityUnit::Days,
            DEFAULT_REFRESH_TOKEN_TTL,
        )
    }

    /// How long a challenge session this client starts stays valid.
    pub fn auth_session_validity(&self) -> Duration {
        let minutes = self
            .auth_session_validity
            .and_then(|minutes| u64::try_from(minutes).ok())
            .unwrap_or(DEFAULT_AUTH_SESSION_VALIDITY_MINUTES);
        Duration::from_secs(minutes * 60)
    }

    /// The modelled view of this client, rendered from what is stored.
    pub fn to_user_pool_client_type(&self) -> UserPoolClientType {
        UserPoolClientType {
            user_pool_id: Some(self.user_pool_id.clone()),
            client_name: Some(self.client_name.clone()),
            client_id: Some(self.client_id.clone()),
            client_secret: self.client_secret.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            refresh_token_validity: self.refresh_token_validity,
            access_token_validity: self.access_token_validity,
            id_token_validity: self.id_token_validity,
            token_validity_units: self.token_validity_units.clone(),
            read_attributes: self.read_attributes.clone(),
            write_attributes: self.write_attributes.clone(),
            explicit_auth_flows: Some(self.explicit_auth_flows.clone()),
            supported_identity_providers: self.supported_identity_providers.clone(),
            callback_ur_ls: self.callback_urls.clone(),
            logout_ur_ls: self.logout_urls.clone(),
            default_redirect_uri: self.default_redirect_uri.clone(),
            allowed_o_auth_flows: self.allowed_oauth_flows.clone(),
            allowed_o_auth_scopes: self.allowed_oauth_scopes.clone(),
            allowed_o_auth_flows_user_pool_client: Some(self.allowed_oauth_flows_user_pool_client),
            analytics_configuration: self.analytics_configuration.clone(),
            prevent_user_existence_errors: Some(self.prevent_user_existence_errors.clone()),
            enable_token_revocation: Some(self.enable_token_revocation),
            enable_propagate_additional_user_context_data: Some(
                self.enable_propagate_additional_user_context_data,
            ),
            auth_session_validity: self.auth_session_validity,
            refresh_token_rotation: self.refresh_token_rotation.clone(),
        }
    }
}

/// A user's confirmation state. The value set is `UserStatusType` from this
/// service's model (`smithy_query(query="shape",shape="UserStatusType")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Unconfirmed,
    Confirmed,
    Archived,
    Compromised,
    Unknown,
    ResetRequired,
    ForceChangePassword,
    ExternalProvider,
}

impl UserStatus {
    /// The wire value of this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Confirmed => "CONFIRMED",
            Self::Archived => "ARCHIVED",
            Self::Compromised => "COMPROMISED",
            Self::Unknown => "UNKNOWN",
            Self::ResetRequired => "RESET_REQUIRED",
            Self::ForceChangePassword => "FORCE_CHANGE_PASSWORD",
            Self::ExternalProvider => "EXTERNAL_PROVIDER",
        }
    }
}

/// One stored user of a user pool.
#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    /// The user's immutable id, also its `sub` attribute.
    pub sub: String,
    /// Attributes keyed by name, `sub` included, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    pub password: Option<String>,
    /// Whether [`User::password`] is permanent; a temporary one must be changed
    /// at the next sign-in and expires with the pool's policy.
    pub password_permanent: bool,
    pub password_set_at: Option<SystemTime>,
    pub status: UserStatus,
    pub enabled: bool,
    pub mfa_options: Vec<MFAOptionType>,
    pub preferred_mfa_setting: Option<String>,
    pub mfa_setting_list: Vec<String>,
    pub user_create_date: SystemTime,
    pub user_last_modified_date: SystemTime,
    pub groups: BTreeSet<String>,
}

impl User {
    /// The stored attributes as the modelled list, in name order.
    pub fn attribute_list(&self) -> Vec<AttributeType> {
        self.attributes
            .iter()
            .map(|(name, value)| AttributeType {
                name: name.clone(),
                value: Some(value.clone()),
            })
            .collect()
    }

    /// The modelled view of this user, rendered from what is stored.
    pub fn to_user_type(&self) -> UserType {
        UserType {
            username: Some(self.username.clone()),
            attributes: Some(self.attribute_list()),
            user_create_date: Some(to_date_time(self.user_create_date)),
            user_last_modified_date: Some(to_date_time(self.user_last_modified_date)),
            enabled: self.enabled,
            user_status: Some(self.status.as_str().to_string()),
            mfa_options: non_empty(&self.mfa_options),
        }
    }

    /// Whether the stored password is a temporary one that has outlived
    /// `validity` as of `now`.
    pub fn temporary_password_expired(&self, validity: Duration, now: SystemTime) -> bool {
        if self.password_permanent {
            return false;
        }
        match self.password_set_at {
            Some(set_at) => now > set_at + validity,
            None => false,
        }
    }
}

/// One stored group of a user pool: what `CreateGroup` stored and every later
/// read of that group renders.
#[derive(Debug, Clone)]
pub struct Group {
    pub group_name: String,
    pub user_pool_id: String,
    pub description: Option<String>,
    pub role_arn: Option<String>,
    pub precedence: Option<i32>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Group {
    /// The modelled view of this group, rendered from what is stored. An
    /// optional member the creating request left out stays absent rather than
    /// being invented.
    pub fn to_group_type(&self) -> GroupType {
        GroupType {
            group_name: Some(self.group_name.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            description: self.description.clone(),
            role_arn: self.role_arn.clone(),
            precedence: self.precedence,
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// One stored OAuth 2.0 resource server of a user pool: what
/// `CreateResourceServer` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct ResourceServer {
    pub user_pool_id: String,
    pub identifier: String,
    pub name: String,
    /// The custom scopes of this resource server, in the order they were
    /// submitted. A resource server created with no `Scopes` holds none.
    pub scopes: Vec<ResourceServerScopeType>,
}

impl ResourceServer {
    /// The modelled view of this resource server, rendered from what is stored.
    pub fn to_resource_server_type(&self) -> ResourceServerType {
        ResourceServerType {
            user_pool_id: Some(self.user_pool_id.clone()),
            identifier: Some(self.identifier.clone()),
            name: Some(self.name.clone()),
            scopes: Some(self.scopes.clone()),
        }
    }
}

/// A challenge session: what a `RespondToAuthChallenge` for this session resumes.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub user_pool_id: String,
    pub client_id: String,
    pub username: String,
    pub challenge_name: String,
    pub expires_at: SystemTime,
}

/// What resolving a `Session` token found
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::session`]).
/// An expired session is told apart from an unknown one because a challenge
/// response owes a different refusal for each.
#[derive(Debug, Clone)]
pub enum SessionLookup {
    /// The stored session, still inside its validity window.
    Found(AuthSession),
    /// A stored session whose validity window had already closed.
    Expired,
    /// No session of that token: this emulator never handed it out.
    Unknown,
}

/// Which token of the triple a [`TokenRecord`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Id,
    Refresh,
}

/// One issued token, so a later operation can resolve it back to its user.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub user_pool_id: String,
    pub client_id: String,
    pub username: String,
    pub kind: TokenKind,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub revoked: bool,
}

/// The time unit a token-validity number is counted in
/// (`TokenValidityUnitsType`: `seconds`, `minutes`, `hours`, `days`).
#[derive(Debug, Clone, Copy)]
enum TokenValidityUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TokenValidityUnit {
    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "seconds" => Some(Self::Seconds),
            "minutes" => Some(Self::Minutes),
            "hours" => Some(Self::Hours),
            "days" => Some(Self::Days),
            _ => None,
        }
    }

    fn seconds(self) -> u64 {
        match self {
            Self::Seconds => 1,
            Self::Minutes => 60,
            Self::Hours => 60 * 60,
            Self::Days => 24 * 60 * 60,
        }
    }
}

/// `validity` counted in `unit` (or `default_unit` when the client set none), or
/// `default_ttl` when the client set no validity at all.
fn token_ttl(
    validity: Option<i32>,
    unit: Option<&str>,
    default_unit: TokenValidityUnit,
    default_ttl: Duration,
) -> Duration {
    let Some(validity) = validity.and_then(|validity| u64::try_from(validity).ok()) else {
        return default_ttl;
    };
    let unit = unit
        .and_then(TokenValidityUnit::parse)
        .unwrap_or(default_unit);
    Duration::from_secs(validity.saturating_mul(unit.seconds()))
}

/// Drop entries from `map`, the one `expiry` puts first soonest, until `map`
/// holds at most `keep`.
fn evict_oldest_until<V, F: Fn(&V) -> SystemTime>(
    map: &mut HashMap<String, V>,
    keep: usize,
    expiry: F,
) {
    while map.len() > keep {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| expiry(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// Amazon Cognito's default password policy for a new pool.
pub fn default_password_policy() -> PasswordPolicyType {
    PasswordPolicyType {
        minimum_length: Some(DEFAULT_PASSWORD_MIN_LENGTH),
        require_uppercase: true,
        require_lowercase: true,
        require_numbers: true,
        require_symbols: true,
        password_history_size: None,
        temporary_password_validity_days: DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS,
    }
}

/// The pool policies a request leaves unset, filled in with the documented
/// defaults. A policy the request DOES carry is stored as sent.
pub fn effective_policies(requested: Option<UserPoolPolicyType>) -> UserPoolPolicyType {
    let mut policies = requested.unwrap_or_default();
    let mut password_policy = policies
        .password_policy
        .unwrap_or_else(default_password_policy);
    if password_policy.minimum_length.is_none() {
        password_policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
    }
    if password_policy.temporary_password_validity_days == 0 {
        password_policy.temporary_password_validity_days =
            DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
    }
    policies.password_policy = Some(password_policy);
    if policies.sign_in_policy.is_none() {
        policies.sign_in_policy = Some(SignInPolicyType {
            // Password is the first auth factor of a pool that configures none.
            allowed_first_auth_factors: Some(vec!["PASSWORD".to_string()]),
        });
    }
    policies
}

/// The `AdminCreateUserConfig` a request leaves unset, with the model's
/// documented defaults filled in.
pub fn effective_admin_create_user_config(
    requested: Option<AdminCreateUserConfigType>,
) -> AdminCreateUserConfigType {
    let mut config = requested.unwrap_or_default();
    if config.unused_account_validity_days == 0 {
        config.unused_account_validity_days = DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS;
    }
    config
}

/// The feature plan a request leaves unset: "Defaults to `ESSENTIALS`".
pub fn effective_user_pool_tier(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_USER_POOL_TIER.to_string())
}

/// The MFA mode a request leaves unset: MFA is `OFF`.
pub fn effective_mfa_configuration(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_MFA_CONFIGURATION.to_string())
}

/// The deletion-protection mode a request leaves unset: `INACTIVE`.
pub fn effective_deletion_protection(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_DELETION_PROTECTION.to_string())
}

/// `PreventUserExistenceErrors` a request leaves unset: "Defaults to `LEGACY`".
pub fn effective_prevent_user_existence_errors(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_PREVENT_USER_EXISTENCE_ERRORS.to_string())
}

/// Amazon Cognito's standard (OIDC) user-pool attributes, the schema every new
/// pool starts with: each entry is `(name, AttributeDataType, mutable,
/// required)`. The names are the standard attributes of the Amazon Cognito
/// Developer Guide's "User pool attributes"; the fields are
/// `SchemaAttributeType` (`smithy_query(query="shape",shape="SchemaAttributeType")`).
const STANDARD_SCHEMA: [(&str, &str, bool, bool); 20] = [
    ("sub", "String", false, true),
    ("name", "String", true, false),
    ("given_name", "String", true, false),
    ("family_name", "String", true, false),
    ("middle_name", "String", true, false),
    ("nickname", "String", true, false),
    ("preferred_username", "String", true, false),
    ("profile", "String", true, false),
    ("picture", "String", true, false),
    ("website", "String", true, false),
    ("email", "String", true, false),
    ("email_verified", "Boolean", true, false),
    ("gender", "String", true, false),
    ("birthdate", "String", true, false),
    ("zoneinfo", "String", true, false),
    ("locale", "String", true, false),
    ("phone_number", "String", true, false),
    ("phone_number_verified", "Boolean", true, false),
    ("address", "String", true, false),
    ("updated_at", "Number", true, false),
];

/// Whether `name` is one of the standard attributes every pool carries.
pub fn is_standard_attribute(name: &str) -> bool {
    STANDARD_SCHEMA.iter().any(|(standard, ..)| *standard == name)
}

/// The schema a new pool is created with: the standard attributes, with a
/// requested attribute of the same name overriding its properties, and every
/// other requested attribute added under the `custom:` prefix Amazon Cognito
/// gives custom attributes.
pub fn effective_schema(requested: Option<Vec<SchemaAttributeType>>) -> Vec<SchemaAttributeType> {
    let requested = requested.unwrap_or_default();
    let mut schema: Vec<SchemaAttributeType> = STANDARD_SCHEMA
        .iter()
        .map(|(name, data_type, mutable, required)| SchemaAttributeType {
            name: Some((*name).to_string()),
            attribute_data_type: Some((*data_type).to_string()),
            developer_only_attribute: Some(false),
            mutable: Some(*mutable),
            required: Some(*required),
            number_attribute_constraints: (*data_type == "Number").then_some(
                NumberAttributeConstraintsType {
                    min_value: None,
                    max_value: None,
                },
            ),
            string_attribute_constraints: (*data_type == "String").then(|| {
                StringAttributeConstraintsType {
                    min_length: Some("0".to_string()),
                    max_length: Some("2048".to_string()),
                }
            }),
        })
        .collect();

    for attribute in requested {
        let Some(name) = attribute.name.clone() else {
            continue;
        };
        if let Some(standard) = schema
            .iter_mut()
            .find(|entry| entry.name.as_deref() == Some(name.as_str()))
        {
            standard.required = attribute.required.or(standard.required);
            standard.mutable = attribute.mutable.or(standard.mutable);
            if attribute.string_attribute_constraints.is_some() {
                standard.string_attribute_constraints = attribute.string_attribute_constraints;
            }
            if attribute.number_attribute_constraints.is_some() {
                standard.number_attribute_constraints = attribute.number_attribute_constraints;
            }
            continue;
        }
        let developer_only = attribute.developer_only_attribute.unwrap_or(false);
        let prefix = if developer_only { "dev:" } else { "custom:" };
        let stored_name = if name.starts_with("custom:") || name.starts_with("dev:") {
            name
        } else {
            format!("{prefix}{name}")
        };
        schema.push(SchemaAttributeType {
            name: Some(stored_name),
            attribute_data_type: Some(
                attribute
                    .attribute_data_type
                    .unwrap_or_else(|| "String".to_string()),
            ),
            developer_only_attribute: Some(developer_only),
            // A custom attribute is mutable unless the request says otherwise;
            // a custom attribute can never be required.
            mutable: Some(attribute.mutable.unwrap_or(true)),
            required: Some(false),
            number_attribute_constraints: attribute.number_attribute_constraints,
            string_attribute_constraints: attribute.string_attribute_constraints,
        });
    }
    schema
}

/// A new user pool with `name` in `region`, with every configuration default
/// filled in. The caller stores it under [`UserPool::id`].
pub fn new_user_pool(name: String, region: &str, now: SystemTime) -> UserPool {
    let region = sanitize_region(region);
    let id = new_pool_id(&region);
    let arn = ::engine::emulator_arn(SIGNING_NAME, &region, &format!("userpool/{id}"));
    UserPool {
        id,
        name,
        arn,
        region,
        creation_date: now,
        last_modified_date: now,
        policies: effective_policies(None),
        lambda_config: None,
        mfa_configuration: effective_mfa_configuration(None),
        deletion_protection: effective_deletion_protection(None),
        user_pool_tier: DEFAULT_USER_POOL_TIER.to_string(),
        username_attributes: Vec::new(),
        alias_attributes: Vec::new(),
        auto_verified_attributes: Vec::new(),
        schema: effective_schema(None),
        admin_create_user_config: effective_admin_create_user_config(None),
        username_configuration: None,
        device_configuration: None,
        email_configuration: None,
        sms_configuration: None,
        verification_message_template: None,
        account_recovery_setting: None,
        user_attribute_update_settings: None,
        user_pool_add_ons: None,
        key_configuration: None,
        issuer_configuration: None,
        acr_configuration: None,
        sms_authentication_message: None,
        tags: HashMap::new(),
        domain: None,
        custom_domain: None,
        clients: HashMap::new(),
        users: HashMap::new(),
        groups: BTreeMap::new(),
        resource_servers: BTreeMap::new(),
        // A pool's token-signing key exists from the moment the pool does, as
        // on AWS, so its key set is publishable before anything signs in. One
        // scalar multiplication, so creating a pool waits on nothing.
        signing_key: PoolSigningKey::generate(),
    }
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. A pool's id and ARN name the region it was created
/// in, so they are derived from the request's scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| sanitize_region(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `region`, reduced to the characters a `UserPoolId` may carry
/// (`^[\w-]+_[0-9a-zA-Z]+$`), falling back to [`DEFAULT_REGION`].
fn sanitize_region(region: &str) -> String {
    let cleaned: String = region
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        cleaned
    }
}

/// A fresh `UserPoolId`: `<region>_<9 random characters>`, which satisfies the
/// modelled pattern `^[\w-]+_[0-9a-zA-Z]+$` and length 1..55.
fn new_pool_id(region: &str) -> String {
    format!("{region}_{}", random_token(9))
}

/// A fresh CloudFront endpoint for a custom domain: `<distribution>.cloudfront.net`,
/// the form AWS reports as "The fully-qualified domain name (FQDN) of the Amazon
/// CloudFront distribution" (`CreateUserPoolDomain.CloudFrontDomain`) and the
/// value a caller's alias record has to point at. A CloudFront distribution
/// domain is `d` followed by 13 lowercase alphanumerics.
fn new_cloud_front_domain() -> String {
    format!("d{}.cloudfront.net", random_token(13))
}

/// A fresh `ClientId`: 26 characters, satisfying the modelled `^[\w+]+$`.
pub fn new_client_id() -> String {
    random_token(26)
}

/// A fresh `ClientSecret`: 51 characters, inside the modelled length 24..64 and
/// satisfying `^[\w+]+$`.
pub fn new_client_secret() -> String {
    random_token(51)
}

/// A fresh user `sub`: a v4 UUID, as Amazon Cognito's `sub` attribute is.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()`
/// rendered canonically — `vendor/emulator-telemetry/src/types.rs`), which this
/// crate already depends on: nothing here is a hand-rolled generator and this
/// crate declares no second path to the same crate.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A fresh `Session` token: 64 characters, inside the modelled length 20..4096.
fn new_session_token() -> String {
    random_token(64)
}

/// `len` random hex characters, drawn from v4 UUIDs (122 bits of entropy each)
/// with their hyphens removed, so the result satisfies `^[\w+]+$`.
fn random_token(len: usize) -> String {
    let mut token = String::with_capacity(len + 32);
    while token.len() < len {
        token.extend(new_uuid().chars().filter(|c| *c != '-'));
    }
    token.truncate(len);
    token
}

/// What one sign-in's tokens are built from, read from the stored pool, app
/// client and user under the state lock before any token is minted.
struct TokenInputs {
    /// The pool's token issuer (its regional user-pool endpoint).
    issuer: String,
    /// The stored user's immutable id, the `sub` claim.
    sub: String,
    /// The username as STORED, which a case-insensitive pool may case
    /// differently from the request.
    stored_username: String,
    /// The stored `email` attribute, when the user has one.
    email: Option<String>,
    /// The names of the pool's groups the stored user belongs to, in stored order.
    groups: Vec<String>,
    /// The pool's own token-signing key pair: every token of this sign-in is
    /// signed with it and names it by `kid`.
    signing_key: PoolSigningKey,
    access_ttl: Duration,
    id_ttl: Duration,
    refresh_ttl: Duration,
}

// ---------------------------------------------------------------------------
// Token signing
//
// Amazon Cognito signs every token a user pool issues against a key pair
// belonging to THAT pool, and publishes the verifying half as a JSON Web Key
// Set: a verifier must "decode the token ... and verify the signature" against
// the key the token's `kid` names in the pool's `jwks.json` (Amazon Cognito
// Developer Guide, "Verifying a JSON web token"). Every consumer of such a
// token — Verified Permissions, an API Gateway JWT authorizer, any JOSE
// library — reads `alg` and `kid` from the header, finds the matching JWK and
// checks the signature over `base64url(header) '.' base64url(claims)`, so a
// token whose third segment is EMPTY is unusable: it is not even a
// syntactically valid signed JWT.
//
// AWS signs with RS256. This emulator signs with ES256 (ECDSA on NIST P-256
// with SHA-256, RFC 7518 §3.4) because the only maintained pure-Rust RSA
// implementation carries two unfixable timing advisories (RUSTSEC-2023-0071 /
// CVE-2023-49092 and GHSA-4grx-2x9w-596c, both "no fixed version") that this
// repository's dependency audit blocks. ES256 comes from `ring`, the
// cryptography library this workspace already signs and encrypts with
// elsewhere, rather than from the `p256`/`elliptic-curve` stack, whose
// `crypto-bigint` 0.5 dependency carries a timing advisory (RUSTSEC-2026-0387)
// with no release in that line that fixes it. Nothing a verifier does changes:
// the algorithm is named in the token header AND in the published JWK, which is
// where a JOSE library reads it from, so the token verifies by the same
// `kid`-lookup-then-check-signature path. Both algorithms are JOSE registered
// (RFC 7518 §3.1), and the signature segment is a non-empty base64url run
// either way.
// ---------------------------------------------------------------------------

/// The `alg` of every token this service signs, and of the JWK it publishes for
/// the key that signed it: ECDSA using P-256 and SHA-256 (RFC 7518 §3.1/§3.4).
const TOKEN_SIGNING_ALGORITHM: &str = "ES256";

/// The `crv` of the curve [`TOKEN_SIGNING_ALGORITHM`] is defined over, as the
/// published JWK names it (RFC 7518 §6.2.1.1).
const TOKEN_SIGNING_CURVE: &str = "P-256";

/// The signing algorithm every pool's key is generated for and signs with:
/// ECDSA on P-256 with SHA-256, in the FIXED (`R || S`) signature encoding RFC
/// 7518 §3.4 puts in a JWS segment — not the ASN.1 DER form, which a JOSE
/// verifier would reject.
const SIGNING_ALGORITHM: &::ring::signature::EcdsaSigningAlgorithm =
    &::ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;

/// One user pool's token-signing key pair: the private key the pool's tokens
/// are signed with, named by the `kid` its published JWK carries.
///
/// The key belongs to the pool, so a token one pool issued carries one pool's
/// `kid` and verifies against only that pool's JWKS.
#[derive(Clone)]
pub struct PoolSigningKey {
    /// The `kid` the token header carries and the published JWK is found by:
    /// this key's RFC 7638 JWK thumbprint, so it is derived from the key itself
    /// rather than minted beside it.
    kid: String,
    /// The pool's private key. Behind an [`::std::sync::Arc`] because the key
    /// pair itself is not clonable and a pool's key is READ from many places —
    /// sharing the one key is also what makes every token of one pool verify
    /// against that pool's single published JWK.
    private: ::std::sync::Arc<::ring::signature::EcdsaKeyPair>,
}

/// Renders the key's PUBLIC identity only. Private key material is never
/// logged, traced or put in a response, so it is not in this rendering either.
impl ::std::fmt::Debug for PoolSigningKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PoolSigningKey")
            .field("kid", &self.kid)
            .field("alg", &TOKEN_SIGNING_ALGORITHM)
            .finish_non_exhaustive()
    }
}

impl PoolSigningKey {
    /// A freshly generated key pair, drawn from the operating system's CSPRNG
    /// (`ring::rand::SystemRandom`).
    ///
    /// One scalar multiplication, so this is microseconds of work: a pool mints
    /// its key when the pool is created and no request path ever generates one.
    /// INFALLIBLE: the only failure the generator reports is the OS CSPRNG or
    /// the key encoding failing, neither of which a pool can be created without.
    #[must_use]
    pub fn generate() -> Self {
        let rng = ::ring::rand::SystemRandom::new();
        let document = ::ring::signature::EcdsaKeyPair::generate_pkcs8(SIGNING_ALGORITHM, &rng)
            .expect("the operating system CSPRNG yields a P-256 key pair");
        let private =
            ::ring::signature::EcdsaKeyPair::from_pkcs8(SIGNING_ALGORITHM, document.as_ref(), &rng)
                .expect("a freshly generated P-256 key pair parses");
        let kid = jwk_thumbprint(&public_coordinates(&private));
        Self {
            kid,
            private: ::std::sync::Arc::new(private),
        }
    }

    /// The `kid` naming this key in the pool's JWKS.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// This key's PUBLIC half as a JWK (RFC 7517), the entry a verifier finds
    /// by `kid` in the pool's key set: `kty`/`crv` and the `x`/`y` curve
    /// coordinates RFC 7518 §6.2.1 defines, with the `use` and `alg` the
    /// signature is checked under. No private parameter (`d`) is in it.
    #[must_use]
    pub fn to_jwk(&self) -> serde_json::Value {
        let (x, y) = public_coordinates(&self.private);
        serde_json::json!({
            "alg": TOKEN_SIGNING_ALGORITHM,
            "crv": TOKEN_SIGNING_CURVE,
            "kid": self.kid,
            "kty": "EC",
            "use": "sig",
            "x": base64_url(&x),
            "y": base64_url(&y),
        })
    }

    /// The ES256 signature over `signing_input` — the 64-byte `R || S` form RFC
    /// 7518 §3.4 puts in the third segment — or `None` when signing failed, in
    /// which case the caller issues no token rather than one carrying bytes
    /// that are not a signature.
    #[must_use]
    pub fn sign(&self, signing_input: &[u8]) -> Option<Vec<u8>> {
        let rng = ::ring::rand::SystemRandom::new();
        let signature = self.private.sign(&rng, signing_input).ok()?;
        Some(signature.as_ref().to_vec())
    }
}

/// The public curve point `point` (uncompressed SEC1, `0x04 || X || Y`) as the
/// two fixed-width coordinates a JWK publishes: `x` and `y`, 32 bytes each
/// (RFC 7518 §6.2.1.2 — "the octet string representation MUST utilize all
/// octets", i.e. no leading zero is dropped).
fn public_coordinates(key: &::ring::signature::EcdsaKeyPair) -> (Vec<u8>, Vec<u8>) {
    // SEC 1 §2.3.3: the uncompressed point is `0x04 || X || Y`, each coordinate
    // the field element's full width, which on P-256 is
    // [`P256_COORDINATE_BYTES`].
    let point = key.public_key().as_ref();
    let x = point
        .get(1..=P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    let y = point
        .get(1 + P256_COORDINATE_BYTES..=2 * P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    (x, y)
}

/// The width of one P-256 coordinate: the curve's field elements are 256 bits,
/// and RFC 7518 §6.2.1.2 publishes each as the full-width octet string.
const P256_COORDINATE_BYTES: usize = 32;

/// The RFC 7638 JWK thumbprint of the public key whose coordinates are
/// `(x, y)`: the base64url SHA-256 of the key's canonical JWK — its required
/// members (`crv`, `kty`, `x`, `y` for an EC key) in lexicographic order, with
/// no whitespace. Two pools' keys therefore never share a `kid`, and a key's
/// `kid` is the same every time it is published.
fn jwk_thumbprint((x, y): &(Vec<u8>, Vec<u8>)) -> String {
    let canonical = serde_json::json!({
        "crv": TOKEN_SIGNING_CURVE,
        "kty": "EC",
        "x": base64_url(x),
        "y": base64_url(y),
    })
    .to_string();
    base64_url(&::sha2::Sha256::digest(canonical.as_bytes()))
}

/// `claims` as a JWS in compact serialization (RFC 7515 §3.1) signed with
/// `key`: `base64url(header) '.' base64url(claims) '.' base64url(signature)`,
/// where the header names the algorithm and the key (`kid`) a verifier checks
/// the signature with.
///
/// `None` when the signature could not be computed, so a token is either
/// signed or not issued.
fn signed_jws(claims: &serde_json::Value, key: &PoolSigningKey) -> Option<String> {
    let header = serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "kid": key.kid(),
        "typ": "JWT",
    });
    // RFC 7515 §5.1: the signature is computed over the ENCODED header and
    // payload, joined by '.', so those exact bytes are what the token carries.
    let signing_input = format!(
        "{}.{}",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes())
    );
    let signature = key.sign(signing_input.as_bytes())?;
    Some(format!("{signing_input}.{}", base64_url(&signature)))
}

/// `bytes` in base64url without padding, as a JWS segment is encoded
/// (RFC 7515 §2): the standard-alphabet codec this workspace already uses for
/// every Smithy blob (`aws_smithy_types::base64`), with `+` and `/` replaced by
/// `-` and `_` and the padding removed.
pub fn base64_url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// `time` as the modelled timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        time.duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default(),
    )
}

/// `time` as whole seconds since the epoch, for a token claim.
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `Some(values)` when `values` is non-empty, else `None`: an absent list is
/// absent on the wire rather than an empty one.
pub fn non_empty<T: Clone>(values: &[T]) -> Option<Vec<T>> {
    (!values.is_empty()).then(|| values.to_vec())
}

// ---------------------------------------------------------------------------
// Pagination
//
// Every paginated listing of this service pages the same way: over a stable
// ascending order of keys, with an OPAQUE `NextToken` naming the key the next
// page resumes strictly after.
// ---------------------------------------------------------------------------

/// The most items one page of a paginated listing holds, and what a request
/// that asks for no page size gets: `QueryLimitType`'s modelled range is 0..60
/// (this service's model, e.g. `AdminListGroupsForUser.Limit`), so 60 is the
/// most a caller can ask for.
pub const MAX_QUERY_LIMIT: i32 = 60;

/// Marks a `NextToken` as one THIS service minted, so a token it never minted
/// is refused on its prefix rather than on whether its bytes happen to decode.
const PAGE_TOKEN_PREFIX: &str = "cognito-idp-page-1:";

/// Separates a page token's listing scope from the key it resumes after. ASCII
/// unit separator: `PaginationKey`'s pattern is `^[\S]+$`, but the token is
/// base64-encoded before it reaches the wire, so the separator only has to be a
/// byte no scope carries.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// A `NextToken` that resumes `scope`'s listing strictly after `last_key`.
///
/// `scope` names the listing the token belongs to — the operation plus the
/// resource whose items it pages — so a token one listing issued names no page
/// of another. The token is base64-encoded with `aws_smithy_types::base64`, the
/// codec the wire protocols already carry, which keeps it opaque to the caller
/// and free of the whitespace `PaginationKey`'s pattern `^[\S]+$` excludes.
#[must_use]
pub fn encode_page_token(scope: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{PAGE_TOKEN_PREFIX}{scope}{PAGE_TOKEN_SEPARATOR}{last_key}"
    ))
}

/// The key `token` resumes `scope`'s listing after, or `None` when `token` is
/// not a token this service minted for that listing.
#[must_use]
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let (token_scope, last_key) = body.split_once(PAGE_TOKEN_SEPARATOR)?;
    (token_scope == scope).then(|| last_key.to_string())
}

/// One page of a paginated listing: the keys it serves and the `NextToken` the
/// page after it resumes at, absent when this page is the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub keys: Vec<String>,
    pub next_token: Option<String>,
}

/// How many items a listing serves for `limit`.
///
/// `QueryLimitType`'s modelled range (0..60) is refused at the boundary before
/// a handler runs, so the cap here never changes a request the framework
/// accepted. A request that asks for no limit takes the modelled maximum, and
/// so does one that asks for `0`: a page that may hold no item terminates no
/// pagination loop, and this service's model already reads a submitted `0` as
/// "no value" elsewhere (`PasswordPolicyType.TemporaryPasswordValidityDays`).
#[must_use]
pub fn page_size(limit: Option<i32>) -> usize {
    let maximum = MAX_QUERY_LIMIT as usize;
    match limit {
        Some(limit) if limit > 0 => (limit as usize).min(maximum),
        _ => maximum,
    }
}

/// The page of `keys` that `limit` and `next_token` ask for in `scope`'s
/// listing, or `None` when `next_token` is not a token this listing minted —
/// which the calling operation refuses with its own modelled error.
///
/// `keys` must be in the listing's stable ASCENDING order; the page resumes at
/// the first key strictly after the one the token names, so a key deleted
/// between two pages neither repeats nor skips the keys around it.
#[must_use]
pub fn page_of(
    scope: &str,
    keys: &[String],
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let after = decode_page_token(scope, token)?;
            keys.partition_point(|key| key <= &after)
        }
    };
    let end = start.saturating_add(page_size(limit)).min(keys.len());
    let served = keys.get(start..end).unwrap_or_default().to_vec();
    let next_token = match served.last() {
        Some(last) if end < keys.len() => Some(encode_page_token(scope, last)),
        _ => None,
    };
    Some(Page {
        keys: served,
        next_token,
    })
}

/// Whether `c` counts as a symbol for a password policy: anything that is
/// neither a letter nor a digit.
fn is_symbol(c: char) -> bool {
    !c.is_alphanumeric()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>)
}
