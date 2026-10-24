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
//! to a target, and `EnableBaseline`/`DisableBaseline` each start an asynchronous
//! operation whose record lands in a SECOND log keyed by `operationIdentifier`
//! ([`ComAmazonawsControltowerAwscontroltowerapisState::baseline_operations`]), which
//! `GetBaselineOperation` resolves. The two logs are separate: an identifier a landing
//! zone call returned is not a baseline operation and vice versa.
//!
//! RETENTION: every store a caller can grow by repeating calls is BOUNDED, so ordinary
//! request traffic cannot grow this process's heap, or the per-request scan [`settle`]
//! runs under the state lock, without limit:
//!
//! * `landing_zones` holds the one landing zone an account may have;
//! * the two operation logs are append-only only WITHIN a bound, retaining at most
//!   [`MAX_LANDING_ZONE_OPERATIONS`] / [`MAX_BASELINE_OPERATIONS`] records and evicting
//!   the oldest TERMINAL record to make room (see [`enforce_log_bound`]);
//! * `enabled_baselines` holds at most [`MAX_ENABLED_BASELINES`] enablements, and
//!   `EnableBaseline` is REFUSED past that cap rather than growing the store (see
//!   [`record_enablement`]), because an enablement is a resource a caller must tear down
//!   rather than a history record a newer one may evict;
//! * `enabled_controls` holds at most [`MAX_ENABLED_CONTROLS`] control enablements — one
//!   per control/target pair `EnableControl` is called for — and `EnableControl` is
//!   REFUSED past that cap for the same reason as `EnableBaseline`;
//! * `tags` is keyed by the ARNs of the two stores above, and an entry is dropped with
//!   the resource it tags, so it is bounded by them.
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

/// The most `EnabledControlParameter`s one `EnableControl` request may carry.
///
/// `EnabledControlParameters` carries no `@length` trait, so the front door imposes no
/// count on it and the only other ceiling is the server's request-body limit — which
/// admits hundreds of thousands of entries. The count a caller chooses is therefore
/// bounded HERE, before any per-parameter work runs, and refused with the modelled
/// `ValidationException`. The value is CHOSEN: an AWS Control Tower control is
/// configured by a handful of parameters, so this is far above any real configuration
/// and small enough that the duplicate-key pass and the stored record are bounded.
pub const MAX_ENABLED_CONTROL_PARAMETERS: usize = 200;

/// The most bytes one `EnableControl` request's `parameters` may weigh, keys and
/// documents together.
///
/// A parameter count alone does not bound the store: an `EnabledControlParameter$value`
/// is an unconstrained `Document`, so one parameter can carry a whole body. The weight
/// is measured with [`document_weight_within`], which stops as soon as this cap is
/// exceeded, and a request above it is refused with the modelled `ValidationException`.
/// CHOSEN for the same reason as the count: orders of magnitude above any real control
/// configuration, and small enough that [`MAX_ENABLED_CONTROLS`] records bound the
/// store's size in bytes as well as in entries.
pub const MAX_ENABLED_CONTROL_PARAMETER_BYTES: usize = 64 * 1024;

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

/// The newest landing zone version this emulator's catalog offers.
///
/// Reported through `LandingZoneDetail$latestAvailableVersion` ("The latest available
/// version of the landing zone"). Satisfies `LandingZoneVersion` (len 3..10, pattern
/// `^\d+.\d+$`).
pub const CATALOG_LANDING_ZONE_VERSION: &str = "3.3";

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
/// ones `EnableBaseline` was given.
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
    /// Enabled baselines keyed by the `EnabledBaseline` ARN this service minted for them.
    pub enabled_baselines: HashMap<String, EnabledBaseline>,
    /// The baseline operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_BASELINE_OPERATIONS`] records; see the module header on retention.
    pub baseline_operations: HashMap<String, BaselineOperationRecord>,
    /// Tags keyed by the tagged resource's ARN — the single home for this service's
    /// tags, written by `CreateLandingZone`'s and `EnableBaseline`'s `tags` members.
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
    /// Accepts the full ARN (the map key) and the bare resource id that ARN ends in, so
    /// either form a caller holds resolves to the one stored landing zone.
    pub fn resolve_landing_zone_arn(&self, identifier: &str) -> Option<String> {
        if self.landing_zones.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.landing_zones
            .keys()
            .find(|arn| arn.split('/').next_back() == Some(identifier))
            .cloned()
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

    /// The stored ARN an `enabledBaselineIdentifier` names, or `None` when no enabled
    /// baseline matches.
    ///
    /// Accepts the full ARN (the map key) and the bare resource id that ARN ends in, the
    /// same two forms [`resolve_landing_zone_arn`] accepts.
    ///
    /// [`resolve_landing_zone_arn`]: Self::resolve_landing_zone_arn
    pub fn resolve_enabled_baseline_arn(&self, identifier: &str) -> Option<String> {
        if self.enabled_baselines.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.enabled_baselines
            .keys()
            .find(|arn| arn.split('/').next_back() == Some(identifier))
            .cloned()
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
            },
        );
        Some(operation_identifier)
    }
}

/// One record of an asynchronous operation log, as the shared retention bound reads it.
///
/// Implemented by both logs' record types so [`enforce_log_bound`] is written once; the
/// two logs hold different record shapes but retain them by the same rule.
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
