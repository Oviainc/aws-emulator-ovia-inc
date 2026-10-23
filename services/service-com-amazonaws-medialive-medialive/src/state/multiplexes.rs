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
//!
//! A multiplex's LIFECYCLE is one stored pending transition per record (a
//! [`MultiplexTransition`]: the instant it comes due and the `MultiplexState` it
//! settles into), because a multiplex is in at most one transient state at a
//! time: `CREATING` settles into `IDLE`, the `STARTING` a start begins into
//! `RUNNING`, and the `DELETING` a delete begins into the terminal `DELETED`.
//! The instants are stored and never slept on —
//! [`ComAmazonawsMedialiveMedialiveState::settle`], which every operation runs
//! first, is what materialises the ones the clock has already made due.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{Multiplex, MultiplexProgram, MultiplexProgramSettings, MultiplexSettings};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `MultiplexState` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="MultiplexState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING
/// | DELETING | DELETED). The four the model declares but this slice does not
/// name are the ones nothing here can reach: `STOPPING` belongs to
/// `StopMultiplex`, still a scaffold stub with its own issue, and
/// `CREATE_FAILED` and `RECOVERING` report a failure of the transport-stream
/// multiplexer the real service runs, which this emulator does not run and
/// therefore never claims.
pub mod multiplex_state {
    pub const CREATING: &str = "CREATING";
    pub const IDLE: &str = "IDLE";
    pub const STARTING: &str = "STARTING";
    pub const RUNNING: &str = "RUNNING";
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
    /// for it. Carries the member's wire name, the value the request carried and
    /// that published range, so the refusal can name all three.
    SettingOutOfRange {
        member: &'static str,
        value: i32,
        min: i32,
        max: i32,
    },
    /// The store already holds [`MAX_MULTIPLEXES`] multiplexes.
    StoreFull,
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
fn setting_out_of_range(settings: &MultiplexSettings) -> Option<MultiplexRefusal> {
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
            Some(value) if !(min..=max).contains(&value) => {
                Some(MultiplexRefusal::SettingOutOfRange {
                    member,
                    value,
                    min,
                    max,
                })
            }
            _ => None,
        })
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
    /// The multiplex already holds [`MAX_MULTIPLEX_PROGRAMS`] programs.
    MultiplexFull,
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
) -> Option<MultiplexProgramRefusal> {
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
                Some(MultiplexProgramRefusal::SettingOutOfRange {
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
) -> Option<MultiplexProgramRefusal> {
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
            MultiplexProgramRefusal::SettingTooLong {
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

/// One created multiplex program, as `CreateMultiplexProgram` stored it.
///
/// The members mirror the model's `MultiplexProgram` shape minus the three it
/// derives or does not report: `ChannelId` is "The MediaLive channel associated
/// with the program", which comes from the channels this store holds
/// ([`ComAmazonawsMedialiveMedialiveState::channel_in_multiplex_program`]);
/// `PacketIdentifiersMap` and `PipelineDetails` report what the transport-stream
/// multiplexer the real service runs allocates and ingests, which this emulator
/// does not run and therefore does not claim.
#[derive(Debug, Clone)]
pub struct MultiplexProgramRecord {
    /// The `ProgramName` the request created the program under, which is also
    /// the key it is stored at: it identifies the program within its multiplex.
    pub program_name: String,
    pub multiplex_program_settings: MultiplexProgramSettings,
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
}

impl MultiplexRecord {
    /// "The number of programs in the multiplex" — the programs the store holds
    /// for it, never a recorded number.
    fn program_count(&self) -> i32 {
        i32::try_from(self.programs.len()).unwrap_or(i32::MAX)
    }

    /// Whether a program can be created in this multiplex right now: in every
    /// state but the two that take the multiplex away, since a multiplex that
    /// is `DELETING` or has reached the terminal `DELETED` holds nothing new.
    /// Creating is NOT one of them — the real service accepts a program in a
    /// multiplex that has not finished coming up, which is why a caller can
    /// create one straight after `CreateMultiplex`.
    fn accepts_programs(&self) -> bool {
        self.state != multiplex_state::DELETING && self.state != multiplex_state::DELETED
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

    /// Put the multiplex into the transient `state` as of `now` and owe `to`
    /// once [`TRANSIENT_STATE_SPAN`] has passed. Stored, never slept on.
    fn begin_transition(&mut self, state: &str, to: &'static str, now: SystemTime) {
        self.state = state.to_string();
        self.transition = Some(MultiplexTransition {
            due: now + TRANSIENT_STATE_SPAN,
            to,
        });
    }

    /// Materialise the transition the clock has already made due: a multiplex
    /// AWS answers `CREATING` becomes `IDLE` once the span has passed, a
    /// `STARTING` one becomes `RUNNING`, and a `DELETING` one becomes the
    /// terminal `DELETED`.
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
    /// of `now`. Called from [`ComAmazonawsMedialiveMedialiveState::settle`],
    /// which every operation runs first.
    pub(super) fn settle_multiplexes(&mut self, now: SystemTime) {
        for multiplex in self.multiplexes.values_mut() {
            multiplex.settle(now);
        }
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
            return Err(refusal);
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
        if let Some(refusal) = program_setting_out_of_range(&multiplex_program_settings)
            .or_else(|| service_descriptor_too_long(&multiplex_program_settings))
        {
            return Err(refusal);
        }
        if video_settings_over_specified(&multiplex_program_settings) {
            return Err(MultiplexProgramRefusal::VideoSettingsOverSpecified);
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
            },
        );
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
            // ABSENT, not empty: `PacketIdentifiersMap` reports the
            // transport-stream packet identifiers the real service's
            // multiplexer allocates for the program, and `PipelineDetails`
            // reports which channel pipeline each multiplex pipeline is
            // currently ingesting from. This emulator runs neither, so it
            // reports neither rather than inventing PIDs and an active
            // pipeline for a transport stream nothing is producing.
            packet_identifiers_map: None,
            pipeline_details: None,
            program_name: Some(program.program_name.clone()),
        })
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
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
            .find(|channel| {
                channel.destinations.iter().any(|destination| {
                    destination
                        .multiplex_settings
                        .as_ref()
                        .is_some_and(|settings| {
                            settings.multiplex_id.as_deref() == Some(multiplex_id)
                                && settings.program_name.as_deref() == Some(program_name)
                        })
                })
            })
            .map(|channel| channel.id.clone())
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
