// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssooidc-awsssooidcservice service.
//!
//! IAM Identity Center OIDC serves the OAuth 2.0 device-authorization grant
//! (RFC 8628) and the refresh-token grant, so the state it keeps is the three
//! long-lived things those flows create — registered clients, device
//! authorizations and issued tokens — plus the indexes that let a later call
//! resolve a record from the opaque value an earlier call handed out.
//!
//! The helpers here are shared by every operation of this service (minting an
//! opaque value, authenticating a client, resolving the request's region), so
//! they live beside the state rather than being written once per handler.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The only `clientType` the service accepts: "The service supports only public
/// as a client type. Anything other than public will be rejected by the
/// service." (`RegisterClientRequest$clientType` documentation).
pub const CLIENT_TYPE_PUBLIC: &str = "public";

/// The OAuth 2.0 authorization-code grant, as `grantTypes` names it.
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";
/// The OAuth 2.0 device-code grant, as `grantTypes` names it.
pub const GRANT_TYPE_DEVICE_CODE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// The OAuth 2.0 refresh-token grant, as `grantTypes` names it.
pub const GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";

/// Every grant type the service supports, from `RegisterClientRequest$grantTypes`:
/// "Supports the following OAuth 2.0 grant types: Authorization Code, Device
/// Code, and Refresh Token. * Authorization Code - authorization_code * Device
/// Code - urn:ietf:params:oauth:grant-type:device_code * Refresh Token -
/// refresh_token"
/// (`smithy_query(query="doc",shape="com.amazonaws.ssooidc#RegisterClientRequest",member="grantTypes")`).
pub const SUPPORTED_GRANT_TYPES: &[&str] = &[
    GRANT_TYPE_AUTHORIZATION_CODE,
    GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_REFRESH_TOKEN,
];

/// The OAuth 2.0 JWT-bearer grant, as `CreateTokenWithIAMRequest$grantType`
/// names it: "JWT Bearer - urn:ietf:params:oauth:grant-type:jwt-bearer".
pub const GRANT_TYPE_JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// The OAuth 2.0 token-exchange grant (RFC 8693), as
/// `CreateTokenWithIAMRequest$grantType` names it: "Token Exchange -
/// urn:ietf:params:oauth:grant-type:token-exchange".
pub const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// Every grant type `CreateTokenWithIAM` serves, from
/// `CreateTokenWithIAMRequest$grantType`: "Supports the following OAuth grant
/// types: Authorization Code, Refresh Token, JWT Bearer, and Token Exchange. ...
/// * Authorization Code - authorization_code * Refresh Token - refresh_token *
/// JWT Bearer - urn:ietf:params:oauth:grant-type:jwt-bearer * Token Exchange -
/// urn:ietf:params:oauth:grant-type:token-exchange". The device-code grant is
/// NOT among them: that one is `CreateToken`'s
/// (`smithy_query(query="doc",shape="com.amazonaws.ssooidc#CreateTokenWithIAMRequest",member="grantType")`).
pub const IAM_SUPPORTED_GRANT_TYPES: &[&str] = &[
    GRANT_TYPE_AUTHORIZATION_CODE,
    GRANT_TYPE_REFRESH_TOKEN,
    GRANT_TYPE_JWT_BEARER,
    GRANT_TYPE_TOKEN_EXCHANGE,
];

/// The `TokenTypeURI` of an access token: "Access Token -
/// urn:ietf:params:oauth:token-type:access_token"
/// (`CreateTokenWithIAMRequest$requestedTokenType`).
pub const TOKEN_TYPE_URI_ACCESS_TOKEN: &str = "urn:ietf:params:oauth:token-type:access_token";

/// The `TokenTypeURI` of a refresh token: "Refresh Token -
/// urn:ietf:params:oauth:token-type:refresh_token"
/// (`CreateTokenWithIAMRequest$requestedTokenType`).
pub const TOKEN_TYPE_URI_REFRESH_TOKEN: &str = "urn:ietf:params:oauth:token-type:refresh_token";

/// Every token type `CreateTokenWithIAM` can be asked to issue, from
/// `CreateTokenWithIAMRequest$requestedTokenType`: "The following values are
/// supported: * Access Token - urn:ietf:params:oauth:token-type:access_token *
/// Refresh Token - urn:ietf:params:oauth:token-type:refresh_token".
pub const ISSUABLE_TOKEN_TYPE_URIS: &[&str] =
    &[TOKEN_TYPE_URI_ACCESS_TOKEN, TOKEN_TYPE_URI_REFRESH_TOKEN];

/// `tokenType` of every token this service issues: the access token "can be used
/// to ... access application APIs using `bearer` authentication" (`CreateToken`
/// documentation).
pub const TOKEN_TYPE_BEARER: &str = "Bearer";

/// How long a registration's `clientSecret` stays usable. CHOSEN by this
/// emulator (the model documents only that `clientSecretExpiresAt` "indicates
/// the time at which the clientId and clientSecret will become invalid"), long
/// enough that a registration outlives any emulator session.
pub const CLIENT_SECRET_LIFETIME: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// `expiresIn` of a device authorization, in seconds. CHOSEN by this emulator:
/// RFC 8628 §3.2 leaves the lifetime to the server, and ten minutes is long
/// enough for a human to read a user code and short enough that a stale code
/// expires within one test run.
pub const DEVICE_CODE_EXPIRES_IN: i32 = 600;

/// `interval` a client must wait between polls, in seconds: RFC 8628 §3.5 fixes
/// the default polling interval at 5 seconds when the server names none.
pub const DEVICE_CODE_INTERVAL: i32 = 5;

/// `expiresIn` of an issued access token, in seconds. CHOSEN by this emulator;
/// the model documents no lifetime.
pub const ACCESS_TOKEN_EXPIRES_IN: i32 = 3600;

/// How long an issued refresh token stays usable, in seconds. CHOSEN by this
/// emulator, and deliberately several access-token lifetimes long: the point of
/// the refresh-token grant is that a client whose access token has already
/// expired can still exchange its refresh token for a new pair, so the refresh
/// token has its own lifetime rather than the access token's.
pub const REFRESH_TOKEN_EXPIRES_IN: i32 = 8 * 3600;

/// How long after `StartDeviceAuthorization` the pending authorization becomes
/// approved. The emulator has no browser to approve it in, so the clock owes the
/// `Pending -> Approved` transition the way a human would have made it: CHOSEN
/// at half a second, so a client polling straight away still observes
/// `AuthorizationPendingException` and its next poll (at `interval`) succeeds.
pub const APPROVAL_DELAY: Duration = Duration::from_millis(500);

/// How long an expired device authorization is kept before it is reclaimed, so a
/// client polling a stale device code still gets `ExpiredTokenException` rather
/// than a bare invalid grant. CHOSEN: one further `expiresIn`.
pub const EXPIRED_RETENTION: Duration = Duration::from_secs(DEVICE_CODE_EXPIRES_IN as u64);

/// The ceiling on how many registrations, device authorizations and access tokens
/// this store holds: the caps that bound caller-driven growth.
///
/// A generated emulator keeps its state in memory for the life of the process and
/// every one of these stores grows on a caller's request, so each needs a maximum
/// entry count beside the reclamation below (`purge_expired_clients`,
/// `purge_spent_device_authorizations`, `purge_expired_tokens`, and the superseded
/// access token dropped by `rotate_refresh_token`). The three operations that grow
/// a store refuse past these caps with `SlowDownException` — the error all three
/// declare for a request that "is more than the service can handle" — rather than
/// inserting unconditionally. The numbers are the emulator's own bound, not a
/// published AWS quota, set high enough that no ordinary workflow meets one.
pub const MAX_CLIENTS: usize = 5_000;

/// See [`MAX_CLIENTS`].
pub const MAX_DEVICE_AUTHORIZATIONS: usize = 5_000;

/// See [`MAX_CLIENTS`].
pub const MAX_ACCESS_TOKENS: usize = 5_000;

/// See [`MAX_CLIENTS`].
pub const MAX_REFRESH_TOKENS: usize = 5_000;

/// How many times a fresh user code is drawn before the service gives up, so the
/// draw cannot spin while the state lock is held.
pub const MAX_USER_CODE_ATTEMPTS: u32 = 32;

/// The region used when a request names none: the region the emulator's own
/// generated client configuration defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// A client registration minted by `RegisterClient`.
#[derive(Clone)]
pub struct OidcClient {
    pub client_id: String,
    pub client_secret: String,
    pub client_name: String,
    pub client_type: String,
    pub scopes: Vec<String>,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub issuer_url: Option<String>,
    pub client_id_issued_at: i64,
    pub client_secret_expires_at: i64,
}

impl OidcClient {
    /// Whether this registration may use `grant_type`. "This list is used to
    /// restrict the token granting flows available to the client"
    /// (`RegisterClientRequest$grantTypes`), so a registration that named no
    /// grant type restricts nothing.
    pub fn allows_grant(&self, grant_type: &str) -> bool {
        self.grant_types.is_empty() || self.grant_types.iter().any(|named| named == grant_type)
    }

    /// Whether this registration may be granted `scope`. "Upon authorization,
    /// this list is used to restrict permissions when granting an access token"
    /// (`RegisterClientRequest$scopes`), so a registration that named no scope
    /// restricts nothing.
    pub fn allows_scope(&self, scope: &str) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|named| named == scope)
    }
}

/// Prints the registration without its secret: a debug line can become a log line.
impl std::fmt::Debug for OidcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcClient")
            .field("client_id", &self.client_id)
            .field("client_name", &self.client_name)
            .field("client_type", &self.client_type)
            .field("scopes", &self.scopes)
            .field("redirect_uris", &self.redirect_uris)
            .field("grant_types", &self.grant_types)
            .field("issuer_url", &self.issuer_url)
            .field("client_id_issued_at", &self.client_id_issued_at)
            .field("client_secret_expires_at", &self.client_secret_expires_at)
            .finish_non_exhaustive()
    }
}

/// Where a device authorization stands: RFC 8628's pending/approved/denied
/// states, plus the redeemed one a device code reaches once a token was issued
/// for it.
///
/// Nothing in this slice writes `Denied`: refusing an authorization happens at
/// the verification page, which is not an operation of this API, so the state is
/// here for the surface that will set it and `CreateToken` already answers it
/// with the modelled `AccessDeniedException`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceAuthStatus {
    #[default]
    Pending,
    Approved,
    Denied,
    Consumed,
}

/// A device authorization minted by `StartDeviceAuthorization`.
#[derive(Clone)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub client_id: String,
    pub start_url: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub created_at: SystemTime,
    /// The instant the clock owes this authorization its `Pending -> Approved`
    /// transition; see [`APPROVAL_DELAY`].
    pub approval_due_at: SystemTime,
    pub expires_in: i32,
    pub interval: i32,
    pub status: DeviceAuthStatus,
}

impl DeviceAuthorization {
    /// The instant this authorization stops being usable.
    pub fn expires_at(&self) -> SystemTime {
        self.created_at + Duration::from_secs(u64::try_from(self.expires_in.max(0)).unwrap_or(0))
    }

    /// Whether the authorization's lifetime has run out at `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at()
    }

    /// Whether the clock already owes this authorization its approval at `now`.
    pub fn approval_is_due(&self, now: SystemTime) -> bool {
        now >= self.approval_due_at
    }
}

/// Prints the authorization without the device code, which is a bearer value.
impl std::fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("user_code", &self.user_code)
            .field("client_id", &self.client_id)
            .field("start_url", &self.start_url)
            .field("verification_uri", &self.verification_uri)
            .field("created_at", &self.created_at)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// An access token (and its refresh token) minted by `CreateToken`.
#[derive(Clone)]
pub struct IssuedToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub token_type: String,
    /// The region of the endpoint that issued this token: a token "was issued by
    /// and is only usable by calling IAM Identity Center endpoints" in one region
    /// (`InvalidRequestRegionException` documentation), so the region is part of
    /// the record rather than re-derived from whoever presents it.
    pub region: String,
    pub issued_at: SystemTime,
    pub expires_in: i32,
}

/// Prints the token's metadata but none of its secret material.
impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field("token_type", &self.token_type)
            .field("region", &self.region)
            .field("issued_at", &self.issued_at)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

impl IssuedToken {
    /// The instant this token stops being usable.
    pub fn expires_at(&self) -> SystemTime {
        self.issued_at + Duration::from_secs(u64::try_from(self.expires_in.max(0)).unwrap_or(0))
    }

    /// Whether the token's lifetime has run out at `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at()
    }
}

/// A refresh token minted beside an access token, with its own lifetime: it
/// outlives the access token it was issued with, which is what lets a client
/// whose access token has expired exchange it for a new pair.
#[derive(Clone)]
pub struct RefreshGrant {
    pub refresh_token: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// The region of the endpoint that issued this refresh token; see
    /// [`IssuedToken::region`].
    pub region: String,
    pub issued_at: SystemTime,
    pub expires_in: i32,
    /// The access token this refresh token was issued with, dropped when the
    /// refresh token is rotated so a refresh loop leaves nothing resident.
    pub bound_access_token: String,
}

impl RefreshGrant {
    /// The instant this refresh token stops being usable.
    pub fn expires_at(&self) -> SystemTime {
        self.issued_at + Duration::from_secs(u64::try_from(self.expires_in.max(0)).unwrap_or(0))
    }

    /// Whether the refresh token's own lifetime has run out at `now`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at()
    }
}

/// Prints the grant's metadata but none of its secret material.
impl std::fmt::Debug for RefreshGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshGrant")
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field("region", &self.region)
            .field("issued_at", &self.issued_at)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Why a refresh token could not be rotated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshFailure {
    /// Unknown, already rotated, or issued to another client: an invalid grant.
    InvalidGrant,
    /// Past its own lifetime: "the token issued by the service is expired and is
    /// no longer valid".
    Expired,
    /// The request asked for a scope the grant being spent does not carry, which
    /// RFC 6749 §6 refuses: "the requested scope MUST NOT include any scope not
    /// originally granted by the resource owner".
    ScopeTooBroad,
}

/// Why a `clientId`/`clientSecret` pair was not accepted. Every variant is an
/// `InvalidClientException`: "this can occur when a client sends an incorrect
/// clientId or an expired clientSecret".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientAuthFailure {
    UnknownClient,
    SecretMismatch,
    SecretExpired,
}

impl ClientAuthFailure {
    /// Text for the failure's `error_description`, naming what the caller must fix.
    pub fn description(self) -> &'static str {
        match self {
            Self::UnknownClient => "no client is registered with the given clientId",
            Self::SecretMismatch => "the clientSecret does not match the registered clientId",
            Self::SecretExpired => "the clientSecret for the given clientId has expired",
        }
    }
}

/// Per-service state for com-amazonaws-ssooidc-awsssooidcservice. A handler reaches it through [`service_state`].
#[derive(Default)]
pub struct ComAmazonawsSsooidcAwsssooidcserviceState {
    /// Registrations by `clientId`.
    pub clients: HashMap<String, OidcClient>,
    /// Device authorizations by `deviceCode`.
    pub device_auths: HashMap<String, DeviceAuthorization>,
    /// `userCode` -> `deviceCode`, the index an approval is looked up through.
    pub user_codes: HashMap<String, String>,
    /// Issued tokens by `accessToken`.
    pub access_tokens: HashMap<String, IssuedToken>,
    /// Issued refresh tokens by `refreshToken`, each with its own lifetime.
    pub refresh_tokens: HashMap<String, RefreshGrant>,
}

/// Prints the shape of the store, never a stored credential.
impl std::fmt::Debug for ComAmazonawsSsooidcAwsssooidcserviceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComAmazonawsSsooidcAwsssooidcserviceState")
            .field("clients", &self.clients.len())
            .field("device_auths", &self.device_auths.len())
            .field("access_tokens", &self.access_tokens.len())
            .field("refresh_tokens", &self.refresh_tokens.len())
            .finish_non_exhaustive()
    }
}

impl ComAmazonawsSsooidcAwsssooidcserviceState {
    /// The registration the pair authenticates, or why it does not.
    pub fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
        now: SystemTime,
    ) -> Result<&OidcClient, ClientAuthFailure> {
        let client = self
            .clients
            .get(client_id)
            .ok_or(ClientAuthFailure::UnknownClient)?;
        if client.client_secret != client_secret {
            return Err(ClientAuthFailure::SecretMismatch);
        }
        if epoch_seconds(now) >= client.client_secret_expires_at {
            return Err(ClientAuthFailure::SecretExpired);
        }
        Ok(client)
    }

    /// Persists a registration under its `clientId`.
    pub fn insert_client(&mut self, client: OidcClient) {
        self.clients.insert(client.client_id.clone(), client);
    }

    /// Whether another registration fits under [`MAX_CLIENTS`].
    pub fn has_room_for_client(&self) -> bool {
        self.clients.len() < MAX_CLIENTS
    }

    /// Whether another device authorization fits under [`MAX_DEVICE_AUTHORIZATIONS`].
    pub fn has_room_for_device_authorization(&self) -> bool {
        self.device_auths.len() < MAX_DEVICE_AUTHORIZATIONS
    }

    /// Whether another token fits under [`MAX_ACCESS_TOKENS`] and
    /// [`MAX_REFRESH_TOKENS`].
    pub fn has_room_for_token(&self) -> bool {
        self.access_tokens.len() < MAX_ACCESS_TOKENS
            && self.refresh_tokens.len() < MAX_REFRESH_TOKENS
    }

    /// Reclaims every registration whose `clientSecret` has expired, together with
    /// the device authorizations, tokens and refresh grants that belonged to it:
    /// once the registration is gone nothing can authenticate for those records,
    /// so leaving them resident would be growth nothing could ever reclaim.
    pub fn purge_expired_clients(&mut self, now: SystemTime) {
        let at = epoch_seconds(now);
        self.clients
            .retain(|_, client| client.client_secret_expires_at > at);
        let live_clients = &self.clients;
        self.device_auths
            .retain(|_, authorization| live_clients.contains_key(&authorization.client_id));
        self.access_tokens
            .retain(|_, token| live_clients.contains_key(&token.client_id));
        self.refresh_tokens
            .retain(|_, grant| live_clients.contains_key(&grant.client_id));
        let live_authorizations = &self.device_auths;
        self.user_codes
            .retain(|_, device_code| live_authorizations.contains_key(device_code));
    }

    /// Reclaims every device authorization that can no longer be exchanged — one
    /// already redeemed, or one more than [`EXPIRED_RETENTION`] past its
    /// `expiresIn` — and the user-code index entries that pointed at them. An
    /// authorization still inside the retention window is kept, so a client
    /// polling a stale device code is still told its token expired.
    pub fn purge_spent_device_authorizations(&mut self, now: SystemTime) {
        self.device_auths.retain(|_, authorization| {
            authorization.status != DeviceAuthStatus::Consumed
                && now < authorization.expires_at() + EXPIRED_RETENTION
        });
        let live = &self.device_auths;
        self.user_codes
            .retain(|_, device_code| live.contains_key(device_code));
    }

    /// Reclaims every access token past its own lifetime, and every refresh token
    /// more than [`EXPIRED_RETENTION`] past ITS OWN. The two are reclaimed
    /// independently, because a refresh token outlives the access token it was
    /// issued with — that is what the refresh-token grant is for — and the
    /// retention window on the refresh token is what keeps `ExpiredTokenException`
    /// answerable for a client that refreshes just too late. Neither is readable
    /// through any operation of this service once expired.
    pub fn purge_expired_tokens(&mut self, now: SystemTime) {
        self.access_tokens.retain(|_, token| !token.is_expired(now));
        self.refresh_tokens
            .retain(|_, grant| now < grant.expires_at() + EXPIRED_RETENTION);
    }

    /// A user code no live device authorization already carries, or `None` when
    /// [`MAX_USER_CODE_ATTEMPTS`] draws all collided.
    pub fn fresh_user_code(&self) -> Option<String> {
        (0..MAX_USER_CODE_ATTEMPTS)
            .map(|_| mint_user_code())
            .find(|candidate| !self.user_codes.contains_key(candidate))
    }

    /// Persists a device authorization and indexes it by its user code.
    pub fn insert_device_authorization(&mut self, authorization: DeviceAuthorization) {
        self.user_codes.insert(
            authorization.user_code.clone(),
            authorization.device_code.clone(),
        );
        self.device_auths
            .insert(authorization.device_code.clone(), authorization);
    }

    /// Mints and persists an access token for `client_id`, issued by the endpoint
    /// in `region`, with a refresh token when the client may use the
    /// refresh-token grant.
    pub fn issue_token(
        &mut self,
        client_id: &str,
        region: &str,
        scopes: Vec<String>,
        with_refresh_token: bool,
        now: SystemTime,
    ) -> IssuedToken {
        let access_token = mint_opaque_value();
        let refresh_token = with_refresh_token.then(mint_opaque_value);
        let token = IssuedToken {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            id_token: None,
            client_id: client_id.to_string(),
            scopes,
            token_type: TOKEN_TYPE_BEARER.to_string(),
            region: region.to_string(),
            issued_at: now,
            expires_in: ACCESS_TOKEN_EXPIRES_IN,
        };
        if let Some(refresh_token) = refresh_token {
            self.refresh_tokens.insert(
                refresh_token.clone(),
                RefreshGrant {
                    refresh_token,
                    client_id: client_id.to_string(),
                    scopes: token.scopes.clone(),
                    region: region.to_string(),
                    issued_at: now,
                    expires_in: REFRESH_TOKEN_EXPIRES_IN,
                    bound_access_token: access_token.clone(),
                },
            );
        }
        self.access_tokens.insert(access_token, token.clone());
        token
    }

    /// Rotates `refresh_token`: the refresh token is spent, the access token it
    /// was issued with is superseded (and dropped, so a refresh loop leaves
    /// nothing resident), and a new access and refresh token pair carrying the
    /// same scopes takes their place. The refresh token is resolved through its
    /// OWN record, so a client whose access token has already expired can still
    /// refresh.
    pub fn rotate_refresh_token(
        &mut self,
        refresh_token: &str,
        client_id: &str,
        region: &str,
        requested_scopes: Option<Vec<String>>,
        now: SystemTime,
    ) -> Result<IssuedToken, RefreshFailure> {
        let grant = self
            .refresh_tokens
            .get(refresh_token)
            .ok_or(RefreshFailure::InvalidGrant)?
            .clone();
        if grant.client_id != client_id {
            return Err(RefreshFailure::InvalidGrant);
        }
        if grant.is_expired(now) {
            // The record stays until `purge_expired_tokens` reclaims it after the
            // retention window, so a client that retries a late refresh is told
            // `expired_token` again rather than `invalid_grant`.
            return Err(RefreshFailure::Expired);
        }
        // RFC 6749 §6: a refreshed token's scope is no broader than the grant it
        // was issued from. A grant carrying no scope restricts nothing, matching
        // how a registration naming no scope restricts nothing.
        let scopes = match requested_scopes {
            Some(requested) => {
                let widened = !grant.scopes.is_empty()
                    && requested
                        .iter()
                        .any(|scope| !grant.scopes.contains(scope));
                if widened {
                    return Err(RefreshFailure::ScopeTooBroad);
                }
                requested
            }
            None => grant.scopes.clone(),
        };
        self.refresh_tokens.remove(refresh_token);
        self.access_tokens.remove(&grant.bound_access_token);
        Ok(self.issue_token(client_id, region, scopes, true, now))
    }
}

/// `at` as epoch seconds, the `LongTimeStampType` the model's timestamps use.
pub fn epoch_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A fresh random (version 4) UUID in its canonical hyphenated form, from the
/// system random source.
///
/// Drawn through `emulator_telemetry`'s own v4 generator, a crate this service
/// already depends on, so every opaque value below is unguessable without this
/// fix adding a dependency of its own.
fn random_uuid() -> String {
    emulator_telemetry::SessionId::random().as_str().to_string()
}

/// The 32 hex digits of a canonical UUID, with the two that are not random left
/// out: digit 12 carries the version and digit 16 the variant.
fn random_uuid_nibbles() -> Vec<u32> {
    random_uuid()
        .chars()
        .filter(|c| *c != '-')
        .enumerate()
        .filter(|(position, _)| *position != 12 && *position != 16)
        .filter_map(|(_, digit)| digit.to_digit(16))
        .collect()
}

/// A fresh opaque value — a client secret, a device code, an access or refresh
/// token — with 244 bits drawn from the system random source, so it can be
/// resolved only by the caller it was handed to.
pub fn mint_opaque_value() -> String {
    let mut value = String::with_capacity(64);
    for _ in 0..2 {
        value.extend(random_uuid().chars().filter(|c| *c != '-'));
    }
    value
}

/// The alphabet a user code is drawn from: upper-case letters and digits with
/// the visually ambiguous `0`/`O` and `1`/`I` left out, as RFC 8628 §6.1
/// recommends for a code a human transcribes. Its length divides 256, so a
/// uniform random byte maps to it uniformly.
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// A fresh user code: two groups of four characters, the short code a human
/// reads off a screen and types into the verification page. Each character is
/// drawn from a byte built out of two random UUID nibbles.
pub fn mint_user_code() -> String {
    let drawn: String = random_uuid_nibbles()
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .take(8)
        .map(|pair| {
            let byte = (pair[0] * 16 + pair[1]) as usize;
            char::from(USER_CODE_ALPHABET[byte % USER_CODE_ALPHABET.len()])
        })
        .collect();
    let (head, tail) = drawn.split_at(4);
    format!("{head}-{tail}")
}

/// Whether `value` is an absolute URI: the `URI` shapes this service takes
/// (`startUrl`, `issuerUrl`, a redirect URI) address a remote endpoint, so each
/// needs a scheme and an authority.
pub fn is_absolute_uri(value: &str) -> bool {
    value
        .parse::<http::Uri>()
        .is_ok_and(|uri| uri.scheme().is_some() && uri.authority().is_some())
}

/// Whether `value` is a single OAuth 2.0 scope token: scopes travel
/// space-delimited (RFC 6749 §3.3), so a scope is non-empty and carries no
/// whitespace.
pub fn is_scope_token(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_whitespace)
}

/// The device-verification page IAM Identity Center sends a device-flow client
/// to, for `region`.
pub fn verification_uri(region: &str) -> String {
    format!("https://device.sso.{region}.amazonaws.com/")
}

/// The service's own endpoint for `region`, from the model's `oidc` endpoint
/// prefix (`aws.api#service` `endpointPrefix`).
pub fn oidc_endpoint(region: &str) -> String {
    format!("https://oidc.{region}.amazonaws.com")
}

/// The region a request is addressed to: its SigV4 credential scope, else the
/// region label of a host-style endpoint (`oidc.<region>.amazonaws.com`), else
/// [`DEFAULT_REGION`]. Every operation of this service is `@optionalAuth`, so an
/// unsigned request must still resolve a region.
pub fn request_region(parts: &http::request::Parts) -> String {
    if let Some(scope) = emulator_server::sigv4_credential_scope(&parts.headers) {
        return scope.region;
    }
    request_host(parts)
        .and_then(|host| host.split('.').nth(1).map(str::to_string))
        .filter(|label| looks_like_region(label))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The host a request addresses, from the `Host` header or the request URI.
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    parts
        .headers
        .get(http::header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| parts.uri.host())
}

/// Whether `label` has the shape of an AWS region (`us-east-1`,
/// `ap-southeast-2`): lower-case letters, digits and hyphens, at least three
/// hyphen-separated parts, ending in a digit.
fn looks_like_region(label: &str) -> bool {
    label.split('-').count() >= 3
        && label.ends_with(|c: char| c.is_ascii_digit())
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsooidcAwsssooidcserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsooidcAwsssooidcserviceState>)
}
