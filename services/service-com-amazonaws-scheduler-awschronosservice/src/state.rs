// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-scheduler-awschronosservice service.
//!
//! EventBridge Scheduler's control plane is two nested resources: a
//! `ScheduleGroup` (named, holds tags) and the `Schedule`s inside it. Both live
//! here, keyed so the ordered maps enumerate deterministically — which is what
//! lets `ListSchedules` resume from an opaque page token.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{FlexibleTimeWindow, Tag, Target};

/// The schedule group every schedule lands in when a request names none.
///
/// AWS creates this group with the account: it is never created by a client, so
/// it is always present and `CreateScheduleGroup` on it conflicts.
pub const DEFAULT_SCHEDULE_GROUP: &str = "default";

/// The region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one.
pub const FALLBACK_REGION: &str = "us-east-1";

/// `FlexibleTimeWindowMode` value that asks for a window.
const FLEXIBLE_TIME_WINDOW_MODE_FLEXIBLE: &str = "FLEXIBLE";

/// The `ScheduleState` a schedule takes when the request omits `State`.
pub const DEFAULT_SCHEDULE_STATE: &str = "ENABLED";

/// Largest number of schedule groups one account/Region holds.
///
/// The EventBridge Scheduler service quota "Schedule groups per Region"
/// (default 500). It is also what bounds the store: a `CreateScheduleGroup`
/// past it answers the modelled `ServiceQuotaExceededException` instead of
/// growing memory without limit.
pub const MAX_SCHEDULE_GROUPS: usize = 500;

/// Largest number of schedules one account/Region holds.
///
/// The EventBridge Scheduler service quota "Schedules per Region" (default
/// 1,000,000), and likewise the store's bound: a `CreateSchedule` past it
/// answers the modelled `ServiceQuotaExceededException`.
pub const MAX_SCHEDULES: usize = 1_000_000;

/// Largest page `ListSchedules` returns, and the page size it uses when the
/// request names none.
///
/// `ListSchedulesInput$MaxResults` is modelled `range 1..100`, so no client can
/// ask for a bigger page; applying the same maximum to a request that omits
/// `MaxResults` means one call can never serialize more than this many records.
pub const MAX_LIST_SCHEDULES_PAGE: usize = 100;

/// A `ScheduleGroup` as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ScheduleGroupRecord {
    pub name: String,
    pub arn: String,
    /// `ScheduleGroupState`: `ACTIVE` | `DELETING`.
    pub state: String,
    pub creation_date: SystemTime,
    pub last_modification_date: SystemTime,
    /// Carried for `TagResource`/`UntagResource`/`ListTagsForResource`.
    pub tags: Vec<Tag>,
}

/// A `Schedule` as this emulator persists it.
///
/// `start_date`/`end_date` keep the wire `DateTime` the client sent, so a
/// `GetSchedule` hands back exactly what `CreateSchedule` was given;
/// `creation_date`/`last_modification_date` are server clock readings, so they
/// are `SystemTime` and can be compared.
#[derive(Debug, Clone)]
pub struct ScheduleRecord {
    pub name: String,
    pub group_name: String,
    pub arn: String,
    pub schedule_expression: String,
    pub schedule_expression_timezone: Option<String>,
    pub start_date: Option<::aws_smithy_types::DateTime>,
    pub end_date: Option<::aws_smithy_types::DateTime>,
    pub description: Option<String>,
    /// `ScheduleState`: `ENABLED` | `DISABLED`.
    pub state: String,
    pub kms_key_arn: Option<String>,
    /// `ActionAfterCompletion`: `DELETE` | `NONE`.
    pub action_after_completion: Option<String>,
    pub target: Target,
    pub flexible_time_window: FlexibleTimeWindow,
    pub creation_date: SystemTime,
    pub last_modification_date: SystemTime,
}

/// Per-service state for com-amazonaws-scheduler-awschronosservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSchedulerAwschronosserviceState {
    /// Client-created groups, by name. The implicit `default` group is NOT
    /// stored: it exists for every account, so membership is answered by
    /// [`Self::group_exists`] rather than by seeding a row on first read.
    pub groups: BTreeMap<String, ScheduleGroupRecord>,
    /// Schedules by `(group name, schedule name)` — the pair that identifies a
    /// schedule, and an ordering `ListSchedules` pages through.
    pub schedules: BTreeMap<(String, String), ScheduleRecord>,
}

impl ComAmazonawsSchedulerAwschronosserviceState {
    /// Whether `name` names a group a schedule may be created in.
    pub fn group_exists(&self, name: &str) -> bool {
        name == DEFAULT_SCHEDULE_GROUP || self.groups.contains_key(name)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSchedulerAwschronosserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSchedulerAwschronosserviceState>)
}

/// The region this request was signed for, which is the region its ARNs carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| FALLBACK_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:scheduler:<region>:<account>:schedule-group/<name>`.
pub fn schedule_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("scheduler", region, &format!("schedule-group/{name}"))
}

/// `arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>`.
pub fn schedule_arn(region: &str, group_name: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "scheduler",
        region,
        &format!("schedule/{group_name}/{name}"),
    )
}

/// The group a request addresses: the one it names, else the implicit `default`.
pub fn effective_group_name(group_name: Option<&str>) -> String {
    group_name
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_SCHEDULE_GROUP)
        .to_string()
}

/// A server clock reading as the wire `DateTime` the timestamp members carry.
pub fn to_wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Refuse a `FlexibleTimeWindow` whose `MaximumWindowInMinutes` does not match
/// its `Mode`.
///
/// EventBridge Scheduler requires the maximum window exactly when the mode is
/// `FLEXIBLE` ("If the flexible time window mode is `FLEXIBLE`, you must specify
/// a `MaximumWindowInMinutes`" — the `FlexibleTimeWindow` reference). The bound
/// on the value itself (1..1440) is modelled, so the boundary validator has
/// already refused anything outside it by the time a handler runs.
pub fn flexible_time_window_violation(window: &FlexibleTimeWindow) -> Option<String> {
    let flexible = window.mode == FLEXIBLE_TIME_WINDOW_MODE_FLEXIBLE;
    match (flexible, window.maximum_window_in_minutes) {
        (true, None) => Some(
            "FlexibleTimeWindow.MaximumWindowInMinutes is required when \
             FlexibleTimeWindow.Mode is FLEXIBLE"
                .to_string(),
        ),
        (false, Some(_)) => Some(format!(
            "FlexibleTimeWindow.MaximumWindowInMinutes must not be set when \
             FlexibleTimeWindow.Mode is {}",
            window.mode
        )),
        _ => None,
    }
}

/// Encode a `ListSchedules` page token for the last `(group, name)` returned.
///
/// The token is opaque to a client but decodable here, so the next page resumes
/// immediately after that key in the ordered map — no index into a list that may
/// have changed between calls. Hex, not base64, so the token is URL-safe: it
/// travels in the `NextToken` QUERY parameter, where `+` and `/` would have to be
/// escaped by every client that echoes it back.
pub fn encode_page_token(group_name: &str, name: &str) -> String {
    let payload = ::serde_json::json!({ "g": group_name, "n": name });
    ::hex::encode(payload.to_string())
}

/// Decode a `ListSchedules` page token. `None` when it is not one this service
/// minted, which the caller turns into a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<(String, String)> {
    let bytes = ::hex::decode(token).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    let group = value.get("g")?.as_str()?.to_string();
    let name = value.get("n")?.as_str()?.to_string();
    Some((group, name))
}
