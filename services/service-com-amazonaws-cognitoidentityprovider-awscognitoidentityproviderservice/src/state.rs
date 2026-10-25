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
    AnalyticsConfigurationType, AttributeType, AuthenticationResultType,
    CodeDeliveryDetailsType, CustomDomainConfigType,
    DeviceConfigurationType, DeviceSecretVerifierConfigType, DeviceType, DomainDescriptionType,
    EmailConfigurationType, GroupType, IdentityProviderType, IssuerConfigurationType,
    KeyConfigurationType, LambdaConfigType, MFAOptionType, NewDeviceMetadataType,
    NumberAttributeConstraintsType, PasswordPolicyType, RefreshTokenRotationType,
    ResourceServerScopeType, ResourceServerType, RoutingType,
    SchemaAttributeType, SignInPolicyType, SmsConfigurationType, StringAttributeConstraintsType,
    TermsType, TokenValidityUnitsType, UserAttributeUpdateSettingsType, UserImportJobType,
    UserPoolAddOnsType,
    UserPoolClientType, UserPoolDescriptionType, UserPoolPolicyType, UserPoolType, UserType,
    UsernameConfigurationType, VerificationMessageTemplateType,
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

/// The `ExplicitAuthFlows` value that enables `AuthFlow=USER_AUTH`:
/// "`ALLOW_USER_AUTH`: Enable selection-based sign-in with `USER_AUTH`"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_AUTH_FLOWS: [&str; 1] = ["ALLOW_USER_AUTH"];

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

/// The message a device operation's `ResourceNotFoundException` carries for a
/// `DeviceKey` the signed-in user holds no confirmed device for. Like
/// [`USER_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so the
/// refusal is the same sentence for every key.
pub const DEVICE_NOT_FOUND_MESSAGE: &str = "Device does not exist.";

/// The message a device operation's `InvalidUserPoolConfigurationException`
/// carries on a pool that stores no `DeviceConfiguration`: "When you provide a
/// value for any property of `DeviceConfiguration`, you activate the device
/// remembering for the user pool" (this service's model), so a pool that
/// provided none remembers no device and has no device surface to serve.
pub const DEVICE_REMEMBERING_NOT_CONFIGURED_MESSAGE: &str =
    "Device tracking is not configured for this user pool.";

/// The message a `ResourceNotFoundException` carries when the group an operation
/// names is not one the named pool holds.
///
/// Amazon Cognito answers the same sentence for every group name: the group
/// lookup reports that nothing was found and names no group, so the refusal
/// cannot be read as a statement about one particular group name. Every
/// operation that resolves a `GroupName` against a pool reports it the same way,
/// exactly as [`USER_NOT_FOUND_MESSAGE`] is reported for a username.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// The message a `TermsExistsException` carries for a `TermsName` the app client
/// already holds terms documents under.
///
/// Terms document names "must be unique to the app client" (this service's
/// model), so the refusal names the name that collided and the app client it
/// collided within — the same sentence wherever that uniqueness is enforced, on
/// a create of a second document of that name and on a rename onto it alike.
pub fn terms_exists_message(terms_name: &str, client_id: &str) -> String {
    format!("Terms with name {terms_name} already exists for Client {client_id}.")
}

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
// User import jobs
//
// A job's `Status` is COMPUTED from what is stored plus the clock, so a read
// renders the status the job is in at that instant and writes nothing.
// Each value below is one of the modelled
// [`crate::ops::types::USER_IMPORT_JOB_STATUS_TYPE_VALUES`].
// ---------------------------------------------------------------------------

/// "`Created` - The job was created but not started"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_CREATED: &str = "Created";

/// "`Pending` - A transition state. You have started the job, but it has not
/// begun importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_PENDING: &str = "Pending";

/// "`InProgress` - The job has started, and users are being imported"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// "`Stopping` - You have stopped the job, but the job has not stopped
/// importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPING: &str = "Stopping";

/// "`Stopped` - You have stopped the job, and the job has stopped importing
/// users" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPED: &str = "Stopped";

/// "`Expired` - You created a job, but did not start the job within 24-48
/// hours" (`UserImportJobType.Status`).
pub const JOB_STATUS_EXPIRED: &str = "Expired";

/// How long a started job stays `Pending` before the clock owes it
/// `InProgress`.
///
/// A chosen span, not a documented duration: `UserImportJobType.Status`
/// documents `Pending` as "A transition state. You have started the job, but it
/// has not begun importing users yet", so a job is NOT importing when
/// `StartUserImportJob` returns. Never zero, so a caller reading the job right
/// after starting it still sees the transient status.
pub const JOB_START_SPAN: Duration = Duration::from_millis(500);

/// How long a stopped job stays `Stopping` before the clock owes it `Stopped`.
///
/// A chosen span, not a documented duration: `Stopping` is "You have stopped the
/// job, but the job has not stopped importing users yet"
/// (`UserImportJobType.Status`), so the stop is not complete when
/// `StopUserImportJob` returns.
pub const JOB_STOP_SPAN: Duration = Duration::from_millis(500);

/// How long a created job can go unstarted before the clock owes it `Expired`:
/// "You created a job, but did not start the job within 24-48 hours ... the job
/// can't be started" (`UserImportJobType.Status`), whose earliest documented
/// bound is 24 hours.
pub const JOB_EXPIRY_SPAN: Duration = Duration::from_secs(24 * 60 * 60);

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

/// The most devices one user holds. A sign-in from a device the user pool does
/// not recognise stores one, so the device authenticated longest ago is evicted
/// past this cap: repeated sign-ins cannot grow one user without limit.
pub const MAX_DEVICES_PER_USER: usize = 200;

/// The most resource servers one user pool holds.
pub const MAX_RESOURCE_SERVERS_PER_POOL: usize = 1_000;

/// The most identity providers one user pool holds.
pub const MAX_IDENTITY_PROVIDERS_PER_POOL: usize = 1_000;

/// The most user import jobs one user pool holds: `UserImportJobsListType`, the
/// list `ListUserImportJobs` answers a pool's jobs in, is modelled with a
/// maximum length of 50, so a pool holds no more jobs than one listing can
/// report.
pub const MAX_USER_IMPORT_JOBS_PER_POOL: usize = 50;

/// The most custom (and developer-only) attributes one user pool's schema
/// holds: Amazon Cognito's quota for custom attributes per user pool is 50 and
/// cannot be raised (Amazon Cognito Developer Guide, "Quotas in Amazon
/// Cognito"). The standard attributes every pool carries are not counted
/// against it.
pub const MAX_CUSTOM_ATTRIBUTES_PER_POOL: usize = 50;

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
            .filter(|stored| stored.expires_at <= now)
            .cloned();
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        match self.auth_sessions.get(token) {
            Some(stored) => SessionLookup::Found(stored.clone()),
            None => match expired {
                Some(stored) => SessionLookup::Expired(stored),
                None => SessionLookup::Unknown,
            },
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

    /// The stored pool and user that an ACCESS token identifies as the currently
    /// signed-in user, as of `now`.
    ///
    /// The self-service half of this API identifies its user by the `AccessToken`
    /// the sign-in returned and models no `UserPoolId` or `Username` member at
    /// all, so the stored token record is what names the pool and the user:
    /// `ListDevices.AccessToken` is "a valid access token that Amazon Cognito
    /// issued to the currently signed-in user" (this service's model). Only an
    /// access token answers — an id or refresh token names its user too, but it
    /// is not the token the member asks for.
    ///
    /// A PURE read: a token the clock has already expired is refused where it
    /// stands rather than swept, so asking changes nothing a later caller sees.
    pub fn signed_in_user(
        &self,
        access_token: &str,
        now: SystemTime,
    ) -> Result<(&UserPool, &User), TokenRejection> {
        let record = self
            .issued_tokens
            .get(access_token)
            .ok_or(TokenRejection::NotAnAccessToken)?;
        if record.kind != TokenKind::Access || record.revoked {
            return Err(TokenRejection::NotAnAccessToken);
        }
        if record.expires_at <= now {
            return Err(TokenRejection::Expired);
        }
        let pool = self
            .pool(&record.user_pool_id)
            .ok_or(TokenRejection::PoolGone)?;
        let user = pool
            .user(&record.username)
            .ok_or(TokenRejection::UserGone)?;
        Ok((pool, user))
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

    /// Record the device a completed sign-in presented and answer the
    /// `AuthenticationResult.NewDeviceMetadata` that sign-in owes.
    ///
    /// `DeviceConfigurationType`: "When you provide a value for any property of
    /// `DeviceConfiguration`, you activate the device remembering for the user
    /// pool", and `NewDeviceMetadataType` is "Information that your user pool
    /// responds with in `AuthenticationResult` when you configure it to remember
    /// devices and a user signs in with an unrecognized device" (this service's
    /// model). So a pool that stores NO `DeviceConfiguration` answers `None`,
    /// whatever the sign-in presented, and so does a sign-in that presented a
    /// `DEVICE_KEY` this user already holds — that device is recognized, and only
    /// its last-authenticated instant moves. Anything else is an unrecognized
    /// device: a device is stored for the user and its key, beside the pool's
    /// device group key, is what the caller gets back. A key the user does not
    /// hold — another user's, or one nothing minted — is unrecognized by this
    /// user pool for this user, so it is answered the same way as none at all
    /// rather than refusing the sign-in the first factor already passed.
    ///
    /// The stored device is NOT confirmed yet: `ConfirmDevice` is what confirms
    /// it and decides whether it is remembered.
    pub fn track_sign_in_device(
        &mut self,
        user_pool_id: &str,
        username: &str,
        presented_device_key: Option<&str>,
        now: SystemTime,
    ) -> Option<NewDeviceMetadataType> {
        let pool = self.user_pools.get_mut(user_pool_id)?;
        // A pool that provided no `DeviceConfiguration` activated no device
        // remembering, so it answers nothing whatever the sign-in presented.
        pool.device_configuration.as_ref()?;
        let region = pool.region.clone();
        let device_group_key = pool.device_group_key.clone();
        let user = pool.user_mut(username)?;
        if let Some(device) = presented_device_key.and_then(|key| user.devices.get_mut(key)) {
            device.last_authenticated_date = Some(now);
            return None;
        }
        let device_key = new_device_key(&region);
        user.store_device(Device::signed_in(device_key.clone(), now));
        Some(NewDeviceMetadataType {
            device_key: Some(device_key),
            device_group_key: Some(device_group_key),
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
    /// This pool's third-party identity providers keyed by their `ProviderName`,
    /// ordered so a read of several is stable. `ProviderNameType` carries no
    /// case-insensitivity trait, so a name is matched exactly as it was created.
    pub identity_providers: BTreeMap<String, IdentityProvider>,
    /// This pool's terms documents keyed by their minted `TermsId`, ordered so a
    /// read of several is stable. A terms document belongs to one app client of
    /// this pool, and "terms document names must be unique to the app client"
    /// (`TermsExistsException` in this service's model), so the (`ClientId`,
    /// `TermsName`) pair is unique across this map too. No separate bound is
    /// needed: `TermsNameType` admits two names and a document names a stored
    /// app client, so this map holds at most twice [`MAX_CLIENTS_PER_POOL`].
    pub terms: BTreeMap<String, Terms>,
    /// This pool's user import jobs keyed by their minted `JobId`, ordered so a
    /// read of several is stable: "Import jobs are retained in user pool
    /// configuration so that you can stage, stop, start, review, and delete
    /// them" (`ListUserImportJobs`).
    pub user_import_jobs: BTreeMap<String, UserImportJob>,
    /// The key pair this pool's tokens are signed with and whose public half
    /// its key set publishes, minted with the pool and kept for its lifetime,
    /// so every token the pool ever issued stays verifiable against the key set
    /// it publishes now.
    pub signing_key: PoolSigningKey,
    /// The `DeviceGroupKey` every device of this pool is reported under:
    /// "an identifier used in generating the `DEVICE_PASSWORD_VERIFIER` for
    /// device SRP authentication" (`NewDeviceMetadataType.DeviceGroupKey`).
    /// Minted with the pool and kept for its lifetime, so two sign-ins of the
    /// same pool report the same group key.
    pub device_group_key: String,
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

    /// Whether this pool remembers devices at all: `DeviceConfigurationType` is
    /// "the device-remembering configuration for a user pool", and "when you
    /// provide a value for any property of `DeviceConfiguration`, you activate
    /// the device remembering for the user pool" (this service's model), so a
    /// pool that stores none has no device surface to serve.
    pub fn remembers_devices(&self) -> bool {
        self.device_configuration.is_some()
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

    /// The keys in [`UserPool::users`] of the users that belong to
    /// `group_name`, in ascending key order — the stable order a paginated
    /// member listing indexes into.
    ///
    /// `GroupNameType` carries no case-insensitivity trait, so the name is
    /// matched exactly as it was created, and a user is reported only while its
    /// stored membership names that group.
    pub fn group_member_keys(&self, group_name: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .users
            .iter()
            .filter(|(_, user)| user.groups.contains(group_name))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
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

    /// This pool's schema entry for the attribute STORED under `name` — the
    /// prefixed name for a custom or developer-only attribute — or `None` when
    /// the schema declares no such attribute.
    pub fn schema_attribute(&self, name: &str) -> Option<&SchemaAttributeType> {
        self.schema
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
    }

    /// How many of this pool's schema entries are custom or developer-only
    /// attributes: the ones added beyond the standard attributes every pool
    /// carries.
    pub fn custom_attribute_count(&self) -> usize {
        self.schema
            .iter()
            .filter(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(|name| !is_standard_attribute(name))
            })
            .count()
    }

    /// Whether this pool's schema lets `name`'s stored value be CHANGED:
    /// `SchemaAttributeType.Mutable` — "Specifies whether the value of the
    /// attribute can be changed" (this service's model). An attribute the
    /// schema does not declare has no entry to read, so it is reported
    /// unchangeable.
    pub fn attribute_mutable(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.mutable)
            .unwrap_or(false)
    }

    /// Whether this pool's schema marks `name` a required attribute.
    pub fn attribute_required(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.required)
            .unwrap_or(false)
    }

    /// Whether a sign-in name of this pool resolves to `name`'s value:
    /// `AliasAttributes` are "attributes supported as an alias for this user
    /// pool" and `UsernameAttributes` "specifies whether a user can use an email
    /// address or phone number as a username when they sign up" (this service's
    /// model). Either way the value names at most one user of the pool.
    pub fn is_alias_attribute(&self, name: &str) -> bool {
        self.alias_attributes
            .iter()
            .chain(self.username_attributes.iter())
            .any(|attribute| attribute == name)
    }

    /// Whether this pool verifies `name` itself: `AutoVerifiedAttributes` are
    /// "the attributes that are auto-verified in a user pool" (this service's
    /// model), so a new value of one of them is what a verification message is
    /// sent for.
    pub fn auto_verifies(&self, name: &str) -> bool {
        self.auto_verified_attributes
            .iter()
            .any(|attribute| attribute == name)
    }

    /// The attribute a sign-up confirmation message for `user` is addressed to,
    /// and the stored value it goes to, or `None` when this pool has no such
    /// destination for that user.
    ///
    /// "Amazon Cognito sends confirmation codes to the user attribute in the
    /// `AutoVerifiedAttributes` property of your user pool"
    /// (`ResendConfirmationCode` in this service's model), so the pool's own
    /// auto-verified attributes decide where the code goes, in the order the
    /// pool declares them; the user has to hold a value for one of them for a
    /// message to have an address at all.
    pub fn confirmation_destination(&self, user: &User) -> Option<(String, String)> {
        self.auto_verified_attributes
            .iter()
            .filter(|attribute| delivery_medium(attribute).is_some())
            .find_map(|attribute| {
                user.destination_of(attribute)
                    .map(|destination| (attribute.clone(), destination))
            })
    }

    /// Where a password-reset message for `user` goes, as this pool's
    /// `AccountRecoverySetting` decides.
    ///
    /// "If `verified_email` has a priority of 1 and `verified_phone_number` has
    /// a priority of 2, your user pool sends account-recovery messages to a
    /// verified email address but falls back to an SMS message if the user has a
    /// verified phone number. The `admin_only` option prevents self-service
    /// account recovery" (`RecoveryOptionType` in this service's model) \u2014 so the
    /// mechanisms are tried in ascending priority and only a VERIFIED value is a
    /// destination.
    pub fn recovery_destination(&self, user: &User) -> RecoveryDestination {
        let mut mechanisms: Vec<(i32, String)> = self
            .account_recovery_setting
            .as_ref()
            .and_then(|setting| setting.recovery_mechanisms.clone())
            .map(|mechanisms| {
                mechanisms
                    .into_iter()
                    .map(|mechanism| (mechanism.priority, mechanism.name))
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_RECOVERY_MECHANISMS
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        (
                            i32::try_from(index).unwrap_or_default() + 1,
                            (*name).to_string(),
                        )
                    })
                    .collect()
            });
        if mechanisms
            .iter()
            .any(|(_, name)| name == RECOVERY_ADMIN_ONLY)
        {
            return RecoveryDestination::AdminOnly;
        }
        mechanisms.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        mechanisms
            .iter()
            .filter_map(|(_, name)| recovered_attribute(name))
            .find_map(|attribute| {
                user.verified_destination_of(attribute)
                    .map(|destination| RecoveryDestination::Delivered {
                        attribute: attribute.to_string(),
                        destination,
                    })
            })
            .unwrap_or(RecoveryDestination::None)
    }

    /// Whether `factor` is a first authentication factor this pool admits:
    /// `SignInPolicyType.AllowedFirstAuthFactors` is "the sign-in methods that a
    /// user pool supports as the first factor" (this service's model), and a pool
    /// that configures none admits `PASSWORD` alone
    /// ([`effective_policies`]).
    pub fn allows_first_auth_factor(&self, factor: &str) -> bool {
        self.policies
            .sign_in_policy
            .as_ref()
            .and_then(|policy| policy.allowed_first_auth_factors.as_ref())
            .is_some_and(|factors| factors.iter().any(|allowed| allowed == factor))
    }

    /// Whether this pool holds a new value of `name` PENDING until the user
    /// confirms it:
    /// `UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`
    /// "requires that your user verifies their email address, phone number, or
    /// both before Amazon Cognito updates the value of that attribute" (this
    /// service's model).
    pub fn requires_verification_before_update(&self, name: &str) -> bool {
        self.user_attribute_update_settings
            .as_ref()
            .and_then(|settings| {
                settings
                    .attributes_require_verification_before_update
                    .as_ref()
            })
            .is_some_and(|names| names.iter().any(|attribute| attribute == name))
    }

    /// Whether a user of this pool OTHER than `except_username` already holds
    /// `value` as its active alias value of `name`.
    ///
    /// An alias value identifies ONE user of the pool, so no two users may hold
    /// the same one at once: for `email` and `phone_number` the value is an
    /// alias only while the user's matching verified flag says the value was
    /// confirmed, and an alias attribute with no such flag counts whatever its
    /// value (see [`User::alias_is_active`]).
    pub fn alias_taken(&self, name: &str, value: &str, except_username: &str) -> bool {
        let except = self.user_key(except_username);
        self.users.iter().any(|(key, user)| {
            key != &except
                && user
                    .attributes
                    .get(name)
                    .is_some_and(|stored| stored == value)
                && user.alias_is_active(name)
        })
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

    /// Take this pool's domain under the name `domain` out of the pool — its
    /// prefix domain or its custom one — and hand back the record that was
    /// stored, or `None` when this pool holds no domain of that name.
    ///
    /// The two slots are the same resource kind under `UserPoolType.Domain` and
    /// `UserPoolType.CustomDomain`, so a delete clears whichever one holds the
    /// name and leaves the other in place.
    pub fn remove_domain_named(&mut self, domain: &str) -> Option<UserPoolDomain> {
        [&mut self.domain, &mut self.custom_domain]
            .into_iter()
            .find(|slot| slot.as_ref().is_some_and(|stored| stored.domain == domain))
            .and_then(Option::take)
    }

    /// This pool's terms documents that app client `client_id` holds under
    /// `terms_name`, or `None` when the client holds none of that name.
    ///
    /// A terms document is assigned to one app client and its name is unique to
    /// that client (`TermsExistsException` in this service's model), so at most
    /// one stored document can answer.
    pub fn terms_for_client(&self, client_id: &str, terms_name: &str) -> Option<&Terms> {
        self.terms
            .values()
            .find(|stored| stored.client_id == client_id && stored.terms_name == terms_name)
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

    /// The SHORT modelled view of this pool, rendered from what is stored: the
    /// entry a listing of several pools serves ("A short description of a user
    /// pool", `UserPoolDescriptionType`). Every member it shares with
    /// [`UserPool::to_user_pool_type`] renders the same stored value, so a
    /// listing entry and a read of the one pool never disagree.
    pub fn to_user_pool_description_type(&self) -> UserPoolDescriptionType {
        UserPoolDescriptionType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            lambda_config: self.lambda_config.clone(),
            // `UserPoolDescriptionType.Status` is `@deprecated` ("This property
            // is no longer available."), so it is left out rather than
            // invented — as `to_user_pool_type` leaves it out too.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            // "A list of Amazon Web Services Regions where replicas of this
            // user pool exist" — nothing in this emulator stores a replica, so
            // there is no replication to report for any pool.
            replica_regions: None,
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

    /// Whether `AuthFlow=USER_AUTH` is enabled for this client.
    pub fn allows_user_auth(&self) -> bool {
        self.allows_any_of(&USER_AUTH_FLOWS)
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

    /// Whether `presented` is the `SecretHash` a request for `username` has to
    /// carry to be accepted by this app client.
    ///
    /// `SecretHash` is "a keyed-hash message authentication code (HMAC)
    /// calculated using the secret key of a user pool client and username plus
    /// the client ID in the message" (`SignUp.SecretHash` in this service's
    /// model), carried base64-encoded as the modelled `SecretHashType` pattern
    /// `^[\w+=/]+$` admits.
    ///
    /// A client with NO stored secret has no such secret key, so there is
    /// nothing to verify and every request to it is accepted. A client that
    /// HOLDS a secret accepts only the tag THAT secret produces over
    /// [`secret_hash_message`]: a request that carries none, or one computed
    /// from another secret, another username or another client, proves nothing
    /// about holding the secret.
    ///
    /// The check is `ring`'s own tag verification, so a rejected hash tells a
    /// caller nothing about how much of the expected one it got right.
    pub fn accepts_secret_hash(&self, username: &str, presented: Option<&str>) -> bool {
        let Some(secret) = self.client_secret.as_deref() else {
            return true;
        };
        let Some(presented) = presented else {
            return false;
        };
        let Ok(tag) = ::aws_smithy_types::base64::decode(presented) else {
            return false;
        };
        let key = ::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, secret.as_bytes());
        ::ring::hmac::verify(
            &key,
            secret_hash_message(username, &self.client_id).as_bytes(),
            &tag,
        )
        .is_ok()
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
    /// The values this user has asked for but not confirmed yet, keyed by
    /// attribute name: a pool that requires verification before an update
    /// "doesn't change the value of the attribute until your user responds to
    /// the verification message and confirms the new value"
    /// (`UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`),
    /// and `VerifyUserAttribute` "updates the affected attribute to its pending
    /// value" (this service's model). Never rendered: a pending value is not
    /// the user's value yet.
    pub pending_attributes: BTreeMap<String, String>,
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
    /// The devices remembered for this user, keyed by `DeviceKey` and ordered so
    /// a listing of several is stable. `DeviceKeyType` carries no
    /// case-insensitivity trait, so a key is matched exactly as it was stored.
    pub devices: BTreeMap<String, Device>,
    /// The verification codes outstanding for this user, keyed by what each was
    /// issued for and ordered so a read is stable.
    ///
    /// A fresh code for one purpose REPLACES the one issued for it before, so a
    /// user holds at most one code per purpose and this map is bounded by
    /// [`CodePurpose`]'s variants and the attributes a message is sent for at all
    /// ([`delivery_medium`]).
    pub outstanding_codes: BTreeMap<CodePurpose, VerificationCode>,
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

    /// Whether this user's stored value of `name` is an ACTIVE alias value: an
    /// `email` or a `phone_number` counts only while its verified flag says the
    /// value was confirmed ([`verified_flag`]), since an unconfirmed value is
    /// not one a sign-in resolves; an alias attribute that carries no such flag
    /// counts whatever its value.
    pub fn alias_is_active(&self, name: &str) -> bool {
        match verified_flag(name) {
            Some(flag) => self
                .attributes
                .get(flag)
                .is_some_and(|value| value == FLAG_CONFIRMED),
            None => true,
        }
    }

    /// The keys of this user's CONFIRMED devices in ascending order — the stable
    /// order a paginated device listing indexes into.
    ///
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to the
    /// currently signed-in user" and `ConfirmDevice` "confirms a device that a
    /// user wants to remember" (this service's model), so a device a sign-in only
    /// minted a key for is not one a listing reports until it is confirmed.
    pub fn device_keys(&self) -> Vec<String> {
        self.confirmed_device_keys()
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

    /// Store `device` for this user, evicting the device authenticated longest
    /// ago while the user is already at [`MAX_DEVICES_PER_USER`].
    pub fn store_device(&mut self, device: Device) {
        while self.devices.len() >= MAX_DEVICES_PER_USER {
            let Some(oldest) = self
                .devices
                .iter()
                .min_by_key(|(_, stored)| stored.last_authenticated_date)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.devices.remove(&oldest);
        }
        self.devices.insert(device.device_key.clone(), device);
    }

    /// The stored device `device_key` names, once `ConfirmDevice` has confirmed
    /// it. `GetDevice` "returns information about a remembered device" and
    /// `ConfirmDevice` "confirms a device that a user wants to remember" (this
    /// service's model), so a device a sign-in minted and nothing confirmed is
    /// not yet a device of this user pool.
    pub fn confirmed_device(&self, device_key: &str) -> Option<&Device> {
        self.devices
            .get(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, mutably, once it is confirmed.
    pub fn confirmed_device_mut(&mut self, device_key: &str) -> Option<&mut Device> {
        self.devices
            .get_mut(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, once it is confirmed AND remembered.
    ///
    /// The self-service reader is narrower than the admin one: `GetDevice`
    /// "returns information about a remembered device for the current user",
    /// while `AdminGetDevice` "returns details for a user's device" and
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to the
    /// currently signed-in user" (this service's model). So a device the user has
    /// not (yet) chosen to remember is registered and listed, but is not a
    /// remembered device.
    pub fn remembered_device(&self, device_key: &str) -> Option<&Device> {
        self.confirmed_device(device_key)
            .filter(|device| device.remembered)
    }

    /// The keys of this user's confirmed devices, in ascending key order: the
    /// stable order a listing of them pages over.
    pub fn confirmed_device_keys(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|(_, device)| device.confirmed)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The stored value of `attribute` a message can be addressed to, or `None`
    /// when this user holds no value for it: a code is a message, so it needs an
    /// address to be sent to.
    pub fn destination_of(&self, attribute: &str) -> Option<String> {
        self.attributes
            .get(attribute)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    /// The stored value of `attribute` whose verified flag says this user
    /// confirmed it ([`verified_flag`]) \u2014 the only kind of destination a
    /// password-reset message is sent to.
    pub fn verified_destination_of(&self, attribute: &str) -> Option<String> {
        self.destination_of(attribute)
            .filter(|_| self.alias_is_active(attribute))
    }

    /// Record `code` as the code outstanding for `purpose`, REPLACING whatever
    /// was outstanding for it before \u2014 which is what makes a re-sent code
    /// supersede the one before it \u2014 and return what was stored.
    pub fn issue_code(&mut self, purpose: CodePurpose, code: VerificationCode) -> VerificationCode {
        self.outstanding_codes.insert(purpose, code.clone());
        code
    }

    /// The code outstanding for `purpose`, or `None` when nothing is.
    pub fn outstanding_code(&self, purpose: &CodePurpose) -> Option<&VerificationCode> {
        self.outstanding_codes.get(purpose)
    }

    /// Check `presented` against the code outstanding for `purpose` as of `now`.
    ///
    /// A PURE read: accepting a code is what spends it ([`User::consume_code`]),
    /// so a refused attempt leaves the outstanding code exactly as it was and a
    /// caller can present the right one afterwards. Nothing outstanding, a value
    /// that is not the outstanding code, and a code a later one superseded are
    /// all the same answer: this is not the code the service is expecting.
    pub fn check_code(
        &self,
        purpose: &CodePurpose,
        presented: &str,
        now: SystemTime,
    ) -> Result<&VerificationCode, CodeRejection> {
        let outstanding = self
            .outstanding_code(purpose)
            .ok_or(CodeRejection::Mismatch)?;
        if outstanding.code != presented {
            return Err(CodeRejection::Mismatch);
        }
        if outstanding.expired(now) {
            return Err(CodeRejection::Expired);
        }
        Ok(outstanding)
    }

    /// Spend the code outstanding for `purpose`: a delivered code is accepted
    /// ONCE, so presenting it again finds nothing outstanding.
    pub fn consume_code(&mut self, purpose: &CodePurpose) -> Option<VerificationCode> {
        self.outstanding_codes.remove(purpose)
    }
}

/// What one stored verification code was issued for.
///
/// Amazon Cognito sends a code for several distinct purposes, and each is
/// answered by its own operation: the sign-up confirmation code by
/// `ConfirmSignUp`, the password-reset code by `ConfirmForgotPassword` and an
/// attribute's code by `VerifyUserAttribute`. Keying the outstanding codes by
/// purpose is what keeps them independent — a resent sign-up code does not
/// disturb an outstanding reset code — while a second code for the SAME purpose
/// supersedes the first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodePurpose {
    /// The code that confirms a new user's sign-up, sent by `SignUp` and
    /// re-sent by `ResendConfirmationCode`.
    SignUpConfirmation,
    /// The code that authorises a password reset, sent by `ForgotPassword`.
    PasswordReset,
    /// The code that confirms the value of ONE attribute, sent by
    /// `GetUserAttributeVerificationCode` and by an update of an attribute the
    /// pool verifies, and carried for the attribute it names.
    AttributeVerification(String),
}

/// One verification code issued to a user: what the operation that sent the
/// code recorded, and the delivery it reported.
#[derive(Debug, Clone)]
pub struct VerificationCode {
    /// The code itself ([`new_verification_code`]). The real service delivers
    /// it out of band — by mail or text message — so no response carries it;
    /// it is stored because it is what the operation that consumes this code
    /// checks the presented one against.
    pub code: String,
    /// The user attribute the message was addressed to (`VerifiedAttributeType`),
    /// which is what `CodeDeliveryDetailsType.AttributeName` reports and what
    /// decides the medium it travelled over ([`delivery_medium`]).
    pub attribute: String,
    /// The attribute value the code was sent to: the address or number the
    /// message was addressed to, which is what
    /// `CodeDeliveryDetailsType.Destination` reports, masked.
    pub destination: String,
    /// When this code was issued, which is what its validity window is counted
    /// from.
    pub issued_at: SystemTime,
}

impl VerificationCode {
    /// A fresh verification code for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_verification_code(), attribute, destination, now)
    }

    /// A fresh email one-time password for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued_email_otp(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_email_otp_code(), attribute, destination, now)
    }

    /// `code`, recorded as issued for `attribute` at `now`.
    fn minted(code: String, attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self {
            code,
            attribute: attribute.to_string(),
            destination: destination.to_string(),
            issued_at: now,
        }
    }

    /// The delivery this code's message reported, rendered from what is stored.
    pub fn delivery_details(&self) -> Option<CodeDeliveryDetailsType> {
        code_delivery_details(&self.attribute, &self.destination)
    }

    /// Whether this code's validity window has closed by `now`.
    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.issued_at + VERIFICATION_CODE_VALIDITY
    }
}

/// Why a presented code was not accepted, which decides the modelled error the
/// operation that was given it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRejection {
    /// The presented code is not the outstanding one — including the case where
    /// nothing is outstanding at all, and the case where a later code for the
    /// same purpose superseded the presented one.
    Mismatch,
    /// The presented code IS the outstanding one, but its validity window has
    /// closed.
    Expired,
}

/// How long an issued code stays acceptable: Amazon Cognito's verification and
/// confirmation codes expire 24 hours after they are sent (Amazon Cognito
/// Developer Guide, "Signing up and confirming user accounts").
pub const VERIFICATION_CODE_VALIDITY: Duration = Duration::from_secs(24 * 60 * 60);

/// The message a code that is not the outstanding one is refused with.
pub const CODE_MISMATCH_MESSAGE: &str = "Invalid verification code provided, please try again.";

/// The message a code past its validity window is refused with.
pub const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// One device of a user: the device a sign-in minted a `DeviceKey` for, as the
/// device operations stored it, and what every later device read of this user
/// renders.
///
/// A sign-in on a pool that remembers devices stores one of these for a device
/// it does not recognise; `ConfirmDevice` is what confirms it — which is what
/// makes the user pool report it — and what decides whether it is remembered.
#[derive(Debug, Clone)]
pub struct Device {
    /// The key this device is known by (`DeviceKeyType`, len 1..55, pattern
    /// `^[\w-]+_[0-9a-f-]+$`), which every device operation names it with.
    pub device_key: String,
    /// The device's attributes keyed by name, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    /// The SRP verifier `ConfirmDevice` registered for this device, when it
    /// carried one. Stored because it is what the device's own authentication
    /// would check; no operation implemented here reads it yet.
    pub secret_verifier: Option<DeviceSecretVerifierConfigType>,
    /// Whether `ConfirmDevice` has confirmed this device, which is what makes it
    /// a device the user pool reports.
    pub confirmed: bool,
    /// Whether this device is remembered, i.e. eligible for device
    /// authentication: `ConfirmDevice` sets it unless the pool asks for the
    /// user's own prompt first, and `UpdateDeviceStatus` is what the user's
    /// answer reaches.
    pub remembered: bool,
    pub create_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// When this device last authenticated a sign-in, absent until one has:
    /// `DeviceType.DeviceLastAuthenticatedDate` is "the date when the device was
    /// last authenticated" (this service's model), which is nothing yet for a
    /// device that has not authenticated.
    pub last_authenticated_date: Option<SystemTime>,
}

impl Device {
    /// The device a sign-in just stored: known to this user pool, not confirmed
    /// and so not remembered, and last authenticated at that sign-in.
    pub fn signed_in(device_key: String, now: SystemTime) -> Self {
        Self {
            device_key,
            attributes: BTreeMap::new(),
            secret_verifier: None,
            confirmed: false,
            remembered: false,
            create_date: now,
            last_modified_date: now,
            last_authenticated_date: Some(now),
        }
    }

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

    /// The modelled view of this device, rendered from what is stored.
    pub fn to_device_type(&self) -> DeviceType {
        DeviceType {
            device_key: Some(self.device_key.clone()),
            device_attributes: non_empty(&self.attribute_list()),
            device_create_date: Some(to_date_time(self.create_date)),
            device_last_modified_date: Some(to_date_time(self.last_modified_date)),
            device_last_authenticated_date: self.last_authenticated_date.map(to_date_time),
        }
    }
}

/// The `DeviceAttributes` name a device's `ConfirmDevice.DeviceName` — "A
/// friendly name for the device" — is reported under.
pub const DEVICE_NAME_ATTRIBUTE: &str = "device_name";

/// `DeviceRememberedStatusType`'s value for a device that is remembered, i.e.
/// eligible for device authentication; its only other value is the one for a
/// device that is not (`crate::ops::types::DEVICE_REMEMBERED_STATUS_TYPE_VALUES`).
pub const DEVICE_REMEMBERED_STATUS: &str = "remembered";

/// Why an access token does not identify a signed-in user
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::signed_in_user`]).
/// The operation that asked maps each one to a modelled error IT declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// Not a token this emulator issued, not an ACCESS token, or revoked.
    NotAnAccessToken,
    /// An access token whose lifetime the clock is past.
    Expired,
    /// The token names a user pool this emulator no longer holds.
    PoolGone,
    /// The token names a user its pool no longer holds.
    UserGone,
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

/// One stored user import job of a user pool: what `CreateUserImportJob` stored
/// and every later read of it renders.
///
/// The job's `Status` is not stored: it is COMPUTED from the instants below and
/// the clock ([`UserImportJob::status`]), so a read never has to move it.
#[derive(Debug, Clone)]
pub struct UserImportJob {
    pub job_id: String,
    pub job_name: String,
    pub user_pool_id: String,
    /// The role "that has permission to log import-job results to Amazon
    /// CloudWatch Logs" (`CreateUserImportJob.CloudWatchLogsRoleArn`), kept
    /// exactly as it was submitted.
    pub cloud_watch_logs_role_arn: String,
    /// The hashing algorithm the CSV's password hashes were made with, absent on
    /// a job created without one: the model documents no default.
    pub password_hashing_algorithm: Option<String>,
    /// Where a client uploads this job's CSV file, as it was minted when the job
    /// was created, so every read of the job reports the same locator. Absent
    /// when the create named no endpoint to build one from.
    pub pre_signed_url: Option<String>,
    pub creation_date: SystemTime,
    /// When `StartUserImportJob` accepted this job, absent until it does.
    pub start_date: Option<SystemTime>,
    /// The instant the clock owes this job's `Pending` -> `InProgress` move.
    /// Stored, never slept on: a read computes the status from it.
    pub in_progress_at: Option<SystemTime>,
    /// The instant the clock owes this job's `Stopping` -> `Stopped` move,
    /// absent until `StopUserImportJob` accepted it.
    pub stopped_at: Option<SystemTime>,
}

impl UserImportJob {
    /// A new job of `user_pool_id`, `Created` until it is started or the clock
    /// makes its expiry due.
    pub fn new(
        job_id: String,
        job_name: String,
        user_pool_id: String,
        cloud_watch_logs_role_arn: String,
        password_hashing_algorithm: Option<String>,
        pre_signed_url: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            job_id,
            job_name,
            user_pool_id,
            cloud_watch_logs_role_arn,
            password_hashing_algorithm,
            pre_signed_url,
            creation_date: now,
            start_date: None,
            in_progress_at: None,
            stopped_at: None,
        }
    }

    /// This job's `UserImportJobStatusType` as of `now`, computed from the
    /// stored instants: a stopped job is `Stopping` until the clock has made its
    /// stop due and `Stopped` from then on, a started one `Pending` until the
    /// clock has made the import due and `InProgress` from then on, and one that
    /// was never started `Created` until its expiry is due.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(stopped_at) = self.stopped_at {
            return if now >= stopped_at {
                JOB_STATUS_STOPPED
            } else {
                JOB_STATUS_STOPPING
            };
        }
        if let Some(in_progress_at) = self.in_progress_at {
            return if now >= in_progress_at {
                JOB_STATUS_IN_PROGRESS
            } else {
                JOB_STATUS_PENDING
            };
        }
        if now >= self.creation_date + JOB_EXPIRY_SPAN {
            JOB_STATUS_EXPIRED
        } else {
            JOB_STATUS_CREATED
        }
    }

    /// Whether this job is importing users as of `now` — `Pending` or
    /// `InProgress` — which is the state a stop acts on.
    pub fn is_running(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            JOB_STATUS_PENDING | JOB_STATUS_IN_PROGRESS
        )
    }

    /// Record that the import was started at `now`, which makes the job
    /// `Pending` and the `InProgress` move due one [`JOB_START_SPAN`] later.
    pub fn start(&mut self, now: SystemTime) {
        self.start_date = Some(now);
        self.in_progress_at = Some(now + JOB_START_SPAN);
    }

    /// Record that the stop was asked for at `now`, which makes the job
    /// `Stopping` and the `Stopped` move due one [`JOB_STOP_SPAN`] later.
    pub fn stop(&mut self, now: SystemTime) {
        self.stopped_at = Some(now + JOB_STOP_SPAN);
    }

    /// The modelled view of this job as of `now`, rendered from what is stored.
    pub fn to_user_import_job_type(&self, now: SystemTime) -> UserImportJobType {
        let status = self.status(now);
        UserImportJobType {
            job_name: Some(self.job_name.clone()),
            job_id: Some(self.job_id.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            // The locator the job was minted with, reported exactly as it is
            // stored, so a later read of the job names the same target.
            pre_signed_url: self.pre_signed_url.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            start_date: self.start_date.map(to_date_time),
            // "The date when the user import job was completed"
            // (`UserImportJobType.CompletionDate`): the instant the job stopped
            // importing users, reported once the clock has made that move due.
            completion_date: self
                .stopped_at
                .filter(|_| status == JOB_STATUS_STOPPED)
                .map(to_date_time),
            status: Some(status.to_string()),
            cloud_watch_logs_role_arn: Some(self.cloud_watch_logs_role_arn.clone()),
            // No CSV was imported, so the three counters stay at the zero the
            // model defaults them to, and `CompletionMessage` — "The message
            // returned when the user import job is completed" — is not invented.
            imported_users: 0,
            skipped_users: 0,
            failed_users: 0,
            completion_message: None,
            password_hashing_algorithm: self.password_hashing_algorithm.clone(),
        }
    }
}

/// One stored third-party identity provider of a user pool: what
/// `CreateIdentityProvider` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct IdentityProvider {
    pub user_pool_id: String,
    pub provider_name: String,
    pub provider_type: String,
    /// "The scopes, URLs, and identifiers for your external identity provider"
    /// (`IdentityProviderType.ProviderDetails`): an opaque key/value map, kept
    /// exactly as it was submitted.
    pub provider_details: HashMap<String, String>,
    /// "A mapping of IdP attributes to standard and custom user pool attributes"
    /// (`IdentityProviderType.AttributeMapping`). A provider created with no
    /// mapping holds an empty one.
    pub attribute_mapping: HashMap<String, String>,
    /// The IdP identifiers of this provider, in the order they were submitted. A
    /// provider created with no identifiers holds none.
    pub idp_identifiers: Vec<String>,
    /// The ACR level mapping, which only an OIDC provider carries, so it stays
    /// absent on a provider that was created without one.
    pub acr_mapping: Option<HashMap<String, String>>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl IdentityProvider {
    /// The modelled view of this provider, rendered from what is stored.
    pub fn to_identity_provider_type(&self) -> IdentityProviderType {
        IdentityProviderType {
            user_pool_id: Some(self.user_pool_id.clone()),
            provider_name: Some(self.provider_name.clone()),
            provider_type: Some(self.provider_type.clone()),
            provider_details: Some(self.provider_details.clone()),
            attribute_mapping: Some(self.attribute_mapping.clone()),
            idp_identifiers: Some(self.idp_identifiers.clone()),
            acr_mapping: self.acr_mapping.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// Every language `LinksType`'s key admits, in the order its target
/// `LanguageIdType` lists them
/// (`^cognito:(default|dutch|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional))$`
/// in this service's model). `cognito:default` is the one a language with no
/// entry of its own falls back to.
pub const LINK_LANGUAGES: [&str; 13] = [
    "cognito:default",
    "cognito:dutch",
    "cognito:english",
    "cognito:french",
    "cognito:spanish",
    "cognito:german",
    "cognito:bahasa-indonesia",
    "cognito:italian",
    "cognito:japanese",
    "cognito:korean",
    "cognito:portuguese-brazil",
    "cognito:chinese-simplified",
    "cognito:chinese-traditional",
];

/// The lowest-ordered key of a submitted `Links` map that is not one of
/// [`LINK_LANGUAGES`], or `None` when every key names a language managed login
/// localizes. Lowest-ordered so one request is refused over the same key every
/// time, whatever order the map arrived in.
pub fn unsupported_link_language<'links>(
    links: impl IntoIterator<Item = &'links String>,
) -> Option<&'links String> {
    links
        .into_iter()
        .filter(|language| !LINK_LANGUAGES.contains(&language.as_str()))
        .min()
}

/// One stored set of terms documents of a user pool: what `CreateTerms` stored
/// and every later read of it renders.
#[derive(Debug, Clone)]
pub struct Terms {
    pub terms_id: String,
    pub user_pool_id: String,
    pub client_id: String,
    pub terms_name: String,
    pub terms_source: String,
    pub enforcement: String,
    /// The language-to-URL map, ordered so a read of several languages is
    /// stable. `LinksType` admits 1..13 entries, so this map is never empty.
    pub links: BTreeMap<String, String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Terms {
    /// The modelled view of these terms documents, rendered from what is stored.
    pub fn to_terms_type(&self) -> TermsType {
        TermsType {
            terms_id: self.terms_id.clone(),
            user_pool_id: self.user_pool_id.clone(),
            client_id: self.client_id.clone(),
            terms_name: self.terms_name.clone(),
            terms_source: self.terms_source.clone(),
            enforcement: self.enforcement.clone(),
            links: self
                .links
                .iter()
                .map(|(language, url)| (language.clone(), url.clone()))
                .collect(),
            creation_date: to_date_time(self.creation_date),
            last_modified_date: to_date_time(self.last_modified_date),
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
    /// The one-time code this challenge is answered with, for a challenge whose
    /// start DELIVERED one (the email one-time password), and `None` for a
    /// challenge that is answered with something else.
    ///
    /// The code lives on the session because that is what bounds it: the
    /// challenge is answerable only while its session lasts
    /// (`CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes,
    /// of that session token"), so the code's window is the session's window.
    pub code: Option<VerificationCode>,
}

/// What resolving a `Session` token found
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::session`]).
/// An expired session is told apart from an unknown one because a challenge
/// response owes a different refusal for each.
#[derive(Debug, Clone)]
pub enum SessionLookup {
    /// The stored session, still inside its validity window.
    Found(AuthSession),
    /// A stored session whose validity window had already closed, as it was
    /// stored: which challenge it was waiting on decides the refusal it owes.
    Expired(AuthSession),
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

/// How many digits a minted verification code carries: Amazon Cognito's
/// verification messages carry a six-digit code.
const VERIFICATION_CODE_LENGTH: usize = 6;

/// How many digits a minted email one-time password carries: Amazon Cognito's
/// `EMAIL_OTP` sign-in challenge delivers an eight-digit one-time password.
const EMAIL_OTP_CODE_LENGTH: usize = 8;

/// A fresh verification code: [`VERIFICATION_CODE_LENGTH`] decimal digits.
pub fn new_verification_code() -> String {
    new_numeric_code(VERIFICATION_CODE_LENGTH)
}

/// A fresh email one-time password: [`EMAIL_OTP_CODE_LENGTH`] decimal digits.
pub fn new_email_otp_code() -> String {
    new_numeric_code(EMAIL_OTP_CODE_LENGTH)
}

/// A fresh code of `digits` decimal digits, which satisfies
/// `ConfirmationCodeType`'s modelled length 1..2048 and pattern `^[\S]+$` — the
/// shape the member that carries it back accepts.
///
/// The randomness is the same v4-UUID source every other minted value in this
/// file draws on ([`new_uuid`]), so nothing here is a hand-rolled generator: a
/// v4 UUID renders as 32 hex characters, each reduced to one decimal digit, so
/// one draw always yields the whole code and this makes no loop.
fn new_numeric_code(digits: usize) -> String {
    new_uuid()
        .chars()
        .filter_map(|character| character.to_digit(16))
        .filter_map(|value| char::from_digit(value % 10, 10))
        .take(digits)
        .collect()
}

/// The attribute that records whether `attribute`'s stored value was confirmed,
/// for the two attributes this API verifies (`VerifiedAttributeType`: `email`
/// and `phone_number`, whose flags are the standard `email_verified` and
/// `phone_number_verified`), or `None` for an attribute that carries no such
/// flag.
pub fn verified_flag(attribute: &str) -> Option<&'static str> {
    match attribute {
        "email" => Some("email_verified"),
        "phone_number" => Some("phone_number_verified"),
        _ => None,
    }
}

/// The value a verified flag carries once the attribute's value is confirmed.
/// `email_verified` and `phone_number_verified` are `Boolean` schema
/// attributes, whose values are carried as the strings every attribute value
/// is.
pub const FLAG_CONFIRMED: &str = "true";

/// The value a verified flag carries while the attribute's value is not
/// confirmed.
pub const FLAG_UNCONFIRMED: &str = "false";

/// The `DeliveryMediumType` a verification message for `attribute` travels by —
/// an email address is mailed and a phone number is texted — or `None` for an
/// attribute no message is sent for.
pub fn delivery_medium(attribute: &str) -> Option<&'static str> {
    match attribute {
        "email" => Some("EMAIL"),
        "phone_number" => Some("SMS"),
        _ => None,
    }
}

/// Where a password-reset message for one user goes
/// ([`UserPool::recovery_destination`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDestination {
    /// The attribute the message is addressed to, and the stored value it goes
    /// to.
    Delivered {
        attribute: String,
        destination: String,
    },
    /// This pool reserves password resets to administrators (`admin_only`), so
    /// no self-service message is sent at all.
    AdminOnly,
    /// The user holds no verified address or number for any mechanism this pool
    /// allows, so there is nowhere to send a reset code.
    None,
}

/// The `RecoveryOptionNameType` value that "prevents self-service account
/// recovery" (`RecoveryOptionType` in this service's model).
pub const RECOVERY_ADMIN_ONLY: &str = "admin_only";

/// The recovery mechanisms a pool that configures none gets, in priority order:
/// Amazon Cognito's default account recovery is the verified email address,
/// falling back to the verified phone number (the two attribute-bearing
/// `RecoveryOptionNameType` values).
const DEFAULT_RECOVERY_MECHANISMS: [&str; 2] = ["verified_email", "verified_phone_number"];

/// The user attribute a `RecoveryOptionNameType` recovers through, or `None` for
/// a mechanism that names no attribute (`admin_only`).
fn recovered_attribute(mechanism: &str) -> Option<&'static str> {
    match mechanism {
        "verified_email" => Some("email"),
        "verified_phone_number" => Some("phone_number"),
        _ => None,
    }
}

/// How many trailing characters of a masked phone number stay readable, which
/// is what identifies the number to the user who gave it.
const UNMASKED_DESTINATION_SUFFIX: usize = 4;

/// `destination` as a `CodeDeliveryDetailsType.Destination`.
///
/// Amazon Cognito reports the address it sent the code to MASKED, so the reply
/// tells the caller which of the user's addresses was used without disclosing
/// an address the caller does not already hold. An email address keeps the
/// first character of its local part, the first of its domain and the domain's
/// last label; anything else keeps a leading `+` and its last
/// [`UNMASKED_DESTINATION_SUFFIX`] characters.
pub fn masked_destination(destination: &str) -> String {
    if let Some((local, domain)) = destination.split_once('@') {
        let local_initial = local.chars().next().unwrap_or('*');
        let domain_initial = domain.chars().next().unwrap_or('*');
        let last_label = domain.rfind('.').map_or("", |dot| &domain[dot..]);
        return format!("{local_initial}***@{domain_initial}***{last_label}");
    }
    let characters: Vec<char> = destination.chars().collect();
    let readable_from = characters
        .len()
        .saturating_sub(UNMASKED_DESTINATION_SUFFIX);
    characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            if index >= readable_from || (index == 0 && *character == '+') {
                *character
            } else {
                '*'
            }
        })
        .collect()
}

/// The delivery details for a verification message sent to `value` for
/// `attribute`, or `None` for an attribute no message is sent for.
///
/// Rendered from the value the message goes to, masked as
/// [`masked_destination`] reports it.
pub fn code_delivery_details(attribute: &str, value: &str) -> Option<CodeDeliveryDetailsType> {
    Some(CodeDeliveryDetailsType {
        destination: Some(masked_destination(value)),
        delivery_medium: Some(delivery_medium(attribute)?.to_string()),
        attribute_name: Some(attribute.to_string()),
    })
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
        if let Some((_, entry)) = custom_schema_attribute(&attribute) {
            schema.push(entry);
        }
    }
    schema
}

/// The STORED NAME and the schema entry a requested CUSTOM attribute becomes,
/// with the defaults a custom attribute carries filled in.
///
/// Its stored name is the requested name under the prefix Amazon Cognito gives
/// it: "When you add an attribute with a `Name` value of `MyAttribute`, Amazon
/// Cognito creates the custom attribute `custom:MyAttribute`. When
/// `DeveloperOnlyAttribute` is `true`, Amazon Cognito creates your attribute as
/// `dev:MyAttribute`" (`SchemaAttributeType.Name`, this service's model). A name
/// that already carries one of those prefixes is stored as it is, so a schema
/// read and a re-add agree on the stored name.
///
/// `None` when the request names no attribute: an entry with no name could never
/// be found again. `CustomAttributeNameType` has a minimum length of 1, so only
/// a request the model does not admit reaches that answer.
pub fn custom_schema_attribute(
    requested: &SchemaAttributeType,
) -> Option<(String, SchemaAttributeType)> {
    let name = requested.name.clone()?;
    if name.is_empty() {
        return None;
    }
    let developer_only = requested.developer_only_attribute.unwrap_or(false);
    let prefix = if developer_only { "dev:" } else { "custom:" };
    let stored_name = if name.starts_with("custom:") || name.starts_with("dev:") {
        name
    } else {
        format!("{prefix}{name}")
    };
    let entry = SchemaAttributeType {
        name: Some(stored_name.clone()),
        attribute_data_type: Some(
            requested
                .attribute_data_type
                .clone()
                .unwrap_or_else(|| "String".to_string()),
        ),
        developer_only_attribute: Some(developer_only),
        // A custom attribute is mutable unless the request says otherwise;
        // a custom attribute can never be required.
        mutable: Some(requested.mutable.unwrap_or(true)),
        required: Some(false),
        number_attribute_constraints: requested.number_attribute_constraints.clone(),
        string_attribute_constraints: requested.string_attribute_constraints.clone(),
    };
    Some((stored_name, entry))
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
        identity_providers: BTreeMap::new(),
        terms: BTreeMap::new(),
        user_import_jobs: BTreeMap::new(),
        // A pool's token-signing key exists from the moment the pool does, as
        // on AWS, so its key set is publishable before anything signs in. One
        // scalar multiplication, so creating a pool waits on nothing.
        signing_key: PoolSigningKey::generate(),
        device_group_key: new_device_group_key(),
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

/// A fresh `JobId` for a user import job: `import-` followed by a v4 UUID,
/// which satisfies the modelled pattern `^import-[0-9a-zA-Z-]+$` and length
/// 1..55.
pub fn new_user_import_job_id() -> String {
    format!("import-{}", new_uuid())
}

/// Where a client uploads the CSV file of the import job `job_id`:
/// `UserImportJobType.PreSignedUrl`, "The pre-signed URL target for uploading
/// the CSV file".
///
/// AWS answers an Amazon S3 pre-signed URL here. This emulator provisions no
/// object store, so the URL names THIS job's CSV on the endpoint THIS REQUEST
/// ARRIVED AT: no operation of this service serves it, and nothing reads it back
/// as content. It is a locator built from the request's own host and the stored
/// ids, never a stand-in for a CSV that was uploaded — and a request that names
/// no host of its own gets no locator at all rather than a baked-in one, which
/// would point a following client's upload at the real service.
///
/// `host` is taken only when it LOOKS like a `host[:port]`, so a caller cannot
/// get a string of its own into the value this service reports.
pub fn user_import_csv_upload_url(host: &str, user_pool_id: &str, job_id: &str) -> Option<String> {
    is_host_shaped(host).then(|| format!("https://{host}/user-import-jobs/{user_pool_id}/{job_id}"))
}

/// Whether `host` is a non-empty `host[:port]` of the characters a hostname
/// carries.
fn is_host_shaped(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':'))
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

/// The message a `SecretHash` authenticates: "username plus the client ID in
/// the message" (`SignUp.SecretHash` in this service's model), in that order, so
/// the hash of one username is not the hash of another and the hash for one app
/// client is not the hash for another.
fn secret_hash_message(username: &str, client_id: &str) -> String {
    format!("{username}{client_id}")
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

/// A fresh `DeviceKey` for a device of a pool in `region`: `<region>_<v4 UUID>`,
/// which satisfies the modelled pattern `^[\w-]+_[0-9a-f-]+$` — `region` carries
/// only the characters [`sanitize_region`] admits, and a v4 UUID is lowercase
/// hex and hyphens.
///
/// The prefix is clamped to [`MAX_DEVICE_KEY_PREFIX`] so the key stays inside
/// the modelled length 1..55 whatever region the pool was created in: the
/// region comes from the caller's credential scope, so its length is not this
/// service's to assume.
fn new_device_key(region: &str) -> String {
    let prefix = region
        .char_indices()
        .take_while(|(at, _)| *at < MAX_DEVICE_KEY_PREFIX)
        .map(|(_, c)| c)
        .collect::<String>();
    format!("{prefix}_{}", new_uuid())
}

/// How many characters of the region a minted `DeviceKey` carries ahead of its
/// separator: `DeviceKeyType`'s modelled length is 1..55 and the UUID after the
/// separator is 36 characters long.
const MAX_DEVICE_KEY_PREFIX: usize = 55 - 1 - 36;

/// A fresh `DeviceGroupKey` for a user pool: 9 characters, inside the modelled
/// length 0..131072.
fn new_device_group_key() -> String {
    random_token(9)
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
