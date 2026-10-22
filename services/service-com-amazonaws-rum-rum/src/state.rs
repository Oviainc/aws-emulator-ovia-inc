// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rum-rum service.
//!
//! The app monitor is this service's primary resource: `CreateAppMonitor`,
//! `GetAppMonitor`, `ListAppMonitors`, `UpdateAppMonitor`, `DeleteAppMonitor` and the
//! data-plane `PutRumEvents` all read and write the ONE [`AppMonitorRecord`] map held
//! here, so every one of them answers from persisted state rather than from its own
//! request. The helpers beside the map (id minting, the derived ARN and CloudWatch Logs
//! group name, the documented defaults, the record -> wire-shape projections and the
//! error builders) live here for the same reason: six operation files share them, and a
//! second copy of any of them would let two operations disagree about one resource.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AppMonitor, AppMonitorConfiguration, AppMonitorDetails, AppMonitorSummary,
    ConflictException, CustomEvents, CwLog, DataStorage, DeobfuscationConfiguration,
    ResourceNotFoundException,
};

/// This service's ARN namespace / sigv4 signing name (`OPERATIONS.md`,
/// `## Wire protocol`: both are `rum`).
pub const ARN_NAMESPACE: &str = "rum";

/// The `appmonitor` resource segment of an app-monitor ARN.
const APP_MONITOR_RESOURCE: &str = "appmonitor";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous, host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `State` of an app monitor this emulator creates — a member of
/// [`crate::ops::types::STATE_ENUM_VALUES`].
///
/// `CreateAppMonitor` is synchronous and the monitor it returns is immediately able to
/// accept `PutRumEvents`, so the resource is created in a terminal state with no
/// clock-owed transition pending.
pub const ACTIVE_STATE: &str = "ACTIVE";

/// `Platform` of an app monitor whose create request named none.
///
/// From the model: `CreateAppMonitorRequest$Platform` documents "If you omit this
/// parameter, the default is Web."
pub const DEFAULT_PLATFORM: &str = "Web";

/// `CustomEvents$Status` of an app monitor whose create request named no `CustomEvents`.
///
/// From the model: `CreateAppMonitorRequest$CustomEvents` documents "If you omit this
/// parameter, custom events are DISABLED." A member of
/// [`crate::ops::types::CUSTOM_EVENTS_STATUS_VALUES`].
pub const DEFAULT_CUSTOM_EVENTS_STATUS: &str = "DISABLED";

/// `SessionSampleRate` of an app monitor whose create request named no
/// `AppMonitorConfiguration`.
///
/// From the model: `CreateAppMonitorRequest$AppMonitorConfiguration` documents "If you
/// omit this argument, the sample rate used for RUM is set to 10% of the user sessions."
/// (The member's own `@default` is `0`, which applies when the request DOES carry a
/// configuration but leaves the rate out.)
pub const DEFAULT_SESSION_SAMPLE_RATE: f64 = 0.1;

/// Prefix of the CloudWatch Logs group an app monitor copies telemetry into when
/// `CwLogEnabled` is set: RUM is a vended-logs service, so the group lives under
/// `/aws/vendedlogs/RUMService_`. The suffix AWS appends is not in the model, so this
/// emulator derives it from the monitor's own (account-unique) name, which keeps
/// `DataStorage.CwLog.CwLogGroup` stable across reads and distinct per monitor.
pub const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The `resourceType` this service's errors report for an app monitor.
pub const APP_MONITOR_RESOURCE_TYPE: &str = "AppMonitor";

/// The most app monitors this emulator holds at once.
///
/// `app_monitors` grows by one record per `CreateAppMonitor` request and shrinks only on
/// an explicit delete, so without a ceiling its entry count — and with it the heap this
/// emulator holds — is set by how many requests a caller makes. AWS bounds the same
/// thing with an adjustable per-account app-monitor quota whose value is not in the
/// model, so this number is the EMULATOR's own bound, while the refusal it produces is
/// the error `CreateAppMonitor` models for exactly this case:
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_APP_MONITORS: usize = 1000;

/// The most RUM events this emulator keeps per app monitor.
///
/// `PutRumEvents` appends to a monitor's buffer on every call, so the buffer needs a
/// ceiling to keep the emulator's heap bounded; past it the OLDEST event is dropped,
/// which is the same direction as the 30-day retention AWS documents for RUM-held data
/// (`CreateAppMonitorRequest$CwLogEnabled`: "Data collected by RUM is kept by RUM for 30
/// days and then deleted"). The count is this emulator's own bound, not AWS's.
pub const MAX_EVENTS_PER_APP_MONITOR: usize = 10_000;

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str = "the com-amazonaws-rum-rum store is not mounted on this engine";

/// One RUM event a `PutRumEvents` call appended to an app monitor's buffer.
///
/// The event itself is kept as the JSON document the wire carried (`event_json`), which
/// is the form `GetAppMonitorData` reports events in (`EventDataList` is a list of
/// `JsonValue`), while the per-batch context the event arrived with — the batch id, the
/// optional alias and the user session it came from — is kept beside it.
#[derive(Debug, Clone)]
pub struct StoredRumEvent {
    /// `PutRumEventsRequest$BatchId` of the call that delivered this event.
    pub batch_id: String,
    /// `PutRumEventsRequest$Alias`, when the call carried one.
    pub alias: Option<String>,
    /// `PutRumEventsRequest$AppMonitorDetails`: the sending web client's own view of the
    /// app monitor (name, id, version). Kept verbatim rather than re-derived from the
    /// stored record, because it is the CLIENT's report — `GetAppMonitorData` returns the
    /// events as they were received, and a client may name a version the server knows
    /// nothing about.
    pub app_monitor_details: AppMonitorDetails,
    /// `UserDetails$userId` of the session this event came from.
    pub user_id: Option<String>,
    /// `UserDetails$sessionId` of the session this event came from.
    pub session_id: Option<String>,
    /// The event as a JSON object: `id`, `timestamp`, `type`, `metadata`, `details`.
    pub event_json: String,
}

/// One persisted app monitor.
///
/// Mirrors the model's `AppMonitor` structure, but with the members the service always
/// knows held as non-optional: a stored monitor always has a name, an id, an ARN, both
/// timestamps, a state, a platform, a `CwLogEnabled` flag, a configuration and a custom
/// events setting. `Domain`/`DomainList` and `DeobfuscationConfiguration` stay optional
/// because a request may omit them.
#[derive(Debug, Clone)]
pub struct AppMonitorRecord {
    pub name: String,
    /// The minted `AppMonitorId`: a UUID, as `AppMonitorId`'s 36-character pattern asks.
    pub id: String,
    /// `arn:aws:rum:<region>:<account>:appmonitor/<name>`, derived once at create time.
    /// Nothing on the wire reports it yet — the tagging operations (`TagResource`,
    /// `UntagResource`, `ListTagsForResource`, which take a `ResourceArn`) are still
    /// scaffolded; it is stored now so those read one ARN rather than minting their own.
    pub arn: String,
    pub domain: Option<String>,
    pub domain_list: Option<Vec<String>>,
    /// `Created` as the modelled 19-character `ISOTimestampString`.
    pub created: String,
    /// `LastModified` as the modelled 19-character `ISOTimestampString`.
    pub last_modified: String,
    /// `State`, a member of [`crate::ops::types::STATE_ENUM_VALUES`].
    pub state: String,
    /// `Platform`, a member of [`crate::ops::types::APP_MONITOR_PLATFORM_VALUES`].
    pub platform: String,
    /// Whether RUM copies this monitor's telemetry into CloudWatch Logs; what
    /// `DataStorage.CwLog` reports.
    pub cw_log_enabled: bool,
    pub app_monitor_configuration: AppMonitorConfiguration,
    pub custom_events: CustomEvents,
    pub deobfuscation_configuration: Option<DeobfuscationConfiguration>,
    /// `Tags` as supplied at create time; the tagging operations are still scaffolded.
    pub tags: BTreeMap<String, String>,
    /// Events delivered by `PutRumEvents`, oldest first. Bounded by
    /// [`MAX_EVENTS_PER_APP_MONITOR`]; append through [`Self::record_events`].
    pub events: VecDeque<StoredRumEvent>,
}

impl AppMonitorRecord {
    /// The name of the CloudWatch Logs group this monitor's data is copied into, or
    /// `None` when `CwLogEnabled` is false — a monitor that stores no copy has no group
    /// (`CwLog$CwLogGroup` is "The name of the log group where the copies are stored").
    pub fn cw_log_group(&self) -> Option<String> {
        self.cw_log_enabled
            .then(|| format!("{CW_LOG_GROUP_PREFIX}{}", self.name))
    }

    /// Project the stored record onto the model's `AppMonitor` shape — the one
    /// conversion `GetAppMonitor` goes through, so what a read reports is exactly what
    /// the create and the updates wrote.
    pub fn to_app_monitor(&self) -> AppMonitor {
        AppMonitor {
            name: Some(self.name.clone()),
            domain: self.domain.clone(),
            domain_list: self.domain_list.clone(),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: Some(self.last_modified.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<HashMap<String, String>>(),
            ),
            state: Some(self.state.clone()),
            app_monitor_configuration: Some(self.app_monitor_configuration.clone()),
            data_storage: Some(DataStorage {
                cw_log: Some(CwLog {
                    cw_log_enabled: Some(self.cw_log_enabled),
                    cw_log_group: self.cw_log_group(),
                }),
            }),
            custom_events: Some(self.custom_events.clone()),
            deobfuscation_configuration: self.deobfuscation_configuration.clone(),
            platform: Some(self.platform.clone()),
        }
    }

    /// Project the stored record onto the model's `AppMonitorSummary` — the subset
    /// `ListAppMonitors` reports, read off the same record `GetAppMonitor` reads.
    pub fn to_summary(&self) -> AppMonitorSummary {
        AppMonitorSummary {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: Some(self.last_modified.clone()),
            state: Some(self.state.clone()),
            platform: Some(self.platform.clone()),
        }
    }

    /// Append events to this monitor's buffer, dropping the oldest once the buffer is at
    /// [`MAX_EVENTS_PER_APP_MONITOR`].
    pub fn record_events(&mut self, events: impl IntoIterator<Item = StoredRumEvent>) {
        for event in events {
            while self.events.len() >= MAX_EVENTS_PER_APP_MONITOR {
                self.events.pop_front();
            }
            self.events.push_back(event);
        }
    }
}

/// Per-service state for com-amazonaws-rum-rum. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRumRumState {
    /// Every app monitor that currently exists, keyed by its `Name` (unique per
    /// account). A `BTreeMap` so `ListAppMonitors` pages in a stable, resumable order.
    /// Bounded by [`MAX_APP_MONITORS`]; write it through [`Self::insert_app_monitor`] /
    /// [`Self::remove_app_monitor`] so the id index below cannot drift from it.
    pub app_monitors: BTreeMap<String, AppMonitorRecord>,
    /// Secondary index `Id` -> `Name`, so the data-plane `PutRumEvents` — which names a
    /// monitor by its minted id, not its name — resolves one in O(1) without scanning.
    pub monitor_ids: HashMap<String, String>,
    /// Per-state seed the minted ids are derived from, set on the first mint from the
    /// nanosecond clock so two runs of the emulator do not hand out the same ids. Only
    /// ever read by [`Self::mint_app_monitor_id`]; it is NOT an id itself.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state. Same note: not
    /// an id, and never reported.
    next_id_seq: u64,
}

impl ComAmazonawsRumRumState {
    /// Mint a fresh `AppMonitorId` that no stored monitor holds.
    ///
    /// The modelled shape is a UUID —
    /// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
    /// length 36..36 — so the id is 16 bytes of a SHA-256 digest of the clock-derived
    /// seed and the counter, laid out as a version-4, RFC 4122 variant UUID (the version
    /// and variant nibbles are forced, as a v4 generator does). The digest gives ids no
    /// caller can guess from a previous one, the seed keeps ids from different runs
    /// apart, the counter keeps ids within one run unique, and the loop re-draws rather
    /// than ever handing back an id already in use. The caller must store the record
    /// under the returned id while still holding the same state lock, or the id it
    /// returned names nothing.
    pub fn mint_app_monitor_id(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = nanos_since_epoch() as u64;
        }
        loop {
            self.next_id_seq = self.next_id_seq.wrapping_add(1);
            let candidate = uuid_v4_shaped(self.id_seed, self.next_id_seq);
            if !self.monitor_ids.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Store `record`, keeping the `Id` -> `Name` index in step with the primary map.
    pub fn insert_app_monitor(&mut self, record: AppMonitorRecord) {
        self.monitor_ids
            .insert(record.id.clone(), record.name.clone());
        self.app_monitors.insert(record.name.clone(), record);
    }

    /// Remove the monitor named `name` — with its tags and its event buffer, which live
    /// on the record — and drop its id from the index. `None` when no monitor has that
    /// name, which every caller renders as `ResourceNotFoundException`.
    pub fn remove_app_monitor(&mut self, name: &str) -> Option<AppMonitorRecord> {
        let removed = self.app_monitors.remove(name)?;
        self.monitor_ids.remove(&removed.id);
        Some(removed)
    }

    /// The stored monitor whose minted id is `id`, for a caller that mutates it (the
    /// data-plane `PutRumEvents`). The id index and the primary map are kept in step by
    /// the two writers above, so this is the ONE place the two-step lookup lives.
    pub fn app_monitor_by_id_mut(&mut self, id: &str) -> Option<&mut AppMonitorRecord> {
        let name = self.monitor_ids.get(id)?.clone();
        self.app_monitors.get_mut(&name)
    }
}

/// The current nanosecond clock, the seed [`ComAmazonawsRumRumState::mint_app_monitor_id`]
/// draws from once per state.
fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// One `(seed, sequence)` pair as a canonical, lowercase, version-4-shaped UUID string.
///
/// The 16 bytes come from a SHA-256 digest of the pair, then byte 6's high nibble is set
/// to `4` (the version) and byte 8's two high bits to `10` (the RFC 4122 variant), which
/// is exactly the layout a v4 generator writes. Distinct pairs give distinct ids: the
/// digest is injective in practice and the sequence is strictly increasing.
fn uuid_v4_shaped(seed: u64, sequence: u64) -> String {
    use sha2::{Digest as _, Sha256};

    let digest = Sha256::digest(format!("rum/appmonitor/{seed}/{sequence}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The ARN of the app monitor named `name` in `region`.
pub fn app_monitor_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APP_MONITOR_RESOURCE}/{name}"),
    )
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An instant as the modelled `ISOTimestampString`: `YYYY-MM-DDTHH:MM:SS`, exactly the
/// 19 characters that shape's `@length(min: 19, max: 19)` admits — RFC 3339 truncated to
/// whole seconds and with no zone suffix, which is how RUM renders `Created` and
/// `LastModified`.
pub fn iso_timestamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    let rendered = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let trimmed = rendered.strip_suffix('Z').unwrap_or(rendered.as_str());
    if trimmed.len() == 19 {
        trimmed.to_string()
    } else {
        // Only reachable for an instant RFC 3339 cannot render (a clock outside year
        // 0001..9999). The shape admits 19 characters and nothing else, so answer the
        // epoch rather than a string no client can parse.
        "1970-01-01T00:00:00".to_string()
    }
}

/// Now, as the modelled `ISOTimestampString`.
pub fn current_iso_timestamp() -> String {
    iso_timestamp(SystemTime::now())
}

/// The configuration an app monitor gets when its create request carried none: every
/// member unset except the documented 10% session sample rate.
pub fn default_app_monitor_configuration() -> AppMonitorConfiguration {
    AppMonitorConfiguration {
        session_sample_rate: DEFAULT_SESSION_SAMPLE_RATE,
        ..AppMonitorConfiguration::default()
    }
}

/// The custom-events setting an app monitor gets when its create request carried none.
pub fn default_custom_events() -> CustomEvents {
    CustomEvents {
        status: Some(DEFAULT_CUSTOM_EVENTS_STATUS.to_string()),
    }
}

/// Why this request's `Domain`/`DomainList` pair cannot be applied, or `None` when it
/// can.
///
/// From the model: `CreateAppMonitorRequest$DomainList` documents "The CreateAppMonitor
/// requires either the domain or the domain list" and
/// `UpdateAppMonitorRequest$DomainList` documents "The UpdateAppMonitor allows either the
/// domain or the domain list" — a monitor's domain authority is held in exactly ONE of
/// the two forms. Neither constraint is expressible in the model, so every operation that
/// accepts the pair refuses it here with the `ValidationException` it declares.
/// `required` is true for the operation that must end up with one (a create), false for
/// one that may leave the stored choice alone (an update).
pub fn domain_choice_refusal(
    domain: Option<&str>,
    domain_list: Option<&[String]>,
    required: bool,
) -> Option<String> {
    match (domain, domain_list) {
        (Some(_), Some(_)) => Some(
            "Domain and DomainList cannot both be specified; name either the domain or \
             the domain list."
                .to_string(),
        ),
        (None, None) if required => {
            Some("Either Domain or DomainList must be specified.".to_string())
        }
        _ => None,
    }
}

/// Why this `AppMonitorConfiguration` cannot be applied, or `None` when it can.
///
/// From the model: `AppMonitorConfiguration$IncludedPages` and `$ExcludedPages` both
/// document "You can't include both `ExcludedPages` and `IncludedPages` in the same
/// operation", a rule no modelled constraint expresses, so every operation that accepts
/// a configuration refuses it here with the `ValidationException` it declares.
pub fn app_monitor_configuration_refusal(config: &AppMonitorConfiguration) -> Option<String> {
    let includes = config.included_pages.as_ref().is_some_and(|p| !p.is_empty());
    let excludes = config.excluded_pages.as_ref().is_some_and(|p| !p.is_empty());
    (includes && excludes).then(|| {
        "AppMonitorConfiguration cannot carry both IncludedPages and ExcludedPages."
            .to_string()
    })
}

/// `ResourceNotFoundException` for a name or id that names no stored app monitor. The
/// message is built from the request, so it names the value given.
pub fn app_monitor_not_found(name_or_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("App monitor {name_or_id} does not exist."),
        resource_name: name_or_id.to_string(),
        resource_type: Some(APP_MONITOR_RESOURCE_TYPE.to_string()),
    }
}

/// `ConflictException` for a create whose `Name` a stored app monitor already holds.
pub fn app_monitor_already_exists(name: &str) -> ConflictException {
    ConflictException {
        message: format!("App monitor {name} already exists."),
        resource_name: name.to_string(),
        resource_type: Some(APP_MONITOR_RESOURCE_TYPE.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRumRumState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRumRumState>)
}
