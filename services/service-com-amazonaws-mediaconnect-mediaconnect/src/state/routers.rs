// SPDX-License-Identifier: CC0-1.0

//! The router records this service stores — router inputs, router outputs and router
//! network interfaces — with the value sets and the validation their operations share.

use super::*;

/// `RouterInputType` (`smithy_query(query="shape",shape="RouterInputType")`: STANDARD |
/// FAILOVER | MERGE | MEDIACONNECT_FLOW | MEDIALIVE_CHANNEL) — the kind of a router input,
/// which is DERIVED from the `RouterInputConfiguration` the create carried rather than
/// asked for: the configuration is a union with one member per kind, and
/// `RouterInput$InputType` is reported for every input. [`router_input_type`] is the one
/// mapping.
pub const ROUTER_INPUT_TYPE_STANDARD: &str = "STANDARD";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_FAILOVER: &str = "FAILOVER";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MERGE: &str = "MERGE";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW: &str = "MEDIACONNECT_FLOW";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL: &str = "MEDIALIVE_CHANNEL";

/// The `RouterOutputState` enum the model declares for a router output
/// (`smithy_query(query="shape",shape="RouterOutputState")`: CREATING | STANDBY | STARTING
/// | ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING). Only the
/// states a router output's creation, start, stop, reconfiguration and deletion reach are
/// named here; the rest belong to operations that are still scaffolded.
pub const ROUTER_OUTPUT_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. A router output that finished being created is
/// not carrying content yet: it waits in `STANDBY` until StartRouterOutput — and it is
/// where a STOPPED output settles, because stopping an output is what returns it to not
/// carrying content (see [`ROUTER_OUTPUT_STATE_STOPPING`]).
pub const ROUTER_OUTPUT_STATE_STANDBY: &str = "STANDBY";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The state of a router output that is carrying
/// content — what StartRouterOutput settles an output into, and the state StopRouterOutput
/// takes it out of.
pub const ROUTER_OUTPUT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The non-terminal state a router output is
/// answered in while a start is in flight: StartRouterOutput accepts the start and the
/// clock settles the output into [`ROUTER_OUTPUT_STATE_ACTIVE`] one [`TRANSITION_SPAN`]
/// later, so a caller reading the output straight after the start still sees it starting.
pub const ROUTER_OUTPUT_STATE_STARTING: &str = "STARTING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The non-terminal state a router output is
/// answered in while a stop is in flight: StopRouterOutput accepts the stop and the clock
/// settles the output into [`ROUTER_OUTPUT_STATE_STANDBY`] one [`TRANSITION_SPAN`] later,
/// so a caller reading the output straight after the stop still sees it stopping.
pub const ROUTER_OUTPUT_STATE_STOPPING: &str = "STOPPING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The settled state of a router output that is not
/// carrying content because something is wrong with it. Nothing in this emulator PUTS an
/// output here (there is no data plane to fail), so it is reached only by a caller seeding
/// the store — the same way [`ROUTER_INPUT_STATE_ERROR`] is; a stop takes an output out of
/// it like any other settled state.
pub const ROUTER_OUTPUT_STATE_ERROR: &str = "ERROR";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The transient state an output is RECONFIGURED in:
/// UpdateRouterOutput answers `202 Accepted`, so the settings it accepted are not in effect
/// the instant it answers, and the clock settles the output back into the state it was
/// heading for one [`TRANSITION_SPAN`] later
/// ([`StoredRouterOutput::begin_transition`]).
pub const ROUTER_OUTPUT_STATE_UPDATING: &str = "UPDATING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The state a router output is in while its delete
/// is in progress — `DeleteRouterOutputResponse$State` is "the current state of the deleted
/// router output, indicating where it is in the deletion process".
///
/// `RouterOutputState` carries NO terminal deleted value, unlike `GatewayState`'s
/// `DELETED`, so a router output delete is a HARD delete: the record LEAVES the store once
/// the clock completes the deletion rather than staying readable in a deleted state. See
/// [`StoredRouterOutput::removal_due_at`].
pub const ROUTER_OUTPUT_STATE_DELETING: &str = "DELETING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The transient state of a router output that is
/// being RESTARTED: RestartRouterOutput "restarts a router output … to recover from errors
/// or refresh the output state", and `RECOVERING` is the one value of the enum that names
/// that work in progress.
pub const ROUTER_OUTPUT_STATE_RECOVERING: &str = "RECOVERING";

/// The `RouterOutputRoutedState` enum the model declares
/// (`smithy_query(query="shape",shape="RouterOutputRoutedState")`: ROUTED | ROUTING |
/// UNROUTED). A router output is ROUTED exactly while a router input is routed to it
/// (`RouterOutput$RoutedInputArn`), and UNROUTED otherwise — see
/// [`StoredRouterOutput::routed_state`].
pub const ROUTED_STATE_ROUTED: &str = "ROUTED";
/// See [`ROUTED_STATE_ROUTED`].
pub const ROUTED_STATE_UNROUTED: &str = "UNROUTED";
/// See [`ROUTED_STATE_ROUTED`]. The NON-TERMINAL routed state: TakeRouterInput answers
/// `202 Accepted`, so the association it recorded is not carrying content the instant it
/// answers, and the clock settles the output into the routed state the association owes it
/// one [`TRANSITION_SPAN`] later ([`StoredRouterOutput::begin_route`]).
pub const ROUTED_STATE_ROUTING: &str = "ROUTING";

/// The `RouterOutputType` enum the model declares
/// (`smithy_query(query="shape",shape="RouterOutputType")`: STANDARD | MEDIACONNECT_FLOW |
/// MEDIALIVE_INPUT). The type is not a request member: it is the variant of the output's
/// `RouterOutputConfiguration` union, reported through
/// [`StoredRouterOutput::output_type`].
pub const ROUTER_OUTPUT_TYPE_STANDARD: &str = "STANDARD";
/// See [`ROUTER_OUTPUT_TYPE_STANDARD`].
pub const ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW: &str = "MEDIACONNECT_FLOW";
/// See [`ROUTER_OUTPUT_TYPE_STANDARD`].
pub const ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT: &str = "MEDIALIVE_INPUT";

/// The `RouterNetworkInterfaceState` enum the model declares for a router network
/// interface (`smithy_query(query="shape",shape="RouterNetworkInterfaceState")`: CREATING |
/// ACTIVE | UPDATING | DELETING | ERROR | RECOVERING). Only the four a network interface's
/// creation, update and deletion reach are named here; the rest belong to operations that
/// are still scaffolded.
pub const ROUTER_NETWORK_INTERFACE_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. A network interface that finished being
/// created is usable: unlike a router input or output there is no start call for one, so
/// `ACTIVE` is where its creation settles.
pub const ROUTER_NETWORK_INTERFACE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. The non-terminal state a network
/// interface is answered in while an accepted UpdateRouterNetworkInterface is being applied
/// (that operation answers HTTP 202, `smithy_query(query="operation",
/// operation="UpdateRouterNetworkInterface")`); the clock settles it back into
/// [`ROUTER_NETWORK_INTERFACE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_NETWORK_INTERFACE_STATE_UPDATING: &str = "UPDATING";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. The non-terminal state an interface is
/// answered in while an accepted DeleteRouterNetworkInterface is being carried out; the
/// enum carries no terminal deleted value, so the clock completes the deletion by taking
/// the record out of the store (see
/// [`StoredRouterNetworkInterface::removal_due_at`]).
pub const ROUTER_NETWORK_INTERFACE_STATE_DELETING: &str = "DELETING";

/// The `RouterNetworkInterfaceType` enum the model declares
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceType")`: PUBLIC | VPC). Like a
/// router input's or output's type it is not a request member: it is the variant of the
/// interface's `RouterNetworkInterfaceConfiguration` union, reported through the one
/// mapping [`router_network_interface_type`].
pub const ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC: &str = "PUBLIC";
/// See [`ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC`].
pub const ROUTER_NETWORK_INTERFACE_TYPE_VPC: &str = "VPC";

/// `RouterNetworkInterface$NetworkInterfaceType` for an interface configured by
/// `configuration` — the variant of the configuration union, which is what the type is.
pub fn router_network_interface_type(
    configuration: &RouterNetworkInterfaceConfiguration,
) -> &'static str {
    match configuration {
        RouterNetworkInterfaceConfiguration::Public(_) => ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
        RouterNetworkInterfaceConfiguration::Vpc(_) => ROUTER_NETWORK_INTERFACE_TYPE_VPC,
    }
}

/// The `MaintenanceType` enum the model declares
/// (`smithy_query(query="shape",shape="MaintenanceType")`: PREFERRED_DAY_TIME | DEFAULT).
/// Like a router resource's type, it is not a request member: it is the variant of the
/// resource's `MaintenanceConfiguration` union, reported through the one mapping
/// [`maintenance_type`] (which [`StoredRouterOutput::maintenance_type`] also resolves
/// through).
pub const MAINTENANCE_TYPE_PREFERRED_DAY_TIME: &str = "PREFERRED_DAY_TIME";
/// See [`MAINTENANCE_TYPE_PREFERRED_DAY_TIME`].
pub const MAINTENANCE_TYPE_DEFAULT: &str = "DEFAULT";

/// `RouterContentQualityAnalysisType`
/// (`smithy_query(query="shape",shape="RouterContentQualityAnalysisType")`): the enum has
/// exactly ONE value, and `RouterInput$ContentQualityAnalysisType` is reported for every
/// router input, so this is the value every input reports.
pub const ROUTER_CQA_TYPE_CONTENT_LEVEL: &str = "CONTENT_LEVEL";

/// The `RouterInputType` of an input configured by `configuration`.
///
/// `RouterInputConfiguration` is a union with one member per input kind
/// (`smithy_query(query="shape",shape="RouterInputConfiguration")`) and `RouterInputType`
/// names exactly those kinds, so the type an input reports is a function of the
/// configuration it was created with — never a separate request member. Every projection
/// of a stored router input resolves it HERE.
pub fn router_input_type(configuration: &RouterInputConfiguration) -> &'static str {
    match configuration {
        RouterInputConfiguration::Standard(_) => ROUTER_INPUT_TYPE_STANDARD,
        RouterInputConfiguration::Failover(_) => ROUTER_INPUT_TYPE_FAILOVER,
        RouterInputConfiguration::Merge(_) => ROUTER_INPUT_TYPE_MERGE,
        RouterInputConfiguration::MediaConnectFlow(_) => ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW,
        RouterInputConfiguration::MediaLiveChannel(_) => ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL,
    }
}

/// The `MaintenanceType` of a resource whose maintenance is configured by
/// `configuration` — the union member it carries (`PreferredDayTime` or `Default`), which
/// is what the `MaintenanceType` enum names.
pub fn maintenance_type(configuration: &MaintenanceConfiguration) -> &'static str {
    match configuration {
        MaintenanceConfiguration::PreferredDayTime(_) => MAINTENANCE_TYPE_PREFERRED_DAY_TIME,
        MaintenanceConfiguration::Default(_) => MAINTENANCE_TYPE_DEFAULT,
    }
}

/// The `MaintenanceScheduleType` enum the model declares
/// (`smithy_query(query="shape",shape="MaintenanceScheduleType")`: WINDOW) — its only
/// value, and `MaintenanceSchedule` is a union with exactly one member (`Window`), so a
/// resource that has a maintenance schedule at all reports this type for it.
pub const MAINTENANCE_SCHEDULE_TYPE_WINDOW: &str = "WINDOW";

/// Seconds in a minute, in an hour and in a day — the units a maintenance preference
/// states its time of day in, and the step the next occurrence of one is found over.
pub(super) const SECONDS_PER_MINUTE: u64 = 60;
/// See [`SECONDS_PER_MINUTE`].
pub(super) const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
/// See [`SECONDS_PER_MINUTE`].
pub(super) const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;
/// Days in a week — how far apart two occurrences of one preferred weekday are.
pub(super) const DAYS_PER_WEEK: u64 = 7;

/// The Monday-first index of the epoch day, 1970-01-01, which was a Thursday.
///
/// The anchor every weekday this module derives from an epoch second is counted from, so the
/// weekday a window OPENS on ([`maintenance_window_matches_configuration`]) and the weekday
/// the next occurrence of a preference falls on ([`next_time_of_day_occurrence`]) cannot
/// disagree.
pub(super) const EPOCH_DAY_OF_WEEK: u64 = 3;

/// The time of day a maintenance window opens for a resource whose maintenance
/// configuration names no preferred time.
///
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format. Minutes must be
/// 00. Example: 13:00. The default value is 02:00."
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`) —
/// the default maintenance hour this service documents for a FLOW. AWS documents no default
/// hour for a router resource, so that one is carried over rather than a second hour being
/// invented: it is the hour this service states it maintains a resource at when nothing was
/// asked for.
pub const DEFAULT_MAINTENANCE_TIME_OF_DAY: Duration = Duration::from_secs(2 * SECONDS_PER_HOUR);

/// How long a maintenance window stays open: the distance between
/// `WindowMaintenanceSchedule$Start` and `WindowMaintenanceSchedule$End`.
///
/// AWS publishes no length for the window, so the span is this emulator's own choice. It is
/// non-zero, so the window a schedule reports always contains its `ScheduledTime`.
pub const MAINTENANCE_WINDOW_SPAN: Duration = Duration::from_secs(SECONDS_PER_HOUR);

/// The day of the week `day` names, for a value of the `Day` enum the model declares
/// (`smithy_query(query="shape",shape="Day")`: MONDAY | TUESDAY | WEDNESDAY | THURSDAY |
/// FRIDAY | SATURDAY | SUNDAY), or `None` for a string that is not one of them.
///
/// The enum's values are matched exactly, so no other spelling of a weekday is read as a
/// preference. A string this does not know leaves the schedule with no preferred day (see
/// [`maintenance_window_schedule`]), which keeps the derivation total; it is not reachable
/// through the API, because the member targets the enum and the boundary refuses every other
/// value.
pub fn maintenance_day_of_week(day: &str) -> Option<u64> {
    let index = match day {
        "MONDAY" => 0,
        "TUESDAY" => 1,
        "WEDNESDAY" => 2,
        "THURSDAY" => 3,
        "FRIDAY" => 4,
        "SATURDAY" => 5,
        "SUNDAY" => 6,
        _ => return None,
    };
    Some(index)
}

/// The distance from midnight UTC that the maintenance time `time` names, or `None` for a
/// string that is not a time of day.
///
/// `PreferredDayTimeMaintenanceConfiguration$Time` is "the preferred time for maintenance
/// operations", stated in the 24-hour `HH:MM` form this service documents for the same
/// preference on a flow
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`:
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format"). A seconds field
/// is accepted after the minutes because that is the same time of day written out in full.
pub fn maintenance_time_of_day(time: &str) -> Option<Duration> {
    /// One field of the time, at most two digits and at most `ceiling`.
    fn field(value: &str, ceiling: u64) -> Option<u64> {
        if value.is_empty() || value.len() > 2 || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        value.parse::<u64>().ok().filter(|parsed| *parsed <= ceiling)
    }

    let mut fields = time.split(':');
    let hours = field(fields.next()?, 23)?;
    let minutes = field(fields.next()?, 59)?;
    let seconds = match fields.next() {
        Some(seconds) => field(seconds, 59)?,
        None => 0,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(Duration::from_secs(
        hours * SECONDS_PER_HOUR + minutes * SECONDS_PER_MINUTE + seconds,
    ))
}

/// Why `configuration` is not a maintenance configuration this service can honour, or
/// `None` when it is one.
///
/// A `PreferredDayTime` preference states WHEN the resource is maintained and the window
/// every start schedules is derived from it ([`maintenance_window_schedule`]), so a `Time`
/// that names no time of day names no preference either: the operation that accepts one
/// refuses it rather than storing a preference no window can be scheduled inside. The
/// `Default` member states no time at all, so there is nothing to refuse in it. Every
/// operation that takes a `MaintenanceConfiguration` (CreateRouterInput,
/// CreateRouterOutput, UpdateRouterOutput) judges it HERE, so one unacceptable preference
/// is refused with one sentence.
pub fn maintenance_configuration_rejection(
    configuration: &MaintenanceConfiguration,
) -> Option<String> {
    match configuration {
        MaintenanceConfiguration::PreferredDayTime(preference) => {
            maintenance_time_of_day(&preference.time).is_none().then(|| {
                format!(
                    "MaintenanceConfiguration PreferredDayTime Time {:?} is not a time of \
                     day. Use 24-hour HH:MM format, for example 02:00.",
                    preference.time
                )
            })
        }
        MaintenanceConfiguration::Default(_) => None,
    }
}

/// The maintenance window a resource configured by `configuration` is next maintained in,
/// as of `from`.
///
/// The schedule is DERIVED from the resource's maintenance configuration and the instant it
/// was scheduled at, never carried by a request: a configuration that states a preferred day
/// and time is scheduled at the next occurrence of that weekday at that UTC time, and the
/// `Default` configuration, which states no day, at the next occurrence of
/// [`DEFAULT_MAINTENANCE_TIME_OF_DAY`]. The window opens there, lasts
/// [`MAINTENANCE_WINDOW_SPAN`], and the maintenance is scheduled for the instant it opens,
/// which is inside it.
///
/// The `Time` of a preference this cannot read is the documented default hour, which keeps
/// this a total function: such a preference does not reach the store, because the operation
/// that takes one refuses it as a bad request (see CreateRouterInput).
pub fn maintenance_window_schedule(
    configuration: &MaintenanceConfiguration,
    from: SystemTime,
) -> MaintenanceSchedule {
    let (preferred_day, time_of_day) = maintenance_preference(configuration);
    let start = next_time_of_day_occurrence(preferred_day, time_of_day, from);
    MaintenanceSchedule::Window(WindowMaintenanceSchedule {
        start: ::aws_smithy_types::DateTime::from(start),
        end: ::aws_smithy_types::DateTime::from(start + MAINTENANCE_WINDOW_SPAN),
        scheduled_time: ::aws_smithy_types::DateTime::from(start),
    })
}

/// The preference `configuration` states, as the weekday a window opens on (`None` for a
/// configuration that prefers no day, so every day qualifies) and its time of day past
/// midnight UTC.
///
/// The ONE reading of a maintenance configuration, so the window a schedule is derived at
/// ([`maintenance_window_schedule`]) and the test of whether a window still belongs to a
/// preference ([`maintenance_window_matches_configuration`]) cannot read one differently.
pub(super) fn maintenance_preference(configuration: &MaintenanceConfiguration) -> (Option<u64>, Duration) {
    match configuration {
        MaintenanceConfiguration::PreferredDayTime(preference) => (
            maintenance_day_of_week(&preference.day),
            maintenance_time_of_day(&preference.time).unwrap_or(DEFAULT_MAINTENANCE_TIME_OF_DAY),
        ),
        MaintenanceConfiguration::Default(_) => (None, DEFAULT_MAINTENANCE_TIME_OF_DAY),
    }
}

/// Whether the window `schedule` reports is an occurrence of the preference `configuration`
/// states: it opens on a weekday that configuration prefers, at the time of day it prefers.
///
/// A window is DERIVED from a maintenance configuration ([`maintenance_window_schedule`]),
/// so this is what tells a window that still belongs to the preference a resource carries
/// from one derived from a preference it no longer carries (see
/// [`apply_maintenance_configuration`]). Only the instant the window OPENS is read: its end
/// and its scheduled time follow from that instant.
pub(super) fn maintenance_window_matches_configuration(
    schedule: &MaintenanceSchedule,
    configuration: &MaintenanceConfiguration,
) -> bool {
    let MaintenanceSchedule::Window(window) = schedule;
    let Ok(start) = u64::try_from(window.start.secs()) else {
        return false;
    };
    let (preferred_day, time_of_day) = maintenance_preference(configuration);
    if start % SECONDS_PER_DAY != time_of_day.as_secs() {
        return false;
    }
    preferred_day
        .is_none_or(|day| (start / SECONDS_PER_DAY + EPOCH_DAY_OF_WEEK) % DAYS_PER_WEEK == day)
}

/// Store `configuration` as the maintenance preference of the resource whose derived window
/// is `stored_schedule`, re-deriving a window that the new preference no longer accounts
/// for.
///
/// A maintenance window is never carried by a request: it is DERIVED from the resource's
/// maintenance configuration ([`maintenance_window_schedule`]), so it belongs to the
/// preference it was derived from. Once that preference is replaced, a window that is not an
/// occurrence of the preference the resource now carries is spent — the resource is next
/// maintained at the next occurrence of the NEW preference, which is the window every later
/// read reports. A resource nothing has scheduled a window for keeps none: the start is the
/// call that schedules the first one. This is the rule the crate already applies to the
/// derived maintenance value on the flow side, where a rescheduled window drops the deadline
/// that was issued for the old one (see `crate::ops::update_flow`).
pub fn apply_maintenance_configuration(
    stored_configuration: &mut MaintenanceConfiguration,
    stored_schedule: &mut Option<MaintenanceSchedule>,
    configuration: MaintenanceConfiguration,
    now: SystemTime,
) {
    *stored_configuration = configuration;
    if stored_schedule.as_ref().is_some_and(|schedule| {
        !maintenance_window_matches_configuration(schedule, stored_configuration)
    }) {
        *stored_schedule = Some(maintenance_window_schedule(stored_configuration, now));
    }
}

/// Whether the maintenance window `schedule` reports has closed by `now`.
///
/// `MaintenanceSchedule` is a union with one member, so the window is always the one to
/// read; a closed window is spent, which is what makes the next occurrence of the
/// preference due (see [`StoredRouterInput::settle`]).
pub(super) fn maintenance_window_has_closed(schedule: &MaintenanceSchedule, now: SystemTime) -> bool {
    let MaintenanceSchedule::Window(window) = schedule;
    let now_seconds = i64::try_from(
        now.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX);
    now_seconds >= window.end.secs()
}

/// The first instant strictly after `from` that falls at `time_of_day` past midnight UTC on
/// the weekday `day_of_week` (a Monday-first index as [`maintenance_day_of_week`] gives it,
/// or `None` for any day).
///
/// A UTC instant's weekday and time of day are a function of its epoch second: UTC keeps no
/// daylight saving, and neither the day boundary nor the week depends on the calendar's
/// months, so no month or leap rule takes part in this derivation — only the weekday the
/// epoch day itself fell on, which [`EPOCH_DAY_OF_WEEK`] anchors. A preference that has
/// already passed comes round again a week later when it names a day, and the next day when
/// it does not.
pub(super) fn next_time_of_day_occurrence(
    day_of_week: Option<u64>,
    time_of_day: Duration,
    from: SystemTime,
) -> SystemTime {
    let since_epoch = from
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let midnight = since_epoch - since_epoch % SECONDS_PER_DAY;
    let (days_ahead, step_days) = match day_of_week {
        Some(day_of_week) => {
            let today = (since_epoch / SECONDS_PER_DAY + EPOCH_DAY_OF_WEEK) % DAYS_PER_WEEK;
            (
                (day_of_week % DAYS_PER_WEEK + DAYS_PER_WEEK - today) % DAYS_PER_WEEK,
                DAYS_PER_WEEK,
            )
        }
        // No preferred day: the window comes round every day, so the next one is today's if
        // it has not passed and tomorrow's otherwise.
        None => (0, 1),
    };
    let candidate = midnight + days_ahead * SECONDS_PER_DAY + time_of_day.as_secs();
    let start = if candidate > since_epoch {
        candidate
    } else {
        // The preference has already passed: the next occurrence is one step on.
        candidate + step_days * SECONDS_PER_DAY
    };
    SystemTime::UNIX_EPOCH + Duration::from_secs(start)
}

/// The Mbps steps a router tier may name, in the order the documentation publishes them.
///
/// Both tier enums declare the SAME ladder, each name prefixed by the side of the router it
/// configures: `RouterInputTier` (`smithy_query(query="shape",shape="RouterInputTier")`:
/// INPUT_20 | INPUT_50 | INPUT_100 | INPUT_250 | INPUT_500) and `RouterOutputTier`
/// (`smithy_query(query="shape",shape="RouterOutputTier")`: OUTPUT_20 | OUTPUT_50 |
/// OUTPUT_100 | OUTPUT_250 | OUTPUT_500), with the Mbps of each spelled out by the tier's
/// own documentation ("INPUT_20 – 20 Mbps, INPUT_50 – 50 Mbps, INPUT_100 – 100 Mbps,
/// INPUT_250 – 250 Mbps, INPUT_500 – 500 Mbps",
/// `smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")`, and the same
/// numbers under `smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`).
/// So the ladder is written ONCE and read through [`RouterTierLadder`].
pub(super) const ROUTER_TIER_MEGABITS_PER_SECOND: [i64; 5] = [20, 50, 100, 250, 500];

/// One side of the router's tier catalog: the tier names it declares and what a refusal
/// calls the resource those tiers configure.
///
/// The tier "sets the maximum bitrate, in Mbps" for the resource
/// (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")` and
/// `smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`, which
/// `UpdateRouterInput$Tier` and `UpdateRouterOutput$Tier` repeat), and that is ONE rule:
/// the tier a resource ends up with resolves to a ceiling on [`ROUTER_TIER_MEGABITS_PER_SECOND`],
/// and the bitrate it is asked for must be positive and at most that ceiling. The two sides
/// differ only in the prefix their tier names carry and in the noun a refusal names, so both
/// are this one type: [`ROUTER_INPUT_TIERS`] and [`ROUTER_OUTPUT_TIERS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterTierLadder {
    /// The prefix every tier name of this side carries, before the `_` and the Mbps step.
    pub(super) prefix: &'static str,
    /// What this side's resource is called in the sentence a refusal carries.
    pub(super) noun: &'static str,
}

/// The router INPUT side of [`RouterTierLadder`] — the catalog `CreateRouterInput$Tier` and
/// `UpdateRouterInput$Tier` name a tier from.
pub const ROUTER_INPUT_TIERS: RouterTierLadder = RouterTierLadder {
    prefix: "INPUT",
    noun: "router input",
};

/// The router OUTPUT side of [`RouterTierLadder`] — the catalog `CreateRouterOutput$Tier`
/// and `UpdateRouterOutput$Tier` name a tier from.
pub const ROUTER_OUTPUT_TIERS: RouterTierLadder = RouterTierLadder {
    prefix: "OUTPUT",
    noun: "router output",
};

impl RouterTierLadder {
    /// The maximum bitrate, in BITS PER SECOND, that `tier` allows a resource of this side,
    /// or `None` when `tier` is not one this side declares.
    ///
    /// The published numbers are in Mbps and MediaConnect states a bitrate in bits per
    /// second, so each step is converted through [`BITS_PER_MEGABIT`].
    pub fn bitrate_ceiling(&self, tier: &str) -> Option<i64> {
        let step = tier.strip_prefix(self.prefix)?.strip_prefix('_')?;
        ROUTER_TIER_MEGABITS_PER_SECOND
            .into_iter()
            .find(|megabits_per_second| megabits_per_second.to_string() == step)
            .map(|megabits_per_second| megabits_per_second * BITS_PER_MEGABIT)
    }

    /// Why a resource of this side at tier `tier` may not carry a `MaximumBitrate` of
    /// `maximum_bitrate`, or `None` when that pair is acceptable.
    ///
    /// The tier a resource ENDS UP with bounds the bitrate it may be asked for, through the
    /// one catalog [`Self::bitrate_ceiling`]; and a bitrate is a count of bits per second, so
    /// it is positive. Every operation that sets either member (CreateRouterInput,
    /// UpdateRouterInput, CreateRouterOutput, UpdateRouterOutput) judges the pair HERE, so
    /// the bound cannot drift between them or between the two sides. The detail is the
    /// sentence the refusal's envelope carries ([`validation_failed_message`]).
    pub fn bitrate_rejection(&self, tier: &str, maximum_bitrate: i64) -> Option<String> {
        let Some(ceiling) = self.bitrate_ceiling(tier) else {
            let noun = self.noun;
            return Some(format!(
                "Tier {tier:?} is not a {noun} tier this service offers."
            ));
        };
        if maximum_bitrate <= 0 {
            return Some(format!(
                "MaximumBitrate is a number of bits per second and must be positive; this \
                 request asks for {maximum_bitrate}."
            ));
        }
        if maximum_bitrate > ceiling {
            return Some(format!(
                "MaximumBitrate {maximum_bitrate} bps exceeds the {ceiling} bps the {tier} tier \
                 allows."
            ));
        }
        None
    }
}

/// The maximum bitrate, in BITS PER SECOND, a router input of `tier` may carry, or `None`
/// for a tier this service does not offer — [`ROUTER_INPUT_TIERS`]' own ceiling.
pub fn router_input_tier_bitrate_ceiling(tier: &str) -> Option<i64> {
    ROUTER_INPUT_TIERS.bitrate_ceiling(tier)
}

/// The `RouterInputTransitEncryption` an input reports when the create named none.
///
/// `RouterInput$TransitEncryption` is a REQUIRED member while
/// `CreateRouterInputRequest$TransitEncryption` is optional, so the service supplies a
/// value for an input whose create omitted it: the key configuration union's `Automatic`
/// member — "automatic" key management, which needs no secret and no role from the caller
/// — under the matching `AUTOMATIC` key type.
pub fn default_router_input_transit_encryption() -> RouterInputTransitEncryption {
    RouterInputTransitEncryption {
        encryption_key_type: Some(ROUTER_INPUT_TRANSIT_ENCRYPTION_KEY_TYPE_AUTOMATIC.to_string()),
        encryption_key_configuration: RouterInputTransitEncryptionKeyConfiguration::Automatic(
            AutomaticEncryptionKeyConfiguration {},
        ),
    }
}

/// `RouterInputTransitEncryptionKeyType`
/// (`smithy_query(query="shape",shape="RouterInputTransitEncryptionKeyType")`:
/// SECRETS_MANAGER | AUTOMATIC) — the key type of the transit encryption
/// [`default_router_input_transit_encryption`] supplies.
pub const ROUTER_INPUT_TRANSIT_ENCRYPTION_KEY_TYPE_AUTOMATIC: &str = "AUTOMATIC";

/// The `RouterContentQualityAnalysisConfiguration` an input reports when the create named
/// none.
///
/// `RouterInput$ContentQualityAnalysisConfiguration` is reported for every input while
/// `CreateRouterInputRequest$ContentQualityAnalysisConfiguration` is optional, and the
/// union has a single member (`ContentLevel`), so an input whose create configured no
/// content quality analysis reports that member with none of its three features
/// (`BlackFrames`, `FrozenFrames`, `SilentAudio`) configured — each is optional, and an
/// absent one is a feature nothing analyses.
pub fn default_router_content_quality_analysis_configuration()
-> RouterContentQualityAnalysisConfiguration {
    RouterContentQualityAnalysisConfiguration::ContentLevel(
        ContentQualityAnalysisFeatureConfiguration {
            black_frames: None,
            frozen_frames: None,
            silent_audio: None,
        },
    )
}

/// The `RouterInputProtocol` one entry of a router input's protocol configuration
/// configures.
///
/// Each protocol-configuration union on the INPUT side carries one variant per protocol it
/// admits — `RouterInputProtocolConfiguration` on a STANDARD configuration,
/// `FailoverRouterInputProtocolConfiguration` and `MergeRouterInputProtocolConfiguration`
/// on the two that configure a pair of sources — so the protocol a configuration uses IS
/// its variant, named by the `RouterInputProtocol` enum
/// (`smithy_query(query="shape",shape="RouterInputProtocol")`: RTP | RIST | SRT_CALLER |
/// SRT_LISTENER). This trait is the crate's ONE mapping between the two on this side of the
/// router — the counterpart of [`router_output_protocol`] — so a STANDARD create derives
/// the `Protocol` it reports from it, and the pair rule
/// ([`router_input_source_pair_rejection`]) compares two entries' protocol types through it.
pub trait RouterInputProtocolVariant {
    /// The `RouterInputProtocol` value this protocol configuration configures.
    fn protocol(&self) -> &'static str;
}

impl RouterInputProtocolVariant for RouterInputProtocolConfiguration {
    fn protocol(&self) -> &'static str {
        match self {
            Self::Rtp(_) => "RTP",
            Self::Rist(_) => "RIST",
            Self::SrtCaller(_) => "SRT_CALLER",
            Self::SrtListener(_) => "SRT_LISTENER",
        }
    }
}

impl RouterInputProtocolVariant for FailoverRouterInputProtocolConfiguration {
    fn protocol(&self) -> &'static str {
        match self {
            Self::Rtp(_) => "RTP",
            Self::Rist(_) => "RIST",
            Self::SrtCaller(_) => "SRT_CALLER",
            Self::SrtListener(_) => "SRT_LISTENER",
        }
    }
}

impl RouterInputProtocolVariant for MergeRouterInputProtocolConfiguration {
    fn protocol(&self) -> &'static str {
        match self {
            Self::Rtp(_) => "RTP",
            Self::Rist(_) => "RIST",
        }
    }
}

/// How many protocol configurations a router input that ingests a PAIR of sources lists.
///
/// "A list of exactly two protocol configurations for the failover input sources"
/// (`smithy_query(query="shape",shape="FailoverRouterInputConfiguration")`, member
/// `ProtocolConfigurations`), and the same sentence for the merge input sources on
/// `MergeRouterInputConfiguration$ProtocolConfigurations`. Neither list shape carries a
/// length constraint, so the count is checked here instead of at the boundary — and it is
/// the count every projection of such an input relies on
/// ([`initial_router_input_stream_details`] reports one entry per listed source).
pub const ROUTER_INPUT_SOURCE_PAIR_LEN: usize = 2;

/// Why the protocol configurations `configurations`, listed by the member `member`, cannot
/// configure a router input's pair of sources, or `None` when they can.
///
/// The two configurations that ingest a PAIR of sources document the same two rules for
/// their list: "a list of exactly two protocol configurations … Both must use the same
/// protocol type" — so a list of any other length names no pair at all, and two entries of
/// different variants ([`RouterInputProtocolVariant`]) name no single protocol type. Named
/// ONCE here, generic over the entry union, so the failover and the merge list are judged
/// by the one rule rather than by two spellings of it.
pub fn router_input_source_pair_rejection<Protocol: RouterInputProtocolVariant>(
    member: &str,
    configurations: &[Protocol],
) -> Option<String> {
    if configurations.len() != ROUTER_INPUT_SOURCE_PAIR_LEN {
        return Some(format!(
            "{member} must list exactly {ROUTER_INPUT_SOURCE_PAIR_LEN} protocol \
             configurations, but it lists {}.",
            configurations.len()
        ));
    }
    let mut protocols = configurations.iter().map(RouterInputProtocolVariant::protocol);
    let first = protocols.next()?;
    let differing = protocols.find(|protocol| *protocol != first)?;
    Some(format!(
        "{member} must list protocol configurations that all use the same protocol type, \
         but it lists a {first} and a {differing} configuration."
    ))
}

/// The `RouterInputConfiguration` a request's configuration is STORED as, or the detail of
/// why it cannot be stored as one.
///
/// A STANDARD input's `Protocol` is implied by the variant of its required
/// `ProtocolConfiguration` ([`RouterInputProtocolVariant`]), so the stored configuration
/// reports the protocol that configuration configures, and a request whose `Protocol` names
/// a different one is refused rather than stored self-contradicting. A FAILOVER or MERGE
/// input ingests a pair of sources, so its list of protocol configurations must be a pair of
/// one protocol type ([`router_input_source_pair_rejection`]). The two configurations that
/// take their content from another AWS resource carry no protocol member and no source list
/// to reconcile, so each is stored as it arrived. Every operation that takes a configuration
/// (CreateRouterInput, UpdateRouterInput) normalises it HERE, so no second spelling can
/// drift from this one — the input-side counterpart of
/// [`normalized_router_output_configuration`].
pub fn normalized_router_input_configuration(
    configuration: RouterInputConfiguration,
) -> Result<RouterInputConfiguration, String> {
    match configuration {
        RouterInputConfiguration::Standard(standard) => {
            let configured = standard.protocol_configuration.protocol();
            if let Some(named) = standard
                .protocol
                .as_deref()
                .filter(|protocol| !protocol.is_empty())
            {
                if named != configured {
                    return Err(format!(
                        "Configuration sets a {configured} protocol configuration, but \
                         Protocol names {named}."
                    ));
                }
            }
            Ok(RouterInputConfiguration::Standard(
                StandardRouterInputConfiguration {
                    protocol: Some(configured.to_string()),
                    ..standard
                },
            ))
        }
        RouterInputConfiguration::Failover(failover) => {
            match router_input_source_pair_rejection(
                "Configuration Failover ProtocolConfigurations",
                &failover.protocol_configurations,
            ) {
                Some(rejection) => Err(rejection),
                None => Ok(RouterInputConfiguration::Failover(failover)),
            }
        }
        RouterInputConfiguration::Merge(merge) => {
            match router_input_source_pair_rejection(
                "Configuration Merge ProtocolConfigurations",
                &merge.protocol_configurations,
            ) {
                Some(rejection) => Err(rejection),
                None => Ok(RouterInputConfiguration::Merge(merge)),
            }
        }
        other => Ok(other),
    }
}

/// The `RouterInputStreamDetails` of an input configured by `configuration` that is not
/// carrying a stream.
///
/// `RouterInputStreamDetails` is a union whose members are the same five input kinds as
/// [`router_input_type`]'s, so which member an input reports follows from its
/// configuration. Every per-source address inside it stays ABSENT: the emulator serves no
/// media data plane, so no source address has been observed on an input — a failover and a
/// merge input still report both of their indexed entries, because the model makes those
/// two members required, and the index is the position the configuration lists the source
/// at, which is a position every stored configuration of either kind has
/// ([`ROUTER_INPUT_SOURCE_PAIR_LEN`]).
pub fn initial_router_input_stream_details(
    configuration: &RouterInputConfiguration,
) -> RouterInputStreamDetails {
    match configuration {
        RouterInputConfiguration::Standard(_) => {
            RouterInputStreamDetails::Standard(StandardRouterInputStreamDetails {
                source_ip_address: None,
            })
        }
        RouterInputConfiguration::Failover(_) => {
            RouterInputStreamDetails::Failover(FailoverRouterInputStreamDetails {
                source_index_zero_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 0,
                    source_ip_address: None,
                },
                source_index_one_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 1,
                    source_ip_address: None,
                },
            })
        }
        RouterInputConfiguration::Merge(_) => {
            RouterInputStreamDetails::Merge(MergeRouterInputStreamDetails {
                source_index_zero_stream_details: MergeRouterInputIndexedStreamDetails {
                    source_index: 0,
                    source_ip_address: None,
                },
                source_index_one_stream_details: MergeRouterInputIndexedStreamDetails {
                    source_index: 1,
                    source_ip_address: None,
                },
            })
        }
        RouterInputConfiguration::MediaConnectFlow(_) => {
            RouterInputStreamDetails::MediaConnectFlow(MediaConnectFlowRouterInputStreamDetails {})
        }
        RouterInputConfiguration::MediaLiveChannel(_) => {
            RouterInputStreamDetails::MediaLiveChannel(MediaLiveChannelRouterInputStreamDetails {})
        }
    }
}

/// The ROUTER NETWORK INTERFACE ARN an input configured by `configuration` ingests on
/// (`StandardRouterInputConfiguration$NetworkInterfaceArn` and the same member on the
/// FAILOVER and MERGE configurations), or `None` for a configuration that takes its content
/// from another AWS resource instead and so names no interface.
///
/// The crate's ONE reading of that relation on the input side: it is what
/// `RouterNetworkInterface$AssociatedInputCount` counts
/// ([`ComAmazonawsMediaconnectMediaconnectState::associated_router_input_count`]) and what
/// [`router_input_has_own_address`] answers from.
pub fn router_input_network_interface_arn(
    configuration: &RouterInputConfiguration,
) -> Option<&str> {
    match configuration {
        RouterInputConfiguration::Standard(standard) => {
            Some(standard.network_interface_arn.as_str())
        }
        RouterInputConfiguration::Failover(failover) => {
            Some(failover.network_interface_arn.as_str())
        }
        RouterInputConfiguration::Merge(merge) => Some(merge.network_interface_arn.as_str()),
        RouterInputConfiguration::MediaConnectFlow(_)
        | RouterInputConfiguration::MediaLiveChannel(_) => None,
    }
}

/// The ROUTER NETWORK INTERFACE ARN an output configured by `configuration` egresses from
/// (`StandardRouterOutputConfiguration$NetworkInterfaceArn`), or `None` for a configuration
/// that delivers into another AWS resource and so names no interface.
///
/// The output-side counterpart of [`router_input_network_interface_arn`], and what
/// `RouterNetworkInterface$AssociatedOutputCount` counts
/// ([`ComAmazonawsMediaconnectMediaconnectState::associated_router_output_count`]).
pub fn router_output_network_interface_arn(
    configuration: &RouterOutputConfiguration,
) -> Option<&str> {
    match configuration {
        RouterOutputConfiguration::Standard(standard) => {
            Some(standard.network_interface_arn.as_str())
        }
        RouterOutputConfiguration::MediaConnectFlow(_)
        | RouterOutputConfiguration::MediaLiveInput(_) => None,
    }
}

/// Whether an input configured by `configuration` ingests on a ROUTER NETWORK INTERFACE of
/// this account — the configurations that carry a `NetworkInterfaceArn`.
///
/// Such an input is reachable at an address of its own, which is what
/// `RouterInput$IpAddress` ("the IP address of the router input") reports; an input that
/// takes its content from another AWS resource instead (a MediaConnect flow output, a
/// MediaLive channel output) has no address of its own, so the optional member stays
/// absent for it rather than carrying an address that addresses nothing.
pub fn router_input_has_own_address(configuration: &RouterInputConfiguration) -> bool {
    router_input_network_interface_arn(configuration).is_some()
}

/// The SOURCE an input configured by `configuration` is currently CONNECTED to, named as
/// the kind of resource it is, or `None` for a configuration that is connected to no such
/// source.
///
/// Two router input configurations take their content from a resource of another AWS
/// service, and each documents what connects it: "to connect the router input to the flow
/// output, provide both `flowArn` and `flowOutputArn`. To disconnect it, call the update
/// operation and omit both fields while keeping this configuration object"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`), and
/// the same for all three of `mediaLiveChannelArn`, `mediaLivePipelineId` and
/// `mediaLiveChannelOutputName`
/// (`smithy_query(query="shape",shape="MediaLiveChannelRouterInputConfiguration")`). The
/// three configurations that ingest on a network interface of this account name no such
/// source, so none of them is connected to one.
pub fn router_input_connected_source(
    configuration: &RouterInputConfiguration,
) -> Option<&'static str> {
    match configuration {
        RouterInputConfiguration::MediaConnectFlow(flow) => {
            (flow.flow_arn.is_some() && flow.flow_output_arn.is_some()).then_some("a flow output")
        }
        RouterInputConfiguration::MediaLiveChannel(channel) => (channel
            .media_live_channel_arn
            .is_some()
            && channel.media_live_pipeline_id.is_some()
            && channel.media_live_channel_output_name.is_some())
        .then_some("a MediaLive channel output"),
        RouterInputConfiguration::Standard(_)
        | RouterInputConfiguration::Failover(_)
        | RouterInputConfiguration::Merge(_) => None,
    }
}

/// The values that tell two transit encryption configurations apart: the key type, and the
/// secret and the role a Secrets Manager key is read with (an automatic key is managed by
/// the service and carries nothing else to compare).
///
/// One reading for both directions of a router: a router input's `sourceTransitDecryption`
/// and a router output's `destinationTransitEncryption` are the SAME two shapes
/// (`FlowTransitEncryption`, `MediaLiveTransitEncryption`), so the rule that neither may
/// change while the resource is connected compares through one identity rather than two.
pub(super) type TransitEncryptionIdentity<'a> = (Option<&'a str>, Option<(&'a str, &'a str)>);

/// [`TransitEncryptionIdentity`] for the transit encryption of a flow connected to a router
/// input (its source decryption) or to a router output (its destination encryption).
pub(super) fn flow_transit_encryption_identity(
    encryption: &FlowTransitEncryption,
) -> TransitEncryptionIdentity<'_> {
    (
        encryption.encryption_key_type.as_deref(),
        match &encryption.encryption_key_configuration {
            FlowTransitEncryptionKeyConfiguration::SecretsManager(key) => {
                Some((key.secret_arn.as_str(), key.role_arn.as_str()))
            }
            FlowTransitEncryptionKeyConfiguration::Automatic(_) => None,
        },
    )
}

/// [`TransitEncryptionIdentity`] for the transit encryption of a MediaLive resource
/// connected to a router input (a channel output's decryption) or to a router output (an
/// input's destination encryption).
pub(super) fn media_live_transit_encryption_identity(
    encryption: &MediaLiveTransitEncryption,
) -> TransitEncryptionIdentity<'_> {
    (
        encryption.encryption_key_type.as_deref(),
        match &encryption.encryption_key_configuration {
            MediaLiveTransitEncryptionKeyConfiguration::SecretsManager(key) => {
                Some((key.secret_arn.as_str(), key.role_arn.as_str()))
            }
            MediaLiveTransitEncryptionKeyConfiguration::Automatic(_) => None,
        },
    )
}

/// Whether the configuration `requested` asks for a different SOURCE TRANSIT DECRYPTION
/// than the one `stored` carries.
///
/// "You can't change `sourceTransitDecryption` while the router input is connected to a
/// flow output. To change it, first disconnect the router input, then update the decryption"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`, and the
/// same note on `MediaLiveChannelRouterInputConfiguration`), so an operation that replaces a
/// configuration asks HERE whether it would change that member. Two configurations of
/// different kinds carry no one decryption to compare, and the three that ingest on a
/// network interface carry none at all, so neither is a change of one.
pub fn router_input_source_transit_decryption_differs(
    stored: &RouterInputConfiguration,
    requested: &RouterInputConfiguration,
) -> bool {
    match (stored, requested) {
        (
            RouterInputConfiguration::MediaConnectFlow(stored),
            RouterInputConfiguration::MediaConnectFlow(requested),
        ) => {
            flow_transit_encryption_identity(&stored.source_transit_decryption)
                != flow_transit_encryption_identity(&requested.source_transit_decryption)
        }
        (
            RouterInputConfiguration::MediaLiveChannel(stored),
            RouterInputConfiguration::MediaLiveChannel(requested),
        ) => {
            media_live_transit_encryption_identity(&stored.source_transit_decryption)
                != media_live_transit_encryption_identity(&requested.source_transit_decryption)
        }
        _ => false,
    }
}

/// The DESTINATION an output configured by `configuration` is currently CONNECTED to, named
/// as the kind of resource it is, or `None` for a configuration that is connected to no such
/// destination.
///
/// The output-side counterpart of [`router_input_connected_source`]: two router output
/// configurations deliver into a resource of another AWS service, and each documents what
/// connects it — "to connect the router output to the flow source, provide both `flowArn`
/// and `flowSourceArn`. To disconnect it, call the update operation and omit both fields
/// while keeping this configuration object"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterOutputConfiguration")`), and
/// the same for both `mediaLiveInputArn` and `mediaLivePipelineId`
/// (`smithy_query(query="shape",shape="MediaLiveInputRouterOutputConfiguration")`). A
/// STANDARD output egresses on a network interface of this account and names no such
/// destination, so it is connected to none.
pub fn router_output_connected_destination(
    configuration: &RouterOutputConfiguration,
) -> Option<&'static str> {
    match configuration {
        RouterOutputConfiguration::MediaConnectFlow(flow) => {
            (flow.flow_arn.is_some() && flow.flow_source_arn.is_some()).then_some("a flow source")
        }
        RouterOutputConfiguration::MediaLiveInput(media_live) => (media_live
            .media_live_input_arn
            .is_some()
            && media_live.media_live_pipeline_id.is_some())
        .then_some("a MediaLive input"),
        RouterOutputConfiguration::Standard(_) => None,
    }
}

/// Whether the configuration `requested` asks for a different DESTINATION TRANSIT
/// ENCRYPTION than the one `stored` carries.
///
/// "You can't change `destinationTransitEncryption` while the router output is connected to
/// a flow source. To change it, first disconnect the router output, then update the
/// encryption"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterOutputConfiguration")`, and the
/// same note on `MediaLiveInputRouterOutputConfiguration`), so an operation that replaces a
/// configuration asks HERE whether it would change that member — the output-side counterpart
/// of [`router_input_source_transit_decryption_differs`]. Two configurations of different
/// kinds carry no one encryption to compare, and a STANDARD one carries none at all, so
/// neither is a change of one.
pub fn router_output_destination_transit_encryption_differs(
    stored: &RouterOutputConfiguration,
    requested: &RouterOutputConfiguration,
) -> bool {
    match (stored, requested) {
        (
            RouterOutputConfiguration::MediaConnectFlow(stored),
            RouterOutputConfiguration::MediaConnectFlow(requested),
        ) => {
            flow_transit_encryption_identity(&stored.destination_transit_encryption)
                != flow_transit_encryption_identity(&requested.destination_transit_encryption)
        }
        (
            RouterOutputConfiguration::MediaLiveInput(stored),
            RouterOutputConfiguration::MediaLiveInput(requested),
        ) => {
            media_live_transit_encryption_identity(&stored.destination_transit_encryption)
                != media_live_transit_encryption_identity(&requested.destination_transit_encryption)
        }
        _ => false,
    }
}

/// A router input, as the service holds it between calls. Every router input operation's
/// response is computed from this record.
///
/// Keyed in the store by its own ARN, which is what every router input operation addresses
/// one by (`@httpLabel Arn`) and which carries the input's generated id, so two router
/// inputs that share a name are still distinct resources.
#[derive(Debug, Clone)]
pub struct StoredRouterInput {
    pub arn: String,
    /// `RouterInput$Id`: the per-input id the ARN carries, minted by
    /// [`router_resource_id`].
    pub id: String,
    pub name: String,
    pub state: String,
    /// The state change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
    pub configuration: RouterInputConfiguration,
    pub maximum_bitrate: i64,
    pub tier: String,
    pub routing_scope: String,
    pub region_name: String,
    pub availability_zone: String,
    pub transit_encryption: RouterInputTransitEncryption,
    pub maintenance_configuration: MaintenanceConfiguration,
    pub content_quality_analysis_configuration: RouterContentQualityAnalysisConfiguration,
    pub tags: BTreeMap<String, String>,
    /// `RouterInput$IpAddress`, for an input that ingests on a network interface of its
    /// own (see [`router_input_has_own_address`]).
    pub ip_address: Option<String>,
    /// The `ClientToken` of the create that minted this input, when it carried one — what
    /// makes a repeated create with the same token return THIS input rather than mint a
    /// second one (`CreateRouterInputRequest$ClientToken` is an `@idempotencyToken`).
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    /// When this record was last changed. A restart is such a change, so the member moves;
    /// a read never moves it.
    pub updated_at: SystemTime,
    /// The maintenance window this input is next maintained in
    /// (`RouterInput$MaintenanceSchedule`), or `None` while nothing has scheduled one.
    ///
    /// StartRouterInput is the call that schedules the window, from the input's maintenance
    /// CONFIGURATION ([`maintenance_window_schedule`]); it is held on the record rather than
    /// recomputed per read so every later read reports the same window — until the clock
    /// passes its end, when the next occurrence of the same preference is owed (see
    /// [`Self::settle`]), or until that preference is replaced, when the next occurrence of
    /// the NEW one is ([`apply_maintenance_configuration`]).
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    /// When the record stops existing, or `None` while the input is not being deleted.
    ///
    /// A router input delete is not instantaneous and it is not a soft delete: the input
    /// reports `DELETING` for one [`TRANSITION_SPAN`], and then it is gone, because
    /// `RouterInputState` declares no terminal state a deleted input could be reported in.
    /// Holding the instant rather than sleeping for it is what lets the delete answer
    /// `DELETING` while a later call sees the input removed; the removal itself is
    /// materialised by [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`],
    /// because it was owed at this instant whether or not anybody called.
    pub removal_due_at: Option<SystemTime>,
}

impl StoredRouterInput {
    /// Materialise the state change the clock already made due, if it is due at `now`, and
    /// re-schedule a maintenance window the clock has already carried this input past.
    ///
    /// As for a flow (see [`StoredFlow::settle`]) these are the only writes a reporting
    /// operation performs: each was owed at its own instant whether or not anybody called. A
    /// window that has closed is spent, and the resource is maintained again at the next
    /// occurrence of the preference it carries, so a read never reports a maintenance window
    /// that lies entirely in the past.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
        if self
            .maintenance_schedule
            .as_ref()
            .is_some_and(|schedule| maintenance_window_has_closed(schedule, now))
        {
            self.maintenance_schedule = Some(maintenance_window_schedule(
                &self.maintenance_configuration,
                now,
            ));
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router input counterpart of [`StoredFlow::begin_transition`], so every state a
    /// router input is answered in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// The state this input is heading for: the one the clock already owes it, or the one it
    /// is settled in when the clock owes none.
    ///
    /// What a call that puts the input through a TRANSIENT state of its own hands
    /// [`Self::begin_transition`] as the settled state, so an input reconfigured while it was
    /// still being created still reaches the state its creation owed it. The input-side
    /// counterpart of [`StoredRouterOutput::settling_state`].
    pub fn settling_state(&self) -> String {
        self.pending
            .as_ref()
            .map_or_else(|| self.state.clone(), |pending| pending.status.clone())
    }

    /// Whether a state change is in flight for this input: the clock still owes it one, so
    /// the input is between two states rather than settled in one — including the deletion
    /// it owes once DeleteRouterInput has accepted one, because an input on its way out is
    /// not an input a later call can still act on.
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// Whether this input is already on its way out: DeleteRouterInput accepted a delete
    /// for it, so a repeat of that `@idempotent` call answers the same response rather than
    /// restarting the deletion.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this input's removal due at `now`.
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead. The delete supersedes whatever state change was owed before it: an input on
    /// its way out does not settle into `ACTIVE` on the way, so the state the accepted delete
    /// reported is the state every later read answers until the record stops existing. The
    /// accepted delete is a change to the record, so it moves `updated_at` as every other
    /// mutation does.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_INPUT_STATE_DELETING.to_string();
        self.pending = None;
        self.removal_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }

    /// `RouterInput$Messages`: "messages with information about the router input".
    ///
    /// Nothing in this emulator records a message about an input it serves, so the required
    /// list is empty. `ListedRouterInput$MessageCount` is "the number of messages associated
    /// with the router input", so the summary counts THIS list
    /// ([`Self::to_listed_router_input`]) and the two reads cannot disagree about how many
    /// messages an input has.
    pub fn messages(&self) -> Vec<RouterInputMessage> {
        Vec::new()
    }

    /// The modelled `RouterInput` for this record.
    ///
    /// `routed_outputs` is the number of router outputs routed to this input, which lives
    /// on the OUTPUT records rather than here
    /// ([`ComAmazonawsMediaconnectMediaconnectState::routed_output_count`] counts them), so
    /// it is passed in by the caller that holds the store.
    pub fn to_router_input(&self, routed_outputs: i32) -> RouterInput {
        RouterInput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            input_type: router_input_type(&self.configuration).to_string(),
            configuration: self.configuration.clone(),
            // `RouterInput$RoutedOutputs` is "the number of outputs currently routed to
            // this input": the router outputs this store holds whose `RoutedInputArn` is
            // this input's ARN, counted by the caller (see above).
            routed_outputs,
            // `MaximumRoutedOutputs` is "the maximum number of outputs that can be
            // simultaneously routed to this input". AWS publishes no number for it and
            // nothing in this emulator enforces one, so the optional member stays absent
            // rather than reporting a ceiling nothing applies.
            maximum_routed_outputs: None,
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            tier: self.tier.clone(),
            routing_scope: self.routing_scope.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            messages: self.messages(),
            transit_encryption: self.transit_encryption.clone(),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            stream_details: initial_router_input_stream_details(&self.configuration),
            ip_address: self.ip_address.clone(),
            maintenance_type: maintenance_type(&self.maintenance_configuration).to_string(),
            maintenance_configuration: self.maintenance_configuration.clone(),
            // A maintenance SCHEDULE is the window MediaConnect picked inside the
            // configured preference, which StartRouterInput schedules as it starts an input
            // (its response makes `MaintenanceScheduleType` and `MaintenanceSchedule`
            // required). Both members are optional HERE because an input nothing has
            // started yet has no window: the type follows from the schedule the record
            // holds, the same way `MaintenanceType` follows from the configuration.
            maintenance_schedule_type: self
                .maintenance_schedule
                .as_ref()
                .map(|_| MAINTENANCE_SCHEDULE_TYPE_WINDOW.to_string()),
            maintenance_schedule: self.maintenance_schedule.clone(),
            content_quality_analysis_type: ROUTER_CQA_TYPE_CONTENT_LEVEL.to_string(),
            content_quality_analysis_configuration: self
                .content_quality_analysis_configuration
                .clone(),
        }
    }

    /// The modelled `ListedRouterInput` summary for this record — what ListRouterInputs
    /// reports for it.
    ///
    /// The summary is a PROJECTION of the same record [`Self::to_router_input`] reports, so
    /// every member it shares with the full resource is the same value read from the same
    /// field, and the members the summary does not declare (the configuration, the stream
    /// details, the address, the tier, the tags, the messages themselves, the transit
    /// encryption and the maintenance configuration) stay on the read side. `routed_outputs`
    /// is resolved by the caller that holds the store, for the same reason as in
    /// [`Self::to_router_input`].
    pub fn to_listed_router_input(&self, routed_outputs: i32) -> ListedRouterInput {
        ListedRouterInput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            input_type: router_input_type(&self.configuration).to_string(),
            state: self.state.clone(),
            routed_outputs,
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            routing_scope: self.routing_scope.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            // "The number of messages associated with the router input": the count of the
            // messages the record holds, which is what `RouterInput$Messages` reports in
            // full (see [`Self::messages`]).
            message_count: i32::try_from(self.messages().len()).unwrap_or(i32::MAX),
            // "The ARN of the network interface associated with the router input" — the
            // interface the input ingests on, which only a configuration that names one has
            // ([`router_input_network_interface_arn`]).
            network_interface_arn: router_input_network_interface_arn(&self.configuration)
                .map(ToString::to_string),
            maintenance_schedule_type: self
                .maintenance_schedule
                .as_ref()
                .map(|_| MAINTENANCE_SCHEDULE_TYPE_WINDOW.to_string()),
            maintenance_schedule: self.maintenance_schedule.clone(),
        }
    }
}

/// The `FabricLatencyMode` the model documents as the DEFAULT recovery latency mode
/// (`FabricConfiguration$RecoveryLatencyMode`: "`BALANCED` (default) – Optimizes for
/// stream quality"), used for a request that sets no `FabricConfiguration`.
pub const FABRIC_LATENCY_MODE_BALANCED: &str = "BALANCED";

/// Bits in one megabit — the unit conversion between the tier's documented ceiling (Mbps)
/// and a router output's `MaximumBitrate` (bits per second, as every other MediaConnect
/// bitrate in this model is; see [`ASSIGNED_SOURCE_MAX_BITRATE_BPS`]).
pub const BITS_PER_MEGABIT: i64 = 1_000_000;

/// The maximum bitrate, in BITS PER SECOND, that the tier `tier` allows a router output,
/// or `None` when `tier` is not one the model declares — [`ROUTER_OUTPUT_TIERS`]' own
/// ceiling.
pub fn router_output_tier_bitrate_ceiling(tier: &str) -> Option<i64> {
    ROUTER_OUTPUT_TIERS.bitrate_ceiling(tier)
}

/// The ceiling on how many router outputs this store holds.
///
/// The same bound, for the same reason, as [`MAX_GATEWAYS`]: nothing evicts a router
/// output and a read must still answer what the create created, so the create refuses once
/// the store is full — with the modelled `RouterOutputServiceQuotaExceededException`, "the
/// request to create a new router output would exceed the service quotas (limits) set for
/// the account", which the model declares on CreateRouterOutput alone. The number is this
/// emulator's own bound, not a published AWS default.
pub const MAX_ROUTER_OUTPUTS: usize = 1_000;

/// The `RouterOutputProtocol` the protocol configuration `configuration` configures —
/// the value `StandardRouterOutputConfiguration$Protocol` names
/// (`smithy_query(query="shape",shape="RouterOutputProtocol")`: RTP | RIST | SRT_CALLER |
/// SRT_LISTENER | RTMP_PUSH).
///
/// The protocol is implied by the union variant, so this is the ONE mapping between the
/// two: a create derives the reported `Protocol` from it rather than trusting the request,
/// and refuses a request whose `Protocol` names a different one.
pub fn router_output_protocol(configuration: &RouterOutputProtocolConfiguration) -> &'static str {
    match configuration {
        RouterOutputProtocolConfiguration::Rtp(_) => "RTP",
        RouterOutputProtocolConfiguration::Rist(_) => "RIST",
        RouterOutputProtocolConfiguration::SrtCaller(_) => "SRT_CALLER",
        RouterOutputProtocolConfiguration::SrtListener(_) => "SRT_LISTENER",
        RouterOutputProtocolConfiguration::RtmpPush(_) => "RTMP_PUSH",
    }
}

/// The `RouterOutputConfiguration` a request's configuration is STORED as, or the detail of
/// why it cannot be stored as one.
///
/// A STANDARD output's `Protocol` is implied by the variant of its `ProtocolConfiguration`
/// ([`router_output_protocol`]), so the stored configuration reports the protocol that
/// configuration configures, and a request whose `Protocol` names a different one is
/// refused rather than stored self-contradicting. The two configurations that deliver into
/// another AWS resource carry no protocol member to reconcile, so each is stored as it
/// arrived. Every operation that takes a configuration (CreateRouterOutput,
/// UpdateRouterOutput) normalises it HERE, so no second spelling can drift from this one.
pub fn normalized_router_output_configuration(
    configuration: RouterOutputConfiguration,
) -> Result<RouterOutputConfiguration, String> {
    match configuration {
        RouterOutputConfiguration::Standard(standard) => {
            let configured = router_output_protocol(&standard.protocol_configuration);
            if let Some(named) = standard
                .protocol
                .as_deref()
                .filter(|protocol| !protocol.is_empty())
            {
                if named != configured {
                    return Err(format!(
                        "Configuration sets a {configured} protocol configuration, but \
                         Protocol names {named}."
                    ));
                }
            }
            Ok(RouterOutputConfiguration::Standard(
                StandardRouterOutputConfiguration {
                    protocol: Some(configured.to_string()),
                    ..standard
                },
            ))
        }
        other => Ok(other),
    }
}

/// The `RouterOutputType` of an output configured by `configuration`.
///
/// `RouterOutputConfiguration` is a union with one member per output kind and
/// `RouterOutputType` names exactly those kinds, so the type an output reports is a
/// function of the configuration it holds — never a separate request member. The router
/// output counterpart of [`router_input_type`], and the one mapping every projection
/// ([`StoredRouterOutput::output_type`]) and every check on the configuration's KIND
/// resolves through.
pub fn router_output_type(configuration: &RouterOutputConfiguration) -> &'static str {
    match configuration {
        RouterOutputConfiguration::Standard(_) => ROUTER_OUTPUT_TYPE_STANDARD,
        RouterOutputConfiguration::MediaConnectFlow(_) => ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
        RouterOutputConfiguration::MediaLiveInput(_) => ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT,
    }
}

/// The destination address a router output's protocol configuration names, or `None` for a
/// protocol that names none.
///
/// Every push protocol carries a `DestinationAddress`; an SRT LISTENER carries only a
/// `Port`, because a listener waits for a caller instead of sending to an address — so a
/// listener output reports no destination address.
pub fn protocol_destination_address(
    configuration: &RouterOutputProtocolConfiguration,
) -> Option<String> {
    match configuration {
        RouterOutputProtocolConfiguration::Rtp(rtp) => Some(rtp.destination_address.clone()),
        RouterOutputProtocolConfiguration::Rist(rist) => Some(rist.destination_address.clone()),
        RouterOutputProtocolConfiguration::SrtCaller(caller) => {
            Some(caller.destination_address.clone())
        }
        RouterOutputProtocolConfiguration::RtmpPush(push) => Some(push.destination_address.clone()),
        RouterOutputProtocolConfiguration::SrtListener(_) => None,
    }
}

/// The 12-character id the service mints for a ROUTER resource (output, input, network
/// interface) from the ARN counter.
///
/// The modelled `RouterOutputArn` pattern
/// (`smithy_query(query="shape",shape="RouterOutputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerOutput:[a-z0-9]{12}$`)
/// fixes the id at exactly twelve `[a-z0-9]` characters, so the counter is rendered as
/// zero-padded lowercase hex and only its low 48 bits are used — the store is bounded far
/// below that ([`MAX_ROUTER_OUTPUTS`]), so the id never repeats within a process.
pub fn router_resource_id(suffix: u64) -> String {
    format!("{:012x}", suffix & 0xffff_ffff_ffff)
}

/// The resource fragment of a ROUTER resource's ARN: `<resource type>:<id>`, with the id
/// [`router_resource_id`] mints.
///
/// Every router resource's modelled ARN pattern has this one shape — the resource type then
/// the twelve-character id, and no name — so the two fragment helpers
/// ([`router_input_resource_fragment`], [`router_output_resource_fragment`]) differ only in
/// that type and are built HERE.
pub(super) fn router_resource_fragment(resource_type: &str, id: &str) -> String {
    format!("{resource_type}:{id}")
}

/// The resource fragment of a router output ARN: `routerOutput:<id>`, with the id
/// [`router_resource_id`] mints.
pub fn router_output_resource_fragment(id: &str) -> String {
    router_resource_fragment("routerOutput", id)
}

/// The resource fragment of a router network interface ARN: `routerNetworkInterface:<id>`,
/// with the id [`router_resource_id`] mints — the form the modelled
/// `RouterNetworkInterfaceArn` pattern fixes
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`).
pub fn router_network_interface_resource_fragment(id: &str) -> String {
    router_resource_fragment("routerNetworkInterface", id)
}

/// Whether `arn` is an ARN this service could have minted for the ROUTER resource
/// `resource_type`: the prefix [`has_mintable_arn_prefix`] describes for that type, then an
/// id of the form [`router_resource_id`] mints.
///
/// The modelled ARN pattern of every router resource
/// (`smithy_query(query="shape",shape="RouterInputArn")` /
/// `…shape="RouterOutputArn"` /  `…shape="RouterNetworkInterfaceArn"`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:<type>:[a-z0-9]{12}$`) differs
/// only in that resource type, so this is the crate's ONE encoding of all of them, checked
/// segment by segment. [`is_mintable_router_input_arn`] and
/// [`is_mintable_router_output_arn`] are the two named entry points, so no second spelling
/// can drift from this one.
pub(super) fn is_mintable_router_resource_arn(arn: &str, resource_type: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., id] = segments.as_slice() else {
        return false;
    };
    segments.len() == 7
        && has_mintable_arn_prefix(&segments, resource_type)
        && id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Whether `arn` is a router output ARN this service could have minted — the modelled
/// `RouterOutputArn` constraint, through the one encoding
/// [`is_mintable_router_resource_arn`]. The integration test asserts minted ARNs through
/// this function.
pub fn is_mintable_router_output_arn(arn: &str) -> bool {
    is_mintable_router_resource_arn(arn, "routerOutput")
}

/// Whether `arn` is a router network interface ARN this service could have minted — the
/// modelled `RouterNetworkInterfaceArn` constraint, through the one encoding
/// [`is_mintable_router_resource_arn`]. The integration test asserts minted ARNs through
/// this function.
pub fn is_mintable_router_network_interface_arn(arn: &str) -> bool {
    is_mintable_router_resource_arn(arn, "routerNetworkInterface")
}

/// Whether `zone` is an Availability Zone of the Region `region`.
///
/// `CreateRouterOutput$AvailabilityZone` must be "a valid Availability Zone for the region
/// specified by `regionName`, or the current region if no `regionName` is provided", and
/// AWS names an Availability Zone by its Region followed by one lowercase letter.
pub fn is_zone_of_region(zone: &str, region: &str) -> bool {
    zone.strip_prefix(region).is_some_and(|letter| {
        letter.len() == 1 && letter.bytes().all(|b| b.is_ascii_lowercase())
    })
}

/// The Availability Zone the service picks in `region` for a resource whose request named
/// none — the first zone of that Region.
pub fn default_availability_zone(region: &str) -> String {
    format!("{region}a")
}

/// The `RoutingScope` member that binds a router resource to its own Region
/// (`smithy_query(query="shape",shape="RoutingScope")` declares REGIONAL and GLOBAL): a
/// REGIONAL router input "connects only to outputs in same Region"
/// (`CreateRouterInputRequest$RoutingScope`) and a REGIONAL router output "can only take
/// inputs from same Region" (`CreateRouterOutputRequest$RoutingScope`). The other member
/// puts no such bound on the side that carries it.
pub const ROUTING_SCOPE_REGIONAL: &str = "REGIONAL";

/// Whether a router input in `router_input_region` whose routing scope is
/// `router_input_scope` may be associated with a router output in `router_output_region`
/// whose routing scope is `router_output_scope`.
///
/// The model states one rule from both sides of the association — a REGIONAL input reaches
/// outputs in its own Region only, and a REGIONAL output takes inputs from its own Region
/// only — so a pair in ONE Region is admitted whatever the two scopes are, and a pair that
/// spans two Regions is admitted only when NEITHER side is bound to its Region
/// ([`ROUTING_SCOPE_REGIONAL`]). This is the crate's one reading of that rule: every
/// operation that would record such an association, or leave a recorded one standing under
/// a narrower scope, asks here.
pub fn routing_scopes_admit_association(
    router_input_scope: &str,
    router_input_region: &str,
    router_output_scope: &str,
    router_output_region: &str,
) -> bool {
    router_input_region == router_output_region
        || (router_input_scope != ROUTING_SCOPE_REGIONAL
            && router_output_scope != ROUTING_SCOPE_REGIONAL)
}

/// The sentence a refusal carries when [`routing_scopes_admit_association`] does not admit
/// the pair: both sides, with the scope and the Region the verdict was taken on, and the
/// rule they break.
///
/// One function, so a take that would record the association and an update that would
/// narrow a scope under one state the same rule the same way.
pub fn cross_region_association_detail(
    router_input_arn: &str,
    router_input_scope: &str,
    router_input_region: &str,
    router_output_arn: &str,
    router_output_scope: &str,
    router_output_region: &str,
) -> String {
    format!(
        "Router input {router_input_arn} (RoutingScope {router_input_scope}, RegionName \
         {router_input_region}) cannot be associated with router output {router_output_arn} \
         (RoutingScope {router_output_scope}, RegionName {router_output_region}): a REGIONAL \
         router input connects only to router outputs in the same Region, and a REGIONAL \
         router output can only take router inputs from the same Region."
    )
}

/// Why the router input `router_input` cannot be associated with the router output
/// `router_output`, or `None` when the pair may be associated.
///
/// The verdict is [`routing_scopes_admit_association`] over the two STORED records, so the
/// association TakeRouterInput records is one the two sides' routing scopes and Regions
/// admit — and the route an output reports, and the count an input reports, name an
/// association the model allows to exist.
pub fn cross_region_route_rejection(
    router_input: &StoredRouterInput,
    router_output: &StoredRouterOutput,
) -> Option<String> {
    if routing_scopes_admit_association(
        &router_input.routing_scope,
        &router_input.region_name,
        &router_output.routing_scope,
        &router_output.region_name,
    ) {
        return None;
    }
    Some(cross_region_association_detail(
        &router_input.arn,
        &router_input.routing_scope,
        &router_input.region_name,
        &router_output.arn,
        &router_output.routing_scope,
        &router_output.region_name,
    ))
}

/// A router output, as the service holds it between calls. Every router output
/// operation's response is computed from this record.
#[derive(Debug, Clone)]
pub struct StoredRouterOutput {
    /// `RouterOutput$Arn` — the output's identity, and the key of
    /// [`ComAmazonawsMediaconnectMediaconnectState::router_outputs`].
    pub arn: String,
    /// `RouterOutput$Id`: "the unique identifier of the router output" — the id segment of
    /// [`Self::arn`] ([`router_resource_id`]).
    pub id: String,
    pub name: String,
    /// The configuration the create stored, with `Protocol` normalised to the protocol its
    /// `ProtocolConfiguration` configures (see [`router_output_protocol`]). Its union
    /// variant is also what [`Self::output_type`] and [`Self::stream_details`] report.
    pub configuration: RouterOutputConfiguration,
    pub maximum_bitrate: i64,
    pub routing_scope: String,
    pub tier: String,
    pub region_name: String,
    pub availability_zone: String,
    /// `RouterOutput$IpAddress`: "the IP address of the router output" — the address the
    /// service assigns the output as it is created, from the same documentation-range pool
    /// a flow's egress address comes from ([`assigned_egress_ip`]).
    pub ip_address: Option<String>,
    pub maintenance_configuration: MaintenanceConfiguration,
    pub fabric_configuration: FabricConfiguration,
    /// `RouterOutput$Tags`, as the create set them. Ordered, so the record has one stable
    /// reading.
    pub tags: BTreeMap<String, String>,
    /// `RouterOutput$State` — see [`ROUTER_OUTPUT_STATE_CREATING`].
    pub state: String,
    /// The state change the clock owes this output, if any. See [`Self::settle`].
    pub pending: Option<PendingStatus>,
    /// `RouterOutput$RoutedInputArn`: the router input currently routed to this output, or
    /// `None` while nothing is. TakeRouterInput is the one call that sets it (and the one
    /// that clears it, by naming no input), so a created output is UNROUTED.
    ///
    /// It names a router input the store HOLDS: once the clock completes the deletion of
    /// that input the association goes with it
    /// ([`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`]), so no output
    /// reports a route into an input no later caller can read.
    pub routed_input_arn: Option<String>,
    /// When the routed state this output's association owes it comes into effect, or `None`
    /// while the clock owes it none. See [`Self::begin_route`].
    pub routing_due_at: Option<SystemTime>,
    /// `RouterOutput$Messages`: the messages the service has recorded about this output. A
    /// freshly created output has none, and the operations that record one are still
    /// scaffolded — the member is reported from here either way, never as a literal.
    pub messages: Vec<RouterOutputMessage>,
    /// The maintenance window currently scheduled for this output, or `None` while none is.
    ///
    /// `RouterOutput$MaintenanceSchedule` is optional while
    /// [`Self::maintenance_configuration`] is reported for every output: the configuration
    /// is the PREFERENCE a create or an update states, and this is the concrete window the
    /// service resolved it to ([`maintenance_window_schedule`]) — which StartRouterOutput
    /// schedules and answers, because an output that is carrying content is one maintenance
    /// has to be scheduled around. It is the router output side of
    /// [`StoredRouterInput::maintenance_schedule`], held rather than recomputed per read so
    /// every later read reports the window the caller was told about — until the clock
    /// passes its end, when the next occurrence of the same preference is owed (see
    /// [`Self::settle`]), or until that preference is replaced, when the next occurrence of
    /// the NEW one is ([`apply_maintenance_configuration`]).
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    /// The `@idempotencyToken` of the create that minted this output
    /// (`CreateRouterOutputRequest$ClientToken`, "a unique identifier for the request to
    /// ensure idempotency"), or `None` when the request carried none: a create replaying a
    /// token this store already holds returns THIS record instead of minting a second one.
    pub client_token: Option<String>,
    /// When the clock completes the delete of this router output, set by DeleteRouterOutput
    /// and `None` while the output is live.
    ///
    /// A router output delete is a HARD delete — `RouterOutputState` has no terminal deleted
    /// value (see [`ROUTER_OUTPUT_STATE_DELETING`]) — so this is the instant the deletion
    /// the call accepted becomes complete: until then the output is readable in `DELETING`
    /// and a repeated delete answers the same thing, and from then on the store no longer
    /// holds it. The removal is owed at this instant whether or not anybody calls, which is
    /// why a reporting operation may materialise it
    /// ([`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`]).
    pub removal_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl StoredRouterOutput {
    /// Materialise the state change the clock already made due, if it is due at `now`, and
    /// re-schedule a maintenance window the clock has already carried this output past.
    ///
    /// The same contract as [`StoredFlow::settle`] and [`StoredRouterInput::settle`]: these
    /// are the only writes a reporting operation (BatchGetRouterOutput, GetRouterOutput)
    /// performs, because each was owed at its own instant whether or not anybody called. A
    /// window that has closed is spent, and the output is maintained again at the next
    /// occurrence of the preference it carries, so a read never reports a maintenance window
    /// that lies entirely in the past.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
        // The routed state an accepted association owes this output is due at its own
        // instant too: once it has passed, the output is settled in the routed state its
        // `RoutedInputArn` reports ([`Self::routed_state`]) rather than still `ROUTING`.
        if self.routing_due_at.is_some_and(|due_at| now >= due_at) {
            self.routing_due_at = None;
        }
        if self
            .maintenance_schedule
            .as_ref()
            .is_some_and(|schedule| maintenance_window_has_closed(schedule, now))
        {
            self.maintenance_schedule = Some(maintenance_window_schedule(
                &self.maintenance_configuration,
                now,
            ));
        }
    }

    /// Record the association `routed_input_arn` names — the router input routed to this
    /// output, or none at all — and owe the routed state it settles into once
    /// [`TRANSITION_SPAN`] has passed.
    ///
    /// The routed-state counterpart of [`Self::begin_transition`], which moves the output's
    /// own lifecycle state instead: TakeRouterInput answers `202 Accepted`, so the
    /// association is reported in the non-terminal `ROUTING` until the clock settles it, and
    /// a caller that reads the output straight after the call still sees the take in flight.
    /// The association is a change to the record, so `updated_at` moves with it as it does
    /// for every other mutation.
    pub fn begin_route(&mut self, routed_input_arn: Option<String>, now: SystemTime) {
        self.routed_input_arn = routed_input_arn;
        self.routing_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router output counterpart of [`StoredFlow::begin_transition`] and
    /// [`StoredRouterInput::begin_transition`], so every state a router output is answered
    /// in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether a state change is in flight for this output: the clock still owes it one, so
    /// the output is between two states rather than settled in one — including the deletion
    /// it owes once DeleteRouterOutput has accepted one, because an output on its way out is
    /// not an output a later call can still act on. The router output counterpart of
    /// [`StoredRouterInput::is_transitioning`].
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// The state this output is heading for: the one the clock already owes it, or the one
    /// it is settled in when the clock owes none.
    ///
    /// What a call that puts the output through a TRANSIENT state of its own hands
    /// [`Self::begin_transition`] as the settled state, so an output reconfigured while it
    /// was still being created still reaches the state its creation owed it.
    pub fn settling_state(&self) -> String {
        self.pending
            .as_ref()
            .map_or_else(|| self.state.clone(), |pending| pending.status.clone())
    }

    /// Whether this router output's delete has been accepted, so it is on its way out of
    /// the store rather than live. See [`Self::removal_due_at`].
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this output's removal due at `now` — the same spelling
    /// [`StoredRouterInput::is_removal_due`] uses, so the two record types are settled by
    /// one rule in
    /// [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`].
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead, and the router output spelling of [`StoredRouterInput::begin_deletion`] and
    /// [`StoredRouterNetworkInterface::begin_deletion`]. The delete supersedes whatever state
    /// change was owed before it: a router output on its way out does not settle into
    /// `STANDBY` on the way, so the state the accepted delete reported is the state every
    /// later read answers until the record stops existing. The accepted delete is a change to
    /// the record, so it moves `updated_at` as every other mutation does.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_OUTPUT_STATE_DELETING.to_string();
        self.pending = None;
        self.removal_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }

    /// `RouterOutput$OutputType`, the variant of the output's configuration union —
    /// resolved through the crate's one mapping, [`router_output_type`].
    pub fn output_type(&self) -> &'static str {
        router_output_type(&self.configuration)
    }

    /// Whether a router input is currently routed to this output — the ACTIVE ROUTE that
    /// `UpdateRouterOutput$FabricConfiguration` names as its precondition ("you cannot
    /// update the fabric configuration while the output has an active route"), which is the
    /// same condition [`Self::routed_state`] reports as `ROUTED`.
    pub fn is_routed(&self) -> bool {
        self.routed_input_arn.is_some()
    }

    /// `RouterOutput$RoutedState`: ROUTING while the clock still owes the association an
    /// accepted take recorded, then ROUTED exactly while a router input is routed to this
    /// output and UNROUTED otherwise.
    ///
    /// Every read of a router output materialises what the clock already made due first
    /// ([`Self::settle`]), so the transient value is reported for exactly as long as the
    /// association is in flight.
    pub fn routed_state(&self) -> &'static str {
        if self.routing_due_at.is_some() {
            ROUTED_STATE_ROUTING
        } else if self.is_routed() {
            ROUTED_STATE_ROUTED
        } else {
            ROUTED_STATE_UNROUTED
        }
    }

    /// `RouterOutput$MaintenanceType`, the variant of the output's maintenance
    /// configuration union — resolved through the crate's one mapping,
    /// [`maintenance_type`].
    pub fn maintenance_type(&self) -> &'static str {
        maintenance_type(&self.maintenance_configuration)
    }

    /// `RouterOutput$StreamDetails` — the union member that matches the output's
    /// configuration, carrying what the service knows about the stream it carries: for a
    /// STANDARD output, the destination address its protocol configuration names
    /// ([`protocol_destination_address`]); for the two that deliver into another AWS
    /// resource, the empty details the model declares for them.
    pub fn stream_details(&self) -> RouterOutputStreamDetails {
        match &self.configuration {
            RouterOutputConfiguration::Standard(standard) => RouterOutputStreamDetails::Standard(
                StandardRouterOutputStreamDetails {
                    destination_ip_address: protocol_destination_address(
                        &standard.protocol_configuration,
                    ),
                },
            ),
            RouterOutputConfiguration::MediaConnectFlow(_) => {
                RouterOutputStreamDetails::MediaConnectFlow(
                    MediaConnectFlowRouterOutputStreamDetails {},
                )
            }
            RouterOutputConfiguration::MediaLiveInput(_) => {
                RouterOutputStreamDetails::MediaLiveInput(MediaLiveInputRouterOutputStreamDetails {})
            }
        }
    }

    /// The modelled `RouterOutput` for this record.
    pub fn to_router_output(&self) -> RouterOutput {
        RouterOutput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            output_type: self.output_type().to_string(),
            configuration: self.configuration.clone(),
            routed_state: self.routed_state().to_string(),
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            routing_scope: self.routing_scope.clone(),
            tier: self.tier.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            messages: self.messages.clone(),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            stream_details: self.stream_details(),
            ip_address: self.ip_address.clone(),
            routed_input_arn: self.routed_input_arn.clone(),
            maintenance_type: self.maintenance_type().to_string(),
            maintenance_configuration: self.maintenance_configuration.clone(),
            // The window the service has scheduled for this output, and the one type the
            // `MaintenanceScheduleType` enum declares for it. Both are absent until
            // something schedules a window (StartRouterOutput does), which is what the
            // model's optional members say about an output that has none yet — reported the
            // way the router input projection reports the same pair.
            maintenance_schedule_type: self
                .maintenance_schedule
                .as_ref()
                .map(|_| MAINTENANCE_SCHEDULE_TYPE_WINDOW.to_string()),
            maintenance_schedule: self.maintenance_schedule.clone(),
            fabric_configuration: self.fabric_configuration.clone(),
        }
    }

    /// The modelled `ListedRouterOutput` summary for this record — what ListRouterOutputs
    /// reports for it.
    ///
    /// The summary is a PROJECTION of the same record [`Self::to_router_output`] reports, so
    /// every member it shares with the full resource is the same value read from the same
    /// field, and the members the summary does not declare (the configuration, the stream
    /// details, the address, the tier, the tags, the messages themselves) stay on the read
    /// side.
    pub fn to_listed_router_output(&self) -> ListedRouterOutput {
        ListedRouterOutput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            output_type: self.output_type().to_string(),
            state: self.state.clone(),
            routed_state: self.routed_state().to_string(),
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            routing_scope: self.routing_scope.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            // `ListedRouterOutput$MessageCount` is "the number of messages associated with
            // the router output": the count of the messages the record holds, which is what
            // `RouterOutput$Messages` reports in full.
            message_count: i32::try_from(self.messages.len()).unwrap_or(i32::MAX),
            routed_input_arn: self.routed_input_arn.clone(),
            // "The ARN of the network interface associated with the router output" — the
            // interface the output egresses from, which only a configuration that names one
            // has ([`router_output_network_interface_arn`]).
            network_interface_arn: router_output_network_interface_arn(&self.configuration)
                .map(ToString::to_string),
            maintenance_schedule_type: self
                .maintenance_schedule
                .as_ref()
                .map(|_| MAINTENANCE_SCHEDULE_TYPE_WINDOW.to_string()),
            maintenance_schedule: self.maintenance_schedule.clone(),
        }
    }
}

/// The ceiling on how many router network interfaces this store holds.
///
/// The same bound, for the same reason, as [`MAX_ROUTER_OUTPUTS`]: nothing evicts a network
/// interface and a read must still answer what the create created, so the create refuses
/// once the store is full — with the modelled
/// `RouterNetworkInterfaceServiceQuotaExceededException` (HTTP 420), which the model
/// declares on CreateRouterNetworkInterface alone. The number is this emulator's own bound,
/// not a published AWS default.
pub const MAX_ROUTER_NETWORK_INTERFACES: usize = 1_000;

/// A router network interface, as the service holds it between calls — "a network interface
/// that can be associated with one or more router inputs and outputs"
/// (`smithy_query(query="shape",shape="RouterNetworkInterface")`). Every router network
/// interface operation's response is computed from this record, and the two association
/// counts from the input and output records that NAME this interface
/// ([`ComAmazonawsMediaconnectMediaconnectState::router_network_interface_projection`]).
#[derive(Debug, Clone)]
pub struct StoredRouterNetworkInterface {
    /// `RouterNetworkInterface$Arn` — the interface's identity, and the key of
    /// [`ComAmazonawsMediaconnectMediaconnectState::router_network_interfaces`].
    pub arn: String,
    /// `RouterNetworkInterface$Id`: "the unique identifier of the router network interface"
    /// — the id segment of [`Self::arn`] ([`router_resource_id`]).
    pub id: String,
    pub name: String,
    /// The configuration the create stored. Its union variant is also what
    /// [`Self::network_interface_type`] reports.
    pub configuration: RouterNetworkInterfaceConfiguration,
    pub region_name: String,
    /// `RouterNetworkInterface$Tags`, as the create set them. Ordered, so the record has
    /// one stable reading.
    pub tags: BTreeMap<String, String>,
    /// `RouterNetworkInterface$State` — see [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`].
    pub state: String,
    /// The state change the clock owes this interface, if any. See [`Self::settle`].
    pub pending: Option<PendingStatus>,
    /// The `@idempotencyToken` of the create that minted this interface
    /// (`CreateRouterNetworkInterfaceRequest$ClientToken`), or `None` when the request
    /// carried none: a create replaying a token this store already holds returns THIS
    /// record instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    /// When this record was last changed. An accepted UpdateRouterNetworkInterface that
    /// changes a member moves it; a read never moves it.
    pub updated_at: SystemTime,
    /// When the record stops existing, or `None` while the interface is not being deleted.
    ///
    /// A router network interface delete is a HARD delete, for the same reason a router
    /// input's and a router output's are: `RouterNetworkInterfaceState` declares no terminal
    /// deleted value (see [`ROUTER_NETWORK_INTERFACE_STATE_DELETING`]). So this is the
    /// instant the deletion the call accepted becomes complete: until then the interface is
    /// readable in `DELETING` and a repeated delete answers the same thing, and from then on
    /// the store no longer holds it. The removal is owed at this instant whether or not
    /// anybody calls, which is why a reporting operation may materialise it
    /// ([`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`]).
    pub removal_due_at: Option<SystemTime>,
}

impl StoredRouterNetworkInterface {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// The same contract as [`StoredRouterOutput::settle`]: this is the only write a
    /// reporting operation (BatchGetRouterNetworkInterface) performs, because the change was
    /// owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router network interface counterpart of [`StoredFlow::begin_transition`], so every
    /// state an interface is answered in while the clock still owes a change is started one
    /// way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether a state change is in flight for this interface: the clock still owes it one,
    /// so the interface is between two states rather than settled in one — the same reading
    /// as [`StoredRouterInput::is_transitioning`], which is what an operation that MOVES the
    /// state (UpdateRouterNetworkInterface) refuses on. It includes the deletion the clock
    /// owes once DeleteRouterNetworkInterface has accepted one, because an interface on its
    /// way out is not an interface a later call can still act on.
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// Whether this interface is already on its way out: DeleteRouterNetworkInterface
    /// accepted a delete for it, so a repeat of that `@idempotent` call answers the same
    /// response rather than restarting the deletion. See [`Self::removal_due_at`].
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this interface's removal due at `now` — the same spelling
    /// [`StoredRouterInput::is_removal_due`] and [`StoredRouterOutput::is_removal_due`] use,
    /// so every record type is settled by one rule in
    /// [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`].
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead. The delete supersedes whatever state change was owed before it: an interface
    /// on its way out does not settle into `ACTIVE` on the way.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_NETWORK_INTERFACE_STATE_DELETING.to_string();
        self.pending = None;
        self.removal_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }

    /// `RouterNetworkInterface$NetworkInterfaceType`, the variant of the interface's
    /// configuration union — through the crate's one mapping,
    /// [`router_network_interface_type`].
    pub fn network_interface_type(&self) -> &'static str {
        router_network_interface_type(&self.configuration)
    }

    /// The modelled `RouterNetworkInterface` for this record, with the two association
    /// counts resolved by the caller (they live on the input and output records, not here —
    /// see [`ComAmazonawsMediaconnectMediaconnectState::router_network_interface_projection`]).
    pub fn to_router_network_interface(
        &self,
        associated_input_count: i32,
        associated_output_count: i32,
    ) -> RouterNetworkInterface {
        RouterNetworkInterface {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            network_interface_type: self.network_interface_type().to_string(),
            configuration: self.configuration.clone(),
            associated_output_count,
            associated_input_count,
            region_name: self.region_name.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }

    /// The modelled `ListedRouterNetworkInterface` summary for this record — what
    /// ListRouterNetworkInterfaces reports for it, with the two association counts resolved
    /// by the caller exactly as [`Self::to_router_network_interface`] takes them.
    ///
    /// The summary is a PROJECTION of the same record the full read reports, so every member
    /// it shares with the resource is the same value read from the same field, and the
    /// members the summary does not declare (the configuration and the tags) stay on the
    /// read side.
    pub fn to_listed_router_network_interface(
        &self,
        associated_input_count: i32,
        associated_output_count: i32,
    ) -> ListedRouterNetworkInterface {
        ListedRouterNetworkInterface {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            network_interface_type: self.network_interface_type().to_string(),
            associated_output_count,
            associated_input_count,
            state: self.state.clone(),
            region_name: self.region_name.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}
