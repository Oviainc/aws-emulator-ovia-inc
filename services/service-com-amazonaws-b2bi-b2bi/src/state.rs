// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-b2bi-b2bi service.
//!
//! The service's primary resources — Profile, Capability, Partnership and Transformer —
//! are persisted here, under the single shared engine, so a create is observable by every
//! later Get/List/Delete. Each collection keeps BOTH a `HashMap` keyed by the resource's
//! generated id (the lookup every Get/Delete does) and a `Vec` of ids in insertion
//! order (the stable enumeration the `@paginated` List operations page through).
//!
//! It also owns the wording of the `ResourceNotFoundException` each resource type is
//! reported absent with, so every handler that resolves an id states it the same way.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ops::types::{
    CapabilityConfiguration, CapabilityOptions, EdiType, InputConversion, Mapping,
    OutputConversion, S3Location, SampleDocuments, Tag,
};

/// The region an emulated ARN carries when the request's SigV4 scope does not name one
/// (the emulator's documented default — see `engine::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Wording for the `InternalServerException` a handler answers when the engine it was
/// constructed from is not the shared `InMemoryEngine` (so no store is reachable).
pub const STATE_UNAVAILABLE: &str = "the b2bi service store is unavailable";

/// The value of the modelled `Logging` enum that turns a profile's logging on.
pub const LOGGING_ENABLED: &str = "ENABLED";

/// This service's own code, as the modelled `ServiceQuotaExceededException$serviceCode`
/// reports it and as Service Quotas publishes this service's quotas under
/// (`smithy_query(query="service_info")` -> `sdkId: b2bi`).
pub const SERVICE_CODE: &str = "b2bi";

/// The quota the profile count is charged against, as
/// `ServiceQuotaExceededException$quotaCode` names it and as the Service Quotas
/// emulator publishes it for [`SERVICE_CODE`].
pub const PROFILE_QUOTA_CODE: &str = "ProfilesPerAccount";

/// AWS's DOCUMENTED profile limit, from `CreateProfile`: "You can have up to five
/// customer profiles, each representing a distinct private network."
///
/// A create past it is the modelled `ServiceQuotaExceededException` (HTTP 402), and
/// deleting a profile releases a slot — so the limit is a live count of stored
/// profiles, never a high-water mark. AWS publishes this limit in Service Quotas
/// (ServiceCode `b2bi`) as adjustable; this emulator enforces the documented default,
/// and does not apply an approved increase to it.
pub const MAX_PROFILES_PER_ACCOUNT: usize = 5;

/// The `TransformerStatus` a freshly created transformer carries.
///
/// `inactive`: the model's own doc for `CreateTransformerResponse$status` says "Returns
/// the state of the newly created transformer. The transformer can be either active or
/// inactive. For the transformer to be used in a capability, its status must active" —
/// i.e. a new transformer is not usable until a caller moves it, so creation cannot be
/// what makes it `active`. The move is an explicit `UpdateTransformer`, not a transition
/// the clock owes: `inactive` is a resting state, not a transient one.
pub const TRANSFORMER_STATUS_INACTIVE: &str = "inactive";

/// How many hex digits follow the resource-type prefix in a generated id.
///
/// 17, matching the `profileId` / `capabilityId` / `partnershipId` / `tradingPartnerId`
/// values in this service's own Smithy model examples (`p-<17 hex>`), and well inside
/// the modelled `len 1..64` / `^[a-zA-Z0-9_-]+$` constraint on every one of those ids.
const ID_HEX_DIGITS: usize = 17;

/// The page size a List operation uses when the caller names no `maxResults`: the top of
/// the modelled `MaxResults` range (`range 1..100`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The most records a store this service keeps no AWS-published limit for holds — the
/// capability and partnership stores. The profile store's ceiling is AWS's own
/// [`MAX_PROFILES_PER_ACCOUNT`], which is far below this one.
///
/// Bounds the memory one emulator process can be made to hold through repeated creates:
/// a create is reachable without a verified caller, so without a ceiling every call
/// would add a resident record for good. Each create refuses past this bound with the
/// modelled `ServiceQuotaExceededException` rather than growing further, which also
/// bounds the per-request work the List operations do under the state lock (their filter
/// and token-resume scans walk at most this many ids).
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The most `@idempotencyToken` replays remembered at once.
///
/// Each distinct caller-chosen `clientToken` adds one entry, so the map is capped and
/// the oldest entry is evicted first
/// ([`ComAmazonawsB2biB2biState::record_idempotency`]); deleting a resource also drops
/// the entries that pointed at it
/// ([`ComAmazonawsB2biB2biState::forget_idempotency_for`]), so the map has a release
/// path as well as a ceiling.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

/// The most tags one resource holds at once.
///
/// 200, the maximum of the modelled `TagList` (`len 0..200`) — the very shape
/// `ListTagsForResourceResponse$Tags` reports a resource's tags through, so a resource
/// holding more than this could not be read back inside its own modelled bound. It is
/// also what bounds tag residency: `TagResource` is reachable without a verified caller,
/// so without a ceiling repeated calls with fresh keys would grow one resource's record
/// for good. A call that would carry a resource past it is refused with the modelled
/// `ValidationException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A customer profile: the private network a partnership is created under.
#[derive(Debug, Clone)]
pub struct Profile {
    pub profile_id: String,
    pub profile_arn: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: String,
    pub business_name: String,
    pub logging: String,
    pub log_group_name: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
}

/// A capability: the EDI processing a partnership references by id.
#[derive(Debug, Clone)]
pub struct Capability {
    pub capability_id: String,
    pub capability_arn: String,
    pub name: String,
    pub r#type: String,
    pub configuration: CapabilityConfiguration,
    /// `Option`, not `Vec`: the modelled member is optional, so an absent
    /// `instructionsDocuments` must read back absent rather than as an empty list.
    pub instructions_documents: Option<Vec<S3Location>>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
}

/// A transformer: the EDI <-> JSON/XML translation a capability references by id.
///
/// Every member is persisted as the create supplied it — including the four members the
/// model marks `@deprecated` (`fileFormat`, `mappingTemplate`, `ediType`,
/// `sampleDocument`) — so a later read cannot disagree with what was created.
#[derive(Debug, Clone)]
pub struct Transformer {
    pub transformer_id: String,
    pub transformer_arn: String,
    pub name: String,
    pub status: String,
    pub file_format: Option<String>,
    pub mapping_template: Option<String>,
    pub edi_type: Option<EdiType>,
    pub sample_document: Option<String>,
    pub input_conversion: Option<InputConversion>,
    pub mapping: Option<Mapping>,
    pub output_conversion: Option<OutputConversion>,
    pub sample_documents: Option<SampleDocuments>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
}

/// A partnership: one trading partner, under a profile, bound to a set of capabilities.
#[derive(Debug, Clone)]
pub struct Partnership {
    pub partnership_id: String,
    pub partnership_arn: String,
    pub profile_id: String,
    pub name: String,
    pub email: String,
    pub phone: Option<String>,
    pub capabilities: Vec<String>,
    pub capability_options: Option<CapabilityOptions>,
    pub trading_partner_id: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
}

/// Per-service state for com-amazonaws-b2bi-b2bi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsB2biB2biState {
    pub profiles: HashMap<String, Profile>,
    /// `profiles` in creation order, so `ListProfiles` paging is stable across calls.
    pub profile_order: Vec<String>,
    /// `profileId`s minted but not yet stored.
    ///
    /// `CreateProfile` provisions a logging-ENABLED profile's log group with no lock
    /// held, so it reserves its id here first: while the reservation stands no
    /// concurrent create can mint the same id, and the reservation counts against
    /// [`MAX_PROFILES_PER_ACCOUNT`] so it cannot take the account's last slot either.
    /// An entry lives for one in-flight create and is removed whichever way it ends.
    pub reserved_profile_ids: HashSet<String>,
    pub capabilities: HashMap<String, Capability>,
    /// `capabilities` in creation order, for `ListCapabilities` paging.
    pub capability_order: Vec<String>,
    pub partnerships: HashMap<String, Partnership>,
    /// `partnerships` in creation order, for `ListPartnerships` paging.
    pub partnership_order: Vec<String>,
    /// The transformers this service holds, keyed by `transformerId`: the set an `edi`
    /// capability's `transformerId` is resolved against. See [`Transformer`].
    pub transformers: HashMap<String, Transformer>,
    /// `transformers` in creation order, for `ListTransformers` paging.
    pub transformer_order: Vec<String>,
    /// `@idempotencyToken` replay: [`idempotency_key`] -> the id of the resource the
    /// first call with that token created. Bounded by [`MAX_IDEMPOTENCY_ENTRIES`].
    pub idempotency: HashMap<String, String>,
    /// The order [`idempotency`](Self::idempotency)'s keys were recorded in, so the
    /// oldest is the one evicted once the map is full.
    pub idempotency_order: VecDeque<String>,
}

impl ComAmazonawsB2biB2biState {
    /// Whether another profile fits inside AWS's documented
    /// [`MAX_PROFILES_PER_ACCOUNT`] limit, counting the ids in-flight creates have
    /// reserved ([`reserved_profile_ids`](Self::reserved_profile_ids)). That limit is far
    /// below [`MAX_RECORDS_PER_STORE`], so it is also what bounds this store's residency.
    pub fn accepts_another_profile(&self) -> bool {
        self.profiles.len() + self.reserved_profile_ids.len() < MAX_PROFILES_PER_ACCOUNT
    }

    /// Whether the capability store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_capability(&self) -> bool {
        self.capabilities.len() < MAX_RECORDS_PER_STORE
    }

    /// Whether the partnership store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_partnership(&self) -> bool {
        self.partnerships.len() < MAX_RECORDS_PER_STORE
    }

    /// Whether the transformer store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_transformer(&self) -> bool {
        self.transformers.len() < MAX_RECORDS_PER_STORE
    }

    /// Remember that `key` minted `id`, evicting the oldest remembered token once
    /// [`MAX_IDEMPOTENCY_ENTRIES`] are held — so a caller cannot grow this map without
    /// limit by sending a fresh `clientToken` on every request.
    pub fn record_idempotency(&mut self, key: String, id: &str) {
        if self.idempotency.insert(key.clone(), id.to_string()).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// The tags stored on the resource `arn` names, or `None` when this service holds
    /// no resource with that ARN.
    ///
    /// Every taggable resource this service persists — profile, capability,
    /// partnership, transformer — keeps the tags it was created with on its own record,
    /// so the tag
    /// list is looked up by the very ARN the create stored and handed back. An ARN that
    /// differs from a stored one in any field (its region or its account, say) names no
    /// resource here, which is what AWS reports as `ResourceNotFoundException`: "the
    /// resource exists in a region other than the region specified in the API call".
    ///
    /// The scan is linear over at most four times [`MAX_RECORDS_PER_STORE`] records,
    /// the same bound the List operations' scans carry.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&Vec<Tag>> {
        self.profiles
            .values()
            .find(|profile| profile.profile_arn == arn)
            .map(|profile| &profile.tags)
            .or_else(|| {
                self.capabilities
                    .values()
                    .find(|capability| capability.capability_arn == arn)
                    .map(|capability| &capability.tags)
            })
            .or_else(|| {
                self.partnerships
                    .values()
                    .find(|partnership| partnership.partnership_arn == arn)
                    .map(|partnership| &partnership.tags)
            })
            .or_else(|| {
                self.transformers
                    .values()
                    .find(|transformer| transformer.transformer_arn == arn)
                    .map(|transformer| &transformer.tags)
            })
    }

    /// The tags stored on the resource `arn` names, for a handler that CHANGES them
    /// (`TagResource`), or `None` when this service holds no resource with that ARN.
    ///
    /// The same resolution as [`tags_by_arn`](Self::tags_by_arn) — the tag list lives on
    /// the resource's own record, found by the very ARN its create stored — so a tag
    /// written here is the one the next `ListTagsForResource` reports, and an ARN that
    /// differs from a stored one in any field names no resource here.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(profile) = self
            .profiles
            .values_mut()
            .find(|profile| profile.profile_arn == arn)
        {
            return Some(&mut profile.tags);
        }
        if let Some(capability) = self
            .capabilities
            .values_mut()
            .find(|capability| capability.capability_arn == arn)
        {
            return Some(&mut capability.tags);
        }
        if let Some(partnership) = self
            .partnerships
            .values_mut()
            .find(|partnership| partnership.partnership_arn == arn)
        {
            return Some(&mut partnership.tags);
        }
        if let Some(transformer) = self
            .transformers
            .values_mut()
            .find(|transformer| transformer.transformer_arn == arn)
        {
            return Some(&mut transformer.tags);
        }
        None
    }

    /// Drop every remembered token that minted `id`, called when `id`'s resource is
    /// deleted: a replay of that token mints a fresh resource anyway, so keeping the
    /// entry would hold memory for a resource that no longer exists.
    pub fn forget_idempotency_for(&mut self, id: &str) {
        self.idempotency.retain(|_, minted| minted != id);
        let remembered = &self.idempotency;
        self.idempotency_order
            .retain(|key| remembered.contains_key(key));
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsB2biB2biState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsB2biB2biState>)
}

/// The `ResourceNotFoundException` message for a `profileId` the store does not hold.
///
/// Every one of these four is built from the request's own identifier, in the form this
/// service states for that resource type (the forms recorded from AWS): a client parses
/// the id back out of the message, so the template is part of the contract and must not
/// be an emulator-local phrase.
pub fn profile_not_found(profile_id: &str) -> String {
    format!("Profile ID {profile_id} not found.")
}

/// The `ResourceNotFoundException` message for an unknown `capabilityId`.
pub fn capability_not_found(capability_id: &str) -> String {
    format!("Capability ID {capability_id} not found.")
}

/// The `ResourceNotFoundException` message for an unknown `partnershipId`.
pub fn partnership_not_found(partnership_id: &str) -> String {
    format!("Partnership with partnershipId {partnership_id} not found.")
}

/// The `ResourceNotFoundException` message for an unknown `transformerId`.
///
/// The recorded form for a transformer carries no trailing period, unlike the three
/// above; each template is the one recorded for that resource type.
pub fn transformer_not_found(transformer_id: &str) -> String {
    format!("Transformer with transformerId {transformer_id} not found")
}

/// The CloudWatch Logs log group a logging-ENABLED profile reports as `logGroupName`,
/// in the `b2bi/<profileId>-Logs` form this service's own model example carries.
///
/// The group with this name is PROVISIONED in CloudWatch Logs when the profile is
/// created (`CreateProfile`), so the name a caller reads back resolves there.
pub fn log_group_name(profile_id: &str) -> String {
    format!("b2bi/{profile_id}-Logs")
}

/// The key a `clientToken` is remembered under, scoped by resource type.
///
/// Scoped because the three creates each carry their own `@idempotencyToken` and a
/// caller may reuse one token value across them; an unscoped map would hand
/// `CreatePartnership` the profile that `CreateProfile` created under the same token.
pub fn idempotency_key(resource_type: &str, client_token: &str) -> String {
    format!("{resource_type}#{client_token}")
}

/// Whether `candidate` satisfies the AWS ARN grammar
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`: six colon-delimited
/// fields, the first literally `arn`, with a non-empty partition, service and
/// resource. `region` and `account-id` are the two fields AWS itself leaves empty on
/// the ARNs of global services, so neither is required to be filled in; the resource
/// field may itself contain colons and slashes, so it is whatever follows the fifth
/// colon.
///
/// This is a SYNTAX test only, and the tagging operations keep it separate from the
/// lookup on purpose: a candidate that is not an ARN at all is a client-side mistake
/// (400 `ValidationException`), which is a different answer from a well-formed ARN
/// that names no resource this service holds (404 `ResourceNotFoundException`).
pub fn is_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// The current wall-clock instant, as the modelled `CreatedDate`/`ModifiedDate` timestamp.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The region this request was signed for, for the ARNs the response carries.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh, system-generated resource id of the form `<prefix>-<17 hex digits>`.
///
/// Unpredictable (a per-process-seeded hasher over a process-wide sequence number and
/// the clock) so ids are not guessable, and `taken` is consulted by the caller to
/// guarantee the id is not already in use.
pub fn new_resource_id(prefix: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Distinguishes two ids minted inside one clock tick.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    /// Odd 64-bit constant (the golden-ratio mix used by `hashbrown`) so the low and
    /// high halves of the digest see different inputs.
    const MIX: u64 = 0x9e37_79b9_7f4a_7c15;

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();

    let seed = ::std::collections::hash_map::RandomState::new();
    let mut high = seed.build_hasher();
    high.write_u64(sequence);
    high.write_u64(nanos);
    let mut low = seed.build_hasher();
    low.write_u64(nanos);
    low.write_u64(sequence.wrapping_mul(MIX));

    let digits = format!("{:016x}{:016x}", high.finish(), low.finish());
    format!("{prefix}-{}", &digits[..ID_HEX_DIGITS])
}

/// Why a `nextToken` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// The opaque `nextToken` that resumes an enumeration at `id`.
fn encode_page_token(id: &str) -> String {
    ::hex::encode(id)
}

/// One page of `ids`, plus the token that resumes after it.
///
/// `next_token` is the token a previous page returned; `max_results` is the caller's
/// page size (already held to the modelled `1..100` range by the boundary validator).
/// A token that does not decode is refused; one that decodes to an id no longer in
/// `ids` (its resource was deleted between pages) ends the enumeration with an empty
/// page, rather than refusing a token this service really did issue.
pub fn paginate(
    ids: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<String>, Option<String>), InvalidPageToken> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let resume_at = ::hex::decode(token)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or(InvalidPageToken)?;
            match ids.iter().position(|id| *id == resume_at) {
                Some(index) => index,
                None => return Ok((Vec::new(), None)),
            }
        }
    };

    let page_size = max_results
        .map(|requested| (requested.max(1) as usize).min(MAX_PAGE_SIZE))
        .unwrap_or(MAX_PAGE_SIZE);
    let end = start.saturating_add(page_size).min(ids.len());
    let page = ids[start..end].to_vec();
    let next = ids.get(end).map(|id| encode_page_token(id));
    Ok((page, next))
}
