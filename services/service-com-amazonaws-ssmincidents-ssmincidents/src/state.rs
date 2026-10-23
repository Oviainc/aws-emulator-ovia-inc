// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmincidents-ssmincidents service.
//!
//! Three resources live here, keyed by their ARN so every read path resolves the same
//! object a write path stored:
//!
//! * `response_plans` — what `CreateResponsePlan` mints and `GetResponsePlan` serves.
//! * `incident_records` — what `StartIncident` creates, `GetIncidentRecord` /
//!   `ListIncidentRecords` report, `UpdateIncidentRecord` mutates and
//!   `DeleteIncidentRecord` removes.
//! * `replication_sets` — the account's Incident Manager onboarding: the replication
//!   set(s) `ListReplicationSets` reports and `GetReplicationSet` resolves an ARN
//!   against. AWS holds at most ONE per account; the map is keyed by ARN like the
//!   other two so every replication-set read path resolves the same stored object,
//!   and the creating path is the single place that invariant is enforced.
//!   `ssm-contacts` is FRONTED by that onboarding, but it does NOT read this map: that
//!   service keeps the account's set as its own record
//!   (`ComAmazonawsSsmcontactsSsmcontactsState::replication_set`), which is what its
//!   data-plane gate consults.
//!
//! `idempotency` records the ARN each `@idempotencyToken` minted the first time, so a
//! replayed `clientToken` answers with the same ARN instead of creating a second
//! resource.

use std::collections::{HashMap, VecDeque};

use aws_smithy_types::DateTime;

use crate::ops::types::{
    Action, ChatChannel, IncidentRecord, IncidentRecordSource, IncidentRecordSummary,
    IncidentTemplate, Integration, NotificationTargetItem, RegionInfo, RelatedItem,
    ReplicationSet, TimelineEvent,
};

/// The `IncidentRecordStatus` value a freshly started incident carries.
///
/// From the model's `IncidentRecordStatus` enum (`OPEN` | `RESOLVED`); see
/// `INCIDENT_RECORD_STATUS_VALUES` in `ops/types/shared_types.rs`.
pub const STATUS_OPEN: &str = "OPEN";

/// The terminal `IncidentRecordStatus` value — the one that gives a record its
/// `resolvedTime`.
pub const STATUS_RESOLVED: &str = "RESOLVED";

/// `IncidentRecordSource$source` for an incident a principal started directly through
/// `StartIncident` rather than through a CloudWatch alarm or EventBridge rule. A
/// request that carries `triggerDetails` reports that trigger's own `source` instead.
pub const MANUAL_INCIDENT_SOURCE: &str = "aws.ssm-incidents";

/// The `ResourceType` enum values this service names in its error payloads. From the
/// model's `ResourceType` enum; see `RESOURCE_TYPE_VALUES` in `shared_types.rs`.
pub const RESOURCE_TYPE_RESPONSE_PLAN: &str = "RESPONSE_PLAN";
/// See [`RESOURCE_TYPE_RESPONSE_PLAN`].
pub const RESOURCE_TYPE_INCIDENT_RECORD: &str = "INCIDENT_RECORD";
/// See [`RESOURCE_TYPE_RESPONSE_PLAN`].
pub const RESOURCE_TYPE_REPLICATION_SET: &str = "REPLICATION_SET";

/// The `quotaCode` this emulator reports when the response-plan store is full. The
/// model constrains neither the code's form nor its value, so it names what the quota
/// counts.
pub const QUOTA_CODE_RESPONSE_PLANS: &str = "ResponsePlansPerAccount";
/// See [`QUOTA_CODE_RESPONSE_PLANS`].
pub const QUOTA_CODE_INCIDENT_RECORDS: &str = "IncidentRecordsPerAccount";

/// The largest page `ListIncidentRecords` may answer with, and the page size it uses
/// when the request names none: the model caps both `maxResults` (`range 1..100`) and
/// `IncidentRecordSummaryList` (`length ..100`) at 100.
///
/// Applied as a CEILING in [`page_size`], not merely as a default, so no number a
/// request carries can size a response or the allocation that builds it — whatever any
/// other layer does or does not refuse.
pub const MAX_INCIDENT_RECORDS_PER_PAGE: i32 = 100;

/// The largest page `ListReplicationSets` may answer with, and the page size it uses
/// when the request names none: its `maxResults` targets the same `MaxResults` shape
/// (`range 1..100`), and `ReplicationSetArnList` declares no length of its own.
///
/// Applied as a CEILING through [`clamped_page_size`], for the same reason as
/// [`MAX_INCIDENT_RECORDS_PER_PAGE`].
pub const MAX_REPLICATION_SET_ARNS_PER_PAGE: i32 = 100;

/// The most filters one `ListIncidentRecords` request may carry: the model's
/// `FilterList` is `length ..5`. Re-checked in the handler so the records x filters
/// work one request can ask for is bounded here too, not only at the boundary.
pub const MAX_LIST_FILTERS: usize = 5;

/// The most response plans this store holds. Bounds the memory one emulator process can
/// be made to hold through repeated `CreateResponsePlan` calls; `CreateResponsePlan`
/// refuses past it with the service's `ServiceQuotaExceededException` rather than
/// growing without limit.
pub const MAX_RESPONSE_PLANS: usize = 1_000;

/// The most incident records this store holds, bounding `StartIncident` the same way.
/// With [`MAX_LIST_FILTERS`] this also bounds the records x filters scan
/// `ListIncidentRecords` performs under the state lock.
pub const MAX_INCIDENT_RECORDS: usize = 10_000;

/// The most replication sets this store holds. Incident Manager onboards an account
/// with ONE replication set — `CreateReplicationSet`'s own documentation describes "a
/// replication set" per account, spanning up to three Regions (`regions`, `length
/// 1..3`) — so there is never a second one to hold.
pub const MAX_REPLICATION_SETS: usize = 1;

/// The most idempotency replays remembered at once. Each distinct caller-supplied
/// `clientToken` adds one entry, so the map is bounded and the oldest entries are
/// evicted first ([`ComAmazonawsSsmincidentsSsmincidentsState::remember_arn`]) — a
/// caller sending a fresh token every request cannot grow it without limit.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

/// A response plan exactly as `CreateResponsePlan` stored it.
#[derive(Debug, Clone)]
pub struct ResponsePlan {
    pub arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub incident_template: IncidentTemplate,
    pub chat_channel: Option<ChatChannel>,
    /// Stored exactly as the request carried them — absent stays absent and an empty
    /// list stays empty, so a read reports the shape that was written.
    pub engagements: Option<Vec<String>>,
    /// See [`ResponsePlan::engagements`].
    pub actions: Option<Vec<Action>>,
    /// See [`ResponsePlan::engagements`].
    pub integrations: Option<Vec<Integration>>,
    pub tags: HashMap<String, String>,
    pub created_time: DateTime,
}

/// An incident record: created by `StartIncident`, mutated by `UpdateIncidentRecord`.
#[derive(Debug, Clone)]
pub struct IncidentRecordEntry {
    pub arn: String,
    /// The response plan the incident was started from, which also names the ARN path.
    pub response_plan_arn: String,
    pub title: String,
    pub summary: Option<String>,
    /// One of [`STATUS_OPEN`] / [`STATUS_RESOLVED`].
    pub status: String,
    pub impact: i32,
    pub creation_time: DateTime,
    pub resolved_time: Option<DateTime>,
    pub last_modified_time: DateTime,
    pub last_modified_by: String,
    pub dedupe_string: String,
    pub incident_record_source: IncidentRecordSource,
    pub chat_channel: Option<ChatChannel>,
    /// Inherited from the response plan's template, or replaced by
    /// `UpdateIncidentRecord`. Absent stays absent and an empty list stays empty.
    pub notification_targets: Option<Vec<NotificationTargetItem>>,
    /// `incidentTags` the response plan's template assigns to the incident.
    pub incident_tags: HashMap<String, String>,
    /// Reserved for the timeline-event slice; the timeline operations are still 501.
    pub timeline_events: Vec<TimelineEvent>,
    /// The `relatedItems` `StartIncident` was called with.
    pub related_items: Vec<RelatedItem>,
}

impl IncidentRecordEntry {
    /// Project the stored record onto the modelled `IncidentRecord` shape
    /// `GetIncidentRecord` returns.
    pub fn to_incident_record(&self) -> IncidentRecord {
        IncidentRecord {
            arn: self.arn.clone(),
            title: self.title.clone(),
            summary: self.summary.clone(),
            status: self.status.clone(),
            impact: self.impact,
            creation_time: self.creation_time,
            resolved_time: self.resolved_time,
            last_modified_time: self.last_modified_time,
            last_modified_by: self.last_modified_by.clone(),
            // No runbook is executed by this emulator, so no execution is reported:
            // an `automationExecutions` entry would claim work that never ran.
            automation_executions: None,
            incident_record_source: self.incident_record_source.clone(),
            dedupe_string: self.dedupe_string.clone(),
            chat_channel: self.chat_channel.clone(),
            notification_targets: self.notification_targets.clone(),
        }
    }

    /// Project the stored record onto the `IncidentRecordSummary` shape
    /// `ListIncidentRecords` returns — the same state `to_incident_record` serves.
    pub fn to_summary(&self) -> IncidentRecordSummary {
        IncidentRecordSummary {
            arn: self.arn.clone(),
            title: self.title.clone(),
            status: self.status.clone(),
            impact: self.impact,
            creation_time: self.creation_time,
            resolved_time: self.resolved_time,
            incident_record_source: self.incident_record_source.clone(),
        }
    }

    /// Sort key putting the newest record first and breaking ties by ARN, so paging
    /// over the same state twice visits the records in the same order.
    pub fn list_order_key(&self) -> (std::cmp::Reverse<(i64, u32)>, &str) {
        (
            std::cmp::Reverse(timestamp_key(&self.creation_time)),
            self.arn.as_str(),
        )
    }
}

/// Total order over a timestamp. `DateTime` exposes whole seconds and the sub-second
/// nanos separately, which together order two instants exactly.
pub fn timestamp_key(at: &DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The current instant, as the modelled `Timestamp` type.
pub fn now() -> DateTime {
    DateTime::from(std::time::SystemTime::now())
}

/// How many summaries one `ListIncidentRecords` page may carry: the request's
/// `maxResults`, or [`MAX_INCIDENT_RECORDS_PER_PAGE`] when the request carried none —
/// never more than that ceiling, and never a non-positive count.
pub fn page_size(max_results: Option<i32>) -> usize {
    clamped_page_size(max_results, MAX_INCIDENT_RECORDS_PER_PAGE)
}

/// How many items one page of any paginated list in this service may carry: the
/// request's `maxResults`, clamped to `ceiling`, which is also the size used when the
/// request carried no `maxResults` or a non-positive one.
///
/// Shared by this service's list operations, so each page — and the allocation that
/// builds it — is sized by the service's own ceiling rather than by a number the
/// request carries, whatever any other layer does or does not refuse.
pub fn clamped_page_size(max_results: Option<i32>, ceiling: i32) -> usize {
    let ceiling = ceiling.max(0);
    let requested = max_results.unwrap_or(ceiling);
    if requested <= 0 {
        return ceiling as usize;
    }
    requested.min(ceiling) as usize
}

/// The principal ARN this service records in `lastModifiedBy` and
/// `incidentRecordSource.createdBy`, derived from the SigV4 access key id the request
/// was signed with. Both members target `Arn`, so the identity is reported as the IAM
/// principal ARN of that key rather than as the bare key id.
pub fn caller_principal(headers: &http::HeaderMap) -> String {
    let akid = ::engine::extract_akid(headers);
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// `arn:aws:ssm-incidents::<account>:response-plan/<name>` — Incident Manager is a
/// regionless (replicated) resource namespace, so the region segment is empty.
pub fn response_plan_arn(name: &str) -> String {
    ::engine::emulator_arn("ssm-incidents", "", &format!("response-plan/{name}"))
}

/// `arn:aws:ssm-incidents::<account>:incident-record/<plan name>/<id>`.
pub fn incident_record_arn(plan_name: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "ssm-incidents",
        "",
        &format!("incident-record/{plan_name}/{id}"),
    )
}

/// An Incident Manager replication set as it is persisted: the account-level resource
/// naming the Regions Incident Manager data is replicated to.
///
/// Every modelled `ReplicationSet` member is held, so a read path reports the stored
/// object instead of recomputing it: `ListReplicationSets` reads `arn` and
/// `created_time` (its list order) and the rest is what the replication-set
/// `Get`/`Update` paths serve.
#[derive(Debug, Clone)]
pub struct ReplicationSetEntry {
    pub arn: String,
    /// Each Region in the set, keyed by Region name, with that Region's status and
    /// optional `sseKmsKeyId` — the modelled `RegionInfoMap`.
    pub region_map: HashMap<String, RegionInfo>,
    /// One of `REPLICATION_SET_STATUS_VALUES` (see `ops::types::shared_types`).
    pub status: String,
    pub deletion_protected: bool,
    pub created_time: DateTime,
    pub created_by: String,
    pub last_modified_time: DateTime,
    pub last_modified_by: String,
}

impl ReplicationSetEntry {
    /// Project the stored set onto the modelled `ReplicationSet` shape
    /// `GetReplicationSet` returns, so that read reports what was stored rather than a
    /// fixed answer.
    pub fn to_replication_set(&self) -> ReplicationSet {
        ReplicationSet {
            arn: Some(self.arn.clone()),
            region_map: self.region_map.clone(),
            status: self.status.clone(),
            deletion_protected: self.deletion_protected,
            created_time: self.created_time,
            created_by: self.created_by.clone(),
            last_modified_time: self.last_modified_time,
            last_modified_by: self.last_modified_by.clone(),
        }
    }

    /// Sort key giving the stored replication sets one total order — oldest first,
    /// ties broken by ARN — so paging over the same state twice visits them in the
    /// same order. The model documents no order for `replicationSetArns` (an account
    /// holds at most one set), so this is the emulator's stable order, not a claim
    /// about AWS.
    pub fn list_order_key(&self) -> ((i64, u32), &str) {
        (timestamp_key(&self.created_time), self.arn.as_str())
    }
}

/// `arn:aws:ssm-incidents::<account>:replication-set/<id>` — as with the other ARNs
/// this service mints, the Region segment is empty because the namespace is replicated
/// rather than regional.
pub fn replication_set_arn(id: &str) -> String {
    ::engine::emulator_arn("ssm-incidents", "", &format!("replication-set/{id}"))
}

/// Per-service state for com-amazonaws-ssmincidents-ssmincidents. A handler reaches it through [`service_state`].
///
/// Every map here is bounded: `response_plans` by [`MAX_RESPONSE_PLANS`] and
/// `incident_records` by [`MAX_INCIDENT_RECORDS`] (both consulted before a create
/// inserts), `replication_sets` by [`MAX_REPLICATION_SETS`] — the one replication set
/// AWS permits an account, enforced where a set is created — and `idempotency` by
/// [`MAX_IDEMPOTENCY_ENTRIES`] with oldest-first eviction, so no sequence of requests
/// can make this process hold memory without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmincidentsSsmincidentsState {
    /// Response plans, keyed by response-plan ARN. Bounded by [`MAX_RESPONSE_PLANS`].
    pub response_plans: HashMap<String, ResponsePlan>,
    /// Incident records, keyed by incident-record ARN. Bounded by
    /// [`MAX_INCIDENT_RECORDS`].
    pub incident_records: HashMap<String, IncidentRecordEntry>,
    /// The account's replication set(s), keyed by replication-set ARN, as
    /// `ListReplicationSets` reports them and `GetReplicationSet` resolves an ARN
    /// against. AWS permits at most one per account ([`MAX_REPLICATION_SETS`]), so
    /// this map is bounded by the operation that creates one: no request adds a
    /// second.
    pub replication_sets: HashMap<String, ReplicationSetEntry>,
    /// `<operation>:<clientToken>` -> the ARN that token minted the first time.
    /// Bounded by [`MAX_IDEMPOTENCY_ENTRIES`].
    pub idempotency: HashMap<String, String>,
    /// The order [`idempotency`](Self::idempotency)'s keys were recorded in, so the
    /// oldest is the one evicted when the map is full.
    pub idempotency_order: VecDeque<String>,
}

impl ComAmazonawsSsmincidentsSsmincidentsState {
    /// The ARN a `clientToken` already minted for `operation`, if this is a replay.
    pub fn replayed_arn(&self, operation: &str, client_token: Option<&str>) -> Option<String> {
        let token = client_token.filter(|t| !t.is_empty())?;
        self.idempotency.get(&idempotency_key(operation, token)).cloned()
    }

    /// Record the ARN `operation` minted for `client_token`, so a replay answers it.
    ///
    /// Evicts the oldest remembered token once [`MAX_IDEMPOTENCY_ENTRIES`] are held, so
    /// a caller sending a fresh token on every request — including on `StartIncident`'s
    /// deduplication path, which mints no resource at all — cannot grow this map
    /// without limit.
    pub fn remember_arn(&mut self, operation: &str, client_token: Option<&str>, arn: &str) {
        let Some(token) = client_token.filter(|t| !t.is_empty()) else {
            return;
        };
        let key = idempotency_key(operation, token);
        if self
            .idempotency
            .insert(key.clone(), arn.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Whether this store can hold another response plan, i.e. is below
    /// [`MAX_RESPONSE_PLANS`].
    pub fn accepts_another_response_plan(&self) -> bool {
        self.response_plans.len() < MAX_RESPONSE_PLANS
    }

    /// Whether this store can hold another incident record, i.e. is below
    /// [`MAX_INCIDENT_RECORDS`].
    pub fn accepts_another_incident_record(&self) -> bool {
        self.incident_records.len() < MAX_INCIDENT_RECORDS
    }
}

/// Idempotency keys are namespaced per operation: the same token used on two different
/// operations names two different resources.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmincidentsSsmincidentsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmincidentsSsmincidentsState>)
}
