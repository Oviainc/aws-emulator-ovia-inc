// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdashboards-awsbcmdashboardsservice service.
//!
//! The dashboard records every dashboard operation reads and writes live here, so
//! `CreateDashboard`, `GetDashboard`, `ListDashboards`, `UpdateDashboard` and
//! `DeleteDashboard` all see one copy of each resource. The helpers below (ARN and
//! widget-id minting, the `DashboardReference` projection, the tag resolution the
//! tagging operations share, the pagination cursor codec) are shared by those handlers
//! rather than duplicated per operation.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DashboardReference, DateTimeRange, HealthStatus, ResourceTag, ScheduleConfig, ScheduledReport,
    ScheduledReportInput, ScheduledReportSummary, Widget,
};

/// The only `DashboardType` the model declares (`DASHBOARD_TYPE_VALUES`), and the
/// type every dashboard a client creates through this API carries.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";

/// The `ScheduleState` (`SCHEDULE_STATE_VALUES`) a schedule carries when the request
/// left `scheduleConfig.state` unset: a report whose state nobody chose runs on its
/// schedule, which is what the recorded unit
/// `bcm-dashboards.scheduled-report-default-schedule-state-enabled` reads back.
pub const SCHEDULE_STATE_ENABLED: &str = "ENABLED";

/// The `HealthStatusCode` (`HEALTH_STATUS_CODE_VALUES`) a scheduled report reports
/// while nothing has refreshed its health: the model documents `HEALTHY` as "the
/// scheduled report is configured properly and has all required permissions to
/// execute", which is what a report this service accepted at create time is.
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";

/// The other `HealthStatusCode` (`HEALTH_STATUS_CODE_VALUES`): the model documents
/// `UNHEALTHY` as "the scheduled report is unable to deliver the notification … and
/// your action is needed", with the reasons in `statusReasons`.
pub const HEALTH_STATUS_UNHEALTHY: &str = "UNHEALTHY";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `dashboardArn` names no dashboard this account stores any more — the dashboard the
/// report renders was deleted after the report was created.
pub const STATUS_REASON_DASHBOARD_NOT_FOUND: &str = "DASHBOARD_NOT_FOUND";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `widgetIds` name a widget its dashboard does not hold — the widget was removed from
/// the dashboard after the report was created.
pub const STATUS_REASON_WIDGET_ID_NOT_FOUND: &str = "WIDGET_ID_NOT_FOUND";

/// How long the `clientToken` of an `ExecuteScheduledReport` call keeps answering with
/// that call's outcome instead of triggering a second execution.
///
/// CITED, not chosen: the model documents the window on the operation itself — "If a
/// `clientToken` is provided, the service uses it for idempotency. Requests with the
/// same client token will not trigger a new execution within the same minute."
/// (`smithy_query(query="doc",shape="ExecuteScheduledReport")`). One minute.
pub const EXECUTION_IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(60);

/// The prefix a pagination cursor carries, so a token this service did not mint is
/// rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";

/// The ARN resource-type segment a `ListDashboards` cursor names.
pub const DASHBOARD_ARN_SEGMENT: &str = ":dashboard/";

/// The ARN resource-type segment a `ListScheduledReports` cursor names.
pub const SCHEDULED_REPORT_ARN_SEGMENT: &str = ":scheduled-report/";

/// The most dashboards this emulator stores for the account before `CreateDashboard`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's dashboard quota is not stated in the model, so this bound is
/// CHOSEN, not cited: it is set far above any plausible client's dashboard count, so
/// normal use never meets it, while the store a caller can grow stays bounded.
pub const MAX_DASHBOARDS: usize = 1_000;

/// The most tags this emulator stores on ONE dashboard or scheduled report. A merge
/// that would carry a resource past it is refused by `TagResource` with its modelled
/// `ValidationException`, storing nothing.
///
/// The model states no per-resource tag quota for this service, so the ceiling is
/// CHOSEN, not cited — and chosen to be exactly `ResourceTagList`'s own modelled maximum
/// (length 0..200, `smithy_query(query="shape",shape="ResourceTagList")`): a resource
/// holds at most as many tags as one request may carry. That is what bounds the store a
/// caller can grow by repeating `TagResource` with fresh keys, and with it the linear
/// scan a merge runs while the state lock is held. The create-time tag path needs no
/// second check for the same reason: `CreateDashboard`'s and `CreateScheduledReport`'s
/// `resourceTags` are that same 0..200 list, so the boundary validator already refuses a
/// create that would enter the store above this ceiling.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix Amazon Web Services reserves for the tags it generates itself.
///
/// AWS's tagging rules state that the `aws:` prefix is reserved for AWS use and that a
/// caller can neither create nor edit nor delete a tag key carrying it, so every path
/// on this surface that takes a caller-supplied tag key refuses one in this namespace
/// instead of storing or removing it ([`check_tag_keys_not_reserved`]). The model cannot
/// express the rule — `ResourceTagKey`'s pattern is `^[\S\s]*$`
/// (`smithy_query(query="shape",shape="ResourceTagKey")`) — so the handlers enforce it.
/// The comparison is case-insensitive, because the prefix is reserved however it is
/// cased.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How the TAGGING path words a `ResourceArn` that resolves to no stored resource.
///
/// A constant, carrying nothing from the request — which is what the recorded
/// `ListTagsForResource` not-found answer says, verbatim. Both tagging operations that
/// resolve an ARN through this state's `resource_tags` / `resource_tags_mut`
/// word that one refusal the same way; the ARN itself travels in the answer's `resourceId`
/// member, not in the text (see `crate::scaffolded::Scaffolded::resource_not_found`).
pub const TAGGING_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// One stored dashboard.
///
/// `widgets` holds the modelled [`Widget`] shape verbatim — including the widget
/// ids this service minted and the `width`/`height`/`horizontalOffset` values the
/// model's defaults filled in — so `GetDashboard` reads back exactly what was
/// stored rather than re-deriving anything from the request.
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// `DashboardType` as it goes on the wire; always [`DASHBOARD_TYPE_CUSTOM`]
    /// for a dashboard created through `CreateDashboard`.
    pub dashboard_type: String,
    pub widgets: Vec<Widget>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags supplied at create time, read back by `ListTagsForResource` and
    /// narrowed by `UntagResource`, in the order they were stored.
    pub resource_tags: Vec<ResourceTag>,
    /// The resource-based policy document attached to this dashboard, or `None`
    /// while no policy is attached — the two are distinct, and `GetResourcePolicy`
    /// projects the stored document rather than deriving one from the request.
    ///
    /// This service's model declares no operation that attaches a policy (there is
    /// no `PutResourcePolicy` among its 15 operations), so a dashboard created
    /// through this API keeps `None` and reads back the empty document the modelled
    /// `policyDocument` length (0..1024) admits. The field is the single place a
    /// policy would live once an operation can write one, so the reader needs no
    /// second map beside the dashboards.
    pub resource_policy: Option<String>,
}

impl Dashboard {
    /// This dashboard projected onto the `DashboardReference` shape `ListDashboards`
    /// returns: the stored record's own fields, never request input.
    pub fn reference(&self) -> DashboardReference {
        DashboardReference {
            arn: self.arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.dashboard_type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The most scheduled reports this emulator stores for the account before
/// `CreateScheduledReport` answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's scheduled-report quota is not stated in the model, so this
/// bound is CHOSEN, not cited: it is set far above any plausible client's report
/// count, so normal use never meets it, while the store a caller can grow stays
/// bounded.
pub const MAX_SCHEDULED_REPORTS: usize = 1_000;

/// One stored scheduled report.
///
/// Holds the modelled members verbatim — including the `scheduleConfig` with its
/// `state` resolved — so every reader projects the stored record rather than
/// re-deriving anything from a request. `last_execution_at` and `health_status` stay
/// unset until an execution of this report sets them: nothing has run at create time.
/// A reader never projects `health_status` directly — [`Self::health`] resolves the
/// unrefreshed record's health for every view at once.
#[derive(Debug, Clone)]
pub struct ScheduledReportRecord {
    pub arn: String,
    pub name: String,
    pub dashboard_arn: String,
    pub execution_role_arn: String,
    pub schedule_config: ScheduleConfig,
    pub description: Option<String>,
    /// `None` when the request named no widget, `Some(list)` otherwise — the two are
    /// distinct on the wire, and `UpdateScheduledReport` has to be able to reset the
    /// member to an empty list.
    pub widget_ids: Option<Vec<String>>,
    pub widget_date_range_override: Option<DateTimeRange>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub last_execution_at: Option<SystemTime>,
    /// The health this report's last refresh established, unset until one has: the
    /// readers project [`ScheduledReportRecord::health`], never this member.
    pub health_status: Option<HealthStatus>,
    /// The tags supplied at create time, read back by `ListTagsForResource` and
    /// narrowed by `UntagResource`, in the order they were stored.
    pub resource_tags: Vec<ResourceTag>,
}

impl ScheduledReportRecord {
    /// The `ScheduleState` this report's schedule is in: the stored value, or
    /// [`SCHEDULE_STATE_ENABLED`] for a record stored without one. `state` is resolved
    /// at write time by [`resolve_schedule_config`], so the fallback only covers a
    /// record written before that resolution existed; keeping it in ONE place is what
    /// lets every reader and `ExecuteScheduledReport` agree on whether a report's
    /// schedule is enabled.
    pub fn schedule_state(&self) -> &str {
        self.schedule_config
            .state
            .as_deref()
            .unwrap_or(SCHEDULE_STATE_ENABLED)
    }

    /// The health of this report, as EVERY view of it reports it.
    ///
    /// Health is a property of the RESOURCE, not of the view that renders it, so the
    /// resolution lives here once rather than in each projection: a detail read and a
    /// listing of the same record can never disagree about it.
    ///
    /// A record whose health nothing has refreshed is [`HEALTH_STATUS_HEALTHY`] — the
    /// model documents `HEALTHY` as "the scheduled report is configured properly and
    /// has all required permissions to execute"
    /// (`smithy_query(query="shape",shape="HealthStatus")`), which is what a report
    /// this service accepted at create time is — with no `lastRefreshedAt` (no refresh
    /// has happened) and no `statusReasons` (the model documents those as present only
    /// when the status is `UNHEALTHY`).
    pub fn health(&self) -> HealthStatus {
        self.health_status.clone().unwrap_or_else(|| HealthStatus {
            status_code: HEALTH_STATUS_HEALTHY.to_string(),
            last_refreshed_at: None,
            status_reasons: None,
        })
    }

    /// This record projected onto the modelled `ScheduledReport` shape that
    /// `GetScheduledReport` (and the other readers) return: the stored record's own
    /// fields, never request input.
    ///
    /// `healthStatus` is optional on this shape and `@required` on the summary, but it
    /// is the same property of the same record either way, so the detail view reports
    /// exactly what [`Self::health`] resolves rather than reporting less than the
    /// listing does.
    pub fn report(&self) -> ScheduledReport {
        ScheduledReport {
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            dashboard_arn: self.dashboard_arn.clone(),
            scheduled_report_execution_role_arn: self.execution_role_arn.clone(),
            schedule_config: self.schedule_config.clone(),
            description: self.description.clone(),
            widget_ids: self.widget_ids.clone(),
            widget_date_range_override: self.widget_date_range_override.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            last_execution_at: self.last_execution_at.map(::aws_smithy_types::DateTime::from),
            health_status: Some(self.health()),
        }
    }

    /// This record projected onto the modelled `ScheduledReportSummary` shape that
    /// `ListScheduledReports` pages over: the stored record's own fields, never
    /// request input.
    ///
    /// Two members the summary declares `@required` are optional on the stored
    /// record, so each is resolved here rather than by every reader:
    ///
    /// * `scheduleExpression` is optional inside `ScheduleConfig`, and a
    ///   `GenericString` of length 0..1024 — so a schedule stored without an
    ///   expression summarises as the empty string rather than inventing one.
    /// * `healthStatus` is optional on the stored record until something refreshes it,
    ///   and is resolved by [`Self::health`] — the same resolution the detail view
    ///   uses, so both views of one record report one health.
    pub fn summary(&self) -> ScheduledReportSummary {
        ScheduledReportSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            dashboard_arn: self.dashboard_arn.clone(),
            schedule_expression: self
                .schedule_config
                .schedule_expression
                .clone()
                .unwrap_or_default(),
            state: self.schedule_state().to_string(),
            health_status: self.health(),
            schedule_expression_time_zone: self
                .schedule_config
                .schedule_expression_time_zone
                .clone(),
            widget_ids: self.widget_ids.clone(),
        }
    }
}

/// How long a `CreateScheduledReport` call's creation counts as still IN PROGRESS, so
/// a duplicate-token call racing it is refused with the modelled `ConflictException`
/// ("attempting to create a resource that already exists or is being created") while a
/// retry that arrives after it — what an SDK resends once its transport has timed out —
/// is answered with the first call's outcome.
///
/// The real service creates the report over some interval and refuses a duplicate token
/// for its duration; this emulator stores a record under one lock, so the interval would
/// otherwise be zero and no caller could ever observe the conflict the service answers.
/// The length is CHOSEN, not cited — the model states none — and kept short: long
/// enough that the calls of one racing client fall inside it, short enough that a
/// client's next retry does not.
pub const SCHEDULED_REPORT_CREATION_WINDOW: Duration = Duration::from_millis(500);

/// The outcome one `CreateScheduledReport` call recorded under the `clientToken` it
/// presented, so a RETRY of that same call answers that same outcome — the ARN the
/// first call minted — instead of creating a second report or being refused.
///
/// `request_fingerprint` is what tells a retry from a different request that happens to
/// reuse the token: it is a canonical rendering of the create request
/// ([`scheduled_report_creation_fingerprint`]), compared only against another rendering
/// built by that same function and never parsed.
///
/// `in_progress_until` is the instant the creation this record describes stops counting
/// as in flight ([`SCHEDULED_REPORT_CREATION_WINDOW`] past the call that made it). It is
/// read, never slept on, so nothing waits for it.
#[derive(Debug, Clone)]
pub struct ScheduledReportCreation {
    pub arn: String,
    pub request_fingerprint: String,
    pub in_progress_until: SystemTime,
}

impl ScheduledReportCreation {
    /// Whether the creation this record describes is still in flight at `now`.
    pub fn in_progress_at(&self, now: SystemTime) -> bool {
        now < self.in_progress_until
    }
}

/// A canonical rendering of a `CreateScheduledReport` request, so two calls can be
/// compared for "the same request" when they present the same `clientToken`.
///
/// It renders exactly the members that decide which resource the call creates — the
/// submitted `scheduledReport` and the `resourceTags` — one line per member, in a fixed
/// order, and NORMALISED the way the create itself normalises them: the schedule's
/// `state` resolved through [`resolve_schedule_config`] and the tags resolved by key
/// through [`merge_resource_tags`] and then ordered by key. Two requests that would store
/// the same record therefore render alike, whatever order their tags arrived in and
/// whether they spelled the default schedule state out.
///
/// Every member is rendered from its own value rather than through a derived formatter,
/// so what counts as "the same request" is this function's list and nothing else: a
/// string is quoted (so a value holding a separator cannot look like one, and an empty
/// string cannot look like an absent member), a timestamp is its seconds and nanoseconds,
/// and an absent optional member is a marker no rendered value can equal.
///
/// The rendering is OPAQUE: it is only ever compared to another rendering this function
/// produced, never parsed, and never put on the wire.
pub fn scheduled_report_creation_fingerprint(
    report: &ScheduledReportInput,
    resource_tags: &[ResourceTag],
) -> String {
    /// The marker for an optional member the request left out.
    const ABSENT: &str = "~absent~";

    fn text(value: Option<&str>) -> String {
        value.map_or_else(|| ABSENT.to_string(), |value| format!("{value:?}"))
    }

    fn instant(value: Option<&::aws_smithy_types::DateTime>) -> String {
        value.map_or_else(
            || ABSENT.to_string(),
            |value| format!("{}.{:09}", value.secs(), value.subsec_nanos()),
        )
    }

    fn date_time_value(value: &crate::ops::types::DateTimeValue) -> String {
        format!("{} {}", text(Some(&value.r#type)), text(Some(&value.value)))
    }

    let schedule = resolve_schedule_config(report.schedule_config.clone());
    let period = schedule.schedule_period.as_ref();
    let mut lines = vec![
        format!("name={}", text(Some(&report.name))),
        format!("dashboardArn={}", text(Some(&report.dashboard_arn))),
        format!(
            "executionRoleArn={}",
            text(Some(&report.scheduled_report_execution_role_arn))
        ),
        format!(
            "scheduleExpression={}",
            text(schedule.schedule_expression.as_deref())
        ),
        format!(
            "scheduleTimeZone={}",
            text(schedule.schedule_expression_time_zone.as_deref())
        ),
        format!(
            "schedulePeriod={}",
            if period.is_some() { "present" } else { ABSENT }
        ),
        format!(
            "schedulePeriodStart={}",
            instant(period.and_then(|period| period.start_time.as_ref()))
        ),
        format!(
            "schedulePeriodEnd={}",
            instant(period.and_then(|period| period.end_time.as_ref()))
        ),
        format!("scheduleState={}", text(schedule.state.as_deref())),
        format!("description={}", text(report.description.as_deref())),
    ];
    match &report.widget_ids {
        None => lines.push(format!("widgetIds={ABSENT}")),
        Some(ids) => {
            lines.push(format!("widgetIdCount={}", ids.len()));
            lines.extend(ids.iter().map(|id| format!("widgetId={}", text(Some(id)))));
        }
    }
    match &report.widget_date_range_override {
        None => lines.push(format!("widgetDateRangeOverride={ABSENT}")),
        Some(range) => {
            lines.push(format!(
                "widgetDateRangeStart={}",
                date_time_value(&range.start_time)
            ));
            lines.push(format!(
                "widgetDateRangeEnd={}",
                date_time_value(&range.end_time)
            ));
        }
    }
    let mut tags: Vec<(String, String)> = merge_resource_tags(Vec::new(), resource_tags.to_vec())
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect();
    tags.sort();
    lines.push(format!("tagCount={}", tags.len()));
    lines.extend(tags.iter().map(|(key, value)| {
        format!(
            "tag={} {}",
            text(Some(key.as_str())),
            text(Some(value.as_str()))
        )
    }));
    lines.join("\n")
}

/// The outcome one `ExecuteScheduledReport` call recorded under the report it executed
/// and the `clientToken` it presented, so a RETRY of that same call inside
/// [`EXECUTION_IDEMPOTENCY_WINDOW`] answers that same outcome rather than triggering a
/// second execution.
///
/// The record is keyed by the pair, never by the token alone: the token suppresses a
/// repeat of the request that carried it, and the request names the report, so the same
/// token presented for another report is a different request and is decided on its own
/// merits.
#[derive(Debug, Clone)]
pub struct ScheduledReportExecution {
    pub executed_at: SystemTime,
    pub execution_triggered: bool,
    pub health_status: HealthStatus,
}

/// Per-service state for com-amazonaws-bcmdashboards-awsbcmdashboardsservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Every stored dashboard, keyed by its ARN. A `BTreeMap` so `ListDashboards`
    /// walks the dashboards in one stable order and a cursor taken on one page
    /// resumes at the right place on the next.
    pub dashboards: BTreeMap<String, Dashboard>,
    /// Monotonic counter behind every minted dashboard resource id, so no two
    /// dashboards — including one created after another was deleted — share an ARN.
    pub next_dashboard_seq: u64,
    /// Monotonic counter behind every minted widget id.
    pub next_widget_seq: u64,
    /// Every stored scheduled report, keyed by its ARN. A `BTreeMap` for the same
    /// reason the dashboards are in one: `ListScheduledReports` needs one stable
    /// order to page over.
    pub scheduled_reports: BTreeMap<String, ScheduledReportRecord>,
    /// Monotonic counter behind every minted scheduled-report resource id.
    pub next_scheduled_report_seq: u64,
    /// The `clientToken` of every `CreateScheduledReport` that has created a report,
    /// mapped to that call's outcome. A token makes the create IDEMPOTENT: a later call
    /// presenting the same token for the same request answers the recorded ARN and
    /// creates nothing, one presenting it while that create is still in flight
    /// ([`SCHEDULED_REPORT_CREATION_WINDOW`]) or for a materially different request is
    /// refused with the modelled `ConflictException` — so a retried token can never mint
    /// a second LIVE report. A record whose report is no longer stored is dropped
    /// ([`Self::forget_creations_of_removed_reports`]), which is what keeps the map
    /// bounded by [`MAX_SCHEDULED_REPORTS`] — one record per stored report at most — and
    /// what stops a retry from being answered with an ARN that resolves to nothing.
    pub scheduled_report_client_tokens: BTreeMap<String, ScheduledReportCreation>,
    /// Every `ExecuteScheduledReport` call that has triggered (or declined to trigger)
    /// an execution under a `clientToken`, keyed by the pair (the executed report's ARN,
    /// that token) and mapped to that call's outcome — the scope an idempotency token
    /// has, which is the request it was presented with, not every request sharing its
    /// value. An entry lives for [`EXECUTION_IDEMPOTENCY_WINDOW`] from the call that
    /// wrote it and is dropped once the clock has closed its window
    /// ([`Self::expire_execution_tokens`]), so the map a caller can grow stays bounded by
    /// the calls of one minute.
    pub scheduled_report_executions: BTreeMap<(String, String), ScheduledReportExecution>,
}

/// The ARN namespace this service's ARNs carry, as the model's `aws.api#service`
/// trait declares it (`arnNamespace: bcm-dashboards`).
const ARN_NAMESPACE: &str = "bcm-dashboards";

/// The ARN resource-type segment of a dashboard ARN, as `DashboardArn`'s pattern
/// declares it.
const DASHBOARD_RESOURCE_TYPE: &str = "dashboard";

/// The ARN resource-type segment of a scheduled-report ARN, as `ScheduledReportArn`'s
/// pattern declares it.
const SCHEDULED_REPORT_RESOURCE_TYPE: &str = "scheduled-report";

/// Advance `counter` and render its new value as a resource id: 32 lowercase hex
/// characters, which satisfies both the `[-a-z0-9]+` resource segment the ARN patterns
/// admit and `WidgetId`'s stricter `^[0-9a-f]{32}$`.
///
/// The counter is monotonic, so no two ids this store mints for one resource type are
/// equal — including an id minted after the resource holding the previous one was
/// deleted. `wrapping_add` keeps the bump panic-free in a debug build; a `u64` counter
/// cannot be driven to its ceiling by API calls.
fn next_resource_id(counter: &mut u64) -> String {
    *counter = counter.wrapping_add(1);
    format!("{:032x}", *counter)
}

/// Mint a fresh, unique ARN for one resource type: the next id from `counter` under
/// `resource_type`, with the emulator's canonical account id supplied by
/// `engine::emulator_arn`.
///
/// `bcm-dashboards` ARNs carry no region (the model's ARN patterns have an empty region
/// segment), so the region passed is empty. Every resource type this store mints an ARN
/// for goes through here, so the id width, the region and the account all stay the same
/// shape across types.
fn mint_arn(counter: &mut u64, resource_type: &str) -> String {
    let resource_id = next_resource_id(counter);
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        "",
        &format!("{resource_type}/{resource_id}"),
    )
}

impl ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Mint a fresh, unique dashboard ARN, as [`mint_arn`] mints every resource ARN
    /// this store hands out: a 32-hex-character resource id under the `dashboard`
    /// resource type, which satisfies the `DashboardArn` pattern's `[-a-z0-9]+`
    /// resource segment.
    pub fn mint_dashboard_arn(&mut self) -> String {
        mint_arn(&mut self.next_dashboard_seq, DASHBOARD_RESOURCE_TYPE)
    }

    /// The ARN the dashboard named `name` is stored under, or `None` when no stored
    /// dashboard carries that name.
    ///
    /// A dashboard's name identifies it within the account — `CreateDashboard`'s `name`
    /// is documented as "The name of the dashboard. The name must be unique within your
    /// account." (`smithy_query(query="doc",shape="CreateDashboardRequest",member="name")`)
    /// — so every operation that ASSIGNS a name to a dashboard resolves the name here
    /// before it writes, rather than each one scanning the map itself. The comparison is
    /// exact: the model states no case folding, so two names that differ only in case are
    /// two names.
    pub fn dashboard_arn_by_name(&self, name: &str) -> Option<&str> {
        self.dashboards
            .values()
            .find(|dashboard| dashboard.name == name)
            .map(|dashboard| dashboard.arn.as_str())
    }

    /// The tags persisted on the resource `resource_arn` names, in the order they
    /// were stored, or `None` when this service holds no resource under that ARN.
    ///
    /// The ARN is the only key a tagging operation resolves a resource by, so every
    /// one of them resolves it here rather than reaching into the resource maps
    /// itself. `ResourceArn`'s pattern admits both resource types this service
    /// stores — `dashboard/...` and `scheduled-report/...` — so both are looked up
    /// here; an ARN neither map holds resolves to `None` and the caller answers it with
    /// `crate::scaffolded::Scaffolded::resource_not_found`.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&[ResourceTag]> {
        if let Some(dashboard) = self.dashboards.get(resource_arn) {
            return Some(dashboard.resource_tags.as_slice());
        }
        self.scheduled_reports
            .get(resource_arn)
            .map(|report| report.resource_tags.as_slice())
    }

    /// The tags persisted on the resource `resource_arn` names, for a handler that
    /// CHANGES them, or `None` when this service holds no resource under that ARN.
    ///
    /// Resolves the ARN exactly as [`Self::resource_tags`] does — both resource types
    /// `ResourceArn` admits, in the same order — so a tagging write and a tagging read
    /// always agree on which resource an ARN names and share one copy of its tags.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<ResourceTag>> {
        if let Some(dashboard) = self.dashboards.get_mut(resource_arn) {
            return Some(&mut dashboard.resource_tags);
        }
        self.scheduled_reports
            .get_mut(resource_arn)
            .map(|report| &mut report.resource_tags)
    }

    /// The resource-based policy document attached to the dashboard `resource_arn`
    /// names, or `None` when this service holds no dashboard under that ARN (the
    /// caller answers that with `crate::scaffolded::Scaffolded::resource_not_found`).
    ///
    /// A dashboard with no policy attached resolves to the empty document, which is
    /// what the modelled `policyDocument` member's 0-character minimum admits: a
    /// missing policy is not a missing resource.
    pub fn dashboard_resource_policy(&self, resource_arn: &str) -> Option<&str> {
        self.dashboards
            .get(resource_arn)
            .map(|dashboard| dashboard.resource_policy.as_deref().unwrap_or_default())
    }

    /// Mint a fresh widget id: the same [`next_resource_id`] rendering the ARN minters
    /// use, which is what `WidgetId`'s `^[0-9a-f]{32}$` pattern requires.
    pub fn mint_widget_id(&mut self) -> String {
        next_resource_id(&mut self.next_widget_seq)
    }

    /// Mint a fresh, unique scheduled-report ARN through the same [`mint_arn`] the
    /// dashboard minter uses, under the `scheduled-report` resource type — whose
    /// `ScheduledReportArn` pattern admits the same `[-a-z0-9]+` resource segment.
    pub fn mint_scheduled_report_arn(&mut self) -> String {
        mint_arn(
            &mut self.next_scheduled_report_seq,
            SCHEDULED_REPORT_RESOURCE_TYPE,
        )
    }

    /// The widgets to store for a create or update: the supplied widgets with an
    /// id minted for every one that arrived without one. `width`, `height` and
    /// `horizontalOffset` already carry the model's declared defaults (4, 7, 0),
    /// which the framework fills in before a handler runs.
    pub fn adopt_widgets(&mut self, widgets: Vec<Widget>) -> Vec<Widget> {
        widgets
            .into_iter()
            .map(|mut widget| {
                if widget.id.as_deref().is_none_or(str::is_empty) {
                    widget.id = Some(self.mint_widget_id());
                }
                widget
            })
            .collect()
    }

    /// The health of `report` as this store can establish it at `now`.
    ///
    /// The model documents `HEALTHY` as "the scheduled report is configured properly and
    /// has all required permissions to execute" and `UNHEALTHY` as the report being
    /// unable to deliver, with the reasons in `statusReasons`
    /// (`smithy_query(query="shape",shape="HealthStatus")`). So the status is DERIVED
    /// from what this store holds, never from a request: a report is healthy exactly
    /// while every resource its configuration points at still resolves —
    ///
    /// * the dashboard its `dashboardArn` names is still stored
    ///   ([`STATUS_REASON_DASHBOARD_NOT_FOUND`] when it is not), and
    /// * every id in its `widgetIds` is still a widget of that dashboard
    ///   ([`STATUS_REASON_WIDGET_ID_NOT_FOUND`] when one is not).
    ///
    /// Reasons are reported in that order and at most once each. The other
    /// `StatusReason` values name failures of resources outside this service (the
    /// execution role, the cost data source), which this store holds nothing about and
    /// therefore never claims.
    ///
    /// `lastRefreshedAt` is `now` because this call is the refresh.
    pub fn scheduled_report_health(
        &self,
        report: &ScheduledReportRecord,
        now: SystemTime,
    ) -> HealthStatus {
        let mut status_reasons: Vec<String> = Vec::new();
        match self.dashboards.get(&report.dashboard_arn) {
            None => status_reasons.push(STATUS_REASON_DASHBOARD_NOT_FOUND.to_string()),
            Some(dashboard) => {
                let present: BTreeSet<&str> = dashboard
                    .widgets
                    .iter()
                    .filter_map(|widget| widget.id.as_deref())
                    .collect();
                let names_a_missing_widget = report
                    .widget_ids
                    .iter()
                    .flatten()
                    .any(|id| !present.contains(id.as_str()));
                if names_a_missing_widget {
                    status_reasons.push(STATUS_REASON_WIDGET_ID_NOT_FOUND.to_string());
                }
            }
        }

        let healthy = status_reasons.is_empty();
        HealthStatus {
            status_code: if healthy {
                HEALTH_STATUS_HEALTHY.to_string()
            } else {
                HEALTH_STATUS_UNHEALTHY.to_string()
            },
            last_refreshed_at: Some(::aws_smithy_types::DateTime::from(now)),
            // The model documents `statusReasons` as "only present when the status is
            // UNHEALTHY", so a healthy report carries none rather than an empty list.
            status_reasons: (!healthy).then_some(status_reasons),
        }
    }

    /// Drop every create idempotency record whose scheduled report this service no
    /// longer stores.
    ///
    /// A record exists to answer the retry of the create it describes with that create's
    /// outcome; once the report it minted is gone, that outcome is gone with it, so the
    /// record would otherwise have a retry answered with an ARN nothing resolves to. The
    /// removal of the report is what makes the record due for dropping — it was owed at
    /// that moment, whether or not anybody called afterwards — and dropping it is also
    /// what bounds the map by the reports actually stored
    /// ([`MAX_SCHEDULED_REPORTS`]) rather than by every token ever presented.
    pub fn forget_creations_of_removed_reports(&mut self) {
        let reports = &self.scheduled_reports;
        self.scheduled_report_client_tokens
            .retain(|_, creation| reports.contains_key(&creation.arn));
    }

    /// Drop every execution idempotency record whose [`EXECUTION_IDEMPOTENCY_WINDOW`]
    /// the clock has already closed at `now`.
    ///
    /// This is the one write an `ExecuteScheduledReport` call makes before it decides
    /// anything: the expiry was owed at `now` whether or not anybody called, so
    /// materialising it changes nothing a later caller could otherwise have seen — it
    /// only stops an expired token from being read as a retry.
    pub fn expire_execution_tokens(&mut self, now: SystemTime) {
        self.scheduled_report_executions.retain(|_, execution| {
            // A clock that went backwards yields `Err` here; treat that as "no time has
            // passed" and keep the record rather than dropping it early.
            now.duration_since(execution.executed_at)
                .unwrap_or_default()
                < EXECUTION_IDEMPOTENCY_WINDOW
        });
    }
}

/// The tracing target every audit record below is emitted on, so a host can route or
/// filter the audit stream on its own. Keep it equal to the `target:` literal in
/// [`record_state_change`] — the macro needs a literal there.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the audit record a state-changing operation owes: when it happened, where the
/// request came from, which access key id it presented, which operation ran, which
/// resource it touched and how it came out.
///
/// Lives here rather than in one operation's file because every state-changing
/// dashboard operation emits the same record. Only the presented access key id is
/// recorded — never a secret, and never the emulator's own credentials. Reporting
/// operations do not call this: they change nothing to account for.
pub fn record_state_change(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    outcome: &str,
) {
    let headers = &ctx.parts().headers;
    let source = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    let timestamp_ms = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| {
            u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
        });
    ::tracing::info!(
        target: "emulator::audit",
        timestamp_ms,
        source,
        akid = ::engine::extract_akid(headers),
        operation,
        resource,
        outcome,
        "state-changing bcm-dashboards operation"
    );
}

/// How every operation that ASSIGNS a dashboard name words the refusal of a name the
/// account already holds.
///
/// Built from the request, the way the boundary validator words a refusal: it names the
/// member that broke the constraint, the value it carried and the constraint itself. One
/// helper so `CreateDashboard` and `UpdateDashboard` cannot drift apart on the wording of
/// one rule.
pub fn duplicate_dashboard_name_message(name: &str) -> String {
    format!(
        "name: a dashboard named '{name}' already exists in this account; \
         the name of a dashboard must be unique within your account"
    )
}

/// Whether `key` is a tag key Amazon Web Services reserves for itself (see
/// [`RESERVED_TAG_KEY_PREFIX`]).
///
/// `str::get` rather than a byte-range slice (`key[..4]`), which PANICS when the key
/// carries a multi-byte character straddling that byte index — `ResourceTagKey`'s
/// modelled pattern is `^[\S\s]*$`, which admits any character at all, so such a key
/// passes boundary validation and reaches this helper. `get` answers `None` for a key
/// shorter than the prefix or one whose boundary falls mid-character, which is exactly
/// "not reserved".
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Ok(())` when no key in `keys` is in the reserved namespace, or the refusal message
/// for the ones that are — the text a `ValidationException` carries.
///
/// One helper so every operation that takes caller-supplied tag keys —
/// `CreateDashboard` and `CreateScheduledReport` at create time, `TagResource` and
/// `UntagResource` afterwards — refuses that one namespace the same way and cannot drift
/// apart. The message is built from the request, the way the boundary validator words a
/// refusal: it names the member that broke the rule, the offending keys and the rule
/// itself. The offenders are sorted and deduplicated so one request that names a key
/// twice reports it once and the text does not depend on iteration order.
pub fn check_tag_keys_not_reserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    Err(format!(
        "{member}: the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved for tags Amazon Web \
         Services generates and cannot be set or removed by a caller, so these keys are not \
         allowed: {}",
        reserved.join(", ")
    ))
}

/// The tag list to STORE when `incoming` is applied to the resource that currently
/// carries `stored`: the tags resolved by tag key, so one key holds one value.
///
/// A resource's tags are a map keyed by tag key, while `ResourceTagList` is a plain
/// list the model lets repeat a key (length 0..200, no `@uniqueItems`,
/// `smithy_query(query="shape",shape="ResourceTagList")`), so every path that accepts
/// caller-supplied tags resolves that list the same way here rather than each one
/// deciding for itself:
///
/// * a key the resource already carries takes the applied value IN PLACE, keeping its
///   position in the stored list, so the order a tagging read reports stays stable;
/// * a key it does not carry yet is appended in the order `incoming` first names it;
/// * a key the request repeats settles on its LAST value — the same answer as applying
///   those entries one request at a time;
/// * a key `incoming` does not name keeps its stored value.
///
/// Create-time tagging passes an empty `stored`, which is the same rule with nothing to
/// update: the request's own repeated keys are resolved, so no resource ever enters the
/// store carrying one key twice — a state no later tagging call could produce, and one
/// that would make a read report two values for one key and an update reach only the
/// first copy.
pub fn merge_resource_tags(
    stored: Vec<ResourceTag>,
    incoming: impl IntoIterator<Item = ResourceTag>,
) -> Vec<ResourceTag> {
    let mut merged = stored;
    for tag in incoming {
        match merged.iter_mut().find(|stored| stored.key == tag.key) {
            Some(stored) => stored.value = tag.value,
            None => merged.push(tag),
        }
    }
    merged
}

/// The `scheduleConfig` to store for a create or an update: the supplied config with
/// `state` resolved to [`SCHEDULE_STATE_ENABLED`] when the request left it unset, so
/// every reader sees a schedule whose state is decided once, at write time, rather
/// than each reader guessing.
pub fn resolve_schedule_config(mut config: ScheduleConfig) -> ScheduleConfig {
    if config.state.as_deref().is_none_or(str::is_empty) {
        config.state = Some(SCHEDULE_STATE_ENABLED.to_string());
    }
    config
}

/// Encode the ARN a listing page resumes at as an opaque `nextToken`.
pub fn encode_page_token(arn: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(arn))
}

/// The ARN a `nextToken` resumes at, or `None` when the token is not one this
/// service minted for a listing over `arn_segment`'s resource type (the caller
/// answers that with a `ValidationException`).
///
/// A cursor this service minted always names an ARN of the resource the listing
/// walks. It is checked against the fixed segments of that resource's ARN pattern
/// (`^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource>/...`), so a token that
/// decodes to anything else — including a cursor minted by a listing over the other
/// resource type — is rejected rather than read as a resume point past every stored
/// record.
pub fn decode_page_token(token: &str, arn_segment: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    let names_the_resource = arn.starts_with("arn:aws")
        && arn.contains(":bcm-dashboards::")
        && arn.contains(arn_segment);
    names_the_resource.then_some(arn)
}

/// One page of a listing over an ARN-keyed resource map: the records of the page,
/// already projected onto the shape the listing answers, and the cursor the next page
/// resumes at — present exactly when a record remains after the page.
#[derive(Debug)]
pub struct ListingPage<T> {
    /// The page's records, in ARN order.
    pub items: Vec<T>,
    /// The `nextToken` to answer, or `None` when the page exhausted the map.
    pub next_token: Option<String>,
}

/// Why a listing request names no page this service can serve. The listing turns it
/// into its own modelled `ValidationException` with [`ListingRefusal::message`], which
/// is the only part of the refusal that differs between listings.
#[derive(Debug)]
pub enum ListingRefusal {
    /// The request carried a `nextToken` this service did not mint for a listing over
    /// the resource type walked.
    UninterpretableToken,
    /// The request asked for a page of this many records, which is not a page any
    /// listing can answer.
    NonPositivePageSize(i32),
}

impl ListingRefusal {
    /// The `ValidationException` message for this refusal of `operation`'s listing.
    pub fn message(&self, operation: &str) -> String {
        match self {
            Self::UninterpretableToken => format!(
                "Member must be a nextToken returned by a previous {operation} call: nextToken"
            ),
            Self::NonPositivePageSize(max_results) => format!(
                "Member must have value greater than or equal to 1: maxResults {max_results}"
            ),
        }
    }
}

/// One page of a listing over `records`, an ARN-keyed map of this service's resources.
///
/// Every listing this service serves pages the same way, so the rule lives here once
/// rather than once per list handler: a `next_token` is the ARN to resume at, decoded
/// for the resource type `arn_segment` names and refused when it is not a cursor this
/// service minted for that type; `max_results` is the page size, refused when it is not
/// at least one (a zero-item page that still carried a cursor would be a client loop
/// that never terminates); the walk starts at the cursor's ARN inclusive, or at the
/// first stored record, takes at most a page of records and projects each one with
/// `project`; and the cursor minted is the ARN of the first record AFTER the page, so
/// `next_token` is present exactly when one remains.
///
/// The token is decoded before the page size is read, so a request that is wrong in
/// both ways is answered for its token.
pub fn listing_page<'records, R, T>(
    records: &'records BTreeMap<String, R>,
    next_token: Option<&str>,
    max_results: i32,
    arn_segment: &str,
    mut project: impl FnMut(&'records R) -> T,
) -> Result<ListingPage<T>, ListingRefusal> {
    let resume_from = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token, arn_segment).ok_or(ListingRefusal::UninterpretableToken)?,
        ),
    };
    let page_size = match usize::try_from(max_results) {
        Ok(size) if size >= 1 => size,
        _ => return Err(ListingRefusal::NonPositivePageSize(max_results)),
    };
    // Borrowed bounds, so resuming copies no ARN.
    let start = match resume_from.as_deref() {
        Some(arn) => Bound::Included(arn),
        None => Bound::Unbounded,
    };
    let mut remaining = records.range::<str, _>((start, Bound::Unbounded));
    let items: Vec<T> = remaining
        .by_ref()
        .take(page_size)
        .map(|(_, record)| project(record))
        .collect();
    let next_token = remaining.next().map(|(arn, _)| encode_page_token(arn));
    Ok(ListingPage { items, next_token })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>)
}
