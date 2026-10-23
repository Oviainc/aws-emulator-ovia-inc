// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdashboards-awsbcmdashboardsservice service.
//!
//! The dashboard records every dashboard operation reads and writes live here, so
//! `CreateDashboard`, `GetDashboard`, `ListDashboards`, `UpdateDashboard` and
//! `DeleteDashboard` all see one copy of each resource. The helpers below (ARN and
//! widget-id minting, the `DashboardReference` projection, the pagination cursor
//! codec) are shared by those handlers rather than duplicated per operation.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    DashboardReference, DateTimeRange, HealthStatus, ResourceTag, ScheduleConfig, ScheduledReport,
    ScheduledReportSummary, Widget,
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
    /// The tags supplied at create time, read back by `ListTagsForResource` (and
    /// state for the other tagging operations once they are implemented).
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
    pub health_status: Option<HealthStatus>,
    /// The tags supplied at create time, read back by `ListTagsForResource` (and
    /// state for the other tagging operations once they are implemented).
    pub resource_tags: Vec<ResourceTag>,
}

impl ScheduledReportRecord {
    /// This record projected onto the modelled `ScheduledReport` shape that
    /// `GetScheduledReport` (and the other readers) return: the stored record's own
    /// fields, never request input.
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
            health_status: self.health_status.clone(),
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
    /// * `healthStatus` is optional on `ScheduledReport` and absent until something
    ///   refreshes it, so a report whose health nothing has refreshed summarises as
    ///   [`HEALTH_STATUS_HEALTHY`] with no `lastRefreshedAt` (no refresh has
    ///   happened) and no `statusReasons` (the model documents those as present
    ///   only when the status is `UNHEALTHY`).
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
            state: self
                .schedule_config
                .state
                .clone()
                .unwrap_or_else(|| SCHEDULE_STATE_ENABLED.to_string()),
            health_status: self.health_status.clone().unwrap_or_else(|| HealthStatus {
                status_code: HEALTH_STATUS_HEALTHY.to_string(),
                last_refreshed_at: None,
                status_reasons: None,
            }),
            schedule_expression_time_zone: self
                .schedule_config
                .schedule_expression_time_zone
                .clone(),
            widget_ids: self.widget_ids.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-bcmdashboards-awsbcmdashboardsservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Every stored dashboard, keyed by its ARN. A `BTreeMap` so `ListDashboards`
    /// walks the dashboards in one stable order and a cursor taken on one page
    /// resumes at the right place on the next.
    pub dashboards: BTreeMap<String, Dashboard>,
    /// Monotonic counter behind every minted dashboard resource id, so two
    /// `CreateDashboard` calls with the same `name` get different ARNs.
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
    /// mapped to the ARN it minted. A token is an at-most-once create key: the first
    /// caller presenting it wins and every later one is refused with the modelled
    /// `ConflictException`, so a retried token can never mint a second report.
    pub scheduled_report_client_tokens: BTreeMap<String, String>,
}

impl ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Mint a fresh, unique dashboard ARN.
    ///
    /// The resource id is 32 lowercase hex characters, which satisfies the
    /// `DashboardArn` pattern's `[-a-z0-9]+` resource segment; the account segment
    /// comes from the emulator's canonical account id via `engine::emulator_arn`.
    /// `bcm-dashboards` ARNs carry no region (the model's pattern has an empty
    /// region segment), so the region passed is empty.
    pub fn mint_dashboard_arn(&mut self) -> String {
        self.next_dashboard_seq = self.next_dashboard_seq.wrapping_add(1);
        let resource_id = format!("{:032x}", self.next_dashboard_seq);
        ::engine::emulator_arn("bcm-dashboards", "", &format!("dashboard/{resource_id}"))
    }

    /// The tags persisted on the resource `resource_arn` names, in the order they
    /// were stored, or `None` when this service holds no resource under that ARN.
    ///
    /// The ARN is the only key a tagging operation resolves a resource by, so every
    /// one of them resolves it here rather than reaching into the resource maps
    /// itself. `ResourceArn`'s pattern admits both resource types this service
    /// stores — `dashboard/...` and `scheduled-report/...` — so both are looked up
    /// here; an ARN neither map holds resolves to `None` and the caller answers its
    /// modelled `ResourceNotFoundException`.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&[ResourceTag]> {
        if let Some(dashboard) = self.dashboards.get(resource_arn) {
            return Some(dashboard.resource_tags.as_slice());
        }
        self.scheduled_reports
            .get(resource_arn)
            .map(|report| report.resource_tags.as_slice())
    }

    /// The resource-based policy document attached to the dashboard `resource_arn`
    /// names, or `None` when this service holds no dashboard under that ARN (the
    /// caller answers that with its modelled `ResourceNotFoundException`).
    ///
    /// A dashboard with no policy attached resolves to the empty document, which is
    /// what the modelled `policyDocument` member's 0-character minimum admits: a
    /// missing policy is not a missing resource.
    pub fn dashboard_resource_policy(&self, resource_arn: &str) -> Option<&str> {
        self.dashboards
            .get(resource_arn)
            .map(|dashboard| dashboard.resource_policy.as_deref().unwrap_or_default())
    }

    /// Mint a fresh widget id: 32 lowercase hex characters, as `WidgetId`'s
    /// `^[0-9a-f]{32}$` pattern requires.
    pub fn mint_widget_id(&mut self) -> String {
        self.next_widget_seq = self.next_widget_seq.wrapping_add(1);
        format!("{:032x}", self.next_widget_seq)
    }

    /// Mint a fresh, unique scheduled-report ARN, the same way dashboard ARNs are
    /// minted: 32 lowercase hex characters for the resource id (which satisfies
    /// `ScheduledReportArn`'s `[-a-z0-9]+` resource segment), the emulator's
    /// canonical account id, and the empty region the model's pattern requires.
    pub fn mint_scheduled_report_arn(&mut self) -> String {
        self.next_scheduled_report_seq = self.next_scheduled_report_seq.wrapping_add(1);
        let resource_id = format!("{:032x}", self.next_scheduled_report_seq);
        ::engine::emulator_arn(
            "bcm-dashboards",
            "",
            &format!("scheduled-report/{resource_id}"),
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
