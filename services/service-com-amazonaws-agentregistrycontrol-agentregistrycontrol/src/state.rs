// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-agentregistrycontrol-agentregistrycontrol service.
//!
//! Besides the state struct itself this module holds the helpers every handler of
//! this service needs: identifier minting, identifier/ARN resolution, the request's
//! region, and the clock-owed status transitions the model documents (a registry is
//! created `CREATING` and becomes `READY`; a record is created `CREATING` and becomes
//! `DRAFT`; a deleted registry sits in `DELETING` and is then removed).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApprovalConfiguration, AutoDetection, CustomMetadataSchemaConfiguration,
    Descriptors, DiscoveryConfiguration, EncryptionConfiguration, Provenance,
};

/// The region used for an ARN when the request carries no usable SigV4 credential
/// scope (an unsigned request). Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, per `OPERATIONS.md`.
pub const ARN_SERVICE: &str = "agent-registry";

/// How long a resource this service creates asynchronously stays in its transient
/// status before the clock owes it the next one.
///
/// CHOSEN, not documented: AWS does not publish how long `CreateRegistry`,
/// `CreateRegistryRecord` or `DeleteRegistry` take, so this is the emulator's own
/// span inside the 250-750 ms band the project fixes for a clock-owed transition.
/// It is never zero, so a caller that acts immediately after a create still observes
/// the transient status and the state errors that go with it.
pub const ASYNC_TRANSITION: Duration = Duration::from_millis(500);

/// How many registries one account may hold before `CreateRegistry` answers the
/// modelled `ServiceQuotaExceededException`.
///
/// CHOSEN, not documented: AWS publishes no quota for this service, so the emulator
/// picks a generous one. It exists so a caller-driven store cannot grow without bound.
pub const MAX_REGISTRIES: usize = 1_000;

/// How many records one registry may hold before `CreateRegistryRecord` answers the
/// modelled `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_REGISTRIES`].
pub const MAX_RECORDS_PER_REGISTRY: usize = 10_000;

/// How many `@idempotencyToken` replays are remembered at once. Entries are dropped
/// when the resource they name is removed; this cap bounds what is left, oldest first,
/// so a long-lived process cannot accumulate tokens for ever.
const MAX_IDEMPOTENCY_ENTRIES: usize = 4_096;

/// The page `ListRegistryRecords` returns when the caller names no `maxResults`, so one
/// small request cannot serialise an entire registry. It is `MaxResults`' own documented
/// maximum (range 1..100), and the caller pages on with `nextToken` exactly as it would
/// with an explicit `maxResults`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

// Status values below are the wire values of `RegistryStatus` and
// `RegistryRecordStatus`; the full catalogues are
// `crate::ops::types::REGISTRY_STATUS_VALUES` and `REGISTRY_RECORD_STATUS_VALUES`.

/// `RegistryStatus` while `CreateRegistry`'s asynchronous creation is in flight.
pub const REGISTRY_CREATING: &str = "CREATING";
/// `RegistryStatus` of a usable registry.
pub const REGISTRY_READY: &str = "READY";
/// `RegistryStatus` while `DeleteRegistry`'s asynchronous deletion is in flight.
pub const REGISTRY_DELETING: &str = "DELETING";

/// `RegistryRecordStatus` while `CreateRegistryRecord` is in flight.
pub const RECORD_CREATING: &str = "CREATING";
/// `RegistryRecordStatus` of a created record before it is submitted for approval.
pub const RECORD_DRAFT: &str = "DRAFT";
/// `RegistryRecordStatus` of a submitted record waiting for a curator's manual review.
pub const RECORD_PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// `RegistryRecordStatus` of a record the approval workflow has approved.
pub const RECORD_APPROVED: &str = "APPROVED";

/// The one `AutoApprovalRule` the model declares — the catalogue it comes from is
/// `crate::ops::types::AUTO_APPROVAL_RULE_VALUES`. It selects every submitted record,
/// so a registry configured with it approves a submission without manual review.
pub const AUTO_APPROVAL_RULE_APPROVE_ALL: &str = "APPROVE_ALL";

/// A status change the clock owes a resource: what it becomes, when that is due on
/// the monotonic clock, and the wall-clock instant to stamp on `updatedAt` when it
/// is materialised. Nothing sleeps for it — the next caller that touches the state
/// applies every transition whose instant has passed.
#[derive(Debug, Clone)]
pub struct DueTransition<T> {
    /// When the transition becomes due, on the monotonic clock.
    pub due_at: Instant,
    /// The wall-clock instant the transition happens at (`updatedAt`).
    pub at: DateTime,
    /// What the resource becomes.
    pub to: T,
}

impl<T> DueTransition<T> {
    /// A transition due [`ASYNC_TRANSITION`] from now.
    pub fn after_async_span(to: T) -> Self {
        Self {
            due_at: Instant::now() + ASYNC_TRANSITION,
            at: DateTime::from_secs_f64(unix_secs() + ASYNC_TRANSITION.as_secs_f64()),
            to,
        }
    }
}

/// What a registry's pending transition turns it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryTransition {
    /// `CREATING` -> `READY`: creation finished.
    Ready,
    /// `DELETING` -> gone: the registry and its records are removed.
    Removed,
}

/// What a record's pending transition turns it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordTransition {
    /// `CREATING` -> `DRAFT`: creation finished; the record can be submitted for approval.
    Draft,
}

/// A registry, exactly as `CreateRegistry` stored it.
#[derive(Debug)]
pub struct Registry {
    pub registry_id: String,
    pub registry_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub discovery_configuration: Option<DiscoveryConfiguration>,
    pub approval_configuration: Option<ApprovalConfiguration>,
    pub custom_metadata_schema_configuration: Option<CustomMetadataSchemaConfiguration>,
    pub auto_detection: Option<AutoDetection>,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: BTreeMap<String, String>,
    /// Keyed by record id, so `ListRegistryRecords` has a stable order and its
    /// `nextToken` names a well-defined resume point.
    pub records: BTreeMap<String, RegistryRecord>,
    pub pending: Option<DueTransition<RegistryTransition>>,
}

impl Registry {
    /// Whether this registry's approval configuration approves a record on submission
    /// instead of sending it to manual review.
    ///
    /// `ApprovalConfiguration.autoApprovalRules` are "the rules that determine which
    /// registry records are automatically approved on submission. When omitted or
    /// empty, submitted records require manual review." A rule covers a submission when
    /// its selector holds for the record; [`AUTO_APPROVAL_RULE_APPROVE_ALL`], the only
    /// rule the model declares, selects every record. A rule outside that catalogue
    /// covers nothing, so an unrecognised one leaves the submission for manual review
    /// rather than approving it unseen.
    pub fn auto_approves_submissions(&self) -> bool {
        self.approval_configuration
            .as_ref()
            .and_then(|configuration| configuration.auto_approval_rules.as_ref())
            .is_some_and(|rules| {
                rules
                    .iter()
                    .any(|rule| rule == AUTO_APPROVAL_RULE_APPROVE_ALL)
            })
    }
}

/// A registry record, exactly as `CreateRegistryRecord` stored it.
#[derive(Debug)]
pub struct RegistryRecord {
    pub record_id: String,
    pub record_arn: String,
    pub registry_arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub record_type: String,
    pub descriptors: Option<Descriptors>,
    pub record_version: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub provenance: Vec<Provenance>,
    pub custom_metadata: Option<::aws_smithy_types::Document>,
    pub custom_metadata_schema_compliance_status: Option<String>,
    pub created_by: Option<String>,
    pub created_by_auto_detection: Option<bool>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<DueTransition<RecordTransition>>,
}

/// What one `@idempotencyToken` replay remembers: the ARN the first call answered, and
/// the order it was remembered in, so the oldest entry is the one evicted at the cap.
#[derive(Debug, Clone)]
pub struct IdempotentAnswer {
    arn: String,
    seq: u64,
}

/// Per-service state for com-amazonaws-agentregistrycontrol-agentregistrycontrol. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAgentregistrycontrolAgentregistrycontrolState {
    /// Every registry, keyed by its registry id.
    pub registries: HashMap<String, Registry>,
    /// Registry name -> registry id, so a duplicate name is a `ConflictException`.
    pub registry_name_index: HashMap<String, String>,
    /// `<operation>#<clientToken>` -> the ARN that call answered. The operation is part
    /// of the key, so one token reused across two different creates cannot make one
    /// replay the other's answer.
    idempotency: HashMap<String, IdempotentAnswer>,
    /// Insertion order for the idempotency map, for eviction at the cap.
    idempotency_seq: u64,
    /// The earliest instant at which some stored transition becomes due, so a request
    /// that arrives before then does no scanning at all. `None` when nothing is pending.
    next_due: Option<Instant>,
}

impl ComAmazonawsAgentregistrycontrolAgentregistrycontrolState {
    /// Apply every status transition the clock already owes.
    ///
    /// Call this at the top of every handler, before reading or writing a resource:
    /// it only materialises a change that was already due at this instant, so a read
    /// that calls it reports what a caller would see whether or not anybody asked.
    pub fn materialise_due_transitions(&mut self) {
        let now = Instant::now();
        // Nothing is due: no store is walked, so this costs the same whatever the
        // stores hold.
        if self.next_due.is_none_or(|due_at| due_at > now) {
            return;
        }

        let mut earliest: Option<Instant> = None;
        let mut arm = |due_at: Instant| {
            earliest = Some(match earliest {
                Some(current) if current <= due_at => current,
                _ => due_at,
            });
        };
        let mut removed: Vec<(String, String)> = Vec::new();
        self.registries.retain(|_, registry| {
            if let Some(pending) = registry.pending.clone() {
                if pending.due_at <= now {
                    match pending.to {
                        RegistryTransition::Ready => {
                            registry.status = REGISTRY_READY.to_string();
                            registry.updated_at = pending.at;
                            registry.pending = None;
                        }
                        RegistryTransition::Removed => {
                            removed.push((registry.name.clone(), registry.registry_arn.clone()));
                            return false;
                        }
                    }
                } else {
                    arm(pending.due_at);
                }
            }
            for record in registry.records.values_mut() {
                if let Some(pending) = record.pending.clone() {
                    if pending.due_at <= now {
                        match pending.to {
                            RecordTransition::Draft => {
                                record.status = RECORD_DRAFT.to_string();
                                record.updated_at = pending.at;
                                record.pending = None;
                            }
                        }
                    } else {
                        arm(pending.due_at);
                    }
                }
            }
            true
        });
        self.next_due = earliest;
        for (name, registry_arn) in removed {
            self.registry_name_index.remove(&name);
            // The registry and its records are gone, so the tokens that named them are
            // of no use to anybody: a replay of that create mints a new resource.
            self.forget_idempotency_under(&registry_arn);
        }
    }

    /// Record that a transition becomes due at `due_at`, so
    /// [`materialise_due_transitions`](Self::materialise_due_transitions) wakes for it.
    pub fn arm_transition(&mut self, due_at: Instant) {
        self.next_due = Some(match self.next_due {
            Some(current) if current <= due_at => current,
            _ => due_at,
        });
    }

    /// The ARN `operation` answered the first time it saw `token`, if it is still
    /// remembered.
    pub fn idempotent_answer(&self, operation: &str, token: &str) -> Option<&str> {
        self.idempotency
            .get(&idempotency_key(operation, token))
            .map(|answer| answer.arn.as_str())
    }

    /// Remember what `operation` answered for `token`, evicting the oldest entry when
    /// the cap is reached so this store cannot grow without bound.
    pub fn remember_idempotent_answer(&mut self, operation: &str, token: &str, arn: &str) {
        self.idempotency_seq = self.idempotency_seq.wrapping_add(1);
        let key = idempotency_key(operation, token);
        if self.idempotency.len() >= MAX_IDEMPOTENCY_ENTRIES && !self.idempotency.contains_key(&key)
        {
            let oldest = self
                .idempotency
                .iter()
                .min_by_key(|(_, answer)| answer.seq)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.idempotency.remove(&oldest);
            }
        }
        self.idempotency.insert(
            key,
            IdempotentAnswer {
                arn: arn.to_string(),
                seq: self.idempotency_seq,
            },
        );
    }

    /// Forget every remembered answer naming a removed resource: the resource itself and
    /// anything nested under it, since a registry's ARN is the prefix of its records'
    /// ARNs. The prefix only counts when the ARN either ends there or continues with the
    /// `/` that starts a nested resource, so one id that is a text prefix of another
    /// (`RegistryId` is 12..16 characters) cannot take the other's tokens with it.
    pub fn forget_idempotency_under(&mut self, arn: &str) {
        self.idempotency.retain(|_, answer| {
            answer.arn != arn
                && !answer
                    .arn
                    .strip_prefix(arn)
                    .is_some_and(|nested| nested.starts_with('/'))
        });
    }

    /// A registry id no registry holds: 12 characters of `[0-9a-f]`, which satisfies
    /// `RegistryId`'s `^[a-zA-Z0-9]{12,16}$`.
    pub fn mint_registry_id(&self) -> String {
        loop {
            let candidate = mint_id();
            if !self.registries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// A record id no record of `registry_id` holds: 12 characters of `[0-9a-f]`,
    /// which satisfies `RegistryRecordId`'s `^[a-zA-Z0-9]{12}$`.
    pub fn mint_record_id(&self, registry_id: &str) -> String {
        loop {
            let candidate = mint_id();
            let taken = self
                .registries
                .get(registry_id)
                .is_some_and(|registry| registry.records.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }
}

/// The key an idempotent answer is remembered under: the operation and the token, so a
/// token reused across two operations cannot cross answers.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// A process-wide mint counter, so two ids minted in the same nanosecond still differ.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// An identifier of 12 characters drawn from `[0-9a-f]`, which is 48 bits of the value
/// below — enough for `RegistryId`'s `^[a-zA-Z0-9]{12,16}$` and `RegistryRecordId`'s
/// `^[a-zA-Z0-9]{12}$`, and the caller re-mints on a collision with a stored id, so
/// uniqueness is guaranteed by the store rather than by the draw.
///
/// The bits come from the standard library's
/// [`std::collections::hash_map::RandomState`], whose SipHash keys the host seeds from
/// the operating system when it is constructed, mixed with the wall clock and a
/// process-wide counter so two draws in the same instant cannot coincide.
fn mint_id() -> String {
    use ::std::hash::BuildHasher;

    let sequence = MINT_SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let drawn = ::std::collections::hash_map::RandomState::new().hash_one((sequence, nanos));
    format!("{:012x}", drawn & 0x0000_ffff_ffff_ffff)
}

/// The id named by a `RegistryIdentifier` / `RecordIdentifier` label, which the model
/// lets a caller give either bare or as the resource's full ARN. The id is the ARN's
/// last `/`-separated segment, and a bare id has no `/` at all.
pub fn identifier_to_id(identifier: &str) -> &str {
    identifier
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(identifier)
}

/// The region to build an ARN in: the one the request's SigV4 credential scope names,
/// else [`DEFAULT_REGION`]. A scope value that is not shaped like a region is ignored,
/// so a minted ARN always satisfies the `[a-z0-9-]+` region segment the model declares.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The current wall-clock instant, as the `DateTimestamp` the model binds to
/// `createdAt` / `updatedAt`.
pub fn now() -> DateTime {
    DateTime::from_secs_f64(unix_secs())
}

/// Seconds since the Unix epoch, as a float, so a timestamp and a timestamp offset
/// by a span are built the same way.
fn unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The ARN of a registry: `arn:aws:agent-registry:<region>:<account>:registry/<id>`.
pub fn registry_arn(region: &str, registry_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("registry/{registry_id}"))
}

/// The ARN of a record, which the model nests under its registry's ARN.
pub fn record_arn(registry_arn: &str, record_id: &str) -> String {
    format!("{registry_arn}/record/{record_id}")
}

/// The opaque `nextToken` `ListRegistryRecords` hands out: it carries the registry it was
/// issued for and the last record id of the page, hex-encoded (the `hex` crate this crate
/// already depends on) so a caller reads it as nothing but a cursor — and hex satisfies
/// `NextToken`'s `^\S*$`, holding no whitespace.
pub fn encode_page_token(registry_id: &str, record_id: &str) -> String {
    ::hex::encode(format!("{registry_id}/{record_id}"))
}

/// The record id a page token resumes after, or `None` when the token is not one this
/// service issued for `registry_id` — which is a refusal, not an implicit first page.
pub fn decode_page_token(registry_id: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (issued_for, record_id) = decoded.split_once('/')?;
    (issued_for == registry_id && !record_id.is_empty()).then(|| record_id.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>)
}
