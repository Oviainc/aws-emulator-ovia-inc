// SPDX-License-Identifier: CC0-1.0

//! The channel schedule slice of this service's state.
//!
//! One file for the whole slice — the per-channel schedule, the rules
//! `BatchUpdateSchedule` validates a request against, the clock-owed cleanup and
//! the start-time order a read reports — so the slice can be read, and merged, on
//! its own. The store itself is the `schedule` member of
//! [`super::ChannelRecord`]: a schedule belongs to exactly one channel, so it
//! lives on that channel's record and goes away with it rather than in a second
//! map beside `channels`.
//!
//! Every rule here comes from the model's own documentation:
//!
//! * `ScheduleAction$ActionName`: "The name of the action, must be unique within
//!   the schedule. … A name is unique if it is no longer in the schedule. The
//!   schedule is automatically cleaned up to remove actions with a start time of
//!   more than 1 hour ago (approximately) so at that point a name can be reused."
//!   — the name-uniqueness rule and [`SCHEDULE_ACTION_RETENTION`].
//! * `ScheduleActionStartSettings`: "Settings to specify when an action should
//!   occur. Only one of the options must be selected." — exactly one start mode.
//! * `FixedModeScheduleActionStartSettings$Time`: "UTC format:
//!   yyyy-mm-ddThh:mm:ss.nnnZ. All the letters are digits … except for the two
//!   constants "T" for time and "Z" for "UTC format"." — [`parse_action_time`].
//! * `FollowModeScheduleActionStartSettings$ReferenceActionName`: "The action name
//!   of another action that this one refers to." — the reference must be an action
//!   that exists once the request has been applied.
//! * `ScheduleActionSettings`: "Holds the settings for a single schedule action."
//!   — exactly one action type per action.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{ScheduleAction, ScheduleActionSettings, ValidationError};

/// How long an action stays in the schedule once its start time has passed.
///
/// NOT a span this emulator chose: `ScheduleAction$ActionName` documents it —
/// "The schedule is automatically cleaned up to remove actions with a start time
/// of more than 1 hour ago (approximately) so at that point a name can be
/// reused." The cleanup is what the clock owes the schedule, so it is
/// materialised in [`ChannelSchedule::settle`] rather than slept for.
pub const SCHEDULE_ACTION_RETENTION: Duration = Duration::from_secs(60 * 60);

/// The most actions this emulator keeps in one channel's schedule.
///
/// A ceiling on the store, not an AWS quota: it exists so a create loop cannot
/// grow the emulator's heap without bound, which is why the refusal it produces
/// names the limit rather than a per-account quota. `BatchUpdateSchedule` models
/// `TooManyRequestsException`, which is that refusal once the schedule is full.
pub const MAX_SCHEDULE_ACTIONS: usize = 500;

/// Why [`ChannelSchedule::apply`] stored nothing. The whole request is one
/// update: either every create and delete in it applies, or none does.
#[derive(Debug)]
pub enum ScheduleRefusal {
    /// The request does not describe a schedule the service can apply. One
    /// [`ValidationError`] per problem found, each naming the member path it was
    /// found at, which is what `UnprocessableEntityException$ValidationErrors`
    /// carries ("A collection of validation error responses").
    Invalid(Vec<ValidationError>),
    /// Applying the request's creates would push this schedule past
    /// [`MAX_SCHEDULE_ACTIONS`].
    Full {
        /// How many actions the schedule would then hold.
        requested: usize,
        /// [`MAX_SCHEDULE_ACTIONS`], so the refusal can name it.
        limit: usize,
    },
}

/// What [`ChannelSchedule::apply`] changed, as the response reports it:
/// `BatchScheduleActionCreateResult` is "List of actions that have been created
/// in the schedule" and `BatchScheduleActionDeleteResult` is "List of actions
/// that have been deleted from the schedule" — so the deleted actions are read
/// out of the stored schedule (the request carries only their names).
#[derive(Debug, Default)]
pub struct AppliedSchedule {
    pub created: Vec<ScheduleAction>,
    pub deleted: Vec<ScheduleAction>,
}

/// One action in a channel's schedule: the action as the request stored it, plus
/// what the schedule itself knows about it.
#[derive(Debug, Clone)]
pub struct ScheduleActionRecord {
    pub action: ScheduleAction,
    /// The instant the action was added. Actions "are always added to the
    /// schedule immediately" (`FixedModeScheduleActionStartSettings$Time`), so
    /// this is also an immediate-mode action's start time.
    pub added_at: SystemTime,
    /// Mint order within this schedule, which breaks a tie between two actions
    /// that start at the same instant so a read's order is stable.
    pub sequence: u64,
}

/// One channel's schedule.
///
/// Keyed by action name, because the model makes the name the schedule's
/// identity: "must be unique within the schedule. This name provides the main
/// reference to an action once it is added to the schedule."
#[derive(Debug, Clone, Default)]
pub struct ChannelSchedule {
    actions: BTreeMap<String, ScheduleActionRecord>,
    next_sequence: u64,
}

impl ChannelSchedule {
    /// Delete every action, which is what `DeleteSchedule` does ("Delete all
    /// schedule actions on a channel").
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// Materialise the cleanup the clock already made due: an action whose start
    /// time is more than [`SCHEDULE_ACTION_RETENTION`] before `now` is no longer
    /// in the schedule, so its name can be reused.
    ///
    /// Owed at `now` whether or not anybody called, so a reporting operation may
    /// run it: it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(SCHEDULE_ACTION_RETENTION) else {
            return;
        };
        // The start instants are read before the removal so a follow-mode action
        // is placed against the schedule it was validated against, rather than
        // against one its reference has already been swept out of.
        let expired: Vec<String> = self
            .actions
            .values()
            .filter(|record| self.start_position(record).0 < cutoff)
            .map(|record| record.action.action_name.clone())
            .collect();
        for name in expired {
            self.actions.remove(&name);
        }
    }

    /// Every action, in the order it starts in the channel: by start instant,
    /// then by how many follow links it took to reach that instant (so an action
    /// comes after the one it follows), then by mint order so the order is total
    /// and stable across reads.
    ///
    /// A schedule is a timeline, so this is the order a reader walks it in — and
    /// `DescribeSchedule` is paginated over exactly this sequence.
    pub fn in_start_order(&self) -> Vec<&ScheduleActionRecord> {
        let mut ordered: Vec<&ScheduleActionRecord> = self.actions.values().collect();
        ordered.sort_by_key(|record| {
            let (at, hops) = self.start_position(record);
            (at, hops, record.sequence)
        });
        ordered
    }

    /// Where `record` starts in the channel: the instant, and how many follow
    /// links were walked to reach it.
    ///
    /// Fixed mode is the `Time` it carries; immediate mode is the instant the
    /// action was added, since it "occurs as soon as possible"
    /// (`ImmediateModeScheduleActionStartSettings`); follow mode starts where the
    /// action it refers to starts, so it is resolved through the reference chain —
    /// and the hop count is what keeps a follower after its reference when both
    /// start at the same instant. The walk is bounded by the schedule's size, so a
    /// reference left dangling by a delete — or a cycle one request's mutually
    /// following actions introduced — falls back to the action's own `added_at`
    /// rather than running on.
    fn start_position(&self, record: &ScheduleActionRecord) -> (SystemTime, usize) {
        let mut current = record;
        let mut hops = 0;
        while hops <= self.actions.len() {
            let start = &current.action.schedule_action_start_settings;
            if let Some(fixed) = start.fixed_mode_schedule_action_start_settings.as_ref() {
                return (
                    parse_action_time(&fixed.time).unwrap_or(current.added_at),
                    hops,
                );
            }
            let Some(follow) = start.follow_mode_schedule_action_start_settings.as_ref() else {
                return (current.added_at, hops);
            };
            match self.actions.get(&follow.reference_action_name) {
                Some(reference) => {
                    current = reference;
                    hops += 1;
                }
                None => return (current.added_at, hops),
            }
        }
        (record.added_at, hops)
    }

    /// Apply one `BatchUpdateSchedule` request: delete the named actions, then
    /// create the given ones.
    ///
    /// Validated as a whole before anything is written, so a request that names
    /// one bad action changes nothing. Deletes are applied before creates, which
    /// is what lets one request free a name and reuse it — the name is "unique if
    /// it is no longer in the schedule".
    pub fn apply(
        &mut self,
        creates: &[ScheduleAction],
        deletes: &[String],
        now: SystemTime,
    ) -> Result<AppliedSchedule, ScheduleRefusal> {
        let mut problems: Vec<ValidationError> = Vec::new();

        // Deletes name actions the schedule holds; a name it does not hold is not
        // deletable. A repeated name asks for the same deletion twice, which is
        // the one deletion.
        //
        // The model documents no outcome for deleting an absent name, so the
        // reading here is the one its error channel supports: `ActionName` is "the
        // main reference to an action once it is added to the schedule", a name
        // the schedule does not hold references no action, and a request naming
        // something the service cannot act on is what
        // `UnprocessableEntityException$ValidationErrors` reports — the same
        // channel every other rule in this file answers through.
        let mut doomed: BTreeSet<String> = BTreeSet::new();
        // The same names in the order the request listed them, so the response
        // reports the deleted actions in the order they were asked for rather than
        // in the set's own.
        let mut in_request_order: Vec<String> = Vec::new();
        for (index, name) in deletes.iter().enumerate() {
            if self.actions.contains_key(name) {
                if doomed.insert(name.clone()) {
                    in_request_order.push(name.clone());
                }
            } else {
                problems.push(problem(
                    &format!("deletes/actionNames/{index}"),
                    &format!("action name {name} is not in the schedule"),
                ));
            }
        }

        // Creates are checked against the schedule as it will be once this whole
        // request has been applied: the names it leaves in place, plus every name
        // it creates. A follow reference may therefore name an action created
        // later in the same request — the request is one update, so nothing in the
        // model makes the order of the `creates` list constrain it.
        let post_apply: BTreeSet<&str> = self
            .actions
            .keys()
            .map(String::as_str)
            .filter(|name| !doomed.contains(*name))
            .chain(creates.iter().map(|action| action.action_name.as_str()))
            .collect();
        let mut minted: BTreeSet<String> = BTreeSet::new();
        for (index, action) in creates.iter().enumerate() {
            let path = format!("creates/scheduleActions/{index}");
            let name = action.action_name.as_str();
            if name.is_empty() {
                problems.push(problem(
                    &format!("{path}/actionName"),
                    "actionName must not be empty",
                ));
                continue;
            }
            if minted.contains(name) {
                problems.push(problem(
                    &format!("{path}/actionName"),
                    &format!("action name {name} is created twice in this request"),
                ));
                continue;
            }
            if self.actions.contains_key(name) && !doomed.contains(name) {
                problems.push(problem(
                    &format!("{path}/actionName"),
                    &format!("action name {name} is already in the schedule"),
                ));
                continue;
            }

            let settings = selected_action_settings(&action.schedule_action_settings);
            if settings != 1 {
                problems.push(problem(
                    &format!("{path}/scheduleActionSettings"),
                    &format!(
                        "exactly one action setting must be selected, {settings} were"
                    ),
                ));
            }

            let start = &action.schedule_action_start_settings;
            let modes = usize::from(start.fixed_mode_schedule_action_start_settings.is_some())
                + usize::from(start.follow_mode_schedule_action_start_settings.is_some())
                + usize::from(start.immediate_mode_schedule_action_start_settings.is_some());
            if modes != 1 {
                problems.push(problem(
                    &format!("{path}/scheduleActionStartSettings"),
                    &format!("exactly one start mode must be selected, {modes} were"),
                ));
            }
            if let Some(fixed) = start.fixed_mode_schedule_action_start_settings.as_ref() {
                if parse_action_time(&fixed.time).is_none() {
                    problems.push(problem(
                        &format!("{path}/scheduleActionStartSettings/fixedModeScheduleActionStartSettings/time"),
                        &format!(
                            "time {} is not UTC format yyyy-mm-ddThh:mm:ss.nnnZ",
                            fixed.time
                        ),
                    ));
                }
            }
            if let Some(follow) = start.follow_mode_schedule_action_start_settings.as_ref() {
                let reference = follow.reference_action_name.as_str();
                let resolves = reference != name && post_apply.contains(reference);
                if !resolves {
                    problems.push(problem(
                        &format!("{path}/scheduleActionStartSettings/followModeScheduleActionStartSettings/referenceActionName"),
                        &format!("reference action name {reference} is not another action in the schedule"),
                    ));
                }
            }

            minted.insert(name.to_string());
        }

        if !problems.is_empty() {
            return Err(ScheduleRefusal::Invalid(problems));
        }

        let requested = self.actions.len() - doomed.len() + minted.len();
        if requested > MAX_SCHEDULE_ACTIONS {
            return Err(ScheduleRefusal::Full {
                requested,
                limit: MAX_SCHEDULE_ACTIONS,
            });
        }

        let mut applied = AppliedSchedule::default();
        for name in &in_request_order {
            if let Some(record) = self.actions.remove(name) {
                applied.deleted.push(record.action);
            }
        }
        for action in creates {
            self.actions.insert(
                action.action_name.clone(),
                ScheduleActionRecord {
                    action: action.clone(),
                    added_at: now,
                    sequence: self.next_sequence,
                },
            );
            self.next_sequence += 1;
            applied.created.push(action.clone());
        }
        Ok(applied)
    }
}

/// One entry of `UnprocessableEntityException$ValidationErrors`: where the
/// problem is (`ElementPath` is "Path to the source of the error") and what it is.
fn problem(element_path: &str, error_message: &str) -> ValidationError {
    ValidationError {
        element_path: Some(element_path.to_string()),
        error_message: Some(error_message.to_string()),
    }
}

/// How many of the 17 action types `settings` selects.
///
/// `ScheduleActionSettings` "Holds the settings for a single schedule action", so
/// exactly one is what an action describes; the count is what the refusal reports.
fn selected_action_settings(settings: &ScheduleActionSettings) -> usize {
    [
        settings.hls_id3_segment_tagging_settings.is_some(),
        settings.hls_timed_metadata_settings.is_some(),
        settings.input_prepare_settings.is_some(),
        settings.input_switch_settings.is_some(),
        settings.motion_graphics_image_activate_settings.is_some(),
        settings.motion_graphics_image_deactivate_settings.is_some(),
        settings.pause_state_settings.is_some(),
        settings.scte35_input_settings.is_some(),
        settings.scte35_return_to_network_settings.is_some(),
        settings.scte35_splice_insert_settings.is_some(),
        settings.scte35_time_signal_settings.is_some(),
        settings.static_image_activate_settings.is_some(),
        settings.static_image_deactivate_settings.is_some(),
        settings.static_image_output_activate_settings.is_some(),
        settings.static_image_output_deactivate_settings.is_some(),
        settings.id3_segment_tagging_settings.is_some(),
        settings.timed_metadata_settings.is_some(),
    ]
    .into_iter()
    .filter(|selected| *selected)
    .count()
}

/// A fixed-mode start time, as its own member documents it: "UTC format:
/// yyyy-mm-ddThh:mm:ss.nnnZ. All the letters are digits (for example, mm might be
/// 01) except for the two constants "T" for time and "Z" for "UTC format"."
///
/// The calendar arithmetic is the LIBRARY's (`Format::DateTime` is RFC 3339, which
/// is that format), and the trailing `Z` is required here because RFC 3339 also
/// admits a numeric offset, which this member does not. `None` is a `Time` the
/// service cannot read, which is a validation failure rather than a stored value.
pub fn parse_action_time(time: &str) -> Option<SystemTime> {
    if !time.ends_with('Z') {
        return None;
    }
    let parsed = ::aws_smithy_types::DateTime::from_str(
        time,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(parsed).ok()
}
