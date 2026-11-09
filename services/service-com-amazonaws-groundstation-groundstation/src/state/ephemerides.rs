// SPDX-License-Identifier: CC0-1.0

//! The ephemerides this service holds, and the trajectories read from them.
//!
//! One concern of [`super`]: an ephemeris as `CreateEphemeris` stored it ([`Ephemeris`]),
//! the statuses and the precedence that decide which one is IN FORCE for a satellite, the
//! validation its supplied data is held to, the program-track reference a contact points
//! its antenna with, and the passes the pointing data puts above a ground station's
//! horizon.

use super::*;
use crate::ops::types::EphemerisResponseData;

/// The status an ephemeris carries while the service is still validating the data it was
/// created with.
///
/// This and the four constants below are values of the model's own `EphemerisStatus` enum,
/// whose members are `VALIDATING`, `INVALID`, `ERROR`, `ENABLED`, `DISABLED` and `EXPIRED`
/// (`smithy_query(query="shape", shape="EphemerisStatus")`). `ERROR` is not among them:
/// the model documents it for an ephemeris the service itself failed to process, and
/// nothing in this emulator fails that way, so no ephemeris here reaches it.
pub const EPHEMERIS_VALIDATING: &str = "VALIDATING";

/// The status of an ephemeris whose data validated and which was asked to be enabled.
pub const EPHEMERIS_ENABLED: &str = "ENABLED";

/// The status of an ephemeris whose data validated and which was not asked to be enabled.
pub const EPHEMERIS_DISABLED: &str = "DISABLED";

/// The status of an ephemeris whose supplied data did not validate.
pub const EPHEMERIS_INVALID: &str = "INVALID";

/// The status of an ephemeris whose expiration time has passed: `CreateEphemeris`
/// documents `expirationTime` as "an overall expiration time for the ephemeris in UTC,
/// after which it will become `EXPIRED`" (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="expirationTime")`).
pub const EPHEMERIS_EXPIRED: &str = "EXPIRED";

/// What this service answers when an ephemeris a request addresses by its `{ephemerisId}`
/// does not exist: a fixed literal that discloses neither the id nor the ARN that was
/// asked for, spelled the way this service already answers for a missing mission profile
/// (see [`MISSION_PROFILE_NOT_FOUND`]).
///
/// It lives here rather than in one handler because every operation addressed at an
/// ephemeris by that label answers it, so it is one text of the service.
pub const EPHEMERIS_NOT_FOUND: &str = "Ephemeris not found";

/// What this service answers while the ephemeris a delete addressed is still in use: the
/// refusal names the contacts that hold it, which is what tells a caller what has to be
/// flown or released first.
///
/// The status (409) and the error code (`ResourceInUseException`) are the contract; this is
/// the wording that goes with them, and the ids in it are the ones the store holds rather
/// than anything echoed from the request — the same composition this service already uses
/// for a config a mission profile still references ([`config_in_use_message`]).
pub fn ephemeris_in_use_message(contact_ids: &[String]) -> String {
    resource_in_use_text("Ephemeris", "contacts", contact_ids)
}

/// What this service answers while a WRITE onto an ephemeris would take the trajectory out
/// from under a pass still to be flown: the refusal names the contacts that would have lost
/// it, which is what tells a caller what has to be flown or released first.
///
/// The status (431) and the error code (`InvalidParameterException`) are the contract —
/// `UpdateEphemeris` models no `ResourceInUseException` (`smithy_query(query="operation",
/// operation="UpdateEphemeris")`), so the in-use refusal is the bad-parameter one this
/// service's other writers held by an in-use resource already answer
/// ([`mission_profile_in_use_message`], [`dataflow_endpoint_group_in_use_message`]). The
/// sentence is the one template every in-use refusal of this service is composed from
/// ([`resource_in_use_text_for`]), in the action AWS's own in-use text pairs with the
/// delete's — "cannot be modified or deleted" — and the ids in it are the ones the store
/// holds rather than anything echoed from the request.
pub fn ephemeris_in_use_for_update_message(contact_ids: &[String]) -> String {
    resource_in_use_text_for("Ephemeris", "modified", "contacts", contact_ids)
}

/// Why an ephemeris whose supplied data describes a backwards-running validity range did
/// not validate: a value of the model's `EphemerisInvalidReason` enum, whose members are
/// `METADATA_INVALID`, `TIME_RANGE_INVALID`, `TRAJECTORY_INVALID`, `KMS_KEY_INVALID` and
/// `VALIDATION_ERROR` (`smithy_query(query="shape", shape="EphemerisInvalidReason")`).
pub const EPHEMERIS_TIME_RANGE_INVALID: &str = "TIME_RANGE_INVALID";

/// Why an ephemeris whose supplied pointing data describes no usable trajectory did not
/// validate: the `EphemerisInvalidReason` the model documents as "provided ephemeris defines
/// invalid spacecraft trajectory" (`smithy_query(query="shape",
/// shape="EphemerisInvalidReason")`), which is what the pointing data itself being unusable
/// is — as against [`EPHEMERIS_TIME_RANGE_INVALID`], documented for the "start, end, or
/// expiration time(s)" the data declares.
pub const EPHEMERIS_TRAJECTORY_INVALID: &str = "TRAJECTORY_INVALID";

/// The priority an ephemeris is created with when the request names none: the model's own
/// documentation of `CreateEphemerisRequest$priority` — "Higher numbers take precedence.
/// The default is 1. Must be 1 or greater." (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="priority")`). The member carries no `@default`
/// trait, so the documented default is applied here instead.
pub const DEFAULT_EPHEMERIS_PRIORITY: i32 = 1;

/// The smallest priority a customer ephemeris may carry, from the same documented sentence
/// as [`DEFAULT_EPHEMERIS_PRIORITY`]: "Must be 1 or greater."
///
/// `CreateEphemeris` takes a `CustomerEphemerisPriority` (range 1..99999), so the model
/// itself refuses anything below this there; `UpdateEphemeris` takes an
/// `EphemerisPriority` (range 0..99999 — `smithy_query(query="shape",
/// shape="EphemerisPriority")`), whose range admits a value the member's own documentation
/// rules out, so the bound lives here for the handler that has to apply it.
pub const MIN_EPHEMERIS_PRIORITY: i32 = 1;

/// Whether an ephemeris created without an `enabled` member is enabled.
///
/// FALSE: the member exists to ask for enabling — "Set to `true` to enable the ephemeris
/// after validation. Set to `false` to keep it disabled." (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="enabled")`) — so a request that asks for
/// neither has not asked to enable it, and the ephemeris validates to
/// [`EPHEMERIS_DISABLED`]. `UpdateEphemeris` then enables it, where `enabled` is required.
pub const EPHEMERIS_ENABLED_BY_DEFAULT: bool = false;

/// The `EphemerisSource` of an ephemeris this account supplied itself: a value of the
/// model's own `EphemerisSource` enum, whose members are `CUSTOMER_PROVIDED` and
/// `SPACE_TRACK` (`smithy_query(query="shape", shape="EphemerisSource")`). The other
/// value belongs to the default ephemeris AWS generates from Space Track, which no
/// request of this API creates.
pub const EPHEMERIS_SOURCE_CUSTOMER_PROVIDED: &str = "CUSTOMER_PROVIDED";

/// How a satellite reports `ephemeris`, one this account supplied, as the ephemeris its
/// trajectory is computed from.
///
/// Each member follows what the model documents for it (`smithy_query(query="shape",
/// shape="EphemerisMetaData")`): `ephemerisId` is the "UUID of a customer-provided
/// ephemeris", `name` is the human-readable identifier the create named, and `epoch` is
/// "the epoch of a default, ephemeris from Space Track" which "is not populated for
/// customer-provided ephemerides".
pub fn customer_ephemeris_metadata(ephemeris: &Ephemeris) -> EphemerisMetaData {
    EphemerisMetaData {
        source: EPHEMERIS_SOURCE_CUSTOMER_PROVIDED.to_string(),
        ephemeris_id: Some(ephemeris.ephemeris_id.clone()),
        epoch: None,
        name: Some(ephemeris.name.clone()),
    }
}

/// The ephemeris of two held for the same satellite that its trajectory is computed from:
/// [`::std::cmp::Ordering::Greater`] when `left` takes precedence over `right`.
///
/// The highest `priority` wins: "A priority score that determines which ephemeris to use
/// when multiple ephemerides overlap. Higher numbers take precedence."
/// (`smithy_query(query="doc", shape="CreateEphemerisRequest", member="priority")`).
///
/// The tie-breaks are CHOSEN, so the answer is total and stable whatever order the store
/// iterates in: of equal priority the most recently created wins (the later upload is the
/// newer trajectory), and of equal creation instant the larger `ephemerisId`.
///
/// This is the ONE order every caller that asks the question takes, both the satellite
/// coupling ([`ComAmazonawsGroundstationGroundstationState::ephemerides_in_force`]) and the
/// availability search that reads a pass's geometry from the ephemeris in force for the
/// instants it covers ([`ComAmazonawsGroundstationGroundstationState::pointing_passes`]).
fn ephemeris_precedence(left: &Ephemeris, right: &Ephemeris) -> ::std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| {
            left.creation_time
                .as_secs_f64()
                .total_cmp(&right.creation_time.as_secs_f64())
        })
        .then_with(|| left.ephemeris_id.cmp(&right.ephemeris_id))
}

/// Which `EphemerisType` an `EphemerisData` union member selects.
///
/// An ephemeris's type is NOT a request field: it is implied by whichever member of the
/// union the caller set, and it is what `ListEphemerides` reports as `ephemerisType` and
/// filters on. The three pairs are the model's own: `smithy_query(query="shape",
/// shape="EphemerisData")` lists the members and `shape="EphemerisType"` the enum values.
/// The fourth value, `SERVICE_MANAGED`, belongs to an ephemeris AWS supplies itself (the
/// Space Track default), which no customer request creates.
pub fn ephemeris_type_of(data: &EphemerisData) -> &'static str {
    match data {
        EphemerisData::Tle(_) => EPHEMERIS_TYPE_TLE,
        EphemerisData::Oem(_) => EPHEMERIS_TYPE_OEM,
        EphemerisData::azEl(_) => EPHEMERIS_TYPE_AZ_EL,
    }
}

/// The `EphemerisType` of an ephemeris supplied as a two-line element set.
///
/// This and the two below are values of the model's own `EphemerisType` enum, whose values
/// are `TLE`, `OEM`, `AZ_EL` and `SERVICE_MANAGED` (`smithy_query(query="shape",
/// shape="EphemerisType")`). They are named here because two rules read them: the type an
/// ephemeris takes from the data it was created with ([`ephemeris_type_of`]) and the type a
/// program track's reference requires ([`program_track_reference`]).
pub const EPHEMERIS_TYPE_TLE: &str = "TLE";

/// The `EphemerisType` of an ephemeris supplied as a CCSDS orbit ephemeris message.
pub const EPHEMERIS_TYPE_OEM: &str = "OEM";

/// The `EphemerisType` of an ephemeris supplied as azimuth/elevation pointing segments.
pub const EPHEMERIS_TYPE_AZ_EL: &str = "AZ_EL";

/// The ephemeris a contact's `trackingOverrides` point its antenna at.
#[derive(Debug, Clone)]
pub struct ProgramTrackReference<'a> {
    /// The `ephemerisId` the overrides name.
    pub ephemeris_id: &'a str,
    /// The `EphemerisType` the member carrying that reference requires.
    pub required_type: &'static str,
    /// The request member the reference was named in, as a refusal reports it.
    pub parameter_name: String,
}

/// The ephemeris `overrides` point the antenna at, or `None` when they override the
/// tracking configuration without naming a program track.
///
/// A program track flies the antenna from a SUPPLIED trajectory, and the member of the
/// union that carries the reference is what says which form that trajectory is in: the
/// three members are documented "Program track settings for AzElEphemeris / OEMEphemeris /
/// TLEEphemeris" and each one's `ephemerisId` as "Unique identifier of the azimuth
/// elevation / OEM / TLE ephemeris" (`smithy_query(query="shape",
/// shape="ProgramTrackSettings")`). So the member fixes the `EphemerisType` the ephemeris
/// it names has to carry — the same type [`ephemeris_type_of`] derives from the data an
/// ephemeris was created with.
pub fn program_track_reference(overrides: &TrackingOverrides) -> Option<ProgramTrackReference<'_>> {
    let (ephemeris_id, required_type, member) = match overrides.program_track_settings.as_ref()? {
        ProgramTrackSettings::azEl(az_el) => (
            az_el.ephemeris_id.as_str(),
            EPHEMERIS_TYPE_AZ_EL,
            "azEl",
        ),
        ProgramTrackSettings::Oem(oem) => (oem.ephemeris_id.as_str(), EPHEMERIS_TYPE_OEM, "oem"),
        ProgramTrackSettings::Tle(tle) => (tle.ephemeris_id.as_str(), EPHEMERIS_TYPE_TLE, "tle"),
    };
    Some(ProgramTrackReference {
        ephemeris_id,
        required_type,
        parameter_name: format!("trackingOverrides.programTrackSettings.{member}.ephemerisId"),
    })
}

/// Why the program track a write takes cannot be flown, as the modelled refusal to answer
/// with.
///
/// The two faults are different conditions and this service answers them differently, the
/// same way it answers them for a config reference
/// ([`ComAmazonawsGroundstationGroundstationState::typed_reference_fault`]).
#[derive(Debug, Clone)]
pub enum ProgramTrackFault {
    /// The `ephemerisId` names no ephemeris this account holds: there is nothing to
    /// reference, so the resource is MISSING (`ResourceNotFoundException`, HTTP 434).
    Absent(ResourceNotFoundException),
    /// It names an ephemeris that EXISTS, of another `EphemerisType` than the member
    /// carrying the reference requires: a BAD PARAMETER (`InvalidParameterException`,
    /// HTTP 431), since the reference resolved and no amount of waiting changes the form
    /// of the trajectory it points at.
    WrongType(InvalidParameterException),
}

/// What this service answers for a program track whose `ephemerisId` resolved to an
/// ephemeris of another `EphemerisType` than the member naming it requires: the reference
/// resolved, so nothing is missing — what is wrong is the form of the trajectory it points
/// at, and the text states the type that position does take, composed as this service
/// already states the type a config reference's position takes
/// ([`wrong_config_type_message`]).
pub fn wrong_ephemeris_type_message(required_type: &str) -> String {
    format!("Invalid ephemerisId provided. Ephemeris Type must be of type \"{required_type}\".")
}

/// The Amazon S3 object `data` was supplied from, when it names one rather than carrying
/// the data inline — what `ListEphemerides` reports as an item's `sourceS3Object`.
pub fn ephemeris_source_s3_object(data: &EphemerisData) -> Option<S3Object> {
    match data {
        EphemerisData::Tle(tle) => tle.s3_object.clone(),
        EphemerisData::Oem(oem) => oem.s3_object.clone(),
        EphemerisData::azEl(az_el) => match &az_el.data {
            AzElSegmentsData::s3Object(object) => Some(object.clone()),
            AzElSegmentsData::azElData(_) => None,
        },
    }
}

/// The validity ranges the data `data` carries, as `(start, end)` epoch seconds.
///
/// A TLE set declares one `validTimeRange` per entry and an az/el set one per segment
/// (`smithy_query(query="shape", shape="TLEData")` / `shape="AzElSegment"`). An OEM body
/// carries its validity inside the CCSDS document this emulator stores verbatim rather
/// than parses, and data supplied from an Amazon S3 object is not fetched here, so both
/// declare none.
fn ephemeris_validity_ranges(data: &EphemerisData) -> Vec<(f64, f64)> {
    match data {
        EphemerisData::Tle(tle) => tle
            .tle_data
            .iter()
            .flatten()
            .map(|entry| {
                (
                    entry.valid_time_range.start_time.as_secs_f64(),
                    entry.valid_time_range.end_time.as_secs_f64(),
                )
            })
            .collect(),
        EphemerisData::Oem(_) => Vec::new(),
        EphemerisData::azEl(az_el) => match &az_el.data {
            AzElSegmentsData::azElData(segments) => segments
                .az_el_segment_list
                .iter()
                .map(|segment| {
                    (
                        segment.valid_time_range.start_time.as_secs_f64(),
                        segment.valid_time_range.end_time.as_secs_f64(),
                    )
                })
                .collect(),
            AzElSegmentsData::s3Object(_) => Vec::new(),
        },
    }
}

/// The last instant the data `data` supplies is valid for, when the data declares its own
/// validity ([`ephemeris_validity_ranges`]); `None` when it declares none.
///
/// This is the expiration an ephemeris created without an explicit `expirationTime`
/// takes: the data stops being usable when its own validity ends.
pub fn ephemeris_validity_end(data: &EphemerisData) -> Option<DateTime> {
    ephemeris_validity_ranges(data)
        .into_iter()
        .map(|(_, end)| end)
        .reduce(f64::max)
        .map(DateTime::from_secs_f64)
}

/// The azimuth-elevation segments `data` carries INLINE, or `None` when it carries none to
/// examine — because it is of another type, or names an Amazon S3 object this emulator does
/// not fetch.
pub(super) fn inline_az_el_segments(data: &EphemerisData) -> Option<&AzElSegments> {
    match data {
        EphemerisData::azEl(az_el) => match &az_el.data {
            AzElSegmentsData::azElData(segments) => Some(segments),
            AzElSegmentsData::s3Object(_) => None,
        },
        EphemerisData::Tle(_) | EphemerisData::Oem(_) => None,
    }
}

/// Why the data `data` supplies cannot be used, as an `EphemerisInvalidReason`, or `None`
/// when nothing in it is wrong.
///
/// Data the model states a rule for and that breaks it describes no usable trajectory, so it
/// is refused rather than enabled — every such rule, not one of them:
///
/// * a validity range must run forwards: one whose end does not follow its start describes
///   no interval at all;
/// * the segments of an az/el set "must be in chronological order with no overlaps"
///   (`smithy_query(query="doc", shape="AzElSegments", member="azElSegmentList")`), so each
///   one's validity begins no earlier than the one before it ends — two segments that touch
///   at an instant describe one continuous span and do not overlap;
/// * a segment's points "must be in ascending order with no duplicates" by `dt`
///   (`smithy_query(query="doc", shape="TimeAzEl", member="dt")`), which is what makes them
///   a trajectory rather than a bag of directions;
/// * an angle must lie inside the range its unit documents ([`angle_ranges`]).
///
/// The first two are faults of the times the data declares and the last two of the pointing
/// data itself, which is how the verdict is spelled: [`EPHEMERIS_TIME_RANGE_INVALID`] or
/// [`EPHEMERIS_TRAJECTORY_INVALID`], following what the model documents each reason for.
///
/// This is the verdict of the validation `CreateEphemeris` starts, applied when that
/// validation falls due ([`EPHEMERIS_VALIDATION_SETTLES_AFTER`]).
pub fn ephemeris_invalid_reason(data: &EphemerisData) -> Option<&'static str> {
    if ephemeris_validity_ranges(data)
        .iter()
        .any(|(start, end)| end <= start)
    {
        return Some(EPHEMERIS_TIME_RANGE_INVALID);
    }
    let segments = inline_az_el_segments(data)?;
    if segments_out_of_order(segments) {
        return Some(EPHEMERIS_TIME_RANGE_INVALID);
    }
    pointing_data_unusable(segments).then_some(EPHEMERIS_TRAJECTORY_INVALID)
}

/// Whether the segments of `segments` are NOT in chronological order with no overlaps: true
/// when any of them begins before the one before it ends.
///
/// One comparison answers both halves of that rule — a segment placed out of order begins
/// before its predecessor ends just as an overlapping one does — and it is a comparison of
/// the validity ranges the segments DECLARE, which is the span each one's data is declared to
/// cover.
fn segments_out_of_order(segments: &AzElSegments) -> bool {
    segments.az_el_segment_list.windows(2).any(|pair| {
        pair[1].valid_time_range.start_time.as_secs_f64()
            < pair[0].valid_time_range.end_time.as_secs_f64()
    })
}

/// Whether the pointing data `segments` carries describes no usable trajectory: true when any
/// segment's points are not strictly ascending in `dt`, or when any angle lies outside the
/// range its unit documents.
///
/// A unit the service knows no range for leaves every angle unplaceable, so that data
/// describes no trajectory either. No request reaches it: `angleUnit` is a modelled value set
/// (`AngleUnits`), so a token outside it fails the model's own rules before the handler runs.
fn pointing_data_unusable(segments: &AzElSegments) -> bool {
    let Some((azimuth, elevation)) = angle_ranges(&segments.angle_unit) else {
        return true;
    };
    segments.az_el_segment_list.iter().any(|segment| {
        segment
            .az_el_list
            .windows(2)
            .any(|pair| pair[1].dt <= pair[0].dt)
            || segment.az_el_list.iter().any(|point| {
                !in_closed_range(point.az, azimuth) || !in_closed_range(point.el, elevation)
            })
    })
}

/// Whether `value` lies inside the closed range `(low, high)`. A value that is not a number
/// lies in no range, so it is outside this one.
///
/// A plain comparison, unlike the frequency test this crate already carries
/// ([`in_any_band`]): that one admits a tolerance either side, because a frequency written in
/// another `FrequencyUnits` is compared after conversion, while an angle is compared in the
/// unit its own segments declare and the model's bounds are exact.
fn in_closed_range(value: f64, (low, high): (f64, f64)) -> bool {
    value >= low && value <= high
}

/// The ranges an `az` and an `el` may hold when the angles are measured in the `AngleUnits`
/// value `angle_unit`, as `(azimuth, elevation)` CLOSED ranges; `None` for a unit this
/// service knows no range for.
///
/// The bounds are the model's own, per unit: an azimuth is "`DEGREE_ANGLE`: -180 to 360
/// degrees" or "`RADIAN`: -π to 2π radians" and an elevation "`DEGREE_ANGLE`: -90 to 90
/// degrees" or "`RADIAN`: -π/2 to π/2 radians" (`smithy_query(query="doc", shape="TimeAzEl",
/// member="az")` / `member="el"`). They are the same angles in either unit, which is why one
/// table carries both: a conversion is not applied to the data, since "all angles in all
/// segments must use the same unit" (`smithy_query(query="doc", shape="AzElSegments",
/// member="angleUnit")`) and that unit is the one every angle read from them is reported in.
fn angle_ranges(angle_unit: &str) -> Option<((f64, f64), (f64, f64))> {
    match angle_unit {
        ANGLE_UNIT_DEGREE_ANGLE => Some(((-180.0, 360.0), (-90.0, 90.0))),
        ANGLE_UNIT_RADIAN => Some((
            (-::std::f64::consts::PI, ::std::f64::consts::TAU),
            (
                -::std::f64::consts::FRAC_PI_2,
                ::std::f64::consts::FRAC_PI_2,
            ),
        )),
        _ => None,
    }
}

/// The `AngleUnits` value that measures a pointing angle in degrees, and the one that
/// measures it in radians: the two values of that enum (`smithy_query(query="shape",
/// shape="AngleUnits")`).
pub const ANGLE_UNIT_DEGREE_ANGLE: &str = "DEGREE_ANGLE";

/// See [`ANGLE_UNIT_DEGREE_ANGLE`].
pub const ANGLE_UNIT_RADIAN: &str = "RADIAN";

/// The elevation at which a satellite crosses a ground station's horizon, in either unit
/// an `AzElSegments` measures its angles in: ZERO.
///
/// The model documents a pointing angle's `el` as "0 is the horizon, 90 is zenith, and
/// negative values are below the horizon" for `DEGREE_ANGLE` and "0 is the horizon, π/2 is
/// zenith, and negative values are below the horizon" for `RADIAN`
/// (`smithy_query(query="doc", shape="TimeAzEl", member="el")`), so an angle puts the
/// satellite above the horizon exactly when its `el` is at least this, whichever unit the
/// segments carrying it are measured in.
pub const HORIZON_ELEVATION: f64 = 0.0;

/// One pass of a satellite over a ground station: the span the pointing data in force for
/// that pairing keeps the antenna at or above the station's horizon, and the highest
/// elevation the satellite reaches inside it.
#[derive(Debug, Clone)]
pub struct PassWindow {
    /// The instant the satellite rises above the horizon.
    pub start_time: DateTime,
    /// The instant it sets below the horizon again.
    pub end_time: DateTime,
    /// The highest elevation the pointing data carries inside the window, in the
    /// `angleUnit` of the segments it was read from — what a response reports as a
    /// contact's `maximumElevation`.
    pub maximum_elevation: Elevation,
    /// The ephemeris whose pointing data this pass was read from.
    pub ephemeris_id: String,
    /// That ephemeris's `EphemerisType` ([`ephemeris_type_of`]).
    pub ephemeris_type: String,
}

/// Most time-tagged pointing points ONE availability search reads, across every ephemeris in
/// force for the pairing it is for; past it the search stops looking for further passes and
/// answers with what it has.
///
/// NOT an AWS quota: `TimeAzElList` carries a minimum of five points and NO maximum
/// (`smithy_query(query="shape", shape="TimeAzElList")`), so the pointing data an account can
/// store is bounded only by the requests that stored it. This bounds the WORK one read of it
/// does, so no single request can make a listing scan without limit — the same reason
/// [`MAX_RECORDS_PER_STORE`] bounds the store itself. It is far above any pointing data a
/// ground station network supplies: `AzElSegments` holds at most 100 segments
/// (`smithy_query(query="shape", shape="AzElSegments")`), so this leaves 200 points per
/// segment.
pub const MAX_POINTING_POINTS_READ: usize = 20_000;

/// Most passes ONE availability search reads, whatever window it asks about: past it the
/// search stops and answers with the passes it has, so a window spanning decades
/// materialises no more than this. A caller that wants fewer says so with `maxResults`,
/// which cuts the page further ([`page_size`]).
///
/// It is a CONSTANT rather than something the request names, so what a search answers is
/// not widened or narrowed by the page a caller asks for. CHOSEN bound:
/// [`MAX_PAGE_SIZE`] pages of it, so a caller can page through everything a search
/// answers rather than being offered more than it can reach.
pub const MAX_OFFERED_WINDOWS: usize = 5 * MAX_PAGE_SIZE;

/// One pass as the azimuth-elevation segments carry it, before it is placed against the
/// ephemeris it was read from: the span the satellite is above the horizon and the peak
/// elevation inside it, in the segments' own `angleUnit`.
struct AzElPass {
    start: f64,
    end: f64,
    peak: f64,
}

/// The passes the azimuth-elevation segments `segments` describe.
///
/// A segment is time-tagged pointing data: each point gives "the antenna pointing direction
/// at a specific time offset from the segment's reference epoch" (`smithy_query(query="shape",
/// shape="TimeAzEl")`), so a point's instant is the segment's `referenceEpoch` plus its `dt`.
/// A pass is then a maximal RUN of consecutive points at or above the horizon
/// ([`HORIZON_ELEVATION`]) lying inside the segment's own `validTimeRange`, which is the span
/// the segment's data is declared to cover; a run of a single point is an instant rather than
/// a window, so it describes no pass.
///
/// `budget` is how many points may still be read ([`MAX_POINTING_POINTS_READ`]), counted down
/// as they are: a search that spends it stops where it stands, so the work this does is
/// bounded whatever the stored data holds.
fn az_el_passes(segments: &AzElSegments, budget: &mut usize) -> Vec<AzElPass> {
    let mut passes: Vec<AzElPass> = Vec::new();
    for segment in &segments.az_el_segment_list {
        let epoch = segment.reference_epoch.as_secs_f64();
        let valid = (
            segment.valid_time_range.start_time.as_secs_f64(),
            segment.valid_time_range.end_time.as_secs_f64(),
        );
        // The run under way: its first instant, its latest instant and the highest
        // elevation seen in it so far.
        let mut run: Option<AzElPass> = None;
        for point in &segment.az_el_list {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let at = epoch + point.dt;
            let usable =
                at >= valid.0 && at <= valid.1 && point.el >= HORIZON_ELEVATION;
            if usable {
                run = Some(match run {
                    None => AzElPass {
                        start: at,
                        end: at,
                        peak: point.el,
                    },
                    Some(open) => AzElPass {
                        start: open.start,
                        end: at,
                        peak: open.peak.max(point.el),
                    },
                });
            } else if let Some(pass) = run.take() {
                passes.push(pass);
            }
        }
        if let Some(pass) = run.take() {
            passes.push(pass);
        }
    }
    passes.retain(|pass| pass.end > pass.start);
    passes
}

/// An ephemeris as `CreateEphemeris` stored it.
#[derive(Debug, Clone)]
pub struct Ephemeris {
    pub ephemeris_id: String,
    /// This ephemeris's ARN — what the tagging operations address it by, and what the one
    /// ARN index resolves to this record.
    pub ephemeris_arn: String,
    /// The satellite this ephemeris applies to, when the request named one.
    pub satellite_id: Option<String>,
    pub name: String,
    /// The `EphemerisType` the supplied data selected ([`ephemeris_type_of`]).
    pub ephemeris_type: String,
    pub priority: i32,
    /// Whether the caller asked for this ephemeris to be enabled once it validates.
    pub enabled: bool,
    pub creation_time: DateTime,
    /// When this ephemeris expires, after which it is [`EPHEMERIS_EXPIRED`]: the
    /// request's `expirationTime`, or the end of the supplied data's own validity when
    /// the request named none. `None` when neither is known, in which case the ephemeris
    /// does not expire.
    pub expiration_time: Option<DateTime>,
    /// The Amazon S3 object the data was supplied from, when it was not supplied inline.
    pub source_s3_object: Option<S3Object>,
    /// The KMS key the request asked for the ephemeris to be encrypted with.
    pub kms_key_arn: Option<String>,
    /// The ephemeris data as supplied, which `DescribeEphemeris` reports as
    /// `suppliedData`.
    pub supplied_data: EphemerisData,
    pub tags: HashMap<String, String>,
    /// This ephemeris's `EphemerisStatus` as it stands now.
    pub status: String,
    /// The verdict of the validation this ephemeris's data failed, as an
    /// `EphemerisInvalidReason`; `None` when the data validated. Set when the ephemeris
    /// is created ([`ephemeris_invalid_reason`]) and reported only once the validation has
    /// fallen due and the status is [`EPHEMERIS_INVALID`].
    pub invalid_reason: Option<&'static str>,
    /// The instant the service owes this ephemeris the move out of
    /// [`EPHEMERIS_VALIDATING`]; `None` once that move has been materialised.
    pub validation_settles_at: Option<SystemTime>,
}

impl ComAmazonawsGroundstationGroundstationState {
    /// The ephemeris in force for every satellite the account holds one for, keyed by the
    /// `satelliteId` it was stored against: this crate's ONE answer to "which ephemeris is
    /// a satellite's trajectory computed from", built once for every caller of it.
    ///
    /// An ephemeris is in force for the satellite it was stored against once it is
    /// [`EPHEMERIS_ENABLED`] — the status of one whose data validated and which the
    /// account asked to be used, where every other status an ephemeris can hold is one no
    /// trajectory is computed from (still validating, kept disabled, failed validation, or
    /// expired) — and the one of highest precedence takes the satellite
    /// ([`ephemeris_precedence`]), so AT MOST ONE is in force per satellite however many
    /// enabled ones the account holds for it.
    ///
    /// Keyed by the satellite each ephemeris is held for in ONE pass over the ephemerides,
    /// so the work is linear in the store rather than a scan of it per satellite. The key
    /// is the id the ephemeris names, whether or not this account is on-boarded to a
    /// satellite under it.
    fn ephemerides_in_force(&self) -> HashMap<&str, &Ephemeris> {
        let mut in_force: HashMap<&str, &Ephemeris> = HashMap::new();
        for ephemeris in self.ephemerides.values() {
            let Some(satellite_id) = ephemeris.satellite_id.as_deref() else {
                continue;
            };
            if ephemeris.status != EPHEMERIS_ENABLED {
                continue;
            }
            let takes_over = in_force.get(satellite_id).is_none_or(|held| {
                ephemeris_precedence(ephemeris, held) == ::std::cmp::Ordering::Greater
            });
            if takes_over {
                in_force.insert(satellite_id, ephemeris);
            }
        }
        in_force
    }

    /// The ephemeris in force for the satellite `satellite_id` names, by the rule
    /// [`Self::ephemerides_in_force`] states; `None` when the account holds none in force
    /// for it.
    pub fn ephemeris_in_force_for_satellite(&self, satellite_id: &str) -> Option<&Ephemeris> {
        self.ephemerides_in_force().get(satellite_id).copied()
    }

    /// The ephemeris whose trajectory this service points a contact's antenna from — "the
    /// ephemeris that determines antenna pointing directions for the contact"
    /// (`smithy_query(query="doc", shape="DescribeContactResponse", member="ephemeris")`,
    /// and the same sentence on `shape="ContactData"`); `None` when this account holds no
    /// ephemeris that determines it.
    ///
    /// Which one it is follows from the configuration the contact is flown with, so the
    /// caller hands in the tracking overrides and the satellite of the version it is
    /// reporting — the version in force for a contact read, the named one for a version
    /// read:
    ///
    /// * a program track flies the antenna from a SUPPLIED trajectory, so overrides that
    ///   name one ([`program_track_reference`]) determine the pointing outright, and the
    ///   ephemeris they name is the answer. It is resolved against the store, so what is
    ///   reported is an ephemeris this account still holds;
    /// * otherwise the pass is flown on the ephemeris in force for the satellite it is
    ///   flown against ([`Self::ephemeris_in_force_for_satellite`]) — the coupling a
    ///   listing's ephemeris filter selects on ([`Self::contacts_pointed_by_ephemeris`])
    ///   and an availability search reads a window's geometry from
    ///   ([`Self::pointing_passes`]), which is why an offered window and the contact
    ///   reserved over it report one and the same ephemeris.
    ///
    /// Reported as that ephemeris's own id and its `EphemerisType`
    /// (`smithy_query(query="shape", shape="EphemerisResponseData")`). The id is always
    /// present, because the member "appears only for custom ephemerides" and every
    /// ephemeris this store holds is one the customer supplied
    /// ([`EPHEMERIS_SOURCE_CUSTOMER_PROVIDED`]): AWS's own Space Track default is an
    /// ephemeris no request here creates, so a satellite with no custom ephemeris in force
    /// has nothing to report — exactly as such a satellite reports no `currentEphemeris`
    /// ([`Self::refresh_current_ephemerides`]).
    pub fn pointing_ephemeris(
        &self,
        tracking_overrides: Option<&TrackingOverrides>,
        satellite_arn: Option<&str>,
    ) -> Option<EphemerisResponseData> {
        let pointing = match tracking_overrides.and_then(program_track_reference) {
            Some(program_track) => self.ephemerides.get(program_track.ephemeris_id)?,
            None => {
                let satellite = self.satellite_by_arn(satellite_arn?)?;
                self.ephemeris_in_force_for_satellite(&satellite.satellite_id)?
            }
        };
        Some(EphemerisResponseData {
            ephemeris_id: Some(pointing.ephemeris_id.clone()),
            ephemeris_type: pointing.ephemeris_type.clone(),
        })
    }

    /// The ARN of the satellite whose passes are flown on the ephemeris `ephemeris_id`
    /// names; `None` when no pass is flown on it.
    ///
    /// A pass is flown on the ephemeris in force for the satellite it is flown against
    /// ([`Self::ephemeris_in_force_for_satellite`]), so an ephemeris is the trajectory of a
    /// satellite's passes only while it is the one in force for it: one that applies to no
    /// satellite, one of a satellite this account is not on-boarded to, and one another
    /// held for the same satellite supersedes are each the trajectory of nothing.
    fn satellite_flown_on_ephemeris(&self, ephemeris_id: &str) -> Option<&str> {
        let satellite_id = self.ephemerides.get(ephemeris_id)?.satellite_id.as_deref()?;
        let in_force = self.ephemeris_in_force_for_satellite(satellite_id)?;
        if in_force.ephemeris_id != ephemeris_id {
            return None;
        }
        Some(self.satellites.get(satellite_id)?.satellite_arn.as_str())
    }

    /// holds — what a listing filtering on an ephemeris selects; empty when it points
    /// nothing.
    ///
    /// An ephemeris is what a contact's antenna is pointed by, in either of the two ways a
    /// contact names one: through the satellite the ephemeris applies to — a contact names
    /// its satellite by ARN ([`Contact::flies_satellite`]), and the ephemeris in force for
    /// that satellite is the trajectory its pass is flown on — or by naming the ephemeris
    /// itself in the program track of its tracking overrides
    /// ([`Contact::program_tracks_ephemeris`]). So the ephemerides that point a contact are
    /// the ones that apply to its satellite plus the one its program track names.
    ///
    /// Which contacts an ephemeris points is a property of the POINTING, not of where each
    /// contact sits in its own lifecycle: a contact whose pass is over or whose reservation
    /// was released was still flown on the trajectory it names. The non-terminal
    /// restriction belongs to the IN-USE question alone
    /// ([`Self::contacts_using_ephemeris`]), which is a different question.
    ///
    /// For the SATELLITE coupling, the ephemeris a satellite's passes are flown on is the
    /// ONE in force for it ([`Self::ephemeris_in_force_for_satellite`]): being
    /// [`EPHEMERIS_ENABLED`] only puts an ephemeris in the running, and of those held for
    /// one satellite the highest precedence takes it, so a superseded ephemeris is the
    /// trajectory of no pass. A contact that names an ephemeris OUTRIGHT holds it whatever
    /// its status: the reference is the contact's own configuration, not a trajectory the
    /// service chose. CHOSEN coupling: AWS documents only which ephemeris a pass is flown
    /// on, so which contacts an ephemeris points is this emulator's rule, stated once here
    /// rather than per operation.
    ///
    /// Sorted, so every answer built from it reads the same on every call whatever order
    /// the map iterates in.
    pub fn contacts_pointed_by_ephemeris(&self, ephemeris_id: &str) -> Vec<String> {
        if !self.ephemerides.contains_key(ephemeris_id) {
            return Vec::new();
        }
        // The satellite whose passes are flown on this ephemeris: the one it was stored
        // against, when this is the ephemeris in force for it and that satellite is one
        // this account is on-boarded to. An ephemeris that applies to no satellite, and one
        // another supersedes, is the trajectory of nothing.
        let flown_satellite_arn = self.satellite_flown_on_ephemeris(ephemeris_id);
        let mut pointed: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| {
                contact.program_tracks_ephemeris(ephemeris_id)
                    || flown_satellite_arn.is_some_and(|arn| contact.flies_satellite(arn))
            })
            .map(|contact| contact.contact_id.clone())
            .collect();
        pointed.sort();
        pointed
    }

    /// The contacts that still depend on the ephemeris `ephemeris_id` names, as the
    /// refusal of a delete lists them; empty when nothing depends on it.
    ///
    /// The contacts it points ([`Self::contacts_pointed_by_ephemeris`]) that are still
    /// going somewhere: a contact depends on what it is flown with for as long as it is not
    /// terminal ([`Contact::is_non_terminal`]) — once its pass is over or its reservation
    /// released, nothing of it is still to be flown, so a resource it names is no longer
    /// held. AWS documents only that such a resource "is in use by non-terminal state
    /// contacts", which is exactly that restriction.
    ///
    /// Sorted, like the pointing it narrows, so a refusal listing them reads the same on
    /// every call.
    pub fn contacts_using_ephemeris(&self, ephemeris_id: &str) -> Vec<String> {
        self.contacts_pointed_by_ephemeris(ephemeris_id)
            .into_iter()
            .filter(|contact_id| {
                self.contacts
                    .get(contact_id)
                    .is_some_and(Contact::is_non_terminal)
            })
            .collect()
    }

    /// Write onto the ephemeris `ephemeris_id` names the members an update carries — the
    /// required `enabled`, plus `name` and `priority` when the request carries them — and
    /// land it on the status its flag selects; or, when that write would leave a pass still
    /// to be flown with no trajectory at all, write NOTHING and answer the contacts it
    /// would have left without one.
    ///
    /// A resource "in use by non-terminal state contacts ... cannot be modified or deleted"
    /// (`smithy_query(query="doc", shape="ResourceInUseException")`), so the hold a contact
    /// still to be flown has on what it is flown with binds THIS writer as well as the
    /// delete ([`Self::contacts_using_ephemeris`]): a write that takes this ephemeris out
    /// from under such a pass is refused, unless another ephemeris the account holds takes
    /// that satellite over ([`Self::ephemeris_in_force_for_satellite`]) — the pointing
    /// follows whichever ephemeris is in force, so a pass handed to the one that was
    /// superseded still has the trajectory it is flown on, and only a write that leaves it
    /// with none takes the trajectory away.
    ///
    /// Which contacts a write releases is decided by APPLYING it and asking that one helper
    /// again, rather than by a second copy of the status and precedence rules that decide
    /// what a pass is flown on, and the ephemeris as it stood goes back before the refusal
    /// returns — so a refused write leaves the store exactly as it was. A contact that
    /// names this ephemeris OUTRIGHT in its program track holds it whatever its status
    /// ([`Contact::program_tracks_ephemeris`]), so no write of these members releases such
    /// a contact and none is ever refused for one.
    ///
    /// Enabling or disabling "doesn't require re-validation" (`smithy_query(query="doc",
    /// shape="UpdateEphemerisRequest", member="enabled")`), so an ephemeris whose data has
    /// already validated takes the status its new flag selects straight away; one still
    /// validating keeps the move it is owed, which lands on the flag as it stands when that
    /// move falls due, and one that did not validate or whose expiration has passed keeps
    /// the status its data earned.
    ///
    /// The ephemeris must be one this store holds: a caller addressed at an id it does not
    /// hold has already answered the missing-resource refusal.
    pub fn write_ephemeris_update(
        &mut self,
        ephemeris_id: &str,
        enabled: bool,
        name: Option<String>,
        priority: Option<i32>,
    ) -> Result<(), Vec<String>> {
        // The passes flown on this ephemeris as it stands, read before anything is written.
        let flown_before = self.contacts_using_ephemeris(ephemeris_id);

        let stored = self
            .ephemerides
            .get_mut(ephemeris_id)
            .expect("an ephemeris this store holds, as this method's contract requires");
        // Kept whole, so a write that turns out to take a trajectory away is undone rather
        // than reconstructed member by member.
        let before = stored.clone();
        let satellite_id = stored.satellite_id.clone();

        stored.enabled = enabled;
        if let Some(name) = name {
            stored.name = name;
        }
        if let Some(priority) = priority {
            stored.priority = priority;
        }
        if matches!(
            stored.status.as_str(),
            EPHEMERIS_ENABLED | EPHEMERIS_DISABLED
        ) {
            stored.status = if stored.enabled {
                EPHEMERIS_ENABLED.to_string()
            } else {
                EPHEMERIS_DISABLED.to_string()
            };
        }

        // The passes the write took this ephemeris out from under, and whether the
        // satellite they are flown against is left with a trajectory at all.
        let flown_after = self.contacts_using_ephemeris(ephemeris_id);
        let released: Vec<String> = flown_before
            .into_iter()
            .filter(|contact_id| !flown_after.contains(contact_id))
            .collect();
        let taken_over = satellite_id.as_deref().is_some_and(|satellite_id| {
            self.ephemeris_in_force_for_satellite(satellite_id).is_some()
        });
        if released.is_empty() || taken_over {
            return Ok(());
        }

        self.ephemerides.insert(ephemeris_id.to_string(), before);
        Err(released)
    }

    /// Remove the ephemeris `ephemeris_id` addresses, along with the ARN index entry that
    /// resolved to it and any satellite reference that named it, and answer it; `None` when
    /// no ephemeris has that id.
    ///
    /// Dropping the index entry is what makes the deletion complete: the tagging
    /// operations address an ephemeris by ARN, and after this that ARN resolves to nothing,
    /// exactly as the ARN of one that never existed does.
    pub fn remove_ephemeris(&mut self, ephemeris_id: &str) -> Option<Ephemeris> {
        let removed = self.ephemerides.remove(ephemeris_id)?;
        self.arn_index.remove(&removed.ephemeris_arn);
        // A satellite reports the ephemeris its trajectory is computed from; a deleted one
        // is no longer that, so the reference goes with it rather than being left dangling
        // — and whichever ephemeris the account still holds for that satellite takes over.
        self.refresh_current_ephemerides();
        Some(removed)
    }

    /// Materialise, on every satellite this account is on-boarded to, the ephemeris its
    /// trajectory is computed from — what a satellite read reports as its
    /// `currentEphemeris`.
    ///
    /// A reporting operation calls this before reading a satellite: the value is derived
    /// from the ephemerides this store already holds, so recomputing it changes nothing a
    /// later caller could otherwise have seen. Callers that depend on an ephemeris status
    /// the clock has already made due call [`Self::settle_due_ephemerides`] first.
    ///
    /// An ephemeris is in force for the satellite it was stored against by the one rule
    /// [`Self::ephemerides_in_force`] states, which this shares with every other caller
    /// that asks which ephemeris a satellite's passes are flown on.
    ///
    /// An ephemeris in force supersedes whatever default ephemeris the onboarding knows
    /// (AWS generates one from Space Track outside this API), so a default is left alone
    /// here and only a customer-provided reference is dropped — which is how a satellite
    /// falls back to its default once the account holds nothing in force for it.
    pub fn refresh_current_ephemerides(&mut self) {
        // The metadata is taken OWNED first, so the borrow of the ephemerides this reads
        // ends before the satellites it writes are borrowed.
        let in_force: HashMap<String, EphemerisMetaData> = self
            .ephemerides_in_force()
            .into_iter()
            .map(|(satellite_id, ephemeris)| {
                (
                    satellite_id.to_string(),
                    customer_ephemeris_metadata(ephemeris),
                )
            })
            .collect();

        for satellite in self.satellites.values_mut() {
            match in_force.get(satellite.satellite_id.as_str()) {
                Some(metadata) => {
                    satellite.current_ephemeris = Some(metadata.clone());
                }
                None => {
                    let customer_provided = satellite.current_ephemeris.as_ref().is_some_and(
                        |metadata| metadata.source == EPHEMERIS_SOURCE_CUSTOMER_PROVIDED,
                    );
                    if customer_provided {
                        satellite.current_ephemeris = None;
                    }
                }
            }
        }
    }

    /// Materialise every ephemeris status move the clock has already made due: the end of
    /// the validation `CreateEphemeris` started, and the expiration of an ephemeris whose
    /// `expirationTime` has passed. A reporting operation calls this before reading: both
    /// moves were owed at this instant whether or not anybody called, so applying them
    /// changes nothing a later caller could otherwise have seen.
    ///
    /// Validation lands the ephemeris on the verdict taken when it was created
    /// ([`ephemeris_invalid_reason`]): [`EPHEMERIS_INVALID`] when its data breaks a rule the
    /// model states for it, else [`EPHEMERIS_ENABLED`] or [`EPHEMERIS_DISABLED`]
    /// according to what the caller asked for. Expiration then overrides either of those
    /// two, and an ephemeris that did not validate stays `INVALID`: there is nothing of
    /// it to expire.
    pub fn settle_due_ephemerides(&mut self) {
        let now = SystemTime::now();
        let now_seconds = now
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        for ephemeris in self.ephemerides.values_mut() {
            if let Some(due) = ephemeris.validation_settles_at {
                if due > now {
                    continue;
                }
                ephemeris.status = match (ephemeris.invalid_reason, ephemeris.enabled) {
                    (Some(_), _) => EPHEMERIS_INVALID.to_string(),
                    (None, true) => EPHEMERIS_ENABLED.to_string(),
                    (None, false) => EPHEMERIS_DISABLED.to_string(),
                };
                ephemeris.validation_settles_at = None;
            }
            let expired = ephemeris
                .expiration_time
                .is_some_and(|expiration| expiration.as_secs_f64() <= now_seconds);
            if expired
                && matches!(
                    ephemeris.status.as_str(),
                    EPHEMERIS_ENABLED | EPHEMERIS_DISABLED
                )
            {
                ephemeris.status = EPHEMERIS_EXPIRED.to_string();
            }
        }
    }

    /// The passes of the satellite `satellite_arn` over `ground_station` that the pointing
    /// data in force for that pairing describes, in start-time order; empty when no pointing
    /// data is in force for it.
    ///
    /// Pointing data is in force when it belongs to an ENABLED ephemeris
    /// ([`EPHEMERIS_ENABLED`]) that applies to that satellite and was supplied
    /// for that station: az/el data "is specific to this ground station and can't be used at
    /// other locations" (`smithy_query(query="doc", shape="AzElEphemeris",
    /// member="groundStation")`).
    ///
    /// SEVERAL enabled ephemerides of one satellite can each describe a pass here, which is
    /// a WIDER reading of "in force" than the single one the satellite coupling takes
    /// ([`Self::ephemerides_in_force`]): `priority` decides "which ephemeris to use when
    /// multiple ephemerides overlap", so two that do NOT overlap each describe passes of
    /// their own.
    ///
    /// Where two ephemerides describe the same instants, the one of higher PRECEDENCE is the
    /// one used — the crate's one order for "which of several held for a satellite is in
    /// force" ([`ephemeris_precedence`]), so a pass overlapping one already read from an
    /// ephemeris of higher precedence is left out, and the answer a caller is offered names
    /// the same ephemeris a satellite read reports as the one its trajectory is computed
    /// from.
    ///
    /// Only the az/el form yields a pass: it IS pointing angles for one station, so the
    /// geometry is READ rather than computed. A `tle` or `oem` ephemeris carries orbital
    /// elements that would have to be propagated against the station's coordinates, which
    /// nothing in this service records, and data held in an Amazon S3 object is not in this
    /// store at all; neither describes a pass here.
    ///
    /// BOUNDED work, so no stored data and no window asked about can make this scan without
    /// limit: `accept` is asked about each pass as it is read and only the ones it admits are
    /// kept, at most [`MAX_OFFERED_WINDOWS`] of them, and at most
    /// [`MAX_POINTING_POINTS_READ`] points are read whatever is still unexamined. A search
    /// that reaches either bound answers with the passes it has. Both bounds are CONSTANTS,
    /// so the page a caller asks for does not change which passes a read resolves. The
    /// passes a LATER read resolves can differ all the same — `accept` is asked about the
    /// clock and about the contacts held now — so a caller paging over them resumes at the
    /// pass its last page ended on ([`PageCursor`]), never at a count of the ones before it.
    pub fn pointing_passes(
        &self,
        satellite_arn: &str,
        ground_station: &str,
        accept: impl Fn(f64, f64) -> bool,
    ) -> Vec<PassWindow> {
        // The ephemeris and the segments it supplies for this station, read once here
        // rather than again per pass.
        let mut in_force: Vec<(&Ephemeris, &AzElSegments)> = self
            .ephemerides
            .values()
            .filter(|ephemeris| ephemeris.status == EPHEMERIS_ENABLED)
            .filter(|ephemeris| {
                ephemeris
                    .satellite_id
                    .as_deref()
                    .and_then(|satellite_id| self.satellites.get(satellite_id))
                    .is_some_and(|satellite| satellite.satellite_arn == satellite_arn)
            })
            .filter_map(|ephemeris| {
                az_el_segments_for(ephemeris, ground_station)
                    .map(|segments| (ephemeris, segments))
            })
            .collect();
        // Precedence-DESCENDING, so the one in force is read first and whatever it describes
        // is what an overlapping pass is left out in favour of. The order is
        // [`ephemeris_precedence`]'s, the crate's single answer to this question, so this
        // search and a satellite read cannot name two different ephemerides as the one in
        // force for the same satellite at the same instant.
        in_force.sort_by(|first, second| ephemeris_precedence(second.0, first.0));

        let mut budget = MAX_POINTING_POINTS_READ;
        // Held in start-time order and never overlapping, so the pass before the candidate
        // is the only one it can collide with: the overlap test is a binary search rather
        // than a scan of everything kept so far.
        let mut passes: Vec<PassWindow> = Vec::new();
        for (ephemeris, segments) in in_force {
            if passes.len() >= MAX_OFFERED_WINDOWS || budget == 0 {
                break;
            }
            for pass in az_el_passes(segments, &mut budget) {
                if passes.len() >= MAX_OFFERED_WINDOWS {
                    break;
                }
                if !accept(pass.start, pass.end) {
                    continue;
                }
                let after =
                    passes.partition_point(|kept| kept.start_time.as_secs_f64() <= pass.end);
                if after > 0 && passes[after - 1].end_time.as_secs_f64() >= pass.start {
                    continue;
                }
                let at = passes.partition_point(|kept| kept.start_time.as_secs_f64() < pass.start);
                passes.insert(
                    at,
                    PassWindow {
                        start_time: DateTime::from_secs_f64(pass.start),
                        end_time: DateTime::from_secs_f64(pass.end),
                        maximum_elevation: Elevation {
                            value: pass.peak,
                            unit: segments.angle_unit.clone(),
                        },
                        ephemeris_id: ephemeris.ephemeris_id.clone(),
                        ephemeris_type: ephemeris.ephemeris_type.clone(),
                    },
                );
            }
        }
        passes
    }

    /// The contact windows the pairing of the satellite `satellite_arn`, the ground station
    /// `ground_station` and the mission profile `profile` can still fly inside `window`
    /// (`(start, end)` in epoch seconds), as at the instant `now`, in start-time order;
    /// only the ones the ephemeris `pointed_by` names determine, when it names one.
    ///
    /// These are the passes the pointing data in force describes ([`Self::pointing_passes`]),
    /// less the ones nothing could reserve: a pass whose own window does not touch the window
    /// asked about, one whose end the clock has already passed (antenna time that has elapsed
    /// holds nothing left to hold), one shorter than the profile's
    /// `minimumViableContactDurationSeconds`, and one every antenna at the station is already
    /// held for ([`Self::free_antenna`]). Those are exactly the conditions `ReserveContact`
    /// refuses a reservation on, so a window this answers is a window that can then be
    /// reserved, and a window already reserved stops being offered.
    ///
    /// The conditions are asked in that order, cheapest first, so the one that reads the
    /// store is only reached for a pass the window and the clock admit.
    ///
    /// `pointed_by` is applied to the RESOLVED windows rather than to the pointing data it
    /// chooses between: which ephemeris determines a window is settled by priority first, so
    /// a window a higher-priority ephemeris points is not offered for a filter naming a
    /// lower-priority one.
    pub fn available_contact_windows(
        &self,
        satellite_arn: &str,
        ground_station: &str,
        profile: &MissionProfile,
        window: (f64, f64),
        now: f64,
        pointed_by: Option<&str>,
    ) -> Vec<PassWindow> {
        let minimum = f64::from(profile.minimum_viable_contact_duration_seconds);
        self.pointing_passes(satellite_arn, ground_station, |start, end| {
            windows_overlap((start, end), window)
                && end > now
                && end - start >= minimum
                && self.free_antenna(ground_station, (start, end)).is_some()
        })
        .into_iter()
        .filter(|pass| pointed_by.is_none_or(|id| pass.ephemeris_id == id))
        .collect()
    }
}
