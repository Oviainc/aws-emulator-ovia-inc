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

/// How many tags one resource may hold before `TagResource` answers the modelled
/// `ServiceQuotaExceededException`.
///
/// MODELLED: it is `TagsMap`'s own `@length` maximum, which every operation carrying a
/// tag map declares for one request's worth of tags, applied to what the resource ends up
/// holding.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The fixed text a tagging operation refuses an ARN with when it is not the ARN of a
/// resource this service holds; the offending value follows it, taken from the request's
/// `resourceArn` label.
pub const UNRECOGNIZED_RESOURCE_ARN: &str = "Unrecognized Agent Registry resource ARN: ";

/// How this service states that the REGISTRY an identifier named is not there.
///
/// The ONE place this sentence is written, so no two operations of this service word the
/// same error class differently. Its fixed text names the resource class and the kind of
/// identifier the registry is addressed by, as a capitalised sentence ending in a period;
/// the one variable part is the identifier exactly as the request supplied it, since
/// `RegistryIdentifier` admits either the bare id or the full ARN and the value that
/// resolved to nothing is the value the caller sent.
pub fn registry_not_found_message(registry_identifier: &str) -> String {
    format!("Registry with ARN {registry_identifier} not found.")
}

/// How this service states that the REGISTRY RECORD an identifier named is not there, in
/// the same form as [`registry_not_found_message`] and for the same reason. Its one
/// variable part is the record's ID — the sentence says so — which is what a
/// `RecordIdentifier` carries whether it was given bare or as the record's ARN
/// ([`identifier_to_id`]). It names the record alone: the registry that was searched is
/// not part of this sentence, so the refusal carries no identifier of another resource.
pub fn record_not_found_message(record_id: &str) -> String {
    format!("Registry record with ID {record_id} not found.")
}

/// How this service states that a name a create asked for is already held by a live
/// resource of the same class in the same scope: the class, the requested name in single
/// quotes so a client can read it back out of the sentence, and the scope the name has to
/// be unique within. The ONE place this sentence is written, for the same reason as the
/// not-found sentences above.
fn duplicate_name_message(resource: &str, name: &str, scope: &str) -> String {
    format!("A {resource} with name '{name}' already exists in {scope}")
}

/// [`duplicate_name_message`] for a record, whose name is unique within its registry —
/// named by the identifier the request addressed that registry by.
pub fn duplicate_record_name_message(name: &str, registry_identifier: &str) -> String {
    duplicate_name_message("record", name, &format!("registry {registry_identifier}"))
}

/// [`duplicate_name_message`] for a registry, whose name is unique within the account.
pub fn duplicate_registry_name_message(name: &str) -> String {
    duplicate_name_message("registry", name, "this account")
}

/// `ValidationExceptionReason`'s modelled default, which the member carries for a request
/// refused because one of its fields did not satisfy a requirement. The whole catalogue is
/// `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`.
pub const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

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

/// What one `@idempotencyToken` replay remembers: the ARN the first call answered, a
/// fingerprint of the parameters it was presented with ([`idempotency_fingerprint`]), and
/// the order it was remembered in, so the oldest entry is the one evicted at the cap.
#[derive(Debug, Clone)]
pub struct IdempotentAnswer {
    arn: String,
    fingerprint: String,
    seq: u64,
}

/// What a presented `@idempotencyToken` means for the call presenting it.
///
/// `ClientToken` is `@idempotencyToken`: "A unique, case-sensitive identifier to ensure
/// that the operation completes no more than one time. If this token matches a previous
/// request, the service ignores the request, but does not return an error" — which binds
/// the token to the parameters of the request that first used it. A second call presenting
/// it therefore has three possible meanings, and only one of them creates anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// No remembered call used this token: the request is a first call and creates.
    Unused,
    /// A remembered call used it with the SAME parameters: the request is ignored and
    /// answered with the ARN that call minted.
    Replayed(String),
    /// A remembered call used it with DIFFERENT parameters: the token is already bound to
    /// another request, which is a conflict rather than a create
    /// ([`IDEMPOTENCY_PARAMETER_MISMATCH`]).
    ParametersDiffer,
}

/// What a create answers when an `@idempotencyToken` is presented with parameters other
/// than the ones it was first used with. Wholly fixed text: it names neither the token nor
/// any parameter, because the token is bound to the whole request and not to one member of
/// it.
pub const IDEMPOTENCY_PARAMETER_MISMATCH: &str =
    "Idempotency token already used with different request parameters";

/// Per-service state for com-amazonaws-agentregistrycontrol-agentregistrycontrol. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAgentregistrycontrolAgentregistrycontrolState {
    /// Every registry, keyed by its registry id.
    pub registries: HashMap<String, Registry>,
    /// Registry name -> registry id, so a duplicate name is a `ConflictException`.
    pub registry_name_index: HashMap<String, String>,
    /// `<operation>#<clientToken>` -> what that call answered, and the parameters it
    /// answered them for. The operation is part of the key, so one token reused across
    /// two different creates cannot make one replay the other's answer.
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

    /// What `token` means for a call of `operation` whose parameters fingerprint as
    /// `fingerprint`: unused, a replay of the same request, or the same token bound to a
    /// different one. See [`TokenReplay`].
    pub fn token_replay(&self, operation: &str, token: &str, fingerprint: &str) -> TokenReplay {
        match self.idempotency.get(&idempotency_key(operation, token)) {
            None => TokenReplay::Unused,
            Some(answer) if answer.fingerprint == fingerprint => {
                TokenReplay::Replayed(answer.arn.clone())
            }
            Some(_) => TokenReplay::ParametersDiffer,
        }
    }

    /// Remember what `operation` answered for `token`, and the parameters it was presented
    /// with, evicting the oldest entry when the cap is reached so this store cannot grow
    /// without bound.
    pub fn remember_idempotent_answer(
        &mut self,
        operation: &str,
        token: &str,
        fingerprint: &str,
        arn: &str,
    ) {
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
                fingerprint: fingerprint.to_string(),
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

    /// The tag set of the resource `addressed` names, to read, or `None` when this state
    /// holds no such resource.
    ///
    /// The reading half of [`Self::resource_tags_mut`]: one lookup for every tagging
    /// operation, so what a read reports is the set a write wrote.
    pub fn resource_tags(&self, addressed: &TaggableResource) -> Option<&BTreeMap<String, String>> {
        match addressed {
            TaggableResource::Registry { registry_id } => {
                Some(&self.registries.get(registry_id)?.tags)
            }
            TaggableResource::Record {
                registry_id,
                record_id,
            } => Some(
                &self
                    .registries
                    .get(registry_id)?
                    .records
                    .get(record_id)?
                    .tags,
            ),
        }
    }

    /// The tag set of the resource `addressed` names, to read or to write, or `None` when
    /// this state holds no such resource.
    ///
    /// One lookup for every tagging operation, so the resource a tag is written to is the
    /// one a later read reports it from.
    pub fn resource_tags_mut(
        &mut self,
        addressed: &TaggableResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match addressed {
            TaggableResource::Registry { registry_id } => {
                Some(&mut self.registries.get_mut(registry_id)?.tags)
            }
            TaggableResource::Record {
                registry_id,
                record_id,
            } => Some(
                &mut self
                    .registries
                    .get_mut(registry_id)?
                    .records
                    .get_mut(record_id)?
                    .tags,
            ),
        }
    }
}

/// The key an idempotent answer is remembered under: the operation and the token, so a
/// token reused across two operations cannot cross answers.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// A fingerprint of the parameters an `@idempotencyToken` was presented with: the request
/// serialized against its own modelled schema, with every object's members in key order.
///
/// The caller hands the request in with its token member CLEARED, so what is fingerprinted
/// is the parameters the token names and not the token itself.
///
/// Rendered through the protocol's own JSON serializer and the model's schema, so the
/// member names and the value forms are the model's rather than a second reading of the
/// request held here. Sorted because neither rendering is a function of the value on its
/// own: a map member (`tags`, and the `Document` a record's `customMetadata` is) is written
/// in its `HashMap` iteration order, which two maps holding the same entries do not share,
/// so one request would otherwise fingerprint two ways. `Debug` is the fallback for a
/// request the serializer cannot render, which leaves such a request compared by value
/// rather than compared not at all.
pub fn idempotency_fingerprint<T>(parameters: &T, schema: &::aws_smithy_schema::Schema) -> String
where
    T: ::aws_smithy_schema::serde::SerializableStruct + ::std::fmt::Debug,
{
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer =
        ::aws_smithy_schema_server::json::JsonShapeSerializer::new_with_config(true);
    if serializer.write_struct(schema, parameters).is_err() {
        return format!("{parameters:?}");
    }
    let rendered = serializer.into_string();
    // `serde_json`'s own parse bounds how deep a document it will read, so this walk is
    // bounded by that crate rather than by the request; a document past that bound is not
    // sorted and is compared as rendered.
    match ::serde_json::from_str::<::serde_json::Value>(&rendered) {
        Ok(parsed) => with_sorted_members(parsed).to_string(),
        Err(_) => rendered,
    }
}

/// `value` with the members of every object in key order, which is what makes two
/// renderings of one request the same text.
fn with_sorted_members(value: ::serde_json::Value) -> ::serde_json::Value {
    match value {
        ::serde_json::Value::Object(members) => ::serde_json::Value::Object(
            members
                .into_iter()
                .collect::<BTreeMap<String, ::serde_json::Value>>()
                .into_iter()
                .map(|(name, member)| (name, with_sorted_members(member)))
                .collect(),
        ),
        ::serde_json::Value::Array(items) => {
            ::serde_json::Value::Array(items.into_iter().map(with_sorted_members).collect())
        }
        scalar => scalar,
    }
}

/// A resource of this service an ARN names: what the tagging operations' `resourceArn`
/// addresses, which the model documents as "registries and registry records".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResource {
    /// `arn:<partition>:agent-registry:<region>:<account>:registry/<registryId>`.
    Registry { registry_id: String },
    /// A record, whose ARN the model nests under its registry's.
    Record {
        registry_id: String,
        record_id: String,
    },
}

/// The resource of this service `arn` names, or `None` when `arn` is not the ARN of one.
///
/// Recognised exactly where the model's two ARN shapes are: `RegistryArn`
/// (`^arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$`)
/// and `RegistryRecordArn` (the same, continued with `/record/[a-zA-Z0-9]{12}`). The
/// `ResourceArn` the tagging operations bind declares no pattern at all — any string of
/// 1..1011 characters reaches the handler — so this is the one place an ARN of another
/// vendor, of a resource type this service does not hold, or with an identifier outside
/// those classes is told apart from a resource it could hold. Splitting the value into its
/// ARN fields is `service-common`'s single ARN parse.
pub fn taggable_resource(arn: &str) -> Option<TaggableResource> {
    let parsed = ::service_common::arn::parse(arn)?;
    let partitioned = parsed.partition == "aws" || parsed.partition.starts_with("aws-");
    let regional = !parsed.region.is_empty()
        && parsed
            .region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let accounted =
        parsed.account.len() == 12 && parsed.account.bytes().all(|byte| byte.is_ascii_digit());
    if parsed.vendor != ARN_SERVICE || !partitioned || !regional || !accounted {
        return None;
    }
    let segments: Vec<&str> = parsed.resource.split('/').collect();
    match segments.as_slice() {
        ["registry", registry_id] if is_registry_id(registry_id) => {
            Some(TaggableResource::Registry {
                registry_id: (*registry_id).to_string(),
            })
        }
        ["registry", registry_id, "record", record_id]
            if is_registry_id(registry_id) && is_record_id(record_id) =>
        {
            Some(TaggableResource::Record {
                registry_id: (*registry_id).to_string(),
                record_id: (*record_id).to_string(),
            })
        }
        _ => None,
    }
}

/// Whether `id` is a `RegistryId`: `^[a-zA-Z0-9]{12,16}$`.
fn is_registry_id(id: &str) -> bool {
    (12..=16).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `id` is a `RegistryRecordId`: `^[a-zA-Z0-9]{12}$`.
fn is_record_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

impl TaggableResource {
    /// How this service states that the resource an ARN named is not there: the same two
    /// sentences every other operation of this service states that error class in — a
    /// registry by the identifier the request supplied (here its ARN), a record by its id.
    pub fn not_found_message(&self, resource_arn: &str) -> String {
        match self {
            Self::Registry { .. } => registry_not_found_message(resource_arn),
            Self::Record { record_id, .. } => record_not_found_message(record_id),
        }
    }
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
