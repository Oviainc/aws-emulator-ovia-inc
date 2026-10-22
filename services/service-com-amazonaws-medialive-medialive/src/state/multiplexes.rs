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

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{Multiplex, MultiplexSettings};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `MultiplexState` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="MultiplexState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING
/// | DELETING | DELETED). The states only a start or a stop can reach are not
/// named here: those operations are still scaffold stubs, each with its own
/// issue, so nothing in this slice can put a multiplex into one of them.
pub mod multiplex_state {
    pub const CREATING: &str = "CREATING";
    pub const IDLE: &str = "IDLE";
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
    /// The instant the clock makes `IDLE` due, while the multiplex is still
    /// `CREATING`. Stored, never slept on.
    idle_due: Option<SystemTime>,
    /// The instant the clock makes the terminal `DELETED` due, while the
    /// multiplex is `DELETING`. Stored, never slept on, and `None` on a
    /// multiplex nobody has deleted.
    deleted_due: Option<SystemTime>,
    /// The names of the programs this multiplex holds, which is what
    /// `ProgramCount` counts. A set, because `CreateMultiplexProgram`'s
    /// `ProgramName` identifies a program within one multiplex.
    ///
    /// `CreateMultiplexProgram` is still a scaffold stub with an issue of its
    /// own, so nothing writes into this set yet and a multiplex therefore
    /// reports zero programs — which is what a multiplex that has had no program
    /// created in it really holds.
    program_names: BTreeSet<String>,
}

impl MultiplexRecord {
    /// "The number of programs in the multiplex" — the programs the store holds
    /// for it, never a recorded number.
    fn program_count(&self) -> i32 {
        i32::try_from(self.program_names.len()).unwrap_or(i32::MAX)
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

    /// Move this multiplex into `DELETING`, owing the terminal `DELETED` state
    /// it then keeps once [`TRANSIENT_STATE_SPAN`] has passed: the record stays
    /// in the account, so a reader goes on reporting the multiplex in that state
    /// (which is what the SDK's `MultiplexDeleted` waiter polls
    /// `DescribeMultiplex` for) instead of losing it.
    pub fn begin_delete(&mut self, now: SystemTime) {
        self.state = multiplex_state::DELETING.to_string();
        self.idle_due = None;
        self.deleted_due = Some(now + TRANSIENT_STATE_SPAN);
    }

    /// Materialise the transition the clock has already made due: a multiplex
    /// AWS answers `CREATING` becomes `IDLE` once the span has passed, and a
    /// `DELETING` one becomes the terminal `DELETED`.
    fn settle(&mut self, now: SystemTime) {
        if self.idle_due.is_some_and(|due| now >= due) {
            self.state = multiplex_state::IDLE.to_string();
            self.idle_due = None;
        }
        if self.deleted_due.is_some_and(|due| now >= due) {
            self.state = multiplex_state::DELETED.to_string();
            self.deleted_due = None;
        }
    }
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
                idle_due: Some(now + TRANSIENT_STATE_SPAN),
                deleted_due: None,
                program_names: BTreeSet::new(),
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
