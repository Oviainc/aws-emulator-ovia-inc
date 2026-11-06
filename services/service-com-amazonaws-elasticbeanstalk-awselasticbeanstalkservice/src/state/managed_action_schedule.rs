// SPDX-License-Identifier: CC0-1.0

//! The managed actions this service schedules, applies and records for an
//! environment, and the maintenance windows they fall in.
//!
//! Split out of [`super`], which holds the store these records live in.

use super::*;

/// One managed action this service has CARRIED OUT on an environment — "the
/// record of a completed or failed managed action" (the model's
/// `ManagedActionHistoryItem` doc), which is what
/// `DescribeEnvironmentManagedActionHistory` reports.
///
/// A managed action is work the SERVICE performs on an environment on its own
/// maintenance schedule (a managed platform update, an instance refresh), not
/// work a caller asked for: a caller-requested change is reported in the event
/// log ([`Event`]) instead. An action that has NOT finished is not held here —
/// an upcoming or in-progress one is the model's `ManagedAction`, which
/// `DescribeEnvironmentManagedActions` reports — so every record in this log is
/// terminal and its [`Self::status`] is one of the modelled
/// `crate::ops::types::ACTION_HISTORY_STATUS_VALUES`.
///
/// A record lands here when an action the service scheduled for an environment
/// ([`ScheduledManagedAction`]) has FINISHED: the clock completes the action an
/// `ApplyEnvironmentManagedAction` started ([`AppliedManagedAction`]), the
/// environment carries the change it made, and the action becomes this
/// record. `DescribeEnvironmentManagedActionHistory` pages over THIS log, so it
/// reports the actions that really ran and nothing else.
#[derive(Debug, Clone)]
pub struct ManagedActionRecord {
    /// Append order within this emulator, so "most recent first" is a total
    /// order even for two actions that finished inside one clock tick, and so a
    /// `NextToken` names one record unambiguously. Drawn from the store's
    /// shared counter, as an [`Event`]'s is.
    pub sequence: u64,
    /// The environment the action ran on, by `EnvironmentId`: the id identifies
    /// the record even after its name has been released by a termination and
    /// taken by a new environment.
    pub environment_id: String,
    /// "A unique identifier for the managed action."
    pub action_id: String,
    /// "The type of the managed action" — one of the modelled
    /// `crate::ops::types::ACTION_TYPE_VALUES`.
    pub action_type: String,
    /// "A description of the managed action."
    pub action_description: Option<String>,
    /// "The status of the action" — one of the modelled
    /// `crate::ops::types::ACTION_HISTORY_STATUS_VALUES`.
    pub status: String,
    /// "If the action failed, the type of failure" — one of the modelled
    /// `crate::ops::types::FAILURE_TYPE_VALUES`, and absent for an action that
    /// completed.
    pub failure_type: Option<String>,
    /// "If the action failed, a description of the failure."
    pub failure_description: Option<String>,
    /// "The date and time that the action started executing."
    pub executed_time: ::aws_smithy_types::DateTime,
    /// "The date and time that the action finished executing."
    pub finished_time: Option<::aws_smithy_types::DateTime>,
}

impl ManagedActionRecord {
    /// The instant this record is ORDERED by, as an exact
    /// (seconds, nanoseconds) key: when the action finished, which is why it is
    /// in the history at all, falling back to when it started for a record that
    /// carries no finish instant.
    pub fn history_instant(&self) -> (i64, u32) {
        instant_key(self.finished_time.as_ref().unwrap_or(&self.executed_time))
    }

    /// The modelled `ManagedActionHistoryItem` for this record.
    pub fn to_history_item(&self) -> ManagedActionHistoryItem {
        ManagedActionHistoryItem {
            action_id: Some(self.action_id.clone()),
            action_type: Some(self.action_type.clone()),
            action_description: self.action_description.clone(),
            failure_type: self.failure_type.clone(),
            status: Some(self.status.clone()),
            failure_description: self.failure_description.clone(),
            executed_time: Some(self.executed_time),
            finished_time: self.finished_time,
        }
    }
}

// ---------------------------------------------------------------------------
// Managed actions
//
// A managed action is maintenance the SERVICE performs on an environment in the
// environment's own maintenance window, rather than work a caller asked for.
// Which actions an environment has is decided by the managed-update options its
// configuration carries (`aws:elasticbeanstalk:managedactions`) together with
// what the service has to do to it: a platform update is due while the
// environment runs a release its platform branch has superseded, and an
// instance replacement is due a week after the instances it runs were launched.
// So the SCHEDULE is DERIVED from the stored environment and the published
// catalog, and only what a caller has changed about it — the action an
// `ApplyEnvironmentManagedAction` started, and the record of one that has
// finished — is stored.
//
// Nothing RUNS a scheduled action when its window comes round: the service's own
// maintenance scheduler is not emulated, so an action is carried out only when a
// caller applies it. A window is days away, so inside one run an action stays
// upcoming; over a longer-lived store the same maintenance is reported in the
// next window once the one it named has passed, under the id that window gives
// it.
// ---------------------------------------------------------------------------

/// The configuration-option namespace Elastic Beanstalk's published option
/// reference puts the managed-update options in, and the options in it this
/// service reads: whether managed actions run at all
/// ("`ManagedActionsEnabled`: enable managed platform updates") and the
/// maintenance window they run in ("`PreferredStartTime`: configure a
/// maintenance window for managed actions in UTC").
pub(super) const NAMESPACE_MANAGED_ACTIONS: &str = "aws:elasticbeanstalk:managedactions";
/// See [`NAMESPACE_MANAGED_ACTIONS`].
pub(super) const OPTION_MANAGED_ACTIONS_ENABLED: &str = "ManagedActionsEnabled";
/// See [`NAMESPACE_MANAGED_ACTIONS`].
pub(super) const OPTION_PREFERRED_START_TIME: &str = "PreferredStartTime";

/// The namespace of that same reference's platform-update options, and the two
/// this service reads: whether the environment's instances are replaced weekly
/// ("`InstanceRefreshEnabled`: enable weekly instance replacement") and how far
/// an update may go ("`UpdateLevel`: the highest level of update to apply with
/// managed platform updates").
pub(super) const NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE: &str =
    "aws:elasticbeanstalk:managedactions:platformupdate";
/// See [`NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE`].
pub(super) const OPTION_INSTANCE_REFRESH_ENABLED: &str = "InstanceRefreshEnabled";
/// See [`NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE`].
pub(super) const OPTION_UPDATE_LEVEL: &str = "UpdateLevel";

/// The `UpdateLevel` that admits every update of the environment's platform
/// branch, "minor" and "patch" releases alike. The option's documented values
/// are `patch | minor`, and it is the level the service applies up to, so this
/// is also how an environment whose configuration states NO level is
/// maintained: nothing is holding the service to patches.
pub(super) const UPDATE_LEVEL_MINOR: &str = "minor";
/// The other `UpdateLevel`: only a release inside the version line the
/// environment already runs — same major and minor version — is applied.
pub(super) const UPDATE_LEVEL_PATCH: &str = "patch";

/// The published restriction on `PreferredStartTime`: the weekday-and-time form
/// a maintenance window is stated in (`Tue:09:00`), which is what
/// [`maintenance_window`] reads one back out of. The weekday names are
/// [`PREFERRED_START_TIME_DAYS`] and the two agree.
pub(super) const PREFERRED_START_TIME_PATTERN: &str =
    "^(Mon|Tue|Wed|Thu|Fri|Sat|Sun):([01][0-9]|2[0-3]):[0-5][0-9]$";

/// The weekday names `PreferredStartTime` states its window's day by, in week
/// order, so a name's position in this list is how many days into the week that
/// day is. The set is the one the option's own published pattern lists
/// ([`PREFERRED_START_TIME_PATTERN`]).
pub(super) const PREFERRED_START_TIME_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Seconds in a day and days in a week.
///
/// A maintenance window is stated in UTC, where every day is exactly this many
/// seconds long, so placing a window is integer arithmetic over whole epoch days
/// — no month, year, leap day or time zone takes part in it, and no calendar is
/// consulted.
pub(super) const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
/// See [`SECONDS_PER_DAY`].
pub(super) const DAYS_PER_WEEK: i64 = 7;

/// How many days into its week the epoch day is: 1970-01-01 was a Thursday, the
/// fourth day of a week that starts on Monday as
/// [`PREFERRED_START_TIME_DAYS`] does.
pub(super) const EPOCH_WEEKDAY: i64 = 3;

/// How often an environment's maintenance window comes round.
///
/// "`PreferredStartTime`: configure a maintenance window for managed actions in
/// UTC" is stated as ONE weekday and time of day
/// ([`PREFERRED_START_TIME_PATTERN`]), so the window it names recurs once a
/// week; "`InstanceRefreshEnabled`: enable WEEKLY instance replacement" puts
/// the replacement on the same period. Both come from the published options, so
/// this period is DOCUMENTED rather than chosen here.
pub(super) const MAINTENANCE_WINDOW_PERIOD: i64 = DAYS_PER_WEEK * SECONDS_PER_DAY;

/// The `ActionStatus` of an action the service has scheduled and not started
/// yet — the one status `ApplyEnvironmentManagedAction` accepts ("A managed
/// action can be applied only if its status is Scheduled"). Read from the
/// modelled value set, so this service reports no status the model does not
/// declare.
pub const ACTION_STATUS_SCHEDULED: &str = ACTION_STATUS_VALUES[2];
/// The `ActionStatus` of an action that is being carried out now. See
/// [`ACTION_STATUS_SCHEDULED`].
pub const ACTION_STATUS_RUNNING: &str = ACTION_STATUS_VALUES[1];

/// The `ActionType` of a managed platform update, and of a managed replacement
/// of the environment's Amazon EC2 instances. Read from the modelled value set
/// ([`crate::ops::types::ACTION_TYPE_VALUES`]).
pub const ACTION_TYPE_INSTANCE_REFRESH: &str = ACTION_TYPE_VALUES[0];
/// See [`ACTION_TYPE_INSTANCE_REFRESH`].
pub const ACTION_TYPE_PLATFORM_UPDATE: &str = ACTION_TYPE_VALUES[1];

/// The `ActionHistoryStatus` of an action that ran to completion — what a
/// managed action the clock has finished is recorded in the history as. Read
/// from the modelled value set
/// ([`crate::ops::types::ACTION_HISTORY_STATUS_VALUES`]).
pub const ACTION_HISTORY_STATUS_COMPLETED: &str = ACTION_HISTORY_STATUS_VALUES[0];

/// The weekday and time of day an environment's maintenance window opens at, as
/// `PreferredStartTime` states it, in the units the window arithmetic works in:
/// whole epoch days and seconds into the day (see [`SECONDS_PER_DAY`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct MaintenanceWindow {
    /// How many days into the week the window's day is; see
    /// [`PREFERRED_START_TIME_DAYS`].
    pub(super) weekday: i64,
    /// How many seconds into that day the window opens.
    pub(super) second_of_day: i64,
}

/// The maintenance window the configuration `settings` state, if they state one.
///
/// The value is read back in exactly the form the option publishes
/// ([`PREFERRED_START_TIME_PATTERN`]); a value outside it states no window, so
/// the environment is maintained on the week of its own launch instead (see
/// [`next_maintenance_window`]).
pub(super) fn maintenance_window(settings: &[ConfigurationOptionSetting]) -> Option<MaintenanceWindow> {
    let stated = option_value(
        settings,
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_PREFERRED_START_TIME,
    )?;
    let mut components = stated.split(':');
    let day = components.next()?;
    // The option publishes the hour and the minute as a two-digit field each,
    // inside the hours of a day and the minutes of an hour
    // ([`PREFERRED_START_TIME_PATTERN`]). Nothing validates a configuration
    // option's value where it is WRITTEN, so a value in any other form — more
    // components, a sign, a word, a number outside the field — reaches here
    // from the request and states no window at all, which leaves the
    // environment maintained on the week of its own launch.
    let hour = preferred_start_time_field(components.next()?, 23)?;
    let minute = preferred_start_time_field(components.next()?, 59)?;
    if components.next().is_some() {
        return None;
    }
    let weekday = PREFERRED_START_TIME_DAYS
        .iter()
        .position(|name| *name == day)?;
    Some(MaintenanceWindow {
        weekday: i64::try_from(weekday).ok()?,
        // Both fields are inside their own field's range, so the second of the
        // day they name is inside a day and nothing here can overflow.
        second_of_day: i64::from(hour) * 60 * 60 + i64::from(minute) * 60,
    })
}

/// The value one field of `PreferredStartTime` names, when the field is exactly
/// the two ASCII digits the option publishes it as and the value it names is not
/// above `highest`.
pub(super) fn preferred_start_time_field(stated: &str, highest: u8) -> Option<u8> {
    if stated.len() != 2 || !stated.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    stated.parse::<u8>().ok().filter(|value| *value <= highest)
}

/// The epoch second an environment's maintenance window next opens at, at or
/// after `not_before`.
///
/// `window` is the weekday and time the configuration states, when it states
/// one. When it does not, the window is counted from `origin`, the environment's
/// own launch instant: the service has no stated window to wait for, so the week
/// it maintains the environment in is the week that started when the environment
/// did.
pub(super) fn next_maintenance_window(
    window: Option<MaintenanceWindow>,
    origin: i64,
    not_before: i64,
) -> i64 {
    let opened = match window {
        // The window's own weekday in the week `not_before` falls in: step back
        // to that week's first day and forward to the window's day. In UTC a day
        // is a fixed number of seconds ([`SECONDS_PER_DAY`]), so this is whole
        // epoch days throughout. Saturating, so an instant near the end of the
        // representable range answers the last window it can rather than
        // overflowing.
        Some(window) => {
            let day = not_before.div_euclid(SECONDS_PER_DAY);
            let weekday = (day + EPOCH_WEEKDAY).rem_euclid(DAYS_PER_WEEK);
            (day - weekday + window.weekday)
                .saturating_mul(SECONDS_PER_DAY)
                .saturating_add(window.second_of_day)
        }
        None => origin,
    };
    first_recurrence_at_or_after(opened, not_before)
}

/// The first instant of the weekly series through `opened` that is at or after
/// `not_before`. Saturating, for the same reason as above.
pub(super) fn first_recurrence_at_or_after(opened: i64, not_before: i64) -> i64 {
    let periods = not_before
        .saturating_sub(opened)
        .saturating_add(MAINTENANCE_WINDOW_PERIOD - 1)
        .div_euclid(MAINTENANCE_WINDOW_PERIOD)
        .max(0);
    opened.saturating_add(periods.saturating_mul(MAINTENANCE_WINDOW_PERIOD))
}

/// The `ActionId` of the managed action of `action_type` scheduled for the
/// environment `environment_id` in the window that opens at `window_start`, for
/// the occasion `occasion`.
///
/// A scheduled action IS those four things — the environment it maintains, the
/// work it does, the window it does it in, and what makes this occurrence of
/// that work the one that is due (the release an update moves to; the instances
/// a replacement replaces) — so its id is DERIVED from exactly them: every call
/// that reports that action reports it under the same id, no report has to mint
/// one, and the next occurrence of recurring work is an action of its own even
/// where it falls in the same window.
///
/// The form is the one the service issues an `ActionId` in: 128 bits in the
/// canonical `8-4-4-4-12` lowercase-hexadecimal grouping of a UUID, with the
/// version and variant bits of a name-based UUID whose name is the action's own
/// (RFC 9562's version 8, the one reserved for a custom derivation). The 128
/// bits are the leading half of the SHA-256 digest of that name, so the digest
/// and its hexadecimal rendering are the `sha2` and `hex` crates' and nothing
/// here hashes or encodes by hand.
pub(super) fn managed_action_id(
    environment_id: &str,
    action_type: &str,
    window_start: i64,
    occasion: &str,
) -> String {
    use ::sha2::Digest;

    let name =
        format!("urn:{ARN_NAMESPACE}:{environment_id}:{action_type}:{window_start}:{occasion}");
    let digest = ::sha2::Sha256::digest(name.as_bytes());
    let mut body = [0u8; UUID_BYTES];
    body.copy_from_slice(&digest[..UUID_BYTES]);
    // The version nibble and the two variant bits, in the octets the UUID layout
    // puts them in.
    body[6] = (body[6] & 0x0f) | 0x80;
    body[8] = (body[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(body);
    UUID_GROUPS
        .iter()
        .scan(0usize, |at, length| {
            let from = *at;
            *at += length;
            rendered.get(from..*at)
        })
        .collect::<Vec<&str>>()
        .join("-")
}

/// How many bytes a UUID carries, and how many hexadecimal characters each group
/// of its canonical rendering does. See [`managed_action_id`].
pub(super) const UUID_BYTES: usize = 16;
/// See [`UUID_BYTES`].
pub(super) const UUID_GROUPS: &[usize] = &[8, 4, 4, 4, 12];

/// What a managed action DOES to the environment it is scheduled for, carried
/// out when the action finishes.
#[derive(Debug, Clone)]
pub enum ManagedActionTarget {
    /// Move the environment to the platform version named by this solution stack
    /// name and this ARN — the two names of the one release the update applies.
    PlatformVersion {
        solution_stack_name: String,
        platform_arn: String,
    },
    /// Replace the environment's Amazon EC2 instances.
    InstanceReplacement,
}

/// One managed action the service has scheduled for an environment and not
/// carried out yet — the model's `ManagedAction`, which
/// `DescribeEnvironmentManagedActions` reports ("an environment's upcoming and
/// in-progress managed actions").
///
/// An upcoming action is DERIVED (see
/// `ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::due_managed_actions`):
/// it exists because the environment's configuration asks the service to
/// maintain it and there is maintenance to do, so reporting one stores nothing.
/// An action a caller has applied is held as an [`AppliedManagedAction`] until
/// the clock finishes it, and reported from there with its status moved to
/// `Running`.
#[derive(Debug, Clone)]
pub struct ScheduledManagedAction {
    /// "A unique identifier for the managed action"; see [`managed_action_id`].
    pub action_id: String,
    /// The environment the action is scheduled for, by `EnvironmentId`.
    pub environment_id: String,
    /// "The type of managed action" — one of
    /// [`crate::ops::types::ACTION_TYPE_VALUES`].
    pub action_type: String,
    /// "A description of the managed action."
    pub action_description: String,
    /// "The status of the managed action" — one of
    /// [`crate::ops::types::ACTION_STATUS_VALUES`].
    pub status: String,
    /// "The start time of the maintenance window in which the managed action
    /// will execute."
    pub window_start_time: ::aws_smithy_types::DateTime,
    /// What the action does when it finishes.
    pub target: ManagedActionTarget,
}

impl ScheduledManagedAction {
    /// The action of `action_type` the service has SCHEDULED for
    /// `environment_id` in the window opening at `window_start`, for the
    /// occasion `occasion` (see [`managed_action_id`]).
    pub(super) fn scheduled(
        environment_id: &str,
        action_type: &str,
        action_description: String,
        window_start: i64,
        occasion: &str,
        target: ManagedActionTarget,
    ) -> Self {
        Self {
            action_id: managed_action_id(environment_id, action_type, window_start, occasion),
            environment_id: environment_id.to_string(),
            action_type: action_type.to_string(),
            action_description,
            status: ACTION_STATUS_SCHEDULED.to_string(),
            window_start_time: ::aws_smithy_types::DateTime::from_secs(window_start),
            target,
        }
    }

    /// The modelled `ManagedAction` for this action.
    pub fn to_managed_action(&self) -> ManagedAction {
        ManagedAction {
            action_id: Some(self.action_id.clone()),
            action_description: Some(self.action_description.clone()),
            action_type: Some(self.action_type.clone()),
            status: Some(self.status.clone()),
            window_start_time: Some(self.window_start_time),
        }
    }
}

/// One managed action an `ApplyEnvironmentManagedAction` has started — "applies
/// a scheduled managed action IMMEDIATELY", so the action is running now rather
/// than waiting for its window — and the work the clock owes when it finishes.
#[derive(Debug, Clone)]
pub struct AppliedManagedAction {
    /// The action as it was scheduled, with its status moved to
    /// [`ACTION_STATUS_RUNNING`]: the same id, type, description and window it
    /// was reported under before it was applied.
    pub action: ScheduledManagedAction,
    /// When the action finishes, one [`STATUS_TRANSITION_SPAN`] after the call
    /// that applied it.
    pub due_at: Instant,
    /// The same instant on the server clock — the `FinishedTime` the history
    /// records, dated when the action finished rather than when a later call
    /// happened to observe it as finished.
    pub due_date: ::aws_smithy_types::DateTime,
    /// "The date and time that the action started executing": when it was
    /// applied.
    pub executed_time: ::aws_smithy_types::DateTime,
    /// The request that applied the action, which the event its completion
    /// records belongs to.
    pub request_id: String,
}

impl AppliedManagedAction {
    /// The history record this action becomes once it has finished.
    pub(super) fn to_record(&self, sequence: u64) -> ManagedActionRecord {
        ManagedActionRecord {
            sequence,
            environment_id: self.action.environment_id.clone(),
            action_id: self.action.action_id.clone(),
            action_type: self.action.action_type.clone(),
            action_description: Some(self.action.action_description.clone()),
            // Nothing fails an applied action here, so every action this
            // emulator completes completed, and carries neither failure member.
            status: ACTION_HISTORY_STATUS_COMPLETED.to_string(),
            failure_type: None,
            failure_description: None,
            executed_time: self.executed_time,
            finished_time: Some(self.due_date),
        }
    }
}

/// The managed platform update available to an environment: the release its
/// branch publishes above the one it runs, by both of that release's names.
pub(super) struct AvailablePlatformUpdate {
    /// The version the environment runs now, and the one the update moves it to.
    pub(super) from_version: String,
    pub(super) to_version: String,
    pub(super) solution_stack_name: String,
    pub(super) platform_arn: String,
}

impl AvailablePlatformUpdate {
    /// "A description of the managed action", naming the update this action
    /// applies. The wording is this emulator's own — the model documents the
    /// member only as a description — and it names the two platform versions the
    /// update moves between, which is what the action is.
    pub(super) fn description(&self) -> String {
        format!(
            "Platform update from {} to {}",
            self.from_version, self.to_version
        )
    }
}

/// "A description of the managed action" for a managed replacement of an
/// environment's Amazon EC2 instances. See
/// [`AvailablePlatformUpdate::description`] for the wording.
pub(super) const INSTANCE_REFRESH_DESCRIPTION: &str = "Instance refresh";

/// Whether moving from the platform version `from` to `to` stays inside one
/// version line — the same major and minor version — which is what makes it a
/// `patch` update rather than a `minor` one.
pub(super) fn is_patch_update(from: &str, to: &str) -> bool {
    let line = |version: &str| {
        let mut components = version.split('.');
        Some((
            components.next()?.to_string(),
            components.next()?.to_string(),
        ))
    };
    match (line(from), line(to)) {
        (Some(from), Some(to)) => from == to,
        _ => false,
    }
}
