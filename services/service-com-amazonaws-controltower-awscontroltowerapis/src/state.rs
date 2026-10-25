// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-controltower-awscontroltowerapis service.
//!
//! The landing zone is Control Tower's primary resource and AWS allows exactly ONE per
//! account, so [`ComAmazonawsControltowerAwscontroltowerapisState::landing_zones`] holds
//! at most one entry and the one-per-account rule is read off its size rather than from a
//! separate flag.
//!
//! Landing zone operations are a log keyed by `operationIdentifier`: a record outlives
//! the landing zone it acted on, so `GetLandingZoneOperation` still resolves the
//! identifier a `DeleteLandingZone` returned.
//!
//! Baselines work the same way one level down: an `EnabledBaseline` is a baseline applied
//! to a target, and `EnableBaseline`/`UpdateEnabledBaseline`/`DisableBaseline` each start
//! an asynchronous operation whose record lands in a SECOND log keyed by `operationIdentifier`
//! ([`ComAmazonawsControltowerAwscontroltowerapisState::baseline_operations`]), which
//! `GetBaselineOperation` resolves. The two logs are separate: an identifier a landing
//! zone call returned is not a baseline operation and vice versa.
//!
//! Control enablement keeps a log of its own, the same shape one level down again: a call
//! that enables, updates, resets or disables a control starts an asynchronous operation
//! whose record lands in
//! [`ComAmazonawsControltowerAwscontroltowerapisState::control_operations`], keyed by
//! `operationIdentifier` and resolved by `GetControlOperation`/`ListControlOperations`.
//! It is a THIRD separate log: an identifier a landing zone or baseline call returned is
//! not a control operation, and vice versa.
//!
//! The baseline CATALOG is a third kind of data: the baselines themselves are AWS-managed
//! and identical in every account, so they live in the constant [`BASELINE_CATALOG`] that
//! `GetBaseline` resolves an identifier against, not in the per-account store. No call
//! creates, changes or deletes a baseline, so nothing a caller does can grow it.
//!
//! CONTROLS are catalog data of exactly that kind: a control is AWS-managed and identical
//! in every account, so the controls a caller may name live in the constant
//! [`CONTROL_CATALOG`] that `EnableControl` resolves a `controlIdentifier` against, and no
//! call creates, changes or deletes one either.
//!
//! RETENTION: every store a caller can grow by repeating calls is BOUNDED, so ordinary
//! request traffic cannot grow this process's heap, or the per-request scan [`settle`]
//! runs under the state lock, without limit:
//!
//! * `landing_zones` holds the one landing zone an account may have;
//! * the three operation logs are append-only only WITHIN a bound, retaining at most
//!   [`MAX_LANDING_ZONE_OPERATIONS`] / [`MAX_BASELINE_OPERATIONS`] /
//!   [`MAX_CONTROL_OPERATIONS`] records and evicting the oldest TERMINAL record to make
//!   room (see [`enforce_log_bound`]);
//! * `enabled_baselines` holds at most [`MAX_ENABLED_BASELINES`] enablements, and
//!   `EnableBaseline` is REFUSED past that cap rather than growing the store (see
//!   [`record_enablement`]), because an enablement is a resource a caller must tear down
//!   rather than a history record a newer one may evict;
//! * `enabled_controls` holds at most [`MAX_ENABLED_CONTROLS`] control enablements — one
//!   per control/target pair `EnableControl` is called for — and `EnableControl` is
//!   REFUSED past that cap for the same reason as `EnableBaseline`;
//! * `tags` is keyed by the ARNs of the stores above, and an entry is dropped with the
//!   resource it tags, so it is bounded by them; one resource's tag map holds at most
//!   [`MAX_TAGS_PER_RESOURCE`] entries and `TagResource` is REFUSED past that count (see
//!   [`merge_tags`]), so repeating it with fresh keys cannot grow the store either.
//!
//! [`merge_tags`]: ComAmazonawsControltowerAwscontroltowerapisState::merge_tags
//!
//! [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
//! [`record_enablement`]: ComAmazonawsControltowerAwscontroltowerapisState::record_enablement

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, from the
/// model's `aws.api#service` trait (`arnNamespace: controltower`).
pub const ARN_NAMESPACE: &str = "controltower";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long an asynchronous operation stays non-terminal before the clock owes it a
/// terminal status — a landing zone or baseline operation its `IN_PROGRESS`, a control
/// enablement its `UNDER_CHANGE`.
///
/// `CreateLandingZone`/`DeleteLandingZone` "start an asynchronous operation" and
/// `EnableControl` leaves the control `UNDER_CHANGE` ("the EnabledControl configuration
/// is changing", model docs), so a caller must be able to observe the transient status.
/// AWS takes tens of minutes, which no emulator can usefully reproduce and which AWS
/// does not document as a contract, so this span is CHOSEN — kept short enough that a
/// test can wait for it and long enough that the call that starts the operation always
/// returns before it elapses.
pub const OPERATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most landing zone operation records the log retains.
///
/// A record stays resolvable through `GetLandingZoneOperation` until the log is full, at
/// which point the OLDEST terminal record is evicted to make room. Without this cap a
/// caller repeating create/delete would grow the store, and the `settle` scan held under
/// the state lock, without limit. AWS documents no retention for this log, so the value
/// is CHOSEN: large enough that no lifecycle an emulator client drives reaches it, small
/// enough to bound the store.
pub const MAX_LANDING_ZONE_OPERATIONS: usize = 1024;

/// The most enabled controls this account's store holds.
///
/// `EnableControl` is the one call a caller can repeat to grow the enabled-control map
/// (each distinct control/target pair is a new entry), so the map is bounded and the
/// call past the bound is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402) rather than growing the store. AWS publishes per-OU control quotas but
/// not one number for an account, so this value is CHOSEN: far above any lifecycle an
/// emulator client drives, small enough to bound the store and the per-request scan
/// `ListEnabledControls` runs under the state lock.
pub const MAX_ENABLED_CONTROLS: usize = 1024;

/// The most bytes one `EnableControl` request's `parameters` may weigh, keys and
/// documents together.
///
/// The declared-key check ([`unacceptable_control_parameters_message`]) already bounds the
/// parameter COUNT of an enablement — a control declares a handful of keys and each may be
/// given once — but an `EnabledControlParameter$value` is an unconstrained `Document`, so
/// one parameter can still carry a whole body. The weight is measured with
/// [`document_weight_within`], which stops as soon as this cap is exceeded, and a request
/// above it is refused with the modelled `ValidationException`. CHOSEN: orders of
/// magnitude above any real control configuration, and small enough that
/// [`MAX_ENABLED_CONTROLS`] records bound the store's size in bytes as well as in entries.
pub const MAX_ENABLED_CONTROL_PARAMETER_BYTES: usize = 64 * 1024;

/// The most control operation records the log retains.
///
/// Same reasoning, and the same CHOSEN size, as [`MAX_LANDING_ZONE_OPERATIONS`]: a record
/// stays resolvable through `GetControlOperation`/`ListControlOperations` until the log is
/// full, at which point the oldest terminal record is evicted. Counted separately because
/// the three logs are separate stores, so a busy control caller cannot evict landing zone
/// or baseline history.
pub const MAX_CONTROL_OPERATIONS: usize = 1024;

/// The most baseline operation records the log retains.
///
/// Same reasoning, and the same CHOSEN size, as [`MAX_LANDING_ZONE_OPERATIONS`]: a record
/// stays resolvable through `GetBaselineOperation` until the log is full, at which point
/// the oldest terminal record is evicted. Counted separately because the two logs are
/// separate stores, so a busy baseline caller cannot evict landing zone history.
pub const MAX_BASELINE_OPERATIONS: usize = 1024;

/// The most `EnabledBaseline` resources this account may hold at once.
///
/// Unlike an operation log, this store holds RESOURCES: a caller tears an enablement down
/// with `DisableBaseline`, so nothing here may be evicted to make room for a newer one
/// without losing a resource its owner still has. `EnableBaseline` is therefore refused
/// past this cap with the `ServiceQuotaExceededException` the model declares for it ("The
/// request would cause a service quota to be exceeded"), which both keeps the store
/// — and the per-request [`settle`] scan — bounded and is the answer AWS gives when an
/// account is at its limit. AWS publishes its own Control Tower quotas; this value is
/// CHOSEN: far above any lifecycle an emulator client drives, small enough to bound the
/// store.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
pub const MAX_ENABLED_BASELINES: usize = 512;

/// The most bytes one baseline enablement's `parameters` may weigh, keys and documents
/// together.
///
/// The declared-key check below already bounds the parameter COUNT of an enablement (a
/// baseline declares a handful of keys and each may be given once), but an
/// `EnabledBaselineParameter$value` is an unconstrained `Document`, so one parameter can
/// still carry a whole body. The weight is measured with [`document_weight_within`],
/// which stops as soon as this cap is exceeded, and a request above it is refused with
/// the modelled `ValidationException`. CHOSEN, for the same reason and at the same size
/// as [`MAX_ENABLED_CONTROL_PARAMETER_BYTES`]: orders of magnitude above any real
/// baseline configuration, and small enough that [`MAX_ENABLED_BASELINES`] records bound
/// the store's size in bytes as well as in entries.
pub const MAX_ENABLED_BASELINE_PARAMETER_BYTES: usize = 64 * 1024;

/// The landing zone versions this service OFFERS, oldest first.
///
/// `LandingZoneVersion` (len 3..10, pattern `^\d+.\d+$`) bounds the SHAPE of a version
/// string, not the set of versions that exist, and the member's docs give one by example
/// ("The landing zone version, for example, 3.0"): a landing zone is created or updated
/// at a version the service publishes, so a value outside this set is refused with the
/// modelled `ValidationException` rather than minted. The set is the 2.x/3.x line the AWS
/// Control Tower user guide's "Landing zone versions" table publishes — 2.0 through 3.3.
///
/// Every entry satisfies `LandingZoneVersion`, and the list is kept in NUMERIC order so
/// its last entry is [`CATALOG_LANDING_ZONE_VERSION`].
pub const CATALOG_LANDING_ZONE_VERSIONS: &[&str] = &[
    "2.0", "2.1", "2.2", "2.3", "2.4", "2.5", "2.6", "2.7", "2.8", "2.9", "3.0", "3.1", "3.2",
    "3.3",
];

/// The most tags one resource may carry.
///
/// This is the model's OWN ceiling rather than a chosen one: `TagMap` carries
/// `@length(min: 0, max: 200)`, and `ListTagsForResource` reports a resource's tags as a
/// `TagMap`, so a resource holding more than 200 tags could not be reported inside the
/// modelled constraint. `TagResource` is therefore refused with the `ValidationException`
/// the model declares for it once the merged set would pass this count, which also bounds
/// the tag store: a caller cannot grow one resource's tag map by repeating calls with
/// fresh keys.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The newest landing zone version this emulator's catalog offers.
///
/// Reported through `LandingZoneDetail$latestAvailableVersion` ("The latest available
/// version of the landing zone"). Satisfies `LandingZoneVersion` (len 3..10, pattern
/// `^\d+.\d+$`), and is the numerically newest entry of [`CATALOG_LANDING_ZONE_VERSIONS`].
pub const CATALOG_LANDING_ZONE_VERSION: &str = "3.3";

/// Whether this service offers `version` as a landing zone version.
///
/// The membership test behind every `version` refusal: a landing zone can only be created
/// or updated at a version the catalog publishes. Private because a handler refuses with
/// [`unsupported_version_message`], which is this test plus the wording.
fn offers_landing_zone_version(version: &str) -> bool {
    CATALOG_LANDING_ZONE_VERSIONS.contains(&version)
}

/// Why `version` cannot be deployed, or `None` when the catalog offers it.
///
/// The one place the refusal's wording is built, so `CreateLandingZone` and
/// `UpdateLandingZone` refuse an unoffered version in the same words. The message is
/// built from the request and from the catalog, so it tells the caller which versions it
/// may ask for.
pub fn unsupported_version_message(version: &str) -> Option<String> {
    if offers_landing_zone_version(version) {
        return None;
    }
    Some(format!(
        "version {version:?} is not a landing zone version this service offers; the \
         available versions are {}",
        CATALOG_LANDING_ZONE_VERSIONS.join(", ")
    ))
}

/// What one documented `Manifest` member's value must be.
///
/// The manifest's full schema "is not published, by design"
/// (`UpdateLandingZoneInput$manifest`), so a member is constrained only as far as the
/// user guide's example manifest documents it ("Launch your landing zone"): its JSON
/// type, and for an account id the 12-digit form every AWS account id has. Anything the
/// manifest carries beyond these members is accepted and stored unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestMemberKind {
    /// A JSON object, whose own members are not constrained further.
    Object,
    /// A JSON array of strings — AWS Region names. May be empty: a manifest that governs
    /// no region leaves the governed-region list empty rather than omitting it.
    StringList,
    /// A 12-digit Amazon Web Services account id, as a JSON string.
    AccountId,
    /// A JSON boolean.
    Boolean,
    /// A JSON object carrying a `name` string: an organizational unit's name.
    NamedOrganizationalUnit,
}

impl ManifestMemberKind {
    /// Whether `value` is of this kind.
    pub fn accepts(self, value: &::aws_smithy_types::Document) -> bool {
        use ::aws_smithy_types::Document;

        match self {
            Self::Object => matches!(value, Document::Object(_)),
            Self::StringList => match value {
                Document::Array(items) => {
                    items.iter().all(|item| matches!(item, Document::String(_)))
                }
                _ => false,
            },
            Self::AccountId => match value {
                Document::String(account_id) => {
                    account_id.len() == 12 && account_id.bytes().all(|b| b.is_ascii_digit())
                }
                _ => false,
            },
            Self::Boolean => matches!(value, Document::Bool(_)),
            Self::NamedOrganizationalUnit => match value {
                Document::Object(members) => {
                    matches!(members.get("name"), Some(Document::String(_)))
                }
                _ => false,
            },
        }
    }

    /// What this kind requires, for the refusal's message.
    pub fn requirement(self) -> &'static str {
        match self {
            Self::Object => "a JSON object",
            Self::StringList => "a JSON array of Amazon Web Services Region names",
            Self::AccountId => "a 12-digit Amazon Web Services account id, as a string",
            Self::Boolean => "a JSON boolean",
            Self::NamedOrganizationalUnit => {
                "a JSON object carrying the organizational unit's `name` as a string"
            }
        }
    }
}

/// The members a landing zone `Manifest` must declare, as dotted paths into the document.
///
/// These are the required parameters of the example manifest the user guide publishes for
/// `CreateLandingZone`/`UpdateLandingZone` ("Launch your landing zone"): the governed
/// Regions, the organization structure's security OU, the centralized logging account and
/// its configuration, the security roles account, and whether access management is
/// enabled. A create or update whose manifest omits one of them describes no landing zone
/// the service could deploy, so it is refused with the modelled `ValidationException`.
///
/// A parent path precedes its children, so a member whose container is of the wrong type
/// is reported against the CONTAINER rather than as a missing child.
///
/// The nested entries (`centralizedLogging`'s account, configuration and flag,
/// `securityRoles`' account, `accessManagement`'s flag) are here because the user guide
/// lists them among the manifest's REQUIRED parameters, not merely because its example
/// happens to carry them — a section present but hollow still describes nothing the
/// service could deploy. Only these paths are checked, and only for their JSON kind:
/// everything else the manifest carries is accepted and stored unchanged, because the
/// schema as a whole is unpublished.
pub const REQUIRED_MANIFEST_MEMBERS: &[(&str, ManifestMemberKind)] = &[
    ("governedRegions", ManifestMemberKind::StringList),
    ("organizationStructure", ManifestMemberKind::Object),
    (
        "organizationStructure.security",
        ManifestMemberKind::NamedOrganizationalUnit,
    ),
    ("centralizedLogging", ManifestMemberKind::Object),
    ("centralizedLogging.accountId", ManifestMemberKind::AccountId),
    (
        "centralizedLogging.configurations",
        ManifestMemberKind::Object,
    ),
    ("centralizedLogging.enabled", ManifestMemberKind::Boolean),
    ("securityRoles", ManifestMemberKind::Object),
    ("securityRoles.accountId", ManifestMemberKind::AccountId),
    ("accessManagement", ManifestMemberKind::Object),
    ("accessManagement.enabled", ManifestMemberKind::Boolean),
];

/// Manifest members the documented example carries OPTIONALLY: absent is fine, but a
/// value of the wrong type is not.
///
/// The sandbox OU is optional in the example manifest (a landing zone may be launched
/// without one), unlike the security OU.
pub const OPTIONAL_MANIFEST_MEMBERS: &[(&str, ManifestMemberKind)] = &[(
    "organizationStructure.sandbox",
    ManifestMemberKind::NamedOrganizationalUnit,
)];

/// The member `path` names, resolved through nested objects, or `None` when any step of
/// the path is absent or is not an object.
fn manifest_member<'a>(
    manifest: &'a ::aws_smithy_types::Document,
    path: &str,
) -> Option<&'a ::aws_smithy_types::Document> {
    use ::aws_smithy_types::Document;

    let mut node = manifest;
    for segment in path.split('.') {
        let Document::Object(members) = node else {
            return None;
        };
        node = members.get(segment)?;
    }
    Some(node)
}

/// The JSON kind of `document`, for a refusal's message.
fn document_kind(document: &::aws_smithy_types::Document) -> &'static str {
    use ::aws_smithy_types::Document;

    match document {
        Document::Object(_) => "an object",
        Document::Array(_) => "an array",
        Document::String(_) => "a string",
        Document::Number(_) => "a number",
        Document::Bool(_) => "a boolean",
        Document::Null => "null",
    }
}

/// The first way `manifest` fails to describe a landing zone configuration, or `None`
/// when it describes one.
///
/// The message names the member and the constraint it failed, so a caller is told what to
/// change; it is built from the request, never from a recorded example.
pub fn manifest_defect(manifest: &::aws_smithy_types::Document) -> Option<String> {
    use ::aws_smithy_types::Document;

    if !matches!(manifest, Document::Object(_)) {
        return Some(format!(
            "manifest must be a JSON object describing the landing zone configuration, got {}",
            document_kind(manifest)
        ));
    }

    for (path, kind) in REQUIRED_MANIFEST_MEMBERS {
        match manifest_member(manifest, path) {
            None => {
                return Some(format!(
                    "manifest is missing the required member {path:?}, which must be {}",
                    kind.requirement()
                ))
            }
            Some(value) if !kind.accepts(value) => {
                return Some(format!(
                    "manifest member {path:?} must be {}, got {}",
                    kind.requirement(),
                    document_kind(value)
                ))
            }
            Some(_) => {}
        }
    }

    for (path, kind) in OPTIONAL_MANIFEST_MEMBERS {
        if let Some(value) = manifest_member(manifest, path) {
            if !kind.accepts(value) {
                return Some(format!(
                    "manifest member {path:?} must be {}, got {}",
                    kind.requirement(),
                    document_kind(value)
                ));
            }
        }
    }

    None
}

/// `LandingZoneStatus`: the landing zone's deployment status.
///
/// Wire values from the model's `LandingZoneStatus` enum
/// (`ACTIVE` | `PROCESSING` | `FAILED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneStatus {
    Active,
    Processing,
    Failed,
}

impl LandingZoneStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Processing => "PROCESSING",
            Self::Failed => "FAILED",
        }
    }
}

/// `LandingZoneOperationType`: which call produced an operation record.
///
/// Wire values from the model's `LandingZoneOperationType` enum
/// (`CREATE` | `DELETE` | `RESET` | `UPDATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneOperationType {
    Create,
    Delete,
    Reset,
    Update,
}

impl LandingZoneOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Delete => "DELETE",
            Self::Reset => "RESET",
            Self::Update => "UPDATE",
        }
    }
}

/// `LandingZoneOperationStatus`: how far an operation has got.
///
/// Wire values from the model's `LandingZoneOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl LandingZoneOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// `LandingZoneDriftStatus`: whether a landing zone still matches its manifest.
///
/// Wire values from the model's `LandingZoneDriftStatus` enum, reported through
/// `LandingZoneDriftStatusSummary$status` (`DRIFTED` | `IN_SYNC`). Typed like the other
/// three modelled enums so the store cannot hold a drift value the model does not
/// declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneDriftStatus {
    Drifted,
    InSync,
}

impl LandingZoneDriftStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Drifted => "DRIFTED",
            Self::InSync => "IN_SYNC",
        }
    }
}

/// A landing zone configuration an asynchronous operation has started deploying but the
/// clock has not yet made due.
///
/// `UpdateLandingZone` "starts an asynchronous operation that updates the landing zone"
/// (model docs) and `LandingZoneDetail$version` is "the landing zone's CURRENT DEPLOYED
/// version", so the configuration a caller asked for is NOT what a reporting call reports
/// while that operation is in flight: it is held here and deployed by [`settle`] at
/// `due_at`, the same instant the operation record reaches its terminal status.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
#[derive(Debug, Clone)]
pub struct PendingLandingZoneConfiguration {
    pub version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    /// The drift status the deployed landing zone will carry once this configuration is
    /// deployed.
    pub drift_status: Option<LandingZoneDriftStatus>,
    /// The instant the clock owes this deployment. Stored, never slept on: a reader
    /// materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// `DriftStatus`: whether an enabled control still matches the configuration Control
/// Tower deployed.
///
/// Wire values from the model's `DriftStatus` enum, reported through
/// `DriftStatusSummary$driftStatus` and the two `EnabledControlDriftTypes` members
/// (`DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftStatus {
    Drifted,
    InSync,
    NotChecking,
    Unknown,
}

impl DriftStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Drifted => "DRIFTED",
            Self::InSync => "IN_SYNC",
            Self::NotChecking => "NOT_CHECKING",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// A persisted enabled control: one control applied to one target.
///
/// Everything `ListEnabledControls` and `GetEnabledControl` report about a control is
/// read from here, so the two agree by construction. `parameters` keeps the caller's
/// `EnabledControlParameter` documents in the order they were supplied, and
/// `target_regions` the regions the control was deployed into at enablement time.
#[derive(Debug, Clone)]
pub struct EnabledControl {
    pub arn: String,
    pub control_identifier: String,
    pub target_identifier: String,
    /// `EnabledControlSummary$parentIdentifier` — "the ARN of the parent enabled control
    /// from which this control inherits its configuration, if applicable". A control
    /// `EnableControl` applied directly to its target inherits from nothing, and this
    /// service stores no organizational hierarchy to derive inheritance from, so it is
    /// `None` for every control this emulator enables.
    pub parent_identifier: Option<String>,
    /// `EnabledControlParameter`s as supplied, in order.
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
    /// The regions this control was deployed into: the landing zone's governed regions
    /// when one is deployed, else the region the enabling request was signed for.
    pub target_regions: Vec<String>,
    pub status: EnablementStatus,
    /// `EnablementStatusSummary$lastOperationIdentifier`: the identifier of the most
    /// recent control operation against this control.
    pub last_operation_identifier: String,
    pub drift_status: DriftStatus,
    /// `EnabledControlDriftTypes$inheritance`.
    pub inheritance_drift_status: DriftStatus,
    /// `EnabledControlDriftTypes$resource`.
    pub resource_drift_status: DriftStatus,
    /// The instant the clock owes this control its terminal enablement status. Stored,
    /// never slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
    /// The configuration an `UPDATE_ENABLED_CONTROL` still in flight will apply, or
    /// `None` when nothing is pending. [`settle`] applies it once that operation is due,
    /// so the configured parameters change when the asynchronous update completes and not
    /// when the call that asked for it returned.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_update: Option<PendingEnabledControlUpdate>,
}

/// A configuration `UpdateEnabledControl` accepted and the clock has yet to apply.
///
/// `UpdateEnabledControl` "updates the configuration of an already enabled control"
/// through an asynchronous operation, so the request's parameters are parked here and
/// become the control's configured ones once the clock has made that operation due.
#[derive(Debug, Clone)]
pub struct PendingEnabledControlUpdate {
    /// The `UPDATE_ENABLED_CONTROL` operation whose completion applies this change.
    pub operation_identifier: String,
    /// The instant the clock owes this change its application — the same instant that
    /// operation becomes due, held here so applying it never depends on the operation's
    /// record still being in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
    /// The configured parameters the update replaces the control's with. Required on the
    /// request, so an accepted update always carries a (possibly empty) list.
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
}

/// A persisted landing zone. Everything `GetLandingZone` reports is read from here.
///
/// `manifest` keeps the caller's `Manifest` document EXACTLY as it was deserialized
/// (no re-encoding through another representation), so what `GetLandingZone` returns is
/// the document `CreateLandingZone` was given.
#[derive(Debug, Clone)]
pub struct LandingZone {
    pub arn: String,
    pub version: String,
    pub latest_available_version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    pub status: LandingZoneStatus,
    /// `LandingZoneDriftStatusSummary$status`, absent when the service has not assessed
    /// drift for this landing zone.
    pub drift_status: Option<LandingZoneDriftStatus>,
    /// The configuration an in-flight `UPDATE` will deploy, `None` when the deployed
    /// configuration is the only one. Deployed by [`settle`] once the clock makes it due.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_configuration: Option<PendingLandingZoneConfiguration>,
    pub created_at: SystemTime,
}

/// One entry of the landing zone operation log.
#[derive(Debug, Clone)]
pub struct LandingZoneOperation {
    pub operation_identifier: String,
    pub operation_type: LandingZoneOperationType,
    /// The landing zone this operation acted on. Kept even after the landing zone is
    /// deleted, so a `DELETE` identifier still resolves.
    pub landing_zone_arn: String,
    pub status: LandingZoneOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// `BaselineOperationType`: which call produced a baseline operation record.
///
/// Wire values from the model's `BaselineOperationType` enum (`ENABLE_BASELINE` |
/// `DISABLE_BASELINE` | `UPDATE_ENABLED_BASELINE` | `RESET_ENABLED_BASELINE`), which is
/// exactly the set of calls `GetBaselineOperation` documents as starting one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineOperationType {
    EnableBaseline,
    DisableBaseline,
    UpdateEnabledBaseline,
    ResetEnabledBaseline,
}

impl BaselineOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnableBaseline => "ENABLE_BASELINE",
            Self::DisableBaseline => "DISABLE_BASELINE",
            Self::UpdateEnabledBaseline => "UPDATE_ENABLED_BASELINE",
            Self::ResetEnabledBaseline => "RESET_ENABLED_BASELINE",
        }
    }
}

/// `BaselineOperationStatus`: how far a baseline operation has got.
///
/// Wire values from the model's `BaselineOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl BaselineOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// `EnablementStatus`: the deployment status of an enabled resource.
///
/// Wire values from the model's `EnablementStatus` enum, reported through
/// `EnablementStatusSummary$status` (`SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, where
/// `UNDER_CHANGE` is "the configuration is changing").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnablementStatus {
    Succeeded,
    Failed,
    UnderChange,
}

impl EnablementStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::UnderChange => "UNDER_CHANGE",
        }
    }
}

/// A persisted `EnabledBaseline`: one baseline applied to one target.
///
/// `parameters` keeps the caller's `EnabledBaselineParameters` as key/`Document` pairs in
/// the order supplied, so the enablement parameters a later reporting call reports are the
/// ones the last call that applied them — `EnableBaseline`, or an `UpdateEnabledBaseline`
/// the clock has made due — was given.
#[derive(Debug, Clone)]
pub struct EnabledBaseline {
    pub arn: String,
    pub baseline_identifier: String,
    pub baseline_version: String,
    pub target_identifier: String,
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
    /// `EnablementStatusSummary$status`, derived from the log by [`settle`].
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub status: EnablementStatus,
    /// `EnablementStatusSummary$lastOperationIdentifier`: the most recent operation
    /// started against this resource.
    pub last_operation_identifier: String,
    pub created_at: SystemTime,
    /// The change an `UPDATE_ENABLED_BASELINE` still in flight will apply, or `None`
    /// when nothing is pending. `settle` applies it once that operation is due, so the
    /// applied version and parameters change when the asynchronous update completes and
    /// not when the call that asked for it returned.
    pub pending_update: Option<PendingEnabledBaselineUpdate>,
}

/// A change `UpdateEnabledBaseline` accepted and the clock has yet to apply.
///
/// `UpdateEnabledBaseline` "updates an `EnabledBaseline` resource's applied parameters or
/// version" through an asynchronous operation, so the request's values are parked here
/// and become the resource's once the clock has made that operation due.
#[derive(Debug, Clone)]
pub struct PendingEnabledBaselineUpdate {
    /// The `UPDATE_ENABLED_BASELINE` operation whose completion applies this change.
    pub operation_identifier: String,
    /// The instant the clock owes this change its application — the same instant that
    /// operation becomes due, held here so applying it never depends on the operation's
    /// record still being in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
    /// The `baselineVersion` the update moves the enablement to. Required on the
    /// request, so always present.
    pub baseline_version: String,
    /// The applied parameters the update replaces the enablement's with, or `None` when
    /// the request carried no `parameters` member at all — the applied parameters then
    /// stay as they are, while a present (even empty) list is the new applied set.
    pub parameters: Option<Vec<(String, ::aws_smithy_types::Document)>>,
}

/// One entry of the baseline operation log.
#[derive(Debug, Clone)]
pub struct BaselineOperationRecord {
    pub operation_identifier: String,
    pub operation_type: BaselineOperationType,
    /// The `EnabledBaseline` this operation acted on. Kept even after that resource is
    /// gone, so a `DISABLE_BASELINE` identifier still resolves.
    pub enabled_baseline_arn: String,
    pub status: BaselineOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// `ControlOperationType`: which call produced a control operation record.
///
/// Wire values from the model's `ControlOperationType` enum (`ENABLE_CONTROL` |
/// `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`), which is
/// exactly the set of calls that act on an `EnabledControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOperationType {
    EnableControl,
    DisableControl,
    UpdateEnabledControl,
    ResetEnabledControl,
}

impl ControlOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnableControl => "ENABLE_CONTROL",
            Self::DisableControl => "DISABLE_CONTROL",
            Self::UpdateEnabledControl => "UPDATE_ENABLED_CONTROL",
            Self::ResetEnabledControl => "RESET_ENABLED_CONTROL",
        }
    }
}

/// `ControlOperationStatus`: how far a control operation has got.
///
/// Wire values from the model's `ControlOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl ControlOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// One entry of the control operation log.
#[derive(Debug, Clone)]
pub struct ControlOperationRecord {
    pub operation_identifier: String,
    pub operation_type: ControlOperationType,
    /// The `controlIdentifier` the call named, carried on the record so a reporting call
    /// reports the control the operation acted on without the enablement it belongs to
    /// still being in the store.
    pub control_identifier: String,
    /// The `targetIdentifier` the call named, carried for the same reason.
    pub target_identifier: String,
    /// The `EnabledControl` this operation acted on. Kept even after that resource is
    /// gone, so a `DISABLE_CONTROL` identifier still resolves.
    pub enabled_control_arn: String,
    pub status: ControlOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// Per-service state for com-amazonaws-controltower-awscontroltowerapis. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsControltowerAwscontroltowerapisState {
    /// Landing zones keyed by ARN. At most one entry: AWS allows one landing zone per
    /// account, and that rule is enforced from this map's size.
    pub landing_zones: HashMap<String, LandingZone>,
    /// The operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_LANDING_ZONE_OPERATIONS`] records; see the module header on retention.
    pub landing_zone_operations: HashMap<String, LandingZoneOperation>,
    /// Enabled controls keyed by their own ARN — the single home for this service's
    /// control enablements, written by `EnableControl` and read by
    /// `ListEnabledControls`/`GetEnabledControl`. Holds at most
    /// [`MAX_ENABLED_CONTROLS`] entries.
    pub enabled_controls: HashMap<String, EnabledControl>,
    /// The control operation log, keyed by `operationIdentifier`. Written by every call
    /// that acts on an `EnabledControl` and read by
    /// `GetControlOperation`/`ListControlOperations`. Append-only up to
    /// [`MAX_CONTROL_OPERATIONS`] records; see the module header on retention.
    pub control_operations: HashMap<String, ControlOperationRecord>,
    /// Enabled baselines keyed by the `EnabledBaseline` ARN this service minted for them.
    pub enabled_baselines: HashMap<String, EnabledBaseline>,
    /// The baseline operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_BASELINE_OPERATIONS`] records; see the module header on retention.
    pub baseline_operations: HashMap<String, BaselineOperationRecord>,
    /// Tags keyed by the tagged resource's ARN — the single home for this service's
    /// tags, written by `CreateLandingZone`'s, `EnableControl`'s and `EnableBaseline`'s
    /// `tags` members and by `TagResource`, and read by `ListTagsForResource`.
    pub tags: HashMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsControltowerAwscontroltowerapisState {
    /// Materialise every transition the CLOCK has already made due, and nothing else.
    ///
    /// An operation whose `due_at` has passed is terminal whether or not anybody called,
    /// so settling it here is not a read-triggered mutation; a landing zone's status is
    /// then derived from whether any operation against it is still in flight, and a
    /// configuration an `UPDATE` queued is deployed once its own `due_at` has passed.
    /// Call this first in every handler so reporting and mutating operations agree on the
    /// clock.
    pub fn settle(&mut self, now: SystemTime) {
        for op in self.landing_zone_operations.values_mut() {
            if op.status == LandingZoneOperationStatus::InProgress && now >= op.due_at {
                op.status = LandingZoneOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                op.end_time = Some(op.due_at);
            }
        }

        // A configuration an UPDATE started deploying becomes the deployed one once the
        // clock has made that deployment due — the same instant the operation record
        // reaches its terminal status — whether or not anybody called. Until then a
        // reporting call reports what is actually deployed.
        for landing_zone in self.landing_zones.values_mut() {
            let due = landing_zone
                .pending_configuration
                .take_if(|pending| now >= pending.due_at);
            if let Some(pending) = due {
                landing_zone.version = pending.version;
                landing_zone.latest_available_version =
                    latest_available_version(&landing_zone.version, CATALOG_LANDING_ZONE_VERSION);
                landing_zone.manifest = pending.manifest;
                landing_zone.remediation_types = pending.remediation_types;
                landing_zone.drift_status = pending.drift_status;
            }
        }

        let operations = &self.landing_zone_operations;
        for (arn, landing_zone) in self.landing_zones.iter_mut() {
            // A FAILED landing zone is not re-derived from the log. Nothing in this
            // slice can set `Failed` yet, so the branch is unreachable today; it is here
            // so the later UpdateLandingZone/ResetLandingZone slice, which can fail a
            // landing zone, does not have its status silently overwritten by a reader.
            if landing_zone.status == LandingZoneStatus::Failed {
                continue;
            }
            let in_flight = any_operation_in_flight(operations, arn);
            landing_zone.status = if in_flight {
                LandingZoneStatus::Processing
            } else {
                LandingZoneStatus::Active
            };
        }

        // An UPDATE_ENABLED_CONTROL the clock has made due has finished applying the
        // configuration it carried, so those parameters are the control's whether or not
        // anybody called. The due instant is the pending change's OWN, not read back off
        // the operation log: `enforce_log_bound` evicts the oldest in-flight record when
        // every record is in flight, and a missing record must not be read as "due" and
        // apply a change the clock does not owe yet.
        let control_operations = &self.control_operations;
        for control in self.enabled_controls.values_mut() {
            // Not due yet leaves the pending change parked, unchanged.
            let due = control
                .pending_update
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                continue;
            };
            if control_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == ControlOperationStatus::Failed)
            {
                // A failed update applied nothing, so the control keeps the parameters it
                // had.
                continue;
            }
            control.parameters = pending.parameters;
        }

        for control in self.enabled_controls.values_mut() {
            if control.status == EnablementStatus::UnderChange && now >= control.due_at {
                control.status = EnablementStatus::Succeeded;
                // Drift is only assessable once the configuration is deployed: while the
                // control is `UNDER_CHANGE` the service "is not able to check the drift
                // status" (`UNKNOWN`), and the deployment that completes here is of the
                // configuration the service expects, so nothing has diverged from it yet.
                if control.drift_status == DriftStatus::Unknown {
                    control.drift_status = DriftStatus::InSync;
                }
                if control.inheritance_drift_status == DriftStatus::Unknown {
                    control.inheritance_drift_status = DriftStatus::InSync;
                }
                if control.resource_drift_status == DriftStatus::Unknown {
                    control.resource_drift_status = DriftStatus::InSync;
                }
            }
        }

        for operation in self.control_operations.values_mut() {
            if operation.status == ControlOperationStatus::InProgress && now >= operation.due_at {
                operation.status = ControlOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                operation.end_time = Some(operation.due_at);
            }
        }

        for operation in self.baseline_operations.values_mut() {
            if operation.status == BaselineOperationStatus::InProgress && now >= operation.due_at {
                operation.status = BaselineOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                operation.end_time = Some(operation.due_at);
            }
        }

        // A DISABLE_BASELINE that has reached its terminal status has finished tearing
        // its `EnabledBaseline` down, so the resource is gone whether or not anybody
        // called; its operation record stays, and still resolves.
        let torn_down: Vec<String> = self
            .baseline_operations
            .values()
            .filter(|operation| {
                operation.operation_type == BaselineOperationType::DisableBaseline
                    && operation.status == BaselineOperationStatus::Succeeded
            })
            .map(|operation| operation.enabled_baseline_arn.clone())
            .collect();
        for arn in torn_down {
            self.enabled_baselines.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the
            // tag map bounded by the two resource stores.
            self.tags.remove(&arn);
        }

        let baseline_operations = &self.baseline_operations;

        // An UPDATE_ENABLED_BASELINE the clock has made due has finished applying the
        // change it carried, so the new version and parameters are the resource's whether
        // or not anybody called. The due instant is the pending change's OWN, not read
        // back off the operation log: `enforce_log_bound` evicts the oldest in-flight
        // record when every record is in flight, and a missing record must not be read as
        // "due" and apply a change the clock does not owe yet.
        for enabled_baseline in self.enabled_baselines.values_mut() {
            let Some(pending) = enabled_baseline.pending_update.take() else {
                continue;
            };
            if now < pending.due_at {
                // Not due yet: park it again, unchanged.
                enabled_baseline.pending_update = Some(pending);
                continue;
            }
            if baseline_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == BaselineOperationStatus::Failed)
            {
                // A failed update applied nothing, so the enablement keeps the version
                // and parameters it had.
                continue;
            }
            enabled_baseline.baseline_version = pending.baseline_version;
            if let Some(parameters) = pending.parameters {
                enabled_baseline.parameters = parameters;
            }
        }

        for (arn, enabled_baseline) in self.enabled_baselines.iter_mut() {
            // A FAILED enablement is not re-derived from the log, for the same reason as
            // a FAILED landing zone: a reader must not overwrite a failure.
            if enabled_baseline.status == EnablementStatus::Failed {
                continue;
            }
            enabled_baseline.status = if baseline_operation_in_flight(baseline_operations, arn)
                .is_some()
            {
                EnablementStatus::UnderChange
            } else {
                EnablementStatus::Succeeded
            };
        }
    }

    /// The stored ARN a `landingZoneIdentifier` names, or `None` when no landing zone
    /// matches.
    ///
    /// A landing zone is addressed by its own ARN — the only identifier this service ever
    /// hands a caller for one — so the identifier is matched against the map key exactly.
    /// A value that is not a landing zone ARN at all is not a lookup that could miss but
    /// a malformed input; a caller checks it with
    /// [`non_landing_zone_identifier_message`] BEFORE asking this.
    pub fn resolve_landing_zone_arn(&self, identifier: &str) -> Option<String> {
        self.landing_zones
            .get_key_value(identifier)
            .map(|(arn, _)| arn.clone())
    }

    /// Whether any operation against `arn` is still `IN_PROGRESS`. Call [`settle`] first.
    ///
    /// [`settle`]: Self::settle
    pub fn has_operation_in_flight(&self, arn: &str) -> bool {
        any_operation_in_flight(&self.landing_zone_operations, arn)
    }

    /// The control already enabled for `control_identifier` on `target_identifier`, or
    /// `None` when that pair is not enabled.
    ///
    /// A control is enabled at most once per target, so this is what `EnableControl`
    /// reads to refuse a repeat. The ARN tie-break keeps the answer independent of the
    /// map's iteration order should a pair ever appear twice.
    pub fn enabled_control_on_target(
        &self,
        control_identifier: &str,
        target_identifier: &str,
    ) -> Option<&EnabledControl> {
        self.enabled_controls
            .values()
            .filter(|control| {
                control.control_identifier == control_identifier
                    && control.target_identifier == target_identifier
            })
            .min_by(|a, b| a.arn.cmp(&b.arn))
    }

    /// The regions the deployed landing zone governs, read out of its `Manifest`
    /// document's `governedRegions` list.
    ///
    /// Empty when no landing zone is deployed or its manifest carries no such list: a
    /// control enablement then falls back to the region its request was signed for. The
    /// account holds at most one landing zone; the ARN order makes the choice total
    /// anyway, so the answer never depends on the map's iteration order.
    pub fn governed_regions(&self) -> Vec<String> {
        use ::aws_smithy_types::Document;

        let Some(landing_zone) = self.landing_zones.values().min_by(|a, b| a.arn.cmp(&b.arn))
        else {
            return Vec::new();
        };
        let Document::Object(members) = &landing_zone.manifest else {
            return Vec::new();
        };
        let Some(Document::Array(regions)) = members.get("governedRegions") else {
            return Vec::new();
        };
        regions
            .iter()
            .filter_map(|region| match region {
                Document::String(name) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// The identifier of the earliest-started operation still `IN_PROGRESS` against ANY
    /// landing zone, or `None` when the account has none. Call [`settle`] first.
    ///
    /// An account holds one landing zone, so an operation in flight against any of them
    /// occupies the account — including a `DELETE`, whose landing zone is already out of
    /// the map. The `start_time`/identifier pair orders candidates totally, so the answer
    /// does not depend on the map's iteration order.
    ///
    /// [`settle`]: Self::settle
    pub fn oldest_operation_in_flight(&self) -> Option<String> {
        self.landing_zone_operations
            .values()
            .filter(|op| op.status == LandingZoneOperationStatus::InProgress)
            .min_by(|a, b| {
                (a.start_time, a.operation_identifier.as_str())
                    .cmp(&(b.start_time, b.operation_identifier.as_str()))
            })
            .map(|op| op.operation_identifier.clone())
    }

    /// Append an operation to the log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_LANDING_ZONE_OPERATIONS`]) on the way
    /// out, so this is the single place the log can grow.
    pub fn record_operation(
        &mut self,
        operation_type: LandingZoneOperationType,
        landing_zone_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.landing_zone_operations.insert(
            operation_identifier.clone(),
            LandingZoneOperation {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                landing_zone_arn: landing_zone_arn.to_string(),
                status: LandingZoneOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(
            &mut self.landing_zone_operations,
            MAX_LANDING_ZONE_OPERATIONS,
        );
        operation_identifier
    }

    /// Append a control operation to its log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_CONTROL_OPERATIONS`]) on the way out, so
    /// this is the single place that log can grow.
    pub fn record_control_operation(
        &mut self,
        operation_type: ControlOperationType,
        control_identifier: &str,
        target_identifier: &str,
        enabled_control_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.control_operations.insert(
            operation_identifier.clone(),
            ControlOperationRecord {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                control_identifier: control_identifier.to_string(),
                target_identifier: target_identifier.to_string(),
                enabled_control_arn: enabled_control_arn.to_string(),
                status: ControlOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(&mut self.control_operations, MAX_CONTROL_OPERATIONS);
        operation_identifier
    }

    /// Start an `UPDATE_ENABLED_CONTROL` against the control `arn` names, parking
    /// `parameters` for [`settle`] to apply once that operation is due, and return the
    /// operation's `operationIdentifier`.
    ///
    /// `None` when `arn` names no stored control, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it
    /// is not bounded by [`MAX_ENABLED_CONTROLS`]; the operation it appends is bounded by
    /// the log's own retention rule.
    ///
    /// The caller refuses an update that overlaps an operation still changing the control
    /// with the modelled `ConflictException`, which is what keeps the single pending slot
    /// from being overwritten while the change it holds is still owed: every accepted
    /// update is applied before another can be accepted.
    ///
    /// [`settle`]: Self::settle
    pub fn record_enabled_control_update(
        &mut self,
        arn: &str,
        parameters: Vec<(String, ::aws_smithy_types::Document)>,
        now: SystemTime,
    ) -> Option<String> {
        let (control_identifier, target_identifier) = self
            .enabled_controls
            .get(arn)
            .map(|control| {
                (
                    control.control_identifier.clone(),
                    control.target_identifier.clone(),
                )
            })?;

        let operation_identifier = self.record_control_operation(
            ControlOperationType::UpdateEnabledControl,
            &control_identifier,
            &target_identifier,
            arn,
            now,
        );
        let control = self.enabled_controls.get_mut(arn)?;
        // The control's configuration is changing until the update is due, so a caller
        // reading straight after the call sees the transient status rather than the
        // configuration it asked for.
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = now + OPERATION_SETTLE_AFTER;
        control.pending_update = Some(PendingEnabledControlUpdate {
            operation_identifier: operation_identifier.clone(),
            due_at: now + OPERATION_SETTLE_AFTER,
            parameters,
        });
        Some(operation_identifier)
    }

    /// The stored ARN an `enabledBaselineIdentifier` names, or `None` when no enabled
    /// baseline matches.
    ///
    /// Matched against the map key exactly, for the same reason
    /// [`resolve_landing_zone_arn`] is: the identifier of an enablement is the ARN this
    /// service minted for it, which is what every call reports
    /// (`EnableBaselineOutput$arn`, `EnabledBaselineSummary$arn`). Every member carrying
    /// one is typed `Arn`, so the front door refuses a value that is not an ARN before any
    /// handler runs.
    ///
    /// [`resolve_landing_zone_arn`]: Self::resolve_landing_zone_arn
    pub fn resolve_enabled_baseline_arn(&self, identifier: &str) -> Option<String> {
        self.enabled_baselines
            .get_key_value(identifier)
            .map(|(arn, _)| arn.clone())
    }

    /// The ARN of the enabled baseline that already applies `baseline_identifier` to
    /// `target_identifier`, or `None` when that pair is not enabled. Call [`settle`]
    /// first, so a pair whose DISABLE has become due reads as not enabled.
    ///
    /// [`settle`]: Self::settle
    pub fn enabled_baseline_on_target(
        &self,
        baseline_identifier: &str,
        target_identifier: &str,
    ) -> Option<String> {
        self.enabled_baselines
            .values()
            .filter(|enabled| {
                enabled.baseline_identifier == baseline_identifier
                    && enabled.target_identifier == target_identifier
            })
            // The ARN orders the candidates totally, so the answer does not depend on the
            // map's iteration order.
            .map(|enabled| enabled.arn.clone())
            .min()
    }

    /// The identifier of the earliest-started baseline operation still `IN_PROGRESS`
    /// against `arn`, or `None` when none is. Call [`settle`] first.
    ///
    /// [`settle`]: Self::settle
    pub fn baseline_operation_in_flight(&self, arn: &str) -> Option<String> {
        baseline_operation_in_flight(&self.baseline_operations, arn)
    }

    /// Append a baseline operation to its log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_BASELINE_OPERATIONS`]) on the way out,
    /// so this is the single place that log can grow.
    pub fn record_baseline_operation(
        &mut self,
        operation_type: BaselineOperationType,
        enabled_baseline_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.baseline_operations.insert(
            operation_identifier.clone(),
            BaselineOperationRecord {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                enabled_baseline_arn: enabled_baseline_arn.to_string(),
                status: BaselineOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(&mut self.baseline_operations, MAX_BASELINE_OPERATIONS);
        operation_identifier
    }

    /// Persist an enablement and start its `ENABLE_BASELINE` operation, returning that
    /// operation's `operationIdentifier`.
    ///
    /// `None` when the account already holds [`MAX_ENABLED_BASELINES`] enablements: the
    /// request is then refused and NOTHING is written — neither the resource nor an
    /// operation record. The single insertion site for `enabled_baselines`, so the cap
    /// cannot be bypassed from a handler.
    pub fn record_enablement(
        &mut self,
        arn: &str,
        baseline_identifier: &str,
        baseline_version: &str,
        target_identifier: &str,
        parameters: Vec<(String, ::aws_smithy_types::Document)>,
        now: SystemTime,
    ) -> Option<String> {
        if self.enabled_baselines.len() >= MAX_ENABLED_BASELINES {
            return None;
        }

        let operation_identifier = self.record_baseline_operation(
            BaselineOperationType::EnableBaseline,
            arn,
            now,
        );
        self.enabled_baselines.insert(
            arn.to_string(),
            EnabledBaseline {
                arn: arn.to_string(),
                baseline_identifier: baseline_identifier.to_string(),
                baseline_version: baseline_version.to_string(),
                target_identifier: target_identifier.to_string(),
                parameters,
                // The enablement is changing until its ENABLE_BASELINE operation is due;
                // the status is derived from the log by `settle`, and seeded to match.
                status: EnablementStatus::UnderChange,
                last_operation_identifier: operation_identifier.clone(),
                created_at: now,
                pending_update: None,
            },
        );
        Some(operation_identifier)
    }

    /// Whether `arn` names a resource of this service that tags attach to.
    ///
    /// The three resource stores are the whole set: a landing zone, an enabled control
    /// and an enabled baseline are the resources this service mints ARNs for, and every
    /// one of those ARNs is its store's key. An ARN outside them names no resource here,
    /// which is what the tagging operations refuse with `ResourceNotFoundException`.
    /// Call [`settle`] first, so a resource the clock has already torn down is gone.
    ///
    /// [`settle`]: Self::settle
    pub fn taggable_resource_exists(&self, arn: &str) -> bool {
        self.landing_zones.contains_key(arn)
            || self.enabled_controls.contains_key(arn)
            || self.enabled_baselines.contains_key(arn)
    }

    /// The tags attached to `arn`, in key order; empty when the resource carries none.
    ///
    /// A resource with no entry in the tag map is untagged, not missing: existence is
    /// [`taggable_resource_exists`]'s question, and a caller must be able to tell an
    /// untagged resource (an empty map) from an absent one (a 404).
    ///
    /// [`taggable_resource_exists`]: Self::taggable_resource_exists
    pub fn tags_of(&self, arn: &str) -> BTreeMap<String, String> {
        self.tags.get(arn).cloned().unwrap_or_default()
    }

    /// Merge `tags` into the tags attached to `arn`, returning the resulting tag count.
    ///
    /// A key already attached takes the new value and every other key is left alone, so
    /// applying tags adds to a resource's tag set rather than replacing it. `None` when
    /// the merged set would exceed [`MAX_TAGS_PER_RESOURCE`]: NOTHING is then written and
    /// the caller refuses the request. The single site that GROWS a resource's tag set,
    /// so that cap cannot be bypassed by repeated tagging; the create/enable operations
    /// write their own `tags` member straight into the store at a freshly minted ARN,
    /// where the front door's `TagMap` `@length` max already holds the same count.
    pub fn merge_tags<I>(&mut self, arn: &str, tags: I) -> Option<usize>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut merged = self.tags_of(arn);
        merged.extend(tags);
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            return None;
        }
        let count = merged.len();
        self.tags.insert(arn.to_string(), merged);
        Some(count)
    }

    /// Start an `UPDATE_ENABLED_BASELINE` against the enablement `arn` names, parking the
    /// change for [`settle`] to apply once that operation is due, and return the
    /// operation's `operationIdentifier`.
    ///
    /// `None` when `arn` names no stored enablement, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it
    /// is not bounded by [`MAX_ENABLED_BASELINES`]; the operation it appends is bounded by
    /// the log's own retention rule.
    ///
    /// The caller refuses an update that overlaps a baseline operation still in flight
    /// against `arn` with the modelled `ConflictException` (`UpdateEnabledBaseline` is not
    /// `@idempotent`), which is what keeps the single pending slot from being overwritten
    /// while the change it holds is still owed: every accepted update is applied before
    /// another can be accepted.
    ///
    /// [`settle`]: Self::settle
    pub fn record_enabled_baseline_update(
        &mut self,
        arn: &str,
        baseline_version: &str,
        parameters: Option<Vec<(String, ::aws_smithy_types::Document)>>,
        now: SystemTime,
    ) -> Option<String> {
        if !self.enabled_baselines.contains_key(arn) {
            return None;
        }

        let operation_identifier =
            self.record_baseline_operation(BaselineOperationType::UpdateEnabledBaseline, arn, now);
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        // The enablement is changing until the update is due; the status is derived from
        // the log by `settle`, and set to match here so a caller reading straight after
        // the call sees the transient state.
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.clone();
        enabled_baseline.pending_update = Some(PendingEnabledBaselineUpdate {
            operation_identifier: operation_identifier.clone(),
            due_at: now + OPERATION_SETTLE_AFTER,
            baseline_version: baseline_version.to_string(),
            parameters,
        });
        Some(operation_identifier)
    }
}

/// One record of an asynchronous operation log, as the shared retention bound reads it.
///
/// Implemented by every log's record type so [`enforce_log_bound`] is written once; the
/// logs hold different record shapes but retain them by the same rule.
trait OperationLogRecord {
    /// The record's `operationIdentifier`, which is also its key in the log.
    fn identifier(&self) -> &str;
    /// When the operation started.
    fn start_time(&self) -> SystemTime;
    /// Whether the operation has yet to reach a terminal status.
    fn is_in_flight(&self) -> bool;
}

impl OperationLogRecord for LandingZoneOperation {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == LandingZoneOperationStatus::InProgress
    }
}

impl OperationLogRecord for BaselineOperationRecord {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == BaselineOperationStatus::InProgress
    }
}

impl OperationLogRecord for ControlOperationRecord {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == ControlOperationStatus::InProgress
    }
}

/// Drop records until `log` holds at most `max_records`.
///
/// Evicts the oldest TERMINAL record first, so an operation a caller is still polling is
/// never dropped out from under it; only when every record is in flight does the oldest
/// overall go. The `start_time`/identifier pair orders the candidates totally, so which
/// record goes does not depend on the map's iteration order.
fn enforce_log_bound<R: OperationLogRecord>(log: &mut HashMap<String, R>, max_records: usize) {
    while log.len() > max_records {
        let oldest = |terminal_only: bool| {
            log.values()
                .filter(|record| !terminal_only || !record.is_in_flight())
                .min_by(|a, b| {
                    (a.start_time(), a.identifier()).cmp(&(b.start_time(), b.identifier()))
                })
                .map(|record| record.identifier().to_string())
        };
        let Some(evicted) = oldest(true).or_else(|| oldest(false)) else {
            break;
        };
        log.remove(&evicted);
    }
}

/// The identifier of the earliest-started operation in `operations` still `IN_PROGRESS`
/// against the enabled baseline `arn`, or `None` when none is.
///
/// The single definition of "in flight" for baselines: `settle` derives an enabled
/// baseline's status from it while holding a mutable borrow of the enabled baseline map,
/// which is why it takes the operation map rather than `&self`.
fn baseline_operation_in_flight(
    operations: &HashMap<String, BaselineOperationRecord>,
    arn: &str,
) -> Option<String> {
    operations
        .values()
        .filter(|operation| {
            operation.enabled_baseline_arn == arn
                && operation.status == BaselineOperationStatus::InProgress
        })
        .min_by(|a, b| {
            (a.start_time, a.operation_identifier.as_str())
                .cmp(&(b.start_time, b.operation_identifier.as_str()))
        })
        .map(|operation| operation.operation_identifier.clone())
}

/// Whether any operation in `operations` against `arn` is still `IN_PROGRESS`.
///
/// The single definition of "in flight": `settle` derives a landing zone's status from it
/// while holding a mutable borrow of the landing zone map, which is why it takes the
/// operation map rather than `&self`.
fn any_operation_in_flight(
    operations: &HashMap<String, LandingZoneOperation>,
    arn: &str,
) -> bool {
    operations
        .values()
        .any(|op| op.landing_zone_arn == arn && op.status == LandingZoneOperationStatus::InProgress)
}

/// How many identifiers this process has minted, mixed into the next one so two
/// identifiers taken within the same clock tick still differ.
static IDENTIFIER_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// This process's identifier seed, taken once.
///
/// `RandomState` seeds its hasher keys from the operating system, so hashing a fixed
/// value through a fresh instance yields a number this process did not choose and a
/// restarted process does not repeat. That is the entropy an identifier needs: it must
/// not be guessable from the request, and it is not a credential.
fn identifier_seed() -> u64 {
    static SEED: ::std::sync::OnceLock<u64> = ::std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        use ::std::hash::{BuildHasher, Hasher};
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write(b"com.amazonaws.controltower identifier seed");
        hasher.finish()
    })
}

/// A fresh `OperationIdentifier`: the lowercase-hex hyphenated UUID form the model's
/// pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, len 36), with
/// RFC 4122's version-4 and variant bits set.
///
/// The single identifier-generation site in this service. The 16 bytes are the first
/// half of a SHA-256 over this process's seed, the mint counter and the clock: the
/// counter makes two identifiers minted in one tick distinct, the clock separates two
/// processes that drew the same seed, and the hash spreads all three over the whole
/// width. Nothing from the request reaches it, so an identifier is never derivable
/// from the call that asked for it.
pub fn new_operation_identifier() -> String {
    use ::sha2::Digest as _;

    let counter = IDENTIFIER_SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0u128, |since_epoch| since_epoch.as_nanos());

    let mut hasher = ::sha2::Sha256::new();
    hasher.update(identifier_seed().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: version 4 in the high nibble of octet 6, variant 0b10 in the two
    // high bits of octet 8 — the form the model's pattern describes.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut identifier = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        // Hyphens after octets 4, 6, 8 and 10, giving the 8-4-4-4-12 grouping.
        if matches!(index, 4 | 6 | 8 | 10) {
            identifier.push('-');
        }
        use ::std::fmt::Write as _;
        let _ = write!(identifier, "{byte:02x}");
    }
    identifier
}

/// `arn:aws:controltower:<region>:<account>:enabledbaseline/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal. Satisfies `Arn`'s
/// `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern and its 20..2048 length.
pub fn enabled_baseline_arn(region: &str, enabled_baseline_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("enabledbaseline/{enabled_baseline_id}"),
    )
}

/// A fresh `EnabledBaseline` resource id, used as the ARN's resource segment.
///
/// The same 36-character lowercase-hex UUID form as an `OperationIdentifier`, which
/// keeps the minted ARN inside `Arn`'s `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern.
pub fn new_enabled_baseline_id() -> String {
    new_operation_identifier()
}

/// One entry of this service's BASELINE CATALOG: an AWS-managed baseline a caller may
/// enable on a target, as `GetBaseline` reports it.
///
/// A baseline is NOT an account resource — no call creates, changes or deletes one — so
/// the catalog is service-managed data ([`BASELINE_CATALOG`]) rather than a map in the
/// per-account store, the same way the landing zone version catalog is the constant
/// [`CATALOG_LANDING_ZONE_VERSION`] rather than something a caller writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogBaseline {
    /// The ARN's resource segment, which is the baseline's identity: 16 characters of
    /// `[A-Z0-9]`, from `BaselineArn`'s pattern
    /// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`.
    pub id: &'static str,
    /// `GetBaselineOutput$name` / `BaselineSummary$name` — "The human-readable name of a
    /// Baseline."
    pub name: &'static str,
    /// `GetBaselineOutput$description` / `BaselineSummary$description` — "A summary
    /// description of a Baseline."
    pub description: &'static str,
    /// The `baselineVersion`s this baseline OFFERS, oldest first.
    ///
    /// `BaselineVersion` (len 1..10, pattern `^\d+(?:\.\d+){0,2}$`) bounds the SHAPE of a
    /// version string, not the set of versions a baseline has, and the member is "the
    /// specific version to be enabled of the specified baseline" (model docs): a baseline
    /// is enabled at a version it offers, so a value outside this set names no deployable
    /// configuration and is refused rather than persisted — the same rule
    /// [`CATALOG_LANDING_ZONE_VERSIONS`] carries one level up.
    ///
    /// Like the ids, these sets are this emulator's own publication: nothing published
    /// derives a baseline id, and a baseline's version list moves with AWS's releases.
    /// The `4.0` of AWS's own `EnableBaseline` example is kept for the baseline that
    /// example enables, every baseline starts at `1.0`, and each entry satisfies
    /// `BaselineVersion`.
    pub versions: &'static [&'static str],
    /// The `EnabledBaselineParameter` keys this baseline DECLARES, in a stable order.
    ///
    /// `EnabledBaselineParameter$key` is "a string denoting the parameter applied to the
    /// Baseline" — a parameter the baseline itself declares, not a name the caller
    /// invents, so a key outside this set configures nothing and is refused.
    /// `IdentityCenterEnabledForBaseline` is the one the AWS Control Tower documentation
    /// publishes, on the baseline that governs an organizational unit's accounts; a
    /// baseline that declares none carries an EMPTY set, and any `parameters` entry at
    /// all is then refused for it.
    pub parameters: &'static [&'static str],
}

impl CatalogBaseline {
    /// Whether this baseline offers `version`.
    ///
    /// The membership test behind every `baselineVersion` refusal. Matched exactly, like
    /// a landing zone version: a baseline publishes version strings, and one differing in
    /// a character names a version it does not have.
    pub fn offers_version(&self, version: &str) -> bool {
        self.versions.contains(&version)
    }

    /// Whether this baseline declares `key` as an enablement parameter.
    pub fn declares_parameter(&self, key: &str) -> bool {
        self.parameters.contains(&key)
    }
}

/// The baselines this emulator publishes, in a stable order.
///
/// The NAMES are the baseline types the AWS Control Tower User Guide documents
/// (`AWSControlTowerBaseline` and its prerequisite `IdentityCenterBaseline`, the
/// `AuditBaseline`/`LogArchiveBaseline` of the shared accounts, and the three AWS Backup
/// baselines), and each description summarises that documented purpose; the model
/// declares `description` free-form text, so it is a summary rather than AWS's wording
/// character for character.
///
/// The IDs are opaque in AWS — nothing published derives one — so the two that appear in
/// AWS's own API examples (`AWSControlTowerBaseline`, `IdentityCenterBaseline`) are kept
/// and the rest are this emulator's own, chosen once and never minted per request: a
/// baseline ARN a caller stores keeps resolving for the life of this emulator.
pub const BASELINE_CATALOG: &[CatalogBaseline] = &[
    CatalogBaseline {
        id: "17BSJV3IGJ2QSGA2",
        name: "AWSControlTowerBaseline",
        description: "Sets up the resources and permissions AWS Control Tower requires \
                      to govern the accounts of a target organizational unit.",
        versions: &["1.0", "2.0", "3.0", "4.0"],
        parameters: &["IdentityCenterEnabledForBaseline"],
    },
    CatalogBaseline {
        id: "4T4HA1KMO10S6311",
        name: "AuditBaseline",
        description: "Sets up the cross-account audit resources AWS Control Tower \
                      requires in the audit account.",
        versions: &["1.0"],
        parameters: &[],
    },
    CatalogBaseline {
        id: "TR7NC4VPG9DH2KLM",
        name: "BackupAdminBaseline",
        description: "Sets up the backup administration resources AWS Control Tower \
                      requires in the backup administration account.",
        versions: &["1.0"],
        parameters: &[],
    },
    CatalogBaseline {
        id: "QXK3R9ZB7PN5TD2H",
        name: "BackupBaseline",
        description: "Sets up the backup plans and roles AWS Control Tower applies to \
                      the accounts of a target organizational unit.",
        versions: &["1.0"],
        parameters: &[],
    },
    CatalogBaseline {
        id: "WM8FJ6YHL3QS5XKB",
        name: "BackupCentralVaultBaseline",
        description: "Sets up the central backup vault AWS Control Tower requires in \
                      the backup central vault account.",
        versions: &["1.0"],
        parameters: &[],
    },
    CatalogBaseline {
        id: "LN25R72TTG6IGPTQ",
        name: "IdentityCenterBaseline",
        description: "Sets up the AWS IAM Identity Center resources that are a \
                      prerequisite for the AWSControlTowerBaseline.",
        versions: &["1.0", "4.0"],
        parameters: &[],
    },
    CatalogBaseline {
        id: "2V6DZ8QKR4JT7MWN",
        name: "LogArchiveBaseline",
        description: "Sets up the centralized log storage AWS Control Tower requires in \
                      the log archive account.",
        versions: &["1.0"],
        parameters: &[],
    },
];

/// `arn:aws:controltower:<region>::baseline/<id>` — the ARN this endpoint serves a
/// catalog baseline under.
///
/// The account segment is EMPTY, which is why this is NOT built through
/// `engine::emulator_arn` the way the per-account resources below are: a baseline is
/// AWS-managed catalog data that no account owns, so there is no account to name in its
/// ARN. `BaselineArn`'s pattern
/// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` admits ZERO
/// digits in the account position for exactly that reason, and the account-less form is
/// the one a caller round-trips back into `GetBaseline` or `EnableBaseline`.
///
/// The resources an account DOES own keep the emulator's canonical account id
/// ([`landing_zone_arn`], [`enabled_baseline_arn`], [`enabled_control_arn`]): an
/// enablement belongs to the caller's account, a baseline belongs to none.
pub fn baseline_arn(region: &str, baseline_id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::baseline/{baseline_id}")
}

/// The catalog entry a `baselineIdentifier` names, or `None` when the identifier is not a
/// baseline ARN or the catalog holds no baseline with its resource id.
///
/// A baseline's identity is the 16-character resource id after `baseline/`: the same
/// catalog is published in every partition, region and account, so the segments before
/// it select nothing and are not compared beyond being the ones a baseline ARN carries
/// ([`is_baseline_arn`]). The id itself is matched exactly, so a value differing from a
/// catalog id in one character names no baseline.
pub fn resolve_baseline(baseline_identifier: &str) -> Option<&'static CatalogBaseline> {
    if !is_baseline_arn(baseline_identifier) {
        return None;
    }
    let (_, baseline_id) = baseline_identifier.rsplit_once(":baseline/")?;
    BASELINE_CATALOG
        .iter()
        .find(|baseline| baseline.id == baseline_id)
}

/// Whether `identifier` is a baseline ARN at all.
///
/// The model's own `BaselineArn` form,
/// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`, checked
/// segment by segment. `GetBaseline$baselineIdentifier` carries that type so its front
/// door answers this question, but `EnableBaseline$baselineIdentifier` is typed as the
/// generic `Arn` (pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`), which cannot express "is a
/// baseline" — so an ARN of some other service, or one naming some other kind of
/// resource, reaches a handler and is told apart here.
pub fn is_baseline_arn(identifier: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(identifier) else {
        return false;
    };
    !partition.is_empty()
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && service == ARN_NAMESPACE
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && account.len() <= 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && resource.strip_prefix("baseline/").is_some_and(|id| {
            id.len() == 16
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
}

/// One entry of this service's CONTROL catalog: a control AWS Control Tower publishes,
/// which a caller names with a `controlIdentifier`.
///
/// A control is NOT an account resource — no call creates, changes or deletes one, and the
/// same controls exist in every account — so the catalog is service-managed data
/// ([`CONTROL_CATALOG`]) rather than a map in the per-account store, exactly like
/// [`BASELINE_CATALOG`] one level up. What an account owns is the ENABLEMENT of a control
/// on a target, which is what `enabled_controls` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogControl {
    /// The control's NAME, which is the resource segment of its AWS Control Tower ARN
    /// (`arn:aws:controltower:<region>::control/<name>`) and the control's identity.
    ///
    /// These names are the ones the AWS Control Tower controls reference guide publishes
    /// for its `AWS-GR_`-prefixed controls. A control whose published name carries a
    /// character `ControlIdentifier`'s pattern (`^arn:aws[0-9a-zA-Z_\-:\/]+$`) does not
    /// admit — a dot, say — cannot reach a handler at all, so the catalog publishes none.
    pub name: &'static str,
    /// The resource segment of the SAME control's AWS Control Catalog ARN
    /// (`arn:<partition>:controlcatalog:::control/<id>`), the newer form of the identifier
    /// a caller may name a control by.
    ///
    /// These ids are opaque in AWS — nothing published derives one from a control's name —
    /// so, like the baseline ids, they are this emulator's own publication: chosen once
    /// and never minted per request, so an identifier a caller stores keeps resolving for
    /// the life of this emulator. Each is 26 lowercase alphanumeric characters, the form
    /// AWS Control Catalog control ARNs carry.
    pub catalog_id: &'static str,
    /// Whether `EnableControl` may enable this control.
    ///
    /// `EnableControlInput$controlIdentifier` is documented as "Only Strongly recommended
    /// and Elective controls are permitted, with the exception of the Region deny
    /// control": the MANDATORY controls AWS Control Tower applies to every governed
    /// account itself are named by a `controlIdentifier` — `ListEnabledControls` reports
    /// them — but cannot be enabled by a caller, so a request naming one is refused rather
    /// than persisted. False for exactly those.
    pub enablable: bool,
    /// The `EnabledControlParameter` keys this control DECLARES, in a stable order.
    ///
    /// `EnabledControlParameter$key` is "a string denoting the parameter applied to the
    /// control" — a parameter the control itself declares, not a name the caller invents,
    /// so a key outside this set configures nothing and is refused. Most AWS Control Tower
    /// controls take no configuration at all and carry an EMPTY set, for which any
    /// `parameters` entry is refused; the Region deny control is the one the API
    /// reference's own `EnableControl` example configures, with `AllowedRegions`, beside
    /// the exemption keys its documentation publishes. Like the ids, this set is this
    /// emulator's publication and moves with AWS's releases.
    pub parameters: &'static [&'static str],
}

impl CatalogControl {
    /// Whether this control declares `key` as an enablement parameter.
    pub fn declares_parameter(&self, key: &str) -> bool {
        self.parameters.contains(&key)
    }
}

/// The controls this emulator publishes, in a stable order.
///
/// A mix of the three kinds of control the AWS Control Tower controls reference guide
/// publishes — the mandatory ones the service applies itself, the strongly recommended and
/// elective ones a caller enables, and the Region deny control — so that every rule
/// `EnableControl` applies to a `controlIdentifier` has catalog entries on both sides of
/// it. It is not AWS's whole catalogue of hundreds of controls: a name outside this set
/// names no control this emulator has.
pub const CONTROL_CATALOG: &[CatalogControl] = &[
    // The mandatory controls: AWS Control Tower applies these to every governed account
    // itself, so a caller may not enable them.
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_DELETION_PROHIBITED",
        catalog_id: "5kvme4m5d2b4d7dcf0fqkdmxt3",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_ENCRYPTION_ENABLED",
        catalog_id: "9h2jr6twx1pzn4bq8sm0lyvke5",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_POLICY_CHANGES_PROHIBITED",
        catalog_id: "3fxn7cgp5rmbt8wzd1qkj4shvy",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_CHANGE_PROHIBITED",
        catalog_id: "2mpr9vxtc6gln6tjb5hwqz0dkf",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_ENABLED",
        catalog_id: "1ysc3nmpx8rvg0kdwt2fqhj7zm",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_VALIDATION_ENABLED",
        catalog_id: "7dcf0fqkdmxt35kvme4m5d2b4d",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CONFIG_CHANGE_PROHIBITED",
        catalog_id: "4bq8sm0lyvke59h2jr6twx1pzn",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CONFIG_ENABLED",
        catalog_id: "8wzd1qkj4shvy3fxn7cgp5rmbt",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_IAM_ROLE_CHANGE_PROHIBITED",
        catalog_id: "6tjb5hwqz0dkf2mpr9vxtc6gln",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_LOG_GROUP_POLICY",
        catalog_id: "0kdwt2fqhj7zm1ysc3nmpx8rvg",
        enablable: false,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_SNS_SUBSCRIPTION_CHANGE_PROHIBITED",
        catalog_id: "5kvme4m5d2b4d9h2jr6twx1pzn",
        enablable: false,
        parameters: &[],
    },
    // The strongly recommended and elective controls, which a caller enables on a target.
    CatalogControl {
        name: "AWS-GR_ENCRYPTED_VOLUMES",
        catalog_id: "7dcf0fqkdmxt34bq8sm0lyvke5",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_EBS_OPTIMIZED_INSTANCE",
        catalog_id: "9h2jr6twx1pzn3fxn7cgp5rmbt",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_EC2_VOLUME_INUSE_CHECK",
        catalog_id: "4bq8sm0lyvke58wzd1qkj4shvy",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RDS_INSTANCE_PUBLIC_ACCESS_CHECK",
        catalog_id: "3fxn7cgp5rmbt2mpr9vxtc6gln",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RDS_STORAGE_ENCRYPTED",
        catalog_id: "8wzd1qkj4shvy6tjb5hwqz0dkf",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RESTRICTED_COMMON_PORTS",
        catalog_id: "2mpr9vxtc6gln1ysc3nmpx8rvg",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_ROOT_ACCOUNT_MFA_ENABLED",
        catalog_id: "6tjb5hwqz0dkf0kdwt2fqhj7zm",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED",
        catalog_id: "1ysc3nmpx8rvg5kvme4m5d2b4d",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_S3_BUCKET_PUBLIC_WRITE_PROHIBITED",
        catalog_id: "0kdwt2fqhj7zm7dcf0fqkdmxt3",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS",
        catalog_id: "5kvme4m5d2b4d4bq8sm0lyvke5",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RESTRICT_ROOT_USER",
        catalog_id: "7dcf0fqkdmxt33fxn7cgp5rmbt",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RESTRICT_ROOT_USER_ACCESS_KEYS",
        catalog_id: "9h2jr6twx1pzn8wzd1qkj4shvy",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_CROSS_REGION_NETWORKING",
        catalog_id: "4bq8sm0lyvke52mpr9vxtc6gln",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_VPC_INTERNET_ACCESS",
        catalog_id: "3fxn7cgp5rmbt6tjb5hwqz0dkf",
        enablable: true,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_VPN_CONNECTIONS",
        catalog_id: "8wzd1qkj4shvy1ysc3nmpx8rvg",
        enablable: true,
        parameters: &[],
    },
    // The Region deny control: permitted by the member's own documentation despite not
    // being one of the strongly recommended or elective controls, and the one control this
    // catalog publishes with parameters.
    CatalogControl {
        name: "AWS-GR_REGION_DENY",
        catalog_id: "2mpr9vxtc6gln0kdwt2fqhj7zm",
        enablable: true,
        parameters: &["AllowedRegions", "ExemptedPrincipalArns", "ExemptedActions"],
    },
];

/// The catalog entry a `controlIdentifier` names, or `None` when the identifier is not a
/// control ARN or the catalog holds no control it names.
///
/// A control is named by either of the two ARN forms AWS publishes for it — the AWS
/// Control Tower form, whose resource segment is the control's NAME, and the AWS Control
/// Catalog form, whose resource segment is the control's opaque id — so both resolve to
/// the same entry here. The same catalog is published in every partition, region and
/// account, so the segments before the resource select nothing and are not compared beyond
/// being the ones a control ARN carries; the name and the id are matched exactly, so a
/// value differing from a catalog entry in one character names no control.
pub fn resolve_control(control_identifier: &str) -> Option<&'static CatalogControl> {
    match control_reference(control_identifier)? {
        ControlReference::Name(name) => CONTROL_CATALOG.iter().find(|control| control.name == name),
        ControlReference::CatalogId(id) => CONTROL_CATALOG
            .iter()
            .find(|control| control.catalog_id == id),
    }
}

/// Whether `identifier` is a control ARN at all, in either form AWS publishes.
///
/// `EnableControl$controlIdentifier` is typed as `ControlIdentifier`, whose pattern
/// (`^arn:aws[0-9a-zA-Z_\-:\/]+$`) admits any ARN of any service, so an ARN naming some
/// other kind of resource reaches the handler and is told apart here: one of these two
/// forms naming no catalog entry references a control that does not exist, while anything
/// else is not a control reference at all.
pub fn is_control_arn(identifier: &str) -> bool {
    control_reference(identifier).is_some()
}

/// How a control ARN names its control: by name in the AWS Control Tower form, by opaque
/// id in the AWS Control Catalog form.
enum ControlReference<'a> {
    /// `arn:<partition>:controltower:<region>::control/<NAME>`.
    Name(&'a str),
    /// `arn:<partition>:controlcatalog:::control/<id>`.
    CatalogId(&'a str),
}

/// The control `identifier` references, or `None` when it is not a control ARN.
///
/// Neither form names an account: a control belongs to no account, which is why the
/// account segment must be empty in both. The Control Catalog form names no region either
/// — that catalog is global — while the Control Tower form carries the region the caller
/// read the control's ARN in.
fn control_reference(identifier: &str) -> Option<ControlReference<'_>> {
    let (partition, service, region, account, resource) = arn_segments(identifier)?;
    if partition.is_empty()
        || !partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        || !account.is_empty()
    {
        return None;
    }
    let resource = resource.strip_prefix("control/")?;
    match service {
        ARN_NAMESPACE
            if region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && is_control_name(resource) =>
        {
            Some(ControlReference::Name(resource))
        }
        "controlcatalog" if region.is_empty() && is_control_catalog_id(resource) => {
            Some(ControlReference::CatalogId(resource))
        }
        _ => None,
    }
}

/// Whether `value` has the form of an AWS Control Tower control name: a non-empty run of
/// the uppercase letters, digits, underscores and hyphens the published names are made of.
fn is_control_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Whether `value` has the form of an AWS Control Catalog control id: 26 lowercase
/// alphanumeric characters.
fn is_control_catalog_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `target` is the ARN of an AWS Organizations organizational unit.
///
/// `EnableBaseline$targetIdentifier` is "the ARN of the target on which the baseline will
/// be enabled. Only OUs are supported as targets" (model docs), and its `Arn` type cannot
/// express that, so the semantic check is here: the ARN must be an Organizations ARN
/// (`arn:<partition>:organizations::<12-digit account>:ou/<organization>/<unit>`) whose
/// resource is an organizational unit, with the `o-` / `ou-` identifier forms the AWS
/// Organizations documentation publishes. The published LENGTH windows of those
/// identifiers are not imposed — the identifiers AWS's own examples carry are shorter
/// than the windows — so what is checked is the structure: an ARN of another service,
/// another resource type, or a value that is not an organizational unit identifier at all
/// names no target a baseline can be applied to.
pub fn is_organizational_unit_arn(target: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(target) else {
        return false;
    };
    if partition.is_empty() || service != "organizations" || !region.is_empty() {
        return false;
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some((organization, organizational_unit)) = resource
        .strip_prefix("ou/")
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    organization.strip_prefix("o-").is_some_and(is_lower_alnum)
        && organizational_unit
            .strip_prefix("ou-")
            .and_then(|rest| rest.split_once('-'))
            .is_some_and(|(root, unit)| is_lower_alnum(root) && is_lower_alnum(unit))
}

/// An ARN's partition, service, region, account and resource segments, or `None` when
/// `value` is not six colon-separated segments beginning with `arn`.
///
/// The resource segment keeps whatever colons it contains, so a caller that expects a
/// slash-delimited resource rejects a colon-delimited one by matching on it.
fn arn_segments(value: &str) -> Option<(&str, &str, &str, &str, &str)> {
    let mut segments = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    Some((partition, service, region, account, resource))
}

/// Whether `value` is a non-empty run of lowercase letters and digits — the character
/// class an AWS Organizations entity identifier is made of.
fn is_lower_alnum(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Why `target` cannot have a baseline enabled on it, or `None` when it is an
/// organizational unit ARN.
///
/// The one place that refusal's wording is built. The message is built from the request
/// and from the rule, so it tells the caller what kind of target it may name.
pub fn non_organizational_unit_target_message(target: &str) -> Option<String> {
    if is_organizational_unit_arn(target) {
        return None;
    }
    Some(format!(
        "targetIdentifier {target:?} is not the ARN of an organizational unit; only \
         organizational units are supported as targets, named as \
         arn:aws:organizations::<account>:ou/<organization>/<organizational unit>"
    ))
}

/// Why `version` cannot be enabled of `baseline`, or `None` when that baseline offers it.
///
/// The one place this refusal's wording is built, so `EnableBaseline` and
/// `UpdateEnabledBaseline` refuse an unoffered version in the same words, and the message
/// is built from the request and the catalog so it tells the caller which versions it may
/// ask for.
pub fn unsupported_baseline_version_message(
    baseline: &CatalogBaseline,
    version: &str,
) -> Option<String> {
    if baseline.offers_version(version) {
        return None;
    }
    Some(format!(
        "baselineVersion {version:?} is not a version baseline {} offers; the available \
         versions are {}",
        baseline.name,
        baseline.versions.join(", ")
    ))
}

/// Why `parameters` does not configure `baseline`, or `None` when it does.
///
/// The one place a baseline enablement's parameter list is checked, so `EnableBaseline`
/// and `UpdateEnabledBaseline` refuse the same lists in the same words. A list is
/// unacceptable when it names a key the baseline does not declare (it would configure
/// nothing), repeats a key (one key addresses one parameter, so a list carrying it twice
/// does not describe one configuration) or weighs more than
/// [`MAX_ENABLED_BASELINE_PARAMETER_BYTES`].
pub fn unacceptable_parameters_message(
    baseline: &CatalogBaseline,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    unacceptable_enablement_parameters_message(
        "baseline",
        baseline.name,
        baseline.parameters,
        MAX_ENABLED_BASELINE_PARAMETER_BYTES,
        parameters,
    )
}

/// Why `parameters` does not configure `control`, or `None` when it does.
///
/// The one place a control's parameter list is checked, so `EnableControl` and
/// `UpdateEnabledControl` refuse the same lists in the same words — the rule
/// [`unacceptable_parameters_message`] applies to a baseline's parameters one level up, so
/// both read it off the same walk.
pub fn unacceptable_control_parameters_message(
    control: &CatalogControl,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    unacceptable_enablement_parameters_message(
        "control",
        control.name,
        control.parameters,
        MAX_ENABLED_CONTROL_PARAMETER_BYTES,
        parameters,
    )
}

/// Why `parameters` does not configure the resource named by `kind`/`name`, or `None` when
/// it does.
///
/// The ONE rule both an enabled baseline's and an enabled control's parameter list are
/// checked by: a list is unacceptable when it names a key the resource does not declare (it
/// would configure nothing), repeats a key (one key addresses one parameter, so a list
/// carrying it twice does not describe one configuration) or weighs more than
/// `byte_cap`. `kind` is the word the refusal calls the resource, so the message names the
/// member and what that resource declares.
fn unacceptable_enablement_parameters_message(
    kind: &str,
    name: &str,
    declared: &[&str],
    byte_cap: usize,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    // One pass over a set of the keys already seen, so detecting a repeat costs
    // O(n log n) in the entry count rather than O(n^2).
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    let mut weight = 0usize;
    for (key, value) in parameters {
        if !declared.contains(&key.as_str()) {
            return Some(if declared.is_empty() {
                format!(
                    "parameters names {key:?}, which {kind} {name} does not declare; that \
                     {kind} declares no enablement parameters"
                )
            } else {
                format!(
                    "parameters names {key:?}, which {kind} {name} does not declare; the \
                     parameters it declares are {}",
                    declared.join(", ")
                )
            });
        }
        if !seen.insert(key.as_str()) {
            return Some(format!(
                "parameters carries key {key:?} more than once; each enablement parameter \
                 key may be given at most once"
            ));
        }
        // The budget left for this value, so the whole list is measured once and the walk
        // stops at the first node that takes it past the cap.
        let budget = byte_cap.saturating_sub(weight.saturating_add(key.len()));
        let Some(value_weight) = document_weight_within(value, budget) else {
            return Some(format!(
                "parameters weighs more than the {byte_cap} bytes one {kind} enablement's \
                 parameters may hold"
            ));
        };
        weight = weight.saturating_add(key.len()).saturating_add(value_weight);
    }
    None
}

/// Whether `configured` and `requested` describe the SAME control configuration.
///
/// A parameter is addressed by its key and a list may carry a key at most once
/// ([`unacceptable_control_parameters_message`]), so a configuration is the key/value
/// mapping a list denotes and the order the entries were sent in does not distinguish two
/// configurations.
pub fn same_control_configuration(
    configured: &[(String, ::aws_smithy_types::Document)],
    requested: &[(String, ::aws_smithy_types::Document)],
) -> bool {
    fn as_map(
        parameters: &[(String, ::aws_smithy_types::Document)],
    ) -> BTreeMap<&str, &::aws_smithy_types::Document> {
        parameters
            .iter()
            .map(|(key, value)| (key.as_str(), value))
            .collect()
    }
    as_map(configured) == as_map(requested)
}

/// Whether `identifier` is a landing zone ARN at all.
///
/// A landing zone is addressed by the ARN this service reports for it — the only
/// identifier a caller is ever handed for one (`CreateLandingZoneOutput$arn`,
/// `LandingZoneSummary$arn`, both typed `Arn`: length 20..2048, pattern
/// `^arn:aws[0-9a-zA-Z_\-:\/]+$`) — and the form that ARN carries is
/// [`landing_zone_arn`]'s: `arn:<partition>:controltower:<region>:<account>:landingzone/<id>`.
/// `landingZoneIdentifier` is modelled as a plain `String`, so no front-door check
/// applies and the shape check is the handler's; it is checked segment by segment here,
/// the way [`is_baseline_arn`] checks a baseline's. The resource id itself is not
/// constrained beyond being one non-empty segment of `Arn`'s character class: the service
/// publishes no pattern for it, so a shape check must not reject an id of another form.
pub fn is_landing_zone_arn(identifier: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(identifier) else {
        return false;
    };
    (20..=2048).contains(&identifier.len())
        && partition.starts_with("aws")
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && service == ARN_NAMESPACE
        && !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && account.len() == 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && resource.strip_prefix("landingzone/").is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

/// Why `identifier` names no landing zone, or `None` when it is a landing zone ARN.
///
/// The one place this refusal's wording is built, so every operation taking a
/// `landingZoneIdentifier` refuses the same values in the same words. A value that is not
/// a landing zone ARN ([`is_landing_zone_arn`]) is a malformed input rather than a
/// reference to a resource that does not exist, so a caller refuses it with the modelled
/// `ValidationException` and keeps `ResourceNotFoundException` for an ARN of that shape
/// naming no stored landing zone. The message is built from the request and the rule, so
/// it tells the caller what it may send.
pub fn non_landing_zone_identifier_message(identifier: &str) -> Option<String> {
    if is_landing_zone_arn(identifier) {
        return None;
    }
    Some(format!(
        "landingZoneIdentifier {identifier:?} is not the ARN of an AWS Control Tower landing \
         zone; a landing zone is named by the ARN CreateLandingZone and ListLandingZones \
         report for it, of the form \
         arn:aws:controltower:<region>:<account>:landingzone/<id>"
    ))
}

/// A fresh landing zone resource id, used as the ARN's resource segment.
///
/// The same 36-character lowercase-hex UUID form as an `OperationIdentifier`, which
/// keeps the minted ARN inside `Arn`'s `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern.
pub fn new_landing_zone_id() -> String {
    new_operation_identifier()
}

/// `arn:aws:controltower:<region>:<account>:landingzone/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal.
pub fn landing_zone_arn(region: &str, landing_zone_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("landingzone/{landing_zone_id}"),
    )
}

/// A fresh enabled-control resource id, used as the ARN's resource segment.
///
/// The same 36-character lowercase-hex UUID form as an `OperationIdentifier`, which
/// keeps the minted ARN inside `Arn`'s `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern.
pub fn new_enabled_control_id() -> String {
    new_operation_identifier()
}

/// `arn:aws:controltower:<region>:<account>:enabledcontrol/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal.
pub fn enabled_control_arn(region: &str, enabled_control_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("enabledcontrol/{enabled_control_id}"),
    )
}

/// The approximate size in bytes of `document`, or `None` as soon as it exceeds `cap`.
///
/// Walks the document with an EXPLICIT stack rather than by recursion, so a deeply
/// nested value cannot overflow the thread's stack, and stops at the first node that
/// takes the running total past `cap`. A container wider than the budget left is refused
/// BEFORE its children are pushed, so both the walk and the stack it uses cost at most
/// `cap` whatever the caller sent. A scalar counts as its machine width and a string,
/// key or structural node as the bytes it holds — an approximation, which is all a bound
/// needs.
pub fn document_weight_within(document: &::aws_smithy_types::Document, cap: usize) -> Option<usize> {
    use ::aws_smithy_types::Document;

    /// What a scalar or a container node itself costs, before its contents.
    const NODE_WEIGHT: usize = 8;

    let mut total = 0usize;
    let mut pending: Vec<&Document> = vec![document];
    while let Some(node) = pending.pop() {
        let weight = match node {
            Document::Null | Document::Bool(_) | Document::Number(_) => NODE_WEIGHT,
            Document::String(value) => value.len(),
            Document::Array(items) => {
                // More children than the whole remaining budget cannot be summarised
                // within it, so the width is refused before the stack grows by it.
                if items.len() > cap.saturating_sub(pending.len()) {
                    return None;
                }
                pending.extend(items.iter());
                NODE_WEIGHT
            }
            Document::Object(members) => {
                if members.len() > cap.saturating_sub(pending.len()) {
                    return None;
                }
                let keys = members.keys().map(String::len).sum::<usize>();
                pending.extend(members.values());
                NODE_WEIGHT.saturating_add(keys)
            }
        };
        total = total.saturating_add(weight);
        if total > cap {
            return None;
        }
    }
    Some(total)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `(major, minor)` of a `LandingZoneVersion`, for NUMERIC version order.
///
/// `None` when the value is not `<digits>.<digits>` — the model's pattern
/// `^\d+.\d+$` leaves the separator unescaped, so it admits values this cannot order.
fn version_order(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The later of a deployed landing zone version and the catalog's newest version.
///
/// Compared NUMERICALLY per component, so `3.10` is newer than `3.3`. A deployed
/// version at or beyond the catalog's — or one this cannot order — is its own latest
/// available version, so the answer is never older than what is deployed.
///
/// A create or update only accepts a version [`CATALOG_LANDING_ZONE_VERSIONS`] offers, so
/// in practice the answer is the catalog's newest; the comparison is what keeps the
/// invariant when the catalog is retuned around an already-deployed landing zone.
pub fn latest_available_version(deployed: &str, catalog: &str) -> String {
    match (version_order(deployed), version_order(catalog)) {
        (Some(deployed_order), Some(catalog_order)) if catalog_order > deployed_order => {
            catalog.to_string()
        }
        _ => deployed.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsControltowerAwscontroltowerapisState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsControltowerAwscontroltowerapisState>)
}
