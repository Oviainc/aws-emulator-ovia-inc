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
//! RETENTION: that log is append-only only WITHIN a bound. It is the one store here a
//! caller can grow by repeating calls — the landing zone and tag maps are capped at the
//! single landing zone an account may hold — so it retains at most
//! [`MAX_LANDING_ZONE_OPERATIONS`] records, evicting the oldest TERMINAL record to make
//! room (see [`ComAmazonawsControltowerAwscontroltowerapisState::record_operation`]).
//! That cap also bounds the per-request scan [`settle`] runs under the state lock.
//!
//! [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, from the
/// model's `aws.api#service` trait (`arnNamespace: controltower`).
pub const ARN_NAMESPACE: &str = "controltower";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a landing zone operation stays `IN_PROGRESS` before the clock owes it a
/// terminal status.
///
/// `CreateLandingZone`/`DeleteLandingZone` "start an asynchronous operation" (model
/// docs), so a caller must be able to observe `IN_PROGRESS` and a `PROCESSING` landing
/// zone. AWS takes tens of minutes, which no emulator can usefully reproduce and which
/// AWS does not document as a contract, so this span is CHOSEN — kept short enough that
/// a test can wait for it and long enough that the call that starts the operation always
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

/// Per-service state for com-amazonaws-controltower-awscontroltowerapis. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsControltowerAwscontroltowerapisState {
    /// Landing zones keyed by ARN. At most one entry: AWS allows one landing zone per
    /// account, and that rule is enforced from this map's size.
    pub landing_zones: HashMap<String, LandingZone>,
    /// The operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_LANDING_ZONE_OPERATIONS`] records; see the module header on retention.
    pub landing_zone_operations: HashMap<String, LandingZoneOperation>,
    /// Tags keyed by the tagged resource's ARN — the single home for this service's
    /// tags, written by `CreateLandingZone`'s `tags` member.
    pub tags: HashMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsControltowerAwscontroltowerapisState {
    /// Materialise every transition the CLOCK has already made due, and nothing else.
    ///
    /// An operation whose `due_at` has passed is terminal whether or not anybody called,
    /// so settling it here is not a read-triggered mutation; a landing zone's status is
    /// then derived from whether any operation against it is still in flight. Call this
    /// first in every handler so reporting and mutating operations agree on the clock.
    pub fn settle(&mut self, now: SystemTime) {
        for op in self.landing_zone_operations.values_mut() {
            if op.status == LandingZoneOperationStatus::InProgress && now >= op.due_at {
                op.status = LandingZoneOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                op.end_time = Some(op.due_at);
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
        self.enforce_operation_log_bound();
        operation_identifier
    }

    /// Drop records until the log holds at most [`MAX_LANDING_ZONE_OPERATIONS`].
    ///
    /// Evicts the oldest TERMINAL record first, so an operation a caller is still polling
    /// is never dropped out from under it; only when every record is in flight does the
    /// oldest overall go. The `start_time`/identifier pair orders the candidates totally,
    /// so which record goes does not depend on the map's iteration order.
    fn enforce_operation_log_bound(&mut self) {
        while self.landing_zone_operations.len() > MAX_LANDING_ZONE_OPERATIONS {
            let oldest = |terminal_only: bool| {
                self.landing_zone_operations
                    .values()
                    .filter(|op| {
                        !terminal_only || op.status != LandingZoneOperationStatus::InProgress
                    })
                    .min_by(|a, b| {
                        (a.start_time, a.operation_identifier.as_str())
                            .cmp(&(b.start_time, b.operation_identifier.as_str()))
                    })
                    .map(|op| op.operation_identifier.clone())
            };
            let Some(evicted) = oldest(true).or_else(|| oldest(false)) else {
                break;
            };
            self.landing_zone_operations.remove(&evicted);
        }
    }
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
