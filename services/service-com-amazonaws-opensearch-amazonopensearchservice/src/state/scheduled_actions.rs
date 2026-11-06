// SPDX-License-Identifier: CC0-1.0

//! Scheduled actions: the changes a domain has queued rather than taken yet.
//!
//! `ListScheduledActions` reports them and `UpdateScheduledAction` moves one to another
//! time or applies it now, both from the ONE [`ScheduledChange`] record below — so the
//! action a listing reports and the action an update moves are the same record.

use super::*;

/// One configuration change scheduled on a domain, as this emulator persists it.
///
/// A scheduled action is a record with an identity of its own — an action id, a type,
/// a severity and the instant it is to happen — rather than a projection of some
/// other record: `ListScheduledActions` reports it, `DescribeDomainAutoTunes` reports
/// the Auto-Tune ones among them and `UpdateScheduledAction` reschedules it, so this
/// is the ONE copy all of them reach. The operations that SCHEDULE
/// one (`StartServiceSoftwareUpdate` for a service software update, and the Auto-Tune
/// side of `UpdateDomainConfig` for a blue/green enhancement) are still scaffold
/// stubs, so today the only writer is whatever puts a record here directly.
///
/// Its members are kept as the values the model's own enums admit (`ActionType`,
/// `ActionSeverity`, `ScheduledBy`, `ActionStatus`) so the report is the stored
/// record rather than a second derivation of it.
#[derive(Debug, Clone)]
pub struct ScheduledChange {
    /// `ScheduledAction$Id`: "the unique identifier of the scheduled action". Unique
    /// per domain, so it is the key the domain holds the record under and the place a
    /// pagination token names.
    pub id: String,
    /// `ScheduledAction$Type`, one of `ActionType`'s values: which change is
    /// scheduled.
    pub action_type: String,
    /// `ScheduledAction$Severity`, one of `ActionSeverity`'s values.
    pub severity: String,
    /// The instant the change is scheduled to happen, reported as
    /// `ScheduledAction$ScheduledTime`.
    pub scheduled_at: SystemTime,
    /// `ScheduledAction$Description`: "a description of the action to be taken",
    /// absent where the type and severity say it all.
    pub description: Option<String>,
    /// `ScheduledAction$ScheduledBy`, one of `ScheduledBy`'s values: whether a caller
    /// scheduled this change or the service did.
    pub scheduled_by: String,
    /// `ScheduledAction$Status`, one of `ActionStatus`'s values: where the change is
    /// in its own lifecycle.
    pub status: String,
    /// `ScheduledAction$Mandatory`: "whether the action is required or optional".
    pub mandatory: bool,
    /// `ScheduledAction$Cancellable`: "whether or not the scheduled action is
    /// cancellable".
    pub cancellable: bool,
}

impl ScheduledChange {
    /// This record as `ScheduledAction` — the ONE projection of it, so every operation
    /// that reports a scheduled action reports the same values.
    pub fn to_action(&self) -> ScheduledAction {
        ScheduledAction {
            id: self.id.clone(),
            r#type: self.action_type.clone(),
            severity: self.severity.clone(),
            scheduled_time: epoch_millis(self.scheduled_at),
            description: self.description.clone(),
            scheduled_by: Some(self.scheduled_by.clone()),
            status: Some(self.status.clone()),
            mandatory: Some(self.mandatory),
            cancellable: Some(self.cancellable),
        }
    }

    /// Whether this scheduled change is an Auto-Tune optimization of the domain.
    ///
    /// `ScheduledAutoTuneDetails$ActionType` is a `ScheduledAutoTuneActionType`, a
    /// SUBSET of the `ActionType` a scheduled change carries: a change of a type
    /// outside that subset (a service software update) has no `AutoTune` form at all,
    /// so it belongs to the scheduled-action listing and not to the Auto-Tune one.
    /// The subset is the model's own enum, as generated into
    /// [`SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES`], so no second list of it is kept
    /// here.
    pub fn is_auto_tune(&self) -> bool {
        SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES.contains(&self.action_type.as_str())
    }

    /// This record as `AutoTune` — the ONE projection of an Auto-Tune optimization, so
    /// the Auto-Tune report and the scheduled-action listing cannot describe the same
    /// change differently.
    ///
    /// `AutoTuneType` admits only `SCHEDULED_ACTION`, and the detail container carries
    /// the stored instant as `Date` ("the date and time when the Auto-Tune action is
    /// scheduled for the domain"), the stored type and severity, and the stored
    /// description as `Action` ("a description of the Auto-Tune action") — absent for
    /// a change stored without one.
    pub fn to_auto_tune(&self) -> AutoTune {
        AutoTune {
            auto_tune_type: Some(AUTO_TUNE_TYPE_SCHEDULED_ACTION.to_string()),
            auto_tune_details: Some(AutoTuneDetails {
                scheduled_auto_tune_details: Some(ScheduledAutoTuneDetails {
                    date: Some(datetime(self.scheduled_at)),
                    action_type: Some(self.action_type.clone()),
                    action: self.description.clone(),
                    severity: Some(self.severity.clone()),
                }),
            }),
        }
    }

    /// Whether the service has already begun or finished this change, so there is no
    /// longer an instant of it left to move: `ActionStatus`'s
    /// [`ACTION_STATUS_IN_PROGRESS`] and [`ACTION_STATUS_COMPLETED`].
    pub fn under_way(&self) -> bool {
        self.status == ACTION_STATUS_IN_PROGRESS || self.status == ACTION_STATUS_COMPLETED
    }

    /// Re-time this change to `at`, as a caller's reschedule leaves the record.
    ///
    /// The instant is the whole of what the caller chose; the other two members the
    /// move settles follow from the call itself: the change is now one a caller
    /// scheduled rather than one the service picked the time of, and a change that has
    /// been given an instant to happen at is waiting for it.
    pub fn reschedule(&mut self, at: SystemTime) {
        self.scheduled_at = at;
        self.scheduled_by = SCHEDULED_BY_CUSTOMER.to_string();
        self.status = ACTION_STATUS_PENDING_UPDATE.to_string();
    }
}

/// `ActionStatus`'s `PENDING_UPDATE`: the change has an instant to happen at and is
/// waiting for it.
pub const ACTION_STATUS_PENDING_UPDATE: &str = "PENDING_UPDATE";

/// `ActionStatus`'s `IN_PROGRESS`: the change is being applied to the domain.
pub const ACTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ActionStatus`'s `COMPLETED`: the change has been applied to the domain.
pub const ACTION_STATUS_COMPLETED: &str = "COMPLETED";

/// `ScheduledBy`'s `CUSTOMER`: a caller chose when this change happens.
pub const SCHEDULED_BY_CUSTOMER: &str = "CUSTOMER";

/// `ScheduleAt`'s `NOW`: "immediately schedules the update to happen in the current
/// hour if there's capacity available".
pub const SCHEDULE_AT_NOW: &str = "NOW";

/// `ScheduleAt`'s `TIMESTAMP`: "lets you specify a custom date and time to apply the
/// update", which is the one value `DesiredStartTime` belongs with.
pub const SCHEDULE_AT_TIMESTAMP: &str = "TIMESTAMP";

/// `ScheduleAt`'s `OFF_PEAK_WINDOW`: "marks the action to be picked up during an
/// upcoming off-peak window".
pub const SCHEDULE_AT_OFF_PEAK_WINDOW: &str = "OFF_PEAK_WINDOW";

/// How long a day is, for the next opening of a domain's daily off-peak window.
pub(super) const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// `at` as a `SystemTime`, from the epoch-millisecond count a `Long` timestamp of
/// this service carries — the inverse of [`epoch_millis`], so a value this service
/// reported and one a caller sent name the same instant.
///
/// The count comes from the CALLER and `Long` bounds nothing, so the arithmetic is
/// checked: a value no instant of this clock can represent answers the far end of the
/// range it ran off rather than unwinding the handler that read it.
pub fn instant_of_epoch_millis(millis: i64) -> SystemTime {
    let offset = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(offset).unwrap_or_else(|| {
            SystemTime::UNIX_EPOCH
                .checked_add(MAX_REPRESENTABLE_OFFSET)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(offset).unwrap_or_else(|| {
            SystemTime::UNIX_EPOCH
                .checked_sub(MAX_REPRESENTABLE_OFFSET)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    }
}

/// How far from the epoch an instant this service reports can be, for the checked
/// arithmetic of [`instant_of_epoch_millis`]: the largest span every platform's
/// `SystemTime` represents on both sides of the epoch, which is more than four
/// centuries.
pub(super) const MAX_REPRESENTABLE_OFFSET: Duration = Duration::from_secs(400 * 365 * 24 * 60 * 60);

/// The `ValidationException` payload for a `ScheduleAt` outside the modelled enum:
/// the value asked for and the values that name a schedule.
pub fn schedule_at_outside_the_enum(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "ScheduleAt must be one of [{}]: {value}",
            SCHEDULE_AT_VALUES.join(", ")
        )),
    }
}

/// The message a request that names `TIMESTAMP` without the instant it is to schedule
/// for is refused with.
///
/// The service's own fixed sentence for that condition, and the WHOLE message: it names
/// neither the member it is about nor the `ScheduleAt` value that required it.
pub const DESIRED_START_TIME_REQUIRED: &str =
    "Please provide the desired start time to schedule for a specific time.";

/// Whether the `DesiredStartTime` a request carries belongs with the `ScheduleAt` it
/// names: "only specify this parameter if you set `ScheduleAt` to `TIMESTAMP`", and
/// that value "must also provide a value for `DesiredStartTime`".
///
/// A property of the REQUEST alone, so the operations that schedule a change decide
/// it before resolving anything the request named — and both of them state the one
/// sentence, because it is one rule about the request.
pub fn check_desired_start_time(
    schedule_at: &str,
    desired_start_time: Option<i64>,
) -> Result<(), ValidationException> {
    match (schedule_at, desired_start_time) {
        (SCHEDULE_AT_TIMESTAMP, None) => Err(ValidationException {
            message: Some(DESIRED_START_TIME_REQUIRED.to_string()),
        }),
        (SCHEDULE_AT_TIMESTAMP, Some(_)) => Ok(()),
        (_, None) => Ok(()),
        (_, Some(_)) => Err(ValidationException {
            message: Some(format!(
                "DesiredStartTime must only be specified when ScheduleAt is \
                 {SCHEDULE_AT_TIMESTAMP}: {schedule_at}."
            )),
        }),
    }
}

/// The instant a reschedule puts a change at, for a request whose `ScheduleAt` and
/// `DesiredStartTime` [`check_desired_start_time`] has already admitted.
///
/// Each `ScheduleAt` value names its own instant: `NOW` the moment of the call,
/// `TIMESTAMP` the one the caller gave, and `OFF_PEAK_WINDOW` the next opening of
/// the window `domain` carries ([`Domain::next_off_peak_window_start`]).
pub fn rescheduled_instant(
    domain: &Domain,
    schedule_at: &str,
    desired_start_time: Option<i64>,
    now: SystemTime,
) -> SystemTime {
    match schedule_at {
        // `NOW` schedules "the update to happen in the current hour", which is the
        // instant of the call itself.
        SCHEDULE_AT_NOW => now,
        SCHEDULE_AT_TIMESTAMP => desired_start_time.map_or(now, instant_of_epoch_millis),
        SCHEDULE_AT_OFF_PEAK_WINDOW => domain.next_off_peak_window_start(now),
        // `ScheduleAt` is an enum of exactly those three values and the boundary
        // validator refuses anything else before a handler runs, so this arm stands for
        // nothing a request can carry.
        _ => now,
    }
}

/// The `ValidationException` payload for a request whose `ActionType` is not the type
/// of the change its `ActionID` names: "the type of action to reschedule … to
/// retrieve this value, send a `ListScheduledActions` request", so the two members
/// must name ONE change.
pub fn scheduled_action_type_mismatch(
    action_id: &str,
    requested: &str,
    stored: &str,
) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Scheduled action {action_id} is of type {stored}, not {requested}."
        )),
    }
}

/// The message a request whose `ActionID` names no scheduled change of the domain is
/// refused with ([`scheduled_action_not_found`]).
///
/// The service's own fixed sentence for that condition — its grammatical quirk included
/// — and the WHOLE message: it has no variable part, so neither the id that resolved to
/// no change nor the domain it was looked for on appears in it. A
/// `SERVICE_SOFTWARE_UPDATE` change is addressed by the release it deploys, which is why
/// the sentence the service states is about a release.
pub const SCHEDULED_ACTION_NOT_FOUND: &str =
    "ReleaseVersion does not applies for this domain or release does not exist";

/// The `ResourceNotFoundException` payload for an `ActionID` that names no scheduled
/// change of a domain the store DOES hold.
///
/// Its wording is the service's fixed sentence for that condition
/// ([`SCHEDULED_ACTION_NOT_FOUND`]), which carries no value of the request.
pub fn scheduled_action_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(SCHEDULED_ACTION_NOT_FOUND.to_string()),
    }
}

/// The `ConflictException` payload for re-timing a change the service has already
/// begun or finished: there is no instant of it left to move, and the refusal names
/// the status that says so.
pub fn scheduled_action_under_way(action_id: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Scheduled action {action_id} is {status} and can no longer be rescheduled."
        )),
    }
}

/// `AutoTuneType`'s only value: every Auto-Tune optimization this service reports is
/// a scheduled action.
pub const AUTO_TUNE_TYPE_SCHEDULED_ACTION: &str = "SCHEDULED_ACTION";
