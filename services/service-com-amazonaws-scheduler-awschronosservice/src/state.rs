// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-scheduler-awschronosservice service.
//!
//! EventBridge Scheduler's control plane is two nested resources: a
//! `ScheduleGroup` (named, holds tags) and the `Schedule`s inside it. Both live
//! here, keyed so the ordered maps enumerate deterministically — which is what
//! lets `ListSchedules` resume from an opaque page token.

use std::collections::{BTreeMap, BTreeSet};
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

/// The `ScheduleGroupState` a usable group reports.
///
/// `ScheduleGroupState` is modelled `ACTIVE | DELETING`, and AWS returns a
/// newly created group ACTIVE, so only `DeleteScheduleGroup` ever moves one.
pub const SCHEDULE_GROUP_STATE_ACTIVE: &str = "ACTIVE";

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

/// Largest page `ListScheduleGroups` returns, and the page size it uses when
/// the request names none.
///
/// `ListScheduleGroupsInput$MaxResults` is modelled `range 1..100`, exactly as
/// `ListSchedulesInput$MaxResults` is, so the same bound holds.
pub const MAX_LIST_SCHEDULE_GROUPS_PAGE: usize = MAX_LIST_SCHEDULES_PAGE;

/// The tag-key prefix AWS keeps for itself.
///
/// "You cannot edit or delete tag keys or values with this prefix. Tags with
/// this prefix do not count against your tags per resource limit" — the AWS
/// tagging restrictions every tagging API shares, so a request that supplies
/// such a key is refused.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Largest number of tags one schedule group holds.
///
/// `TagList` (what `TagResource$Tags` sends and `ListTagsForResourceOutput$Tags`
/// answers) is modelled `length 0..200`, so a group that held more could not be
/// reported: a merge past the bound is refused instead.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The ARN infix that separates a schedule group's ARN prefix from its name.
const SCHEDULE_GROUP_ARN_INFIX: &str = ":schedule-group/";

/// The name of the schedule group `arn` identifies.
///
/// `TagResourceArn` is modelled `^arn:aws(-[a-z]+)?:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+$`,
/// so the name is everything after the resource type — the boundary validator has
/// already refused an ARN of any other shape (a schedule's, say) by the time a
/// handler runs.
pub fn schedule_group_name_from_arn(arn: &str) -> Option<&str> {
    arn.split_once(SCHEDULE_GROUP_ARN_INFIX)
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

/// The reserved tag key among `keys`, if any — the lowest one, so the refusal
/// names the same key however the request ordered its tags. The comparison is
/// case-insensitive on the prefix: `AWS:` is reserved exactly as `aws:` is.
///
/// See [`RESERVED_TAG_KEY_PREFIX`].
pub fn reserved_tag_key<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> Option<&'a str> {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    keys.into_iter()
        .filter(|key| {
            let key = key.as_bytes();
            key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix)
        })
        .min()
}

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

impl ScheduleGroupRecord {
    /// Merge `tags` into the group's tag set and answer whether anything changed.
    ///
    /// A key the group does not carry is appended; a key it does carry keeps its
    /// position and takes the new value — tagging a resource with a key already
    /// on it replaces that key's value rather than adding a second pair, which is
    /// what makes a tag set a map keyed by tag key.
    pub fn merge_tags(&mut self, tags: impl IntoIterator<Item = Tag>) -> bool {
        let mut changed = false;
        for tag in tags {
            match self.tags.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => {
                    if held.value != tag.value {
                        held.value = tag.value;
                        changed = true;
                    }
                }
                None => {
                    self.tags.push(tag);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Drop every tag whose key is in `keys` and answer whether anything changed.
    ///
    /// A key the group does not carry is not an error: `UntagResource` is modelled
    /// `@idempotent`, so removing a key twice has the same effect as removing it
    /// once.
    pub fn remove_tags<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) -> bool {
        let keys: BTreeSet<&str> = keys.into_iter().collect();
        let before = self.tags.len();
        self.tags.retain(|tag| !keys.contains(tag.key.as_str()));
        self.tags.len() != before
    }
}

/// The distinct keys a resource holding `held` would carry once `incoming` were
/// merged in — what [`MAX_TAGS_PER_RESOURCE`] bounds.
pub fn tag_keys_after<'a>(held: &'a [Tag], incoming: &'a [Tag]) -> BTreeSet<&'a str> {
    held.iter()
        .chain(incoming.iter())
        .map(|tag| tag.key.as_str())
        .collect()
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
#[derive(Debug)]
pub struct ComAmazonawsSchedulerAwschronosserviceState {
    /// Groups by name. The implicit `default` group is not seeded: it exists for
    /// every account, so membership is answered by [`Self::group_exists`] rather
    /// than by a row, and a row for it appears only once a request has to STORE
    /// something on it (its tags). An operation that enumerates groups therefore
    /// reports `default` whether or not this map holds a row for it.
    pub groups: BTreeMap<String, ScheduleGroupRecord>,
    /// Schedules by `(group name, schedule name)` — the pair that identifies a
    /// schedule, and an ordering `ListSchedules` pages through.
    pub schedules: BTreeMap<(String, String), ScheduleRecord>,
    /// When this account's scheduler state came into being, which is the
    /// `CreationDate` the implicit `default` group reports.
    ///
    /// The default group is created with the account, so no request mints it and
    /// nothing can read its creation date off a client call. Fixing it here, when
    /// the store is first reached, keeps it STABLE across reads — a read of the
    /// default group never writes.
    pub origin: SystemTime,
}

impl Default for ComAmazonawsSchedulerAwschronosserviceState {
    fn default() -> Self {
        Self {
            groups: BTreeMap::new(),
            schedules: BTreeMap::new(),
            origin: SystemTime::now(),
        }
    }
}

impl ComAmazonawsSchedulerAwschronosserviceState {
    /// Whether `name` names a group a schedule may be created in.
    pub fn group_exists(&self, name: &str) -> bool {
        name == DEFAULT_SCHEDULE_GROUP || self.groups.contains_key(name)
    }

    /// The group `name` addresses, or `None` when no such group exists.
    ///
    /// Every account has the `default` group without having created it, so it is
    /// answered from [`Self::origin`] rather than from a stored row — the same
    /// membership rule [`Self::group_exists`] applies, as one record a reader can
    /// serialize. `region` is the request's region, which the ARN of a group no
    /// create call minted carries.
    pub fn group_record(&self, region: &str, name: &str) -> Option<ScheduleGroupRecord> {
        if let Some(record) = self.groups.get(name) {
            return Some(record.clone());
        }
        if name == DEFAULT_SCHEDULE_GROUP {
            return Some(ScheduleGroupRecord {
                name: DEFAULT_SCHEDULE_GROUP.to_string(),
                arn: schedule_group_arn(region, DEFAULT_SCHEDULE_GROUP),
                state: SCHEDULE_GROUP_STATE_ACTIVE.to_string(),
                creation_date: self.origin,
                last_modification_date: self.origin,
                tags: Vec::new(),
            });
        }
        None
    }

    /// Every schedule group this account holds, in `ScheduleGroupName` order:
    /// the stored rows plus the implicit `default` group in its sorted place,
    /// as the one record [`Self::group_record`] would hand a reader.
    ///
    /// One total order over the whole set is what lets a page token be a
    /// POSITION in it rather than an index into a list that may have changed
    /// between calls.
    pub fn groups_in_name_order(&self, region: &str) -> Vec<ScheduleGroupRecord> {
        let mut all: Vec<ScheduleGroupRecord> = self.groups.values().cloned().collect();
        // The built-in group is listed from its stored row once one exists — a
        // write such as tagging it materialises one — and synthesised otherwise,
        // so it appears exactly once either way.
        if !self.groups.contains_key(DEFAULT_SCHEDULE_GROUP) {
            all.extend(self.group_record(region, DEFAULT_SCHEDULE_GROUP));
        }
        all.sort_by(|left, right| left.name.cmp(&right.name));
        all
    }

    /// The name of the schedule group `arn` identifies in this account and
    /// Region, or `None` when no group there answers to that ARN.
    ///
    /// An ARN is a resource's identity, so it matches only the group whose own
    /// ARN it equals: a well-formed ARN naming another partition, Region or
    /// account addresses no group reachable from this endpoint. Resolution goes
    /// through [`Self::group_record`], so the account's built-in `default` group
    /// answers to its ARN with no row stored for it.
    pub fn group_name_for_arn(&self, arn: &str, region: &str) -> Option<String> {
        let name = schedule_group_name_from_arn(arn)?;
        let record = self.group_record(region, name)?;
        (record.arn == arn).then(|| name.to_string())
    }

    /// The tags held by `name`, which is empty for a group that carries none.
    pub fn group_tags(&self, name: &str) -> &[Tag] {
        self.groups
            .get(name)
            .map_or(&[][..], |record| record.tags.as_slice())
    }

    /// The record tags are stored on for the group `name`, materialising the row
    /// the account's built-in `default` group needs on its first write.
    ///
    /// Only a caller that already resolved `name` through [`Self::group_name_for_arn`]
    /// reaches this, so the group exists — and every group a create call minted
    /// already has a row, so the one row this can add is the `default` group's.
    /// Its dates come from [`Self::origin`], the same reading
    /// [`Self::group_record`] reports for it, so storing its tags never moves the
    /// creation date a reader sees.
    pub fn group_for_write(&mut self, name: &str, region: &str) -> &mut ScheduleGroupRecord {
        let origin = self.origin;
        self.groups
            .entry(name.to_string())
            .or_insert_with(|| ScheduleGroupRecord {
                name: name.to_string(),
                arn: schedule_group_arn(region, name),
                // The built-in group AWS creates with the account is ACTIVE; this
                // row only starts holding its tags.
                state: SCHEDULE_GROUP_STATE_ACTIVE.to_string(),
                creation_date: origin,
                last_modification_date: origin,
                tags: Vec::new(),
            })
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

/// Encode a `ListScheduleGroups` page token for the last group name returned.
///
/// Same encoding as [`encode_page_token`] — hex, so it survives the `NextToken`
/// query parameter unescaped — under its own payload key, so a token minted for
/// one listing never decodes as a position in the other.
pub fn encode_group_page_token(name: &str) -> String {
    let payload = ::serde_json::json!({ "sg": name });
    ::hex::encode(payload.to_string())
}

/// Decode a `ListScheduleGroups` page token. `None` when it is not one this
/// service minted, which the caller turns into a `ValidationException`.
pub fn decode_group_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    Some(value.get("sg")?.as_str()?.to_string())
}
