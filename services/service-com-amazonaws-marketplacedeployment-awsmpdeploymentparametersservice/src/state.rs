// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice service.
//!
//! The deployment-parameter lifecycle is backed here: every value
//! `PutDeploymentParameter`, `TagResource`, `UntagResource` and
//! `ListTagsForResource` report is read out of one of these records, never built
//! from the request that asked for it.

use std::collections::{BTreeMap, HashMap, VecDeque};
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

/// The characters a tag key or value may hold, as the modelled `TagKey` /
/// `TagValue` pattern `^[a-zA-Z0-9/_+=.:@-]+$` spells them.
pub const TAG_CHARACTER_CLASS: &str = "^[a-zA-Z0-9/_+=.:@-]+$";

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

/// The modelled `ResourceNotFoundException` (HTTP 404) for an ARN that names no
/// stored deployment parameter. Every member the model marks `@required` is filled.
pub fn parameter_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("no deployment parameter resource is identified by '{resource_arn}'"),
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
