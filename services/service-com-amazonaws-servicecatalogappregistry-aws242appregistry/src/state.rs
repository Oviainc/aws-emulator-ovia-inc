// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicecatalogappregistry-aws242appregistry service.
//!
//! Besides the state struct, this module is the one home for the few helpers every
//! AppRegistry handler needs: resolving a `{application}` / `{attributeGroup}` path
//! label, minting a resource id, fingerprinting an idempotency token's parameters,
//! encoding the opaque pagination cursor and reading the request's region. `src/lib.rs`
//! is generated and cannot declare another module, so put a new service-wide helper
//! here rather than copying it into a second operation file.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

/// The width of every resource id this service mints.
///
/// `ApplicationId` is `len 26..26, ^[a-z0-9]+$` (smithy_query shape=ApplicationId), so 26
/// is not a choice; `AttributeGroupId` (`len 1..256, ^[-.\w]+$`) accepts the same width,
/// and one width keeps both mints in one helper.
const ID_LENGTH: usize = 26;

/// The region an ARN carries when the request does not name one.
const DEFAULT_REGION: &str = "us-east-1";

/// The most applications this emulator stores, and the most attribute groups.
///
/// A cap, not a guess at AWS's published figure: both create operations model
/// `ServiceQuotaExceededException` ("The maximum number of resources per account has been
/// reached"), and WITHOUT a cap a caller could grow the store until the process ran out of
/// memory, since every record is request-derived (a name, a description, up to 50 tags and,
/// for an attribute group, an up-to-8000-byte `attributes` document). The value is well
/// above anything a client workflow creates, so the bound is invisible in normal use and
/// absolute under abuse.
pub const APPLICATION_QUOTA: usize = 1000;
/// See [`APPLICATION_QUOTA`] — the same bound for the attribute-group store.
pub const ATTRIBUTE_GROUP_QUOTA: usize = 1000;

/// The key of the AppRegistry system tag that says which application a resource belongs
/// to: the pair `CreateApplication` publishes as an application's `applicationTag`, and the
/// tag `SyncResource` reconciles on a resource.
pub const APPLICATION_TAG_KEY: &str = "awsApplication";

/// The most resources whose system-tag sync this emulator remembers.
///
/// A sync records, per resource, what it last left on that resource's AppRegistry system
/// tag, and the resources a caller can sync are not bounded by this service's own stores —
/// so without a cap this map would grow once per distinct resource ever synced. Beyond the
/// cap the OLDEST record is forgotten first: forgetting one only means the next sync of
/// that resource reconciles from the tag the resource itself reports, which is where a
/// first sync starts from anyway.
pub const SYNCED_RESOURCE_LIMIT: usize = 1024;

/// The most idempotency tokens this emulator remembers.
///
/// A token's key is caller-chosen, so this map would otherwise grow once per distinct
/// `clientToken` for the life of the process even when no resource is created by it.
/// Beyond the cap the OLDEST record is forgotten first: forgetting a token only means a
/// retry carrying it creates a new resource instead of replaying one, which is how a real
/// idempotency window behaves once it has expired.
pub const IDEMPOTENCY_TOKEN_LIMIT: usize = 1024;

/// The page size a paginated listing uses when the caller names no `maxResults`.
///
/// The model bounds `maxResults` at 1..100, so 100 is the largest page the caller could
/// ask for and therefore the natural ceiling on one a caller did NOT ask for; without it a
/// zero-parameter listing would serialise the whole store into one response.
const DEFAULT_PAGE_SIZE: usize = 100;

/// Which store an idempotency token's resource lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Application,
    AttributeGroup,
}

/// A persisted application: everything `CreateApplication` was given plus what the
/// service itself minted, so every later read is computed from here.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub creation_time: SystemTime,
    pub last_update_time: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// Ids of the attribute groups associated with this application.
    pub attribute_group_ids: BTreeSet<String>,
    /// `(resourceType, resource)` of every resource associated with this application.
    /// `GetApplication`'s required `associatedResourceCount` is this set's size.
    pub resources: BTreeSet<(String, String)>,
    pub application_tag: BTreeMap<String, String>,
}

/// A persisted attribute group. `attributes` is held verbatim: it is a caller-supplied
/// JSON document and `GetAttributeGroup` must hand back the same string.
#[derive(Debug, Clone)]
pub struct AttributeGroupRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub attributes: String,
    pub creation_time: SystemTime,
    pub last_update_time: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub created_by: Option<String>,
}

/// What a `clientToken` already created, so a retry replays it instead of creating a
/// second resource.
#[derive(Debug, Clone)]
pub struct IdempotentResult {
    pub resource_kind: ResourceKind,
    pub resource_id: String,
    pub request_fingerprint: u64,
    /// When this token was recorded, on the store's own monotonic sequence. The oldest
    /// record is the one [`IDEMPOTENCY_TOKEN_LIMIT`] forgets first.
    pub recorded: u64,
}

/// What the last sync of one resource left on its AppRegistry system tag.
#[derive(Debug, Clone)]
pub struct SyncedApplicationTag {
    /// The value the sync applied, which is the associated application's ARN — `None` where
    /// the sync removed the tag because the resource is associated with no application.
    pub value: Option<String>,
    /// When this record was written, on the store's own monotonic sequence. The oldest
    /// record is the one [`SYNCED_RESOURCE_LIMIT`] forgets first.
    recorded: u64,
}

/// Per-service state for com-amazonaws-servicecatalogappregistry-aws242appregistry. A handler reaches it through [`service_state`].
///
/// One store serves one account and region: the emulator presents a single account id
/// (`engine::EMULATOR_ACCOUNT_ID`) and the engine holds one instance of this struct, so the
/// name indexes below ARE the per-region uniqueness AppRegistry documents for a name.
#[derive(Debug, Default)]
pub struct ComAmazonawsServicecatalogappregistryAws242appregistryState {
    /// Applications by id. A `BTreeMap` so a listing has one total, stable order.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// Application id by name — names are unique per region, and a path label may be one.
    pub application_names: HashMap<String, String>,
    /// Attribute groups by id, ordered for the same reason as `applications`.
    pub attribute_groups: BTreeMap<String, AttributeGroupRecord>,
    /// Attribute group id by name.
    pub attribute_group_names: HashMap<String, String>,
    /// Idempotency tokens that already created something.
    pub idempotency: HashMap<String, IdempotentResult>,
    /// What the last sync left on a resource's AppRegistry system tag, by the resource's
    /// ARN — the record a later sync reconciles against. Ordered, so the listing of what
    /// has been synced has one total order.
    pub synced_application_tags: BTreeMap<String, SyncedApplicationTag>,
    /// Feeds the id mint, so two mints in the same nanosecond still differ.
    mint_counter: u64,
}

impl ComAmazonawsServicecatalogappregistryAws242appregistryState {
    /// The id of the application a `{application}` label names, or `None` when nothing
    /// stored answers to it.
    pub fn resolve_application_id(&self, label: &str) -> Option<String> {
        let key = label_key(label);
        if self.applications.contains_key(key) {
            return Some(key.to_string());
        }
        self.application_names.get(key).cloned()
    }

    /// The id of the attribute group an `{attributeGroup}` label names, or `None`.
    pub fn resolve_attribute_group_id(&self, label: &str) -> Option<String> {
        let key = label_key(label);
        if self.attribute_groups.contains_key(key) {
            return Some(key.to_string());
        }
        self.attribute_group_names.get(key).cloned()
    }

    /// An application id no stored application holds.
    pub fn mint_application_id(&mut self) -> String {
        loop {
            let candidate = mint_id(&self.next_seed());
            if !self.applications.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// An attribute group id no stored attribute group holds.
    pub fn mint_attribute_group_id(&mut self) -> String {
        loop {
            let candidate = mint_id(&self.next_seed());
            if !self.attribute_groups.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn next_seed(&mut self) -> String {
        let sequence = self.next_sequence();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        format!("{sequence}/{nanos}")
    }

    /// The next value of this store's monotonic sequence.
    fn next_sequence(&mut self) -> u64 {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        self.mint_counter
    }

    /// The application a resource of `resource_type` addressed by any of `specifiers` is
    /// associated with, or `None` when no stored application holds that association.
    ///
    /// A resource is addressable by name, by id or by ARN, and an association records the
    /// specifier the caller used — so every specifier that names the SAME resource is
    /// tried, rather than only the one this request carried.
    pub fn application_associated_with(
        &self,
        resource_type: &str,
        specifiers: &[String],
    ) -> Option<&ApplicationRecord> {
        self.applications.values().find(|application| {
            specifiers.iter().any(|specifier| {
                application
                    .resources
                    .contains(&(resource_type.to_string(), specifier.clone()))
            })
        })
    }

    /// The AppRegistry system tag the resource `resource_arn` names currently carries: the
    /// value the last sync of it left, or — where no sync has run — the one the resource
    /// itself reports in `resource_tags`.
    pub fn application_tag_on(
        &self,
        resource_arn: &str,
        resource_tags: &BTreeMap<String, String>,
    ) -> Option<String> {
        match self.synced_application_tags.get(resource_arn) {
            Some(synced) => synced.value.clone(),
            None => resource_tags.get(APPLICATION_TAG_KEY).cloned(),
        }
    }

    /// Record what a sync of `resource_arn` leaves on that resource's AppRegistry system
    /// tag, forgetting the oldest records once more than [`SYNCED_RESOURCE_LIMIT`] are
    /// held.
    pub fn record_application_tag_sync(&mut self, resource_arn: &str, value: Option<String>) {
        let recorded = self.next_sequence();
        self.synced_application_tags.insert(
            resource_arn.to_string(),
            SyncedApplicationTag { value, recorded },
        );
        while self.synced_application_tags.len() > SYNCED_RESOURCE_LIMIT {
            let oldest = self
                .synced_application_tags
                .iter()
                .min_by_key(|(_, held)| held.recorded)
                .map(|(resource, _)| resource.clone());
            match oldest {
                Some(resource) => {
                    self.synced_application_tags.remove(&resource);
                }
                None => break,
            }
        }
    }

    /// Whether the application store is full, so `CreateApplication` must answer
    /// `ServiceQuotaExceededException` rather than grow it further.
    pub fn application_quota_reached(&self) -> bool {
        self.applications.len() >= APPLICATION_QUOTA
    }

    /// Whether the attribute group store is full. See [`Self::application_quota_reached`].
    pub fn attribute_group_quota_reached(&self) -> bool {
        self.attribute_groups.len() >= ATTRIBUTE_GROUP_QUOTA
    }

    /// Remember what a `clientToken` created, forgetting the oldest records once more than
    /// [`IDEMPOTENCY_TOKEN_LIMIT`] are held.
    pub fn remember_token(
        &mut self,
        client_token: String,
        resource_kind: ResourceKind,
        resource_id: String,
        request_fingerprint: u64,
    ) {
        let recorded = self.next_sequence();
        self.idempotency.insert(
            client_token,
            IdempotentResult {
                resource_kind,
                resource_id,
                request_fingerprint,
                recorded,
            },
        );
        while self.idempotency.len() > IDEMPOTENCY_TOKEN_LIMIT {
            let oldest = self
                .idempotency
                .iter()
                .min_by_key(|(_, held)| held.recorded)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.idempotency.remove(&token);
                }
                None => break,
            }
        }
    }
}

impl ApplicationRecord {
    /// This record as the modelled `Application` structure.
    pub fn application(&self) -> crate::ops::types::Application {
        crate::ops::types::Application {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            creation_time: Some(as_date_time(self.creation_time)),
            last_update_time: Some(as_date_time(self.last_update_time)),
            tags: map_out(&self.tags),
            application_tag: map_out(&self.application_tag),
        }
    }

    /// This record as the modelled `ApplicationSummary` structure.
    pub fn summary(&self) -> crate::ops::types::ApplicationSummary {
        crate::ops::types::ApplicationSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            creation_time: Some(as_date_time(self.creation_time)),
            last_update_time: Some(as_date_time(self.last_update_time)),
        }
    }

    /// The number of resources currently associated with this application, as
    /// `GetApplication`'s required `associatedResourceCount`.
    pub fn associated_resource_count(&self) -> i32 {
        i32::try_from(self.resources.len()).unwrap_or(i32::MAX)
    }

    /// `tags`, absent when the application carries none.
    pub fn tags_out(&self) -> Option<HashMap<String, String>> {
        map_out(&self.tags)
    }

    /// `applicationTag`, the key-value pair identifying resources of this application.
    pub fn application_tag_out(&self) -> Option<HashMap<String, String>> {
        map_out(&self.application_tag)
    }

    /// The value a resource of this application carries under [`APPLICATION_TAG_KEY`].
    pub fn application_tag_value(&self) -> Option<String> {
        self.application_tag.get(APPLICATION_TAG_KEY).cloned()
    }
}

impl AttributeGroupRecord {
    /// This record as the modelled `AttributeGroup` structure.
    pub fn attribute_group(&self) -> crate::ops::types::AttributeGroup {
        crate::ops::types::AttributeGroup {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            creation_time: Some(as_date_time(self.creation_time)),
            last_update_time: Some(as_date_time(self.last_update_time)),
            tags: map_out(&self.tags),
        }
    }

    /// `tags`, absent when the attribute group carries none.
    pub fn tags_out(&self) -> Option<HashMap<String, String>> {
        map_out(&self.tags)
    }
}

/// The name or id a path label carries.
///
/// Every `{application}` / `{attributeGroup}` label accepts a name, an id or an ARN
/// (see the label patterns in `OPERATIONS.md`), and an ARN ends in
/// `/applications/<id>` or `/attribute-groups/<id>` — so the last `/`-separated
/// segment is the key to resolve, and a bare name or id is already that key.
fn label_key(label: &str) -> &str {
    if label.starts_with("arn:") {
        label.rsplit('/').next().unwrap_or(label)
    } else {
        label
    }
}

/// A stored instant as the modelled `date-time` timestamp.
fn as_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// A stored map as a modelled optional map member: absent when empty, which is how the
/// real service answers a resource that carries no tags.
fn map_out(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if map.is_empty() {
        None
    } else {
        Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// A stable 64-bit digest of the parameters a `clientToken` was first used with.
///
/// A retry carrying the same token and the same parameters replays the stored resource;
/// one carrying the same token and different parameters is a conflict, so the two have
/// to be told apart. Each field is length-prefixed, so no two different field lists hash
/// the same bytes.
pub fn fingerprint(fields: &[&str]) -> u64 {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    let digest = hasher.finalize();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(head)
}

/// [`ID_LENGTH`] lowercase-alnum characters: the hex of a SHA-256 over `seed`, truncated.
fn mint_id(seed: &str) -> String {
    let mut id = hex::encode(Sha256::digest(seed.as_bytes()));
    id.truncate(ID_LENGTH);
    id
}

/// A `nextToken` the named listing never issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// The cursor a paginated listing hands back.
///
/// It carries the listing it was issued for and the last id that listing returned, so
/// the next page resumes immediately after that id even when items were added or removed
/// in between. Hex of those two keeps the token opaque to the caller and inside the
/// model's `nextToken` pattern `^[A-Za-z0-9+/=]+$`.
fn encode_page_token(kind: &str, after: &str) -> String {
    hex::encode(format!("{kind}/{after}"))
}

/// The id a token of this `kind` resumes after, or `None` when the token is not one this
/// listing issued.
fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let after = decoded.strip_prefix(kind)?.strip_prefix('/')?;
    Some(after.to_string())
}

/// The slice of `ordered_ids` one page covers, and the token that resumes after it.
///
/// `max_results` truncates the page (the model already refuses a value outside 1..100); an
/// absent one falls back to [`DEFAULT_PAGE_SIZE`], so no listing serialises an unbounded
/// response. The last page carries no token.
pub fn page<'a>(
    ordered_ids: &'a [String],
    kind: &str,
    token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(&'a [String], Option<String>), InvalidPageToken> {
    let total = ordered_ids.len();
    let start = match token {
        None => 0,
        Some(token) => {
            let after = decode_page_token(kind, token).ok_or(InvalidPageToken)?;
            ordered_ids.partition_point(|id| id.as_str() <= after.as_str())
        }
    }
    .min(total);
    let limit = match max_results {
        Some(max) => usize::try_from(max).unwrap_or(0),
        None => DEFAULT_PAGE_SIZE,
    };
    let end = start.saturating_add(limit).min(total);
    let page = &ordered_ids[start..end];
    let next_token = (end < total && !page.is_empty())
        .then(|| encode_page_token(kind, &ordered_ids[end - 1]));
    Ok((page, next_token))
}

/// The region the ARNs this service mints carry: the one in the request's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned or its scope
/// names something that is not an AWS region id — `ApplicationArn` and
/// `AttributeGroupArn` require `[a-z]{2}(-gov)?-[a-z]+-\d` in that position.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| looks_like_a_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` matches the region position of this service's ARN patterns,
/// `[a-z]{2}(-gov)?-[a-z]+-\d`.
fn looks_like_a_region(region: &str) -> bool {
    let mut segments: Vec<&str> = region.split('-').collect();
    if segments.len() == 4 {
        if segments[1] != "gov" {
            return false;
        }
        segments.remove(1);
    }
    let [area, direction, number] = segments[..] else {
        return false;
    };
    area.len() == 2
        && area.bytes().all(|byte| byte.is_ascii_lowercase())
        && !direction.is_empty()
        && direction.bytes().all(|byte| byte.is_ascii_lowercase())
        && number.len() == 1
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicecatalogappregistryAws242appregistryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicecatalogappregistryAws242appregistryState>)
}
