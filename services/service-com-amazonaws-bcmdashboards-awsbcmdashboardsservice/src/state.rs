// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdashboards-awsbcmdashboardsservice service.
//!
//! The dashboard records every dashboard operation reads and writes live here, so
//! `CreateDashboard`, `GetDashboard`, `ListDashboards`, `UpdateDashboard` and
//! `DeleteDashboard` all see one copy of each resource. The helpers below (ARN and
//! widget-id minting, the `DashboardReference` projection, the tag resolution the
//! tagging operations share, the pagination cursor codec) are shared by those handlers
//! rather than duplicated per operation.
//!
//! # An execution of a scheduled report is a delivery
//!
//! A scheduled report's whole purpose is a notification on the account's default Amazon
//! EventBridge event bus: `HealthStatus$statusCode` documents `UNHEALTHY` as "the
//! scheduled report is unable to deliver the notification to the default Amazon
//! EventBridge EventBus in your account". So an execution is not internal bookkeeping —
//! it hands one event to that bus ([`deliver_execution_notification`]), and the health
//! the execution establishes reports whether that delivery succeeded
//! ([`undelivered_notification_health`]). EventBridge is reached the way any emulated
//! service reaches another: over the in-process callback, so nothing is dialled and this
//! crate names no other service crate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    CostAndUsageQuery, DashboardReference, DateTimeRange, DateTimeValue, DisplayConfig,
    GraphDisplayConfig, GroupDefinition, HealthStatus, QueryParameters, ReservationUtilizationQuery,
    ResourceTag, SavingsPlansCoverageQuery, ScheduleConfig, ScheduledReport, ScheduledReportInput,
    ScheduledReportSummary, TableDisplayConfigStruct, Widget, WidgetConfig,
};

/// The only `DashboardType` the model declares (`DASHBOARD_TYPE_VALUES`), and the
/// type every dashboard a client creates through this API carries.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";

/// The `DashboardType` the dashboards Amazon Web Services publishes itself carry.
///
/// The model's `DashboardType` enum lists `CUSTOM` alone
/// (`smithy_query(query="shape",shape="DashboardType")`), while every reference in the
/// AWS-managed catalog this service answers a brand-new account with carries
/// `AWS_MANAGED`. A dashboard's `type` travels as a string on the wire, so the value is
/// representable: it is stored on the record and projected like any other, rather than
/// derived per request.
pub const DASHBOARD_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The `policyDocument` a dashboard with no resource-based policy attached reads back:
/// the empty JSON document.
///
/// `GetResourcePolicy$policyDocument` is @required and reports a policy DOCUMENT, which
/// a caller feeds straight to a JSON parser — so "no policy" is the document that holds
/// no statement, which parses to an empty policy. The empty STRING is not a JSON document
/// at all and fails that parse, even though `GenericString`'s modelled length (0..1024)
/// would admit it.
pub const EMPTY_RESOURCE_POLICY_DOCUMENT: &str = "{}";

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

/// The `StatusReason` values this store can DERIVE from the resources a scheduled
/// report's configuration points at, so resolving a report's health re-derives them
/// from what the store holds now instead of repeating what an earlier refresh found.
/// Every other reason a refresh established is carried forward from the record, because
/// nothing in the store can re-establish it.
const DERIVED_STATUS_REASONS: [&str; 2] = [
    STATUS_REASON_DASHBOARD_NOT_FOUND,
    STATUS_REASON_WIDGET_ID_NOT_FOUND,
];

/// The `StatusReason` (`STATUS_REASON_VALUES`) for an execution whose notification the
/// account's default event bus did not take: the report's configuration resolved, so the
/// failure is neither a missing dashboard nor a missing widget — it is the delivery
/// itself, which is what this value names.
pub const STATUS_REASON_INTERNAL_FAILURE: &str = "INTERNAL_FAILURE";

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
    /// through this API keeps `None` and reads back
    /// [`EMPTY_RESOURCE_POLICY_DOCUMENT`]. The field is the single place a policy
    /// would live once an operation can write one, so the reader needs no second map
    /// beside the dashboards.
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

/// The resource-id prefix of every AWS-managed dashboard's ARN, which is what tells one
/// apart from a dashboard a caller created: the catalog's ARNs are
/// `arn:aws:bcm-dashboards::<account>:dashboard/managed-<32 hex>`, while
/// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::mint_dashboard_arn`] mints a
/// bare 32-hex resource id. Both spellings satisfy `DashboardArn`'s `[-a-z0-9]+` resource
/// segment.
const MANAGED_DASHBOARD_ID_PREFIX: &str = "managed-";

/// When the AWS-managed catalog this service seeds was published, and when it was last
/// revised, as seconds since the Unix epoch.
///
/// The catalog is Amazon Web Services' own data, not the account's: its dashboards exist
/// before a caller writes anything, so their `createdAt`/`updatedAt` are the instants AWS
/// published and last revised the catalog rather than anything this emulator's clock
/// produced — which is why they are stored constants and are stable across reads and
/// across restarts. A dashboard a CALLER creates is stamped from the clock
/// (`CreateDashboard`), as its own record's instants.
const MANAGED_CATALOG_CREATED_AT: u64 = 1_786_644_000;
const MANAGED_CATALOG_UPDATED_AT: u64 = 1_787_079_600;

/// The `DateTimeType` (`DATE_TIME_TYPE_VALUES`) a widget's time range uses when its
/// bounds are offsets from the instant the widget is rendered rather than fixed dates.
const DATE_TIME_TYPE_RELATIVE: &str = "RELATIVE";

/// The `Granularity` (`GRANULARITY_VALUES`) every widget in the AWS-managed catalog
/// aggregates at: a month, which is the period a bill covers.
const GRANULARITY_MONTHLY: &str = "MONTHLY";

/// The `GroupDefinitionType` (`GROUP_DEFINITION_TYPE_VALUES`) a grouping by a cost
/// dimension carries — also the member's modelled `@default`.
const GROUP_DEFINITION_TYPE_DIMENSION: &str = "DIMENSION";

/// A widget time range running from `start` to `end`, both relative offsets (ISO 8601
/// durations, as `DateTimeValue$value` carries them for a `RELATIVE` bound).
fn relative_range(start: &str, end: &str) -> DateTimeRange {
    DateTimeRange {
        start_time: DateTimeValue {
            r#type: DATE_TIME_TYPE_RELATIVE.to_string(),
            value: start.to_string(),
        },
        end_time: DateTimeValue {
            r#type: DATE_TIME_TYPE_RELATIVE.to_string(),
            value: end.to_string(),
        },
    }
}

/// A `DisplayConfig` drawing one metric as `visual_type` (a `VisualType`:
/// `VISUAL_TYPE_VALUES` are `LINE`, `BAR`, `STACK`).
fn graph_display(metric: &str, visual_type: &str) -> DisplayConfig {
    DisplayConfig::Graph(HashMap::from([(
        metric.to_string(),
        GraphDisplayConfig {
            visual_type: visual_type.to_string(),
        },
    )]))
}

/// A `DisplayConfig` drawing the query's result as a table.
fn table_display() -> DisplayConfig {
    DisplayConfig::Table(TableDisplayConfigStruct {})
}

/// One widget of the AWS-managed catalog: the members a caller's widget carries too,
/// with the model's declared `width`/`height`/`horizontalOffset` defaults (4, 7, 0)
/// unless the catalog entry states its own.
struct ManagedWidget {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    width: i32,
    height: i32,
    configs: Vec<WidgetConfig>,
}

impl ManagedWidget {
    /// This catalog widget as the modelled [`Widget`] every reader projects.
    fn widget(self) -> Widget {
        Widget {
            id: Some(self.id.to_string()),
            title: self.title.to_string(),
            description: Some(self.description.to_string()),
            width: self.width,
            height: self.height,
            horizontal_offset: 0,
            configs: self.configs,
        }
    }
}

/// The widgets of the `Cost Overview and Trends` catalog entry.
fn cost_overview_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "a18b58807f3934dbcfebd21ad359afaa",
        title: "Total Current And Forecasted Cost",
        description: "Historical monthly unblended cost with forecasted cost.",
        width: 2,
        height: 4,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::costAndUsage(CostAndUsageQuery {
                metrics: vec!["UnblendedCost".to_string()],
                time_range: relative_range("-P12M", "P4M"),
                granularity: GRANULARITY_MONTHLY.to_string(),
                group_by: Some(Vec::new()),
                filter: None,
            }),
            display_config: graph_display("UnblendedCost", "BAR"),
        }],
    }
    .widget()]
}

/// The widgets of a catalog entry that reports cost and usage grouped by one
/// `Dimension` (`DIMENSION_VALUES`) — the shape the `Compute` and `Database` entries
/// take, each over its own dimension.
fn cost_by_dimension_widgets(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    dimension: &'static str,
) -> Vec<Widget> {
    vec![ManagedWidget {
        id,
        title,
        description,
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::costAndUsage(CostAndUsageQuery {
                metrics: vec!["UnblendedCost".to_string()],
                time_range: relative_range("-P6M", "P1M"),
                granularity: GRANULARITY_MONTHLY.to_string(),
                group_by: Some(vec![GroupDefinition {
                    key: dimension.to_string(),
                    r#type: GROUP_DEFINITION_TYPE_DIMENSION.to_string(),
                }]),
                filter: None,
            }),
            display_config: graph_display("UnblendedCost", "STACK"),
        }],
    }
    .widget()]
}

/// The widgets of the `Reservations` catalog entry: how much of the account's
/// reservations were used.
fn reservation_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "c5a7d0b3e19f4c82a6d4b8e01f3c7a95",
        title: "Reservation Utilization",
        description: "Monthly utilization of your Reserved Instances.",
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::reservationUtilization(
                ReservationUtilizationQuery {
                    time_range: relative_range("-P6M", "P0M"),
                    group_by: None,
                    granularity: Some(GRANULARITY_MONTHLY.to_string()),
                    filter: None,
                },
            ),
            display_config: table_display(),
        }],
    }
    .widget()]
}

/// The widgets of the `Savings Plans` catalog entry: how much of the account's eligible
/// spend the Savings Plans it holds covered.
fn savings_plans_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "d6b8e1c4f20a5d93b7e5c9f12a4d8b06",
        title: "Savings Plans Coverage",
        description: "Monthly share of eligible spend covered by your Savings Plans.",
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::savingsPlansCoverage(SavingsPlansCoverageQuery {
                time_range: relative_range("-P6M", "P0M"),
                metrics: Some(vec!["SpendCoveredBySavingsPlans".to_string()]),
                granularity: Some(GRANULARITY_MONTHLY.to_string()),
                group_by: None,
                filter: None,
            }),
            display_config: table_display(),
        }],
    }
    .widget()]
}

/// One stored dashboard of the AWS-managed catalog, built the same way a created one is:
/// the record carries its own ARN, name, description, type and widgets, and every reader
/// projects it from the store.
fn managed_dashboard(
    resource_id: &str,
    name: &str,
    description: &str,
    widgets: Vec<Widget>,
) -> Dashboard {
    Dashboard {
        arn: ::engine::emulator_arn(
            ARN_NAMESPACE,
            "",
            &format!("{DASHBOARD_RESOURCE_TYPE}/{MANAGED_DASHBOARD_ID_PREFIX}{resource_id}"),
        ),
        name: name.to_string(),
        description: Some(description.to_string()),
        dashboard_type: DASHBOARD_TYPE_AWS_MANAGED.to_string(),
        widgets,
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_CATALOG_CREATED_AT),
        updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_CATALOG_UPDATED_AT),
        // The catalog carries no tags and no resource-based policy: a tag is an
        // account's own metadata, and this model declares no operation that attaches a
        // policy at all.
        resource_tags: Vec::new(),
        resource_policy: None,
    }
}

/// The AWS-managed dashboard catalog every account already holds, keyed by ARN exactly
/// as a created dashboard is.
///
/// These dashboards are the service's OWN data rather than the account's: an account that
/// has created nothing still lists them, and `GetDashboard` reads one back with its
/// widgets, which is the first call a console-like client makes. They are therefore
/// SEEDED into the store ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::default`])
/// and read back through the same map, the same projection and the same pagination as a
/// dashboard a caller created — no handler branches on whether a dashboard is managed, and
/// nothing about the catalog is derived from a request.
///
/// `pub` so a test can state the catalog a listing must report alongside its own
/// dashboards from the one place that defines it.
pub fn managed_dashboard_catalog() -> BTreeMap<String, Dashboard> {
    [
        managed_dashboard(
            "4dc5619130f2463687fc45e003e4b930",
            "Cost Overview and Trends",
            "High-level view of your AWS costs, usage trends, and forecasts",
            cost_overview_widgets(),
        ),
        managed_dashboard(
            "6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d",
            "Compute",
            "Cost and usage of your compute services, including Amazon EC2, AWS Lambda and \
             AWS Fargate",
            cost_by_dimension_widgets(
                "b29c69918a0f45e3d7fce32be46a0bb6",
                "Compute Cost By Instance Type",
                "Monthly unblended cost of your compute usage, grouped by instance type.",
                "INSTANCE_TYPE",
            ),
        ),
        managed_dashboard(
            "7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e",
            "Database",
            "Cost and usage of your database services, including Amazon RDS, Amazon DynamoDB \
             and Amazon ElastiCache",
            cost_by_dimension_widgets(
                "e7c9f2a5b38d4e06c1a9f7b2d5e8c304",
                "Database Cost By Engine",
                "Monthly unblended cost of your database usage, grouped by database engine.",
                "DATABASE_ENGINE",
            ),
        ),
        managed_dashboard(
            "8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f",
            "Reservations",
            "Utilization and coverage of the Reserved Instances in your account",
            reservation_widgets(),
        ),
        managed_dashboard(
            "9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a",
            "Savings Plans",
            "Utilization and coverage of the Savings Plans in your account",
            savings_plans_widgets(),
        ),
    ]
    .into_iter()
    .map(|dashboard| (dashboard.arn.clone(), dashboard))
    .collect()
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
/// A reader never projects `health_status` directly — the health every view reports is
/// resolved from the whole store by
/// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`],
/// because it depends on the resources this record points at and not on this record
/// alone.
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
    /// readers project
    /// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`],
    /// never this member.
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

    /// This record projected onto the modelled `ScheduledReport` shape that
    /// `GetScheduledReport` (and the other readers) return: the stored record's own
    /// fields, never request input.
    ///
    /// `healthStatus` is optional on this shape and `@required` on the summary, but it
    /// is the same property of the same record either way, so the detail view reports
    /// exactly the `health` its caller resolved rather than reporting less than the
    /// listing does. The resolution is the store's
    /// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`]),
    /// never a projection's own.
    pub fn report(&self, health: HealthStatus) -> ScheduledReport {
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
            health_status: Some(health),
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
    ///   and is the `health` its caller resolved from the store
    ///   ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`])
    ///   — the same resolution the detail view uses, so both views of one record report
    ///   one health.
    pub fn summary(&self, health: HealthStatus) -> ScheduledReportSummary {
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
            health_status: health,
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

    /// The `ConflictException` message a `CreateScheduledReport` call presenting this
    /// record's `clientToken` is refused with at `now`.
    ///
    /// The service words the refusal from the state of the create the token already
    /// started, and the wording is part of the contract: a client decides from it whether
    /// to back off and retry (the creation is still in flight, so the conflict is
    /// transient) or to pick another token (the report exists, so the conflict is
    /// terminal). Both texts are therefore FIXED; the only value either of them carries
    /// is the ARN this record stores, which the caller may act on. The presented token is
    /// deliberately absent from both: it is the caller's idempotency key, and an error
    /// body is not where it belongs.
    pub fn conflict_message(&self, now: SystemTime) -> String {
        if self.in_progress_at(now) {
            "A scheduled report creation is already in progress for the provided \
             clientToken. Please retry after some time."
                .to_string()
        } else {
            format!(
                "A scheduled report with the provided clientToken already exists (ARN: {}). \
                 Use a different clientToken to create a new scheduled report, or use the \
                 existing report's ARN to update it.",
                self.arn
            )
        }
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
#[derive(Debug)]
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

/// A fresh store already holds the AWS-managed dashboard catalog
/// ([`managed_dashboard_catalog`]) and nothing else: those dashboards are the service's
/// own, so they exist before any caller writes anything, which is what `ListDashboards`
/// reports on a brand-new account and what `GetDashboard` reads back. Everything else
/// starts empty — an account owns no custom dashboard and no scheduled report until it
/// creates one.
impl Default for ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    fn default() -> Self {
        Self {
            dashboards: managed_dashboard_catalog(),
            next_dashboard_seq: 0,
            next_widget_seq: 0,
            scheduled_reports: BTreeMap::new(),
            next_scheduled_report_seq: 0,
            scheduled_report_client_tokens: BTreeMap::new(),
            scheduled_report_executions: BTreeMap::new(),
        }
    }
}

/// The ARN namespace this service's ARNs carry, as the model's `aws.api#service`
/// trait declares it (`arnNamespace: bcm-dashboards`).
const ARN_NAMESPACE: &str = "bcm-dashboards";

/// The ARN resource-type segment of a dashboard ARN, as `DashboardArn`'s pattern
/// declares it. Also the resource type a `ListDashboards` cursor's ARN names.
pub const DASHBOARD_RESOURCE_TYPE: &str = "dashboard";

/// The ARN resource-type segment of a scheduled-report ARN, as `ScheduledReportArn`'s
/// pattern declares it. Also the resource type a `ListScheduledReports` cursor's ARN
/// names.
pub const SCHEDULED_REPORT_RESOURCE_TYPE: &str = "scheduled-report";

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
    /// A dashboard with no policy attached resolves to
    /// [`EMPTY_RESOURCE_POLICY_DOCUMENT`]: a missing policy is not a missing resource,
    /// and `policyDocument` reports a POLICY DOCUMENT, so the absence of one is the
    /// document that grants nothing rather than a value that is not a document at all.
    /// A stored document that holds no characters says the same thing, so it resolves
    /// the same way.
    pub fn dashboard_resource_policy(&self, resource_arn: &str) -> Option<&str> {
        self.dashboards.get(resource_arn).map(|dashboard| {
            dashboard
                .resource_policy
                .as_deref()
                .filter(|document| !document.is_empty())
                .unwrap_or(EMPTY_RESOURCE_POLICY_DOCUMENT)
        })
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

    /// The reasons the resources `report`'s stored configuration points at do not resolve
    /// in this store, in the order the status reports them and at most once each:
    ///
    /// * the dashboard its `dashboardArn` names is no longer stored
    ///   ([`STATUS_REASON_DASHBOARD_NOT_FOUND`]), and
    /// * an id in its `widgetIds` is no longer a widget of that dashboard
    ///   ([`STATUS_REASON_WIDGET_ID_NOT_FOUND`]).
    ///
    /// Derived from what the store holds AT THE CALL, never from a request and never
    /// from what an earlier refresh found: the dashboard and the widgets a report points
    /// at can be deleted, replaced or repointed after any refresh, so the reasons that
    /// name them are re-derived every time the health is resolved.
    ///
    /// The other `StatusReason` values name failures of resources outside this service
    /// (the execution role, the cost data source), which this store holds nothing about
    /// and therefore never claims.
    fn unresolved_configuration_reasons(&self, report: &ScheduledReportRecord) -> Vec<String> {
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
        status_reasons
    }

    /// The health of `report` as this store establishes it at `now`, for the refresh that
    /// is running.
    ///
    /// The model documents `HEALTHY` as "the scheduled report is configured properly and
    /// has all required permissions to execute" and `UNHEALTHY` as the report being
    /// unable to deliver, with the reasons in `statusReasons`
    /// (`smithy_query(query="shape",shape="HealthStatus")`). So the status is DERIVED
    /// from what this store holds, never from a request: a report is healthy exactly
    /// while every resource its configuration points at still resolves
    /// ([`Self::unresolved_configuration_reasons`]).
    ///
    /// `lastRefreshedAt` is `now` because this call is the refresh.
    pub fn scheduled_report_health(
        &self,
        report: &ScheduledReportRecord,
        now: SystemTime,
    ) -> HealthStatus {
        let status_reasons = self.unresolved_configuration_reasons(report);
        // `lastRefreshedAt` is `now`: this call IS the refresh.
        health_status(status_reasons, Some(::aws_smithy_types::DateTime::from(now)))
    }

    /// The health EVERY view of `report` reports: `HealthStatus$statusCode` with the
    /// reasons behind it, resolved from this store.
    ///
    /// Health is a property of the RESOURCE and of the resources it points at, so it is
    /// resolved here — once, from the whole store — rather than projected out of the
    /// record by each view: no two views of one report can disagree about it, and none
    /// reports a status the store itself contradicts.
    ///
    /// * The reasons that name resources this store holds are re-derived from it
    ///   ([`Self::unresolved_configuration_reasons`]), so a report whose dashboard was
    ///   deleted — or whose named widget was removed from it — is reported `UNHEALTHY`
    ///   with those reasons whether or not anything has ever refreshed the record.
    /// * A reason the last refresh established that this store cannot re-derive (a
    ///   notification the account's default event bus did not take,
    ///   [`STATUS_REASON_INTERNAL_FAILURE`]) is carried forward from the record: the
    ///   refresh is the only evidence of it.
    /// * `lastRefreshedAt` is documented as "the timestamp when the health status was
    ///   last refreshed", so it is the instant of the record's last refresh and stays
    ///   absent until one has happened — resolving the health for a reader is not a
    ///   refresh, and this call writes nothing.
    ///
    /// A record with no refresh and every resource resolving is therefore
    /// [`HEALTH_STATUS_HEALTHY`] with no reasons and no refresh instant — the model's
    /// "configured properly and has all required permissions to execute", which is what
    /// a report this service accepted and whose resources still resolve is.
    pub fn reported_scheduled_report_health(
        &self,
        report: &ScheduledReportRecord,
    ) -> HealthStatus {
        let mut status_reasons = self.unresolved_configuration_reasons(report);
        let refreshed = report.health_status.as_ref();
        let refreshed_reasons = refreshed
            .and_then(|health| health.status_reasons.as_ref())
            .into_iter()
            .flatten();
        for reason in refreshed_reasons {
            if !DERIVED_STATUS_REASONS.contains(&reason.as_str())
                && !status_reasons.contains(reason)
            {
                status_reasons.push(reason.clone());
            }
        }

        health_status(
            status_reasons,
            refreshed.and_then(|health| health.last_refreshed_at),
        )
    }

    /// `report` projected onto the modelled `ScheduledReport` the detail readers answer,
    /// with the health this store resolves for it.
    pub fn scheduled_report_detail(&self, report: &ScheduledReportRecord) -> ScheduledReport {
        report.report(self.reported_scheduled_report_health(report))
    }

    /// `report` projected onto the modelled `ScheduledReportSummary` the listing pages
    /// over, with the health this store resolves for it — the SAME resolution the detail
    /// view gets.
    pub fn scheduled_report_summary(
        &self,
        report: &ScheduledReportRecord,
    ) -> ScheduledReportSummary {
        report.summary(self.reported_scheduled_report_health(report))
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

/// The `@pattern` of `DashboardName` and `ScheduledReportName`, as the model writes it
/// (`smithy_query(query="shape",shape="DashboardName,ScheduledReportName")`): both shapes
/// declare this one expression.
pub const RESOURCE_NAME_PATTERN: &str = r"^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$";

/// The `@pattern` of `Description`, as the model writes it
/// (`smithy_query(query="shape",shape="Description")`) — the shape every `description`
/// member of this service targets, on a dashboard, a widget and a scheduled report alike.
pub const DESCRIPTION_PATTERN: &str = r"^(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*$";

/// The `@pattern` of `WidgetTitle`, as the model writes it
/// (`smithy_query(query="shape",shape="WidgetTitle")`).
pub const WIDGET_TITLE_PATTERN: &str =
    r"^(?!.* {2})[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]$";

/// A constrained string of this service whose modelled `@pattern` the boundary validator
/// does NOT apply, so the request reaches a handler unchecked against it.
///
/// Each of these expressions opens with the negative lookahead `(?!.* {2})` — no two
/// consecutive spaces anywhere in the value — and the boundary's regex engine compiles no
/// lookahead; a pattern it cannot compile is treated as matching
/// (`aws_smithy_schema_server::constraints`'s pattern check). So the three grammars are
/// applied HERE, by the same clause and envelope the boundary states for a pattern it can
/// compile ([`text_pattern_refusal`]), before the handler writes anything. The `@length`
/// bounds on the same members compile and are already enforced at the boundary, so they
/// are not repeated — which is also what bounds the value a clause echoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstrainedText {
    /// A resource name: [`RESOURCE_NAME_PATTERN`].
    ResourceName,
    /// A description: [`DESCRIPTION_PATTERN`].
    Description,
    /// A widget title: [`WIDGET_TITLE_PATTERN`].
    WidgetTitle,
}

/// The characters `[a-zA-Z0-9_-]`, which every one of the three grammars admits in the
/// interior of a value and two of them require at its end.
fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

/// The characters [`DESCRIPTION_PATTERN`]'s one class lists.
fn is_description_character(character: char) -> bool {
    matches!(
        character,
        ' ' | 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '.'
            | ','
            | '!'
            | '?'
            | ';'
            | ':'
            | '@'
            | '#'
            | '$'
            | '%'
            | '&'
            | '-'
            | '_'
            | '/'
            | '\\'
    )
}

/// Whether `value` satisfies a grammar of the form "one character the `first` class
/// admits, then at most `max_interior` characters the `interior` class admits, then one
/// the `last` class admits" — the shape both of the two-ended grammars here take, so the
/// reading of their quantifiers lives in one place.
fn admits_bounded_run(
    value: &str,
    first: fn(char) -> bool,
    interior: fn(char) -> bool,
    last: fn(char) -> bool,
    max_interior: Option<usize>,
) -> bool {
    let characters: Vec<char> = value.chars().collect();
    // Two characters at least: the expression spells a first and a last one.
    let Some((first_character, rest)) = characters.split_first() else {
        return false;
    };
    let Some((last_character, interior_characters)) = rest.split_last() else {
        return false;
    };
    first(*first_character)
        && last(*last_character)
        && interior_characters.iter().copied().all(interior)
        && max_interior.is_none_or(|max| interior_characters.len() <= max)
}

impl ConstrainedText {
    /// The modelled `@pattern` this grammar is, anchors included.
    pub fn pattern(self) -> &'static str {
        match self {
            Self::ResourceName => RESOURCE_NAME_PATTERN,
            Self::Description => DESCRIPTION_PATTERN,
            Self::WidgetTitle => WIDGET_TITLE_PATTERN,
        }
    }

    /// Whether `value` satisfies this grammar — the whole expression, lookahead included.
    pub fn admits(self, value: &str) -> bool {
        // The lookahead `(?!.* {2})`, which is what the boundary cannot apply: two
        // consecutive spaces anywhere in the value, wherever the rest of the expression
        // would otherwise admit them.
        if value.contains("  ") {
            return false;
        }
        match self {
            // `[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]`.
            Self::ResourceName => admits_bounded_run(
                value,
                |character| character.is_ascii_alphabetic(),
                |character| character == ' ' || is_word_character(character),
                is_word_character,
                Some(48),
            ),
            // `[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*` — one class, any number of characters.
            Self::Description => value.chars().all(is_description_character),
            // `[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]`: the same two-ended shape as a
            // resource name, with a digit or `_`/`-` admitted at the start and no bound
            // on the interior of its own (`@length` 2..50 bounds it).
            Self::WidgetTitle => admits_bounded_run(
                value,
                is_word_character,
                |character| character == ' ' || is_word_character(character),
                is_word_character,
                None,
            ),
        }
    }
}

/// One text member of a request, by the path a refusal states it at, the value submitted
/// and the grammar it has to satisfy.
pub type TextMember<'a> = (String, &'a str, ConstrainedText);

/// How this service refuses the text members of `members` that break their modelled
/// `@pattern`, or `None` when every one of them satisfies it.
///
/// The refusal is the boundary's own: one clause per member that broke its pattern,
/// quoting the expression the service publishes — the model's text without the `^`/`$`
/// Smithy wraps it in, exactly as [`crate::service`]'s refusal seam quotes it — under the
/// framework's `N validation error(s) detected` envelope. So a value refused here is
/// refused in the same words as one the boundary could judge itself, and the caller
/// cannot tell which layer applied the pattern.
pub fn text_pattern_refusal<'a>(members: impl IntoIterator<Item = TextMember<'a>>) -> Option<String> {
    let violations: Vec<Violation> = members
        .into_iter()
        .filter(|(_, value, text)| !text.admits(value))
        .map(|(path, value, text)| Violation {
            path,
            kind: ViolationKind::Pattern(::service_common::validation::unanchored(text.pattern())),
            // The value the caller submitted, which a constraint clause echoes. Bounded
            // by the member's own `@length` maximum (50 for a name or a title, 200 for a
            // description), which the boundary already enforced.
            value: Some(value.to_string()),
        })
        .collect();
    envelope(&violations)
}

/// The text members a write of a DASHBOARD submits, for [`text_pattern_refusal`]: the
/// dashboard's own name and description, and the title and description of every widget it
/// carries.
///
/// One list for both writes, so `CreateDashboard` and `UpdateDashboard` apply the same
/// grammars at the same member paths. A widget names its 1-based position the way the
/// framework spells a list element's path.
pub fn dashboard_text_members<'a>(
    name: Option<&'a str>,
    description: Option<&'a str>,
    widgets: &'a [Widget],
) -> Vec<TextMember<'a>> {
    let mut members: Vec<TextMember<'a>> = Vec::new();
    if let Some(name) = name {
        members.push(("name".to_string(), name, ConstrainedText::ResourceName));
    }
    if let Some(description) = description {
        members.push((
            "description".to_string(),
            description,
            ConstrainedText::Description,
        ));
    }
    for (index, widget) in widgets.iter().enumerate() {
        let position = ::aws_smithy_schema_server::constraints::path::element(index + 1);
        members.push((
            ::aws_smithy_schema_server::constraints::path::join(&[
                "widgets".to_string(),
                position.clone(),
                "title".to_string(),
            ]),
            widget.title.as_str(),
            ConstrainedText::WidgetTitle,
        ));
        if let Some(widget_description) = widget.description.as_deref() {
            members.push((
                ::aws_smithy_schema_server::constraints::path::join(&[
                    "widgets".to_string(),
                    position,
                    "description".to_string(),
                ]),
                widget_description,
                ConstrainedText::Description,
            ));
        }
    }
    members
}

/// The text members a write of a SCHEDULED REPORT submits, for [`text_pattern_refusal`]:
/// the report's name and description.
///
/// `prefix` is the member the two are nested under, or `None` when the request carries
/// them at its top level — `CreateScheduledReport` submits them inside `scheduledReport`
/// while `UpdateScheduledReport` carries them directly, and a clause states the path the
/// request used.
pub fn report_text_members<'a>(
    prefix: Option<&str>,
    name: Option<&'a str>,
    description: Option<&'a str>,
) -> Vec<TextMember<'a>> {
    let path = |member: &str| match prefix {
        Some(prefix) => format!("{prefix}.{member}"),
        None => member.to_string(),
    };
    let mut members: Vec<TextMember<'a>> = Vec::new();
    if let Some(name) = name {
        members.push((path("name"), name, ConstrainedText::ResourceName));
    }
    if let Some(description) = description {
        members.push((
            path("description"),
            description,
            ConstrainedText::Description,
        ));
    }
    members
}

/// Encode the ARN a listing page resumes at as an opaque `nextToken`.
pub fn encode_page_token(arn: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(arn))
}

/// The `@length` bounds a resource ARN of this service carries: `DashboardArn` and
/// `ScheduledReportArn` both declare 20..2048
/// (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
const RESOURCE_ARN_LENGTH: ::std::ops::RangeInclusive<usize> = 20..=2048;

/// The literal first field of every ARN.
const ARN_PREFIX_FIELD: &str = "arn";

/// The fixed opening of the partition field, which the ARN patterns spell
/// `aws[-a-z0-9]*`: the literal `aws` followed by a run of [`is_arn_label`]'s characters.
const ARN_PARTITION_PREFIX: &str = "aws";

/// How many digits an account field holds, as the ARN patterns' `[0-9]{12}` spells it.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The resource id that stands for every resource of a type, which the ARN patterns admit
/// beside a concrete id (`(\*|[-a-z0-9]+)`).
const ARN_RESOURCE_WILDCARD: &str = "*";

/// Whether `label` is a (possibly empty) run of the characters `[-a-z0-9]`, the one class
/// this service's ARN patterns admit in a partition's suffix and in a resource id.
fn is_arn_label(label: &str) -> bool {
    label
        .chars()
        .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

/// Whether `arn` is an ARN of this service's `resource_type` resource, judged by that
/// resource's whole modelled ARN pattern — `DashboardArn`'s
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$` and
/// `ScheduledReportArn`'s, which differ only in the resource type
/// (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
///
/// The expression is anchored at both ends, so the check is positional: six
/// colon-separated fields (a resource id of these patterns carries no colon of its own),
/// each judged against the pattern's requirement for THAT field — the literal `arn`, a
/// partition opening with `aws`, this service's ARN namespace, an empty region, twelve
/// digits of account, and the resource type followed by `/` and one resource id. A value
/// that satisfies the segments in some other arrangement is not an ARN of this resource.
pub fn names_resource_arn(arn: &str, resource_type: &str) -> bool {
    if !RESOURCE_ARN_LENGTH.contains(&arn.len()) {
        return false;
    }
    let fields: Vec<&str> = arn.split(':').collect();
    let &[prefix, partition, namespace, region, account, resource] = fields.as_slice() else {
        return false;
    };
    prefix == ARN_PREFIX_FIELD
        && partition
            .strip_prefix(ARN_PARTITION_PREFIX)
            .is_some_and(is_arn_label)
        && namespace == ARN_NAMESPACE
        && region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.chars().all(|digit| digit.is_ascii_digit())
        && resource
            .strip_prefix(resource_type)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|resource_id| {
                resource_id == ARN_RESOURCE_WILDCARD
                    || (!resource_id.is_empty() && is_arn_label(resource_id))
            })
}

/// The ARN a `nextToken` resumes at, or `None` when the token is not one this
/// service minted for a listing over `resource_type`'s resource (the caller
/// answers that with a `ValidationException`).
///
/// A cursor this service minted always names an ARN of the resource the listing
/// walks. It is checked against that resource's whole ARN pattern
/// ([`names_resource_arn`]), so a token that decodes to anything else — a cursor minted
/// by a listing over the other resource type, or a hand-written body that merely carries
/// the pattern's segments somewhere in it — is rejected rather than read as a resume
/// point past every stored record.
pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    names_resource_arn(&arn, resource_type).then_some(arn)
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
/// for the resource `resource_type` names and refused when it is not a cursor this
/// service minted for that resource; `max_results` is the page size, refused when it is not
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
    resource_type: &str,
    mut project: impl FnMut(&'records R) -> T,
) -> Result<ListingPage<T>, ListingRefusal> {
    let resume_from = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token, resource_type).ok_or(ListingRefusal::UninterpretableToken)?,
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

/// The event bus one execution of a scheduled report delivers its notification to.
///
/// "Events generated by Amazon Web Services services go to your account's default event
/// bus" (EventBridge's event-bus reference), and the default bus is named `default` —
/// which is also the bus the model names for this service's notification
/// (`HealthStatus$statusCode`: "the default Amazon Web Services EventBridge EventBus in
/// your account").
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `source` every event this service publishes carries.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`,
/// where the label is the service's own namespace — this service's
/// `aws.api#service$arnNamespace`, i.e. the `ARN_NAMESPACE` this module mints ARNs under.
/// Spelled as one literal because a `source` is a wire value a client writes an
/// `EventPattern` against.
pub const EVENT_SOURCE: &str = "aws.bcm-dashboards";

/// The `detail-type` of the notification one execution of a scheduled report delivers.
///
/// CHOSEN, not cited: the model documents THAT an execution delivers a notification to
/// the account's default event bus, and names neither a detail type nor a detail schema
/// for it. The value follows the form EventBridge's service events use — the service, the
/// resource and what happened to it — so a rule can match this notification and nothing
/// else.
pub const SCHEDULED_REPORT_EXECUTION_DETAIL_TYPE: &str =
    "BCM Dashboards Scheduled Report Execution";

/// The Region a notification is delivered in when the request carried no SigV4 credential
/// scope to read one from.
const DEFAULT_REGION: &str = "us-east-1";

/// The Region this request was signed for, which is the Region whose default event bus an
/// execution's notification goes to: the account's buses are regional, so the delivery
/// belongs to the Region the caller addressed and not to a Region frozen at startup.
///
/// This service's own ARNs carry no Region (the model's ARN patterns have an empty Region
/// segment), so the scope is the only thing that names one.
fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Render `at` as the RFC 3339 instant an event's `detail` carries.
fn rfc3339(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The `detail` of the notification one execution delivers: the report the execution ran,
/// read from the STORED record in the member names the modelled `ScheduledReport` shape
/// spells them, plus the instant the execution ran.
///
/// Nothing here is taken from the request or invented: a reader of this event and a
/// reader of `GetScheduledReport` see one record. `widgetIds` appears only when the
/// record carries the member, exactly as the readers report it.
pub fn scheduled_report_execution_detail(
    report: &ScheduledReportRecord,
    executed_at: SystemTime,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert("arn".to_string(), ::serde_json::json!(report.arn));
    detail.insert("name".to_string(), ::serde_json::json!(report.name));
    detail.insert(
        "dashboardArn".to_string(),
        ::serde_json::json!(report.dashboard_arn),
    );
    if let Some(widget_ids) = &report.widget_ids {
        detail.insert("widgetIds".to_string(), ::serde_json::json!(widget_ids));
    }
    detail.insert(
        "executionTime".to_string(),
        ::serde_json::json!(rfc3339(executed_at)),
    );
    ::serde_json::Value::Object(detail)
}

/// A `HealthStatus` carrying `status_reasons`, refreshed at `last_refreshed_at`.
///
/// The one place the model's two invariants on this shape hold, so no caller can report a
/// status that breaks them: the code is [`HEALTH_STATUS_HEALTHY`] exactly while there is
/// no reason — the model documents `HEALTHY` as "configured properly and has all required
/// permissions to execute" and `UNHEALTHY` as the report being unable to deliver, "the
/// reason for the unhealthy state is captured in the health status reasons" — and
/// `statusReasons` is documented "only present when the status is UNHEALTHY", so a healthy
/// status carries the member absent rather than an empty list
/// (`smithy_query(query="shape",shape="HealthStatus")`).
fn health_status(
    status_reasons: Vec<String>,
    last_refreshed_at: Option<::aws_smithy_types::DateTime>,
) -> HealthStatus {
    let healthy = status_reasons.is_empty();
    HealthStatus {
        status_code: if healthy {
            HEALTH_STATUS_HEALTHY.to_string()
        } else {
            HEALTH_STATUS_UNHEALTHY.to_string()
        },
        last_refreshed_at,
        status_reasons: (!healthy).then_some(status_reasons),
    }
}

/// The health of a report whose execution could NOT hand its notification to the
/// account's default event bus.
///
/// This is the state the model grounds `UNHEALTHY` in — "the scheduled report is unable
/// to deliver the notification to the default Amazon EventBridge EventBus in your
/// account" — with [`STATUS_REASON_INTERNAL_FAILURE`] as the reason, because the report's
/// own configuration resolved and the delivery is what failed. `lastRefreshedAt` is `now`
/// because the execution that attempted the delivery is the refresh.
pub fn undelivered_notification_health(now: SystemTime) -> HealthStatus {
    health_status(
        vec![STATUS_REASON_INTERNAL_FAILURE.to_string()],
        Some(::aws_smithy_types::DateTime::from(now)),
    )
}

/// Deliver the notification one execution of `report` owes to the account's default
/// EventBridge event bus, and answer whether the bus took it.
///
/// This is what an execution of a scheduled report IS: the model defines the unhealthy
/// state as the report being unable to deliver this notification, so a healthy execution
/// delivers it and a client that wires an EventBridge rule to a scheduled report sees it
/// arrive. The envelope names the two resources the notification is about (the report and
/// the dashboard it renders) in `resources`, carries the stored record as its `detail`
/// ([`scheduled_report_execution_detail`]) and is stamped with the instant the execution
/// ran rather than the instant the bus was reached — `PutEventsRequestEntry$Time` is "the
/// time stamp of the event, per RFC3339. If no time stamp is provided, the time stamp of
/// the PutEvents call is used."
///
/// EventBridge is reached through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), the way any emulated service
/// reaches another: the request never leaves this process, so nothing is dialled and this
/// crate names no other service crate. Call it with NO store guard held — the guard is a
/// lock, and a lock is not held across an await.
///
/// `false` means the notification was not delivered, which the caller reports as the
/// report's health: either EventBridge refused the entry (`FailedEntryCount` counts "the
/// number of failed entries") or there was no EventBridge in this emulator to take it. A
/// report whose notification nothing accepted is exactly the report the model calls
/// unhealthy, so the outcome is reported rather than assumed either way.
pub async fn deliver_execution_notification(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    report: &ScheduledReportRecord,
    executed_at: SystemTime,
) -> bool {
    let region = request_region(ctx);
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(region.clone()))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
        .event_bus_name(DEFAULT_EVENT_BUS)
        .source(EVENT_SOURCE)
        .detail_type(SCHEDULED_REPORT_EXECUTION_DETAIL_TYPE)
        .time(::aws_sdk_eventbridge::primitives::DateTime::from(executed_at))
        .resources(&report.arn)
        .resources(&report.dashboard_arn)
        .detail(scheduled_report_execution_detail(report, executed_at).to_string())
        .build();
    let answer = ::aws_sdk_eventbridge::Client::from_conf(config)
        .put_events()
        .entries(entry)
        .send()
        .await;
    match answer {
        Ok(output) => {
            // One entry was published, so one failed entry is this notification's.
            if output.failed_entry_count() == 0 {
                return true;
            }
            ::tracing::warn!(
                scheduled_report = %report.arn,
                entries = ?output.entries(),
                "the default event bus refused the scheduled report's execution notification"
            );
            false
        }
        Err(error) => {
            ::tracing::warn!(
                scheduled_report = %report.arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the scheduled report's execution notification could not be delivered to the \
                 default event bus"
            );
            false
        }
    }
}
