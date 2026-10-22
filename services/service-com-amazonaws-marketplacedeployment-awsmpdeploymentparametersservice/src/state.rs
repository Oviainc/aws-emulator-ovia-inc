// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice service.
//!
//! The deployment-parameter lifecycle is backed here: every value
//! `PutDeploymentParameter`, `TagResource`, `UntagResource` and
//! `ListTagsForResource` report is read out of one of these records, never built
//! from the request that asked for it.
//!
//! The AGREEMENTS a deployment parameter can be written against are held here
//! too ([`ProvisionedAgreement`]). This API models no operation that creates a
//! product or an agreement — in AWS an agreement appears when a buyer accepts a
//! seller's offer, entirely out of band of this service — so that store is
//! populated through [`seed_agreements`], the emulator's fixture/admin seam
//! standing in for that out-of-band creation, and `PutDeploymentParameter` only
//! ever reads it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    ConflictException, ResourceNotFoundException, ServiceQuotaExceededException, ValidationException,
};

/// This service's ARN namespace and sigv4 signing name
/// (`aws.api#service$arnNamespace` / `aws.auth#sigv4$name` are both
/// `aws-marketplace`).
pub const ARN_NAMESPACE: &str = "aws-marketplace";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; `engine::emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The catalogs this service serves.
///
/// MODELLED, not chosen: `PutDeploymentParameterRequest$catalog` documents "The catalog
/// related to the request. Fixed value: `AWSMarketplace`"
/// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`).
/// So the `Catalog` shape's `length 1..64` / `^[a-zA-Z_-]+$` is the SYNTAX a catalog name
/// takes, and this list is the set of catalogs that EXIST: a syntactically valid value
/// outside it names no catalog at all, and the request that carries it is refused as
/// invalid input rather than looked up.
///
/// A list rather than a single constant because the member's constraint is an
/// allowed-value set that AWS can extend; everything below reads the set, so adding an
/// entry here is the whole change.
pub const SUPPORTED_CATALOGS: &[&str] = &["AWSMarketplace"];

/// Whether `candidate` names a catalog this service serves ([`SUPPORTED_CATALOGS`]).
///
/// Compared exactly: `Catalog` carries no `@pattern` casing freedom and AWS's own
/// catalog names are case-sensitive, so a value differing only in case names no catalog.
pub fn is_supported_catalog(candidate: &str) -> bool {
    SUPPORTED_CATALOGS.contains(&candidate)
}

/// The modelled `ValidationException` (HTTP 400) for a `catalog` that names no catalog
/// this service serves.
///
/// The status and the code are what the model fixes; the sentence names the member and
/// lists the values that ARE supported, read off [`SUPPORTED_CATALOGS`] — so it cannot
/// drift from what the service accepts. `fieldName` names the member the way the
/// validation report's own paths spell it (lowerCamel, as the request carries it).
pub fn unsupported_catalog() -> ValidationException {
    let supported = SUPPORTED_CATALOGS
        .iter()
        .map(|catalog| format!("\"{catalog}\""))
        .collect::<Vec<_>>()
        .join(", ");
    ValidationException {
        message: format!(
            "Catalog: The provided Catalog is invalid. Provide a valid Catalog. \
             Supported values are [{supported}]."
        ),
        field_name: "catalog".to_string(),
    }
}

/// The resource portion prefix of a deployment-parameter ARN.
///
/// The ARN's resource segment is the minted `dp-…` id ALONE. It deliberately
/// does not embed `catalog` or `productId`: those are `ResourceId`/`Catalog`
/// values, whose patterns admit `_`, while the modelled `ResourceArn` pattern is
/// `^[a-zA-Z0-9:*/-]+$` — so an ARN carrying them could not satisfy its own
/// shape. The id is unique on its own, which is all the tagging operations need
/// to resolve by.
pub const ARN_RESOURCE_PREFIX: &str = "deployment-parameter/";

/// The number of tags one deployment parameter holds.
///
/// MODELLED, not chosen: `PutDeploymentParameterRequest$tags` and
/// `PutDeploymentParameterResponse$tags` are a `TagsMap` with `length max 50`
/// (`smithy_query(query="shape",shape="TagsMap")`), so a stored set larger than
/// this could not be reported back through the response at all.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Longest tag key this service stores — the modelled `TagKey` `length 1..128`.
pub const TAG_KEY_MAX_LEN: usize = 128;

/// Longest tag value this service stores — the modelled `TagValue`
/// `length 1..256`.
pub const TAG_VALUE_MAX_LEN: usize = 256;

/// The characters a tag key or value may hold: the expression the modelled
/// `TagKey` / `TagValue` `@pattern` states, WITHOUT the `^` / `$` the model
/// spells it with.
///
/// Smithy's `@pattern` is an unanchored find, so those two characters are how the
/// model makes the match total; they are not part of the character class a client
/// may send, and a message that quotes them reports a pattern the caller never
/// sent. The model's own form is `^[a-zA-Z0-9/_+=.:@-]+$`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
pub const TAG_CHARACTER_CLASS: &str = "[a-zA-Z0-9/_+=.:@-]+";

/// The number of deployment parameters this emulator holds at once.
///
/// `PutDeploymentParameter` models `ServiceQuotaExceededException` (HTTP 402)
/// for passing an account maximum. AWS publishes no figure, so the number
/// itself is CHOSEN: its purpose is that the store cannot grow without bound,
/// and that the ceiling is reached through the modelled error rather than by
/// exhausting memory.
pub const MAX_DEPLOYMENT_PARAMETERS: usize = 500;

/// The `ServiceCode` this service reports in its quota error: its own signing
/// name, which is also its ARN namespace.
pub const SERVICE_CODE: &str = "aws-marketplace";

/// How many `@idempotencyToken` entries the replay ledger keeps.
///
/// The ledger grows by one per `PutDeploymentParameter` that carries a fresh
/// `clientToken` — including on the update-in-place path, which mints nothing and
/// so is not covered by [`MAX_DEPLOYMENT_PARAMETERS`] — so it needs a bound of its
/// own: it is capped here with FIFO eviction of the oldest token
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::remember_client_token`]).
/// A token evicted past the cap simply stops replaying and the request is served as
/// an ordinary create-or-update, which is the same answer the caller would get from
/// a token the service never saw.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// One deployment parameter, as written by `PutDeploymentParameter`.
///
/// `Debug` is hand-written so the secret never reaches a log or a panic message:
/// `DeploymentParameterInput$secretString` is `@sensitive`.
#[derive(Clone)]
pub struct DeploymentParameterRecord {
    /// The ARN minted for this record; the key both tagging operations resolve by.
    pub resource_arn: String,
    /// The minted `dp-…` id, matching `^dp-[a-zA-Z0-9]+$` within `len 1..32`.
    pub deployment_parameter_id: String,
    /// The catalog the record was written under.
    pub catalog: String,
    /// The product the record was written for.
    pub product_id: String,
    /// The agreement the record is targeted at.
    pub agreement_id: String,
    /// The parameter name: the identifier deployment parameters are keyed on for
    /// a given buyer and product.
    pub name: String,
    /// The secret text last written for this parameter. `@sensitive`: no
    /// operation reports it and nothing logs it.
    pub secret_string: String,
    /// When the parameter expires and is scheduled for deletion, when the last
    /// write named a date.
    pub expiration_date: Option<::aws_smithy_types::DateTime>,
    /// Tags, keyed so a later `TagResource` with the same key overwrites.
    pub tags: BTreeMap<String, String>,
    /// When the record was first written.
    pub created_at: SystemTime,
    /// When the record was last written.
    pub updated_at: SystemTime,
}

impl std::fmt::Debug for DeploymentParameterRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentParameterRecord")
            .field("resource_arn", &self.resource_arn)
            .field("deployment_parameter_id", &self.deployment_parameter_id)
            .field("catalog", &self.catalog)
            .field("product_id", &self.product_id)
            .field("agreement_id", &self.agreement_id)
            .field("name", &self.name)
            // `secretString` is @sensitive: its length is enough to debug with.
            .field("secret_string", &format_args!("<{} bytes>", self.secret_string.len()))
            .field("expiration_date", &self.expiration_date)
            .field("tags", &self.tags)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl DeploymentParameterRecord {
    /// The tag map as the modelled `TagsMap` / `Tags` wire shape.
    pub fn tags_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// One AWS Marketplace agreement this emulator holds, and the products it
/// covers — the target `PutDeploymentParameter` writes against.
///
/// `PutDeploymentParameter` "creates or updates a deployment parameter and is
/// targeted by `catalog` and `agreementId`" for the product the request
/// addresses, and models `ResourceNotFoundException` (HTTP 404) for a target the
/// account does not hold. Nothing in THIS model creates an agreement or a
/// product, so a record gets here through [`seed_agreements`].
#[derive(Debug, Clone, Default)]
pub struct ProvisionedAgreement {
    /// The catalog the agreement lives in; a request naming another catalog does
    /// not reach it.
    pub catalog: String,
    /// The agreement's own identifier, as `PutDeploymentParameter$agreementId`
    /// names it.
    pub agreement_id: String,
    /// The products this agreement covers, as
    /// `PutDeploymentParameter$productId` names them.
    pub product_ids: BTreeSet<String>,
}

impl ProvisionedAgreement {
    /// An agreement in `catalog` with no products yet.
    pub fn new(catalog: impl Into<String>, agreement_id: impl Into<String>) -> Self {
        Self {
            catalog: catalog.into(),
            agreement_id: agreement_id.into(),
            product_ids: BTreeSet::new(),
        }
    }

    /// The same agreement, also covering `product_id`.
    #[must_use]
    pub fn with_product(mut self, product_id: impl Into<String>) -> Self {
        self.product_ids.insert(product_id.into());
        self
    }

    /// The key this agreement is stored under: an agreement id is unique inside
    /// its catalog.
    pub fn key(&self) -> AgreementKey {
        (self.catalog.clone(), self.agreement_id.clone())
    }

    /// Whether this agreement covers `product_id`.
    pub fn covers(&self, product_id: &str) -> bool {
        self.product_ids.iter().any(|held| held == product_id)
    }
}

/// How a provisioned agreement is keyed: `(catalog, agreementId)`.
pub type AgreementKey = (String, String);

/// Which half of a write target the account does not hold.
///
/// Both answers are the one modelled `ResourceNotFoundException` (HTTP 404); the
/// member named is which of the two the request got wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingTarget {
    /// No agreement in the named catalog covers the named product.
    Product,
    /// The product is covered in that catalog, but not by the named agreement.
    Agreement,
}

impl MissingTarget {
    /// The input member this missing target names, as AWS reports it.
    pub fn member(self) -> &'static str {
        match self {
            Self::Product => "ProductId",
            Self::Agreement => "AgreementId",
        }
    }
}

/// The identity a deployment parameter is keyed on: the catalog, the product,
/// the agreement and the parameter name.
///
/// From the model's own doc for `DeploymentParameterInput$name`: it "is the
/// identifier on which deployment parameters are keyed for a given buyer and
/// product. If this name matches an existing deployment parameter, this request
/// will update the existing resource."
pub type ParameterKey = (String, String, String, String);

/// Per-service state for com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// Every stored deployment parameter, keyed by its minted `resourceArn`.
    pub parameters: BTreeMap<String, DeploymentParameterRecord>,
    /// The identity index that makes a repeat `PutDeploymentParameter` update
    /// the existing record in place: [`ParameterKey`] -> `resourceArn`.
    pub by_name: BTreeMap<ParameterKey, String>,
    /// `clientToken` -> `resourceArn`, for `@idempotencyToken` replay. Bounded by
    /// [`MAX_CLIENT_TOKENS`]; write it only through [`Self::remember_client_token`],
    /// which keeps [`Self::client_token_order`] in step.
    pub client_tokens: BTreeMap<String, String>,
    /// The agreements this account holds, keyed by [`AgreementKey`]. Created out
    /// of band of this API — see [`seed_agreements`] — and only ever READ by
    /// `PutDeploymentParameter`, which refuses a target that is not in here.
    pub agreements: BTreeMap<AgreementKey, ProvisionedAgreement>,
    /// The `client_tokens` keys in insertion order, so the ledger evicts the oldest
    /// token first once it is at [`MAX_CLIENT_TOKENS`].
    pub client_token_order: VecDeque<String>,
    /// Source of the unique `dp-…` identifiers.
    pub next_id: u64,
}

impl ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// Mint the next deployment-parameter id.
    ///
    /// `dp-` plus 16 hex digits is 19 characters of `^dp-[a-zA-Z0-9]+$`, inside
    /// the modelled `DeploymentParameterResourceIdentifier` `length 1..32`.
    pub fn mint_id(&mut self) -> String {
        self.next_id += 1;
        format!("dp-{:016x}", self.next_id)
    }

    /// Whether another deployment parameter fits inside
    /// [`MAX_DEPLOYMENT_PARAMETERS`].
    pub fn has_capacity(&self) -> bool {
        self.parameters.len() < MAX_DEPLOYMENT_PARAMETERS
    }

    /// Which half of the target `(catalog, product_id, agreement_id)` this
    /// account does not hold, or `None` when it holds the whole target.
    ///
    /// A deployment parameter is written against an agreement of the account,
    /// for a product that agreement covers, inside one catalog; a target the
    /// store does not hold is not a parameter that can be written, however
    /// well-formed its identifiers are. The PRODUCT is reported when no
    /// agreement of that catalog covers it — it is the parent resource of the
    /// URI the write is addressed to
    /// (`/catalogs/{catalog}/products/{productId}/deployment-parameters`) — and
    /// the AGREEMENT when the product is covered there but not by this
    /// agreement.
    pub fn missing_target(
        &self,
        catalog: &str,
        product_id: &str,
        agreement_id: &str,
    ) -> Option<MissingTarget> {
        if !self.catalog_covers_product(catalog, product_id) {
            return Some(MissingTarget::Product);
        }
        let holds_pair = self
            .agreements
            .get(&(catalog.to_string(), agreement_id.to_string()))
            .is_some_and(|agreement| agreement.covers(product_id));
        (!holds_pair).then_some(MissingTarget::Agreement)
    }

    /// Whether any agreement of `catalog` covers `product_id`.
    fn catalog_covers_product(&self, catalog: &str, product_id: &str) -> bool {
        self.agreements
            .values()
            .any(|agreement| agreement.catalog == catalog && agreement.covers(product_id))
    }

    /// Record that `client_token` wrote the parameter at `resource_arn`, evicting the
    /// oldest token once the ledger is at [`MAX_CLIENT_TOKENS`], so repeated writes
    /// cannot grow it without limit.
    pub fn remember_client_token(&mut self, client_token: String, resource_arn: String) {
        if self
            .client_tokens
            .insert(client_token.clone(), resource_arn)
            .is_none()
        {
            self.client_token_order.push_back(client_token);
        }
        while self.client_token_order.len() > MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

/// The ARN of the deployment parameter `id` in `region`.
pub fn deployment_parameter_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{ARN_RESOURCE_PREFIX}{id}"))
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern (`^[a-zA-Z0-9:*/-]+$`) admits only `[a-zA-Z0-9-]` there, so
/// anything else is refused here and the default is used instead: a generated
/// ARN can never carry text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `byte` is one of the characters [`TAG_CHARACTER_CLASS`] admits.
fn is_tag_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'/' | b'_' | b'+' | b'=' | b'.' | b':' | b'@' | b'-')
}

/// Whether `candidate` is a tag key this service can store and report: the
/// modelled `TagKey` `length 1..128` and [`TAG_CHARACTER_CLASS`].
pub fn is_tag_key(candidate: &str) -> bool {
    (1..=TAG_KEY_MAX_LEN).contains(&candidate.len()) && candidate.bytes().all(is_tag_byte)
}

/// Whether `candidate` is a tag value this service can store and report: the
/// modelled `TagValue` `length 1..256` and [`TAG_CHARACTER_CLASS`].
pub fn is_tag_value(candidate: &str) -> bool {
    (1..=TAG_VALUE_MAX_LEN).contains(&candidate.len()) && candidate.bytes().all(is_tag_byte)
}

/// Merge `incoming` onto `existing`, or say why the result could not be stored.
///
/// A key already present takes the new value. The merge is computed BEFORE
/// anything is stored, so a request the service cannot report back is refused
/// outright rather than growing the record.
///
/// `TagResource`'s own `tags` member targets the unconstrained `Tags` map, but
/// the tag set it writes is reported by `PutDeploymentParameter` through the
/// modelled `TagsMap` (`length max 50`, keys `TagKey`, values `TagValue`) — so a
/// write outside those bounds is refused with the modelled `ValidationException`
/// rather than stored unreportably. `incoming` is walked in key order, so the
/// member a refusal names does not depend on hash iteration order.
pub fn merge_tags(
    existing: &BTreeMap<String, String>,
    incoming: &HashMap<String, String>,
) -> Result<BTreeMap<String, String>, ValidationException> {
    let mut merged = existing.clone();
    let ordered: BTreeMap<&String, &String> = incoming.iter().collect();
    for (key, value) in ordered {
        if !is_tag_key(key) {
            return Err(ValidationException {
                message: format!(
                    "tag key '{key}' is outside the 1..{TAG_KEY_MAX_LEN} characters of {TAG_CHARACTER_CLASS} a tag key may hold"
                ),
                field_name: "tags".to_string(),
            });
        }
        if !is_tag_value(value) {
            return Err(ValidationException {
                message: format!(
                    "the value of tag key '{key}' is outside the 1..{TAG_VALUE_MAX_LEN} characters of {TAG_CHARACTER_CLASS} a tag value may hold"
                ),
                field_name: "tags".to_string(),
            });
        }
        merged.insert(key.clone(), value.clone());
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(ValidationException {
            message: format!(
                "the request would leave the resource with {} tags; a deployment parameter holds at most {MAX_TAGS_PER_RESOURCE}",
                merged.len()
            ),
            field_name: "tags".to_string(),
        });
    }
    Ok(merged)
}

/// The not-found failure for an ARN that names no stored deployment parameter:
/// HTTP 404 with the error code `NotFoundException`.
///
/// NOT the modelled `ResourceNotFoundException`, whose shape name would reach the
/// wire as the error code: this service's recorded responses answer a resource ARN
/// it holds no record for with the code `NotFoundException`
/// ([`crate::scaffolded::NOT_FOUND_CODE`]) and the constant text
/// [`crate::scaffolded::NOT_FOUND_TEXT`], at the same 404 the modelled shape
/// carries, and a client branches on the code. It names no ARN because the service
/// names none. Generic over the operation's modelled error enum, so every operation
/// that resolves a caller-supplied resource ARN answers the one failure.
pub fn parameter_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found()
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for a write aimed at a
/// product/agreement target the account does not hold.
///
/// The status and the code are what the model fixes; the sentence names which of
/// the two members the request got wrong, the way AWS's own report does.
pub fn target_not_found(missing: MissingTarget) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "{}: Request references a resource which does not exist.",
            missing.member()
        ),
    }
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a write that
/// would pass [`MAX_DEPLOYMENT_PARAMETERS`].
pub fn parameter_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "this account already holds {held} deployment parameters, the maximum for this quota"
        ),
    }
}

/// The modelled `ConflictException` (HTTP 409) for a `clientToken` already used
/// to write a DIFFERENT deployment parameter.
///
/// `clientToken` is the request's `@idempotencyToken` — "a unique identifier for
/// the new version" — so one token identifies one write; reusing it for another
/// parameter is a conflicting request, not a replay. `resourceId` names the
/// parameter the token is already bound to, which satisfies the modelled
/// `ResourceId` pattern `^[A-Za-z0-9_/-]+$` by construction.
pub fn client_token_conflict(
    client_token: &str,
    bound_parameter_id: &str,
) -> ConflictException {
    ConflictException {
        message: format!(
            "clientToken '{client_token}' was already used to write deployment parameter \
             {bound_parameter_id}; a token identifies exactly one write"
        ),
        resource_id: bound_parameter_id.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState>)
}

/// Record the agreements this account holds, so `PutDeploymentParameter` can be
/// written against them.
///
/// This is the out-of-band creation seam: an AWS Marketplace agreement exists
/// when a buyer accepts a seller's offer and a product exists when a seller
/// publishes it, and THIS API models no operation that does either — it only
/// writes deployment parameters against an agreement that is already there — so
/// a fixture (or an admin path) hands the records to the emulator here instead.
/// A record for an agreement already stored REPLACES it, so seeding the same
/// agreement twice holds one agreement, not two.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so
/// there is no store to seed), otherwise the number of agreements the store holds
/// afterwards.
pub fn seed_agreements<Eng, I>(engine: &Eng, records: I) -> Option<usize>
where
    Eng: Send + Sync + 'static,
    I: IntoIterator<Item = ProvisionedAgreement>,
{
    let state = service_state(engine)?;
    let mut store = state.lock();
    for record in records {
        store.agreements.insert(record.key(), record);
    }
    Some(store.agreements.len())
}
