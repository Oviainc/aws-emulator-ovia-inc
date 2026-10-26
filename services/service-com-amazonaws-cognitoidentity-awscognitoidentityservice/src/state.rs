// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentity-awscognitoidentityservice service.
//!
//! The identity-pool lifecycle and the identity/credential flow are stored here, in the
//! single shared engine, so every handler in this crate observes the same store across
//! requests. The records below are the SERVICE's own view of a resource; the modelled
//! wire shapes live in `crate::ops::types` and each handler builds its own output from
//! the record it read.

/// Whether IAM lets a request to this service do what it asks — the authorization gate
/// every developer-credentials operation of this service consults before it reads or
/// writes anything.
pub mod authorization;

use crate::ops::types::{CognitoIdentityProvider, RoleMapping};
use emulator_server::CredentialStore;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

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

impl IdentityPool {
    /// Whether this pool accepts a `Logins` entry made in the name of `provider_name`.
    ///
    /// `GetCredentialsForIdentity` is documented "Any provided logins will be validated
    /// against supported login providers" (this service's model). What this emulator can
    /// validate is the trust the POOL's own configuration states, and a login whose
    /// issuer the provider name itself names is exactly that case: an Amazon Cognito
    /// user pool, named `cognito-idp.<region>.amazonaws.com/<UserPoolId>` (`GetId.Logins`),
    /// is a login provider of this pool only when one of its `CognitoIdentityProviders`
    /// entries names that user pool — so a token minted by any OTHER user pool
    /// authenticates nobody here, however validly it was signed where it was issued.
    ///
    /// A login of any other kind is decided by the provider that issued its token
    /// (Facebook, Google, an OIDC or SAML issuer), which this emulator does not reach,
    /// so the pool's stored configuration alone does not refuse it.
    pub fn accepts_login_provider(&self, provider_name: &str) -> bool {
        !names_cognito_user_pool(provider_name)
            || self
                .cognito_identity_providers
                .iter()
                .flatten()
                .filter_map(|provider| provider.provider_name.as_deref())
                .any(|name| name == provider_name)
    }
}

/// Whether `provider_name` names an Amazon Cognito user pool: the
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` form `GetId.Logins` publishes for
/// one ("Amazon Cognito user pool: `cognito-idp.<region>.amazonaws.com/<YOUR_USER_POOL_ID>`",
/// this service's model). The name itself carries the issuer, which is what makes the
/// pool's stored configuration able to decide whether the login is one it accepts.
pub fn names_cognito_user_pool(provider_name: &str) -> bool {
    provider_name
        .strip_prefix(COGNITO_USER_POOL_PROVIDER_PREFIX)
        .is_some_and(|rest| rest.contains('/'))
}

/// The prefix of every Amazon Cognito user-pool provider name: the user-pools service's
/// own endpoint prefix, which every such provider name starts with.
const COGNITO_USER_POOL_PROVIDER_PREFIX: &str = "cognito-idp.";

/// The message a `NotAuthorizedException` carries for a `Logins` entry whose provider is
/// not one the identity pool accepts.
///
/// One fixed sentence, naming neither the provider the request presented nor the
/// providers the pool is configured with: the caller of this refusal is an
/// UNAUTHENTICATED one — the credential-exchange operations are the calls a client makes
/// before it holds any credential — so a message built from the pool's configuration
/// would report that configuration to anyone who asked with a token the pool rejects.
/// It states that the token's provider is not supported here, which is what a client has
/// to act on.
pub const UNSUPPORTED_LOGIN_PROVIDER_MESSAGE: &str =
    "Token is not from a supported provider of this identity pool.";

/// The message a `NotAuthorizedException` carries for a request naming a developer-identity
/// mapping the identity pool does not hold: a developer user identifier it has indexed
/// against no identity, under a developer provider that is not the pool's own, or in a pool
/// that names no developer provider at all.
///
/// Resolving a developer user identifier is an authorization question rather than the
/// lookup of a resource the caller already names: the identifier is a LOGIN of the pool, so
/// a request carrying one the pool holds no mapping for is answered as access the caller
/// does not have. One fixed sentence, naming neither the identifier nor any identity, so
/// nothing is disclosed about what the pool holds for it — and shared, so every operation
/// that resolves one answers alike.
pub const DEVELOPER_IDENTIFIER_FORBIDDEN_MESSAGE: &str = "You do not have access to this identity";

/// The most tags one identity pool can carry, from the model's own doc for `TagResource`:
/// "An identity pool can have as many as 50 tags"
/// (`smithy_query(query="doc",shape="TagResource")`). An AWS-published ceiling on the
/// resource, so it bounds the tag set a pool ends up with however the tags got there, and
/// a call that would take a pool past it is refused with the modelled
/// `InvalidParameterException` — the only client-input error the tagging operations model
/// for a quota of the resource.
pub const MAX_TAGS_PER_IDENTITY_POOL: usize = 50;

/// The tag-key prefix AWS reserves for its own tags. AWS's tagging rules state that the
/// `aws:` prefix is reserved for AWS use and that a user cannot create, edit or delete a
/// tag whose key carries it, so a request assigning such a key is a bad input parameter
/// rather than a tag this service stores.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `tag_key` lies in the AWS-reserved tag namespace ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// The prefix is matched case-insensitively — the reservation is on the namespace, not on
/// one spelling of it — and on BYTES, so a key whose first characters are multi-byte is
/// compared without slicing inside a character.
pub fn is_reserved_tag_key(tag_key: &str) -> bool {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    let key = tag_key.as_bytes();
    key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The MODELLED length bounds of an identity-pool tag KEY, `TagKeysType` (`len 1..128`):
/// the shape both the tag map's key (`IdentityPoolTagsType`) and the untag list's member
/// (`IdentityPoolTagsListType`) target
/// (`smithy_query(query="shape",shape="IdentityPoolTagsType,IdentityPoolTagsListType")`).
/// Held here so every operation that reads a tag key checks the one bound the model states.
pub const TAG_KEY_LENGTH: ::std::ops::RangeInclusive<usize> = 1..=128;

/// The MODELLED length bounds of a tag VALUE, `TagValueType` (`len 0..256`), from the same
/// query.
pub const TAG_VALUE_LENGTH: ::std::ops::RangeInclusive<usize> = 0..=256;

/// Whether `value` lies inside `bounds`, counted in CHARACTERS, as the `length` trait
/// measures a string.
pub fn length_is_within(value: &str, bounds: &::std::ops::RangeInclusive<usize>) -> bool {
    bounds.contains(&value.chars().count())
}

/// The one message a constraint refusal carries: the envelope AWS wraps modelled-constraint
/// violations in, stating how many it DETECTED and naming the ones in `violations`. Shared
/// by every operation of this service that refuses a value the model bounds, so the refusals
/// read alike however many violations one request carries.
///
/// `detected` is the number of violations the request has, which is what AWS's envelope
/// counts; `violations` are the clauses this refusal names, so a caller reading them against
/// the count can see when a refusal names fewer clauses than the request has violations.
pub fn validation_errors_detected(detected: usize, violations: &[String]) -> String {
    format!(
        "{detected} validation error{} detected: {}",
        if detected == 1 { "" } else { "s" },
        violations.join("; ")
    )
}

/// The punctuation an AWS tag key admits beside letters, digits and separators. AWS's
/// tagging rules publish the character set of a tag key as letters, numbers, spaces
/// representable in UTF-8 and `_ . : / = + - @`; a principal tag is referenced from an IAM
/// policy as a session tag of the federated principal, so a key outside that set names a
/// tag no policy could ever reference.
pub const TAG_KEY_PUNCTUATION: &str = "_.:/=+-@";

/// Whether `tag_key` is spelled with the characters a tag key admits
/// ([`TAG_KEY_PUNCTUATION`] beside letters, digits and separator whitespace, in any
/// script). The emptiness and the length of a key are the MODELLED bounds of the shape it
/// is written through, checked where it is read; this is the character rule alone.
pub fn is_well_formed_tag_key(tag_key: &str) -> bool {
    tag_key.chars().all(|character| {
        character.is_alphanumeric()
            // A separator, which is what AWS's rule admits as a "space": a control
            // character is whitespace to Rust but is not a separator.
            || (character.is_whitespace() && !character.is_control())
            || TAG_KEY_PUNCTUATION.contains(character)
    })
}

/// The most providers of ONE pool this emulator holds a principal-tag attribute map for.
/// Not an AWS quota: THIS emulator's own ceiling on a store that grows by one record per
/// `SetPrincipalTagAttributeMap` call naming a provider it has not seen before — the
/// provider name is a free-form caller string — and whose records live for the process. Set
/// to the same order as the other per-request ceilings here, well above the providers one
/// pool can configure, so a legitimate caller never meets it, while a caller varying the
/// name cannot grow the store without limit. A call that would create a record past it is
/// refused with the modelled `InvalidParameterException`; replacing the mapping of a
/// provider already stored is never refused for it.
pub const MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL: usize = 50;

/// The principal-tag attribute map of ONE identity provider of one identity pool, as
/// `SetPrincipalTagAttributeMap` wrote it and `GetPrincipalTagAttributeMap` reports it.
/// A set call REPLACES the provider's whole mapping, so one record holds everything that
/// mapping consists of.
#[derive(Debug, Clone, Default)]
pub struct PrincipalTagAttributeMap {
    /// Whether the call asked for the default (username and clientID) attribute mappings.
    pub use_defaults: bool,
    /// Principal tag key -> the user attribute (claim) it is taken from, ordered so a read
    /// reports the same map twice. Empty when the call carried no `PrincipalTags`.
    pub principal_tags: BTreeMap<String, String>,
}

/// The DEFAULT principal-tag attribute map, which a call asking for the default mappings
/// configures. `SetPrincipalTagAttributeMap`'s own doc calls them the "default (username
/// and clientID) attribute mappings", so those are the two tags; each is taken from the
/// claim an issued OpenID Connect token carries it in — the subject (`sub`), which names
/// the identity, and the audience (`aud`), which names what the token was issued to (see
/// [`open_id_token`]). Choosing the defaults is a choice INSTEAD of a custom mapping, so
/// this map is the provider's whole mapping rather than something merged into one.
pub const DEFAULT_PRINCIPAL_TAGS: [(&str, &str); 2] = [("clientID", "aud"), ("username", "sub")];

/// [`DEFAULT_PRINCIPAL_TAGS`] as a stored mapping.
pub fn default_principal_tags() -> BTreeMap<String, String> {
    DEFAULT_PRINCIPAL_TAGS
        .iter()
        .map(|(tag_key, attribute)| (tag_key.to_string(), attribute.to_string()))
        .collect()
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
    /// The role a vend assumes when the request names no `CustomRoleArn`: this pool's
    /// `authenticated` role for a request that presented a login, its `unauthenticated`
    /// role for one that did not. `None` when the pool configured no role for that flow,
    /// which is a pool that can vend no session at all for it.
    pub fn role_for(&self, authenticated: bool) -> Option<&str> {
        let role_type = if authenticated {
            AUTHENTICATED_ROLE_TYPE
        } else {
            UNAUTHENTICATED_ROLE_TYPE
        };
        self.roles.get(role_type).map(String::as_str)
    }

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
/// `GetOpenIdToken` / `GetOpenIdTokenForDeveloperIdentity` call — including the calls that
/// resolve an identity the pool already has — and whose records live as long as the window
/// the caller asked for, up to the modelled `TokenDuration` maximum of 24 hours. Expiry
/// alone would bound only how long a record lives, not how many exist, so the count is
/// capped too: see
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

/// The `Logins` provider name a token THIS service minted is presented under — the Cognito
/// Identity principal: "If the token is for cognito-identity.amazonaws.com, it will be
/// passed through to Security Token Service with the appropriate role for the token"
/// (`GetCredentialsForIdentity`, this service's model). A `Logins` entry under any other
/// provider name carries a token an EXTERNAL provider issued, which this emulator never saw
/// minted and so cannot decide; an entry under THIS name names an issuance this service
/// recorded itself, which is what makes its validity window readable here.
pub const COGNITO_IDENTITY_LOGIN_PROVIDER: &str = COGNITO_IDENTITY_PRINCIPAL;

/// How AWS words the refusal of a request presenting a token of this service whose validity
/// window has closed — the window `GetOpenIdToken` / `GetOpenIdTokenForDeveloperIdentity`
/// recorded when they minted it.
///
/// Fixed text with no request- or state-derived part: the caller of a credential exchange
/// holds no credential yet, so the refusal says that the token's time is up and nothing
/// about the identity, the pool or the token it was made with.
pub const EXPIRED_LOGIN_TOKEN_MESSAGE: &str = "Invalid login token. Token is expired.";

/// How AWS words the refusal of a request presenting a token under this service's own
/// provider name that this service cannot resolve at all: one it never issued, or one it
/// has forgotten. Nothing in it is vouched for, so nothing in it authorizes anything.
pub const UNVERIFIABLE_LOGIN_TOKEN_MESSAGE: &str =
    "Invalid login token. Couldn't verify signed token.";

/// Why a token presented under [`COGNITO_IDENTITY_LOGIN_PROVIDER`] authorizes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTokenRefusal {
    /// The validity window the issuance recorded has closed.
    Expired,
    /// This service holds no issuance for the token.
    Unverifiable,
}

impl LoginTokenRefusal {
    /// The message the refusal carries.
    pub fn message(self) -> &'static str {
        match self {
            Self::Expired => EXPIRED_LOGIN_TOKEN_MESSAGE,
            Self::Unverifiable => UNVERIFIABLE_LOGIN_TOKEN_MESSAGE,
        }
    }
}

/// The credentials `GetCredentialsForIdentity` issued for one identity. The id and the
/// secrets are the emulator's own credential store's: a handler never constructs them.
#[derive(Debug, Clone)]
pub struct IssuedCredentials {
    pub access_key_id: String,
    pub secret_key: String,
    pub session_token: String,
    pub expiration: SystemTime,
    /// The ARN of the pool role this session was assumed of, so a later vend hands the
    /// same session back only while it still stands for the role THAT call resolves — a
    /// vend for another role is another session.
    pub role_arn: String,
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
    /// The principal-tag attribute map of one `(IdentityPoolId, IdentityProviderName)`
    /// pair, as `SetPrincipalTagAttributeMap` wrote it. A pair with no entry has had no
    /// such call, so there is no mapping of that provider's attributes to report. The two
    /// parts are kept as separate key fields rather than joined into one string: the
    /// provider name comes from the caller and any separator it may also contain would
    /// make two distinct providers share a key. Bounded per pool by
    /// [`MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL`].
    pub principal_tag_attribute_maps: HashMap<(String, String), PrincipalTagAttributeMap>,
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
    /// The OpenID Connect tokens outstanding, by the token itself, as `GetOpenIdToken`
    /// and `GetOpenIdTokenForDeveloperIdentity` minted them. A token is a bearer of the
    /// identity it names, so the record is what tells this service which identity and
    /// pool a token presented later belongs to, and whether its window is still open.
    pub open_id_tokens: HashMap<String, IssuedOpenIdToken>,
}

impl ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// Whether another pool would take this emulator past [`MAX_IDENTITY_POOLS`].
    pub fn identity_pool_store_is_full(&self) -> bool {
        self.identity_pools.len() >= MAX_IDENTITY_POOLS
    }

    /// Whether this pool already holds as many principal-tag attribute maps as
    /// [`MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL`] allows, so a mapping for a provider it
    /// does not hold one for yet would take it past that ceiling. Replacing the mapping of
    /// a provider it already holds grows nothing, so the caller asks only about a NEW one.
    pub fn principal_tag_attribute_map_store_is_full(&self, identity_pool_id: &str) -> bool {
        self.principal_tag_attribute_maps
            .keys()
            .filter(|(pool, _)| pool == identity_pool_id)
            .count()
            >= MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL
    }

    /// Every stored `IdentityPoolId`, in the order a listing of the account's pools
    /// reports them: sorted, so two successive pages of one listing neither overlap nor
    /// skip a pool. The pools are held in a hash map, whose iteration order is not
    /// stable from one call to the next, so the order has to come from the ids
    /// themselves — as it does for the login values of an identity.
    pub fn identity_pool_ids_in_listing_order(&self) -> Vec<String> {
        let mut identity_pool_ids: Vec<String> = self.identity_pools.keys().cloned().collect();
        identity_pool_ids.sort();
        identity_pool_ids
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

    /// Every login value this pool has indexed for `provider` against `identity_id`, in
    /// a stable (sorted) order — for the pool's own `DeveloperProviderName`, the developer
    /// user identifiers one identity is registered under. The index is a hash map, so the
    /// order has to come from the values themselves for two successive pages of a listing
    /// to neither overlap nor skip one.
    pub fn logins_of_identity(
        &self,
        identity_pool_id: &str,
        provider: &str,
        identity_id: &str,
    ) -> Vec<String> {
        let mut logins: Vec<String> = self
            .login_index
            .iter()
            .filter(|((pool_id, login_provider, _), linked)| {
                pool_id == identity_pool_id
                    && login_provider == provider
                    && linked.as_str() == identity_id
            })
            .map(|((_, _, login), _)| login.clone())
            .collect();
        logins.sort();
        logins
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

    /// Forget the link between `(provider, login)` in this pool and whatever identity it
    /// resolved to, and report that identity — the inverse of [`Self::link_login`], so a
    /// later call presenting the same login resolves nothing and is seen as new. `None`
    /// when this pool indexed no such login at all.
    pub fn unlink_login(
        &mut self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
    ) -> Option<String> {
        self.login_index.remove(&(
            identity_pool_id.to_string(),
            provider.to_string(),
            login.to_string(),
        ))
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

    /// The issuance a token presented under [`COGNITO_IDENTITY_LOGIN_PROVIDER`] resolves
    /// to, or why it resolves to none: a token this service minted is honoured only INSIDE
    /// the validity window its issuance recorded, and only when this service issued it at
    /// all.
    ///
    /// The window is read from the token's OWN claims as well as from the record, because
    /// the two are the same window ([`open_id_token`] writes the recorded instants into the
    /// claims): a token whose record a sweep has already forgotten is still answered as the
    /// expired token it is rather than as one this service never issued.
    pub fn resolve_login_token(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Result<&IssuedOpenIdToken, LoginTokenRefusal> {
        if claimed_token_expiration(token).is_some_and(|expiration| expiration <= now) {
            return Err(LoginTokenRefusal::Expired);
        }
        match self.open_id_tokens.get(token) {
            Some(issued) if issued.expiration > now => Ok(issued),
            Some(_) => Err(LoginTokenRefusal::Expired),
            None => Err(LoginTokenRefusal::Unverifiable),
        }
    }

    /// Why the `Logins` map of a request authorizes nothing, if so: every token it presents
    /// under this service's own provider name has to resolve to an issuance whose window is
    /// still open ([`Self::resolve_login_token`]). A map presenting none is decided by the
    /// providers that issued its tokens, which this emulator does not reach, so it is not
    /// refused here.
    pub fn refuse_presented_login_tokens(
        &self,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> Option<LoginTokenRefusal> {
        logins
            .iter()
            .filter(|(provider, _)| provider.as_str() == COGNITO_IDENTITY_LOGIN_PROVIDER)
            .find_map(|(_, token)| self.resolve_login_token(token, now).err())
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
    /// index entries, their outstanding credentials, the OIDC tokens it issued and the
    /// principal-tag attribute maps of its providers — and
    /// report the access key ids whose credentials the caller must now forget. `None` when
    /// no such pool is stored.
    pub fn delete_identity_pool(&mut self, identity_pool_id: &str) -> Option<Vec<String>> {
        self.identity_pools.remove(identity_pool_id)?;
        self.identity_pool_roles.remove(identity_pool_id);
        self.principal_tag_attribute_maps
            .retain(|(pool, _), _| pool != identity_pool_id);
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

/// This service's ARN namespace, from the model's own `aws.api#service` trait
/// (`smithy_query(query="service_info")`: `arnNamespace: cognito-identity`). It is the
/// third field of every ARN that names one of this service's resources.
pub const ARN_NAMESPACE: &str = "cognito-identity";

/// The resource type an identity pool's ARN names. An Amazon Cognito identity pool is
/// named by `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`,
/// which is the ARN form the tagging operations' `ResourceArn` carries.
pub const IDENTITY_POOL_RESOURCE_TYPE: &str = "identitypool";

/// The ARN that names the identity pool `identity_pool_id`: this service's namespace, the
/// region the pool's own id names, the emulator's canonical account id
/// ([`::engine::emulator_account_id`]) and the pool as the resource.
pub fn identity_pool_arn(identity_pool_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region_of(identity_pool_id),
        &format!("{IDENTITY_POOL_RESOURCE_TYPE}/{identity_pool_id}"),
    )
}

/// The `IdentityPoolId` an identity-pool ARN names, or `None` when `arn` does not name an
/// identity pool of this service at all (a different ARN namespace or resource type, a
/// malformed account or partition, or no pool id after the resource type).
///
/// Only the ARN's FORM is checked here, and only the pool id is returned: an
/// `IdentityPoolId` is unique across this service, so it is the id — not the ARN's region
/// or account, which are the naming context around it — that selects the stored pool.
/// Whether that pool exists is the caller's lookup, which is what distinguishes a
/// malformed `ResourceArn` (an input error) from one naming a resource that is not stored
/// (a missing resource).
pub fn identity_pool_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(namespace), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let partition_is_named = !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    // An AWS account id is twelve digits.
    let account_is_an_id = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    if namespace != ARN_NAMESPACE
        || !partition_is_named
        || !is_region_like(region)
        || !account_is_an_id
    {
        return None;
    }
    resource
        .strip_prefix(IDENTITY_POOL_RESOURCE_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|identity_pool_id| !identity_pool_id.is_empty())
}

/// The pagination scope of the account-wide identity-pool listing, for a listing that
/// pages the account's POOLS rather than the contents of one pool. No `IdentityPoolId`
/// can take this value — a modelled id is `REGION:GUID` (`^[\w-]+:[0-9a-f-]+$`), so it
/// always carries a colon — which keeps a token of this listing from resuming a listing
/// inside a pool, and the other way round.
pub const IDENTITY_POOL_LISTING_SCOPE: &str = "identitypools";

/// An opaque pagination token naming the listing it was minted for (`scope`: the pool a
/// listing pages the contents of, or [`IDENTITY_POOL_LISTING_SCOPE`] for the account's
/// pools) and the item a page resumes at. Hex, so the token is opaque to a caller and
/// still matches the modelled `^[\S]+$` pattern.
pub fn encode_pagination_token(scope: &str, cursor: &str) -> String {
    hex::encode(format!("{scope}#{cursor}"))
}

/// The cursor inside a token from [`encode_pagination_token`], or `None` when the token
/// was not produced by this service for THIS listing — a token minted for one listing
/// cannot resume another.
pub fn decode_pagination_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (minted_for, cursor) = decoded.split_once('#')?;
    (minted_for == scope).then(|| cursor.to_string())
}

/// The OpenID Connect token for `identity_id` in `identity_pool_id`, valid from
/// `issued_at` until `expiration` and authenticated by `authenticated_by` (the provider
/// names the issuing call presented).
///
/// The claims are the OIDC registered ones (RFC 7519 §4.1) an Amazon Cognito Identity
/// token carries: the service's own `iss`, the identity pool as the audience, the
/// identity as the subject, the validity window, the `amr` authentication-methods
/// reference ([`authentication_methods`]) and a `jti` that makes every issued token
/// distinct, so two calls in the same second never produce one token.
pub fn open_id_token(
    identity_pool_id: &str,
    identity_id: &str,
    authenticated_by: &[String],
    issued_at: SystemTime,
    expiration: SystemTime,
) -> String {
    let amr: Vec<::serde_json::Value> = authentication_methods(authenticated_by)
        .iter()
        .map(|method| ::serde_json::Value::from(method.as_str()))
        .collect();
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

/// The claim set a token this service minted carries, read back out of the token itself:
/// the payload segment of the unsecured JWS [`open_id_token`] produced. `None` for a value
/// that is not one of this service's tokens — one that is no compact JWS, one whose payload
/// is not a JSON claim set, or one naming another issuer.
///
/// The reading is [`::service_common::jwt`], the tree's ONE bounded reading of a compact
/// JWS, through the reader a service that MINTED the token it is reading calls: the tokens
/// [`unsecured_jws`] produces carry no signature, which is the form that reader admits.
/// Nothing is VERIFIED here and nothing is granted on what it reads — an unsecured token is
/// one nothing signed, so the claim set is only ever read to REFUSE a token the service's
/// own issuance registry also refuses.
fn minted_token_claims(token: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let read = ::service_common::jwt::read_unsecured(token).ok()?;
    (read.claim_str(ISSUER_CLAIM) == Some(OIDC_TOKEN_ISSUER)).then_some(read.claims)
}

/// The claim naming who issued a token (RFC 7519 §4.1.1), which [`open_id_token`] writes
/// this service's own issuer into.
const ISSUER_CLAIM: &str = "iss";

/// The claim naming the instant a token's validity window closes (RFC 7519 §4.1.4).
const EXPIRATION_CLAIM: &str = "exp";

/// The instant a token this service minted says its validity window closes at, from the
/// `exp` claim [`open_id_token`] writes it into. `None` for a value that is not one of this
/// service's tokens, or whose claim set carries no instant there.
pub fn claimed_token_expiration(token: &str) -> Option<SystemTime> {
    let seconds = minted_token_claims(token)?
        .get(EXPIRATION_CLAIM)?
        .as_u64()?;
    // An instant past what this platform can represent is no instant the clock has
    // reached, so it is answered as no window rather than overflowing.
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
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

// ---------------------------------------------------------------------------
// The role session a pool vends: the web-identity assume behind
// `GetCredentialsForIdentity`.
// ---------------------------------------------------------------------------

/// The `RoleSessionName` every role session an identity pool vends carries, and so the
/// last field of the assumed-role ARN a vended session speaks for.
///
/// The caller of `GetCredentialsForIdentity` names no session: the pool's own exchange
/// does, with one name for every identity it vends for.
pub const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The key of the pool role a vend assumes for a request that PRESENTED a login, from the
/// modelled `RoleType` pattern `^(un)?authenticated$` and the `Roles` doc ("the key will
/// be either \"authenticated\" or \"unauthenticated\"").
pub const AUTHENTICATED_ROLE_TYPE: &str = "authenticated";

/// The key of the pool role a vend assumes for a request that presented NO login; see
/// [`AUTHENTICATED_ROLE_TYPE`].
pub const UNAUTHENTICATED_ROLE_TYPE: &str = "unauthenticated";

/// The IAM action a role's trust policy has to grant the Cognito Identity principal for a
/// pool to be able to vend a session of it: "Your role trust policy must grant
/// AssumeRoleWithWebIdentity permissions to cognito-identity.amazonaws.com"
/// (`InvalidIdentityPoolConfigurationException`, this service's model).
pub const ASSUME_ROLE_WITH_WEB_IDENTITY_ACTION: &str = "sts:AssumeRoleWithWebIdentity";

/// The condition key carrying the identity pool a vended session is being assumed FOR —
/// `<the Cognito Identity principal>:aud`. A role's trust policy conditions the assume on
/// it, which is what keeps one identity pool from federating into another pool's role
/// (`AssumeRoleWithWebIdentityRequest$RoleArn`: the trust policies of these roles "must
/// contain the cognito-identity.amazonaws.com:aud condition key to restrict role
/// assumption to users from your intended identity pools").
pub const AUDIENCE_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:aud";

/// The condition key carrying the identity the session is vended for: the `sub` claim of
/// the token the exchange is made with.
pub const SUBJECT_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:sub";

/// The condition key carrying the authentication methods the exchange was made under —
/// `authenticated` or `unauthenticated`, beside the providers that authenticated the user.
/// Multi-valued, so a trust policy decides it under one of IAM's set qualifiers.
pub const AUTHENTICATION_METHODS_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:amr";

/// The condition keys a vend publishes about the assume it makes, so a trust-policy clause
/// over one of them is read as "the request carried no value" rather than as a key this
/// emulator does not model; every other key stays undecided and withholds the grant.
pub const WEB_IDENTITY_PUBLISHED_CONDITION_KEYS: &[&str] = &[
    AUDIENCE_CONDITION_KEY,
    SUBJECT_CONDITION_KEY,
    AUTHENTICATION_METHODS_CONDITION_KEY,
];

/// How AWS words the refusal of a vend the pool's role configuration cannot serve: the
/// pool names no role for the request, the role it names is not one the account holds, or
/// that role's trust policy does not admit an assume by this pool's users.
///
/// One fixed sentence for all three, naming neither the role nor the pool: the caller of
/// `GetCredentialsForIdentity` is an unauthenticated one, so the refusal reports nothing
/// about the pool's configuration beyond the fact that it is the configuration at fault.
pub const INVALID_POOL_CONFIGURATION_MESSAGE: &str =
    "Invalid identity pool configuration. Check assigned IAM roles for this pool.";

/// A role as the IAM store in the same engine holds it, reduced to the facts an assume
/// decides on.
///
/// STS's OWN type, read through STS's own reader, so a role this service assumes is the
/// very record `sts:AssumeRole` would resolve for the same ARN.
pub type ResolvedRole =
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ResolvedRole;

/// The STS session registry in the SAME shared engine: where the sessions
/// `sts:GetCallerIdentity` resolves live.
///
/// STS's own accessor is called rather than copied, so a session this service mints is
/// recorded in the very registry that service reports from. `None` means the engine
/// carries no such store.
pub fn sts_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>>
{
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::service_state(engine)
}

/// The role `role_arn` names, as the IAM store in the same engine holds it, or `None` when
/// the account holds no such role — which is also the answer for an ARN of another account,
/// and for an engine that carries no IAM store at all.
pub fn resolve_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<ResolvedRole> {
    let iam = ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::iam_state(
        engine,
    )?;
    let iam = iam.lock();
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::resolve_role(
        &iam, role_arn,
    )
}

/// The condition keys a vend publishes about the assume it is making, for a trust policy's
/// `Condition` to be decided against: the pool the session is vended for, the identity it
/// is vended to, and the authentication methods the exchange was made under.
///
/// `providers` are the `Logins` provider names the request presented, which is what makes
/// the exchange an authenticated one. Nothing is invented: every pair comes from the
/// request and from the stored identity it names.
pub fn web_identity_condition_context(
    identity_pool_id: &str,
    identity_id: &str,
    providers: &[String],
) -> Vec<(String, String)> {
    let mut context = vec![
        (
            AUDIENCE_CONDITION_KEY.to_string(),
            identity_pool_id.to_string(),
        ),
        (SUBJECT_CONDITION_KEY.to_string(), identity_id.to_string()),
    ];
    context.extend(
        authentication_methods(providers)
            .into_iter()
            .map(|method| (AUTHENTICATION_METHODS_CONDITION_KEY.to_string(), method)),
    );
    context
}

/// The authentication methods an exchange made with `providers` — the `Logins` provider
/// names the call presented — was performed under.
///
/// A call that presented at least one login is an AUTHENTICATED one, and names
/// `authenticated`, this service (every token is minted by it, whatever provider
/// authenticated the user) and each of those providers; a call that presented none is
/// UNAUTHENTICATED and names that alone. One function, because these are the values a
/// minted token carries in its `amr` claim ([`open_id_token`]) AND the ones the
/// `...:amr` condition key publishes to a trust policy
/// ([`web_identity_condition_context`]): a policy clause written against the token a
/// caller holds has to decide the same way as the exchange that token is made with.
pub fn authentication_methods(providers: &[String]) -> Vec<String> {
    if providers.is_empty() {
        return vec![UNAUTHENTICATED_ROLE_TYPE.to_string()];
    }
    let mut methods = vec![
        AUTHENTICATED_ROLE_TYPE.to_string(),
        COGNITO_IDENTITY_PRINCIPAL.to_string(),
    ];
    methods.extend(providers.iter().cloned());
    methods
}

/// Whether `role`'s trust policy admits the web-identity assume `context` describes.
///
/// The DECISION is [`::service_common::iam_authorization`], the tree's one evaluation of
/// IAM policy documents, read as the RESOURCE-based policy a trust policy is: its
/// statements name the principals they grant to — here the Cognito Identity service
/// principal, the `Principal.Federated` spelling an identity pool's users are admitted by
/// — and a statement naming no resource covers the role itself. A role carrying no trust
/// policy, or one this cannot read as a policy, admits nobody: nothing is assumed on
/// evidence the account does not hold.
pub fn role_admits_web_identity(role: &ResolvedRole, context: &[(String, String)]) -> bool {
    let Some(document) = role.trust_policy.as_deref() else {
        return false;
    };
    let principals = [COGNITO_IDENTITY_PRINCIPAL.to_string()];
    let request = ::service_common::iam_authorization::Request::new(
        ASSUME_ROLE_WITH_WEB_IDENTITY_ACTION,
        &role.arn,
        &principals,
    )
    .with_context(context)
    .with_published_keys(WEB_IDENTITY_PUBLISHED_CONDITION_KEYS);
    ::service_common::iam_authorization::resource_decision(document, &request)
        == ::service_common::iam_authorization::Decision::Allowed
}

/// The session a vend hands back: the credential the caller signs with, and the
/// assumed-role identity it speaks for.
#[derive(Debug, Clone)]
pub struct VendedSession {
    pub access_key_id: String,
    pub secret_key: String,
    pub session_token: String,
    pub expiration: SystemTime,
    /// `arn:<partition>:sts::<account>:assumed-role/<role>/<session>`, what
    /// `sts:GetCallerIdentity` answers for a request signed with this credential.
    pub assumed_role_arn: String,
    /// `<RoleId>:<RoleSessionName>`, the `AssumedRoleUser$AssumedRoleId` of the assume.
    pub assumed_role_id: String,
}

/// Mint the role session a vend hands back: an assumed-role session of `role`, recorded in
/// the STS session registry in the same engine ([`sts_state`]) by STS's own mint, so
/// `sts:GetCallerIdentity` under the credential reports the assumed-role ARN and anything
/// correlating the session back to its role matches.
///
/// The session lasts `validity`, capped by the role's own `MaxSessionDuration` — a session
/// is never minted for longer than the role admits.
///
/// `Err` says why no session could be minted — this engine carries no STS registry, or the
/// registry is full of sessions that have not expired — so the caller refuses the request
/// rather than answering with a credential that stands for nothing.
pub fn mint_vended_role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role: &ResolvedRole,
    now: SystemTime,
    validity: Duration,
) -> Result<VendedSession, String> {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

    let Some(registry) = sts_state(engine) else {
        return Err(
            "the session registry the vended credentials are recorded in is unavailable"
                .to_string(),
        );
    };
    let duration_seconds = i32::try_from(validity.as_secs())
        .unwrap_or(i32::MAX)
        .min(role.max_session_duration);
    let assumed_role_id = format!("{}:{VENDED_ROLE_SESSION_NAME}", role.role_id);
    let assumed_role_arn = sts::sts_arn(
        &role.account,
        &format!("assumed-role/{}/{VENDED_ROLE_SESSION_NAME}", role.role_name),
    );
    let identity = sts::Identity {
        user_id: assumed_role_id.clone(),
        account: role.account.clone(),
        arn: assumed_role_arn.clone(),
    };
    let mut registry = registry.lock();
    let minted = sts::mint_session(
        store,
        &mut registry,
        now,
        sts::NewSession {
            role_arn: Some(role.arn.clone()),
            ..sts::NewSession::new(identity, sts::SessionKind::AssumedRole, duration_seconds)
        },
    )
    .map_err(|why| why.message())?;
    Ok(VendedSession {
        access_key_id: minted.credentials.access_key_id,
        secret_key: minted.credentials.secret_access_key,
        session_token: minted.credentials.session_token,
        expiration: minted.session.expires_at,
        assumed_role_arn,
        assumed_role_id,
    })
}

/// Forget a session this service vended, in both stores that hold it: the credential store
/// (so a request still presenting it no longer verifies) and the STS session registry (so
/// it names no identity either).
pub fn discard_vended_role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    access_key_id: &str,
) {
    match sts_state(engine) {
        Some(registry) => registry.lock().discard(access_key_id, store),
        None => {
            let _ = store.delete(access_key_id);
        }
    }
}

/// What the audit record of one vended assume reports.
pub struct VendedAssume<'a> {
    /// The role the session was assumed of.
    pub role: &'a ResolvedRole,
    /// The session that was minted.
    pub session: &'a VendedSession,
    /// The identity pool the assume was made for: the `aud` of the exchanged token.
    pub identity_pool_id: &'a str,
    /// The identity it was made for: the `sub` of the exchanged token.
    pub identity_id: &'a str,
    /// The Region the vend was served in.
    pub region: &'a str,
    /// How long the minted session lasts.
    pub duration_seconds: i32,
}

/// `userIdentity.type` of a call made by a user a web identity provider authenticated,
/// which is who an identity pool's exchange assumes the role on behalf of.
const WEB_IDENTITY_PRINCIPAL_TYPE: &str = "WebIdentityUser";

/// The resource type an assume's audit record names the role by.
const ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";

/// Record the web-identity assume a vend made in the account's management-event log, so
/// CloudTrail's `LookupEvents` reports it — the event is STS's, because the assume is: the
/// pool exchanges the token for a role session through `sts:AssumeRoleWithWebIdentity`.
///
/// Only a vend that was SERVED is recorded: a vend the role's trust policy refused assumed
/// nothing, so there is no assume to report.
///
/// Recording is best-effort by design, as every other recorder in this tree: an engine
/// carrying no log (a unit test over a bare engine) still serves the call, because failing
/// a caller because the audit log is unreachable would be a worse answer than serving it
/// unrecorded. No secret is written — the record carries the session's access key id and
/// expiry, never its secret key or session token.
pub fn record_vended_assume<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
    assume: &VendedAssume<'_>,
) {
    let Some(log) = management_event_log(engine) else {
        ::tracing::warn!(
            "the engine carries no management-event log: the vended assume was served \
             unrecorded"
        );
        return;
    };
    let expiration = ::aws_smithy_types::DateTime::from(assume.session.expiration)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    log.lock().record(
        ::service_common::management_events::ServedApiCall {
            event_name: "AssumeRoleWithWebIdentity".to_string(),
            event_source: STS_EVENT_SOURCE.to_string(),
            aws_region: assume.region.to_string(),
            read_only: false,
            // The assume is made for a federated user, which holds no access key of its
            // own: what names it is the identity the token carries.
            access_key_id: None,
            user_name: Some(assume.identity_id.to_string()),
            principal_type: WEB_IDENTITY_PRINCIPAL_TYPE.to_string(),
            account_id: ::engine::emulator_account_id().to_string(),
            user_agent: parts
                .headers
                .get(http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            resources: vec![::service_common::management_events::AuditedResource::new(
                ROLE_RESOURCE_TYPE,
                assume.role.arn.clone(),
            )],
            request_parameters: ::serde_json::json!({
                "roleArn": assume.role.arn,
                "roleSessionName": VENDED_ROLE_SESSION_NAME,
                "durationSeconds": assume.duration_seconds,
            }),
            response_elements: Some(::serde_json::json!({
                "credentials": {
                    "accessKeyId": assume.session.access_key_id,
                    "expiration": expiration,
                },
                "assumedRoleUser": {
                    "assumedRoleId": assume.session.assumed_role_id,
                    "arn": assume.session.assumed_role_arn,
                },
                "provider": COGNITO_IDENTITY_PRINCIPAL,
                "audience": assume.identity_pool_id,
                "subjectFromWebIdentityToken": assume.identity_id,
            })),
            error_code: None,
            error_message: None,
        },
        SystemTime::now(),
    );
}

/// The `eventSource` an assume's audit record carries: STS's endpoint prefix in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event, because the assume is a
/// call to STS however it was reached.
pub const STS_EVENT_SOURCE: &str = "sts.amazonaws.com";

/// This service's own `eventSource`: its endpoint prefix (`smithy_query(query="service_info")`
/// -> `endpointPrefix: cognito-identity`) in the same form.
pub const EVENT_SOURCE: &str = "cognito-identity.amazonaws.com";

/// What a vend reports about itself for its own audit record.
pub struct ServedVend<'a> {
    /// The identity the vend was asked for.
    pub identity_id: &'a str,
    /// The `Logins` provider NAMES the request presented. The tokens they carry are the
    /// caller's secrets and are never recorded.
    pub providers: &'a [String],
    /// The `CustomRoleArn` the request named, if any.
    pub custom_role_arn: Option<&'a str>,
}

/// The CloudFormation-style type name of the resource a vend's audit record refers to.
const IDENTITY_POOL_RESOURCE_TYPE_NAME: &str = "AWS::Cognito::IdentityPool";

/// Record the vend itself in the account's management-event log, whatever it answered, so
/// CloudTrail's `LookupEvents` reports the call — with the error code and message its
/// caller was answered with when it was refused.
///
/// No secret is written: the provider TOKENS the request carried and the session the
/// response carried are both left out, which is why the record carries no
/// `responseElements` at all.
///
/// The caller of this operation presents no credential — "This is a public API. You do not
/// need any credentials to call this API" (this operation's own doc) — so the record names
/// no access key and no principal this emulator could resolve.
pub fn record_vend<Eng: Send + Sync + 'static, T, E>(
    engine: &Eng,
    parts: &http::request::Parts,
    vend: &ServedVend<'_>,
    outcome: &Result<T, E>,
) where
    E: ::smithy_server_runtime::OperationError + ::std::fmt::Display,
{
    let Some(log) = management_event_log(engine) else {
        ::tracing::warn!("the engine carries no management-event log: the vend was served unrecorded");
        return;
    };
    let (error_code, error_message) = match outcome {
        Ok(_) => (None, None),
        Err(error) => {
            // The modelled error's `Display` prefixes its shape name, which the record
            // already carries as its `errorCode`, so the message is recorded alone.
            let rendered = error.to_string();
            let message = rendered
                .strip_prefix(&format!("{}: ", error.shape_name()))
                .unwrap_or(&rendered)
                .to_string();
            (Some(error.shape_name().to_string()), Some(message))
        }
    };
    let mut request_parameters = ::serde_json::Map::new();
    request_parameters.insert(
        "identityId".to_string(),
        ::serde_json::Value::from(vend.identity_id),
    );
    if !vend.providers.is_empty() {
        request_parameters.insert(
            "logins".to_string(),
            ::serde_json::Value::from(vend.providers.to_vec()),
        );
    }
    if let Some(custom_role_arn) = vend.custom_role_arn {
        request_parameters.insert(
            "customRoleArn".to_string(),
            ::serde_json::Value::from(custom_role_arn),
        );
    }
    // The resource the call acted on is the pool the identity belongs to, whatever the
    // call answered. A request naming an identity this service does not hold acted on no
    // pool at all, so such a record names no resource rather than one built from an id
    // nothing resolves.
    let identity_pool_id = service_state(engine).and_then(|state| {
        state
            .lock()
            .identities
            .get(vend.identity_id)
            .map(|identity| identity.identity_pool_id.clone())
    });
    log.lock().record(
        ::service_common::management_events::ServedApiCall {
            event_name: "GetCredentialsForIdentity".to_string(),
            event_source: EVENT_SOURCE.to_string(),
            // The Region the call was served in is the identity's own, which its
            // `REGION:GUID` id carries: the request names none, being unsigned.
            aws_region: region_of(vend.identity_id).to_string(),
            // A vend mints and stores a session, so it is not a reporting call.
            read_only: false,
            access_key_id: None,
            user_name: None,
            principal_type: ::service_common::management_events::PRINCIPAL_TYPE_UNKNOWN
                .to_string(),
            account_id: ::engine::emulator_account_id().to_string(),
            user_agent: parts
                .headers
                .get(http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            resources: identity_pool_id
                .map(|identity_pool_id| {
                    ::service_common::management_events::AuditedResource::new(
                        IDENTITY_POOL_RESOURCE_TYPE_NAME,
                        identity_pool_arn(&identity_pool_id),
                    )
                })
                .into_iter()
                .collect(),
            request_parameters: request_parameters.into(),
            response_elements: None,
            error_code,
            error_message,
        },
        SystemTime::now(),
    );
}

/// The account's management-event (audit) log in the SAME shared engine, the one store
/// CloudTrail's `LookupEvents` reads. `None` means the engine carries no such log.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::management_events::ManagementEventLog>)
}
