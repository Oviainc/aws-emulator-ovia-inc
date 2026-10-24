// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-internetmonitor-internetmonitor20210603 service.
//!
//! The service's primary resource is the **monitor**: `CreateMonitor` stores one here and
//! every other monitor operation (`GetMonitor`, `ListMonitors`, `UpdateMonitor`,
//! `DeleteMonitor`, `TagResource`, `UntagResource`, `ListTagsForResource`) computes its
//! answer from this one store, so a create is observable from a get/list and a delete is
//! observable as an absence.
//!
//! A monitor's **health events** live beside it, in one store keyed by
//! `(MonitorName, EventId)`, so `ListHealthEvents` reports exactly the events recorded for
//! the monitor it names and a monitor nothing has recorded an event for reports none.
//!
//! The small helpers below (region, ARN, wire timestamp, page token) live beside the
//! store rather than in one operation's file because several handlers need exactly the
//! same rule and two copies of it would be two rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{HealthEventsConfig, ImpactedLocation, InternetMeasurementsLogDelivery};

/// `MonitorConfigState` members, as the model spells them (see
/// `crate::ops::types::MONITOR_CONFIG_STATE_VALUES`).
pub const MONITOR_STATE_PENDING: &str = "PENDING";
/// See [`MONITOR_STATE_PENDING`].
pub const MONITOR_STATE_ACTIVE: &str = "ACTIVE";
/// See [`MONITOR_STATE_PENDING`].
pub const MONITOR_STATE_INACTIVE: &str = "INACTIVE";

/// `MonitorProcessingStatusCode` members used by this slice (see
/// `crate::ops::types::MONITOR_PROCESSING_STATUS_CODE_VALUES`).
pub const PROCESSING_STATUS_COLLECTING_DATA: &str = "COLLECTING_DATA";
/// See [`PROCESSING_STATUS_COLLECTING_DATA`].
pub const PROCESSING_STATUS_INACTIVE: &str = "INACTIVE";

/// `HealthEventStatus` members, as the model spells them (see
/// `crate::ops::types::HEALTH_EVENT_STATUS_VALUES`).
pub const HEALTH_EVENT_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`HEALTH_EVENT_STATUS_ACTIVE`].
pub const HEALTH_EVENT_STATUS_RESOLVED: &str = "RESOLVED";

/// The region an emulated ARN carries when the request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace of this service, as `OPERATIONS.md` records it.
pub const ARN_NAMESPACE: &str = "internetmonitor";

/// How long a freshly created monitor reports `PENDING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateMonitor` returns a monitor in the non-terminal `PENDING` state, so the
/// transition to `ACTIVE` is owed by the clock rather than by any later call. The span is
/// chosen (AWS documents no duration for it) and is deliberately non-zero, so a caller
/// polling immediately after the create still observes `PENDING`.
pub const MONITOR_ACTIVATION_DELAY: Duration = Duration::from_millis(400);

/// The most monitors this emulator holds for its one account.
///
/// A bound on the store, not a claim about an AWS quota: the store lives as long as the
/// process and grows one record per `CreateMonitor`, so the request path needs a ceiling.
/// `CreateMonitor` answers its own modelled `LimitExceededException` (HTTP 403, "The
/// request exceeded a service quota") at the ceiling rather than growing without limit.
pub const MAX_MONITORS: usize = 200;

/// The most `ClientToken` records this emulator keeps per operation.
///
/// The backstop that bounds the idempotency maps even inside one retention window: a
/// caller can send a fresh token on every `UpdateMonitor` call, and an update creates no
/// resource whose deletion would release the record.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// How long a `ClientToken` record is honoured before the clock expires it.
///
/// A chosen window (AWS documents no retention for this service's idempotency tokens),
/// long enough that a client's own retries are still de-duplicated.
pub const CLIENT_TOKEN_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The most resources one monitor holds.
///
/// `Resources` / `ResourcesToAdd` are `SetOfARNs` lists the model gives no length
/// constraint, and the set they merge into lives as long as the process, so the per-record
/// collection needs a ceiling just as the record count does. A bound on this emulator's
/// store, not a claim about an AWS quota: `CreateMonitor` and `UpdateMonitor` answer their
/// modelled `LimitExceededException` (HTTP 403, "The request exceeded a service quota")
/// rather than growing past it.
pub const MAX_RESOURCES_PER_MONITOR: usize = 100;

/// The most tags one monitor holds, which the model states: "The tags for a monitor. You
/// can add a maximum of 50 tags in Internet Monitor."
/// (`smithy_query(query="doc",shape="CreateMonitorInput",member="Tags")`). `TagMap`'s own
/// 0..200 length bounds one REQUEST, not the stored map that `TagResource` merges into.
pub const MAX_TAGS_PER_MONITOR: usize = 50;

/// The most health events this emulator holds for one monitor.
///
/// A bound on the store, not a claim about an AWS quota: [`HealthEventRecord`]s live as
/// long as the process, so the collection needs a ceiling just as the monitor store does.
/// [`ComAmazonawsInternetmonitorInternetmonitor20210603State::record_health_event`]
/// refuses a record past the ceiling rather than growing without limit.
pub const MAX_HEALTH_EVENTS_PER_MONITOR: usize = 100;

/// One idempotency record: the monitor a `ClientToken` acted on, and when it did, so the
/// record can be expired and evicted.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The `MonitorName` the call carrying this token created or updated.
    pub monitor_name: String,
    /// When the token was recorded.
    pub recorded_at: SystemTime,
}

/// One stored monitor: every member `GetMonitor` reports, plus the instant at which the
/// clock owes this monitor its `PENDING` -> `ACTIVE` transition.
#[derive(Debug, Clone)]
pub struct MonitorRecord {
    /// `MonitorName` — the store's key, kept on the record so a value-only scan can
    /// report it.
    pub name: String,
    /// `MonitorArn`, built by [`monitor_arn`].
    pub arn: String,
    /// `Resources`. A `SetOfARNs` in the model, so it is stored as a set: duplicates
    /// collapse and `UpdateMonitor`'s add/remove are a union/difference.
    pub resources: BTreeSet<String>,
    /// `Status`, one of [`MONITOR_STATE_PENDING`] / [`MONITOR_STATE_ACTIVE`] /
    /// [`MONITOR_STATE_INACTIVE`].
    pub status: String,
    /// `ProcessingStatus`.
    pub processing_status: Option<String>,
    /// `ProcessingStatusInfo`.
    pub processing_status_info: Option<String>,
    /// `CreatedAt`.
    pub created_at: SystemTime,
    /// `ModifiedAt`, bumped by every call that mutates the record.
    pub modified_at: SystemTime,
    /// When set, the instant from which this monitor's `PENDING` -> `ACTIVE` transition
    /// is due; [`MonitorRecord::settle`] materialises it. `None` once the monitor has
    /// reached a state the clock owes nothing from.
    pub active_at: Option<SystemTime>,
    /// `Tags`, shared by `GetMonitor`, `TagResource`, `UntagResource` and
    /// `ListTagsForResource`.
    pub tags: BTreeMap<String, String>,
    /// `MaxCityNetworksToMonitor`.
    pub max_city_networks_to_monitor: Option<i32>,
    /// `TrafficPercentageToMonitor`.
    pub traffic_percentage_to_monitor: Option<i32>,
    /// `InternetMeasurementsLogDelivery`.
    pub internet_measurements_log_delivery: Option<InternetMeasurementsLogDelivery>,
    /// `HealthEventsConfig`.
    pub health_events_config: Option<HealthEventsConfig>,
}

impl MonitorRecord {
    /// Materialise the transition the clock already owed at `now`.
    ///
    /// A reporting operation calls this before it reads: the monitor would have been
    /// `ACTIVE` at this instant whether or not anybody called, so making it so is not a
    /// mutation driven by the read. It never moves a monitor an explicit call has put
    /// into another state, because such a call clears [`MonitorRecord::active_at`].
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.active_at {
            if now >= due {
                self.status = MONITOR_STATE_ACTIVE.to_string();
                self.active_at = None;
            }
        }
    }

    /// The `ProcessingStatus` a monitor in `status` reports: a monitor that is not
    /// configured to run is not processing data, so it reports `INACTIVE`; otherwise it
    /// is still gathering its first measurements (`COLLECTING_DATA`).
    pub fn processing_status_for(status: &str) -> String {
        if status == MONITOR_STATE_INACTIVE {
            PROCESSING_STATUS_INACTIVE.to_string()
        } else {
            PROCESSING_STATUS_COLLECTING_DATA.to_string()
        }
    }
}

/// One stored health event of one monitor: every member a health-event report carries.
///
/// Internet Monitor raises these itself, from the internet-measurement pipeline it runs
/// behind a monitor, and its API has no operation that creates one — so this store holds
/// exactly the events something recorded here, and a monitor nothing has recorded an event
/// for has none. `Status` is NOT stored: it follows from whether the event has ended (see
/// [`HealthEventRecord::status`]), so one record cannot disagree with itself.
#[derive(Debug, Clone)]
pub struct HealthEventRecord {
    /// The `MonitorName` this event belongs to — half of the store's key, kept on the
    /// record so a value-only scan can report it.
    pub monitor_name: String,
    /// `EventId`, the other half of the store's key.
    pub event_id: String,
    /// `EventArn`, built by [`health_event_arn`].
    pub arn: String,
    /// `StartedAt`.
    pub started_at: SystemTime,
    /// `EndedAt`: unset while the event is still ongoing, which is also what makes it
    /// report `ACTIVE`.
    pub ended_at: Option<SystemTime>,
    /// `CreatedAt`.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`.
    pub last_updated_at: SystemTime,
    /// `ImpactedLocations`.
    pub impacted_locations: Vec<ImpactedLocation>,
    /// `ImpactType`, one of `crate::ops::types::HEALTH_EVENT_IMPACT_TYPE_VALUES`.
    pub impact_type: String,
    /// `PercentOfTotalTrafficImpacted`.
    pub percent_of_total_traffic_impacted: Option<f64>,
    /// `HealthScoreThreshold`, whose modelled default is 0.
    pub health_score_threshold: f64,
}

impl HealthEventRecord {
    /// A record for `event_id` of `monitor_name`, started at `started_at` and still
    /// ongoing, with the ARN this service mints for it. A caller then sets the members
    /// its event actually carries.
    pub fn new(
        region: &str,
        monitor_name: &str,
        event_id: &str,
        impact_type: &str,
        started_at: SystemTime,
    ) -> Self {
        Self {
            monitor_name: monitor_name.to_string(),
            event_id: event_id.to_string(),
            arn: health_event_arn(region, monitor_name, event_id),
            started_at,
            ended_at: None,
            created_at: started_at,
            last_updated_at: started_at,
            impacted_locations: Vec::new(),
            impact_type: impact_type.to_string(),
            percent_of_total_traffic_impacted: None,
            health_score_threshold: 0.0,
        }
    }

    /// `Status`: an event that has not ended is still `ACTIVE`, and one that has is
    /// `RESOLVED`.
    pub fn status(&self) -> &'static str {
        if self.ended_at.is_some() {
            HEALTH_EVENT_STATUS_RESOLVED
        } else {
            HEALTH_EVENT_STATUS_ACTIVE
        }
    }

    /// The instant this event's own interval ends: when it ended, or `now` for an event
    /// that is still ongoing — never before it started.
    pub fn interval_end(&self, now: SystemTime) -> SystemTime {
        self.ended_at.unwrap_or(now).max(self.started_at)
    }
}

/// Per-service state for com-amazonaws-internetmonitor-internetmonitor20210603. A handler reaches it through [`service_state`].
///
/// Every map here is explicitly bounded, because this store lives for the whole life of
/// the process and grows on the request path: `monitors` is capped at [`MAX_MONITORS`]
/// (`CreateMonitor` answers its modelled `LimitExceededException` at the cap), and the two
/// idempotency maps are capped at [`MAX_CLIENT_TOKENS`] entries each and expire after
/// [`CLIENT_TOKEN_RETENTION`] (see [`Self::record_create_token`]).
#[derive(Debug, Default)]
pub struct ComAmazonawsInternetmonitorInternetmonitor20210603State {
    /// Every monitor, keyed by `MonitorName`. A `BTreeMap` so `ListMonitors` pages over a
    /// stable total order and its `NextToken` can name a resume point. Bounded by
    /// [`MAX_MONITORS`].
    pub monitors: BTreeMap<String, MonitorRecord>,
    /// `CreateMonitor` `ClientToken` -> the monitor that call created. Bounded; reach it
    /// through [`Self::record_create_token`] / [`Self::create_token_target`].
    pub create_client_tokens: HashMap<String, ClientTokenRecord>,
    /// `UpdateMonitor` `ClientToken` -> the monitor that call updated. Kept apart from
    /// [`Self::create_client_tokens`] so one token reused across the two operations cannot
    /// make an update look already-applied. Bounded the same way.
    pub update_client_tokens: HashMap<String, ClientTokenRecord>,
    /// Every health event, keyed by `(MonitorName, EventId)`. A `BTreeMap` so one
    /// monitor's events are a contiguous range in a stable total order, which is what lets
    /// `ListHealthEvents` page over them with a `NextToken` that names a resume point.
    /// Bounded by [`MAX_HEALTH_EVENTS_PER_MONITOR`] per monitor; reach it through
    /// [`Self::health_events_for`] / [`Self::record_health_event`].
    pub health_events: BTreeMap<(String, String), HealthEventRecord>,
}

impl ComAmazonawsInternetmonitorInternetmonitor20210603State {
    /// Materialise every clock-owed monitor transition due at `now`.
    pub fn settle_all(&mut self, now: SystemTime) {
        for monitor in self.monitors.values_mut() {
            monitor.settle(now);
        }
    }

    /// The monitor whose ARN is `arn`, for the tagging operations that identify a monitor
    /// by ARN rather than by name. One lookup, so a caller needs no second one and has one
    /// place to answer `NotFoundException` from.
    pub fn monitor_by_arn(&self, arn: &str) -> Option<&MonitorRecord> {
        self.monitors.values().find(|monitor| monitor.arn == arn)
    }

    /// [`Self::monitor_by_arn`], for a caller that mutates what it finds.
    pub fn monitor_by_arn_mut(&mut self, arn: &str) -> Option<&mut MonitorRecord> {
        self.monitors.values_mut().find(|monitor| monitor.arn == arn)
    }

    /// The monitor a `CreateMonitor` `ClientToken` already created, if that token is still
    /// held.
    pub fn create_token_target(&self, token: &str) -> Option<String> {
        self.create_client_tokens
            .get(token)
            .map(|record| record.monitor_name.clone())
    }

    /// The monitor an `UpdateMonitor` `ClientToken` already updated, if that token is
    /// still held.
    pub fn update_token_target(&self, token: &str) -> Option<String> {
        self.update_client_tokens
            .get(token)
            .map(|record| record.monitor_name.clone())
    }

    /// Remember that `token` created `monitor_name` at `now`, expiring and evicting so the
    /// map stays bounded.
    pub fn record_create_token(&mut self, token: String, monitor_name: String, now: SystemTime) {
        Self::record_token(&mut self.create_client_tokens, token, monitor_name, now);
    }

    /// Remember that `token` updated `monitor_name` at `now`, expiring and evicting so the
    /// map stays bounded.
    pub fn record_update_token(&mut self, token: String, monitor_name: String, now: SystemTime) {
        Self::record_token(&mut self.update_client_tokens, token, monitor_name, now);
    }

    /// Every health event of `monitor_name`, in `EventId` order.
    pub fn health_events_for<'a>(
        &'a self,
        monitor_name: &'a str,
    ) -> impl Iterator<Item = &'a HealthEventRecord> + 'a {
        self.health_events
            .range((monitor_name.to_string(), String::new())..)
            .take_while(move |((name, _), _)| name == monitor_name)
            .map(|(_, record)| record)
    }

    /// Store `record`, replacing any earlier record of the same event of the same monitor.
    ///
    /// Answers `false`, storing nothing, when the monitor already holds
    /// [`MAX_HEALTH_EVENTS_PER_MONITOR`] other events: the store is bounded.
    pub fn record_health_event(&mut self, record: HealthEventRecord) -> bool {
        let key = (record.monitor_name.clone(), record.event_id.clone());
        if !self.health_events.contains_key(&key)
            && self.health_events_for(&record.monitor_name).count() >= MAX_HEALTH_EVENTS_PER_MONITOR
        {
            return false;
        }
        self.health_events.insert(key, record);
        true
    }

    /// Forget every health event of `monitor_name`, for a monitor that no longer exists:
    /// its events are no longer reportable, and a monitor later created with the same name
    /// is a new monitor that has had none.
    pub fn forget_health_events_for(&mut self, monitor_name: &str) {
        self.health_events
            .retain(|(name, _), _| name != monitor_name);
    }

    /// Forget every idempotency record that names `monitor_name`, for a monitor that no
    /// longer exists and so can no longer be reported.
    pub fn forget_tokens_for(&mut self, monitor_name: &str) {
        self.create_client_tokens
            .retain(|_, record| record.monitor_name != monitor_name);
        self.update_client_tokens
            .retain(|_, record| record.monitor_name != monitor_name);
    }

    /// Drop every idempotency record the clock has already expired at `now`, then, if the
    /// map is still over [`MAX_CLIENT_TOKENS`], the oldest records until it is not.
    ///
    /// Expiring a record the retention window already ended is owed by the clock, not by
    /// this call: the record would have been gone at this instant whether or not anybody
    /// called. The size cap is the backstop that makes the map bounded even inside one
    /// retention window.
    pub fn prune_client_tokens(&mut self, now: SystemTime) {
        Self::prune(&mut self.create_client_tokens, now);
        Self::prune(&mut self.update_client_tokens, now);
    }

    fn record_token(
        tokens: &mut HashMap<String, ClientTokenRecord>,
        token: String,
        monitor_name: String,
        now: SystemTime,
    ) {
        // Make room for the record that is about to exist, rather than inserting it and
        // then evicting again (which could evict the new record itself): pruning to one
        // below the cap leaves the map at [`MAX_CLIENT_TOKENS`] once the insert lands.
        Self::prune_to(tokens, now, MAX_CLIENT_TOKENS.saturating_sub(1));
        tokens.insert(
            token,
            ClientTokenRecord {
                monitor_name,
                recorded_at: now,
            },
        );
    }

    fn prune(tokens: &mut HashMap<String, ClientTokenRecord>, now: SystemTime) {
        Self::prune_to(tokens, now, MAX_CLIENT_TOKENS);
    }

    fn prune_to(tokens: &mut HashMap<String, ClientTokenRecord>, now: SystemTime, cap: usize) {
        tokens.retain(|_, record| {
            now.duration_since(record.recorded_at)
                .is_ok_and(|age| age < CLIENT_TOKEN_RETENTION)
                || record.recorded_at > now
        });
        if tokens.len() <= cap {
            return;
        }
        let mut oldest_first: Vec<(SystemTime, String)> = tokens
            .iter()
            .map(|(token, record)| (record.recorded_at, token.clone()))
            .collect();
        oldest_first.sort();
        let excess = tokens.len() - cap;
        for (_, token) in oldest_first.into_iter().take(excess) {
            tokens.remove(&token);
        }
    }
}

/// The region the request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A monitor's ARN: `arn:aws:internetmonitor:<region>:<account>:monitor/<MonitorName>`.
pub fn monitor_arn(region: &str, monitor_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("monitor/{monitor_name}"))
}

/// A health event's ARN:
/// `arn:aws:internetmonitor:<region>:<account>:monitor/<MonitorName>/health-event/<EventId>`
/// — an event is a child of its monitor, so its ARN extends the monitor's resource path.
pub fn health_event_arn(region: &str, monitor_name: &str, event_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("monitor/{monitor_name}/health-event/{event_id}"),
    )
}

/// A stored instant as the wire timestamp the model's `Timestamp` members carry.
pub fn wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A listing's `NextToken`: the opaque form of "resume after this key" — the
/// `MonitorName` a `ListMonitors` page ended on, the `EventId` a `ListHealthEvents` page
/// ended on.
///
/// Hex (the `hex` crate) rather than a hand-rolled scheme, and opaque to the caller by
/// construction: the only contract is that [`decode_page_token`] round-trips it.
pub fn encode_page_token(after_key: &str) -> String {
    ::hex::encode(after_key.as_bytes())
}

/// The key a `NextToken` resumes after, or `None` when the token was not produced by
/// [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInternetmonitorInternetmonitor20210603State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInternetmonitorInternetmonitor20210603State>)
}
