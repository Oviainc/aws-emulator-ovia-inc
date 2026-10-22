// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-datazone-datazone service.
//!
//! The DataZone emulator stores every resource it serves here: a handler reads and
//! writes this struct and NOTHING else, so a create is observable by the next
//! get/list and a delete is observable by the next call.
//!
//! # Time and clock-owed transitions
//!
//! Instants are kept as milliseconds since the Unix epoch ([`now_ms`]) and rendered
//! with [`date_time`]. A domain AWS answers `CREATING` becomes `AVAILABLE`, and one it
//! answers `DELETING` disappears, once the clock passes the instant recorded in
//! `status_due_ms` — materialised by [`ComAmazonawsDatazoneDatazoneState::settle_clock`],
//! which every handler calls first. No handler ever sleeps for it.

use std::collections::BTreeMap;

/// The span the emulator takes to finish a domain lifecycle step AWS reports as
/// in-progress (`CREATING` -> `AVAILABLE`, `DELETING` -> gone).
///
/// AWS does not document how long either takes, so this is a CHOSEN span. It is
/// deliberately non-zero: a caller that polls immediately after the call must still
/// observe the in-progress status the real service reports.
pub const LIFECYCLE_SETTLE_MS: i64 = 300;

/// `DomainStatus` values this emulator uses (model enum `com.amazonaws.datazone#DomainStatus`:
/// AVAILABLE | CREATING | CREATION_FAILED | DELETED | DELETING | DELETION_FAILED).
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";
pub const DOMAIN_STATUS_AVAILABLE: &str = "AVAILABLE";
pub const DOMAIN_STATUS_DELETING: &str = "DELETING";

/// The `ProjectStatus` a project AWS has finished creating carries (model enum
/// `com.amazonaws.datazone#ProjectStatus`: ACTIVE | DELETING | UPDATING | DELETE_FAILED).
pub const PROJECT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `DomainVersion` a domain gets when the request names none (model enum
/// `com.amazonaws.datazone#DomainVersion`: V1 | V2).
pub const DEFAULT_DOMAIN_VERSION: &str = "V1";

/// The `Revision` (`com.amazonaws.datazone#Revision`, len 1..64) a freshly created
/// revisioned resource carries: DataZone numbers revisions from 1, as a decimal string.
pub const FIRST_REVISION: &str = "1";

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Render an epoch-millisecond instant as the modelled timestamp type.
pub fn date_time(ms: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(ms as f64 / 1000.0)
}

/// One stored DataZone domain, with the projects nested under it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub single_sign_on: Option<StoredSingleSignOn>,
    pub domain_execution_role: String,
    pub service_role: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub status: String,
    pub portal_url: Option<String>,
    pub domain_version: String,
    pub root_domain_unit_id: String,
    pub managed_account_id: String,
    pub tags: BTreeMap<String, String>,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    /// When the status this domain currently reports stops being true: the instant
    /// `CREATING` becomes `AVAILABLE`, or `DELETING` becomes gone. `None` for a
    /// settled domain.
    pub status_due_ms: Option<i64>,
    /// The projects stored in this domain, so they go with it when it is deleted.
    /// Bounded by [`MAX_PROJECTS_PER_DOMAIN`].
    pub projects: BTreeMap<String, Project>,
    /// The custom asset types stored in this domain, keyed by the type's NAME — the
    /// `AssetTypeIdentifier` `GetAssetType`/`DeleteAssetType` address a type by — so a
    /// name is unique within the domain and the types go with it when it is deleted.
    /// Bounded by [`MAX_ASSET_TYPES_PER_DOMAIN`].
    pub asset_types: BTreeMap<String, AssetType>,
}

/// The stored form of the modelled `SingleSignOn` structure.
#[derive(Debug, Clone, Default)]
pub struct StoredSingleSignOn {
    pub sso_type: Option<String>,
    pub user_assignment: Option<String>,
    pub idc_instance_arn: Option<String>,
}

/// One stored DataZone project.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    pub project_status: String,
    pub created_by: String,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    pub glossary_terms: Vec<String>,
    pub domain_unit_id: Option<String>,
    pub project_profile_id: Option<String>,
    /// `resourceTags` as stored: the request takes a map, the response a list of
    /// `ResourceTag`, so the key order is kept stable by storing it sorted.
    pub resource_tags: BTreeMap<String, String>,
    pub project_category: Option<String>,
}

/// One stored DataZone asset type, at one revision.
///
/// An asset type is a named, revisioned schema: the metadata forms an asset of that
/// type carries. `CreateAssetType` mints revision [`FIRST_REVISION`] and the model
/// declares no update operation for it, so a stored type keeps the revision it was
/// created at.
#[derive(Debug, Clone)]
pub struct AssetType {
    pub name: String,
    pub domain_id: String,
    pub revision: String,
    pub description: Option<String>,
    /// The metadata forms attached to the type, keyed by form name. A `BTreeMap` so the
    /// forms are reported in one stable order whatever order the request listed them in;
    /// the modelled `FormsOutputMap` is bounded at 10 entries, which the request-level
    /// constraint on `formsInput` already enforces.
    pub forms: BTreeMap<String, AssetTypeForm>,
    pub owning_project_id: String,
    pub created_at_ms: i64,
    pub created_by: String,
    pub updated_at_ms: i64,
    pub updated_by: String,
}

/// One metadata form attached to an asset type: the stored form of the modelled
/// `FormEntryOutput` (`typeName`, `typeRevision`, `required`).
#[derive(Debug, Clone)]
pub struct AssetTypeForm {
    pub type_name: String,
    pub type_revision: String,
    pub required: Option<bool>,
}

/// The most domains this emulator stores.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop cannot
/// grow the emulator's heap — or the per-request [`ComAmazonawsDatazoneDatazoneState::settle_clock`]
/// scan — without bound. `CreateDomain` models `ServiceQuotaExceededException`, which
/// is the refusal once the store is full.
pub const MAX_DOMAINS: usize = 100;

/// The most projects this emulator stores in one domain, for the same reason and with
/// the same refusal (`CreateProject` models `ServiceQuotaExceededException` too).
pub const MAX_PROJECTS_PER_DOMAIN: usize = 500;

/// The most asset types this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateAssetType` models `ServiceQuotaExceededException` too).
pub const MAX_ASSET_TYPES_PER_DOMAIN: usize = 500;

/// The hard ceiling on remembered `@idempotencyToken`s.
///
/// The token keys are CALLER-supplied, so the map is a caller-driven growth site and
/// must be bounded: [`ComAmazonawsDatazoneDatazoneState::bound_idempotency`] first
/// releases every token whose domain is gone (such a token can never be replayed into
/// anything), then evicts the oldest entries beyond this ceiling. At most one token is
/// recorded per stored domain, so this is never the binding cap in practice; it is
/// enforced anyway so caller-chosen strings cannot accumulate on their own.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_DOMAINS;

/// What one remembered `@idempotencyToken` points at, plus when it was remembered so
/// the oldest can be evicted first.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub resource_id: String,
    pub recorded_at_ms: i64,
}

/// Per-service state for com-amazonaws-datazone-datazone. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatazoneDatazoneState {
    /// Every live domain, keyed by its `dzd_…` id. A `BTreeMap` so every listing
    /// iterates in one stable order and a pagination token can resume from a key.
    /// Bounded by [`MAX_DOMAINS`].
    pub domains: BTreeMap<String, Domain>,
    /// `clientToken` -> the domain the first request carrying it created, so a repeated
    /// `@idempotencyToken` request returns that domain instead of creating a second one.
    ///
    /// Bounded: see [`MAX_IDEMPOTENCY_TOKENS`]. Write it through
    /// [`Self::record_idempotency`], which applies the bound, rather than inserting
    /// directly.
    idempotency: BTreeMap<String, IdempotencyRecord>,
    /// Monotonic counter behind every minted id, so no two resources in one process
    /// ever share an id even when minted in the same millisecond.
    id_seq: u64,
}

impl ComAmazonawsDatazoneDatazoneState {
    /// The domain a `clientToken` already created, if that domain is still stored.
    pub fn idempotent_domain(&self, token: &str) -> Option<&Domain> {
        let record = self.idempotency.get(token)?;
        self.domains.get(&record.resource_id)
    }

    /// Remember that `token` created `resource_id`, and re-apply the bound.
    ///
    /// Call it AFTER the resource is stored: a token naming a resource that is not in
    /// the store is released immediately by the bound below.
    pub fn record_idempotency(&mut self, token: String, resource_id: String, now: i64) {
        self.idempotency.insert(
            token,
            IdempotencyRecord {
                resource_id,
                recorded_at_ms: now,
            },
        );
        self.bound_idempotency();
    }

    /// Keep the remembered-token map bounded: release every token whose domain is no
    /// longer stored, then evict the oldest entries beyond [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// Reached from [`Self::record_idempotency`] and from [`Self::settle_clock`] (which
    /// every handler calls), so a create/delete loop with fresh tokens releases each
    /// token as its domain disappears and can never grow the store without limit.
    fn bound_idempotency(&mut self) {
        let domains = &self.domains;
        self.idempotency
            .retain(|_, record| domains.contains_key(&record.resource_id));
        let Some(excess) = self
            .idempotency
            .len()
            .checked_sub(MAX_IDEMPOTENCY_TOKENS)
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let mut by_age: Vec<(i64, String)> = self
            .idempotency
            .iter()
            .map(|(token, record)| (record.recorded_at_ms, token.clone()))
            .collect();
        by_age.sort();
        for (_, token) in by_age.into_iter().take(excess) {
            self.idempotency.remove(&token);
        }
    }
    /// Mint an opaque id suffix: lowercase base-36, so it satisfies both the
    /// `^[a-zA-Z0-9_-]{1,36}$` (project, domain) and `^[a-z0-9_\-]+$` (domain unit)
    /// patterns the model declares. Unique within a process, and never reused.
    pub fn mint_id(&mut self, now: i64) -> String {
        self.id_seq += 1;
        format!(
            "{}{}",
            base36(u64::try_from(now).unwrap_or(0)),
            base36(self.id_seq)
        )
    }

    /// Materialise every domain lifecycle step the clock already owes at `now`: a
    /// `CREATING` domain whose span has elapsed becomes `AVAILABLE`, and a `DELETING`
    /// one disappears together with its projects.
    ///
    /// This is the ONLY write a reporting operation makes, and it writes exactly what
    /// was already owed at this instant whether or not anybody called.
    pub fn settle_clock(&mut self, now: i64) {
        let due: Vec<String> = self
            .domains
            .iter()
            .filter(|(_, d)| d.status_due_ms.is_some_and(|due| due <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(domain) = self.domains.get_mut(&id) else {
                continue;
            };
            let due_at = domain.status_due_ms.unwrap_or(now);
            match domain.status.as_str() {
                DOMAIN_STATUS_CREATING => {
                    domain.status = DOMAIN_STATUS_AVAILABLE.to_string();
                    domain.status_due_ms = None;
                    domain.last_updated_at_ms = due_at;
                }
                DOMAIN_STATUS_DELETING => {
                    // The deletion the earlier DeleteDomain call started has
                    // completed: the domain and the projects under it are gone.
                    self.domains.remove(&id);
                }
                // No other status carries a due instant.
                _ => domain.status_due_ms = None,
            }
        }
        // A token naming a domain that has just disappeared can never be replayed
        // into anything, so it is released with the domain.
        self.bound_idempotency();
    }

    /// The domain `id`, when it exists and is not already being deleted.
    pub fn live_domain(&self, id: &str) -> Option<&Domain> {
        self.domains
            .get(id)
            .filter(|d| d.status != DOMAIN_STATUS_DELETING)
    }
}

/// Lowercase base-36 encoding of `value` — the alphabet every DataZone id pattern
/// accepts.
fn base36(mut value: u64) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(ALPHABET[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Request-derived values and pagination, shared by this service's handlers.
//
// `src/ops/mod.rs` is generated (it `include!`s the generated module list) and so is
// `src/lib.rs`, so a helper several operations need lives in this file — the service's
// own hand-editable module — rather than in a module neither generated file can name.
// ---------------------------------------------------------------------------

/// The region the caller signed for, defaulting to the region a generated emulator
/// presents when the request carries no SigV4 scope (an anonymous call).
pub fn caller_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an anonymous request is served in: AWS SDK clients default to
/// `us-east-1`, so an unsigned call is reported there.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The principal that made this call, as the `createdBy` of what it creates.
///
/// Derived from the CALLER's identity — the access key id its SigV4 scope names —
/// never from the request body, so two different callers creating the same-named
/// project get different `createdBy` values.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::engine::extract_akid(&ctx.parts().headers);
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// Encode the key a page ended on as the opaque `nextToken` a caller passes back.
///
/// Opaque to the client (hex of the key's bytes) but a pure function of the key, so
/// the next page resumes exactly after it however the stored map has changed.
pub fn encode_page_token(last_key: &str) -> String {
    hex::encode(last_key.as_bytes())
}

/// The key a `nextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatazoneDatazoneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatazoneDatazoneState>)
}
