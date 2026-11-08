// SPDX-License-Identifier: CC0-1.0

//! The resources a user pool holds beside its users: its hosted domains, its
//! replicas and its app clients.

use super::*;

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
    /// The instant the clock owes this domain's move to `ACTIVE`. Stored, never
    /// slept on: a read computes the status from it.
    pub active_at: SystemTime,
    /// The `DomainStatusType` this domain reports until [`Self::active_at`] is
    /// due: `CREATING` for a domain that was just created, `UPDATING` for one
    /// whose configuration was just changed.
    pub status_until_active: &'static str,
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
            status_until_active: DOMAIN_STATUS_CREATING,
        }
    }

    /// Whether this is a custom domain rather than an Amazon Cognito prefix one.
    pub fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }

    /// Record that this domain's configuration changed at `now`, which the
    /// service does not serve instantly: "Changes to the domain branding version
    /// take up to one minute to take effect for a prefix domain and up to five
    /// minutes for a custom domain" (`UpdateUserPoolDomain`), so the domain is
    /// `UPDATING` until the clock makes it `ACTIVE` again one
    /// [`DOMAIN_ACTIVATION_SPAN`] from `now`.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.active_at = now + DOMAIN_ACTIVATION_SPAN;
        self.status_until_active = DOMAIN_STATUS_UPDATING;
    }

    /// This domain's `DomainStatusType` as of `now`: the status its last write
    /// left it in until the clock has made its activation due, `ACTIVE` from
    /// then on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            DOMAIN_STATUS_ACTIVE
        } else {
            self.status_until_active
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

/// One stored SECONDARY replica of a user pool: what `CreateUserPoolReplica`
/// persisted and every later read of that replica renders.
///
/// The PRIMARY replica is the pool itself in [`UserPool::region`], rendered by
/// [`UserPool::to_primary_replica`], so it is not stored here a second time.
#[derive(Debug, Clone)]
pub struct UserPoolReplica {
    /// The Region the replica was created in, which is this resource's key.
    pub region_name: String,
    /// The ARN of the replica user pool: this pool's id under the replica's
    /// Region ([`UserPool::arn_in_region`]).
    pub user_pool_arn: String,
    /// The tags the create submitted. "You can maintain tags independently on
    /// replica user pools" (`CreateUserPoolReplica.UserPoolTags`), so these are
    /// the replica's own rather than the pool's.
    pub tags: HashMap<String, String>,
    /// When the replica was created.
    pub creation_date: SystemTime,
    /// The instant the clock owes this replica's `CREATING` -> `INACTIVE` move.
    /// Stored, never slept on: a read computes the status from it.
    pub active_at: SystemTime,
    /// The status an `UpdateUserPoolReplica` call last wrote, which SUPERSEDES
    /// the clock-owed one: that operation is how a caller "modif[ies] the
    /// status to activate or deactivate the replica", so the status it set is
    /// the status every later read reports. `None` until a call sets one.
    pub requested_status: Option<String>,
    /// The instant the clock owes this replica's removal, once a delete has
    /// started; until then the replica reports [`REPLICA_STATUS_DELETING`], and
    /// from then on it is gone.
    pub removal_due: Option<SystemTime>,
}

impl UserPoolReplica {
    /// A new secondary replica in `region_name`, `CREATING` until the clock
    /// makes its activation due one [`REPLICA_ACTIVATION_SPAN`] from `now`.
    pub fn new(
        region_name: String,
        user_pool_arn: String,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            region_name,
            user_pool_arn,
            tags,
            creation_date: now,
            active_at: now + REPLICA_ACTIVATION_SPAN,
            requested_status: None,
            removal_due: None,
        }
    }

    /// This replica's `ReplicaStatusType` as of `now`: `DELETING` once a delete
    /// has started, else the status a call last set, else `CREATING` until the
    /// clock has made its activation due and `INACTIVE` from then on — "The
    /// replica has been created, but is not accepting requests for end-users"
    /// (`UserPoolReplicaType.Status`), which `UpdateUserPoolReplica` is what
    /// activates.
    pub fn status(&self, now: SystemTime) -> &str {
        if self.removal_due.is_some() {
            return REPLICA_STATUS_DELETING;
        }
        if let Some(status) = &self.requested_status {
            return status;
        }
        if now >= self.active_at {
            REPLICA_STATUS_INACTIVE
        } else {
            REPLICA_STATUS_CREATING
        }
    }

    /// Whether the clock has made this replica's removal due, which is when a
    /// started deletion has finished.
    pub fn removed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Start this replica's deletion: it reports [`REPLICA_STATUS_DELETING`]
    /// until the clock makes [`REPLICA_DELETION_SPAN`] elapse, and is gone from
    /// then on.
    pub fn start_deletion(&mut self, now: SystemTime) {
        self.removal_due = Some(now + REPLICA_DELETION_SPAN);
    }

    /// The modelled view of this replica, rendered from what is stored.
    pub fn to_user_pool_replica_type(&self, now: SystemTime) -> UserPoolReplicaType {
        UserPoolReplicaType {
            region_name: Some(self.region_name.clone()),
            status: Some(self.status(now).to_string()),
            role: Some(REPLICA_ROLE_SECONDARY.to_string()),
            user_pool_arn: Some(self.user_pool_arn.clone()),
        }
    }
}

/// How many client secrets one app client holds at a time:
/// `AddUserPoolClientSecret` "supports up to 2 active secrets per app client for
/// zero-downtime credential rotation workflows" (that operation in this
/// service's model), so the cap is the API's rather than this emulator's.
pub const MAX_CLIENT_SECRETS_PER_CLIENT: usize = 2;

/// The message a `ResourceNotFoundException` carries when the `ClientSecretId` a
/// request names is not one the named app client currently holds a secret for.
///
/// Amazon Cognito answers the same sentence for every secret id — it reports
/// that the secret lookup resolved nothing and quotes neither the id, the app
/// client nor the pool, exactly as [`USER_NOT_FOUND_MESSAGE`] and
/// [`TERMS_NOT_FOUND_MESSAGE`] report their own unresolved lookups, and unlike
/// those two it carries no trailing period. So an id no add ever minted, an id
/// another app client holds and an id an earlier delete already took away are
/// all reported identically, and the refusal never echoes the caller's secret
/// identifier back to it.
pub const CLIENT_SECRET_NOT_FOUND_MESSAGE: &str = "Client secret does not exist";

/// The message `DeleteUserPoolClientSecret`'s `InvalidParameterException`
/// carries when the secret the request names is the only one the app client
/// still holds: "you cannot delete the last remaining secret for an app client"
/// (that operation in this service's model), so an app client never loses the
/// credential that authenticates it.
///
/// Like [`CLIENT_SECRET_NOT_FOUND_MESSAGE`] it is one fixed sentence: it reports
/// the precondition that failed and quotes neither the `ClientSecretId` asked
/// for, the app client nor the pool, so rotation code that branches on this
/// refusal sees the same text for every client, and no `@sensitive` `ClientId`
/// reaches the error string.
pub const LAST_CLIENT_SECRET_MESSAGE: &str = "Cannot delete the only client secret.";

/// One active client secret of an app client: the credential a confidential app
/// client authenticates itself with, beside the id and creation time a listing
/// reports it by.
#[derive(Debug, Clone)]
pub struct ClientSecret {
    /// The secret's unique id. "This identifier follows the format
    /// <client-id>--<epoch-create-time>"
    /// (`ClientSecretDescriptorType.ClientSecretId`), minted by
    /// [`ClientSecret::new`].
    pub id: String,
    /// The secret value itself: either the one the request supplied or one this
    /// emulator generated.
    pub value: String,
    /// When this secret was created.
    pub create_date: SystemTime,
}

impl ClientSecret {
    /// A secret of the app client `client_id` holding `value`, created at `now`.
    ///
    /// The id is "`<client-id>--<epoch-create-time>`"
    /// (`ClientSecretDescriptorType.ClientSecretId`), with the creation time
    /// counted in milliseconds since the epoch so two secrets rotated within one
    /// second still carry different ids.
    pub fn new(client_id: &str, value: String, now: SystemTime) -> Self {
        let created_millis = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            id: format!("{client_id}--{created_millis}"),
            value,
            create_date: now,
        }
    }

    /// The modelled view of this secret, rendered from what is stored.
    ///
    /// `ClientSecretValue` is left out: "this is only returned when creating a new
    /// secret and only if Amazon Cognito generated the secret"
    /// (`ClientSecretDescriptorType.ClientSecretValue`), so the call that
    /// generates a value is the only one that carries it and every later read
    /// renders the descriptor without it.
    pub fn to_client_secret_descriptor_type(&self) -> ClientSecretDescriptorType {
        ClientSecretDescriptorType {
            client_secret_id: Some(self.id.clone()),
            client_secret_value: None,
            client_secret_create_date: Some(to_date_time(self.create_date)),
        }
    }
}

/// The message a `ResourceNotFoundException` carries when the `ClientId`
/// `DescribeUserPoolClient` reads or `UpdateUserPoolClient` writes names no app
/// client of this emulator at all — an id no create minted, or one an earlier
/// `DeleteUserPoolClient` took away.
///
/// These two operations report that lookup without quoting the id: the sentence
/// is fixed text in full, exactly as [`USER_NOT_FOUND_MESSAGE`] and
/// [`GROUP_NOT_FOUND_MESSAGE`] report their own unresolved lookups, so a
/// `@sensitive` `ClientId` (`ClientIdType` in this service's model) reaches no
/// error string.
///
/// An id that DOES name an app client, of another pool, is a different failure
/// and carries a different sentence — see
/// [`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::app_client_lookup_refusal`].
pub const APP_CLIENT_NOT_FOUND_MESSAGE: &str = "User pool client does not exist.";

/// The sentence the operations that report an unresolved app-client lookup BY
/// ITS `ClientId` carry — the ones that resolve a client on the way to another
/// resource (its terms documents, its branding, its UI customization, its
/// secrets), the authentication operations that take a `UserPoolId` beside the
/// `ClientId`, and `DeleteUserPoolClient`, which names the id it was asked to
/// remove.
///
/// The fixed text is `User pool client ` and ` does not exist.`; the one
/// variable part is the `ClientId` the REQUEST carried.
pub fn app_client_not_found_message(client_id: &str) -> String {
    format!("User pool client {client_id} does not exist.")
}

/// The sentence every operation carries when the `ClientId` it was given names
/// an app client that exists but is NOT in the pool the request named.
///
/// The fixed text is `User pool client `, ` does not exist in user pool ` and
/// the trailing `.`; the two variable parts are the `ClientId` and the
/// `UserPoolId` the REQUEST carried, never a value read out of the store.
pub fn app_client_not_found_in_pool_message(client_id: &str, user_pool_id: &str) -> String {
    format!("User pool client {client_id} does not exist in user pool {user_pool_id}.")
}

/// One stored app client of a user pool.
#[derive(Debug, Clone)]
pub struct UserPoolClient {
    pub client_id: String,
    /// This app client's active client secrets, oldest first. A confidential app
    /// client is created holding one, and `AddUserPoolClientSecret` "supports up
    /// to 2 active secrets per app client for zero-downtime credential rotation
    /// workflows" (that operation in this service's model), so a client holds a
    /// LIST of them and each one it holds authenticates a request. A public app
    /// client — one created with neither `GenerateSecret` nor a `ClientSecret` —
    /// holds none.
    pub client_secrets: Vec<ClientSecret>,
    pub user_pool_id: String,
    pub client_name: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// The effective `RefreshTokenValidity`: never `0`, which the model
    /// documents as overridden by the service default
    /// ([`effective_refresh_token_validity`]).
    pub refresh_token_validity: i32,
    pub access_token_validity: Option<i32>,
    pub id_token_validity: Option<i32>,
    /// The app client's time units. Every app client has a block — empty when no
    /// unit was overridden ([`effective_token_validity_units`]).
    pub token_validity_units: TokenValidityUnitsType,
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
    /// The effective `AuthSessionValidity` in minutes: the value asked for, or
    /// the service default ([`effective_auth_session_validity`]).
    pub auth_session_validity: i32,
    pub refresh_token_rotation: Option<RefreshTokenRotationType>,
    /// This app client's own threat-protection settings, once a call has set
    /// them: `RiskConfigurationType.ClientId` is "the app client where this
    /// configuration is applied", and a client that holds none uses the pool's
    /// — "to reset the threat protection settings of an app client to be
    /// inherited from the user pool, send `UserPoolId` and `ClientId`
    /// parameters only" (`SetRiskConfiguration`), which stores a configuration
    /// with nothing in it.
    pub risk_configuration: Option<RiskConfiguration>,
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

    /// Whether this app client's stored `ExplicitAuthFlows` enable
    /// machine-to-machine authorization: "to use this operation, you must
    /// configure the app client with a client secret and enable the
    /// `ALLOW_CLIENT_TOKEN_AUTH` authentication flow" (`GetClientToken` in this
    /// service's model). The client secret is the other half of that
    /// configuration, and it is what authenticates the request.
    pub fn allows_client_token_auth(&self) -> bool {
        self.allows_any_of(&[CLIENT_TOKEN_AUTH_FLOW])
    }

    /// Whether this client's stored `ExplicitAuthFlows` carry any of `flows`.
    pub(super) fn allows_any_of(&self, flows: &[&str]) -> bool {
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
    /// A client that holds SEVERAL secrets accepts the tag ANY of them produces:
    /// each one is an active credential of the app client, which is what lets a
    /// caller move from one to the next without an outage.
    pub fn accepts_secret_hash(&self, username: &str, presented: Option<&str>) -> bool {
        if self.client_secrets.is_empty() {
            return true;
        }
        let Some(presented) = presented else {
            return false;
        };
        let Ok(tag) = ::aws_smithy_types::base64::decode(presented) else {
            return false;
        };
        let message = secret_hash_message(username, &self.client_id);
        self.client_secrets.iter().any(|secret| {
            let key = ::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, secret.value.as_bytes());
            ::ring::hmac::verify(&key, message.as_bytes(), &tag).is_ok()
        })
    }

    /// Whether `presented` is the `ClientSecret` this app client requires a request
    /// to carry.
    ///
    /// `ClientSecret` is "the client secret of the requested app client, if the
    /// client has a secret" (this service's model), so a client that holds NO
    /// secret has nothing for a request to present and every request to it is
    /// accepted, while one that holds a secret accepts only that value — a request
    /// that carries none, or another client's secret, proves nothing about holding
    /// it. The comparison is a constant-time one, so a rejected secret tells a
    /// caller nothing about how much of it was right. A client that holds SEVERAL
    /// secrets accepts any one of them: each is an active credential of the app
    /// client.
    pub fn accepts_client_secret(&self, presented: Option<&str>) -> bool {
        if self.client_secrets.is_empty() {
            return true;
        }
        presented.is_some_and(|presented| {
            self.client_secrets.iter().any(|secret| {
                bool::from(::subtle::ConstantTimeEq::ct_eq(
                    presented.as_bytes(),
                    secret.value.as_bytes(),
                ))
            })
        })
    }

    /// The app client secret a read of this client renders: its oldest active
    /// secret, which is the one the create minted or accepted, or `None` for a
    /// public app client that holds none.
    ///
    /// `UserPoolClientType.ClientSecret` is "the app client secret" (this
    /// service's model) — one value, while an app client under rotation holds up
    /// to [`MAX_CLIENT_SECRETS_PER_CLIENT`], so the member renders the secret the
    /// client already had and a later read of the SAME client still renders it.
    pub fn client_secret(&self) -> Option<&str> {
        self.client_secrets
            .first()
            .map(|secret| secret.value.as_str())
    }

    /// Whether this app client already holds every secret it may hold at once,
    /// which is what makes one more [`MAX_CLIENT_SECRETS_PER_CLIENT`] exceeded.
    pub fn client_secrets_at_limit(&self) -> bool {
        self.client_secrets.len() >= MAX_CLIENT_SECRETS_PER_CLIENT
    }

    /// Store `value` as one more active secret of this app client, created at
    /// `now`, and answer the record as stored.
    ///
    /// Secrets are appended, so the order they were created in is the order a
    /// read reports them in and [`Self::client_secret`] keeps naming the same
    /// one. The minted id names this app client and this instant, and the instant
    /// is carried forward until no secret of this client already holds that id,
    /// so one id never names two secrets.
    pub fn add_client_secret(&mut self, value: String, now: SystemTime) -> ClientSecret {
        let mut secret = ClientSecret::new(&self.client_id, value, now);
        while self.client_secrets.iter().any(|held| held.id == secret.id) {
            let created = secret.create_date + Duration::from_millis(1);
            secret = ClientSecret::new(&self.client_id, secret.value, created);
        }
        self.client_secrets.push(secret.clone());
        secret
    }

    /// How long a refresh token this client rotates out stays valid for retries, or
    /// `None` when this client does not rotate refresh tokens at all.
    ///
    /// `RefreshTokenRotationType` is "the configuration of your app client for
    /// refresh token rotation. When enabled, your app client issues new ID, access,
    /// and refresh tokens when users renew their sessions with refresh tokens. When
    /// disabled, token refresh issues only ID and access tokens", and
    /// `RetryGracePeriodSeconds` "is `0`" means "the grace period is disabled and a
    /// successful request immediately invalidates the submitted refresh token"
    /// (this service's model). A client that stores no rotation configuration, and
    /// one whose `Feature` is not switched on, rotates nothing; one that rotates
    /// and asked for no grace period gets none. The modelled range of
    /// `RetryGracePeriodSeconds` starts at 0, so the boundary validator has already
    /// refused a negative value before any handler reads one.
    pub fn refresh_token_rotation_grace(&self) -> Option<Duration> {
        let rotation = self.refresh_token_rotation.as_ref()?;
        if rotation.feature != FEATURE_ENABLED {
            return None;
        }
        let seconds = rotation
            .retry_grace_period_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .unwrap_or_default();
        Some(Duration::from_secs(seconds))
    }

    /// The access token lifetime this client issues.
    pub fn access_token_ttl(&self) -> Duration {
        token_ttl(
            self.access_token_validity,
            self.token_validity_units.access_token.as_deref(),
            // "The default time unit for AccessTokenValidity in an API request is hours."
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The id token lifetime this client issues.
    pub fn id_token_ttl(&self) -> Duration {
        token_ttl(
            self.id_token_validity,
            self.token_validity_units.id_token.as_deref(),
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The refresh token lifetime this client issues. The stored validity is
    /// already the effective one — never the `0` the model documents as
    /// overridden — so it is what the lifetime is computed from.
    pub fn refresh_token_ttl(&self) -> Duration {
        token_ttl(
            Some(self.refresh_token_validity),
            self.token_validity_units.refresh_token.as_deref(),
            // "The default time unit for RefreshTokenValidity in an API request is days."
            TokenValidityUnit::Days,
            DEFAULT_REFRESH_TOKEN_TTL,
        )
    }

    /// How long a challenge session this client starts stays valid. The stored
    /// duration is read at no less than the documented minimum, so a session
    /// window is never zero-length.
    pub fn auth_session_validity(&self) -> Duration {
        let minutes = self
            .auth_session_validity
            .max(DEFAULT_AUTH_SESSION_VALIDITY_MINUTES);
        Duration::from_secs(u64::try_from(minutes).unwrap_or_default() * 60)
    }

    /// The modelled view of this client, rendered from what is stored.
    pub fn to_user_pool_client_type(&self) -> UserPoolClientType {
        UserPoolClientType {
            user_pool_id: Some(self.user_pool_id.clone()),
            client_name: Some(self.client_name.clone()),
            client_id: Some(self.client_id.clone()),
            client_secret: self.client_secret().map(ToString::to_string),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            refresh_token_validity: self.refresh_token_validity,
            access_token_validity: self.access_token_validity,
            id_token_validity: self.id_token_validity,
            token_validity_units: Some(self.token_validity_units.clone()),
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
            auth_session_validity: Some(self.auth_session_validity),
            refresh_token_rotation: self.refresh_token_rotation.clone(),
        }
    }
}
