// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice service.
//!
//! One [`UserPool`] per created pool, each owning its app clients and its users,
//! beside the auth sessions and the tokens the sign-in operations issue. Every
//! operation computes its response from this state — a create stores a resource
//! and a later read renders the STORED resource, never the request that made it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccountRecoverySettingType, AcrLevelConfigType, AdminCreateUserConfigType,
    AnalyticsConfigurationType, AttributeType, AuthenticationResultType, DeviceConfigurationType,
    EmailConfigurationType, GroupType, IssuerConfigurationType, KeyConfigurationType,
    LambdaConfigType, MFAOptionType, NumberAttributeConstraintsType, PasswordPolicyType,
    RefreshTokenRotationType,
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

/// The most unexpired challenge sessions this emulator holds; the oldest are
/// evicted past it.
const MAX_AUTH_SESSIONS: usize = 10_000;

/// The most unexpired issued tokens this emulator holds; the oldest are evicted
/// past it. Three are recorded per sign-in.
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
    /// pool, and each lifetime from the app client's validity settings.
    /// `None` only when the pool, the client or the user is gone.
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
        let TokenInputs {
            issuer,
            sub,
            stored_username,
            email,
            groups,
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

        let access_token = unsecured_jws(&access_claims);
        let id_token = unsecured_jws(&id_claims);
        let refresh_token = unsecured_jws(&refresh_claims);

        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(3),
            |record| record.expires_at,
        );

        for (token, kind, ttl) in [
            (&access_token, TokenKind::Access, access_ttl),
            (&id_token, TokenKind::Id, id_ttl),
            (&refresh_token, TokenKind::Refresh, refresh_ttl),
        ] {
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
            refresh_token: Some(refresh_token),
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
    pub clients: HashMap<String, UserPoolClient>,
    /// Users keyed by [`UserPool::user_key`], so a pool configured
    /// case-insensitive treats `Alice` and `alice` as one user.
    pub users: HashMap<String, User>,
    /// This pool's groups keyed by `GroupName`, ordered so a read of several is
    /// stable. `GroupNameType` carries no case-insensitivity trait, so a name is
    /// matched exactly as it was created.
    pub groups: BTreeMap<String, Group>,
}

impl UserPool {
    /// The token issuer for this pool: the regional user-pool endpoint.
    pub fn issuer(&self) -> String {
        format!(
            "https://{SIGNING_NAME}.{}.amazonaws.com/{}",
            self.region, self.id
        )
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
    pub fn check_password(&self, password: &str) -> Option<String> {
        let policy = self.password_policy();
        let minimum = policy.minimum_length.unwrap_or(DEFAULT_PASSWORD_MIN_LENGTH);
        if i64::try_from(password.chars().count()).unwrap_or(i64::MAX) < i64::from(minimum) {
            return Some(format!(
                "Password did not conform with policy: Password must be at least {minimum} characters long"
            ));
        }
        if policy.require_uppercase && !password.chars().any(char::is_uppercase) {
            return Some(
                "Password did not conform with policy: Password must have uppercase characters"
                    .to_string(),
            );
        }
        if policy.require_lowercase && !password.chars().any(char::is_lowercase) {
            return Some(
                "Password did not conform with policy: Password must have lowercase characters"
                    .to_string(),
            );
        }
        if policy.require_numbers && !password.chars().any(|c| c.is_ascii_digit()) {
            return Some(
                "Password did not conform with policy: Password must have numeric characters"
                    .to_string(),
            );
        }
        if policy.require_symbols && !password.chars().any(is_symbol) {
            return Some(
                "Password did not conform with policy: Password must have symbol characters"
                    .to_string(),
            );
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

    /// The names of the attributes this pool's schema marks required that `user`
    /// has no value for: what a `NEW_PASSWORD_REQUIRED` challenge asks the user
    /// to supply. `sub` is never among them — the pool mints it.
    pub fn missing_required_attributes(&self, user: &User) -> Vec<String> {
        self.schema
            .iter()
            .filter(|entry| entry.required == Some(true))
            .filter_map(|entry| entry.name.clone())
            .filter(|name| name != "sub" && !user.attributes.contains_key(name))
            .collect()
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
            domain: None,
            custom_domain: None,
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
        self.explicit_auth_flows
            .iter()
            .any(|flow| ADMIN_PASSWORD_AUTH_FLOWS.contains(&flow.as_str()))
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

/// A challenge session: what a `RespondToAuthChallenge` for this session resumes.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub user_pool_id: String,
    pub client_id: String,
    pub username: String,
    pub challenge_name: String,
    pub expires_at: SystemTime,
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
        clients: HashMap::new(),
        users: HashMap::new(),
        groups: BTreeMap::new(),
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
    access_ttl: Duration,
    id_ttl: Duration,
    refresh_ttl: Duration,
}

/// `claims` as an UNSECURED JWS in compact serialization (RFC 7515 §6, "Using
/// the Algorithm none"): `base64url(header) '.' base64url(claims) '.'`, with an
/// empty signature segment.
///
/// Amazon Cognito signs its tokens with RS256 against the key pair published at
/// a pool's JWKS endpoint. This emulator holds no such key pair, and no
/// operation here verifies a token, so it issues the form RFC 7515 defines for
/// a JWS that carries no signature rather than filling the third segment with
/// bytes that would only look like one. The token is still this emulator's own:
/// its claims come from stored state, each carries a unique `jti`, and every
/// token issued is recorded in `issued_tokens`.
fn unsecured_jws(claims: &serde_json::Value) -> String {
    let header = serde_json::json!({ "alg": "none", "typ": "JWT" });
    let encoded_header = base64_url(header.to_string().as_bytes());
    let encoded_claims = base64_url(claims.to_string().as_bytes());
    format!("{encoded_header}.{encoded_claims}.")
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
