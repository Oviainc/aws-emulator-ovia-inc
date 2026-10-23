// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentity-awscognitoidentityservice service.
//!
//! The identity-pool lifecycle and the identity/credential flow are stored here, in the
//! single shared engine, so every handler in this crate observes the same store across
//! requests. The records below are the SERVICE's own view of a resource; the modelled
//! wire shapes live in `crate::ops::types` and each handler builds its own output from
//! the record it read.

use crate::ops::types::{CognitoIdentityProvider, RoleMapping};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// The region an id is minted in when the request carries no SigV4 credential scope to
/// read one from (`GetId` and `GetCredentialsForIdentity` are modelled `@optionalAuth`,
/// so an unsigned request is legitimate).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most pools this emulator stores. Not an AWS quota: it is THIS emulator's own
/// ceiling on a store that grows by one record per `CreateIdentityPool` call and lives
/// for the process, so an unbounded caller cannot grow it without limit. A call past it
/// is refused with the modelled `LimitExceededException`.
pub const MAX_IDENTITY_POOLS: usize = 1_000;

/// The most identities this emulator stores in one pool, for the same reason: `GetId`
/// mints one per unauthenticated call. Also the emulator's own ceiling, refused with the
/// modelled `LimitExceededException`.
pub const MAX_IDENTITIES_PER_POOL: usize = 10_000;

/// The most federated logins this emulator indexes for one pool. `GetId` adds one entry
/// per `Logins` entry it has not seen before, whether or not it mints an identity, so
/// this store needs its own ceiling: twice [`MAX_IDENTITIES_PER_POOL`], which leaves room
/// for identities reached through more than one provider. The emulator's own bound,
/// refused with the modelled `LimitExceededException`.
pub const MAX_LOGINS_PER_POOL: usize = 2 * MAX_IDENTITIES_PER_POOL;

/// The most providers one identity is linked to. THIS EMULATOR's own ceiling on a record
/// a caller can grow — `GetId` links up to ten providers per call (`LoginsMap` is modelled
/// `len 0..10`) and may be called again — not an AWS quota, so it is set well above what a
/// single call can reach while still bounding the stored record and the `Logins` list
/// every `ListIdentities` page carries. Refused with the modelled `LimitExceededException`.
pub const MAX_LOGIN_PROVIDERS_PER_IDENTITY: usize = 50;

/// A stored identity pool, as `CreateIdentityPool` wrote it and
/// `DescribeIdentityPool` reports it. Every optional member is held as an `Option` so a
/// read returns exactly what was stored, rather than inventing an empty map or list for
/// a member the caller never sent.
#[derive(Debug, Clone)]
pub struct IdentityPool {
    pub identity_pool_id: String,
    pub identity_pool_name: String,
    pub allow_unauthenticated_identities: bool,
    pub allow_classic_flow: Option<bool>,
    pub supported_login_providers: Option<BTreeMap<String, String>>,
    pub developer_provider_name: Option<String>,
    pub open_id_connect_provider_arns: Option<Vec<String>>,
    pub cognito_identity_providers: Option<Vec<CognitoIdentityProvider>>,
    pub saml_provider_arns: Option<Vec<String>>,
    pub identity_pool_tags: Option<BTreeMap<String, String>>,
}

/// The most mapping rules one identity provider may carry, from the model's own doc for
/// `RulesConfigurationType$Rules`: "An array of rules. You can specify up to 25 rules per
/// identity provider." (`smithy_query(query="shape",shape="RulesConfigurationType")`).
/// The modelled `length 1..400` on the list is wider, so the documented per-provider
/// ceiling is the service's own rule and is enforced where the rules are written.
pub const MAX_RULES_PER_IDENTITY_PROVIDER: usize = 25;

/// The role configuration of one identity pool, as `SetIdentityPoolRoles` wrote it and
/// `GetIdentityPoolRoles` reports it. `SetIdentityPoolRoles` replaces the whole
/// configuration, so one record holds everything a pool's roles consist of.
#[derive(Debug, Clone, Default)]
pub struct IdentityPoolRoles {
    /// `authenticated` / `unauthenticated` -> role ARN, keyed by the modelled `RoleType`.
    /// Ordered, so a read reports the same map twice.
    pub roles: BTreeMap<String, String>,
    /// Identity-provider name -> its role mapping. `None` when the call that wrote this
    /// record carried no `RoleMappings`, so a read reports exactly what was stored rather
    /// than an empty map the caller never sent.
    pub role_mappings: Option<BTreeMap<String, RoleMapping>>,
}

impl IdentityPoolRoles {
    /// Whether this configuration names `role_arn` — as one of the pool's two roles, or
    /// as the role a mapping rule resolves to. This is the set of roles the pool can vend
    /// credentials for, which is what a `CustomRoleArn` has to be drawn from.
    pub fn names_role_arn(&self, role_arn: &str) -> bool {
        self.roles.values().any(|arn| arn == role_arn)
            || self.role_mappings.iter().flatten().any(|(_, mapping)| {
                mapping.rules_configuration.iter().any(|configuration| {
                    configuration
                        .rules
                        .iter()
                        .any(|rule| rule.role_arn == role_arn)
                })
            })
    }
}

/// A stored identity, as `GetId` minted it. `logins` holds the PROVIDER NAMES the
/// identity is linked to, which is what the modelled `IdentityDescription$Logins` list
/// carries.
#[derive(Debug, Clone)]
pub struct Identity {
    pub identity_id: String,
    pub identity_pool_id: String,
    pub logins: Vec<String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

/// The most OpenID Connect tokens this emulator holds outstanding at once. Not an AWS
/// quota: THIS emulator's own ceiling on a store that grows by one record per successful
/// `GetOpenIdTokenForDeveloperIdentity` call — including the calls that resolve an
/// identity the pool already has — and whose records live as long as the window the
/// caller asked for, up to the modelled `TokenDuration` maximum of 24 hours. Expiry alone
/// would bound only how long a record lives, not how many exist, so the count is capped
/// too: see
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::record_open_id_token`],
/// which forgets the oldest issuances to stay inside it.
pub const MAX_OUTSTANDING_OPEN_ID_TOKENS: usize = 10_000;

/// The issuer every OpenID Connect token this service mints names in its `iss` claim.
/// Amazon Cognito Identity's OIDC tokens are issued by
/// `https://cognito-identity.amazonaws.com`, which is also the principal an IAM role's
/// trust policy names to let an identity pool's users assume it
/// (`AssumeRoleWithWebIdentity$RoleArn`: "The trust policies of these roles must accept
/// the cognito-identity.amazonaws.com service principal").
pub const OIDC_TOKEN_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// The Cognito Identity service principal, which an issued token names among the
/// authentication methods (`amr`) that produced it, beside the providers that
/// authenticated the user.
pub const COGNITO_IDENTITY_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// An OpenID Connect token this service issued, keyed by the token itself in
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::open_id_tokens`]: the
/// identity it names, the pool that issued it, the window it is valid in and the
/// principal tags the issuing call asked for.
#[derive(Debug, Clone)]
pub struct IssuedOpenIdToken {
    pub identity_id: String,
    pub identity_pool_id: String,
    pub issued_at: SystemTime,
    pub expiration: SystemTime,
    /// The `PrincipalTags` the issuing call carried, ordered so a read reports the same
    /// map twice. `None` when the caller sent none, so nothing invents an empty map.
    pub principal_tags: Option<BTreeMap<String, String>>,
}

/// The credentials `GetCredentialsForIdentity` issued for one identity. The id and the
/// secrets are the emulator's own credential store's: a handler never constructs them.
#[derive(Debug, Clone)]
pub struct IssuedCredentials {
    pub access_key_id: String,
    pub secret_key: String,
    pub session_token: String,
    pub expiration: SystemTime,
}

/// Per-service state for com-amazonaws-cognitoidentity-awscognitoidentityservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// Pools by `IdentityPoolId`.
    pub identity_pools: HashMap<String, IdentityPool>,
    /// The roles of a pool, by `IdentityPoolId`, as `SetIdentityPoolRoles` wrote them. A
    /// pool with no entry has had no `SetIdentityPoolRoles` call, which is an EMPTY role
    /// configuration, not a missing pool.
    pub identity_pool_roles: HashMap<String, IdentityPoolRoles>,
    /// Identities by `IdentityId`.
    pub identities: HashMap<String, Identity>,
    /// The identities of one pool, in the order `GetId` minted them, so a paginated
    /// `ListIdentities` resumes at a stable position.
    pub identities_by_pool: HashMap<String, Vec<String>>,
    /// `(IdentityPoolId, provider name, login value)` -> `IdentityId`, so `GetId` is
    /// idempotent for a federated login. The three parts are kept as separate key fields
    /// rather than joined into one string: both the provider name and the login come from
    /// the caller, and any separator they may also contain would make two distinct logins
    /// share a key.
    pub login_index: HashMap<(String, String, String), String>,
    /// The credentials outstanding for an identity, by `IdentityId`.
    pub credentials: HashMap<String, IssuedCredentials>,
    /// The OpenID Connect tokens outstanding, by the token itself, as
    /// `GetOpenIdTokenForDeveloperIdentity` minted them. A token is a bearer of the
    /// identity it names, so the record is what tells this service which identity and
    /// pool a token presented later belongs to, and whether its window is still open.
    pub open_id_tokens: HashMap<String, IssuedOpenIdToken>,
}

impl ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// Whether another pool would take this emulator past [`MAX_IDENTITY_POOLS`].
    pub fn identity_pool_store_is_full(&self) -> bool {
        self.identity_pools.len() >= MAX_IDENTITY_POOLS
    }

    /// Whether another identity in this pool would take it past
    /// [`MAX_IDENTITIES_PER_POOL`].
    pub fn pool_identity_store_is_full(&self, identity_pool_id: &str) -> bool {
        self.identities_by_pool
            .get(identity_pool_id)
            .is_some_and(|identities| identities.len() >= MAX_IDENTITIES_PER_POOL)
    }

    /// The `IdentityId` the pool has linked to this `(provider, login)`, if any.
    pub fn linked_identity(
        &self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
    ) -> Option<&String> {
        self.login_index.get(&(
            identity_pool_id.to_string(),
            provider.to_string(),
            login.to_string(),
        ))
    }

    /// Link `(provider, login)` in this pool to `identity_id`, so a later `GetId`
    /// presenting it resolves to that identity.
    pub fn link_login(
        &mut self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
        identity_id: &str,
    ) {
        self.login_index.insert(
            (
                identity_pool_id.to_string(),
                provider.to_string(),
                login.to_string(),
            ),
            identity_id.to_string(),
        );
    }

    /// Whether indexing `logins` for this pool would take it past
    /// [`MAX_LOGINS_PER_POOL`]. Only the logins not already indexed count:
    /// re-presenting a login this pool already knows grows nothing.
    pub fn pool_login_index_would_overflow(
        &self,
        identity_pool_id: &str,
        logins: &[(String, String)],
    ) -> bool {
        let indexed = self
            .login_index
            .keys()
            .filter(|(pool, _, _)| pool == identity_pool_id)
            .count();
        let fresh = logins
            .iter()
            .filter(|(provider, login)| {
                self.linked_identity(identity_pool_id, provider, login)
                    .is_none()
            })
            .count();
        indexed + fresh > MAX_LOGINS_PER_POOL
    }

    /// Whether linking `providers` to this identity would take it past
    /// [`MAX_LOGIN_PROVIDERS_PER_IDENTITY`]. Only providers it is not linked to yet count.
    pub fn identity_providers_would_overflow(
        &self,
        identity_id: &str,
        providers: &[String],
    ) -> bool {
        self.identities.get(identity_id).is_some_and(|identity| {
            let fresh = providers
                .iter()
                .filter(|provider| !identity.logins.contains(provider))
                .count();
            identity.logins.len() + fresh > MAX_LOGIN_PROVIDERS_PER_IDENTITY
        })
    }

    /// Forget every credential whose validity window has closed, and report the access
    /// key ids the caller must now delete from the emulator's credential store. The clock
    /// made these due: they are swept whenever credentials are issued, so an expired
    /// session is not held for the life of the process.
    pub fn purge_expired_credentials(&mut self, now: SystemTime) -> Vec<String> {
        let expired: Vec<(String, String)> = self
            .credentials
            .iter()
            .filter(|(_, issued)| issued.expiration <= now)
            .map(|(identity_id, issued)| (identity_id.clone(), issued.access_key_id.clone()))
            .collect();
        let mut revoked = Vec::with_capacity(expired.len());
        for (identity_id, access_key_id) in expired {
            self.credentials.remove(&identity_id);
            revoked.push(access_key_id);
        }
        revoked
    }

    /// Forget every OpenID Connect token whose validity window has closed. The clock made
    /// these due: an expired token names nothing, so the sweep runs whenever a token is
    /// issued and no expired token is held for the life of the process.
    pub fn purge_expired_open_id_tokens(&mut self, now: SystemTime) {
        self.open_id_tokens
            .retain(|_, issued| issued.expiration > now);
    }

    /// Record an issued OpenID Connect token, keeping the store within
    /// [`MAX_OUTSTANDING_OPEN_ID_TOKENS`]: what the clock has already expired is swept
    /// first, and if the ceiling is still reached the OLDEST issuances are forgotten to
    /// make room. Eviction rather than refusal, because every call mints a token and a
    /// caller must not be locked out of its own next token by its own history; a token
    /// this emulator has forgotten is simply one it can no longer resolve.
    pub fn record_open_id_token(
        &mut self,
        token: String,
        issued: IssuedOpenIdToken,
        now: SystemTime,
    ) {
        self.purge_expired_open_id_tokens(now);
        while self.open_id_tokens.len() >= MAX_OUTSTANDING_OPEN_ID_TOKENS {
            // Oldest first, with the token itself breaking a tie, so the eviction order is
            // the same whatever order the map iterates in.
            let oldest = self
                .open_id_tokens
                .iter()
                .min_by(|(left, left_issued), (right, right_issued)| {
                    left_issued
                        .issued_at
                        .cmp(&right_issued.issued_at)
                        .then_with(|| left.cmp(right))
                })
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            self.open_id_tokens.remove(&oldest);
        }
        self.open_id_tokens.insert(token, issued);
    }

    /// The record of a token this service issued, while its window is still open. `None`
    /// for a token this service did not issue, and for one whose window has closed.
    pub fn open_id_token_record(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Option<&IssuedOpenIdToken> {
        self.open_id_tokens
            .get(token)
            .filter(|issued| issued.expiration > now)
    }

    /// Mint an `IdentityPoolId` in `region` that no stored pool already uses.
    pub fn new_identity_pool_id(&self, region: &str) -> String {
        std::iter::repeat_with(|| cognito_id(region))
            .find(|id| !self.identity_pools.contains_key(id))
            .expect("the id generator is infinite")
    }

    /// Mint an `IdentityId` in `region` that no stored identity already uses.
    pub fn new_identity_id(&self, region: &str) -> String {
        std::iter::repeat_with(|| cognito_id(region))
            .find(|id| !self.identities.contains_key(id))
            .expect("the id generator is infinite")
    }

    /// Remove a pool with everything that belongs to it — its identities, their login
    /// index entries, their outstanding credentials and the OIDC tokens it issued — and
    /// report the access key ids whose credentials the caller must now forget. `None` when
    /// no such pool is stored.
    pub fn delete_identity_pool(&mut self, identity_pool_id: &str) -> Option<Vec<String>> {
        self.identity_pools.remove(identity_pool_id)?;
        self.identity_pool_roles.remove(identity_pool_id);
        let identity_ids = self
            .identities_by_pool
            .remove(identity_pool_id)
            .unwrap_or_default();
        let mut revoked = Vec::new();
        for identity_id in &identity_ids {
            self.identities.remove(identity_id);
            if let Some(issued) = self.credentials.remove(identity_id) {
                revoked.push(issued.access_key_id);
            }
        }
        self.login_index
            .retain(|(pool, _, _), _| pool != identity_pool_id);
        // A token names an identity of this pool, and neither the pool nor the identity
        // exists any more, so the token names nothing.
        self.open_id_tokens
            .retain(|_, issued| issued.identity_pool_id != identity_pool_id);
        Some(revoked)
    }
}

/// An identity-pool / identity id: `REGION:GUID`, the form the modelled
/// `^[\w-]+:[0-9a-f-]+$` pattern (and real Cognito Identity) uses.
fn cognito_id(region: &str) -> String {
    format!("{region}:{}", guid())
}

/// How many ids this process has minted, so two ids minted in the same nanosecond still
/// differ in what is mixed into them.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// The GUID half of a Cognito id: 32 lowercase hex digits in RFC 4122's 8-4-4-4-12
/// layout with the version-4 and variant bits set, which is what AWS's `REGION:GUID` ids
/// carry and what the modelled `[0-9a-f-]+` half admits.
///
/// The bytes come from the standard library's `RandomState`, whose SipHash keys the host
/// seeds per process and which varies per instance, mixed with the wall clock and a
/// process-unique counter through SHA-256 (the `sha2` crate) and rendered with `hex`, so
/// no input is recoverable from an id and two ids never coincide by construction. An id
/// is a NAME, not a secret: every secret this service returns is minted by the emulator's
/// own credential store, and [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::new_identity_pool_id`]
/// / [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::new_identity_id`]
/// additionally reject a value the store already holds, so no id can alias two resources.
fn guid() -> String {
    use sha2::Digest as _;
    use std::hash::{BuildHasher as _, Hasher as _};

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut entropy = std::collections::hash_map::RandomState::new().build_hasher();
    entropy.write_u64(minted);
    entropy.write_u128(nanos);

    let mut digest = sha2::Sha256::new();
    digest.update(entropy.finish().to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    let hashed = digest.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    // RFC 4122 §4.4: the version in the high nibble of octet 6, the variant in the two
    // high bits of octet 8. Every other bit is the random material above.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&bytes[0..4]),
        hex::encode(&bytes[4..6]),
        hex::encode(&bytes[6..8]),
        hex::encode(&bytes[8..10]),
        hex::encode(&bytes[10..16])
    )
}

/// The region part of an `IdentityPoolId` / `IdentityId` (`REGION:GUID`), so an id
/// minted for a resource inside a pool lands in the pool's own region.
pub fn region_of(cognito_id: &str) -> &str {
    match cognito_id.split_once(':') {
        Some((region, _)) if is_region_like(region) => region,
        _ => DEFAULT_REGION,
    }
}

/// The region this request is for: the SigV4 credential scope's third field, which every
/// signed AWS SDK request carries. Falls back to [`DEFAULT_REGION`] for an unsigned
/// request, or one whose scope region could not appear in an id.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` can stand in the `^[\w-]+` half of an id's modelled pattern.
fn is_region_like(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `arn` is an IAM ROLE ARN —
/// `arn:<partition>:iam::<12-digit-account-id>:role/<path and name>` — which is what the
/// modelled `ARNString` carries wherever this service names a role (`RolesMap`'s value
/// and `MappingRule$RoleARN`, both documented "the Role ARN"). `ARNString` constrains
/// only the length (20..2048), so the ARN's own form is the service's rule and is checked
/// where a role is written.
pub fn is_iam_role_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let partition_is_named = !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    // An AWS account id is twelve digits.
    let account_is_an_id = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    let names_a_role = resource
        .strip_prefix("role/")
        .is_some_and(|path_and_name| !path_and_name.is_empty() && !path_and_name.ends_with('/'));
    partition_is_named && account_is_an_id && names_a_role
}

/// An opaque pagination token naming the pool it was minted for and the item a page
/// resumes at. Hex, so the token is opaque to a caller and still matches the modelled
/// `^[\S]+$` pattern.
pub fn encode_pagination_token(identity_pool_id: &str, cursor: &str) -> String {
    hex::encode(format!("{identity_pool_id}#{cursor}"))
}

/// The cursor inside a token from [`encode_pagination_token`], or `None` when the token
/// was not produced by this service for THIS pool — a token minted for one pool cannot
/// resume a listing of another.
pub fn decode_pagination_token(identity_pool_id: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (minted_for, cursor) = decoded.split_once('#')?;
    (minted_for == identity_pool_id).then(|| cursor.to_string())
}

/// The OpenID Connect token for `identity_id` in `identity_pool_id`, valid from
/// `issued_at` until `expiration` and authenticated by `authenticated_by` (the provider
/// names the issuing call presented).
///
/// The claims are the OIDC registered ones (RFC 7519 §4.1) an Amazon Cognito Identity
/// token carries: the service's own `iss`, the identity pool as the audience, the
/// identity as the subject, the validity window, the `amr` authentication-methods
/// reference — `authenticated`, the Cognito service principal and each provider that
/// authenticated the user — and a `jti` that makes every issued token distinct, so two
/// calls in the same second never produce one token.
pub fn open_id_token(
    identity_pool_id: &str,
    identity_id: &str,
    authenticated_by: &[String],
    issued_at: SystemTime,
    expiration: SystemTime,
) -> String {
    let mut amr: Vec<::serde_json::Value> = vec![
        ::serde_json::Value::from("authenticated"),
        ::serde_json::Value::from(COGNITO_IDENTITY_PRINCIPAL),
    ];
    amr.extend(
        authenticated_by
            .iter()
            .map(|provider| ::serde_json::Value::from(provider.as_str())),
    );
    let claims = ::serde_json::json!({
        "iss": OIDC_TOKEN_ISSUER,
        "aud": identity_pool_id,
        "sub": identity_id,
        "amr": amr,
        "iat": unix_seconds(issued_at),
        "exp": unix_seconds(expiration),
        "jti": guid(),
    });
    unsecured_jws(&claims)
}

/// `claims` as an UNSECURED JWS in compact serialization (RFC 7515 §6, "Using the
/// Algorithm none"): `base64url(header) '.' base64url(claims) '.'`, with an empty
/// signature segment.
///
/// Amazon Cognito Identity signs its OIDC tokens with a key pair whose public half it
/// publishes. This emulator holds no such key pair, so it issues the form RFC 7515
/// defines for a JWS that carries no signature rather than filling the third segment with
/// bytes that would only look like one. The token is still this emulator's own: its claims
/// come from stored state and every token issued is recorded in
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::open_id_tokens`], so the
/// service can say which identity a token it handed out names.
fn unsecured_jws(claims: &::serde_json::Value) -> String {
    let header = ::serde_json::json!({ "alg": "none", "typ": "JWT" });
    format!(
        "{}.{}.",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes())
    )
}

/// `bytes` in base64url without padding, as a JWS segment is encoded (RFC 7515 §2): the
/// standard-alphabet codec this workspace already uses for every Smithy blob
/// (`aws_smithy_types::base64`), with `+` and `/` replaced by `-` and `_` and the padding
/// removed.
pub fn base64_url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// `time` as whole seconds since the epoch, which is how a token claim carries an instant
/// (RFC 7519 §2's NumericDate).
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>)
}
