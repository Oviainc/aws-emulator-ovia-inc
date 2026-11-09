// SPDX-License-Identifier: CC0-1.0

//! The contacts this service holds, and their versions.
//!
//! One concern of [`super`]: a reserved contact ([`Contact`]), the `ContactStatus` values
//! its lifecycle runs through, and the version history each update appends to it
//! ([`ContactVersionRecord`]).

use super::*;

/// A contact as `ReserveContact` stored it, plus every version `UpdateContact` has
/// appended to it.
///
/// The satellite the contact is flown against and the tracking overrides the antenna
/// uses are NOT fields here: they belong to a VERSION of the contact
/// ([`ContactVersionRecord`]), and the contact is flown with whichever version is active
/// — [`Contact::satellite_arn`] and [`Contact::tracking_overrides`] read it. That keeps
/// one record of each value, rather than a contact-level copy that can drift from the
/// version that produced it.
#[derive(Debug, Clone)]
pub struct Contact {
    pub contact_id: String,
    pub mission_profile_arn: String,
    pub start_time: DateTime,
    pub end_time: DateTime,
    pub pre_pass_start_time: DateTime,
    pub post_pass_end_time: DateTime,
    pub ground_station: String,
    /// The antenna at that ground station this contact's pass is placed on, as
    /// [`antenna_name`] spells it. Assigned when the contact is reserved
    /// ([`ComAmazonawsGroundstationGroundstationState::free_antenna`]) and never changed
    /// afterwards — the antenna is what the reservation holds, and `UpdateContact` changes
    /// what the antenna does during the pass, not which antenna flies it. It is what
    /// `ListGroundStationReservations` reports as the reservation's `antennaName`.
    pub antenna_name: String,
    pub contact_status: String,
    pub region: String,
    pub tags: HashMap<String, String>,
    /// Every version of this contact, oldest first and never empty: `ReserveContact`
    /// mints version [`FIRST_VERSION_ID`] and each accepted `UpdateContact` appends the
    /// next one.
    pub versions: Vec<ContactVersionRecord>,
    /// When the reservation was accepted.
    pub created: DateTime,
    /// When the contact last changed.
    pub last_updated: DateTime,
    /// The instant the scheduler owes this contact its `SCHEDULING` -> `SCHEDULED`
    /// move; `None` once the move has been materialised or the contact left
    /// `SCHEDULING`.
    pub scheduling_settles_at: Option<SystemTime>,
    /// The instant the service owes this contact its `CANCELLING` -> `CANCELLED` move —
    /// the end of the stop a cancel of a pass already under way started; `None` when no
    /// stop is being carried out.
    pub cancellation_settles_at: Option<SystemTime>,
    /// Whether this contact's pass has already held the antenna on the account's behalf,
    /// so those minutes stay spent however the status moves on afterwards: stopping a
    /// pass under way lands the contact on `CANCELLED`, and a release cannot give back
    /// antenna time the pass has already used (see [`Contact::holds_antenna_time`]).
    ///
    /// A record of minutes SPENT, not of an antenna held: which antennas a ground station
    /// still has held is [`Contact::reserves_antenna`].
    pub antenna_time_spent: bool,
    /// The dataflow wiring this contact's pass is flown through, once that wiring is no
    /// longer the one its mission profile holds: the edges the profile carried at the
    /// moment an update of it moved away from them
    /// ([`ComAmazonawsGroundstationGroundstationState::freeze_contact_dataflow_edges`]).
    ///
    /// `None` while the profile still holds the wiring this contact was reserved against,
    /// which is every contact until such an update lands — so there is ONE record of each
    /// edge for as long as the profile and the contact agree about it, and a contact whose
    /// profile was deleted without ever having been rewired under it still reports no
    /// wiring at all rather than an empty one.
    pub reserved_dataflow_edges: Option<Vec<Vec<String>>>,
    /// The parameters of the configs this contact's pass is flown with, once a config's
    /// stored parameters are no longer the ones this contact reached: the `configData` a
    /// config carried at the moment an update of it replaced them, keyed by the
    /// `(configType, configId)` pair
    /// [`ComAmazonawsGroundstationGroundstationState::configs`] is keyed on
    /// ([`ComAmazonawsGroundstationGroundstationState::freeze_contact_config_data`]).
    ///
    /// EMPTY while every config this contact's wiring reaches still holds the parameters
    /// the contact was scheduled with, which is every contact until such an update lands —
    /// so there is ONE record of each config's parameters for as long as the config and the
    /// contact agree about them, exactly as [`Contact::reserved_dataflow_edges`] keeps one
    /// record of each edge.
    pub reserved_config_data: HashMap<(String, String), ConfigTypeData>,
}

/// The model's `VersionStatus` values, whose full set is `UPDATING`, `ACTIVE`,
/// `SUPERSEDED` and `FAILED_TO_UPDATE` (`smithy_query(query="shape",
/// shape="VersionStatus")`).
///
/// A version this service holds carries one of the first three: it is `UPDATING` while
/// the service is applying it ("the version is being applied to the contact"), `ACTIVE`
/// once it is the one in force ("the current active version of the contact") and
/// `SUPERSEDED` once a newer version replaced it ("replaced by a newer version").
/// `FAILED_TO_UPDATE` is the status of an update this emulator never produces: nothing
/// here fails applying a version, so no version is ever left in it.
pub const ACTIVE_VERSION_STATUS: &str = "ACTIVE";

/// See [`ACTIVE_VERSION_STATUS`].
pub const UPDATING_VERSION_STATUS: &str = "UPDATING";

/// See [`ACTIVE_VERSION_STATUS`].
pub const SUPERSEDED_VERSION_STATUS: &str = "SUPERSEDED";

/// The version a reserved contact starts at, and the smallest `versionId` the model
/// admits (`VersionId`, range 1..128 — `smithy_query(query="shape",
/// shape="VersionId")`).
pub const FIRST_VERSION_ID: i32 = 1;

/// How many versions one contact can have: the LARGEST `versionId` the model admits
/// (`VersionId`, range 1..128), since versions are numbered from [`FIRST_VERSION_ID`]
/// without a gap. An `UpdateContact` that would need a version past it cannot be
/// answered with a `versionId` the model can carry, so it is refused with the modelled
/// `ResourceLimitExceededException`.
pub const MAX_CONTACT_VERSIONS: i32 = 128;

/// The `ContactStatus` values in which a contact still takes a new version.
///
/// `UpdateContact` changes what the antenna does during the pass — which satellite it
/// tracks (`satelliteArn`) and how (`trackingOverrides`) — so it can only be answered
/// while the pass is still ahead of the contact. Of the model's `ContactStatus` values
/// (`smithy_query(query="shape", shape="ContactStatus")`), these two are the ones before
/// a pass begins: from `PREPASS` on the ground station is already working the contact,
/// `CANCELLING` is a release under way, `CANCELLED`, `AWS_CANCELLED`, `COMPLETED`,
/// `FAILED`, `FAILED_TO_SCHEDULE` and `AWS_FAILED` are contacts that will never be
/// flown again, and `AVAILABLE` is a window that was offered and never reserved, so it
/// is not a contact this store holds.
///
/// CHOSEN gate: AWS documents `UpdateContact` only as "Updates a specific contact", so
/// which statuses still take an update is this emulator's rule, stated once here rather
/// than per operation.
pub const STATUSES_OPEN_TO_UPDATE: &[&str] = &["SCHEDULED", "SCHEDULING"];

/// The `ContactStatus` a released contact holds, and the one it reports while a release
/// of a pass already under way is being carried out — both the model's own
/// (`smithy_query(query="shape", shape="ContactStatus")`).
pub const CONTACT_CANCELLED: &str = "CANCELLED";

/// See [`CONTACT_CANCELLED`].
pub const CONTACT_CANCELLING: &str = "CANCELLING";

/// The `ContactStatus` a reservation is admitted in, and the one the scheduler moves it
/// to once the contact holds its place in the schedule — both the model's own
/// (`smithy_query(query="shape", shape="ContactStatus")`).
pub const CONTACT_SCHEDULING: &str = "SCHEDULING";

/// See [`CONTACT_SCHEDULING`].
pub const CONTACT_SCHEDULED: &str = "SCHEDULED";

/// The `ContactStatus` of a contact whose pre-pass period has begun, of one whose pass is
/// running, of one in its post-pass period and of one whose pass has finished — the
/// model's own values again, in the order a contact reaches them.
pub const CONTACT_PREPASS: &str = "PREPASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_PASS: &str = "PASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_POSTPASS: &str = "POSTPASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_COMPLETED: &str = "COMPLETED";

/// The `ContactStatus` of a contact window the service OFFERS and nothing has reserved —
/// the model's own value again (`smithy_query(query="shape", shape="ContactStatus")`).
///
/// No contact this store holds carries it: a reservation is admitted in
/// [`CONTACT_SCHEDULING`] and never returns to being merely offered, so a listing asked for
/// this status answers with the windows the pairing it names can still fly
/// ([`ComAmazonawsGroundstationGroundstationState::available_contact_windows`]) rather than
/// with anything selected from the store.
pub const CONTACT_AVAILABLE: &str = "AVAILABLE";

/// The statuses a contact moves through as the clock crosses its OWN timestamps, in
/// lifecycle order; it is also the set of statuses that move along it at all.
///
/// Every boundary is a timestamp the contact already carries and every read already
/// reports: the pre-pass period starts at `prePassStartTime`, "at which you receive a
/// CloudWatch event indicating an upcoming pass", the pass itself runs `startTime` to
/// `endTime`, and the post-pass period ends at `postPassEndTime`, "at which you receive a
/// CloudWatch event indicating the pass has finished" (`smithy_query(query="operation",
/// operation="DescribeContact")`). The lifecycle is therefore owed by the contact's own
/// window rather than by a span this emulator chooses, and a contact that is somewhere
/// else entirely — still [`CONTACT_SCHEDULING`], being released, or already in one of
/// [`TERMINAL_CONTACT_STATUSES`] — is not on it and is not carried along it.
pub const CONTACT_PASS_LIFECYCLE: &[&str] = &[
    CONTACT_SCHEDULED,
    CONTACT_PREPASS,
    CONTACT_PASS,
    CONTACT_POSTPASS,
    CONTACT_COMPLETED,
];

/// How far along [`CONTACT_PASS_LIFECYCLE`] `status` stands, or `None` when it is not on
/// that lifecycle at all.
pub fn pass_lifecycle_position(status: &str) -> Option<usize> {
    CONTACT_PASS_LIFECYCLE
        .iter()
        .position(|stage| *stage == status)
}

/// The `ContactStatus` values in which a cancel releases a reservation whose pass has not
/// begun, so the contact is `CANCELLED` outright.
///
/// `CancelContact` "cancels or stops a contact with a specified contact ID based on its
/// position in the contact lifecycle. For contacts that: Have yet to start, the contact
/// will be cancelled" (`smithy_query(query="operation", operation="CancelContact")`).
/// These are the statuses of a contact whose pass has yet to start: the reservation is
/// held and the ground station has not begun working it.
pub const STATUSES_CANCELLABLE_BEFORE_PASS: &[&str] = &["SCHEDULING", "SCHEDULED"];

/// The `ContactStatus` values in which a cancel STOPS a contact whose pass is under way:
/// "[for contacts that] have started but have yet to finish, the contact will be stopped"
/// (same doc as [`STATUSES_CANCELLABLE_BEFORE_PASS`]). From `PREPASS` on the ground
/// station is already working the contact, and through `POSTPASS` it has not finished, so
/// a cancel here is a stop — reported as [`CONTACT_CANCELLING`] while it is carried out.
pub const STATUSES_CANCELLABLE_DURING_PASS: &[&str] = &["PREPASS", "PASS", "POSTPASS"];

/// The `ContactStatus` values a contact can no longer leave: its pass is over, it was
/// released, or it will never be flown.
///
/// Source: the model's own enum (`smithy_query(query="shape", shape="ContactStatus")`).
/// Of its thirteen values these six are the ends of the lifecycle — a contact released by
/// the customer or by AWS (`CANCELLED`, `AWS_CANCELLED`), one whose pass finished
/// (`COMPLETED`), one that failed or that the service itself failed (`FAILED`,
/// `AWS_FAILED`) and a reservation that never became a contact (`FAILED_TO_SCHEDULE`).
/// Every other value is a contact still going somewhere: `SCHEDULING` and `SCHEDULED`
/// have their pass ahead of them, `PREPASS`, `PASS` and `POSTPASS` are in it,
/// `CANCELLING` is a release under way, and `AVAILABLE` is a window still on offer.
///
/// It is what makes a contact NON-terminal, which is the state the model's
/// `ResourceInUseException` is about: "the specified resource is in use by non-terminal
/// state contacts and cannot be modified or deleted" (`smithy_query(query="shape",
/// shape="ResourceInUseException")`).
pub const TERMINAL_CONTACT_STATUSES: &[&str] = &[
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "COMPLETED",
    "FAILED",
    "FAILED_TO_SCHEDULE",
];

/// Every `ContactStatus` in which a cancel still has a reservation to release, in
/// lifecycle order, as a refusal naming them spells them.
pub fn cancellable_statuses() -> String {
    STATUSES_CANCELLABLE_BEFORE_PASS
        .iter()
        .chain(STATUSES_CANCELLABLE_DURING_PASS)
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// One version of a contact: the configuration that version carries, and where the
/// version stands in the contact's history.
///
/// A contact is versioned because `UpdateContact` POSTs to
/// `/contact/{contactId}/versions` and answers a `versionId`
/// (`smithy_query(query="operation", operation="UpdateContact")`): an update does not
/// overwrite the contact's configuration, it appends a version which the service then
/// applies, superseding the version before it.
#[derive(Debug, Clone)]
pub struct ContactVersionRecord {
    pub version_id: i32,
    /// The satellite this version flies the contact against; `None` when neither this
    /// version nor the one it was based on named one.
    pub satellite_arn: Option<String>,
    /// The tracking overrides this version flies the contact with.
    pub tracking_overrides: Option<TrackingOverrides>,
    /// When the version was created ("time the contact version was created in UTC" —
    /// `ContactVersion$created`).
    pub created: DateTime,
    /// When it became the contact's active version ("a version is activated when it
    /// becomes the current active version of the contact"); `None` while it is still
    /// being applied, or if a newer version replaced it before it was.
    pub activated: Option<DateTime>,
    /// When a newer version replaced it ("a version is superseded when a newer version
    /// becomes active").
    pub superseded: Option<DateTime>,
    /// This version's [`ACTIVE_VERSION_STATUS`]-family status.
    pub status: String,
    /// The `clientToken` of the `UpdateContact` that appended this version, when it
    /// carried one. `clientToken` is `@idempotencyToken` on an `@idempotent` operation,
    /// so a retry carrying the same token answers with THIS version instead of
    /// appending another.
    pub client_token: Option<String>,
    /// The instant the service owes this version its `UPDATING` -> `ACTIVE` move; `None`
    /// once the move has been materialised, or for a version that was never pending.
    pub activates_at: Option<SystemTime>,
}

impl ContactVersionRecord {
    /// Version [`FIRST_VERSION_ID`], as `ReserveContact` mints it: the configuration the
    /// contact is reserved with is in force from the instant the reservation is
    /// accepted, so this version is `ACTIVE` and activated right away — nothing is still
    /// applying it and nothing precedes it.
    pub fn initial(
        created: DateTime,
        satellite_arn: Option<String>,
        tracking_overrides: Option<TrackingOverrides>,
    ) -> Self {
        Self {
            version_id: FIRST_VERSION_ID,
            satellite_arn,
            tracking_overrides,
            created,
            activated: Some(created),
            superseded: None,
            status: ACTIVE_VERSION_STATUS.to_string(),
            client_token: None,
            activates_at: None,
        }
    }
}

/// The `ContactStatus` values in which a contact's reservation holds NO antenna at the
/// ground station: the block of antenna time was never held on the account's behalf, or it
/// has been given back.
///
/// Source: the model's own enum, whose values are `SCHEDULING`, `FAILED_TO_SCHEDULE`,
/// `SCHEDULED`, `CANCELLED`, `AWS_CANCELLED`, `PREPASS`, `PASS`, `POSTPASS`, `COMPLETED`,
/// `FAILED`, `AVAILABLE`, `CANCELLING` and `AWS_FAILED` (`smithy_query(query="shape",
/// shape="ContactStatus")`). The five listed here are the ones where the antenna is not
/// held: a window merely offered and never reserved (`AVAILABLE`), a reservation that
/// never became a contact (`FAILED_TO_SCHEDULE`), one released by the customer or by AWS
/// (`CANCELLED`, `AWS_CANCELLED`) and one the service itself failed (`AWS_FAILED`). Every
/// other value is a contact whose reservation still holds its antenna — including
/// `CANCELLING`, whose release has not completed, and `FAILED`, whose antenna time was
/// still reserved.
///
/// It answers TWO different questions, which [`Contact::reserves_antenna`] and
/// [`Contact::holds_antenna_time`] keep apart: whether the antenna is held right now (the
/// status alone decides it), and whether the account's minutes were spent (a pass already
/// under way spent them, and a release cannot give them back —
/// [`Contact::antenna_time_spent`]).
///
/// CHOSEN classification: the API documents the member only as "total scheduled minutes",
/// so which statuses release their minutes is this emulator's rule, applied in one place
/// rather than per operation.
pub const STATUSES_WITHOUT_ANTENNA_TIME: &[&str] = &[
    "AVAILABLE",
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "FAILED_TO_SCHEDULE",
];

impl Contact {
    /// Whether this contact's reservation holds its antenna at the ground station RIGHT
    /// NOW, so that antenna is not free for another pass over the same window and the
    /// reservation is one the ground station reports: its status is not one of
    /// [`STATUSES_WITHOUT_ANTENNA_TIME`].
    ///
    /// The current status is the whole answer. Antenna time a pass already used
    /// ([`Contact::antenna_time_spent`]) stays spent for the account's usage, but it is not
    /// a hold on the antenna: a reservation that has been released holds nothing, so the
    /// antenna it was on is free again even when the pass it was stopped mid-way through
    /// had already used some of its time.
    pub fn reserves_antenna(&self) -> bool {
        !STATUSES_WITHOUT_ANTENNA_TIME.contains(&self.contact_status.as_str())
    }

    /// Whether this contact holds (or held) antenna time for the account, so its minutes
    /// count against the usage of the month it starts in: its reservation still holds its
    /// antenna ([`Contact::reserves_antenna`]), or its pass already spent the antenna time
    /// ([`Contact::antenna_time_spent`]) and no later status move gives it back.
    ///
    /// A BILLING question, not a placement one — see [`Contact::reserves_antenna`] for
    /// which antennas are held.
    pub fn holds_antenna_time(&self) -> bool {
        self.antenna_time_spent || self.reserves_antenna()
    }

    /// Whether a cancel still has something to release here: the reservation is live and
    /// the pass has not finished, so the contact is either cancelled before its pass or
    /// stopped during it.
    ///
    /// Everything else holds no reservation a cancel could release — a contact already
    /// released (`CANCELLED`, `AWS_CANCELLED`), one whose pass is over or will never be
    /// flown (`COMPLETED`, `FAILED`, `FAILED_TO_SCHEDULE`, `AWS_FAILED`) and a window
    /// merely offered (`AVAILABLE`). `CANCELLING` is not here either, but for the
    /// opposite reason: a release is already under way, which `CancelContact` answers
    /// without starting a second one.
    pub fn is_cancellable(&self) -> bool {
        let status = self.contact_status.as_str();
        STATUSES_CANCELLABLE_BEFORE_PASS.contains(&status)
            || STATUSES_CANCELLABLE_DURING_PASS.contains(&status)
    }

    /// Whether this contact is still going somewhere: its status is not one of
    /// [`TERMINAL_CONTACT_STATUSES`], so what it is flown with is still a live dependency.
    pub fn is_non_terminal(&self) -> bool {
        !TERMINAL_CONTACT_STATUSES.contains(&self.contact_status.as_str())
    }

    /// Whether anything of this contact is STILL TO BE FLOWN: its pass can still happen
    /// and its reservation has not been released, so a configuration change the service
    /// is applying still has a pass to take force for.
    ///
    /// A contact already in one of [`TERMINAL_CONTACT_STATUSES`] will never be flown
    /// again, and so will a contact whose release is being carried out
    /// ([`CONTACT_CANCELLING`]): the reservation was given up the moment the cancel was
    /// accepted, and the stop only finishes giving it back. It is the question
    /// `UpdateContact` asks before it STARTS a version change
    /// ([`STATUSES_OPEN_TO_UPDATE`]) — asked here of a change already in flight, so one
    /// the service cannot start on a contact is not one it goes on applying to it either.
    pub fn is_still_to_be_flown(&self) -> bool {
        self.is_non_terminal() && self.contact_status != CONTACT_CANCELLING
    }

    /// Whether this contact is flown against the satellite `satellite_arn` names.
    ///
    /// The version in force answers it, and so does the version still being applied: an
    /// `UPDATING` version is the configuration the contact is about to be flown with, so a
    /// satellite either version names is one this contact depends on.
    pub fn flies_satellite(&self, satellite_arn: &str) -> bool {
        [self.active_version(), self.latest_version()]
            .into_iter()
            .flatten()
            .any(|version| version.satellite_arn.as_deref() == Some(satellite_arn))
    }

    /// Whether this contact's program track points its antenna at the ephemeris
    /// `ephemeris_id` names.
    ///
    /// Read from the version in force and from the version still being applied, for the
    /// same reason as [`Contact::flies_satellite`]: an `UPDATING` version is the
    /// configuration the contact is about to be flown with, so an ephemeris either version
    /// names is one this contact depends on.
    pub fn program_tracks_ephemeris(&self, ephemeris_id: &str) -> bool {
        [self.active_version(), self.latest_version()]
            .into_iter()
            .flatten()
            .filter_map(|version| version.tracking_overrides.as_ref())
            .filter_map(program_track_reference)
            .any(|reference| reference.ephemeris_id == ephemeris_id)
    }

    /// Release this contact's reservation, from whichever side of its pass the cancel
    /// arrived on. Only called for a contact [`Contact::is_cancellable`] admits.
    ///
    /// A cancel ends the contact: whichever side of its pass it arrived on, the service
    /// owes the contact no further move of its own afterwards, so EVERY move still owed
    /// to it is withdrawn here — the scheduler's `SCHEDULING` -> `SCHEDULED` move and the
    /// `UPDATING` -> `ACTIVE` move of a version an update left in flight
    /// ([`Contact::withdraw_owed_version_moves`]). The release the cancel starts is the
    /// one move that survives it.
    ///
    /// A contact whose pass has yet to start is `CANCELLED` outright: there is nothing to
    /// stop. One whose pass is under way is STOPPED instead — it reports [`CONTACT_CANCELLING`]
    /// while the stop is carried out and the service owes it its move to
    /// [`CONTACT_CANCELLED`] [`CANCELLATION_SETTLES_AFTER`] from now (materialised by
    /// `settle_due_contacts`). That pass already had the antenna on the account's behalf,
    /// so the stop also records the antenna time as spent: a release cannot give back
    /// time the pass has already used. Those spent minutes are a BILLING record only — the
    /// released reservation holds no antenna once the stop has settled
    /// ([`Contact::reserves_antenna`]).
    pub fn begin_cancellation(&mut self, now: DateTime) {
        if STATUSES_CANCELLABLE_DURING_PASS.contains(&self.contact_status.as_str()) {
            self.antenna_time_spent = true;
            self.contact_status = CONTACT_CANCELLING.to_string();
            self.cancellation_settles_at = Some(SystemTime::now() + CANCELLATION_SETTLES_AFTER);
        } else {
            self.contact_status = CONTACT_CANCELLED.to_string();
            self.cancellation_settles_at = None;
        }
        self.scheduling_settles_at = None;
        self.withdraw_owed_version_moves();
        self.last_updated = now;
    }

    /// The stage of its own pass the clock has carried this contact to at `now` (epoch
    /// seconds): [`CONTACT_SCHEDULED`] while its pre-pass period is still ahead,
    /// [`CONTACT_PREPASS`] from `prePassStartTime`, [`CONTACT_PASS`] from `startTime`,
    /// [`CONTACT_POSTPASS`] from `endTime` and [`CONTACT_COMPLETED`] from
    /// `postPassEndTime`.
    ///
    /// A boundary two of the contact's timestamps share is a stage of no duration, which
    /// the contact therefore never reports: a mission profile that widens the window by
    /// nothing gives the contact no pre-pass period, so such a contact goes from
    /// scheduled straight into its pass.
    pub fn pass_stage(&self, now: f64) -> &'static str {
        if now >= self.post_pass_end_time.as_secs_f64() {
            CONTACT_COMPLETED
        } else if now >= self.end_time.as_secs_f64() {
            CONTACT_POSTPASS
        } else if now >= self.start_time.as_secs_f64() {
            CONTACT_PASS
        } else if now >= self.pre_pass_start_time.as_secs_f64() {
            CONTACT_PREPASS
        } else {
            CONTACT_SCHEDULED
        }
    }

    /// Carry this contact to the stage of [`CONTACT_PASS_LIFECYCLE`] its own timestamps
    /// have already made due at `now` (epoch seconds), and answer whether that moved it.
    ///
    /// The move only ever goes FORWARD along the lifecycle, and only for a contact that is
    /// on it: a contact whose status the clock cannot explain — one still being scheduled
    /// or released, one already terminal, or a window merely offered — keeps the status it
    /// holds. A contact that has reached [`CONTACT_PREPASS`] or beyond has had the antenna
    /// on the account's behalf, so the move also records the antenna time as spent
    /// ([`Contact::antenna_time_spent`]), which no later status move gives back.
    pub fn advance_pass_stage(&mut self, now: f64) -> bool {
        let Some(reached) = pass_lifecycle_position(&self.contact_status) else {
            return false;
        };
        let stage = self.pass_stage(now);
        let Some(due) = pass_lifecycle_position(stage) else {
            return false;
        };
        if due <= reached {
            return false;
        }
        self.contact_status = stage.to_string();
        self.antenna_time_spent = true;
        true
    }

    /// The whole minutes of antenna time this contact is scheduled for: its own window,
    /// `startTime` to `endTime`, rounded UP to the next whole minute.
    ///
    /// The pre- and post-pass widening is deliberately not part of it — those spans are
    /// the set-up and tear-down around the pass, while the antenna time is the window the
    /// reservation itself names. CHOSEN rounding: minute usage is reported as an
    /// `Integer` of minutes (`smithy_query(query="operation",
    /// operation="GetMinuteUsage")`), and a part minute of a pass is still a minute the
    /// account has the antenna for, so it counts as a whole one.
    pub fn scheduled_minutes(&self) -> i32 {
        let seconds = (self.end_time.as_secs_f64() - self.start_time.as_secs_f64()).max(0.0);
        (seconds / 60.0).ceil().min(f64::from(i32::MAX)) as i32
    }

    /// The version in force: the newest one that has been activated, which is the
    /// configuration the contact is flown with. A version still being applied
    /// (`UPDATING`) is not in force yet, so it is not this one.
    pub fn active_version(&self) -> Option<&ContactVersionRecord> {
        self.versions
            .iter()
            .rev()
            .find(|version| version.status == ACTIVE_VERSION_STATUS)
    }

    /// The newest version of this contact, whatever its status — the one an update is
    /// based on and the one a `versionId` answer names.
    pub fn latest_version(&self) -> Option<&ContactVersionRecord> {
        self.versions.last()
    }

    /// The `versionId` this contact's operations answer with: its newest version's.
    pub fn latest_version_id(&self) -> i32 {
        self.latest_version()
            .map_or(FIRST_VERSION_ID, |version| version.version_id)
    }

    /// The satellite the contact is flown against: the active version's.
    ///
    /// The ACTIVE version, not the latest: a version still being applied is not in force
    /// yet, so a read reports the configuration the contact would fly with right now.
    /// [`Contact::append_version`] bases its merge on the LATEST version instead, so
    /// chained updates compose rather than each one starting from the version in force.
    pub fn satellite_arn(&self) -> Option<&str> {
        self.active_version()
            .and_then(|version| version.satellite_arn.as_deref())
    }

    /// The tracking overrides the contact is flown with: the active version's (see
    /// [`Contact::satellite_arn`] on active versus latest).
    pub fn tracking_overrides(&self) -> Option<&TrackingOverrides> {
        self.active_version()
            .and_then(|version| version.tracking_overrides.as_ref())
    }

    /// This contact's version record, as `DescribeContact` and `ListContacts` report
    /// it: its NEWEST version — the one in force, or the one being applied while an
    /// update settles, which is what a caller polls to watch an update land.
    ///
    /// INFALLIBLE, because a contact always has at least one version: a reservation
    /// mints [`FIRST_VERSION_ID`] and nothing removes a version. Were that invariant
    /// ever broken the answer falls back to the contact's own first version rather than
    /// reporting no version at all.
    pub fn version(&self) -> ContactVersion {
        match self.latest_version() {
            Some(record) => self.version_record(record),
            None => self.version_record(&ContactVersionRecord::initial(self.created, None, None)),
        }
    }

    /// `record` as a `ContactVersion` response member.
    pub fn version_record(&self, record: &ContactVersionRecord) -> ContactVersion {
        ContactVersion {
            version_id: Some(record.version_id),
            created: Some(record.created),
            activated: record.activated,
            superseded: record.superseded,
            // A version that has been superseded was last touched when it was
            // superseded; the newest version is the one every later change to the
            // contact lands on, so its lastUpdated is the contact's own.
            last_updated: Some(record.superseded.unwrap_or(self.last_updated)),
            status: Some(record.status.clone()),
            // Nothing in this emulator fails applying a version, so the members
            // reporting a failed update carry nothing.
            failure_codes: None,
            failure_message: None,
        }
    }

    /// This contact's whole version HISTORY, as `ListContactVersions` reports it: every
    /// version the store holds, oldest first.
    ///
    /// The stored sequence is append-only and each appended version takes the next
    /// `versionId` ([`Contact::append_version`]), so its own order IS ascending
    /// `versionId` — which is the order the listing pages over, and the order a version
    /// history reads in.
    pub fn version_history(&self) -> Vec<ContactVersion> {
        self.versions
            .iter()
            .map(|record| self.version_record(record))
            .collect()
    }

    /// Append a version carrying `satellite_arn` and `tracking_overrides`, and answer
    /// its `versionId`.
    ///
    /// A member the update does not name keeps the value the newest version already
    /// carries, so an update that names only one of them does not drop the other. The
    /// version is admitted `UPDATING`: the service owes it its move to `ACTIVE`
    /// [`VERSION_ACTIVATES_AFTER`] from now, which is when it takes force and supersedes
    /// the version before it (see [`Contact::apply_due_versions`]).
    pub fn append_version(
        &mut self,
        satellite_arn: Option<String>,
        tracking_overrides: Option<TrackingOverrides>,
        client_token: Option<String>,
        now: DateTime,
    ) -> i32 {
        let base = self.latest_version();
        let version_id = base.map_or(FIRST_VERSION_ID, |version| version.version_id + 1);
        let satellite_arn =
            satellite_arn.or_else(|| base.and_then(|version| version.satellite_arn.clone()));
        let tracking_overrides = tracking_overrides
            .or_else(|| base.and_then(|version| version.tracking_overrides.clone()));
        self.versions.push(ContactVersionRecord {
            version_id,
            satellite_arn,
            tracking_overrides,
            created: now,
            activated: None,
            superseded: None,
            status: UPDATING_VERSION_STATUS.to_string(),
            client_token,
            activates_at: Some(SystemTime::now() + VERSION_ACTIVATES_AFTER),
        });
        self.last_updated = now;
        version_id
    }

    /// The version `client_token` already appended to this contact, when that token has
    /// been seen: `clientToken` is `@idempotencyToken`, so a retry carrying it answers
    /// with the version it minted the first time rather than appending another.
    pub fn version_minted_by(&self, client_token: &str) -> Option<&ContactVersionRecord> {
        self.versions
            .iter()
            .find(|version| version.client_token.as_deref() == Some(client_token))
    }

    /// Materialise the `UPDATING` -> `ACTIVE` move of every version the clock has
    /// already made due: the newest due version takes force, every version before it is
    /// superseded, and the contact is flown with the new one from here on.
    ///
    /// The move was owed at this instant whether or not anybody called, so applying it
    /// on a read changes nothing a later caller could otherwise have seen.
    ///
    /// A version is applied only while there is still a pass to fly it with
    /// ([`Contact::is_still_to_be_flown`]): a contact that was released or that will never
    /// be flown again is owed no version move, so the moves it is still carrying are
    /// withdrawn instead of materialised, and every later read reports the versions and
    /// the `lastUpdated` the end of the contact left behind.
    pub fn apply_due_versions(&mut self) {
        if !self.is_still_to_be_flown() {
            self.withdraw_owed_version_moves();
            return;
        }

        let now = SystemTime::now();
        let Some(activating) = self
            .versions
            .iter()
            .rposition(|version| version.activates_at.is_some_and(|due| due <= now))
        else {
            return;
        };

        let stamp = now_timestamp();
        for (position, version) in self.versions.iter_mut().enumerate() {
            if position == activating {
                version.status = ACTIVE_VERSION_STATUS.to_string();
                version.activated = Some(stamp);
                version.activates_at = None;
            } else if position < activating && version.superseded.is_none() {
                // Every older version is replaced — including one still pending, which
                // a newer version overtook before it could take force.
                version.status = SUPERSEDED_VERSION_STATUS.to_string();
                version.superseded = Some(stamp);
                version.activates_at = None;
            }
        }
        self.last_updated = stamp;
    }

    /// Withdraw every `UPDATING` -> `ACTIVE` version move the service still owes this
    /// contact, leaving each version's own record exactly where the withdrawal found it: a
    /// version that was still being applied is never applied now, so it keeps the status,
    /// the absent `activated` and the absent `superseded` it held.
    ///
    /// Called wherever the contact stops being one with a pass still to fly, so no later
    /// settle can move a version of it (see [`Contact::apply_due_versions`]).
    pub fn withdraw_owed_version_moves(&mut self) {
        for version in &mut self.versions {
            version.activates_at = None;
        }
    }

    /// `tags` the way a response carries them: always the map, empty when the
    /// resource carries no tag.
    pub fn response_tags(&self) -> Option<HashMap<String, String>> {
        response_tags(&self.tags)
    }
}
