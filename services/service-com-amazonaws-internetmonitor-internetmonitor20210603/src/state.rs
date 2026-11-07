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
//! The service's **internet events** (see [`InternetEventRecord`]), which
//! `ListInternetEvents` and `GetInternetEvent` report, live in their own store beside
//! them, holding the feed [`internet_event_feed`] observes for the service itself (an
//! internet event is a global observation every account reads, not a resource a caller
//! creates).
//!
//! The query interface's **queries** (see [`QueryRecord`]) live in one store keyed by
//! `QueryId`: `StartQuery` mints one, `GetQueryStatus` reports its status and `StopQuery`
//! cancels it, so a started query is observable from a poll and a cancel is observable in
//! what the next poll reports.
//!
//! The small helpers below (region, ARN, wire timestamp, epoch seconds, page token) live
//! beside the store rather than in one operation's file because several handlers need
//! exactly the same rule and two copies of it would be two rules.

mod internet_event_feed;

pub use internet_event_feed::{
    seed_event_id, seed_internet_events, INTERNET_EVENT_FEED_WINDOW, SEEDED_INTERNET_EVENTS,
};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ClientLocation, FilterParameter, HealthEventsConfig, ImpactedLocation,
    InternetMeasurementsLogDelivery, ValidationException,
};

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

/// `InternetEventStatus` members, as the model spells them (see
/// `crate::ops::types::INTERNET_EVENT_STATUS_VALUES`).
pub const INTERNET_EVENT_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`INTERNET_EVENT_STATUS_ACTIVE`].
pub const INTERNET_EVENT_STATUS_RESOLVED: &str = "RESOLVED";

/// The most internet events this emulator holds.
///
/// A bound on the store, not a claim about an AWS quota: the store lives as long as the
/// process, so [`ComAmazonawsInternetmonitorInternetmonitor20210603State::record_internet_event`]
/// keeps the most recently started events and drops the rest once the ceiling is reached.
pub const MAX_INTERNET_EVENTS: usize = 1024;

/// The most health events this emulator holds for one monitor.
///
/// A bound on the store, not a claim about an AWS quota: [`HealthEventRecord`]s live as
/// long as the process, so the collection needs a ceiling just as the monitor store does.
/// [`ComAmazonawsInternetmonitorInternetmonitor20210603State::record_health_event`]
/// refuses a record past the ceiling rather than growing without limit.
pub const MAX_HEALTH_EVENTS_PER_MONITOR: usize = 100;

/// `QueryStatus` members, as the model spells them (see
/// `crate::ops::types::QUERY_STATUS_VALUES`), with the meaning the model documents for
/// each: `QUEUED` is scheduled to run, `RUNNING` in progress, `SUCCEEDED` complete,
/// `FAILED` ended in an error and `CANCELED` stopped by its caller.
pub const QUERY_STATUS_QUEUED: &str = "QUEUED";
/// See [`QUERY_STATUS_QUEUED`].
pub const QUERY_STATUS_RUNNING: &str = "RUNNING";
/// See [`QUERY_STATUS_QUEUED`].
pub const QUERY_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`QUERY_STATUS_QUEUED`].
pub const QUERY_STATUS_FAILED: &str = "FAILED";
/// See [`QUERY_STATUS_QUEUED`].
pub const QUERY_STATUS_CANCELED: &str = "CANCELED";

/// How long each step of a started query's lifecycle takes before the clock owes the next
/// one: `QUEUED` -> `RUNNING` after one span, `RUNNING` -> `SUCCEEDED` after the second.
///
/// `StartQuery` answers with the query in the non-terminal `QUEUED` state, so both
/// transitions are owed by the clock rather than by any later call. The span is chosen
/// (AWS documents no duration for it) and is deliberately non-zero, so a caller polling
/// immediately after the start still observes `QUEUED`.
pub const QUERY_STEP_DELAY: Duration = Duration::from_secs(3);

/// The most queries this emulator holds.
///
/// A bound on the store, not a claim about an AWS quota: the store lives as long as the
/// process and grows one record per `StartQuery`, so
/// [`ComAmazonawsInternetmonitorInternetmonitor20210603State::record_query`] evicts the
/// queries started longest ago rather than growing without limit.
pub const MAX_QUERIES: usize = 256;

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

/// One started query of the query interface: which monitor it queries, what it asked for,
/// and where it is in the lifecycle the model documents (`QUEUED` -> `RUNNING` ->
/// `SUCCEEDED`).
///
/// `StartQuery` mints one of these — the model documents it as starting a query and
/// returning "the internally-generated identifier of a specific query" — and
/// `GetQueryStatus` reports it, `StopQuery` cancels it. The two instants below are the
/// clock-owed transitions, materialised by [`QueryRecord::settle`] rather than by a timer.
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// `QueryId` — the store's key, kept on the record so a value-only scan can report it.
    pub query_id: String,
    /// The `MonitorName` the query runs against: a `QueryId` is reportable only through
    /// the monitor it was started for, so both halves have to match for a lookup to
    /// resolve (see
    /// [`ComAmazonawsInternetmonitorInternetmonitor20210603State::query_for`]).
    pub monitor_name: String,
    /// `QueryType`, one of `crate::ops::types::QUERY_TYPE_VALUES`.
    pub query_type: String,
    /// The requested `StartTime`, on the scale [`epoch_seconds`] uses.
    pub start_time: f64,
    /// The requested `EndTime`, on the same scale as [`Self::start_time`].
    pub end_time: f64,
    /// The `FilterParameters` the request carried, which define what the query returns.
    pub filter_parameters: Vec<FilterParameter>,
    /// `Status`, one of [`QUERY_STATUS_QUEUED`] / [`QUERY_STATUS_RUNNING`] /
    /// [`QUERY_STATUS_SUCCEEDED`] / [`QUERY_STATUS_FAILED`] / [`QUERY_STATUS_CANCELED`].
    pub status: String,
    /// When the query was started.
    pub created_at: SystemTime,
    /// When set, the instant from which this query's `QUEUED` -> `RUNNING` transition is
    /// due. `None` once it is no longer owed.
    pub running_at: Option<SystemTime>,
    /// When set, the instant from which this query's `RUNNING` -> `SUCCEEDED` transition
    /// is due. `None` once it is no longer owed.
    pub succeeded_at: Option<SystemTime>,
}

impl QueryRecord {
    /// A freshly started query of `monitor_name`, `QUEUED` and owed both of the
    /// transitions the model's lifecycle documents.
    pub fn new(
        query_id: impl Into<String>,
        monitor_name: impl Into<String>,
        query_type: impl Into<String>,
        start_time: f64,
        end_time: f64,
        filter_parameters: Vec<FilterParameter>,
        now: SystemTime,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            monitor_name: monitor_name.into(),
            query_type: query_type.into(),
            start_time,
            end_time,
            filter_parameters,
            status: QUERY_STATUS_QUEUED.to_string(),
            created_at: now,
            running_at: Some(now + QUERY_STEP_DELAY),
            succeeded_at: Some(now + QUERY_STEP_DELAY + QUERY_STEP_DELAY),
        }
    }

    /// Materialise the transitions the clock already owed at `now`.
    ///
    /// A reporting operation calls this before it reads, for the same reason
    /// [`MonitorRecord::settle`] exists: the query would have reached this status at this
    /// instant whether or not anybody polled. A query that has reached a terminal status
    /// is owed nothing, so nothing moves it again.
    pub fn settle(&mut self, now: SystemTime) {
        if self.is_terminal() {
            return;
        }
        if let Some(due) = self.running_at {
            if now >= due {
                self.status = QUERY_STATUS_RUNNING.to_string();
                self.running_at = None;
            }
        }
        if let Some(due) = self.succeeded_at {
            if now >= due {
                self.status = QUERY_STATUS_SUCCEEDED.to_string();
                self.succeeded_at = None;
            }
        }
    }

    /// Whether this query has reached a status nothing moves it out of.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            QUERY_STATUS_SUCCEEDED | QUERY_STATUS_FAILED | QUERY_STATUS_CANCELED
        )
    }

    /// Stop a query that is still in progress, which is what `StopQuery` does ("Stop a
    /// query that is progress for a specific monitor"): it becomes `CANCELED` and the
    /// clock owes it nothing further. A query that has already reached a terminal status
    /// keeps it — stopping cannot un-finish work that is done.
    pub fn cancel(&mut self) {
        if self.is_terminal() {
            return;
        }
        self.status = QUERY_STATUS_CANCELED.to_string();
        self.running_at = None;
        self.succeeded_at = None;
    }
}

/// One internet event the service knows about: an issue on the public internet that
/// degrades performance or availability for a client location.
///
/// Internet events are the service's own observations of the global internet, never
/// something a caller creates: the model declares no operation that creates, updates or
/// deletes one, and both operations that report them (`ListInternetEvents`,
/// `GetInternetEvent`) are `@readonly`. The feed every account reads is therefore a
/// property of the service, held in this store from the moment it exists (see
/// [`seed_internet_events`]), and the reporting operations answer from exactly the records
/// it holds.
#[derive(Debug, Clone)]
pub struct InternetEventRecord {
    /// `EventId` — the store's key, kept on the record so a value-only scan can report
    /// it.
    pub event_id: String,
    /// `EventArn`, built by [`internet_event_arn`].
    pub event_arn: String,
    /// `StartedAt`: when the event started. Every time filter in
    /// `ListInternetEvents` bounds THIS instant ("Both start time and end time specify
    /// the time when an event started").
    pub started_at: SystemTime,
    /// `EndedAt`, which the model leaves empty until the event ends: "The time when an
    /// internet event ended. If the event hasn't ended yet, this value is empty."
    /// [`InternetEventRecord::status`] is derived from it.
    pub ended_at: Option<SystemTime>,
    /// `ClientLocation`: the impacted location.
    pub client_location: ClientLocation,
    /// `EventType`, one of `crate::ops::types::INTERNET_EVENT_TYPE_VALUES`.
    pub event_type: String,
}

impl InternetEventRecord {
    /// A record for the event `event_id`, with its ARN built once by
    /// [`internet_event_arn`] so every reporting operation reports the same one.
    pub fn new(
        event_id: impl Into<String>,
        started_at: SystemTime,
        ended_at: Option<SystemTime>,
        client_location: ClientLocation,
        event_type: impl Into<String>,
    ) -> Self {
        let event_id = event_id.into();
        Self {
            event_arn: internet_event_arn(&event_id),
            event_id,
            started_at,
            ended_at,
            client_location,
            event_type: event_type.into(),
        }
    }

    /// `EventStatus`, derived rather than stored: an event the model still reports with no
    /// `EndedAt` has not ended, so it is `ACTIVE`; one that has an end instant is
    /// `RESOLVED`. Deriving it is what keeps the two members from disagreeing.
    pub fn status(&self) -> &'static str {
        if self.ended_at.is_some() {
            INTERNET_EVENT_STATUS_RESOLVED
        } else {
            INTERNET_EVENT_STATUS_ACTIVE
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
///
/// A fresh store holds no monitor, no query and no health event — those exist only once a
/// caller creates them — but it DOES hold the service's own internet-event feed, which no
/// caller creates (see [`Default`] and [`seed_internet_events`]).
#[derive(Debug)]
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
    /// Every internet event the service knows about, keyed by `EventId`. A `BTreeMap` so
    /// `ListInternetEvents` pages over a stable total order and its `NextToken` can name a
    /// resume point. Bounded by [`MAX_INTERNET_EVENTS`] and written through
    /// [`Self::record_internet_event`]; a fresh store already holds the service's own feed
    /// (see [`seed_internet_events`]), because an internet event is an observation the
    /// service makes of the public internet rather than anything a caller creates (see
    /// [`InternetEventRecord`]).
    pub internet_events: BTreeMap<String, InternetEventRecord>,
    /// Every health event, keyed by `(MonitorName, EventId)`. A `BTreeMap` so one
    /// monitor's events are a contiguous range in a stable total order, which is what lets
    /// `ListHealthEvents` page over them with a `NextToken` that names a resume point.
    /// Bounded by [`MAX_HEALTH_EVENTS_PER_MONITOR`] per monitor; reach it through
    /// [`Self::health_events_for`] / [`Self::record_health_event`].
    pub health_events: BTreeMap<(String, String), HealthEventRecord>,
    /// Every started query, keyed by `QueryId`. Bounded by [`MAX_QUERIES`] and written
    /// through [`Self::record_query`]; read through [`Self::query_for`], which also
    /// checks the monitor the query belongs to.
    pub queries: BTreeMap<String, QueryRecord>,
    /// The next query sequence number, which [`mint_query_id`] renders into a `QueryId`.
    /// Monotonic for the life of the store, so no two queries can be handed the same id.
    pub next_query_seq: u64,
}

impl Default for ComAmazonawsInternetmonitorInternetmonitor20210603State {
    /// A fresh store: no monitor, query, health event or idempotency record — a caller
    /// has created none yet — and the service's own internet-event feed, which no caller
    /// creates and which every account therefore reads from the first call (see
    /// [`seed_internet_events`]).
    fn default() -> Self {
        let mut store = Self {
            monitors: BTreeMap::new(),
            create_client_tokens: HashMap::new(),
            update_client_tokens: HashMap::new(),
            internet_events: BTreeMap::new(),
            health_events: BTreeMap::new(),
            queries: BTreeMap::new(),
            next_query_seq: 0,
        };
        for event in seed_internet_events(SystemTime::now()) {
            store.record_internet_event(event);
        }
        store
    }
}

impl ComAmazonawsInternetmonitorInternetmonitor20210603State {
    /// Materialise every clock-owed monitor and query transition due at `now`.
    pub fn settle_all(&mut self, now: SystemTime) {
        for monitor in self.monitors.values_mut() {
            monitor.settle(now);
        }
        for query in self.queries.values_mut() {
            query.settle(now);
        }
    }

    /// The query `query_id` of `monitor_name`, or `None` when no query of that monitor
    /// carries that id.
    ///
    /// A `QueryId` is "an internally-generated identifier for a specific query" of one
    /// monitor, so a request has to name both: one lookup, so every operation of the query
    /// interface refuses an id it cannot resolve from the same place.
    pub fn query_for(&self, monitor_name: &str, query_id: &str) -> Option<&QueryRecord> {
        self.queries
            .get(query_id)
            .filter(|query| query.monitor_name == monitor_name)
    }

    /// [`Self::query_for`], for a caller that mutates what it finds.
    pub fn query_for_mut(
        &mut self,
        monitor_name: &str,
        query_id: &str,
    ) -> Option<&mut QueryRecord> {
        self.queries
            .get_mut(query_id)
            .filter(|query| query.monitor_name == monitor_name)
    }

    /// Hold `record` as a started query, keeping the map bounded by [`MAX_QUERIES`]: the
    /// queries started longest ago are evicted to make room BEFORE the insert (as
    /// [`Self::record_internet_event`] does), so the record this call stores is never the
    /// one dropped.
    pub fn record_query(&mut self, record: QueryRecord) {
        while !self.queries.contains_key(&record.query_id) && self.queries.len() >= MAX_QUERIES {
            let Some(oldest) = self
                .queries
                .values()
                .min_by_key(|query| (query.created_at, query.query_id.as_str()))
                .map(|query| query.query_id.clone())
            else {
                break;
            };
            self.queries.remove(&oldest);
        }
        self.queries.insert(record.query_id.clone(), record);
    }

    /// Take the next query sequence number, advancing the counter. [`mint_query_id`]
    /// renders it into the `QueryId` `StartQuery` hands back.
    pub fn take_query_seq(&mut self) -> u64 {
        let seq = self.next_query_seq;
        self.next_query_seq = self.next_query_seq.saturating_add(1);
        seq
    }

    /// Forget every query of `monitor_name`, for a monitor that no longer exists: its
    /// queries are no longer reportable, and a monitor later created with the same name is
    /// a new monitor that has started none.
    pub fn forget_queries_for(&mut self, monitor_name: &str) {
        self.queries
            .retain(|_, query| query.monitor_name != monitor_name);
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

    /// Hold `record` as a known internet event, keeping the map bounded by
    /// [`MAX_INTERNET_EVENTS`]: the events that started longest ago are evicted to make
    /// room BEFORE the insert (as [`Self::record_token`] does), so the record this call
    /// stores is never the one dropped and what is held is the most recent view of
    /// internet health.
    ///
    /// The API models no operation that creates an internet event, so the writers are this
    /// store's own construction — which seeds the feed the service observes for itself
    /// (see [`seed_internet_events`]) — and tests. Any future writer belongs here rather
    /// than beside it, so one store stays the single source of truth for both reporting
    /// operations.
    pub fn record_internet_event(&mut self, record: InternetEventRecord) {
        // One eviction per record over the ceiling, each found as a running minimum rather
        // than by sorting the whole map.
        while !self.internet_events.contains_key(&record.event_id)
            && self.internet_events.len() >= MAX_INTERNET_EVENTS
        {
            let Some(oldest) = self
                .internet_events
                .values()
                .min_by_key(|event| event.started_at)
                .map(|event| event.event_id.clone())
            else {
                break;
            };
            self.internet_events.remove(&oldest);
        }
        self.internet_events.insert(record.event_id.clone(), record);
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
///
/// Its resource half is [`monitor_resource_path`], so the ARN and the messages that name
/// the same resource cannot spell it differently.
pub fn monitor_arn(region: &str, monitor_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &monitor_resource_path(monitor_name),
    )
}

/// An internet event's ARN:
/// `arn:aws:internetmonitor::<account>:internet-event/<EventId>`.
///
/// The region segment is EMPTY, as it is for every global AWS resource: an internet event
/// is a global observation of the public internet, shown on one outages map to every
/// customer, not a resource that lives in the region the request was signed for.
pub fn internet_event_arn(event_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, "", &internet_event_resource_path(event_id))
}

/// The resource path an internet event is identified by: the resource half of
/// [`internet_event_arn`], and what [`invalid_resource_id_message`] names it with, so the
/// ARN and the refusal that names the same event cannot spell it differently.
pub fn internet_event_resource_path(event_id: &str) -> String {
    format!("internet-event/{event_id}")
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

/// A stored instant as seconds since the Unix epoch, negative before it.
///
/// The common scale on which a handler compares a stored instant with a `Timestamp` the
/// request carried (`::aws_smithy_types::DateTime::as_secs_f64`), so a time-window filter
/// needs no fallible `SystemTime` arithmetic. Every listing that bounds a stored instant
/// by a requested one (`ListHealthEvents`, `ListInternetEvents`) compares on this scale,
/// so there is one rule for all of them.
pub fn epoch_seconds(at: SystemTime) -> f64 {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => since_epoch.as_secs_f64(),
        Err(before_epoch) => -before_epoch.duration().as_secs_f64(),
    }
}

/// The `QueryId` `StartQuery` hands back for the query it starts: "the
/// internally-generated identifier of a specific query", minted from the store's
/// monotonic query counter so it is unique for the life of the store and DERIVED FROM
/// STORED STATE rather than from a random source this crate would need a new dependency
/// for.
///
/// Internet Monitor renders one as a canonical hyphenated UUID, so that is the form: the
/// version (`4`) and variant (`8`) nibbles sit where a UUID carries them and the counter
/// fills the node field, offset by one so the first id minted is not the nil UUID.
pub fn mint_query_id(seq: u64) -> String {
    format!("00000000-0000-4000-8000-{:012x}", seq.saturating_add(1))
}

/// The message AWS words a not-found monitor with, built from the request and the
/// caller's account: it names the account the lookup was made in and the monitor's own
/// resource path (`monitor/<MonitorName>`, the resource half of [`monitor_arn`]).
///
/// One rule for every operation whose addressed resource IS the monitor the request names
/// (`GetMonitor`, `UpdateMonitor`), so the two cannot word the same refusal differently.
pub fn monitor_not_found_message(monitor_name: &str) -> String {
    format!(
        "No resource found for account {} and resource {}",
        ::engine::emulator_account_id(),
        monitor_resource_path(monitor_name)
    )
}

/// The message this service words a request it refuses as failing input validation with —
/// a value outside a constraint the model declares, or a documented limit on what one
/// request may carry. The refusal names no member: the service states the class of the
/// failure, and the shape it carries is `BadRequestException` (HTTP 400).
pub const INVALID_REQUEST_BODY_MESSAGE: &str = "Invalid request body";

/// The message the query interface words a `QueryId` it cannot resolve for the monitor the
/// request named with, carried by the modelled `ValidationException` (HTTP 400).
pub const INVALID_QUERY_ID_MESSAGE: &str = "Invalid query ID provided";

/// The message this service words a request whose time window selects no instant with,
/// carried by the modelled `ValidationException` (HTTP 400). The text is fixed: it states
/// the class of the failure and names neither bound, nor a member, nor any value from the
/// request.
pub const INVALID_TIME_WINDOW_MESSAGE: &str = "Invalid time window specified";

/// Refuse a time window whose start instant lies after its end instant, which selects no
/// instant at all.
///
/// One rule — and so one wording, [`INVALID_TIME_WINDOW_MESSAGE`] — for every operation
/// that bounds instants by a window the request carries (`ListInternetEvents`,
/// `ListHealthEvents`), so none of them can word the same refusal differently. Both bounds
/// are optional in the model: a window with only one bound, or with neither, holds instants
/// and is not refused. The caller lifts the returned exception into its own operation's
/// error channel, the only part that differs between them.
pub fn ordered_time_window(start: Option<f64>, end: Option<f64>) -> Result<(), ValidationException> {
    match (start, end) {
        (Some(start), Some(end)) if start > end => Err(ValidationException {
            message: Some(INVALID_TIME_WINDOW_MESSAGE.to_string()),
        }),
        _ => Ok(()),
    }
}

/// The message this service words a resource id the request named but this account does
/// not hold with, carried by the modelled `ValidationException` (HTTP 400): it states the
/// resource PATH the request named, which is what identifies the resource inside the
/// account (`health-event/<MonitorName>/<EventId>` for a health event,
/// `monitor/<MonitorName>` for a monitor).
///
/// One rule for every operation whose addressed resource is NOT the monitor the request
/// names — one reached through a monitor, or an internet event
/// ([`internet_event_resource_path`]) — so they cannot word the same refusal differently.
pub fn invalid_resource_id_message(resource_path: &str) -> String {
    format!("resourceID is invalid: {resource_path}")
}

/// The resource path a monitor is identified by inside an account: the resource half of
/// [`monitor_arn`], and what [`monitor_not_found_message`] and
/// [`invalid_resource_id_message`] both name it with.
pub fn monitor_resource_path(monitor_name: &str) -> String {
    format!("monitor/{monitor_name}")
}

/// The value of an enum-valued filter a request carried, refused with this service's
/// `ValidationException` when it names no member of `allowed`.
///
/// One rule for every listing that filters on an enum member — `ListMonitors`'
/// `MonitorStatus`, `ListHealthEvents`' `EventStatus`, `ListInternetEvents`'
/// `EventStatus` and `EventType` — because the model types several of them as a plain
/// `String`, which the boundary validator cannot constrain. The caller lifts the returned
/// exception into its own operation's error channel, the only part that differs between
/// them. The message is built from the request: the member it rejected and the members it
/// allows.
pub fn enum_filter<'a>(
    member: &str,
    value: Option<&'a str>,
    allowed: &[&str],
) -> Result<Option<&'a str>, ValidationException> {
    match value {
        Some(filter) if !allowed.contains(&filter) => Err(ValidationException {
            message: Some(format!(
                "{member} {filter} is not one of: {}.",
                allowed.join(", ")
            )),
        }),
        other => Ok(other),
    }
}

/// A listing's `NextToken`: the opaque form of "resume after this key" — the
/// `MonitorName` a `ListMonitors` page ended on, the `EventId` a `ListHealthEvents` or
/// `ListInternetEvents` page ended on. Each pages over a `BTreeMap`, so one key is one
/// resume point.
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
