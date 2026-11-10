// SPDX-License-Identifier: CC0-1.0

//! The alarm mute rules this store holds, and the mute WINDOWS each rule's schedule names.
//!
//! One concern in one file: the stored rule ([`StoredAlarmMuteRule`]), the identity it is keyed
//! by ([`AlarmMuteRuleKey`]), the schedule a rule's `Rule` carries read as the windows it opens
//! ([`MuteWindows`]), and the `Status` a read reports from the clock
//! ([`alarm_mute_rule_status`]).
//!
//! A schedule is read through the maintained parsers rather than a second grammar written here:
//! the cron expression through `cron`, the local instant of an `at` expression and the time zone
//! it is read in through `chrono` / `chrono-tz`, and the window's length through the one ISO 8601
//! duration reading this project shares (`service_common::iso8601_span`).

use ::chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use ::chrono_tz::Tz;
use ::serde_json::{Map, Value};

use crate::ops::types::{AlarmHistoryItem, GetAlarmMuteRuleOutput, Schedule, Tag};

/// The identity of one alarm mute rule: the Region it lives in and its name.
///
/// A mute rule's name "must be unique within your Amazon Web Services account and region"
/// (`PutAlarmMuteRuleInput$Name`) and its ARN carries the Region
/// (`arn:aws:cloudwatch:[REGION]:123456789012:alarm-mute-rule:*`, `PutAlarmMuteRule`), so one
/// name in two Regions is two independent rules and the Region is part of the key this store
/// holds a rule under rather than merely part of what it reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AlarmMuteRuleKey {
    /// The Region the rule lives in, as the request that wrote it was signed for.
    pub region: String,
    /// The rule's name.
    pub name: String,
}

/// The key one Region's rule of this name is held under.
pub fn alarm_mute_rule_key(region: &str, name: &str) -> AlarmMuteRuleKey {
    AlarmMuteRuleKey {
        region: region.to_string(),
        name: name.to_string(),
    }
}

/// An alarm mute rule's ARN:
/// `arn:aws:cloudwatch:<region>:<account>:alarm-mute-rule:<Name>`, the form
/// `PutAlarmMuteRule` documents the resource by
/// (`arn:aws:cloudwatch:[REGION]:123456789012:alarm-mute-rule:*`).
pub fn alarm_mute_rule_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("alarm-mute-rule:{name}"))
}

/// The largest number of alarm mute rules this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the records live in this process's memory, so their number
/// has to be finite. `PutAlarmMuteRule` answers its modelled limit error rather than growing
/// without limit once the bound is reached; updating a rule that already exists adds none, so it
/// is unaffected.
pub const MAX_ALARM_MUTE_RULES: usize = 10_000;

/// A mute rule that is configured and will become active in the future:
/// "SCHEDULED: The mute rule is configured and will become active in the future"
/// (`GetAlarmMuteRule`).
pub const ALARM_MUTE_RULE_SCHEDULED: &str = "SCHEDULED";

/// A mute rule that is muting alarm actions now: "ACTIVE: The mute rule is currently muting
/// alarm actions" (`GetAlarmMuteRule`).
pub const ALARM_MUTE_RULE_ACTIVE: &str = "ACTIVE";

/// A mute rule that will not become active again: "EXPIRED: The mute rule has passed its
/// expiration date and will no longer become active" (`GetAlarmMuteRule`).
pub const ALARM_MUTE_RULE_EXPIRED: &str = "EXPIRED";

/// A mute rule whose schedule opens ONE window: "Indicates whether the mute rule is one-time or
/// recurring. Valid values are ONE_TIME or RECURRING." (`GetAlarmMuteRuleOutput$MuteType`), and
/// an `at` expression is the form documented "For one-time mute windows"
/// (`Schedule$Expression`).
pub const MUTE_TYPE_ONE_TIME: &str = "ONE_TIME";

/// A mute rule whose schedule opens a window per occurrence — the `cron` expression form,
/// documented "For recurring mute windows" (`Schedule$Expression`).
pub const MUTE_TYPE_RECURRING: &str = "RECURRING";

/// A stored alarm mute rule: the modelled shape its reader returns, plus the tags the write
/// accepted (no member of that shape carries them).
///
/// Held as [`GetAlarmMuteRuleOutput`] itself rather than as a parallel record, so every member
/// `PutAlarmMuteRule` accepts round-trips through `GetAlarmMuteRule` without a field-by-field
/// copy that can silently drop one. Its `Status` is the one member NOT stored: the status is
/// what the CLOCK makes of the rule's schedule, so it is computed on every read
/// ([`StoredAlarmMuteRule::reported`]) rather than frozen at the instant of the write.
#[derive(Debug, Clone, Default)]
pub struct StoredAlarmMuteRule {
    /// The rule as its reader reports it, with `Status` left unset.
    pub configuration: GetAlarmMuteRuleOutput,
    /// The tags the rule carries.
    pub tags: Vec<Tag>,
}

impl StoredAlarmMuteRule {
    /// The rule as `GetAlarmMuteRule` reports it at `now`: every member as the write left it,
    /// with the `Status` the clock decides.
    pub fn reported(&self, now: f64) -> GetAlarmMuteRuleOutput {
        let mut reported = self.configuration.clone();
        reported.status = Some(alarm_mute_rule_status(&self.configuration, now).to_string());
        reported
    }

    /// Whether this rule applies to the alarm `alarm_name`: "The list of alarm names that this
    /// mute rule targets." (`MuteTargets$AlarmNames`).
    ///
    /// A rule carrying no `MuteTargets` names no alarm, so it targets none: the member is what
    /// "specifies which alarms an alarm mute rule applies to" (`MuteTargets`).
    pub fn targets(&self, alarm_name: &str) -> bool {
        self.configuration
            .mute_targets
            .as_ref()
            .is_some_and(|targets| {
                targets
                    .alarm_names
                    .iter()
                    .any(|targeted| targeted == alarm_name)
            })
    }
}

/// Which of the three statuses the rule `configuration` describes is in at `now`.
///
/// The statuses are documented by what they say about the rule rather than by a transition:
/// ACTIVE is "currently muting alarm actions", SCHEDULED is "configured and will become active in
/// the future", and EXPIRED is "has passed its expiration date and will no longer become active"
/// (`GetAlarmMuteRule`). So the question each is answered from is whether a mute window of the
/// rule's schedule holds `now`, whether one is still to come, or whether none ever will be:
///
/// * a rule past its `ExpireDate` "is no longer evaluated" (`GetAlarmMuteRuleOutput$ExpireDate`),
///   so no window of it can open again: EXPIRED;
/// * otherwise, a window opens at each instant the schedule names at or after the rule's
///   `StartDate` ("the date and time after which the mute rule takes effect",
///   `PutAlarmMuteRuleInput$StartDate`) and stays open for the schedule's `Duration` ("the length
///   of time that alarms remain muted when the schedule activates", `Schedule$Duration`). The
///   FIRST window to open after `now - Duration` is the only one that can hold `now`, since every
///   earlier one has closed: it holds `now` when it opens at or before it (ACTIVE) and is still to
///   come when it opens after it (SCHEDULED);
/// * a schedule with no window left to open at all — a one-time window already closed — can never
///   make the rule active again, which is the same statement EXPIRED makes: EXPIRED.
///
/// A configuration whose schedule this cannot read names no window, and so no window holding
/// `now`: it reports SCHEDULED, the status of a rule that is not muting anything. A write is
/// refused unless its schedule reads ([`alarm_mute_rule_schedule_error`]), so this is the status
/// of nothing a caller can store.
pub fn alarm_mute_rule_status(configuration: &GetAlarmMuteRuleOutput, now: f64) -> &'static str {
    if configuration
        .expire_date
        .as_ref()
        .map(super::from_timestamp)
        .is_some_and(|expires_at| now >= expires_at)
    {
        return ALARM_MUTE_RULE_EXPIRED;
    }
    let Some(windows) = configuration
        .rule
        .as_ref()
        .and_then(|rule| MuteWindows::of(&rule.schedule))
    else {
        return ALARM_MUTE_RULE_SCHEDULED;
    };
    // The earliest instant a window that could still hold `now` opens at: one that opened a whole
    // duration ago has closed, and one that opened before the rule took effect never opened.
    let takes_effect_at = configuration
        .start_date
        .as_ref()
        .map_or(f64::NEG_INFINITY, super::from_timestamp);
    let floor = (now - windows.duration_secs).max(takes_effect_at);
    match windows.opens_after(floor) {
        Some(opens_at) if opens_at <= now => ALARM_MUTE_RULE_ACTIVE,
        Some(_) => ALARM_MUTE_RULE_SCHEDULED,
        None => ALARM_MUTE_RULE_EXPIRED,
    }
}

/// The mute question this store answers about one alarm: whether a rule it holds is muting that
/// alarm's actions right now.
impl super::ComAmazonawsCloudwatchGraniteserviceversion20100801State {
    /// The name of the ACTIVE mute rule targeting `alarm_name` at `now`, or `None` when the
    /// alarm's actions are muted by nothing.
    ///
    /// "When a mute rule is active, the targeted alarms continue to evaluate metrics and
    /// transition between states, but their configured actions are muted." (`MuteTargets`) — so
    /// the question a transition asks is whether a rule that NAMES this alarm among its targets
    /// (`MuteTargets$AlarmNames`, "The list of alarm names that this mute rule targets") is ACTIVE
    /// at the instant of the transition, which is the one status reading every mute-rule operation
    /// shares ([`alarm_mute_rule_status`]).
    ///
    /// A rule is a resource of ONE Region ([`AlarmMuteRuleKey`]), and so is an alarm — its ARN
    /// carries the Region it was created in — so only the rules of the alarm's own Region can mute
    /// it. An alarm whose ARN names no Region is matched against every rule, because nothing then
    /// says which Region's rules are not its own.
    ///
    /// Several rules may target one alarm; the one reported is the first in name order, so what the
    /// history records about a muted transition does not depend on the store's iteration order.
    pub fn muting_alarm_mute_rule(
        &self,
        alarm_name: &str,
        alarm_arn: Option<&str>,
        now: f64,
    ) -> Option<String> {
        let region = alarm_arn
            .and_then(::service_common::arn::parse)
            .map(|arn| arn.region.to_string());
        self.alarm_mute_rules
            .iter()
            .filter(|(key, _)| region.as_ref().is_none_or(|region| &key.region == region))
            .filter(|(_, stored)| stored.targets(alarm_name))
            .filter(|(_, stored)| {
                alarm_mute_rule_status(&stored.configuration, now) == ALARM_MUTE_RULE_ACTIVE
            })
            .map(|(key, _)| key.name.clone())
            .min()
    }
}

/// The `HistoryItemType` a muted transition is recorded under: `Action` is the type every record
/// of what became of a transition's actions carries (`smithy_query(query="shape",
/// shape="HistoryItemType")`), and a mute is what became of these.
const MUTED_HISTORY_ITEM_TYPE: &str = "Action";

/// What the record of a muted transition states happened to the actions, beside the `Succeeded`
/// and `Failed` an executed action is recorded with.
const MUTED_ACTION_STATE: &str = "Muted";

/// The history item recording that a transition's configured actions were MUTED by a mute rule,
/// rather than executed.
///
/// "When a mute rule is active, the targeted alarms continue to evaluate metrics and transition
/// between states, but their configured actions […] are muted." (`PutAlarmMuteRule`) — the
/// transition happens and is recorded as itself, and this is the account of the actions it did not
/// execute: which rule muted them, which state they were configured for, and which ARNs they were.
/// Without it a muted transition is indistinguishable, in the only record an operator has, from an
/// action that was executed and failed.
pub fn muted_actions_history_item(
    alarm_name: &str,
    alarm_type: &str,
    rule_name: &str,
    new_state_value: &str,
    actions: &[String],
    now_secs: f64,
) -> AlarmHistoryItem {
    let mut document = Map::new();
    document.insert(
        "version".to_string(),
        Value::from(super::HISTORY_DATA_VERSION),
    );
    document.insert(
        "actionState".to_string(),
        Value::from(MUTED_ACTION_STATE),
    );
    document.insert("muteRuleName".to_string(), Value::from(rule_name));
    document.insert("stateValue".to_string(), Value::from(new_state_value));
    let muted = Value::from(
        actions
            .iter()
            .map(|action| Value::from(action.as_str()))
            .collect::<Vec<Value>>(),
    );
    let mut with_actions = document.clone();
    with_actions.insert("mutedActions".to_string(), muted);
    AlarmHistoryItem {
        alarm_name: Some(alarm_name.to_string()),
        alarm_contributor_id: None,
        // The item belongs to the kind of alarm whose actions were muted, so a reader that asks
        // for one kind of alarm's history sees it.
        alarm_type: Some(alarm_type.to_string()),
        timestamp: Some(super::to_timestamp(now_secs)),
        history_item_type: Some(MUTED_HISTORY_ITEM_TYPE.to_string()),
        // A rule's `Name` is `@length 1..255`, so a summary naming one need not fit the summary's
        // own bound: it is cut at it by the one reading of that bound this store shares.
        history_summary: Some(super::alarm_actions::bounded(format!(
            "Alarm actions muted by alarm mute rule {rule_name}"
        ))),
        // An action ARN is `@length 1..1024` and a state may carry five of them, so the list of
        // muted ARNs is the part of this document that need not fit the modelled `HistoryData`
        // length: it is dropped rather than truncated, because a document is parsed rather than
        // read, and what identifies the mute — the rule's own name — is in both renderings.
        history_data: Some(super::bounded_history_data(&[
            Value::Object(with_actions),
            Value::Object(document),
        ])),
        alarm_contributor_attributes: None,
    }
}

/// The mute windows one rule's `Schedule` names: when each opens, and how long it stays open.
pub struct MuteWindows {
    /// The instants the windows open at, as the schedule's expression states them.
    form: ScheduleForm,
    /// The zone the expression is read in: "The time zone to use when evaluating the schedule
    /// expression […] If you don't specify a time zone, UTC is used by default."
    /// (`Schedule$Timezone`).
    timezone: Tz,
    /// How long each window stays open, from the instant it opens (`Schedule$Duration`).
    duration_secs: f64,
}

/// The two expression forms a mute schedule states its instants in (`Schedule$Expression`).
enum ScheduleForm {
    /// `at(yyyy-MM-ddThh:mm)` — one window, opening at the local instant the expression names.
    Once(NaiveDateTime),
    /// `cron(Minutes Hours Day-of-month Month Day-of-week)` — one window per occurrence. Boxed
    /// because the parsed cron schedule is far larger than the local instant beside it.
    Recurring(Box<::cron::Schedule>),
}

/// The format an `at` expression states its instant in: "Format: `at(yyyy-MM-ddThh:mm)`"
/// (`Schedule$Expression`) — a LOCAL instant, read in the schedule's own time zone, carrying no
/// zone of its own.
const AT_EXPRESSION_FORMAT: &str = "%Y-%m-%dT%H:%M";

/// How many fields a mute schedule's cron expression carries: "Format: `cron(Minutes Hours
/// Day-of-month Month Day-of-week)`" (`Schedule$Expression`).
const CRON_FIELDS: usize = 5;

/// The seconds field the shared cron grammar reads an expression's instants from, which the
/// five documented fields carry no place for: a mute window opens on the minute the expression
/// names.
const CRON_SECONDS_FIELD: &str = "0";

impl MuteWindows {
    /// The windows `schedule` names, or `None` when it names none this emulator can read — an
    /// expression in neither documented form, a time zone no IANA zone answers to, or a duration
    /// naming no fixed span.
    pub fn of(schedule: &Schedule) -> Option<Self> {
        Some(Self {
            form: schedule_form(&schedule.expression)?,
            timezone: schedule_timezone(schedule.timezone.as_deref())?,
            duration_secs: schedule_duration_secs(&schedule.duration)?,
        })
    }

    /// Whether these windows recur or there is only ever one of them — the rule's `MuteType`.
    pub fn mute_type(&self) -> &'static str {
        match self.form {
            ScheduleForm::Once(_) => MUTE_TYPE_ONE_TIME,
            ScheduleForm::Recurring(_) => MUTE_TYPE_RECURRING,
        }
    }

    /// The first instant strictly after `after` (epoch seconds) at which one of these windows
    /// opens, or `None` when none ever opens again.
    fn opens_after(&self, after: f64) -> Option<f64> {
        let after = instant(after)?.with_timezone(&self.timezone);
        let opens_at = match &self.form {
            ScheduleForm::Once(local) => {
                // The expression names a local wall-clock instant; the zone turns it into the
                // moment it happens at. A local instant its zone skips (the spring-forward gap)
                // is a moment that never happens, so the window never opens.
                let opens_at = self.timezone.from_local_datetime(local).earliest()?;
                (opens_at > after).then_some(opens_at)?
            }
            ScheduleForm::Recurring(schedule) => schedule.after(&after).next()?,
        };
        Some(epoch_secs(&opens_at))
    }
}

/// The instants `expression` names, or `None` when it is in neither documented form.
///
/// "You can use one of two expression formats: Cron expressions - For recurring mute windows.
/// Format: `cron(Minutes Hours Day-of-month Month Day-of-week)` […] At expressions - For one-time
/// mute windows. Format: `at(yyyy-MM-ddThh:mm)`" (`Schedule$Expression`). The cron body is read by
/// the shared grammar, which takes the seconds field the documented five do not carry.
fn schedule_form(expression: &str) -> Option<ScheduleForm> {
    let inside = expression.trim().strip_suffix(')')?;
    if let Some(at) = inside.strip_prefix("at(") {
        return NaiveDateTime::parse_from_str(at.trim(), AT_EXPRESSION_FORMAT)
            .ok()
            .map(ScheduleForm::Once);
    }
    let fields: Vec<&str> = inside.strip_prefix("cron(")?.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return None;
    }
    format!("{CRON_SECONDS_FIELD} {}", fields.join(" "))
        .parse::<::cron::Schedule>()
        .ok()
        .map(|schedule| ScheduleForm::Recurring(Box::new(schedule)))
}

/// The zone a schedule is evaluated in: the one it names, or UTC when it names none ("If you
/// don't specify a time zone, UTC is used by default.", `Schedule$Timezone`). `None` for a value
/// that names no zone at all.
fn schedule_timezone(timezone: Option<&str>) -> Option<Tz> {
    match timezone {
        None => Some(Tz::UTC),
        Some(named) => named.parse::<Tz>().ok(),
    }
}

/// How long a window of this schedule stays open, or `None` for a `Duration` naming no fixed
/// span — read through the one ISO 8601 duration reading this project shares, so a mute window's
/// length is read exactly as every other service reads a duration member.
fn schedule_duration_secs(duration: &str) -> Option<f64> {
    ::service_common::iso8601_span::iso8601_span(duration).map(|span| span.as_secs_f64())
}

/// `secs` as the moment the schedule grammars measure from, or `None` for a value no instant
/// answers to.
fn instant(secs: f64) -> Option<DateTime<Utc>> {
    if !secs.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    DateTime::from_timestamp(secs.floor() as i64, 0)
}

/// One of those moments back as the epoch seconds this store measures its clock in.
fn epoch_secs<Zone: TimeZone>(at: &DateTime<Zone>) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let secs = at.timestamp() as f64;
    secs
}

/// The shortest span a mute window may stay open, and the longest: "Specify the duration using
/// ISO 8601 duration format with a minimum of 1 minute (`PT1M`) and maximum of 15 days (`P15D`)."
/// (`Schedule$Duration`).
pub const MIN_MUTE_DURATION_SECS: f64 = 60.0;

/// The other end of that published range (`Schedule$Duration`).
pub const MAX_MUTE_DURATION_SECS: f64 = 15.0 * 24.0 * 60.0 * 60.0;

/// The request member path a refusal of the expression names.
const SCHEDULE_EXPRESSION_PARAMETER: &str = "Rule.Schedule.Expression";

/// The request member path a refusal of the window's length names.
const SCHEDULE_DURATION_PARAMETER: &str = "Rule.Schedule.Duration";

/// The request member path a refusal of the time zone names.
const SCHEDULE_TIMEZONE_PARAMETER: &str = "Rule.Schedule.Timezone";

/// Why `schedule` names no mute window this service can run, as the sentence CloudWatch states an
/// unusable parameter value in ([`super::value_is_invalid_message`]), or `None` when it names one.
///
/// Three rules the model cannot express, so the boundary walk lets all three through: the
/// expression must be in one of the two documented forms, the time zone must be a zone, and the
/// duration must name a fixed span inside the published range
/// ([`MIN_MUTE_DURATION_SECS`]..[`MAX_MUTE_DURATION_SECS`]). Stated here rather than in the
/// handler so every operation that stores or reads a schedule is judged by one rule.
pub fn alarm_mute_rule_schedule_error(schedule: &Schedule) -> Option<String> {
    if schedule_form(&schedule.expression).is_none() {
        return Some(super::value_is_invalid_message(
            &schedule.expression,
            SCHEDULE_EXPRESSION_PARAMETER,
        ));
    }
    if schedule_timezone(schedule.timezone.as_deref()).is_none() {
        return Some(super::value_is_invalid_message(
            schedule.timezone.as_deref().unwrap_or_default(),
            SCHEDULE_TIMEZONE_PARAMETER,
        ));
    }
    let within_range = schedule_duration_secs(&schedule.duration).is_some_and(|span| {
        (MIN_MUTE_DURATION_SECS..=MAX_MUTE_DURATION_SECS).contains(&span)
    });
    (!within_range).then(|| {
        super::value_is_invalid_message(&schedule.duration, SCHEDULE_DURATION_PARAMETER)
    })
}
