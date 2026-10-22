// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcases-amazonconnectcases service.
//!
//! One `Domain` is the container for everything Cases persists: its fields,
//! templates and cases all hang off the domain, which is how the real service
//! scopes them (every operation but `CreateDomain`/`ListDomains` carries a
//! `domainId` path label). Handlers reach this store through [`service_state`]
//! and never build their own engine.
//!
//! # Bounding request-driven growth
//!
//! Every map here grows one entry per request, with caller-chosen contents, so
//! each one carries an explicit ceiling: [`MAX_DOMAINS`],
//! [`MAX_FIELDS_PER_DOMAIN`], [`MAX_TEMPLATES_PER_DOMAIN`],
//! [`MAX_CASES_PER_DOMAIN`] and [`MAX_IDEMPOTENCY_TOKENS`]. Creating past a
//! ceiling is refused with a modelled error, and the idempotency table evicts
//! its oldest entry rather than growing, so no caller can grow this store
//! without end.
//!
//! The per-request work is bounded the same way, and does not rest on the
//! boundary validator alone: [`MAX_CASE_FIELD_IDS`],
//! [`MAX_BATCH_FIELD_IDS`] and [`MAX_EVENT_INCLUDED_FIELD_IDS`] are re-checked
//! in the handler ([`list_too_long`]),
//! the scans below are linear in the list length, and a refusal names at most
//! [`MAX_IDS_NAMED_IN_A_REFUSAL`] ids. A list that carries the same id twice is
//! refused on the WRITE paths only ([`duplicate_ids`]), where two values for one
//! id is ambiguous; a read answers one entry per occurrence, since the model
//! declares no distinctness constraint and the caps already bound the response.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    EventBridgeConfiguration, FieldAttributes, FieldValueUnion, InternalServerException,
    LayoutConfiguration, RequiredField, ResourceNotFoundException, ServiceQuotaExceededException,
    TagPropagationConfiguration, UserUnion,
};

/// The ARN namespace (and SigV4 signing name / endpoint prefix) of this service,
/// as `OPERATIONS.md` records it from the model: `cases`.
pub const ARN_SERVICE: &str = "cases";

/// Region used in an ARN when the request carries no SigV4 credential scope to
/// read one from. The emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `DomainStatus` value a domain that exists and serves requests reports.
/// One of `DOMAIN_STATUS_VALUES` (`Active | CreationFailed | CreationInProgress`).
pub const DOMAIN_STATUS_ACTIVE: &str = "Active";

/// `TemplateStatus` a template takes when `CreateTemplate` is given no `status`.
/// One of `TEMPLATE_STATUS_VALUES` (`Active | Inactive`).
pub const TEMPLATE_STATUS_ACTIVE: &str = "Active";

/// `FieldNamespace` of a field a customer created through `CreateField`; the
/// service's own built-in fields are the other value, `System`.
/// One of `FIELD_NAMESPACE_VALUES` (`Custom | System`).
pub const FIELD_NAMESPACE_CUSTOM: &str = "Custom";

/// Most domains this emulator's store holds. EMULATOR-OWNED ceiling on
/// request-driven growth, not an AWS-published quota: the real service's limit
/// is an account quota a support request can raise, while this one exists so a
/// caller cannot grow the in-memory store without end.
pub const MAX_DOMAINS: usize = 100;

/// Most fields one domain holds. Emulator-owned ceiling, as [`MAX_DOMAINS`].
/// It also bounds a case's stored field values, since only a field the domain
/// defines can be written.
pub const MAX_FIELDS_PER_DOMAIN: usize = 500;

/// Most templates one domain holds. Emulator-owned ceiling, as [`MAX_DOMAINS`].
pub const MAX_TEMPLATES_PER_DOMAIN: usize = 500;

/// Most cases one domain holds. Emulator-owned ceiling, as [`MAX_DOMAINS`].
pub const MAX_CASES_PER_DOMAIN: usize = 10_000;

/// Most `CreateCase` idempotency tokens the store remembers. Emulator-owned
/// ceiling, as [`MAX_DOMAINS`]: past it the oldest remembered token is
/// forgotten, which is how a real idempotency window behaves — a replay after
/// the window mints a new resource rather than being refused.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// Most field ids one case request may carry. This is the model's own cap,
/// declared as a member-level `length` trait rather than on the list shape:
/// `GetCaseRequest$fields` is `1..220` and `CreateCaseRequest$fields` /
/// `UpdateCaseRequest$fields` are `..220`
/// (`smithy_query(query="shape", shape="GetCaseRequest,CreateCaseRequest,UpdateCaseRequest")`).
/// It is restated here and re-checked inside the handler by [`list_too_long`],
/// so the bound on caller-chosen per-request work does not rest on the boundary
/// validator alone.
pub const MAX_CASE_FIELD_IDS: usize = 220;

/// Most field ids one `BatchGetField` request may carry: the model's
/// `BatchGetFieldRequest$fields` cap of `1..50`, re-checked in the handler for
/// the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_BATCH_FIELD_IDS: usize = 50;

/// Most required fields one template may declare: the model's
/// `CreateTemplateRequest$requiredFields` cap of `..100`, re-checked in the
/// handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_REQUIRED_FIELDS: usize = 100;

/// Most field identifiers one domain's case-event publishing configuration may
/// include: the model's `CaseEventIncludedData$fields` cap of `..400`
/// (`smithy_query(query="shape", shape="CaseEventIncludedData")`), re-checked in
/// the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_EVENT_INCLUDED_FIELD_IDS: usize = 400;

/// Most ids a refusal message names, so a refusal over a long list cannot be
/// amplified into a long response.
pub const MAX_IDS_NAMED_IN_A_REFUSAL: usize = 10;

/// A Cases domain and everything scoped to it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub domain_arn: String,
    pub name: String,
    pub created_time: SystemTime,
    pub domain_status: String,
    pub tags: HashMap<String, String>,
    pub fields: HashMap<String, Field>,
    pub templates: HashMap<String, Template>,
    pub cases: HashMap<String, Case>,
    pub event_bridge: Option<EventBridgeConfiguration>,
}

/// A case field definition: the schema a case's field values are written against.
#[derive(Debug, Clone)]
pub struct Field {
    pub field_id: String,
    pub field_arn: String,
    pub name: String,
    /// One of `FIELD_TYPE_VALUES`.
    pub field_type: String,
    /// One of `FIELD_NAMESPACE_VALUES`.
    pub namespace: String,
    pub description: Option<String>,
    pub attributes: Option<FieldAttributes>,
    pub deleted: bool,
    /// Reserved for the tagging slice: `CreateField` takes no tags and
    /// `TagResource`/`UntagResource` are still 501s, so nothing can populate
    /// this yet and a read reports no `tags` member for a field.
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A case template.
#[derive(Debug, Clone)]
pub struct Template {
    pub template_id: String,
    pub template_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub layout_configuration: Option<LayoutConfiguration>,
    pub required_fields: Vec<RequiredField>,
    pub tag_propagation_configurations: Vec<TagPropagationConfiguration>,
    /// One of `TEMPLATE_STATUS_VALUES`.
    pub status: String,
    pub deleted: bool,
    /// Reserved for the tagging slice, as [`Field::tags`].
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A case: the field values written against a template, keyed by field id.
#[derive(Debug, Clone)]
pub struct Case {
    pub case_id: String,
    pub case_arn: String,
    pub domain_id: String,
    pub template_id: String,
    /// The union variant each value was written with is preserved exactly, so a
    /// read hands back what was written rather than a re-encoding of it. Keyed
    /// by field id, so it is bounded by [`MAX_FIELDS_PER_DOMAIN`].
    pub field_values: HashMap<String, FieldValueUnion>,
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub performed_by: Option<UserUnion>,
}

/// A remembered `CreateCase` idempotency token: the domain it was used in and
/// the token itself. The domain is part of the key because the same token used
/// against a different domain is a different request.
type IdempotencyKey = (String, String);

/// Per-service state for com-amazonaws-connectcases-amazonconnectcases. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcasesAmazonconnectcasesState {
    /// Every domain, keyed by `domainId`. Bounded by [`MAX_DOMAINS`].
    pub domains: HashMap<String, Domain>,
    /// `(domainId, CreateCase clientToken)` -> the `caseId` that token already
    /// minted, so a replay resolves to the same case instead of making another.
    /// Reached through [`Self::idempotent_case`] / [`Self::remember_idempotent_case`]
    /// so the eviction order below cannot drift from it.
    case_idempotency: HashMap<IdempotencyKey, String>,
    /// Insertion order of `case_idempotency`, so the OLDEST remembered token is
    /// the one dropped at [`MAX_IDEMPOTENCY_TOKENS`].
    idempotency_order: VecDeque<IdempotencyKey>,
}

impl ComAmazonawsConnectcasesAmazonconnectcasesState {
    /// The domain `domain_id` names, or the modelled 404 for it.
    pub fn domain(&self, domain_id: &str) -> Result<&Domain, ResourceNotFoundException> {
        self.domains
            .get(domain_id)
            .ok_or_else(|| not_found("Domain", domain_id))
    }

    /// The domain `domain_id` names for writing, or the modelled 404 for it.
    pub fn domain_mut(&mut self, domain_id: &str) -> Result<&mut Domain, ResourceNotFoundException> {
        self.domains
            .get_mut(domain_id)
            .ok_or_else(|| not_found("Domain", domain_id))
    }

    /// Refuse a new domain once the store holds [`MAX_DOMAINS`] of them.
    pub fn domain_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(self.domains.len(), MAX_DOMAINS, "Cases domains in this store")
    }

    /// The case the `client_token` already minted in `domain_id`, if the store
    /// still remembers that token AND the case it names still exists.
    pub fn idempotent_case(&self, domain_id: &str, client_token: &str) -> Option<&Case> {
        let case_id = self
            .case_idempotency
            .get(&(domain_id.to_string(), client_token.to_string()))?;
        self.domains.get(domain_id)?.cases.get(case_id)
    }

    /// Remember that `client_token` minted `case_id` in `domain_id`, evicting
    /// the oldest remembered token once [`MAX_IDEMPOTENCY_TOKENS`] is reached.
    pub fn remember_idempotent_case(
        &mut self,
        domain_id: &str,
        client_token: &str,
        case_id: &str,
    ) {
        let key = (domain_id.to_string(), client_token.to_string());
        if self
            .case_idempotency
            .insert(key.clone(), case_id.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.idempotency_order.pop_front() {
                self.case_idempotency.remove(&evicted);
            }
        }
    }

    /// Forget every token that resolved to `case_id` in `domain_id`, so a
    /// replay cannot answer with the id of a case that no longer exists.
    pub fn forget_idempotent_case(&mut self, domain_id: &str, case_id: &str) {
        let dropped: Vec<IdempotencyKey> = self
            .case_idempotency
            .iter()
            .filter(|((domain, _), stored)| domain == domain_id && stored.as_str() == case_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in dropped {
            self.case_idempotency.remove(&key);
            self.idempotency_order.retain(|existing| existing != &key);
        }
    }
}

impl Domain {
    /// The case `case_id` names, or the modelled 404 for it.
    pub fn case(&self, case_id: &str) -> Result<&Case, ResourceNotFoundException> {
        self.cases
            .get(case_id)
            .ok_or_else(|| not_found("Case", case_id))
    }

    /// The template `template_id` names, or the modelled 404 for it. A template
    /// that has been deleted is gone as far as a reader is concerned.
    pub fn template(&self, template_id: &str) -> Result<&Template, ResourceNotFoundException> {
        self.templates
            .get(template_id)
            .filter(|template| !template.deleted)
            .ok_or_else(|| not_found("Template", template_id))
    }

    /// Refuse a new field once the domain holds [`MAX_FIELDS_PER_DOMAIN`].
    pub fn field_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.fields.len(),
            MAX_FIELDS_PER_DOMAIN,
            "fields in this domain",
        )
    }

    /// Refuse a new template once the domain holds [`MAX_TEMPLATES_PER_DOMAIN`].
    pub fn template_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.templates.len(),
            MAX_TEMPLATES_PER_DOMAIN,
            "templates in this domain",
        )
    }

    /// Refuse a new case once the domain holds [`MAX_CASES_PER_DOMAIN`].
    pub fn case_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.cases.len(),
            MAX_CASES_PER_DOMAIN,
            "cases in this domain",
        )
    }

    /// The ids among `ids` that this domain holds no live field for, in the
    /// order they were given and without repeats. Empty means every reference
    /// resolves. Linear in the number of ids: one hash lookup each.
    pub fn unknown_field_ids<'a, I: IntoIterator<Item = &'a str>>(&self, ids: I) -> Vec<String> {
        let mut reported: HashSet<&str> = HashSet::new();
        let mut unknown: Vec<String> = Vec::new();
        for id in ids {
            let known = self.fields.get(id).is_some_and(|field| !field.deleted);
            if !known && reported.insert(id) {
                unknown.push(id.to_string());
            }
        }
        unknown
    }

    /// The ids among `values` whose union variant is not a value of the
    /// declared `FieldType` of the field it is written against, each reported
    /// once in first-seen order with the type and the variant named. An id the
    /// domain holds no live field for is left to [`Self::unknown_field_ids`].
    /// Linear in the number of values.
    pub fn mismatched_field_values<'a, I>(&self, values: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, &'a FieldValueUnion)>,
    {
        let mut reported: HashSet<&str> = HashSet::new();
        let mut mismatched: Vec<String> = Vec::new();
        for (id, value) in values {
            let Some(field) = self.fields.get(id).filter(|field| !field.deleted) else {
                continue;
            };
            if !value_fits_field_type(&field.field_type, value) && reported.insert(id) {
                mismatched.push(format!(
                    "{id} is a {} field and cannot hold {}",
                    field.field_type,
                    variant_name(value)
                ));
            }
        }
        mismatched
    }
}

/// `Ok` while `held` is below `limit`, else the modelled quota error naming
/// what filled up.
fn capacity(
    held: usize,
    limit: usize,
    what: &str,
) -> Result<(), ServiceQuotaExceededException> {
    if held < limit {
        Ok(())
    } else {
        Err(ServiceQuotaExceededException {
            message: format!(
                "the emulator holds at most {limit} {what}; delete one before creating another"
            ),
        })
    }
}

/// The `FieldValueUnion` variant name, for a refusal that has to say which
/// variant it saw.
pub fn variant_name(value: &FieldValueUnion) -> &'static str {
    match value {
        FieldValueUnion::stringValue(_) => "stringValue",
        FieldValueUnion::doubleValue(_) => "doubleValue",
        FieldValueUnion::booleanValue(_) => "booleanValue",
        FieldValueUnion::emptyValue(_) => "emptyValue",
        FieldValueUnion::userArnValue(_) => "userArnValue",
    }
}

/// Whether `value`'s union variant is a value OF `field_type`.
///
/// A case field's value has the field's own declared type, so the variant and
/// the `FieldType` have to agree: a `Boolean` field holds `booleanValue`, a
/// `Number` field `doubleValue`, a `User` field `userArnValue`, and the
/// string-valued types (`DateTime`, `SingleSelect`, `Text`, `Url`) hold
/// `stringValue`. `emptyValue` is the absent value and clears a field of any
/// type. `field_type` is one of `FIELD_TYPE_VALUES`; an unrecognised one
/// matches nothing, because a value cannot be checked against a type this code
/// does not know.
pub fn value_fits_field_type(field_type: &str, value: &FieldValueUnion) -> bool {
    match value {
        FieldValueUnion::emptyValue(_) => true,
        FieldValueUnion::booleanValue(_) => field_type == "Boolean",
        FieldValueUnion::doubleValue(_) => field_type == "Number",
        FieldValueUnion::userArnValue(_) => field_type == "User",
        FieldValueUnion::stringValue(_) => {
            matches!(field_type, "DateTime" | "SingleSelect" | "Text" | "Url")
        }
    }
}

/// The ids that appear more than once in `ids`, each reported once in first-seen
/// order. Empty means every id is distinct. Linear in the number of ids.
///
/// Used by the WRITE paths (`CreateCase`, `UpdateCase`, `CreateTemplate`), where
/// one id carrying two values — or one template naming the same required field
/// twice — has no defined meaning, so last-one-wins would be an arbitrary
/// choice. The reads do not use it: the model declares no distinctness
/// constraint and they answer one entry per occurrence.
pub fn duplicate_ids<'a, I: IntoIterator<Item = &'a str>>(ids: I) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut reported: HashSet<&str> = HashSet::new();
    let mut duplicates: Vec<String> = Vec::new();
    for id in ids {
        if !seen.insert(id) && reported.insert(id) {
            duplicates.push(id.to_string());
        }
    }
    duplicates
}

/// The ids among `required` that `provided` does not name, each reported once in
/// first-seen order. Empty means every required id has an entry.
///
/// A template's `requiredFields` is, per the model, the "List of fields that
/// must have a value provided to create a case", so what this checks is exactly
/// that: an id is PROVIDED when the request's `fields` carries an entry for it,
/// whatever union variant that entry holds — `emptyValue` is a value of the
/// union ("An empty value"), so it counts as provided and the rule is not
/// widened past what the model states. Linear in both lists.
pub fn ids_not_provided<'a, I: IntoIterator<Item = &'a str>>(
    required: &[String],
    provided: I,
) -> Vec<String> {
    let provided: HashSet<&str> = provided.into_iter().collect();
    let mut reported: HashSet<&str> = HashSet::new();
    let mut missing: Vec<String> = Vec::new();
    for id in required {
        if !provided.contains(id.as_str()) && reported.insert(id.as_str()) {
            missing.push(id.clone());
        }
    }
    missing
}

/// `Some(message)` when a request's `member` list is longer than `cap`: the
/// refusal text naming the member, the length seen and the ceiling.
///
/// Checked in the handler even though the model declares the same cap, so the
/// per-request work (and the response built from it) is bounded by this code
/// rather than only by the boundary validator.
pub fn list_too_long(member: &str, len: usize, cap: usize) -> Option<String> {
    (len > cap).then(|| {
        format!("{member} carries {len} entries; at most {cap} are accepted in one request")
    })
}

/// `ids` rendered for a refusal message, naming at most
/// [`MAX_IDS_NAMED_IN_A_REFUSAL`] of them and counting the rest.
pub fn name_ids(ids: &[String]) -> String {
    if ids.len() <= MAX_IDS_NAMED_IN_A_REFUSAL {
        ids.join(", ")
    } else {
        format!(
            "{}, and {} more",
            ids[..MAX_IDS_NAMED_IN_A_REFUSAL].join(", "),
            ids.len() - MAX_IDS_NAMED_IN_A_REFUSAL
        )
    }
}

/// A fresh opaque resource id, in the textual form the real service's ids take
/// (a hyphenated, version-4 UUID): SHA-256 over this process's OS-seeded
/// entropy, a monotonic mint counter and the mint instant.
///
/// Nothing in it comes from the request — two ids minted for byte-identical
/// input differ, because the counter does — and nothing about it is guessable
/// from another id, because the per-process entropy behind it never leaves this
/// process. The counter is what makes the ids UNIQUE; the entropy is what makes
/// them UNPREDICTABLE.
///
/// The digest and the hex codec come from `sha2`/`hex`, as the sibling service
/// crates mint their opaque ids, and the entropy from `std`'s OS-seeded
/// [`RandomState`]; this crate implements neither a hash nor a random generator
/// of its own.
pub fn new_id() -> String {
    /// Mints so far in this process. `Relaxed` is enough: the value only has to
    /// be distinct per mint, not ordered against anything else.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha256::new();
    hasher.update(process_entropy());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 version (4) and variant bits, so the id has the same textual
    // shape as the ids the service hands out.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// This process's 128 bits of id-minting entropy, taken ONCE from `std`'s
/// OS-seeded [`RandomState`] — the same keys a `HashMap` is randomised with —
/// and never recomputed, so every id this process mints is unpredictable to
/// anyone who has only seen the others.
fn process_entropy() -> &'static [u8; 16] {
    static ENTROPY: OnceLock<[u8; 16]> = OnceLock::new();
    ENTROPY.get_or_init(|| {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&RandomState::new().hash_one(0_u8).to_be_bytes());
        bytes[8..].copy_from_slice(&RandomState::new().hash_one(u64::MAX).to_be_bytes());
        bytes
    })
}

/// The ARN of a domain.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("domain/{domain_id}"))
}

/// The ARN of a field of a domain.
pub fn field_arn(region: &str, domain_id: &str, field_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/field/{field_id}"),
    )
}

/// The ARN of a template of a domain.
pub fn template_arn(region: &str, domain_id: &str, template_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/template/{template_id}"),
    )
}

/// The ARN of a case of a domain.
pub fn case_arn(region: &str, domain_id: &str, case_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/case/{case_id}"),
    )
}

/// The region to build a new resource's ARN in: the one the caller signed for,
/// else [`DEFAULT_REGION`]. An ARN is minted once and stored, so a later read in
/// another region still reports the ARN the resource was created with.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled 404 for a resource this service does not hold.
pub fn not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{resource_type} {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled 500 for a request that reached a handler with no store behind
/// it. Answered rather than papered over with a private engine: a handler that
/// builds its own store writes somewhere nothing else can read.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the ConnectCases state store is unavailable".to_string(),
        retry_after_seconds: None,
    }
}

/// Render a stored instant the way the model's `date-time` timestamps expect.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The case-event publishing configuration a domain reports when no
/// `PutCaseEventConfiguration` has been made against it: delivery disabled, with
/// no included data.
///
/// The configuration is a PROPERTY of the domain, not a resource of its own:
/// `PutCaseEventConfiguration` "Adds case event publishing configuration" to a
/// domain and `GetCaseEventConfiguration` "Returns the case event publishing
/// configuration" with `eventBridge` as a REQUIRED output member
/// (`smithy_query(query="operation", operation="GetCaseEventConfiguration")`), so
/// an unconfigured domain reports event delivery off rather than a missing
/// resource — the 404 of either operation is for a domain this store does not
/// hold.
pub fn event_bridge_disabled() -> EventBridgeConfiguration {
    EventBridgeConfiguration {
        enabled: false,
        included_data: None,
    }
}

/// How many field identifiers `configuration` asks the case event stream to
/// include: the length of its `includedData.caseData.fields` list, or zero when
/// it names no case data at all. Linear in nothing — it reads one length.
pub fn included_field_count(configuration: &EventBridgeConfiguration) -> usize {
    configuration
        .included_data
        .as_ref()
        .and_then(|included| included.case_data.as_ref())
        .map_or(0, |case_data| case_data.fields.len())
}

/// A tag map for a response member: absent when the resource carries no tags,
/// which is how the service reports one that was never tagged.
pub fn tags_output(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(tags.clone())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcasesAmazonconnectcasesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcasesAmazonconnectcasesState>)
}
