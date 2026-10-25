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
//!
//! A created action is also validated against the CHANNEL it is being added to,
//! because several of its members do not describe the action at all: they name an
//! element of the channel's own configuration, which only the channel record
//! holds. [`ChannelScheduleScope`] is that record's side of the check, and the
//! members are documented as channel-scoped:
//!
//! * `InputSwitchScheduleActionSettings$InputAttachmentNameReference`: "The name
//!   of the input attachment (not the name of the input!) to switch to. The name
//!   is specified in the channel configuration." — and the names are
//!   `InputAttachment$InputAttachmentName`, "User-specified name for the
//!   attachment. This is required if the user wants to use this input in an input
//!   switch action." The same reference is carried by
//!   `InputPrepareScheduleActionSettings` and `Scte35InputScheduleActionSettings`
//!   ("the name of the input attachment that you want to use as a SCTE-35
//!   input").
//! * `StaticImageOutputActivateScheduleActionSettings$OutputNames` and its
//!   deactivate counterpart: "The name(s) of the output(s) the activation should
//!   apply to." — the outputs are `Output$OutputName`, "The name used to identify
//!   an output", declared in `EncoderSettings$OutputGroups`.
//! * `FeatureActivations$InputPrepareScheduleActions`: "You can create Input
//!   Prepare actions in the schedule only if this feature is enabled."
//! * `FeatureActivations$OutputStaticImageOverlayScheduleActions`: "Enabling this
//!   feature allows you to send channel schedule updates to display/clear/modify
//!   image overlays on an output-by-output bases."
//! * `EncoderSettings$MotionGraphicsConfiguration` is what a motion graphics
//!   activate action renders into ("Settings to specify the rendering of motion
//!   graphics into the video stream"), so a channel that declares no such
//!   configuration, or declares it `DISABLED`, renders none.
//! * `PipelinePauseStateSettings$PipelineId`: "Pipeline ID to pause ("PIPELINE_0"
//!   or "PIPELINE_1")" — of the channel, which has two pipelines only in the
//!   `STANDARD` class ("a single pipeline channel only has one").

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

/// The value a channel-configuration flag carries when the feature is on.
/// `FeatureActivationsInputPrepareScheduleActions`,
/// `FeatureActivationsOutputStaticImageOverlayScheduleActions` and
/// `MotionGraphicsInsertion` are all enums over `DISABLED` and `ENABLED`
/// (`smithy_query(query="shape", shape="MotionGraphicsInsertion")`), so one
/// spelling serves all three.
const FEATURE_ENABLED: &str = "ENABLED";

/// The same flags' off value, which is what a configuration present but turned
/// off carries.
const FEATURE_DISABLED: &str = "DISABLED";

/// What a created schedule action may reference IN THE CHANNEL it is being added
/// to: the names and the features the stored channel configuration declares.
///
/// Taken from [`super::ChannelRecord`] before anything is written, because the
/// schedule store holds only actions: an action's channel-scoped members ("The
/// name is specified in the channel configuration") are resolvable against the
/// channel record alone, and a reference the channel does not declare names
/// nothing the channel could ever execute.
#[derive(Debug, Clone, Default)]
pub struct ChannelScheduleScope {
    /// Every `InputAttachment$InputAttachmentName` the channel declares. An
    /// attachment the channel left unnamed is not in here: a name "is required if
    /// the user wants to use this input in an input switch action", so an unnamed
    /// attachment is not referenceable at all.
    attachment_names: BTreeSet<String>,
    /// Every `Output$OutputName` the channel's `EncoderSettings$OutputGroups`
    /// declare — the outputs an output-by-output static image action may name.
    output_names: BTreeSet<String>,
    /// The pipelines this channel's class gives it, as
    /// [`super::ChannelRecord::pipelines`] numbers them, so a pause action's
    /// target is resolved against the same rule every other reading uses.
    pipelines: BTreeSet<i32>,
    /// Whether `FeatureActivations$InputPrepareScheduleActions` is enabled.
    input_prepare_actions: bool,
    /// Whether `FeatureActivations$OutputStaticImageOverlayScheduleActions` is
    /// enabled.
    output_static_image_overlay_actions: bool,
    /// Whether the channel's `EncoderSettings` declare motion graphics the
    /// activate action could render into.
    motion_graphics: bool,
}

impl ChannelScheduleScope {
    /// The channel configuration a request's created actions are resolved
    /// against, read off the channel record that owns the schedule.
    pub fn of(channel: &super::ChannelRecord) -> Self {
        let encoder = channel.encoder_settings.as_ref();
        let features = encoder.and_then(|settings| settings.feature_activations.as_ref());
        Self {
            attachment_names: channel
                .input_attachments
                .iter()
                .filter_map(|attachment| attachment.input_attachment_name.clone())
                .collect(),
            output_names: encoder
                .map(|settings| settings.output_groups.as_slice())
                .unwrap_or_default()
                .iter()
                .flat_map(|group| group.outputs.iter())
                .filter_map(|output| output.output_name.clone())
                .collect(),
            // The channel's own pipelines, so `PIPELINE_1` names one of them
            // only in the two-pipeline class.
            pipelines: channel.pipelines().collect(),
            input_prepare_actions: features
                .and_then(|features| features.input_prepare_schedule_actions.as_deref())
                == Some(FEATURE_ENABLED),
            output_static_image_overlay_actions: features
                .and_then(|features| {
                    features.output_static_image_overlay_schedule_actions.as_deref()
                })
                == Some(FEATURE_ENABLED),
            // A configuration that is there and not turned off: the member is
            // what declares the feature on the channel, and its own flag can
            // turn it back off.
            motion_graphics: encoder
                .and_then(|settings| settings.motion_graphics_configuration.as_ref())
                .is_some_and(|configuration| {
                    configuration.motion_graphics_insertion.as_deref() != Some(FEATURE_DISABLED)
                }),
        }
    }

    /// The problem with `reference` as an input attachment name of this channel,
    /// if any: a reference "specified in the channel configuration" that the
    /// channel does not declare names no attachment the channel can switch to or
    /// prepare.
    fn unknown_attachment(&self, element_path: &str, reference: &str) -> Option<ValidationError> {
        (!self.attachment_names.contains(reference)).then(|| {
            problem(
                element_path,
                &format!(
                    "{reference} is not the name of an input attachment of this channel"
                ),
            )
        })
    }

    /// The problem with `reference` as an output name of this channel, if any.
    fn unknown_output(&self, element_path: &str, reference: &str) -> Option<ValidationError> {
        (!self.output_names.contains(reference)).then(|| {
            problem(
                element_path,
                &format!("{reference} is not the name of an output of this channel"),
            )
        })
    }

    /// The problem with `reference` as a pipeline of this channel, if any: the
    /// class fixes how many pipelines a channel has ("a single pipeline channel
    /// only has one"), so a pipeline it does not have names nothing that could
    /// pause.
    fn unknown_pipeline(&self, element_path: &str, reference: &str) -> Option<ValidationError> {
        let known = super::pipeline_index(reference)
            .is_some_and(|pipeline| self.pipelines.contains(&pipeline));
        (!known).then(|| {
            problem(
                element_path,
                &format!(
                    "{reference} is not a pipeline of this channel, which has {} pipeline(s)",
                    self.pipelines.len()
                ),
            )
        })
    }
}

/// The problem with an action type the channel has not declared the feature for,
/// if `available` says it has not. A free function, because the answer is in the
/// flag the caller already read off the scope.
fn unavailable_feature(
    available: bool,
    element_path: &str,
    feature: &str,
) -> Option<ValidationError> {
    (!available).then(|| {
        problem(
            element_path,
            &format!("this channel's configuration does not enable {feature}"),
        )
    })
}

/// Every problem the channel has with one created action's settings, in the order
/// `ScheduleActionSettings` declares its members. `settings_path` is the path of
/// the action's settings member, so each entry's `ElementPath` — "Path to the
/// source of the error" — names the channel-scoped member the request got wrong
/// rather than the action as a whole.
fn channel_problems(
    settings_path: &str,
    settings: &ScheduleActionSettings,
    scope: &ChannelScheduleScope,
) -> Vec<ValidationError> {
    let mut problems: Vec<ValidationError> = Vec::new();

    if let Some(prepare) = settings.input_prepare_settings.as_ref() {
        let path = format!("{settings_path}/inputPrepareSettings");
        problems.extend(unavailable_feature(
            scope.input_prepare_actions,
            &path,
            "input prepare schedule actions",
        ));
        // The reference is optional here, and absent means the action names no
        // attachment at all: "If no name is provided, the action will stop the
        // most recent prepare (if any) when activated."
        if let Some(reference) = prepare.input_attachment_name_reference.as_deref() {
            problems.extend(scope.unknown_attachment(
                &format!("{path}/inputAttachmentNameReference"),
                reference,
            ));
        }
    }

    if let Some(switch) = settings.input_switch_settings.as_ref() {
        problems.extend(scope.unknown_attachment(
            &format!("{settings_path}/inputSwitchSettings/inputAttachmentNameReference"),
            &switch.input_attachment_name_reference,
        ));
    }

    if settings.motion_graphics_image_activate_settings.is_some() {
        problems.extend(unavailable_feature(
            scope.motion_graphics,
            &format!("{settings_path}/motionGraphicsImageActivateSettings"),
            "motion graphics insertion",
        ));
    }

    if let Some(pause) = settings.pause_state_settings.as_ref() {
        for (index, pipeline) in pause.pipelines.as_deref().unwrap_or_default().iter().enumerate() {
            problems.extend(scope.unknown_pipeline(
                &format!("{settings_path}/pauseStateSettings/pipelines/{index}/pipelineId"),
                &pipeline.pipeline_id,
            ));
        }
    }

    if let Some(scte35) = settings.scte35_input_settings.as_ref() {
        // "In fixed mode, enter the name of the input attachment that you want to
        // use as a SCTE-35 input": a request that carries the reference is naming
        // an attachment, whatever mode it asked for.
        if let Some(reference) = scte35.input_attachment_name_reference.as_deref() {
            problems.extend(scope.unknown_attachment(
                &format!("{settings_path}/scte35InputSettings/inputAttachmentNameReference"),
                reference,
            ));
        }
    }

    // The two output-by-output static image actions are the ones
    // `OutputStaticImageOverlayScheduleActions` gates, and the ones that name the
    // channel's outputs; the layer-scoped `StaticImageActivateSettings` /
    // `StaticImageDeactivateSettings` carry no channel-scoped member, so nothing
    // about them is resolved against the channel.
    let output_actions: [(&str, Option<&Vec<String>>); 2] = [
        (
            "staticImageOutputActivateSettings",
            settings
                .static_image_output_activate_settings
                .as_ref()
                .map(|activate| &activate.output_names),
        ),
        (
            "staticImageOutputDeactivateSettings",
            settings
                .static_image_output_deactivate_settings
                .as_ref()
                .map(|deactivate| &deactivate.output_names),
        ),
    ];
    for (member, names) in output_actions {
        let Some(names) = names else {
            continue;
        };
        let path = format!("{settings_path}/{member}");
        problems.extend(unavailable_feature(
            scope.output_static_image_overlay_actions,
            &path,
            "output static image overlay schedule actions",
        ));
        for (index, name) in names.iter().enumerate() {
            problems.extend(scope.unknown_output(&format!("{path}/outputNames/{index}"), name));
        }
    }

    problems
}

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
    ///
    /// `scope` is the CHANNEL the schedule belongs to: a created action's
    /// channel-scoped members are resolved against it, so an action the channel
    /// could never execute is refused here instead of being stored.
    pub fn apply(
        &mut self,
        creates: &[ScheduleAction],
        deletes: &[String],
        scope: &ChannelScheduleScope,
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

            // What the action references in the channel it is being added to: a
            // reference the channel does not declare names nothing the channel
            // could execute, so the request is refused rather than stored.
            problems.extend(channel_problems(
                &format!("{path}/scheduleActionSettings"),
                &action.schedule_action_settings,
                scope,
            ));

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
