// SPDX-License-Identifier: CC0-1.0

//! The multiplex slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, its refusals, the
//! store's methods and the output projection — so the slice can be read, and
//! merged, on its own. The store itself is the `multiplexes` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module, beside the
//! channels and inputs, so every operation that reaches a multiplex reads the
//! one copy.
//!
//! Every value a multiplex-shaped response carries is either stored verbatim
//! from the request that created the multiplex or derived from the stored state
//! ([`ComAmazonawsMedialiveMedialiveState::multiplex_description`]) — no
//! operation builds a multiplex out of its own request.
//!
//! The PROGRAMS a multiplex holds live in the same slice, on the
//! [`MultiplexRecord`] that owns them: one [`MultiplexProgramRecord`] per
//! created program, keyed by the `ProgramName` that identifies it within its
//! multiplex, which is what the multiplex's own derived `ProgramCount` counts. A
//! program has no id and no ARN of its own — it is reached by naming its
//! multiplex and its name — and it goes away with the multiplex that holds it.
//! A program's `PacketIdentifiersMap` lives on that record too, because
//! `UpdateMultiplex`'s `PacketIdentifiersMapping` is keyed by program name: an
//! entry of that map is the packet identifiers of one program of the multiplex
//! the update names.
//!
//! A multiplex's LIFECYCLE is one stored pending transition per record (a
//! [`MultiplexTransition`]: the instant it comes due and the `MultiplexState` it
//! settles into), because a multiplex is in at most one transient state at a
//! time: `CREATING` settles into `IDLE`, the `STARTING` a start begins into
//! `RUNNING`, the `STOPPING` a stop begins back into `IDLE`, and the `DELETING`
//! a delete begins into the terminal `DELETED`.
//! The instants are stored and never slept on —
//! [`ComAmazonawsMedialiveMedialiveState::settle`], which every operation runs
//! first, is what materialises the ones the clock has already made due.
//!
//! A multiplex's ALERTS live in the same slice too, on the [`MultiplexRecord`]
//! they are about: one [`MultiplexAlertRecord`] per alert instance, keyed by the
//! minted alert id so a walk over them is in the order they were raised. An
//! alert is not something a caller creates — the service raises it when it
//! observes a condition on the multiplex and clears it when that condition stops
//! holding (the model's `MultiplexAlertState`: "SET - The alert is actively
//! happening. CLEARED - The alert is no longer happening.") — so the conditions
//! are observed out of the stored state (`multiplex_alert_conditions`), from the
//! same `settle` every operation runs first, and never out of the request being
//! served.
//!
//! The only conditions observed are the ones this emulator's CONTROL PLANE
//! determines ([`multiplex_alert_type`]). The alerts the real service's
//! transport-stream multiplexer raises out of the media it is carrying — a
//! pipeline that lost its source, a bitrate it could not fit — are NOT claimed
//! here, for the same reason this slice reports no `Destinations` and no
//! `PipelineDetails`: this emulator does not run that multiplexer, so it has
//! nothing to report about it.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::SystemTime;

use crate::ops::types::{
    Multiplex, MultiplexAlert, MultiplexProgram, MultiplexProgramPacketIdentifiersMap,
    MultiplexProgramSettings,
    MultiplexProgramSummary, MultiplexSettings,
};

use super::{ChannelRecord, ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `MultiplexState` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="MultiplexState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING
/// | DELETING | DELETED). The two the model declares but this slice does not
/// name are the ones nothing here can reach: `CREATE_FAILED` and `RECOVERING`
/// report a failure of the transport-stream multiplexer the real service runs,
/// which this emulator does not run and therefore never claims.
pub mod multiplex_state {
    pub const CREATING: &str = "CREATING";
    pub const IDLE: &str = "IDLE";
    pub const STARTING: &str = "STARTING";
    pub const RUNNING: &str = "RUNNING";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
}

/// How many availability zones a multiplex is created with:
/// `CreateMultiplexRequest$AvailabilityZones` documents "A list of availability
/// zones for the multiplex. You must specify exactly two."
///
/// The model targets the member at `__listOf__string`, which carries no length
/// constraint, so the boundary validator cannot refuse any other count and this
/// slice is where that documented rule lives.
pub const MULTIPLEX_AVAILABILITY_ZONES: usize = 2;

/// The id the first multiplex of a process gets.
///
/// MediaLive resource ids are numeric strings, so they are minted from a counter
/// at a fixed seven-digit width, which keeps the `BTreeMap` key order equal to
/// the mint order. A range of its own keeps the service's id spaces apart: a
/// caller that passes a channel's or an input's id where a multiplex id belongs
/// gets a `NotFoundException` rather than a coincidental hit.
const FIRST_MULTIPLEX_ID: u64 = 6_000_000;

/// The most multiplexes this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateMultiplex` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_MULTIPLEXES: usize = 100;

/// The most `RequestId` entries kept for `CreateMultiplex` idempotency — the
/// same bound, for the same reason, as [`super::MAX_IDEMPOTENCY_TOKENS`] on
/// channels: a token is only useful while the multiplex it created still exists.
pub const MAX_MULTIPLEX_TOKENS: usize = MAX_MULTIPLEXES;

/// The most programs this emulator stores in ONE multiplex.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateMultiplexProgram`
/// models `TooManyRequestsException`, which is the refusal once a multiplex is
/// full.
pub const MAX_MULTIPLEX_PROGRAMS: usize = 100;

/// The most `RequestId` entries kept for `CreateMultiplexProgram` idempotency —
/// the same bound, for the same reason, as [`MAX_MULTIPLEX_TOKENS`].
pub const MAX_MULTIPLEX_PROGRAM_TOKENS: usize = MAX_MULTIPLEX_PROGRAMS;

/// The `MultiplexAlertState` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="MultiplexAlertState")`: SET | CLEARED,
/// documented as "SET - The alert is actively happening. CLEARED - The alert is
/// no longer happening.").
///
/// The same two values every alert-state enum of this service declares, so they
/// live once in [`super::alerts`] and this is the multiplex slice's name for
/// them.
pub use super::alerts::alert_state as multiplex_alert_state;

/// The alert types this emulator raises on a multiplex.
///
/// `MultiplexAlert$AlertType` ("The type of the alert") targets the free-form
/// `__string` (`smithy_query(query="shape", shape="MultiplexAlert")`), so the
/// model publishes no catalog of values: each constant here NAMES the condition
/// this emulator observed, and nothing is raised for a condition it cannot
/// observe. One type so far, because the control plane determines exactly one
/// thing about a multiplex that is worth alerting on: a program of a RUNNING
/// multiplex whose output nothing is feeding, i.e. no MediaLive channel with a
/// healthy pipeline sends its output to that program
/// (`MultiplexProgramChannelDestinationSettings` is how a channel's
/// `OutputDestination` names the multiplex and program it feeds).
pub mod multiplex_alert_type {
    pub const PROGRAM_NO_RUNNING_CHANNEL: &str = "MULTIPLEX_PROGRAM_NO_RUNNING_CHANNEL";
}

/// The `StateFilter` values `ListMultiplexAlerts` accepts, from the member's own
/// documentation: "Specifies the set of alerts to return based on their state.
/// SET - Return only alerts with SET state. CLEARED - Return only alerts with
/// CLEARED state. ALL - Return all alerts." The member targets `__string`, so
/// the request boundary cannot refuse any other value and this is what the
/// operation refuses against: the shared
/// [`super::alerts::ALERT_STATE_FILTERS`], because every alert listing of this
/// service documents the same set and must accept the same values.
pub const MULTIPLEX_ALERT_STATE_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;

/// The `StateFilter` value that asks for every alert whatever its state ("ALL -
/// Return all alerts"), which is NOT one of the two states an alert can be in.
pub const MULTIPLEX_ALERT_STATE_FILTER_ALL: &str = super::alerts::ALERT_STATE_FILTER_ALL;

/// The most alert instances this emulator keeps for ONE multiplex.
///
/// A ceiling on the store, not an AWS quota: a condition that keeps holding,
/// stopping and holding again raises one alert instance each time, so without a
/// bound a long-lived multiplex would grow the emulator's heap without end. Once
/// it is reached, the oldest alert that has already CLEARED is dropped first —
/// a `SET` alert is still happening, so it is the last thing to go.
pub const MAX_MULTIPLEX_ALERTS: usize = 200;

/// The id the first alert of a process gets.
///
/// `MultiplexAlert$Id` is "The unique ID for this alert instance", and it is
/// also the `NextToken` a page of alerts resumes at, so it is minted from a
/// monotonic counter: the ids a later alert gets are greater than the ones an
/// earlier alert got, which is what makes the token a resume point.
const FIRST_MULTIPLEX_ALERT_ID: u64 = 7_000_000;

/// One alert instance on a multiplex, as the service raised it.
///
/// The members mirror the model's `MultiplexAlert` shape minus the one it does
/// not report: `PipelineId` is "The ID of the pipeline this alert is associated
/// with", and every condition this emulator observes is a property of the
/// multiplex and its program rather than of one of the multiplex's pipelines, so
/// there is no pipeline to name. `State` is derived from
/// [`MultiplexAlertRecord::cleared_at`] rather than stored beside it, so an
/// alert cannot report `SET` and carry a `ClearedTimestamp`.
#[derive(Debug, Clone)]
pub struct MultiplexAlertRecord {
    /// The minted id, "The unique ID for this alert instance", which is also the
    /// key this alert is stored at and the `NextToken` a page resumes from.
    id: u64,
    /// Which condition was observed, one of [`multiplex_alert_type`]'s values.
    alert_type: &'static str,
    /// The program the condition was observed on. Part of the condition's
    /// identity, together with [`Self::alert_type`]: one program's alert is not
    /// another's, so a multiplex carries at most one SET alert of a given type
    /// per program.
    program_name: String,
    /// "The user facing alert message which can have more context" — built from
    /// the state observed when the alert was raised, so it names the program and
    /// what was (or was not) feeding it.
    message: String,
    /// "The time when the alert was set": the instant the condition was first
    /// observed to hold.
    set_at: SystemTime,
    /// "The time when the alert was cleared": the instant the condition was
    /// first observed NOT to hold any more, or `None` while the alert is still
    /// happening.
    cleared_at: Option<SystemTime>,
}

impl MultiplexAlertRecord {
    /// Whether this alert is still happening, which is what makes its `State`
    /// `SET` rather than `CLEARED`.
    fn is_set(&self) -> bool {
        self.cleared_at.is_none()
    }

    /// This alert's `MultiplexAlertState`, derived from whether it has been
    /// cleared, through the one derivation every alert surface of this service
    /// shares.
    fn state(&self) -> &'static str {
        super::alerts::alert_state_of(self.cleared_at)
    }

    /// Whether `ListMultiplexAlerts`' `StateFilter` admits this alert — the one
    /// predicate every alert listing of this service shares, so a `SET` filter
    /// means the same thing on a multiplex as it does on a cluster.
    fn admitted_by(&self, filter: Option<&str>) -> bool {
        super::alerts::alert_admitted_by(self.cleared_at, filter)
    }

    /// Whether this alert is the one `condition` would raise: the same type, on
    /// the same program.
    fn is_about(&self, condition: &MultiplexAlertCondition) -> bool {
        self.alert_type == condition.alert_type && self.program_name == condition.program_name
    }
}

/// One alert condition observed on a multiplex as of some instant: which alert
/// it would raise, the program it is about, and the message naming what was
/// observed.
///
/// Observed out of the stored state and thrown away again on every `settle`; the
/// records it raises are what persist.
#[derive(Debug)]
struct MultiplexAlertCondition {
    alert_type: &'static str,
    program_name: String,
    message: String,
}

/// The longest `ProviderName` or `ServiceName` a program's
/// `MultiplexProgramServiceDescriptor` may carry: both members target
/// `__stringMax256` (`smithy_query(query="shape",
/// shape="MultiplexProgramServiceDescriptor")`), a `@length` max the request
/// boundary does not apply to the members of a nested structure.
pub const MAX_SERVICE_DESCRIPTOR_NAME_LEN: usize = 256;

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_multiplex`] stored nothing.
/// Each variant is one refusal `CreateMultiplex` models.
#[derive(Debug)]
pub enum MultiplexRefusal {
    /// The request named a number of availability zones other than the
    /// [`MULTIPLEX_AVAILABILITY_ZONES`] its member documents. Carries the count
    /// the request asked for, so the refusal can name it.
    AvailabilityZoneCount(usize),
    /// A `MultiplexSettings` member fell outside the range the model publishes
    /// for it.
    SettingOutOfRange(MultiplexSettingOutOfRange),
    /// The store already holds [`MAX_MULTIPLEXES`] multiplexes.
    StoreFull,
}

/// A `MultiplexSettings` member whose value falls outside the range the model
/// publishes for it: the member's wire name, the value the request carried and
/// that published range, so the refusal can name all three.
///
/// Its own type rather than a variant's fields, because every writer of a
/// `MultiplexSettings` — `CreateMultiplex` and `UpdateMultiplex` — refuses the
/// same values for the same reason, so the check ([`setting_out_of_range`]) and
/// the wording ([`Display`]) live here once rather than once per operation.
#[derive(Debug, Clone, Copy)]
pub struct MultiplexSettingOutOfRange {
    pub member: &'static str,
    pub value: i32,
    pub min: i32,
    pub max: i32,
}

/// The refusal's wording, which names the member, the published range and the
/// value the request carried, so a caller can fix the request from that text
/// alone. One copy, so two writers of the same settings cannot word the same
/// refusal differently.
impl std::fmt::Display for MultiplexSettingOutOfRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            member,
            value,
            min,
            max,
        } = self;
        write!(
            f,
            "multiplexSettings.{member} must be between {min} and {max}, got {value}"
        )
    }
}

/// The first `MultiplexSettings` member whose value falls outside the range the
/// model publishes for it, if any.
///
/// Every range here is the `@range` trait of the integer shape the member
/// targets (`smithy_query(query="shape", shape="MultiplexSettings")`:
/// `TransportStreamBitrate` -> `__integerMin1000000Max100000000`,
/// `TransportStreamId` -> `__integerMin0Max65535`,
/// `MaximumVideoBufferDelayMilliseconds` -> `__integerMin800Max3000`,
/// `TransportStreamReservedBitrate` -> `__integerMin0Max100000000`). They are
/// re-stated here because the request boundary validates range traits only on
/// the operation's own input members, not on the members of a nested structure
/// like this one, so a value the real service refuses would otherwise be stored.
fn setting_out_of_range(settings: &MultiplexSettings) -> Option<MultiplexSettingOutOfRange> {
    // (wire member name, the value the request carried, minimum, maximum)
    let members: [(&'static str, Option<i32>, i32, i32); 4] = [
        (
            "transportStreamBitrate",
            Some(settings.transport_stream_bitrate),
            1_000_000,
            100_000_000,
        ),
        (
            "transportStreamId",
            Some(settings.transport_stream_id),
            0,
            65_535,
        ),
        (
            "maximumVideoBufferDelayMilliseconds",
            settings.maximum_video_buffer_delay_milliseconds,
            800,
            3_000,
        ),
        (
            "transportStreamReservedBitrate",
            settings.transport_stream_reserved_bitrate,
            0,
            100_000_000,
        ),
    ];
    members
        .into_iter()
        .find_map(|(member, value, min, max)| match value {
            Some(value) if !(min..=max).contains(&value) => Some(MultiplexSettingOutOfRange {
                member,
                value,
                min,
                max,
            }),
            _ => None,
        })
}

/// Why [`ComAmazonawsMedialiveMedialiveState::stop_multiplex`] stopped nothing.
///
/// One variant, because a multiplex's own state is NOT a precondition of a stop:
/// `StopMultiplex` is documented as "Stops a running multiplex. If the multiplex
/// isn't running, this action has no effect", so the only request the store
/// cannot satisfy is one naming a multiplex it does not hold.
#[derive(Debug)]
pub enum MultiplexStopRefusal {
    /// No multiplex in the store has the id the request named, so there is
    /// nothing to stop and nothing to report.
    NotFound,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::start_multiplex`] started
/// nothing. Each variant is one refusal `StartMultiplex` models.
#[derive(Debug)]
pub enum MultiplexStartRefusal {
    /// No multiplex in the store has the id the request named, so there is
    /// nothing to start.
    NotFound,
    /// The multiplex is not in a state a start accepts. Carries its current
    /// `MultiplexState`, so the refusal can name it.
    NotStartable(String),
}

/// Why [`ComAmazonawsMedialiveMedialiveState::update_multiplex`] wrote nothing.
/// Each variant is one refusal `UpdateMultiplex` models.
#[derive(Debug)]
pub enum MultiplexUpdateRefusal {
    /// No multiplex in the store has the id the request named, so there is
    /// nothing to change.
    NotFound,
    /// The multiplex the request named is going away (`DELETING`) or gone
    /// (`DELETED`): its record is still readable, but a delete has already been
    /// accepted for that id, so it names no multiplex a caller may still
    /// change. Carries its current `MultiplexState`, so the refusal can name
    /// it.
    Gone(String),
    /// A new `MultiplexSettings` member fell outside the range the model
    /// publishes for it — the same refusal, with the same wording, the create
    /// makes.
    Settings(MultiplexSettingOutOfRange),
    /// The `PacketIdentifiersMapping` named a program the multiplex does not
    /// hold, so there is no program to set those packet identifiers on. Carries
    /// the `ProgramName` the mapping asked for.
    ProgramNotFound(String),
}

/// Why a `MultiplexProgramSettings` a request carried is not one the real
/// service accepts, whatever operation carried it: every writer of a program's
/// settings — `CreateMultiplexProgram` and `UpdateMultiplexProgram` — refuses
/// the same values for the same reasons, so the checks
/// ([`program_settings_refusal`]) and the wording ([`Display`]) live here once
/// rather than once per operation.
#[derive(Debug)]
pub enum MultiplexProgramSettingsRefusal {
    /// A `MultiplexProgramSettings` member fell outside the range the model
    /// publishes for it. Carries the member's wire path inside the structure,
    /// the value the request carried and that published range.
    SettingOutOfRange {
        member: &'static str,
        value: i32,
        min: i32,
        max: i32,
    },
    /// A `MultiplexProgramSettings` string member was longer than the `@length`
    /// max the model publishes for it. Carries the member's wire path, the
    /// length the request carried and that max.
    SettingTooLong {
        member: &'static str,
        length: usize,
        max: usize,
    },
    /// `VideoSettings` defined both `ConstantBitrate` and `StatmuxSettings`,
    /// which each member's own documentation forbids ("When this field is
    /// defined, StatmuxSettings must be undefined" and conversely).
    VideoSettingsOverSpecified,
}

/// The refusal's wording, which names the member, the rule and the value the
/// request carried, so a caller can fix the request from that text alone. One
/// copy, so two writers of the same settings cannot word the same refusal
/// differently.
impl std::fmt::Display for MultiplexProgramSettingsRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SettingOutOfRange {
                member,
                value,
                min,
                max,
            } => write!(
                f,
                "multiplexProgramSettings.{member} must be between {min} and {max}, got {value}"
            ),
            Self::SettingTooLong {
                member,
                length,
                max,
            } => write!(
                f,
                "multiplexProgramSettings.{member} must be at most {max} characters, got {length}"
            ),
            Self::VideoSettingsOverSpecified => f.write_str(
                "multiplexProgramSettings.videoSettings must define exactly one of constantBitrate and statmuxSettings",
            ),
        }
    }
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_multiplex_program`] stored
/// nothing. Each variant is one refusal `CreateMultiplexProgram` models.
#[derive(Debug)]
pub enum MultiplexProgramRefusal {
    /// No multiplex in the store has the id the request named, so there is
    /// nothing to create the program in.
    MultiplexNotFound,
    /// The multiplex the request named is going away (`DELETING`) or gone
    /// (`DELETED`), so no program can be created in it. Carries its current
    /// `MultiplexState`, so the refusal can name it.
    MultiplexGone(String),
    /// The multiplex already holds a program with that `ProgramName`, which is
    /// what identifies a program within one multiplex.
    ProgramNameInUse,
    /// The multiplex already holds a program with that `ProgramNumber`
    /// ("Unique program number"). Carries the number and the name of the
    /// program already holding it.
    ProgramNumberInUse { program_number: i32, held_by: String },
    /// The `MultiplexProgramSettings` the request carried are not ones the real
    /// service accepts.
    Settings(MultiplexProgramSettingsRefusal),
    /// The multiplex already holds [`MAX_MULTIPLEX_PROGRAMS`] programs.
    MultiplexFull,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::update_multiplex_program`] wrote
/// nothing. Each variant is one refusal `UpdateMultiplexProgram` models — a
/// different set from the create's: an update resolves a program that must
/// ALREADY exist (so "not found" is one of its refusals and "name in use" is
/// not) and never grows the multiplex (so its per-multiplex cap cannot be
/// reached).
#[derive(Debug)]
pub enum MultiplexProgramUpdateRefusal {
    /// No multiplex in the store has the id the request named.
    MultiplexNotFound,
    /// The multiplex exists but holds no program of that `ProgramName`.
    ProgramNotFound,
    /// The multiplex the request named is going away (`DELETING`) or gone
    /// (`DELETED`), so its programs are no longer updatable. Carries its
    /// current `MultiplexState`, so the refusal can name it.
    MultiplexGone(String),
    /// ANOTHER program of the same multiplex already holds the `ProgramNumber`
    /// the new settings ask for ("Unique program number"). Carries the number
    /// and the name of that program.
    ProgramNumberInUse { program_number: i32, held_by: String },
    /// The new `MultiplexProgramSettings` are not ones the real service
    /// accepts — the same refusals, with the same wording, the create makes.
    Settings(MultiplexProgramSettingsRefusal),
}

/// The first `MultiplexProgramSettings` member whose value falls outside the
/// range the model publishes for it, if any.
///
/// Every range here is the `@range` trait of the integer shape the member
/// targets (`smithy_query(query="shape",
/// shape="MultiplexProgramSettings,MultiplexVideoSettings,MultiplexStatmuxVideoSettings")`:
/// `ProgramNumber` -> `__integerMin0Max65535`,
/// `VideoSettings$ConstantBitrate` -> `__integerMin100000Max100000000`,
/// `StatmuxSettings$MaximumBitrate` and `$MinimumBitrate` ->
/// `__integerMin100000Max100000000`, `StatmuxSettings$Priority` ->
/// `__integerMinNegative5Max5`). They are re-stated here for the same reason as
/// [`setting_out_of_range`] above: the request boundary validates range traits
/// only on the operation's own input members, not on the members of a nested
/// structure, so a value the real service refuses would otherwise be stored.
fn program_setting_out_of_range(
    settings: &MultiplexProgramSettings,
) -> Option<MultiplexProgramSettingsRefusal> {
    let video = settings.video_settings.as_ref();
    let statmux = video.and_then(|video| video.statmux_settings.as_ref());
    // (wire path within `multiplexProgramSettings`, the value the request
    // carried, minimum, maximum)
    let members: [(&'static str, Option<i32>, i32, i32); 5] = [
        ("programNumber", Some(settings.program_number), 0, 65_535),
        (
            "videoSettings.constantBitrate",
            video.and_then(|video| video.constant_bitrate),
            100_000,
            100_000_000,
        ),
        (
            "videoSettings.statmuxSettings.maximumBitrate",
            statmux.and_then(|statmux| statmux.maximum_bitrate),
            100_000,
            100_000_000,
        ),
        (
            "videoSettings.statmuxSettings.minimumBitrate",
            statmux.and_then(|statmux| statmux.minimum_bitrate),
            100_000,
            100_000_000,
        ),
        (
            "videoSettings.statmuxSettings.priority",
            statmux.and_then(|statmux| statmux.priority),
            -5,
            5,
        ),
    ];
    members
        .into_iter()
        .find_map(|(member, value, min, max)| match value {
            Some(value) if !(min..=max).contains(&value) => {
                Some(MultiplexProgramSettingsRefusal::SettingOutOfRange {
                    member,
                    value,
                    min,
                    max,
                })
            }
            _ => None,
        })
}

/// The first `MultiplexProgramServiceDescriptor` name longer than the `@length`
/// max the model publishes for it ([`MAX_SERVICE_DESCRIPTOR_NAME_LEN`]), if any.
fn service_descriptor_too_long(
    settings: &MultiplexProgramSettings,
) -> Option<MultiplexProgramSettingsRefusal> {
    let descriptor = settings.service_descriptor.as_ref()?;
    [
        (
            "serviceDescriptor.providerName",
            descriptor.provider_name.as_str(),
        ),
        (
            "serviceDescriptor.serviceName",
            descriptor.service_name.as_str(),
        ),
    ]
    .into_iter()
    .find_map(|(member, value)| {
        let length = value.chars().count();
        (length > MAX_SERVICE_DESCRIPTOR_NAME_LEN).then_some(
            MultiplexProgramSettingsRefusal::SettingTooLong {
                member,
                length,
                max: MAX_SERVICE_DESCRIPTOR_NAME_LEN,
            },
        )
    })
}

/// Whether `VideoSettings` defines both of the two alternatives its own
/// documentation makes mutually exclusive: `ConstantBitrate` ("When this field
/// is defined, StatmuxSettings must be undefined") and `StatmuxSettings` ("When
/// this field is defined, ConstantBitrate must be undefined"). The model makes
/// them plain optional members rather than a union, so this is the rule's only
/// home.
fn video_settings_over_specified(settings: &MultiplexProgramSettings) -> bool {
    settings.video_settings.as_ref().is_some_and(|video| {
        video.constant_bitrate.is_some() && video.statmux_settings.is_some()
    })
}

/// The first reason these `MultiplexProgramSettings` are not ones the real
/// service accepts, if any: the published ranges, the published lengths and the
/// documented mutual exclusion inside `VideoSettings`, in one place.
///
/// Every writer of a program's settings runs this same check, so a value
/// `CreateMultiplexProgram` refuses is a value `UpdateMultiplexProgram` refuses
/// too — an update cannot store settings a create could not have stored.
fn program_settings_refusal(
    settings: &MultiplexProgramSettings,
) -> Option<MultiplexProgramSettingsRefusal> {
    program_setting_out_of_range(settings)
        .or_else(|| service_descriptor_too_long(settings))
        .or_else(|| {
            video_settings_over_specified(settings)
                .then_some(MultiplexProgramSettingsRefusal::VideoSettingsOverSpecified)
        })
}

/// One created multiplex program, as `CreateMultiplexProgram` stored it.
///
/// The members mirror the model's `MultiplexProgram` shape minus the two it
/// derives or does not report: `ChannelId` is "The MediaLive channel associated
/// with the program", which comes from the channels this store holds
/// ([`ComAmazonawsMedialiveMedialiveState::channel_in_multiplex_program`]);
/// `PipelineDetails` reports which channel pipeline each multiplex pipeline is
/// currently ingesting from, which this emulator does not run and therefore does
/// not claim.
///
/// `PacketIdentifiersMap` is stored, because a caller CAN set it: it is what
/// `UpdateMultiplex`'s `PacketIdentifiersMapping` carries, keyed by the
/// `ProgramName` the identifiers belong to (the value shape's own documentation:
/// "Packet identifiers map for a given Multiplex program"). Until a caller sets
/// it the program carries none — the PIDs the real service's multiplexer
/// allocates on its own are not something this emulator invents.
#[derive(Debug, Clone)]
pub struct MultiplexProgramRecord {
    /// The `ProgramName` the request created the program under, which is also
    /// the key it is stored at: it identifies the program within its multiplex.
    pub program_name: String,
    pub multiplex_program_settings: MultiplexProgramSettings,
    /// The packet identifiers a caller set for this program through
    /// `UpdateMultiplex`, or `None` while none have been set.
    pub packet_identifiers_map: Option<MultiplexProgramPacketIdentifiersMap>,
}

/// One created multiplex, as `CreateMultiplex` stored it plus its lifecycle
/// state.
///
/// The members mirror the model's `Multiplex` shape minus the three it derives:
/// `State` is the lifecycle below, `ProgramCount` is "The number of programs in
/// the multiplex" and `PipelinesRunningCount` is "The number of currently
/// healthy pipelines", so all three are computed on every read rather than
/// recorded at create time. `Destinations` is not stored either: the real
/// service fills it with the MediaConnect flow entitlements it provisions for
/// the multiplex, which this emulator does not run and therefore does not claim.
#[derive(Debug, Clone)]
pub struct MultiplexRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// The availability zones the request listed, in the order it listed them.
    pub availability_zones: Vec<String>,
    pub multiplex_settings: MultiplexSettings,
    pub tags: HashMap<String, String>,
    /// The multiplex's current `MultiplexState`.
    state: String,
    /// The transition the clock owes this multiplex, while it is in one of the
    /// transient states; `None` once it has settled. One member rather than one
    /// instant per transient state, because a multiplex is in at most one of
    /// them at a time, so a start cannot leave a stale delete owed.
    transition: Option<MultiplexTransition>,
    /// The programs this multiplex holds, keyed by the `ProgramName` that
    /// identifies a program within one multiplex, which is what `ProgramCount`
    /// counts. A `BTreeMap`, so a walk over them is in name order on every
    /// call.
    programs: BTreeMap<String, MultiplexProgramRecord>,
    /// The alerts raised on this multiplex, keyed by the minted alert id, so a
    /// walk over them is in the order they were raised on every call and a
    /// `NextToken` means the same thing on the next one. They live on the
    /// multiplex because that is what they are about: a deleted multiplex takes
    /// its alerts with it, exactly as it takes its programs.
    alerts: BTreeMap<u64, MultiplexAlertRecord>,
}

impl MultiplexRecord {
    /// "The number of programs in the multiplex" — the programs the store holds
    /// for it, never a recorded number.
    fn program_count(&self) -> i32 {
        i32::try_from(self.programs.len()).unwrap_or(i32::MAX)
    }

    /// Whether a delete has already been accepted for this multiplex: it is
    /// `DELETING`, or it has reached the terminal `DELETED`. The ONE predicate
    /// behind both rules below, so the two cannot drift apart — the id names a
    /// record a reader goes on reporting, and nothing may write to it again.
    fn delete_accepted(&self) -> bool {
        self.state == multiplex_state::DELETING || self.state == multiplex_state::DELETED
    }

    /// Whether a program can be created in this multiplex right now: in every
    /// state but the two that take the multiplex away, since a multiplex that
    /// is `DELETING` or has reached the terminal `DELETED` holds nothing new.
    /// Creating is NOT one of them — the real service accepts a program in a
    /// multiplex that has not finished coming up, which is why a caller can
    /// create one straight after `CreateMultiplex`.
    fn accepts_programs(&self) -> bool {
        !self.delete_accepted()
    }

    /// Whether this multiplex is one a caller may still CHANGE: the same
    /// [`MultiplexRecord::delete_accepted`] states rule it out, and for the same
    /// reason. Every other state is updatable, including the transient ones: the
    /// model publishes no state precondition for an update, so this emulator
    /// claims none beyond the delete it has already accepted.
    fn is_updatable(&self) -> bool {
        !self.delete_accepted()
    }

    /// "The number of currently healthy pipelines". A multiplex runs one
    /// pipeline per availability zone it was created in (`AvailabilityZones`:
    /// "A list of availability zones for the multiplex"), and they are healthy
    /// exactly while the multiplex is `RUNNING` — in every other state nothing
    /// is running.
    fn pipelines_running_count(&self) -> i32 {
        if self.state == multiplex_state::RUNNING {
            i32::try_from(self.availability_zones.len()).unwrap_or(i32::MAX)
        } else {
            0
        }
    }

    /// The multiplex's current `MultiplexState`, which is stored rather than
    /// derived so a refusal can name it.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Whether this multiplex is in a state a delete accepts: a multiplex is
    /// deletable once it has stopped (`IDLE`), never while it is creating,
    /// starting, running, stopping, already deleting or deleted.
    pub fn is_deletable(&self) -> bool {
        self.state == multiplex_state::IDLE
    }

    /// Whether this multiplex is in a state a start accepts: a multiplex runs
    /// only from a stopped one (`IDLE`), never while it is still coming up,
    /// already starting or running, stopping, deleting or deleted.
    fn is_startable(&self) -> bool {
        self.state == multiplex_state::IDLE
    }

    /// Whether this multiplex is RUNNING, which is the one state a stop has an
    /// effect in: `StopMultiplex` "Stops a running multiplex. If the multiplex
    /// isn't running, this action has no effect."
    fn is_running(&self) -> bool {
        self.state == multiplex_state::RUNNING
    }

    /// Whether any alert on this multiplex is still happening, so a condition of
    /// its could still have to be CLEARED. Bounded by [`MAX_MULTIPLEX_ALERTS`],
    /// and short-circuiting, so it is cheap enough to ask on every call.
    fn has_set_alert(&self) -> bool {
        self.alerts.values().any(MultiplexAlertRecord::is_set)
    }

    /// Move this multiplex into `DELETING`, owing the terminal `DELETED` state
    /// it then keeps once [`TRANSIENT_STATE_SPAN`] has passed: the record stays
    /// in the account, so a reader goes on reporting the multiplex in that state
    /// (which is what the SDK's `MultiplexDeleted` waiter polls
    /// `DescribeMultiplex` for) instead of losing it.
    pub fn begin_delete(&mut self, now: SystemTime) {
        self.begin_transition(multiplex_state::DELETING, multiplex_state::DELETED, now);
    }

    /// Move this multiplex into `STARTING`, owing `RUNNING` once
    /// [`TRANSIENT_STATE_SPAN`] has passed: starting a multiplex is
    /// asynchronous, so a caller waits on `State` (which is what the SDK's
    /// `MultiplexRunning` waiter polls `DescribeMultiplex` for).
    fn begin_start(&mut self, now: SystemTime) {
        self.begin_transition(multiplex_state::STARTING, multiplex_state::RUNNING, now);
    }

    /// Move this multiplex into `STOPPING`, owing the stopped `IDLE` state once
    /// [`TRANSIENT_STATE_SPAN`] has passed: stopping a multiplex is
    /// asynchronous, exactly like the start it undoes, so a caller waits on
    /// `State` (which is what the SDK's `MultiplexStopped` waiter polls
    /// `DescribeMultiplex` for).
    fn begin_stop(&mut self, now: SystemTime) {
        self.begin_transition(multiplex_state::STOPPING, multiplex_state::IDLE, now);
    }

    /// Put the multiplex into the transient `state` as of `now` and owe `to`
    /// once [`TRANSIENT_STATE_SPAN`] has passed. Stored, never slept on.
    fn begin_transition(&mut self, state: &str, to: &'static str, now: SystemTime) {
        self.state = state.to_string();
        self.transition = Some(MultiplexTransition {
            due: now + TRANSIENT_STATE_SPAN,
            to,
        });
    }

    /// Bring this multiplex's alerts in line with the conditions `observed`
    /// says hold as of `now`: raise one for a condition that is not already
    /// alerted, clear the alerts whose condition no longer holds, and keep the
    /// store bounded.
    ///
    /// An alert instance is one OCCURRENCE of its condition: while the condition
    /// goes on holding, the alert it raised stays `SET` with the message and the
    /// `SetTimestamp` it was raised with, and a condition that holds again after
    /// clearing raises a NEW instance with a new id.
    ///
    /// `next_alert_id` is the store's mint counter, passed in because an alert's
    /// id is unique across the account rather than within one multiplex.
    fn apply_alerts(
        &mut self,
        observed: &[MultiplexAlertCondition],
        now: SystemTime,
        next_alert_id: &mut u64,
    ) {
        // Clear first, so a condition that stopped holding frees its alert
        // before this instant's conditions are matched against what is SET.
        for alert in self.alerts.values_mut().filter(|alert| alert.is_set()) {
            if !observed.iter().any(|condition| alert.is_about(condition)) {
                alert.cleared_at = Some(now);
            }
        }

        for condition in observed {
            if self
                .alerts
                .values()
                .any(|alert| alert.is_set() && alert.is_about(condition))
            {
                continue;
            }
            let id = FIRST_MULTIPLEX_ALERT_ID + *next_alert_id;
            *next_alert_id += 1;
            self.alerts.insert(
                id,
                MultiplexAlertRecord {
                    id,
                    alert_type: condition.alert_type,
                    program_name: condition.program_name.clone(),
                    message: condition.message.clone(),
                    set_at: now,
                    cleared_at: None,
                },
            );
        }

        // Bounded, evicting store, in ONE pass over the id-ordered map: the
        // alerts that have already cleared go first, oldest first, because a
        // `SET` alert is still happening and dropping it would hide a condition
        // that holds right now; only if dropping every cleared alert is not
        // enough does the oldest still-set one go too.
        if self.alerts.len() > MAX_MULTIPLEX_ALERTS {
            let excess = self.alerts.len() - MAX_MULTIPLEX_ALERTS;
            let mut evicted: Vec<u64> = self
                .alerts
                .iter()
                .filter(|(_, alert)| !alert.is_set())
                .map(|(id, _)| *id)
                .take(excess)
                .collect();
            if evicted.len() < excess {
                let still_over = excess - evicted.len();
                evicted.extend(
                    self.alerts
                        .iter()
                        .filter(|(_, alert)| alert.is_set())
                        .map(|(id, _)| *id)
                        .take(still_over),
                );
            }
            for id in evicted {
                self.alerts.remove(&id);
            }
        }
    }

    /// Materialise the transition the clock has already made due: a multiplex
    /// AWS answers `CREATING` becomes `IDLE` once the span has passed, a
    /// `STARTING` one becomes `RUNNING`, a `STOPPING` one becomes `IDLE` again,
    /// and a `DELETING` one becomes the terminal `DELETED`.
    fn settle(&mut self, now: SystemTime) {
        let Some(transition) = self.transition else {
            return;
        };
        if now >= transition.due {
            self.state = transition.to.to_string();
            self.transition = None;
        }
    }
}

/// A transition the clock owes a multiplex: the instant it becomes due and the
/// `MultiplexState` it settles into. Stored, never slept on — the same shape as
/// the channel slice's own [`super::Transition`], for the same reason.
#[derive(Debug, Clone, Copy)]
struct MultiplexTransition {
    due: SystemTime,
    to: &'static str,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every multiplex transition the clock had already made due as
    /// of `now`, then every alert the state it left behind calls for. Called
    /// from [`ComAmazonawsMedialiveMedialiveState::settle`], which every
    /// operation runs first.
    ///
    /// Raising and clearing alerts here is what keeps them owed to the STATE
    /// rather than to the call that happens to notice them: the condition was
    /// true, and the alert owed, from the instant the mutation that created it
    /// ran, whether or not anybody called afterwards. So a reporting operation
    /// may run this — it never changes what a later caller would otherwise have
    /// seen — and `ListMultiplexAlerts` is a pure read of the result.
    ///
    /// WORK PER CALL, which every request of this service pays while it holds
    /// the state lock, is a SUM of the stored collections rather than a product
    /// of them:
    ///
    /// * one walk of the multiplexes to settle their transitions, at most
    ///   [`MAX_MULTIPLEXES`];
    /// * nothing more at all unless some multiplex is transmitting or carries an
    ///   alert that is still set — a store nothing runs in pays only the walk
    ///   above;
    /// * otherwise ONE pass over the live channels' multiplex destinations to
    ///   index the feeders ([`Self::multiplex_program_feeders`]), at most
    ///   [`super::MAX_CHANNELS`] × [`super::MAX_CHANNEL_DESTINATIONS`] = 3 200
    ///   entries, and then ONE hash lookup per program of each such multiplex,
    ///   at most [`MAX_MULTIPLEXES`] × [`MAX_MULTIPLEX_PROGRAMS`] = 10 000.
    ///
    /// So the ceiling is 13 200 compare-and-hash steps and one short string per
    /// condition that holds, whatever a caller stores: the index is what keeps
    /// the channel scan OUT of the per-program loop, where it would otherwise
    /// have multiplied the two bounds together.
    pub(super) fn settle_multiplexes(&mut self, now: SystemTime) {
        for multiplex in self.multiplexes.values_mut() {
            multiplex.settle(now);
        }

        // Every condition is observed BEFORE anything is written, because a
        // condition reads the channel store as well as the multiplex it is
        // about: an alert on one multiplex can therefore not be raised out of a
        // store another alert has already changed.
        //
        // Only a multiplex that is transmitting can have a condition hold, and
        // only one carrying a set alert can have one stop holding, so no other
        // multiplex is observed and the feeder index is not even built unless
        // one of them is.
        let observed: Vec<(String, Vec<MultiplexAlertCondition>)> = {
            let alerting: Vec<&MultiplexRecord> = self
                .multiplexes
                .values()
                .filter(|multiplex| multiplex.is_running() || multiplex.has_set_alert())
                .collect();
            if alerting.is_empty() {
                return;
            }
            let feeders = self.multiplex_program_feeders();
            alerting
                .into_iter()
                .map(|multiplex| {
                    (
                        multiplex.id.clone(),
                        multiplex_alert_conditions(multiplex, &feeders),
                    )
                })
                .collect()
        };

        let mut next_alert_id = self.next_multiplex_alert_id;
        for (id, conditions) in observed {
            if let Some(multiplex) = self.multiplexes.get_mut(&id) {
                multiplex.apply_alerts(&conditions, now, &mut next_alert_id);
            }
        }
        self.next_multiplex_alert_id = next_alert_id;
    }

    /// The channel feeding each multiplex program the live channels name, in ONE
    /// pass over their destinations: the batch form of
    /// [`Self::channel_feeding_multiplex_program`], which resolves one program at
    /// a time.
    ///
    /// Both answer the same channel for the same program — the first in channel
    /// id order whose destinations name it, since the walk is in that order and
    /// an entry already indexed is kept — so an alert about a program cannot
    /// disagree with the `ChannelId` that program's own describe reports.
    fn multiplex_program_feeders(&self) -> HashMap<(&str, &str), &ChannelRecord> {
        let mut feeders: HashMap<(&str, &str), &ChannelRecord> = HashMap::new();
        for channel in self.channels.values().filter(|channel| !channel.is_deleted()) {
            for fed in multiplex_program_destinations(channel) {
                feeders.entry(fed).or_insert(channel);
            }
        }
        feeders
    }

    /// One page of the alerts raised on the multiplex `multiplex_id`, as the
    /// `MultiplexAlert` list `ListMultiplexAlerts` reports: the alerts the
    /// `StateFilter` `filter` admits whose id is at or after `first`, in the
    /// order they were raised, at most `page_size` of them, together with the id
    /// the next page starts at when an alert is left over after this one.
    ///
    /// `filter` is the `StateFilter` the request carried, which the operation has
    /// already refused unless it is one of [`MULTIPLEX_ALERT_STATE_FILTERS`], and
    /// `None` when it carried none — the unfiltered listing, which is every
    /// alert.
    ///
    /// `None` when the store holds no multiplex with that id — the one thing
    /// that distinguishes "this multiplex has no alerts" (an empty page) from
    /// "there is no such multiplex" (the caller's `NotFoundException`), exactly
    /// as it does for
    /// [`ComAmazonawsMedialiveMedialiveState::multiplex_program_summaries`].
    pub fn multiplex_alert_page(
        &self,
        multiplex_id: &str,
        filter: Option<&str>,
        first: Option<u64>,
        page_size: usize,
    ) -> Option<(Vec<MultiplexAlert>, Option<String>)> {
        let multiplex = self.multiplexes.get(multiplex_id)?;
        let first = first.map_or(Bound::Unbounded, Bound::Included);
        let mut remaining = multiplex
            .alerts
            .range((first, Bound::Unbounded))
            .map(|(_, alert)| alert)
            .filter(|alert| alert.admitted_by(filter));
        let alerts: Vec<MultiplexAlert> = remaining
            .by_ref()
            .take(page_size)
            .map(multiplex_alert_output)
            .collect();
        // Absent on the last page: a token is handed out only when an alert the
        // same filter admits is left over after the ones this page carries.
        let next_token = remaining.next().map(|alert| alert.id.to_string());
        Some((alerts, next_token))
    }

    /// The next multiplex id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    fn mint_multiplex_id(&mut self) -> String {
        let id = FIRST_MULTIPLEX_ID + self.next_multiplex_id;
        self.next_multiplex_id += 1;
        id.to_string()
    }

    /// Store a multiplex created in `region` and answer its minted id, or
    /// refuse.
    ///
    /// The documented availability-zone count, the published ranges of the
    /// settings, the store's cap, the mint and the insert live together here, so
    /// a handler cannot check one of them and write past another; the handler's
    /// job is only to turn a [`MultiplexRefusal`] into the operation's own
    /// modelled error.
    ///
    /// A created multiplex starts `CREATING` and owes `IDLE` once
    /// [`TRANSIENT_STATE_SPAN`] has passed, which is what the real service does:
    /// creation is asynchronous and a caller waits on `State`.
    pub fn insert_multiplex(
        &mut self,
        region: &str,
        name: String,
        availability_zones: Vec<String>,
        multiplex_settings: MultiplexSettings,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Result<String, MultiplexRefusal> {
        if availability_zones.len() != MULTIPLEX_AVAILABILITY_ZONES {
            return Err(MultiplexRefusal::AvailabilityZoneCount(
                availability_zones.len(),
            ));
        }
        if let Some(refusal) = setting_out_of_range(&multiplex_settings) {
            return Err(MultiplexRefusal::SettingOutOfRange(refusal));
        }
        if self.multiplexes.len() >= MAX_MULTIPLEXES {
            return Err(MultiplexRefusal::StoreFull);
        }

        let id = self.mint_multiplex_id();
        let arn = ::engine::emulator_arn("medialive", region, &format!("multiplex:{id}"));
        self.multiplexes.insert(
            id.clone(),
            MultiplexRecord {
                id: id.clone(),
                arn,
                name,
                availability_zones,
                multiplex_settings,
                tags,
                state: multiplex_state::CREATING.to_string(),
                transition: Some(MultiplexTransition {
                    due: now + TRANSIENT_STATE_SPAN,
                    to: multiplex_state::IDLE,
                }),
                programs: BTreeMap::new(),
                alerts: BTreeMap::new(),
            },
        );
        Ok(id)
    }

    /// Record `token` as having created the multiplex `multiplex_id`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_MULTIPLEX_TOKENS`].
    pub fn record_multiplex_token(&mut self, token: String, multiplex_id: &str) {
        self.multiplex_tokens
            .record(token, multiplex_id, MAX_MULTIPLEX_TOKENS);
    }

    /// The multiplex a stored `RequestId` created, if that multiplex still
    /// exists.
    pub fn multiplex_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.multiplex_tokens.created(request_id)?;
        self.multiplexes
            .get(id)
            .map(|multiplex| multiplex.id.as_str())
    }

    /// The full `Multiplex` description of one stored multiplex, or `None` when
    /// no multiplex has that id.
    pub fn multiplex_description(&self, id: &str) -> Option<Multiplex> {
        let multiplex = self.multiplexes.get(id)?;
        Some(Multiplex {
            arn: Some(multiplex.arn.clone()),
            availability_zones: Some(multiplex.availability_zones.clone()),
            // ABSENT, not empty: `Destinations` reports the MediaConnect flow
            // entitlements the real service provisions for a multiplex, which
            // this emulator does not run. An empty list would claim the
            // multiplex outputs nowhere; absent says it is not reported.
            destinations: None,
            id: Some(multiplex.id.clone()),
            multiplex_settings: Some(multiplex.multiplex_settings.clone()),
            name: Some(multiplex.name.clone()),
            pipelines_running_count: Some(multiplex.pipelines_running_count()),
            program_count: Some(multiplex.program_count()),
            state: Some(multiplex.state.clone()),
            tags: Some(multiplex.tags.clone()),
        })
    }

    /// Change the multiplex `multiplex_id` as `UpdateMultiplex` asks, or refuse.
    ///
    /// Each member the request carries REPLACES the stored one, and a member it
    /// omits leaves that part of the multiplex exactly as it stands — the model
    /// makes all three optional, so an update that carries none asks for no
    /// change and the caller is answered with the multiplex as it is. A
    /// `MultiplexSettings` the request does carry replaces the stored structure
    /// WHOLE (the update member is the structure itself, not a per-member
    /// patch), so an absent nested member becomes absent on the multiplex.
    ///
    /// `packet_identifiers_mapping` is keyed by the `ProgramName` the
    /// identifiers belong to, which is what identifies a program within one
    /// multiplex, so each entry replaces the `PacketIdentifiersMap` of that
    /// program of THIS multiplex; a program the mapping names and the multiplex
    /// does not hold is refused rather than stored under a name nothing reads.
    ///
    /// The published ranges of the settings, the state the multiplex must be in,
    /// the programs the mapping resolves to and the writes live together here,
    /// so a handler cannot check one of them and write past another; the
    /// handler's job is only to turn a [`MultiplexUpdateRefusal`] into the
    /// operation's own modelled error. The request is validated before it is
    /// resolved, and every refusal is raised before the FIRST write, so a
    /// refused update leaves the multiplex and all of its programs exactly as
    /// they were.
    pub fn update_multiplex(
        &mut self,
        multiplex_id: &str,
        name: Option<String>,
        multiplex_settings: Option<MultiplexSettings>,
        packet_identifiers_mapping: Option<HashMap<String, MultiplexProgramPacketIdentifiersMap>>,
    ) -> Result<(), MultiplexUpdateRefusal> {
        if let Some(refusal) = multiplex_settings.as_ref().and_then(setting_out_of_range) {
            return Err(MultiplexUpdateRefusal::Settings(refusal));
        }

        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexUpdateRefusal::NotFound)?;
        if !multiplex.is_updatable() {
            return Err(MultiplexUpdateRefusal::Gone(multiplex.state.clone()));
        }

        // Every program the mapping names is resolved BEFORE anything is
        // written, so an update naming one program that exists and one that does
        // not writes neither. A `BTreeMap`, so the names are walked in order and
        // the refusal is the same whichever order the request's own map happened
        // to hash them in.
        let mut mapping: BTreeMap<String, MultiplexProgramPacketIdentifiersMap> =
            packet_identifiers_mapping
                .unwrap_or_default()
                .into_iter()
                .collect();
        if let Some(program_name) = mapping
            .keys()
            .find(|program_name| !multiplex.programs.contains_key(*program_name))
        {
            return Err(MultiplexUpdateRefusal::ProgramNotFound(
                program_name.clone(),
            ));
        }

        if let Some(name) = name {
            multiplex.name = name;
        }
        if let Some(multiplex_settings) = multiplex_settings {
            multiplex.multiplex_settings = multiplex_settings;
        }
        // The walk is over the PROGRAMS, taking each one's entry out of the
        // mapping, so every entry the check above proved present is written
        // exactly once and none can be quietly skipped.
        for (program_name, program) in &mut multiplex.programs {
            if let Some(packet_identifiers_map) = mapping.remove(program_name) {
                program.packet_identifiers_map = Some(packet_identifiers_map);
            }
        }
        debug_assert!(
            mapping.is_empty(),
            "every program the mapping names was resolved before the first write"
        );
        Ok(())
    }

    /// Run the multiplex `multiplex_id`: move it into `STARTING`, owing
    /// `RUNNING`, or refuse.
    ///
    /// The state a start needs and the transition live together here with the
    /// write, so a handler cannot check the one and transition past the other;
    /// the handler's job is only to turn a [`MultiplexStartRefusal`] into the
    /// operation's own modelled error.
    ///
    /// The multiplex's own lifecycle state is the ONLY precondition: a multiplex
    /// runs from the stopped state, so starting one that is still creating,
    /// already starting or running, stopping, deleting or deleted is refused
    /// rather than treated as a no-op. How many programs it holds is NOT a
    /// precondition — the real service accepts a start on a multiplex holding
    /// none (the recorded `StartMultiplex` call this slice was corrected
    /// against) and runs a transport stream carrying no program.
    pub fn start_multiplex(
        &mut self,
        multiplex_id: &str,
        now: SystemTime,
    ) -> Result<(), MultiplexStartRefusal> {
        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexStartRefusal::NotFound)?;
        if !multiplex.is_startable() {
            return Err(MultiplexStartRefusal::NotStartable(
                multiplex.state.clone(),
            ));
        }
        multiplex.begin_start(now);
        Ok(())
    }

    /// Stop the multiplex `multiplex_id`: move it into `STOPPING`, owing the
    /// stopped `IDLE`, or refuse because the store holds no such multiplex.
    ///
    /// The state a stop acts on and the transition live together here with the
    /// write, for the same reason as
    /// [`ComAmazonawsMedialiveMedialiveState::start_multiplex`]: a handler
    /// cannot check the one and transition past the other.
    ///
    /// A multiplex that is NOT running is left exactly as it is, which is what
    /// `StopMultiplex` documents — "Stops a running multiplex. If the multiplex
    /// isn't running, this action has no effect." So a stop of a multiplex that
    /// is still creating, still starting, already stopping, already stopped,
    /// deleting or deleted writes nothing at all (in particular it never
    /// resurrects a record a delete has taken away) and the caller is answered
    /// with the multiplex as it stands. This is the one asymmetry with the
    /// start, which the same documentation makes a precondition rather than a
    /// no-op.
    pub fn stop_multiplex(
        &mut self,
        multiplex_id: &str,
        now: SystemTime,
    ) -> Result<(), MultiplexStopRefusal> {
        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexStopRefusal::NotFound)?;
        if multiplex.is_running() {
            multiplex.begin_stop(now);
        }
        Ok(())
    }

    /// Store a program in the multiplex `multiplex_id`, or refuse.
    ///
    /// The published ranges and lengths of the nested settings, the documented
    /// mutual exclusion inside `VideoSettings`, the state the multiplex must be
    /// in, the uniqueness of `ProgramName` and of `ProgramNumber` within that
    /// multiplex, the per-multiplex cap and the insert live together here, so a
    /// handler cannot check one of them and write past another; the handler's
    /// job is only to turn a [`MultiplexProgramRefusal`] into the operation's
    /// own modelled error.
    ///
    /// The request is validated before it is resolved: a settings value the
    /// real service refuses is refused whatever multiplex the request names.
    pub fn insert_multiplex_program(
        &mut self,
        multiplex_id: &str,
        program_name: String,
        multiplex_program_settings: MultiplexProgramSettings,
    ) -> Result<(), MultiplexProgramRefusal> {
        if let Some(refusal) = program_settings_refusal(&multiplex_program_settings) {
            return Err(MultiplexProgramRefusal::Settings(refusal));
        }

        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexProgramRefusal::MultiplexNotFound)?;
        if !multiplex.accepts_programs() {
            return Err(MultiplexProgramRefusal::MultiplexGone(
                multiplex.state.clone(),
            ));
        }
        if multiplex.programs.contains_key(&program_name) {
            return Err(MultiplexProgramRefusal::ProgramNameInUse);
        }
        // `ProgramNumber` is documented as "Unique program number", and it is
        // the program's number in this multiplex's transport stream, so two
        // programs of one multiplex cannot share it.
        let program_number = multiplex_program_settings.program_number;
        if let Some(held_by) = multiplex
            .programs
            .values()
            .find(|program| program.multiplex_program_settings.program_number == program_number)
        {
            return Err(MultiplexProgramRefusal::ProgramNumberInUse {
                program_number,
                held_by: held_by.program_name.clone(),
            });
        }
        if multiplex.programs.len() >= MAX_MULTIPLEX_PROGRAMS {
            return Err(MultiplexProgramRefusal::MultiplexFull);
        }

        multiplex.programs.insert(
            program_name.clone(),
            MultiplexProgramRecord {
                program_name,
                multiplex_program_settings,
                // A created program carries no packet identifiers: the real
                // service's multiplexer allocates them, and this emulator does
                // not run it. `UpdateMultiplex`'s `PacketIdentifiersMapping` is
                // the one thing that sets them.
                packet_identifiers_map: None,
            },
        );
        Ok(())
    }

    /// Replace the settings of the program `program_name` of the multiplex
    /// `multiplex_id`, or refuse.
    ///
    /// `multiplex_program_settings` is the `MultiplexProgramSettings` the
    /// request carried, which the model makes OPTIONAL: an update that carries
    /// none asks for no change, so the stored program is left exactly as it is
    /// and the caller is answered with it. When settings ARE carried they
    /// REPLACE the stored ones whole — the model's update member is the
    /// structure itself, not a per-member patch — so an absent nested member
    /// becomes absent on the program.
    ///
    /// The checks and the write live together here for the same reason as in
    /// [`ComAmazonawsMedialiveMedialiveState::insert_multiplex_program`]: a
    /// handler cannot check one rule and write past another. The settings are
    /// validated before anything is resolved, so settings the real service
    /// refuses are refused whatever program the request names.
    ///
    /// The refusals are ordered outside in, each check reaching only what the
    /// one before it resolved: the multiplex, then its state, then the program,
    /// then the number. So a program of a multiplex that is going away is
    /// refused for the MULTIPLEX (`MultiplexGone`) whether or not that program
    /// exists, and a program that does not exist is refused as
    /// `ProgramNotFound` whatever number the new settings ask for.
    pub fn update_multiplex_program(
        &mut self,
        multiplex_id: &str,
        program_name: &str,
        multiplex_program_settings: Option<MultiplexProgramSettings>,
    ) -> Result<(), MultiplexProgramUpdateRefusal> {
        if let Some(refusal) = multiplex_program_settings
            .as_ref()
            .and_then(program_settings_refusal)
        {
            return Err(MultiplexProgramUpdateRefusal::Settings(refusal));
        }

        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexProgramUpdateRefusal::MultiplexNotFound)?;
        // A multiplex that is going away or gone carries no updatable program,
        // the same state rule the create applies.
        if !multiplex.accepts_programs() {
            return Err(MultiplexProgramUpdateRefusal::MultiplexGone(
                multiplex.state.clone(),
            ));
        }

        let Some(multiplex_program_settings) = multiplex_program_settings else {
            // No settings, so nothing to write — but the program the caller is
            // about to be answered with still has to exist.
            return if multiplex.programs.contains_key(program_name) {
                Ok(())
            } else {
                Err(MultiplexProgramUpdateRefusal::ProgramNotFound)
            };
        };

        // `ProgramNumber` is "Unique program number" within the multiplex, so
        // the new number may be the program's OWN but not another program's.
        // The holder is taken by name here, while the map is only read, so the
        // write below borrows it once and the program is resolved once.
        let program_number = multiplex_program_settings.program_number;
        let held_by = multiplex
            .programs
            .values()
            .find(|program| {
                program.program_name != program_name
                    && program.multiplex_program_settings.program_number == program_number
            })
            .map(|program| program.program_name.clone());

        // The one lookup of the program itself: it is both the existence check
        // and the write, so there is no second, unreachable resolution.
        let program = multiplex
            .programs
            .get_mut(program_name)
            .ok_or(MultiplexProgramUpdateRefusal::ProgramNotFound)?;
        if let Some(held_by) = held_by {
            return Err(MultiplexProgramUpdateRefusal::ProgramNumberInUse {
                program_number,
                held_by,
            });
        }
        program.multiplex_program_settings = multiplex_program_settings;
        Ok(())
    }

    /// Record `token` as having created the program `program_name` of the
    /// multiplex `multiplex_id`, evicting the oldest token first if that would
    /// push the map past [`MAX_MULTIPLEX_PROGRAM_TOKENS`].
    pub fn record_multiplex_program_token(
        &mut self,
        token: String,
        multiplex_id: &str,
        program_name: &str,
    ) {
        self.multiplex_program_tokens.record(
            token,
            &multiplex_program_key(multiplex_id, program_name),
            MAX_MULTIPLEX_PROGRAM_TOKENS,
        );
    }

    /// The program a stored `RequestId` created \u2014 the multiplex's id and the
    /// program's name \u2014 if that program still exists.
    pub fn multiplex_program_for_token(&self, request_id: &str) -> Option<(String, String)> {
        let key = self.multiplex_program_tokens.created(request_id)?;
        let (multiplex_id, program_name) = key.split_once(MULTIPLEX_PROGRAM_KEY_SEPARATOR)?;
        let program = self.multiplexes.get(multiplex_id)?.programs.get(program_name)?;
        Some((multiplex_id.to_string(), program.program_name.clone()))
    }

    /// The full `MultiplexProgram` description of one stored program, or `None`
    /// when that multiplex holds no program of that name.
    pub fn multiplex_program_description(
        &self,
        multiplex_id: &str,
        program_name: &str,
    ) -> Option<MultiplexProgram> {
        let program = self.multiplexes.get(multiplex_id)?.programs.get(program_name)?;
        Some(MultiplexProgram {
            channel_id: self.channel_in_multiplex_program(multiplex_id, &program.program_name),
            multiplex_program_settings: Some(program.multiplex_program_settings.clone()),
            // The packet identifiers a caller set through `UpdateMultiplex`,
            // and ABSENT while none were set: the PIDs the real service's
            // multiplexer allocates on its own are not ones this emulator
            // invents for a transport stream nothing is producing.
            packet_identifiers_map: program.packet_identifiers_map.clone(),
            // ABSENT, not empty: `PipelineDetails` reports which channel
            // pipeline each multiplex pipeline is currently ingesting from.
            // This emulator runs no multiplexer, so it reports no active
            // pipeline rather than inventing one.
            pipeline_details: None,
            program_name: Some(program.program_name.clone()),
        })
    }

    /// One page of the programs the multiplex `multiplex_id` holds, as the
    /// `MultiplexProgramSummary` list `ListMultiplexPrograms` reports: the
    /// programs whose name is at or after `first`, in `ProgramName` order, at
    /// most `page_size` of them, together with the name the next page starts at
    /// when a program is left over after this one.
    ///
    /// `None` when the store holds no multiplex with that id — the one thing
    /// that distinguishes "this multiplex has no programs" (an empty page) from
    /// "there is no such multiplex" (the caller's `NotFoundException`).
    ///
    /// A summary's two members come from the same places the full
    /// [`ComAmazonawsMedialiveMedialiveState::multiplex_program_description`]
    /// takes them from — the stored `ProgramName` and the channel derived from
    /// this store's channels — so a listed program cannot disagree with the
    /// same program's describe.
    pub fn multiplex_program_summaries(
        &self,
        multiplex_id: &str,
        first: Bound<&str>,
        page_size: usize,
    ) -> Option<(Vec<MultiplexProgramSummary>, Option<String>)> {
        let multiplex = self.multiplexes.get(multiplex_id)?;
        let mut remaining = multiplex.programs.range::<str, _>((first, Bound::Unbounded));
        let summaries: Vec<MultiplexProgramSummary> = remaining
            .by_ref()
            .take(page_size)
            .map(|(_, program)| MultiplexProgramSummary {
                channel_id: self
                    .channel_in_multiplex_program(multiplex_id, &program.program_name),
                program_name: Some(program.program_name.clone()),
            })
            .collect();
        // Absent on the last page: a token is handed out only when a program is
        // left over after the ones this page carries.
        let next_token = remaining.next().map(|(program_name, _)| program_name.clone());
        Some((summaries, next_token))
    }

    /// "The MediaLive channel associated with the program": the channel whose
    /// `Destinations` name this multiplex and this program
    /// (`OutputDestination$MultiplexSettings` is a
    /// `MultiplexProgramChannelDestinationSettings` carrying exactly that
    /// pair), or `None` while no channel sends its output to the program.
    ///
    /// Derived on every read, so the association tracks the channels created
    /// and deleted after the program: a channel that has reached the terminal
    /// `DELETED` state is no longer associated with anything.
    fn channel_in_multiplex_program(
        &self,
        multiplex_id: &str,
        program_name: &str,
    ) -> Option<String> {
        self.channel_feeding_multiplex_program(multiplex_id, program_name)
            .map(|channel| channel.id.clone())
    }

    /// The channel [`Self::channel_in_multiplex_program`] reports, as its
    /// record: one definition of "the channel that feeds this program", so the
    /// channel a program's `ChannelId` names is the channel an alert about that
    /// program looks at.
    fn channel_feeding_multiplex_program(
        &self,
        multiplex_id: &str,
        program_name: &str,
    ) -> Option<&ChannelRecord> {
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
            .find(|channel| {
                multiplex_program_destinations(channel)
                    .any(|fed| fed == (multiplex_id, program_name))
            })
    }
}

/// The alert conditions that hold on `multiplex` as of the state `feeders`
/// indexes.
///
/// Only a RUNNING multiplex has any: in every other state its pipelines are not
/// up ([`MultiplexRecord::pipelines_running_count`] is 0), so nothing it holds
/// is being transmitted and there is nothing to be wrong. That is also why
/// stopping a multiplex clears the alerts a run raised.
///
/// The one condition observed is
/// [`multiplex_alert_type::PROGRAM_NO_RUNNING_CHANNEL`]: a program of that
/// running multiplex that no channel with a healthy pipeline feeds, which the
/// control plane determines on its own — either no channel's `OutputDestination`
/// names the program at all, or the one that does is not running. Conditions
/// about the media itself are not observed here at all; see this module's own
/// documentation.
///
/// `feeders` is the index [`ComAmazonawsMedialiveMedialiveState::multiplex_program_feeders`]
/// built once for the whole store, so this is one hash lookup per program rather
/// than a scan of the channels per program.
fn multiplex_alert_conditions(
    multiplex: &MultiplexRecord,
    feeders: &HashMap<(&str, &str), &ChannelRecord>,
) -> Vec<MultiplexAlertCondition> {
    if !multiplex.is_running() {
        return Vec::new();
    }
    multiplex
        .programs
        .values()
        .filter_map(|program| {
            let message = match feeders
                .get(&(multiplex.id.as_str(), program.program_name.as_str()))
            {
                // Fed: at least one of the channel's pipelines is up, so its
                // output is reaching this program.
                Some(channel) if !channel.healthy_pipelines().is_empty() => return None,
                Some(channel) => format!(
                    "Multiplex program {} is fed by channel {}, which is in state {} and \
                     has no healthy pipeline",
                    program.program_name, channel.id, channel.state
                ),
                None => format!(
                    "Multiplex program {} has no MediaLive channel sending its output to it",
                    program.program_name
                ),
            };
            Some(MultiplexAlertCondition {
                alert_type: multiplex_alert_type::PROGRAM_NO_RUNNING_CHANNEL,
                program_name: program.program_name.clone(),
                message,
            })
        })
        .collect()
}

/// The multiplex programs one channel's output destinations name, as
/// (multiplex id, program name) pairs.
///
/// `OutputDestination$MultiplexSettings` is a
/// `MultiplexProgramChannelDestinationSettings` carrying exactly that pair, and
/// this is the ONE definition of "this channel sends its output to that
/// program": both the per-program lookup and the batch index below read the
/// destinations through it.
fn multiplex_program_destinations(
    channel: &ChannelRecord,
) -> impl Iterator<Item = (&str, &str)> + '_ {
    channel.destinations.iter().filter_map(|destination| {
        let settings = destination.multiplex_settings.as_ref()?;
        Some((
            settings.multiplex_id.as_deref()?,
            settings.program_name.as_deref()?,
        ))
    })
}

/// One stored alert as the model's `MultiplexAlert` shape, so every reader of an
/// alert reports it the same way.
fn multiplex_alert_output(alert: &MultiplexAlertRecord) -> MultiplexAlert {
    MultiplexAlert {
        alert_type: Some(alert.alert_type.to_string()),
        cleared_timestamp: alert
            .cleared_at
            .map(::aws_smithy_types::DateTime::from),
        id: Some(alert.id.to_string()),
        message: Some(alert.message.clone()),
        // ABSENT: `PipelineId` is "The ID of the pipeline this alert is
        // associated with", and the conditions this emulator observes are
        // properties of the multiplex and its program rather than of one
        // pipeline, so there is no pipeline to name. Naming one would claim an
        // observation of a pipeline this emulator does not run.
        pipeline_id: None,
        set_timestamp: Some(::aws_smithy_types::DateTime::from(alert.set_at)),
        state: Some(alert.state().to_string()),
    }
}

/// What separates the two halves of a multiplex program's idempotency-token key.
///
/// A program is identified by the multiplex it lives in and its name rather than
/// by an id, so the token map records both. `ProgramName` is a URI path label
/// (`smithy_query(query="operation", operation="DescribeMultiplexProgram")`:
/// `/prod/multiplexes/{MultiplexId}/programs/{ProgramName}`) and a multiplex id
/// is a numeric string, so a `/` cannot appear in the first half and the split
/// is unambiguous.
const MULTIPLEX_PROGRAM_KEY_SEPARATOR: char = '/';

/// The key [`ComAmazonawsMedialiveMedialiveState::record_multiplex_program_token`]
/// records a token against, read back by `multiplex_program_for_token`.
fn multiplex_program_key(multiplex_id: &str, program_name: &str) -> String {
    format!("{multiplex_id}{MULTIPLEX_PROGRAM_KEY_SEPARATOR}{program_name}")
}

/// Project a `Multiplex` onto one of the structurally identical per-operation
/// multiplex-description output shapes (`DescribeMultiplexOutput`,
/// `DeleteMultiplexOutput`, `StartMultiplexOutput`, `StopMultiplexOutput`).
///
/// One macro rather than a hand-written copy per operation, so a create and the
/// read that follows it cannot report the same stored multiplex differently.
macro_rules! multiplex_description_output {
    ($out:ident, $multiplex:expr) => {{
        let multiplex: $crate::ops::types::Multiplex = $multiplex;
        $out {
            arn: multiplex.arn,
            availability_zones: multiplex.availability_zones,
            destinations: multiplex.destinations,
            id: multiplex.id,
            multiplex_settings: multiplex.multiplex_settings,
            name: multiplex.name,
            pipelines_running_count: multiplex.pipelines_running_count,
            program_count: multiplex.program_count,
            state: multiplex.state,
            tags: multiplex.tags,
        }
    }};
}

pub(crate) use multiplex_description_output;

/// Project a `MultiplexProgram` onto one of the structurally identical
/// per-operation program-description output shapes
/// (`DescribeMultiplexProgramOutput`, `DeleteMultiplexProgramOutput`), whose
/// members are the `MultiplexProgram`'s own rather than a wrapper.
///
/// One macro rather than a hand-written copy per operation, so a create and the
/// read that follows it cannot report the same stored program differently.
macro_rules! multiplex_program_description_output {
    ($out:ident, $program:expr) => {{
        let program: $crate::ops::types::MultiplexProgram = $program;
        $out {
            channel_id: program.channel_id,
            multiplex_program_settings: program.multiplex_program_settings,
            packet_identifiers_map: program.packet_identifiers_map,
            pipeline_details: program.pipeline_details,
            program_name: program.program_name,
        }
    }};
}

pub(crate) use multiplex_program_description_output;
