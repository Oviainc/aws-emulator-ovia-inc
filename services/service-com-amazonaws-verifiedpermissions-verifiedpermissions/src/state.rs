// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-verifiedpermissions-verifiedpermissions service.
//!
//! Everything a handler persists lives here: policy stores, the policies,
//! policy templates and identity sources inside them, the alias table and the
//! idempotency-token ledger. The Cedar-specific helpers (parsing a policy's
//! scope out of its statement, parsing a template, evaluating an authorization
//! request) live beside it in [`cedar`], so every operation reaches one
//! implementation rather than re-deriving the policy language.

pub mod cedar;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

/// Published Verified Permissions quota: *Policies per policy store* — 200,000.
pub const MAX_POLICIES_PER_STORE: usize = 200_000;

/// Published Verified Permissions quota: *Policy stores per Region per account* —
/// 100. It is also this emulator's MEMORY bound: `policy_stores` is the one
/// structure a caller grows by an entry per request, so without a cap a loop of
/// `CreatePolicyStore` calls grows host memory without limit. Every other
/// per-store dimension is already bounded by the model (`description` 0..150,
/// `tags` ..200) or by [`MAX_POLICIES_PER_STORE`] /
/// [`MAX_POLICY_BYTES_PER_STORE`] below.
pub const MAX_POLICY_STORES: usize = 100;

/// How many policy store aliases this account may hold. AWS publishes no figure
/// for this dimension, so this is the emulator's own MEMORY bound and is
/// deliberately stated as such: `aliases` is, beside `policy_stores`, the other
/// structure a caller grows by an entry per request, so without a cap a loop of
/// `CreatePolicyStoreAlias` calls grows host memory without limit. It matches
/// [`MAX_POLICY_STORES`] because an alias is an alternative identifier for a
/// store, and a store that reaches it refuses further aliases with the
/// `ServiceQuotaExceededException` `CreatePolicyStoreAlias` models.
pub const MAX_POLICY_STORE_ALIASES: usize = 100;

/// The prefix every policy store alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s own model documentation: "The alias
/// name must always be prefixed with `policy-store-alias/`." It is also what
/// makes an alias name usable in a `policyStoreId` member: a reference carrying
/// this prefix names an alias, and anything else names a store id.
pub const ALIAS_PREFIX: &str = "policy-store-alias/";

/// `AliasState$Active`: the alias resolves and can be used normally.
pub const ALIAS_STATE_ACTIVE: &str = "Active";

/// `AliasState$PendingDeletion`: the alias has been soft-deleted. It no longer
/// resolves, and its name cannot be taken by a new alias.
pub const ALIAS_STATE_PENDING_DELETION: &str = "PendingDeletion";

/// The total Cedar text one policy store may hold, and therefore the per-request
/// STEP BUDGET for [`cedar::authorize`]: an authorization request parses every
/// static policy in the store, so the work one request can ask for is bounded by
/// this many bytes of Cedar — not by the 200,000-policy count alone, which at the
/// 10 KB-per-policy limit would be gigabytes. Chosen so one request's parse stays
/// in the low milliseconds; a store that reaches it refuses further policies with
/// the `ServiceQuotaExceededException` `CreatePolicy` already models.
///
/// It bounds the store's Cedar text as a whole — [`PolicyStore::cedar_bytes`] —
/// so a policy template's statement counts against it exactly as a policy's
/// does: a template is Cedar text a caller stores in the same store, and the
/// policies linked to it re-use that same text.
pub const MAX_POLICY_BYTES_PER_STORE: usize = 1024 * 1024;

/// How many policy templates one policy store may hold. This is this emulator's
/// MEMORY bound and not a figure quoted from AWS: `policy_templates` is a
/// structure a caller grows by an entry per `CreatePolicyTemplate` request, so
/// without a cap a loop of them grows host memory without limit. The Cedar text
/// those entries hold is bounded alongside the store's policies by
/// [`MAX_POLICY_BYTES_PER_STORE`]. A store that reaches either refuses further
/// templates with the `ServiceQuotaExceededException` `CreatePolicyTemplate`
/// models.
pub const MAX_POLICY_TEMPLATES_PER_STORE: usize = 10_000;

/// This emulator's MEMORY bound on identity sources per policy store:
/// `identity_sources` is a structure a caller grows by an entry per
/// `CreateIdentitySource` call, so without a cap a loop of calls grows host
/// memory without limit. It is this emulator's bound, not a published AWS
/// quota; a store that reaches it refuses further identity sources with the
/// `ServiceQuotaExceededException` `CreateIdentitySource` models.
pub const MAX_IDENTITY_SOURCES_PER_STORE: usize = 100;

/// The Cedar entity type a Cognito identity source's principals carry when the
/// create request named no `principalEntityType`.
///
/// `CreateIdentitySource$principalEntityType` is optional while
/// `GetIdentitySource$principalEntityType` is **required**, so an omitted value
/// has to resolve to the service's own default; for an Amazon Cognito user pool
/// source that default is the `AWS::Cognito` entity type Verified Permissions
/// generates principals as when the policy store maps none of its own.
pub const DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE: &str = "AWS::Cognito";

/// The region used for a request that carries no SigV4 credential scope — the
/// region every AWS SDK defaults to when none is configured.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Published Verified Permissions quota: *Tags per resource* — 50. The model
/// states this limit twice: `TagResource`'s own documentation ("You can
/// associate as many as 50 tags with a resource") and `TooManyTagsException`'s
/// ("No more tags be added because the limit (50) has been reached"). It is NOT
/// the same bound as `TagMap`'s modelled length (..200), which bounds ONE
/// request's map; this bounds what a resource ends up holding.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The Cedar language version a policy store is created with, one of
/// `CedarVersion`'s two values (`smithy_query(query="shape",
/// shape="CedarVersion")`: `CEDAR_2 | CEDAR_4`).
///
/// It is a SERVICE default, not a request value: no member of any modelled
/// operation carries a Cedar version — `CreatePolicyStore`, `UpdatePolicyStore`
/// and `PutSchema` have none — and `CedarVersion` is only ever reported, by
/// `GetPolicyStore`. The corpus's recorded `GetPolicyStore` responses for a
/// plainly created store report `CEDAR_4`, the newer of the two, which is what
/// the service creates stores with.
pub const DEFAULT_CEDAR_VERSION: &str = "CEDAR_4";

/// The message EVERY `ResourceNotFoundException` this service raises carries.
///
/// A CONSTANT, embedding no request or generated value: the model gives the
/// exception its own `resourceId` ("the unique ID of the resource referenced in
/// the failed request") and `resourceType` members, so the identity of what was
/// not found is reported there rather than spliced into prose. The corpus
/// records this same text for three different operations' 404s — `GetPolicy`,
/// `GetPolicyStore` (fixture `verifiedpermissions-policy-store-lifecycle-and-tags`,
/// steps task#2 and teardown#11) and `ListTagsForResource` — which is what
/// establishes it as one fixed sentence rather than per-operation prose.
///
/// Every not-found path of this service fills it through the single builder
/// [`crate::ops::resource_not_found`]. `IsAuthorized`'s recording expects an
/// id-BEARING message, but no rule in the model or the corpus produces that
/// text, so under the message rule it is an example rather than a requirement
/// and that path answers this constant too.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The message EVERY `ConflictException` this service raises carries.
///
/// A CONSTANT, for the same reason as [`RESOURCE_NOT_FOUND_MESSAGE`]: the model
/// gives the exception a required `resources` member — "the list of resources
/// referenced with this failed request", each a `ResourceConflict{resourceId,
/// resourceType}` — so the identity of what was collided with is reported
/// there, machine-readably, rather than spliced into prose, and the message
/// stays the exception's own documented summary ("The request failed because
/// another request to modify a resource occurred at the same time"). The corpus
/// records this same text for two STRUCTURALLY DIFFERENT conflict causes on
/// `CreatePolicyStoreAlias` — racing claims of one alias name by different
/// stores, and re-creating a name that is in [`ALIAS_STATE_PENDING_DELETION`] —
/// which is what establishes it as one fixed sentence rather than per-cause
/// prose. Every conflicting path of this service fills it through the single
/// builder [`crate::ops::conflict`].
pub const CONFLICT_MESSAGE: &str = "Conflict occurred during the processing of this request";

/// The message EVERY `TooManyTagsException` this service raises carries.
///
/// A CONSTANT for the same reason again: the exception models its own
/// `resourceName` member (an `AmazonResourceName`), so the resource that is at
/// the limit is reported there and neither the count nor the offending key
/// belongs in the text — [`MAX_TAGS_PER_RESOURCE`] is a published quota a client
/// already knows, not news carried by one refusal. The corpus records this text
/// for the refusal of a 51st distinct key. Filled through
/// [`crate::ops::too_many_tags`].
pub const TOO_MANY_TAGS_MESSAGE: &str = "The request exceeds the maximum number of user tags allowed";

/// The refusal `DeletePolicyStore` answers when deletion protection is enabled.
///
/// A CONSTANT for the same reason: `InvalidStateException` models a single
/// `message` member and documents exactly this refusal with no resource named
/// in it, and `DeletePolicyStoreInput` carries nothing the service could report
/// back — so no request value and no minted identifier belongs in the text, and
/// a client, test or log rule that matches it matches identically for every
/// store. The wording is the sentence pair the service emits ON THE WIRE, which
/// is what a client reads; the shape's documentation states the same fixed
/// refusal in slightly different prose ("The policy store can't be deleted
/// because deletion protection is enabled. To delete this policy store, disable
/// deletion protection."), so taking the doc's phrasing instead would diverge
/// from the recorded bytes while asserting the same rule. Filled through
/// [`crate::ops::deletion_protection_enabled`].
pub const DELETION_PROTECTION_MESSAGE: &str = "The policy store cannot be deleted because deletion \
     protection is enabled. Deletion protection must be disabled first.";

/// How long a `clientToken` is remembered, from `CreatePolicyStore`'s own model
/// documentation: "Verified Permissions recognizes a ClientToken for eight hours."
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// How many `clientToken` records are kept. The TTL above expires them by the
/// clock, but within one window a caller can present a fresh token per request, so
/// the ledger needs a count bound too; the oldest record is evicted past it. A
/// token evicted early simply stops being recognised as a replay, which is what
/// the model says happens once a token is forgotten.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// The page size a `@paginated` list operation of this service uses when the
/// caller names none, from each one's own `maxResults` model documentation: "If
/// you do not specify this parameter, the operation defaults to 10 ... per
/// response" — `ListPolicies` and `ListIdentitySources` both state it, in those
/// words, for their own item. Every list operation here is `@paginated`, so an
/// absent `maxResults` must not mean "the whole store in one response": the
/// caller gets a bounded page plus a `nextToken`. ONE constant, so two list
/// operations of one service cannot page differently.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The largest page a list operation of this service will build, whatever
/// `maxResults` asks for, from the next sentence of that same documentation:
/// "You can specify a maximum of 50 ... per response." The modelled `range` on
/// `maxResults` states only `min 1`, so this documented cap is also this
/// emulator's response-size bound.
pub const MAX_PAGE_SIZE: usize = 50;

/// Per-service state for com-amazonaws-verifiedpermissions-verifiedpermissions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsVerifiedpermissionsVerifiedpermissionsState {
    /// Every policy store, keyed by `policyStoreId`. A `BTreeMap` so
    /// `ListPolicies`/`ListPolicyStores` enumerate in a stable order and a
    /// pagination cursor resumes exactly where the previous page stopped.
    pub policy_stores: BTreeMap<String, PolicyStore>,
    /// Every policy store alias, keyed by its FULL `aliasName` — the name
    /// including its [`ALIAS_PREFIX`], exactly as the caller gave it, which is
    /// also the form a `policyStoreId` member carries when it names an alias. A
    /// `BTreeMap` so `ListPolicyStoreAliases` can enumerate in a stable order.
    pub aliases: BTreeMap<String, PolicyStoreAlias>,
    /// `(operation, clientToken)` -> the resource that token already created, so
    /// an `@idempotencyToken` replay answers with the first result. The OPERATION
    /// is part of the key because an idempotency token is scoped to one request:
    /// presenting the same token value to `CreatePolicyStore` and to
    /// `CreatePolicy` is legal and must not be read as a conflicting reuse.
    pub client_tokens: BTreeMap<(String, String), IdempotencyRecord>,
    /// Strictly increasing, so every minted `policyStoreId` is distinct.
    pub next_store_seq: u64,
}

/// What one `@idempotencyToken` already did, so a replay can be told apart from a
/// reuse of the same token with different parameters.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first call created.
    pub resource_id: String,
    /// A canonical rendering of that first call's parameters. A later call with
    /// the same token but a different rendering is a `ConflictException`.
    pub parameters: String,
    /// When the token was first seen; it is forgotten [`CLIENT_TOKEN_TTL`] later.
    pub recorded_at: SystemTime,
}

/// One policy store alias: an alternative identifier for a policy store.
#[derive(Debug, Clone)]
pub struct PolicyStoreAlias {
    /// The full alias name, including [`ALIAS_PREFIX`].
    pub alias_name: String,
    pub arn: String,
    pub policy_store_id: String,
    pub created_at: DateTime,
    /// [`ALIAS_STATE_ACTIVE`] or [`ALIAS_STATE_PENDING_DELETION`].
    pub state: String,
}

/// How a policy store and its child resources are encrypted: the state behind
/// `GetPolicyStoreOutput$encryptionState`, whose modelled union has exactly
/// these two arms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StoreEncryption {
    /// `EncryptionState$default`: "This is the default encryption state. The
    /// policy store is encrypted using an Amazon Web Services owned key."
    #[default]
    AwsOwnedKey,
    /// `EncryptionState$kmsEncryptionState`: the store is encrypted with a
    /// customer-managed KMS key, under a user-defined encryption context.
    CustomerManagedKey {
        /// `KmsEncryptionState$key` — the key being used for encryption.
        key: String,
        /// `KmsEncryptionState$encryptionContext` — the user-defined context
        /// added during encryption processes.
        encryption_context: BTreeMap<String, String>,
    },
}

/// One policy store and everything it owns.
#[derive(Debug, Clone)]
pub struct PolicyStore {
    pub policy_store_id: String,
    pub arn: String,
    /// `ValidationSettings.mode`: `OFF` or `STRICT`.
    pub validation_mode: String,
    pub description: Option<String>,
    /// `DISABLED` or `ENABLED`; absent on the request means `DISABLED`.
    pub deletion_protection: String,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
    /// `CedarVersion`: the Cedar language version this store's policies are
    /// written against, [`DEFAULT_CEDAR_VERSION`] at creation.
    pub cedar_version: String,
    /// How this store is encrypted. A store is created
    /// [`StoreEncryption::AwsOwnedKey`] unless a customer-managed key was
    /// configured for it, and `GetPolicyStore` reports whichever arm is
    /// persisted here.
    pub encryption: StoreEncryption,
    pub tags: BTreeMap<String, String>,
    /// The store's policies, keyed by `policyId`.
    pub policies: BTreeMap<String, Policy>,
    /// The store's policy templates, keyed by `policyTemplateId`. A `BTreeMap`
    /// so `ListPolicyTemplates` enumerates in a stable order and a pagination
    /// cursor resumes where the previous page stopped.
    pub policy_templates: BTreeMap<String, PolicyTemplate>,
    /// The store's identity sources, keyed by `identitySourceId`. A `BTreeMap`
    /// so `ListIdentitySources` enumerates in a stable order and a pagination
    /// cursor resumes where the previous page stopped.
    pub identity_sources: BTreeMap<String, IdentitySource>,
    /// Strictly increasing, so every id minted inside this store — a `policyId`,
    /// a `policyTemplateId` or an `identitySourceId` — is distinct. ONE counter
    /// serves them all, so the minters can never be advanced out of step with
    /// each other.
    pub next_id_seq: u64,
    /// The Cedar schema `PutSchema` stored, when one has been stored. A store
    /// holds at most ONE schema — `PutSchema` "creates or updates" it — so this
    /// is an `Option`, not a map. `None` is the EMPTY schema document: see
    /// [`PolicyStore::schema_document`], which is what `GetSchema` reports.
    pub schema: Option<StoredSchema>,
}

/// The one Cedar schema a policy store holds: the text exactly as `PutSchema`
/// received it, plus the namespaces read out of it and its own timestamps.
#[derive(Debug, Clone)]
pub struct StoredSchema {
    /// The Cedar schema JSON, byte-identical to what `PutSchema` was given, so
    /// `GetSchema` reports the caller's own text back.
    pub cedar_json: String,
    /// The namespaces the schema declares, in the order the parse reported them.
    pub namespaces: Vec<String>,
    /// When the store's FIRST schema was put: the model documents this as "the
    /// date and time that the schema was originally created", so a later
    /// `PutSchema` on the same store keeps it.
    pub created_date: DateTime,
    /// When the schema was last put.
    pub last_updated_date: DateTime,
}

/// The EMPTY Cedar schema document: a schema that declares no namespace, and the
/// document a policy store holds until `PutSchema` writes one. It is also the
/// document `PutSchema` returns a store to — `SchemaDefinition$cedarJson`
/// documents "To delete the schema, run PutSchema with `{}` for this parameter" —
/// so "no schema" and "the empty schema" are one state of one document.
pub const EMPTY_SCHEMA_JSON: &str = "{}";

/// One identity source in a store: the provider it points at, the Cedar entity
/// type its principals carry, and when it was created and last changed.
#[derive(Debug, Clone)]
pub struct IdentitySource {
    pub identity_source_id: String,
    pub policy_store_id: String,
    /// The resolved `principalEntityType`: the request's value, or
    /// [`DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE`] when it named none.
    pub principal_entity_type: String,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
    pub configuration: IdentitySourceConfiguration,
}

/// The provider an identity source points at — the `Configuration` union's two
/// arms, reduced to what this service persists.
#[derive(Debug, Clone)]
pub enum IdentitySourceConfiguration {
    CognitoUserPool(CognitoUserPoolSource),
    OpenIdConnect(OpenIdConnectSource),
}

impl IdentitySourceConfiguration {
    /// A canonical rendering of this configuration, for the idempotency ledger:
    /// two `CreateIdentitySource` calls carrying one `clientToken` are the same
    /// call only when every member of their configurations agrees. Spelled out
    /// member by member rather than taken from `#[derive(Debug)]`, so what
    /// counts as "different parameters" is a stated contract that a later field
    /// rename cannot silently change.
    pub fn fingerprint(&self) -> String {
        match self {
            Self::CognitoUserPool(cognito) => format!(
                "cognito:userPoolArn={};clientIds={:?};groupEntityType={:?}",
                cognito.user_pool_arn, cognito.client_ids, cognito.group_entity_type
            ),
            Self::OpenIdConnect(oidc) => {
                let groups = oidc.group_configuration.as_ref().map(|groups| {
                    (groups.group_claim.as_str(), groups.group_entity_type.as_str())
                });
                let tokens = match &oidc.token_selection {
                    OpenIdConnectTokens::AccessToken {
                        principal_id_claim,
                        audiences,
                    } => format!("accessTokenOnly:claim={principal_id_claim};audiences={audiences:?}"),
                    OpenIdConnectTokens::IdentityToken {
                        principal_id_claim,
                        client_ids,
                    } => format!("identityTokenOnly:claim={principal_id_claim};clientIds={client_ids:?}"),
                };
                format!(
                    "oidc:issuer={};entityIdPrefix={:?};groups={:?};tokenSelection={}",
                    oidc.issuer, oidc.entity_id_prefix, groups, tokens
                )
            }
        }
    }
}

/// An Amazon Cognito user pool identity source. `region` and `user_pool_id` are
/// the segments [`resolve_user_pool_arn`] read out of `user_pool_arn`, so the
/// derived issuer and discovery URL below are computed from the ARN rather than
/// taken from the request.
#[derive(Debug, Clone)]
pub struct CognitoUserPoolSource {
    pub user_pool_arn: String,
    pub user_pool_id: String,
    pub region: String,
    pub client_ids: Vec<String>,
    /// `groupConfiguration.groupEntityType`, when the request set one.
    pub group_entity_type: Option<String>,
}

impl CognitoUserPoolSource {
    /// The pool's OIDC issuer, in the form the model documents for
    /// `CognitoUserPoolConfigurationDetail$issuer`:
    /// `https://cognito-idp.<region>.amazonaws.com/<user-pool-id>`.
    pub fn issuer(&self) -> String {
        format!(
            "https://cognito-idp.{}.amazonaws.com/{}",
            self.region, self.user_pool_id
        )
    }

    /// The pool's OIDC discovery URL, in the form the model documents for
    /// `IdentitySourceDetails$discoveryUrl`:
    /// `https://cognito-idp.<region>.amazonaws.com/<user-pool-id>/.well-known/openid-configuration`.
    pub fn discovery_url(&self) -> String {
        format!("{}/.well-known/openid-configuration", self.issuer())
    }
}

/// An OpenID Connect identity source.
#[derive(Debug, Clone)]
pub struct OpenIdConnectSource {
    pub issuer: String,
    pub entity_id_prefix: Option<String>,
    /// Both members of `groupConfiguration` are required when it is present, so
    /// the pair is stored — and reported — together or not at all.
    pub group_configuration: Option<OpenIdConnectGroups>,
    pub token_selection: OpenIdConnectTokens,
}

/// `OpenIdConnectGroupConfiguration`: which token claim carries group
/// membership, and the entity type it maps to.
#[derive(Debug, Clone)]
pub struct OpenIdConnectGroups {
    pub group_claim: String,
    pub group_entity_type: String,
}

/// `OpenIdConnectTokenSelection`: a policy store processes either the access
/// token or the identity token from one OIDC provider, never both.
#[derive(Debug, Clone)]
pub enum OpenIdConnectTokens {
    AccessToken {
        principal_id_claim: String,
        audiences: Vec<String>,
    },
    IdentityToken {
        principal_id_claim: String,
        client_ids: Vec<String>,
    },
}

/// The region and user pool id a Cognito `userPoolArn` names, or why this
/// emulator cannot resolve the pool it names.
///
/// Verified Permissions resolves the user pool before it creates the identity
/// source, and the pool has to be one the service can reach: AWS documents that
/// the user pool must be in the same account and Region as the policy store.
/// This emulator presents exactly one account ([`engine::emulator_account_id`])
/// and serves the region the request was signed for, so an ARN naming any other
/// account or region names a pool that does not exist here. The pool id itself
/// carries the region it was created in (`<region>_<suffix>`), so an id that
/// disagrees with its own ARN names no pool either.
///
/// The `Err` is an [`crate::ops::InvalidInput`] naming
/// `configuration.cognitoUserPoolConfiguration.userPoolArn`, so the caller is
/// told which member — and which segment of the ARN in it — to fix.
///
/// What this CANNOT check: whether a pool with that id was ever created. The
/// cognito-idp emulator holds its user pools in its own state struct, and
/// [`service_state`] reaches per-service state BY TYPE — that type lives in the
/// cognito-idp crate, which this crate does not depend on (one crate per
/// service), and no shared crate publishes a pool registry. The partition, the
/// account, the region and the region the pool id names are therefore the whole
/// of the resolution this service can perform.
pub fn resolve_user_pool_arn(
    arn: &str,
    request_region: &str,
) -> Result<(String, String), crate::ops::InvalidInput> {
    let invalid = |reason: String| {
        crate::ops::InvalidInput::new(crate::ops::paths::COGNITO_USER_POOL_ARN, reason)
    };
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let ["arn", partition, "cognito-idp", region, account, resource] = fields[..] else {
        return Err(invalid(format!(
            "{arn} is not an Amazon Cognito user pool ARN"
        )));
    };
    let Some(user_pool_id) = resource.strip_prefix("userpool/") else {
        return Err(invalid(format!("{arn} does not name a user pool")));
    };
    // A resource in another partition is not reachable from this one at all, so
    // the partition is resolved like the account and the region below.
    let own_partition = emulator_partition();
    if partition != own_partition {
        return Err(invalid(format!(
            "no Amazon Cognito user pool {user_pool_id} exists in partition {partition}; an \
             identity source can only reference a user pool in partition {own_partition}"
        )));
    }
    let own_account = ::engine::emulator_account_id();
    if account != own_account {
        return Err(invalid(format!(
            "no Amazon Cognito user pool {user_pool_id} exists in account {account}; an \
             identity source can only reference a user pool in account {own_account}"
        )));
    }
    if region != request_region {
        return Err(invalid(format!(
            "no Amazon Cognito user pool {user_pool_id} exists in region {region}; an identity \
             source can only reference a user pool in its policy store's region \
             {request_region}"
        )));
    }
    if !user_pool_id.starts_with(&format!("{region}_")) {
        return Err(invalid(format!(
            "no Amazon Cognito user pool {user_pool_id} exists in region {region}; a user pool \
             id names the region the pool was created in"
        )));
    }
    Ok((region.to_string(), user_pool_id.to_string()))
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. A Cognito user pool is reachable only from the
/// region it lives in, and an alias ARN names the region the alias was created
/// in, so the region is read off the request rather than fixed.
///
/// The scope's region is honoured only when it has an AWS region's SHAPE
/// ([`is_region_shaped`]): it is interpolated into an ARN, so an arbitrary
/// header value must not be able to shape one. The scope itself is parsed by
/// `emulator_server::sigv4_credential_scope`, the framework's single source of
/// truth for it.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` has an AWS region code's shape:
/// `<partition>-<area>[-<area>]-<number>` (`us-east-1`, `ap-southeast-3`,
/// `us-gov-west-1`) — at least TWO lowercase alphabetic segments joined by `-`,
/// plus a trailing all-digit segment.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The ARN partition this emulator presents, read out of an ARN it builds
/// itself ([`engine::emulator_arn`]) rather than written as a literal, so the
/// two can never disagree.
pub fn emulator_partition() -> String {
    ::engine::emulator_arn("verifiedpermissions", "", "")
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// One policy in a store. `statement` is the Cedar text exactly as it arrived;
/// every derived field below is computed from it by [`cedar::parse_static_policy`].
#[derive(Debug, Clone)]
pub struct Policy {
    pub policy_id: String,
    pub policy_store_id: String,
    /// `STATIC` or `TEMPLATE_LINKED`.
    pub policy_type: String,
    pub name: Option<String>,
    /// The Cedar policy text, byte-identical to what `CreatePolicy` was given.
    pub statement: String,
    pub description: Option<String>,
    /// `Permit` or `Forbid`, from the parsed statement's effect.
    pub effect: Option<String>,
    /// The principal the statement's scope names, when it names one.
    pub principal: Option<ScopeEntity>,
    /// The resource the statement's scope names, when it names one.
    pub resource: Option<ScopeEntity>,
    /// The actions the statement's scope names; empty when the scope is `action`.
    pub actions: Vec<ScopeEntity>,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
}

/// One policy template in a store. `statement` is the Cedar template text
/// exactly as it arrived — placeholders (`?principal`, `?resource`) included —
/// and it is parsed by [`cedar::parse_policy_template`] before it is stored, so
/// a store never holds a template that is not valid Cedar.
#[derive(Debug, Clone)]
pub struct PolicyTemplate {
    pub policy_template_id: String,
    pub policy_store_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The Cedar template text, byte-identical to what `CreatePolicyTemplate`
    /// was given.
    pub statement: String,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
}

/// Why a `policyStoreId` request member named no store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLookupMiss {
    /// The reference was `policy-store-alias/<name>` and no alias is named
    /// `<name>`.
    AliasNotFound,
    /// The reference named a store id — directly, or through an alias that does
    /// resolve — that no policy store has.
    StoreNotFound,
}

/// A `type::"id"` pair, as both `EntityIdentifier` and `ActionIdentifier` carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEntity {
    pub entity_type: String,
    pub entity_id: String,
}

impl ComAmazonawsVerifiedpermissionsVerifiedpermissionsState {
    /// Mint the next `policyStoreId`.
    ///
    /// `PS` + 22 base62 characters, the shape AWS publishes
    /// (`PSEXAMPLEabcdefg111111`) and well inside `PolicyStoreId`'s
    /// `^[a-zA-Z0-9-/_]*$`, length 1..200. The suffix is
    /// [`scramble`]d from a strictly increasing counter, which is a bijection, so
    /// two stores never share an id.
    pub fn mint_policy_store_id(&mut self) -> String {
        self.next_store_seq += 1;
        format!("PS{}", base62(scramble(self.next_store_seq)))
    }

    /// Resolve a `policyStoreId` request member: either a store id, or
    /// `policy-store-alias/<name>` naming one.
    ///
    /// Answers `Some` ONLY for a store that is present, on both branches — a
    /// caller-supplied reference must never resolve to a key that is not in
    /// `policy_stores`, or a handler that then looks the key up has no store to
    /// find. An alias resolves only while it is [`ALIAS_STATE_ACTIVE`]:
    /// `DeletePolicyStoreAlias` documents that once an alias is in
    /// `PendingDeletion`, "if the policy store alias is used in an API that has a
    /// `policyStoreId` field, the operation will fail with a `ResourceNotFound`
    /// exception".
    pub fn resolve_store_id(&self, reference: &str) -> Option<String> {
        self.lookup_store_id(reference).ok()
    }

    /// [`Self::resolve_store_id`], but saying WHICH reference failed: an alias
    /// name no active alias carries is a different miss from a store id no store
    /// has, and the batch operations report the two with distinct codes.
    pub fn lookup_store_id(&self, reference: &str) -> Result<String, StoreLookupMiss> {
        let candidate = if reference.starts_with(ALIAS_PREFIX) {
            self.aliases
                .get(reference)
                .filter(|alias| alias.state == ALIAS_STATE_ACTIVE)
                .ok_or(StoreLookupMiss::AliasNotFound)?
                .policy_store_id
                .clone()
        } else {
            reference.to_string()
        };
        self.policy_stores
            .contains_key(&candidate)
            .then_some(candidate)
            .ok_or(StoreLookupMiss::StoreNotFound)
    }

    /// Resolve a `resourceArn` request member to the `policyStoreId` of the
    /// resource it names.
    ///
    /// In Verified Permissions the taggable resource is the policy store, so an
    /// ARN is matched against the ARNs `CreatePolicyStore` minted and PERSISTED
    /// rather than re-parsed into an id: an ARN that names no live resource then
    /// never resolves, whatever its shape, and the caller gets the
    /// `ResourceNotFoundException` every tagging operation models.
    pub fn resolve_resource_arn(&self, resource_arn: &str) -> Option<String> {
        self.policy_stores
            .values()
            .find(|store| store.arn == resource_arn)
            .map(|store| store.policy_store_id.clone())
    }

    /// The resource an unexpired `clientToken` already created for `operation`,
    /// plus the parameters it was created with.
    pub fn idempotency_replay(
        &mut self,
        operation: &str,
        token: &str,
    ) -> Option<IdempotencyRecord> {
        let now = SystemTime::now();
        // Materialise the expiry the clock already made due before answering, so
        // a token past its documented eight hours is not reported as a replay.
        self.client_tokens.retain(|_, record| {
            now.duration_since(record.recorded_at)
                .is_ok_and(|age| age < CLIENT_TOKEN_TTL)
        });
        self.client_tokens
            .get(&(operation.to_string(), token.to_string()))
            .cloned()
    }

    /// Remember that `token` created `resource_id` with `parameters` under
    /// `operation`.
    pub fn record_idempotency(
        &mut self,
        operation: &str,
        token: &str,
        resource_id: &str,
        parameters: String,
    ) {
        self.client_tokens.insert(
            (operation.to_string(), token.to_string()),
            IdempotencyRecord {
                resource_id: resource_id.to_string(),
                parameters,
                recorded_at: SystemTime::now(),
            },
        );
        // Keep the ledger inside [`MAX_CLIENT_TOKENS`] by dropping the oldest
        // records first, so a caller presenting a new token every request cannot
        // grow it without limit.
        while self.client_tokens.len() > MAX_CLIENT_TOKENS {
            let Some(oldest) = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.client_tokens.remove(&oldest);
        }
    }
}

impl PolicyStore {
    /// The schema document this store holds, as a reader reports it.
    ///
    /// A store that has never been given a schema holds the EMPTY document
    /// ([`EMPTY_SCHEMA_JSON`]), dated at the store's own creation: the schema is
    /// one document of the store, in the state a caller also returns it to by
    /// putting `{}`. It is not a MISSING resource, which is why `GetSchema`
    /// answers 200 for a store that exists and has never been given a schema —
    /// the behaviour the recorded `get-schema-lifecycle` unit pins. Nothing is
    /// written to reach this answer; the dates come from the persisted store.
    pub fn schema_document(&self) -> StoredSchema {
        self.schema.clone().unwrap_or_else(|| StoredSchema {
            cedar_json: EMPTY_SCHEMA_JSON.to_string(),
            namespaces: Vec::new(),
            created_date: self.created_date,
            last_updated_date: self.created_date,
        })
    }

    /// Mint the next `policyId` for this store. `SP` + 22 base62 characters, the
    /// shape AWS publishes (`SPEXAMPLEabcdefg111111`).
    pub fn mint_policy_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!("SP{}", base62(scramble(self.next_id_seq)))
    }

    /// Mint the next `policyTemplateId` for this store. `PT` + 22 base62
    /// characters, the shape AWS publishes (`PTEXAMPLEabcdefg111111`), and well
    /// inside `PolicyTemplateId`'s `^[a-zA-Z0-9-/_]*$`, length 1..200.
    pub fn mint_policy_template_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!("PT{}", base62(scramble(self.next_id_seq)))
    }

    /// Resolve a `policyTemplateId` request member: either a template id, or
    /// `name/<policyTemplateName>` naming one, which `GetPolicyTemplate`
    /// documents as interchangeable with the id ("You can use the policy
    /// template name in place of the policy template ID. When using a name,
    /// prefix it with name/").
    pub fn resolve_policy_template_id(&self, reference: &str) -> Option<String> {
        match reference.strip_prefix("name/") {
            Some(name) => self
                .policy_templates
                .values()
                .find(|template| template.name.as_deref() == Some(name))
                .map(|template| template.policy_template_id.clone()),
            None => self
                .policy_templates
                .contains_key(reference)
                .then(|| reference.to_string()),
        }
    }

    /// Mint the next `identitySourceId` for this store. `IS` + 22 base62
    /// characters, the shape AWS publishes (`ISEXAMPLEabcdefg111111`) and well
    /// inside `IdentitySourceId`'s `^[a-zA-Z0-9-]*$`, length 1..200. It draws on
    /// the same strictly increasing counter as [`Self::mint_policy_id`], so no
    /// two ids minted in one store ever share a suffix.
    pub fn mint_identity_source_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!("IS{}", base62(scramble(self.next_id_seq)))
    }

    /// Resolve a `policyId` request member: either a policy id, or
    /// `name/<policyName>` naming one, which the API documents as interchangeable
    /// with the id.
    pub fn resolve_policy_id(&self, reference: &str) -> Option<String> {
        match reference.strip_prefix("name/") {
            Some(name) => self
                .policies
                .values()
                .find(|policy| policy.name.as_deref() == Some(name))
                .map(|policy| policy.policy_id.clone()),
            None => self
                .policies
                .contains_key(reference)
                .then(|| reference.to_string()),
        }
    }

    /// The total bytes of Cedar text this store holds — its policies AND its
    /// policy templates, the quantity [`MAX_POLICY_BYTES_PER_STORE`] bounds.
    pub fn cedar_bytes(&self) -> usize {
        self.policies
            .values()
            .map(|policy| policy.statement.len())
            .chain(
                self.policy_templates
                    .values()
                    .map(|template| template.statement.len()),
            )
            .sum()
    }
}

/// SplitMix64's finalizer: a BIJECTION on `u64`, so distinct counter values give
/// distinct — and unguessably spread — outputs. That is what makes a minted id
/// both unique and AWS-shaped; a plain counter would be unique but sequential.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render `value` as exactly 22 base62 characters (zero-padded), the width a
/// `u64` needs: 62^11 > 2^64, but AWS ids are longer, so the value is rendered
/// twice-folded to fill the published width.
fn base62(value: u64) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    // Two independent 11-digit halves: the value itself and its scramble, so all
    // 22 positions carry entropy instead of 11 zeros.
    let mut out = String::with_capacity(22);
    for mut v in [value, scramble(value ^ 0x5DEE_CE66_D000_0000)] {
        let mut half = [b'0'; 11];
        for slot in half.iter_mut().rev() {
            *slot = ALPHABET[(v % 62) as usize];
            v /= 62;
        }
        // Every byte came out of ALPHABET, which is ASCII, so each is its own
        // `char`; pushing them individually keeps this infallible rather than
        // routing through a UTF-8 check that could only fail if ALPHABET changed.
        out.extend(half.iter().map(|b| char::from(*b)));
    }
    out
}

/// The resource id an ARN names: its last non-empty `/`- or `:`-separated
/// segment (`arn:aws:verifiedpermissions::0:policy-store/PS…` -> `PS…`).
///
/// Used only to fill `ResourceNotFoundException$resourceId`, which the model
/// makes required, for a request that named a resource by ARN and found none.
pub fn resource_id_from_arn(resource_arn: &str) -> String {
    resource_arn
        .rsplit(['/', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(resource_arn)
        .to_string()
}

/// `ResourceType$POLICY_STORE`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_POLICY_STORE: &str = "POLICY_STORE";

/// `ResourceType$POLICY_STORE_ALIAS`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_POLICY_STORE_ALIAS: &str = "POLICY_STORE_ALIAS";

/// `ResourceType$POLICY`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_POLICY: &str = "POLICY";

/// `ResourceType$POLICY_TEMPLATE`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_POLICY_TEMPLATE: &str = "POLICY_TEMPLATE";

/// `ResourceType$SCHEMA`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_SCHEMA: &str = "SCHEMA";

/// `ResourceType$IDENTITY_SOURCE`, from the model's `ResourceType` enum.
pub const RESOURCE_TYPE_IDENTITY_SOURCE: &str = "IDENTITY_SOURCE";

/// The resource-path prefix a `policyId` member carries when it names a policy by
/// name instead of by id, which `resolve_policy_id` already accepts.
pub const POLICY_NAME_PREFIX: &str = "name/";

/// What a caller-supplied identifier reference NAMES: the kind of resource, and
/// that resource's own id.
///
/// Several identifier members of this API accept either a bare id or a
/// `<resource-path>/<name>` form documented as interchangeable with it — a
/// `policyStoreId` takes `policy-store-alias/<name>`, a `policyId` takes
/// `name/<policyName>`. `ResourceNotFoundException` models `resourceId` as "the
/// unique ID of the resource referenced in the failed request" and `resourceType`
/// as "the resource type of the resource referenced in the failed request", so
/// when such a reference does not resolve the error names the KIND the path named
/// and the BARE name — never the path, which is not an id a caller can feed back
/// into the API, and never the kind the member usually carries.
#[derive(Debug, Clone)]
pub struct ReferencedResource {
    /// A `ResourceType` enum value: the kind of resource the reference named.
    pub resource_type: String,
    /// The named resource's own id, with any resource-path prefix removed.
    pub resource_id: String,
}

impl ReferencedResource {
    /// The modelled `ResourceNotFoundException` for this reference not resolving.
    ///
    /// The message comes from [`crate::ops::resource_not_found`], the one place
    /// this service builds the shape, so every not-found path answers the same
    /// sentence; what this type contributes is the identity of the resource the
    /// reference NAMED, which is what the exception's required `resourceId` and
    /// `resourceType` members model.
    pub fn not_found(self) -> crate::ops::types::ResourceNotFoundException {
        crate::ops::resource_not_found(&self.resource_id, &self.resource_type)
    }
}

/// Split `reference` on one documented resource path: a reference carrying
/// `prefix` names a `prefixed_type` by its bare name, anything else names a
/// `bare_type` by its id.
fn split_reference(
    reference: &str,
    prefix: &str,
    prefixed_type: &str,
    bare_type: &str,
) -> ReferencedResource {
    match reference.strip_prefix(prefix) {
        Some(name) => ReferencedResource {
            resource_type: prefixed_type.to_string(),
            resource_id: name.to_string(),
        },
        None => ReferencedResource {
            resource_type: bare_type.to_string(),
            resource_id: reference.to_string(),
        },
    }
}

/// What a `policyStoreId` request member names: a policy store by id, or — when
/// it carries [`ALIAS_PREFIX`], the form [`ComAmazonawsVerifiedpermissionsVerifiedpermissionsState::resolve_store_id`]
/// resolves — a policy store alias by name.
pub fn policy_store_reference(reference: &str) -> ReferencedResource {
    split_reference(
        reference,
        ALIAS_PREFIX,
        RESOURCE_TYPE_POLICY_STORE_ALIAS,
        RESOURCE_TYPE_POLICY_STORE,
    )
}

/// What a `policyId` request member names: a policy by id, or — when it carries
/// [`POLICY_NAME_PREFIX`], the form [`PolicyStore::resolve_policy_id`] resolves —
/// a policy by name. Either way the resource is a policy.
pub fn policy_reference(reference: &str) -> ReferencedResource {
    split_reference(
        reference,
        POLICY_NAME_PREFIX,
        RESOURCE_TYPE_POLICY,
        RESOURCE_TYPE_POLICY,
    )
}

/// What an `aliasName` request member names: always a policy store alias, whose
/// own id is the name without its documented [`ALIAS_PREFIX`] resource path.
pub fn alias_reference(alias_name: &str) -> ReferencedResource {
    split_reference(
        alias_name,
        ALIAS_PREFIX,
        RESOURCE_TYPE_POLICY_STORE_ALIAS,
        RESOURCE_TYPE_POLICY_STORE_ALIAS,
    )
}

/// Encode a pagination cursor: the key the next page starts at, hex-encoded so
/// the token is opaque to the caller and matches `NextToken`'s
/// `^[A-Za-z0-9-_=+/\.]*$`.
pub fn encode_page_token(next_key: &str) -> String {
    hex::encode(next_key.as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]. `None` when the token was
/// not produced by this service, which the caller refuses as invalid.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The ARN of a policy store alias, in `region`.
///
/// The alias name already carries its [`ALIAS_PREFIX`], which is the resource
/// segment's type, so the resource part is that name verbatim — the same string
/// `aliases` is keyed on, so the ARN and the state key cannot disagree.
/// `CreatePolicyStoreAlias` refuses a name without the prefix before anything
/// reaches here, which the assertion states rather than silently repairing.
///
/// Verified Permissions is a REGIONAL service and an alias name "is unique in
/// the account and Region", so the alias ARN's third colon-separated field — the
/// region, per `ResourceArn`'s `^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$` — is the region
/// the request was signed for ([`request_region`]). A present-but-empty region
/// segment shifts nothing on the wire but makes the ARN match no ARN pattern or
/// resource-based policy written for the real service.
///
/// The policy-store ARN `CreatePolicyStore` mints is deliberately NOT built this
/// way: AWS publishes policy-store ARNs without a region
/// (`arn:aws:verifiedpermissions::<account>:policy-store/<id>`), which is also
/// what the recorded responses carry, so that ARN keeps its empty region segment.
pub fn alias_arn(region: &str, alias_name: &str) -> String {
    debug_assert!(
        alias_name.starts_with(ALIAS_PREFIX),
        "an alias name reaching alias_arn carries {ALIAS_PREFIX}, got {alias_name}"
    );
    ::engine::emulator_arn(ARN_SERVICE, region, alias_name)
}

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`arnNamespace: verifiedpermissions`).
pub const ARN_SERVICE: &str = "verifiedpermissions";

/// A canonical, UNAMBIGUOUS rendering of the parameters one `clientToken` was
/// first used with, for the [`IdempotencyRecord::parameters`] field: a later
/// call carrying the same token but a different rendering is the
/// `ConflictException` the model documents.
///
/// Every value is written through `{:?}`, which quotes and escapes it, so a
/// member whose value contains the `=` or `;` separators cannot make two
/// different requests render alike.
pub fn idempotency_parameters(members: &[(&str, Option<&str>)]) -> String {
    members
        .iter()
        .map(|(name, value)| format!("{name}={value:?};"))
        .collect()
}

/// `SystemTime::now()` as the model's `date-time` timestamp.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>)
}
