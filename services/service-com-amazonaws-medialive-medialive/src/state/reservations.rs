// SPDX-License-Identifier: CC0-1.0

//! The RESERVATION slice of this service's data model: the reserved resources an
//! account has bought.
//!
//! A reservation is what `PurchaseOffering` ("Purchase an offering and create a
//! reservation") creates against ONE entry of the [`super::offerings`] catalog,
//! so this slice is a STORE layered on that catalog: one [`ReservationRecord`]
//! per purchase, its own id counter and its own idempotency-token map.
//!
//! Every member a reservation reports about the thing it reserves — the resource
//! specification, the lease term, the currency, the prices and the offering's
//! description — is read from the catalog entry the purchase named, through the
//! SAME projection `ListOfferings` and `DescribeOffering` answer from, so a
//! reservation cannot drift from the offering it was bought from. What the record
//! itself holds is what the purchase CHOSE: how many resources, the name, when
//! the lease starts, the renewal settings and the tags.
//!
//! ## What the clock owns
//!
//! A reservation's `State` is DERIVED on every read rather than stored. The lease
//! runs from its `Start` for the offering's `Duration` in its `DurationUnits`, so
//! the reservation is `ACTIVE` until its `End` and `EXPIRED` from that instant on
//! — a transition the clock makes due whether or not anybody calls, which is why
//! reporting it writes nothing.
//!
//! `CANCELED` and `DELETED`, the other two states the enum declares
//! (`smithy_query(query="shape", shape="ReservationState")`: ACTIVE | EXPIRED |
//! CANCELED | DELETED), are not reachable from what is implemented here: no
//! operation of this service cancels a reservation, and deleting an expired one
//! is `DeleteReservation`, still a scaffold with an issue of its own
//! (`20261024T010738Z-medialive-deletereservation-stub.md`), which is where that
//! state lands.

use std::collections::HashMap;
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Months, NaiveDateTime, Timelike, Utc};

use crate::ops::types::{RenewalSettings, Reservation};

use super::{
    catalog, offering_id, ComAmazonawsMedialiveMedialiveState, OfferingSpec,
    OFFERING_CURRENCY_CODE, OFFERING_DURATION, OFFERING_DURATION_UNITS, OFFERING_FIXED_PRICE,
    OFFERING_TYPE,
};

/// The `ReservationState` values this slice reaches, spelled exactly as the model
/// declares them. The two it does not name are the ones nothing implemented here
/// can reach — see this module's docs.
pub mod reservation_state {
    pub const ACTIVE: &str = "ACTIVE";
    pub const EXPIRED: &str = "EXPIRED";
}

/// The automatic-renewal status a reservation whose purchase asked for none
/// carries.
///
/// Such a reservation does not renew itself, which is the `DISABLED` its own enum
/// declares (`smithy_query(query="shape", shape="ReservationAutomaticRenewal")`:
/// DISABLED | ENABLED | UNAVAILABLE).
pub const DEFAULT_AUTOMATIC_RENEWAL: &str = "DISABLED";

/// The id the first reservation of a process gets.
///
/// `ReservationId` is "Unique reservation ID, e.g. '1234567'" — a seven-digit
/// numeric string — so ids are minted from a counter at that fixed width, which
/// keeps the `BTreeMap` key order equal to the mint order. A range of its own
/// keeps the service's id spaces apart (as `FIRST_SDI_SOURCE_ID` and the other
/// bases do): every base is at least half a million apart from its nearest
/// neighbour, far more than any store's cap, so a caller that passes a node's or
/// a channel's id where a reservation's belongs gets a `NotFoundException` rather
/// than a coincidental hit.
const FIRST_RESERVATION_ID: u64 = 9_500_000;

/// The most reservations this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a purchase loop
/// cannot grow the emulator's heap without bound. `PurchaseOffering` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_RESERVATIONS: usize = 100;

/// The most `RequestId` entries kept for `PurchaseOffering` idempotency — the
/// same bound, for the same reason, as [`super::MAX_IDEMPOTENCY_TOKENS`] on
/// channels: a token is only useful while the reservation it created exists.
pub const MAX_RESERVATION_TOKENS: usize = MAX_RESERVATIONS;

/// A year, in the months calendar arithmetic counts: what "one year from now"
/// bounds a requested start by.
const MONTHS_IN_A_YEAR: u32 = 12;

/// The form this service reads and writes a reservation's `Start` and `End` in:
/// the ISO-8601 UTC date-time both members document by example (`Start`:
/// "Reservation UTC start date and time in ISO-8601 format, e.g.
/// '2018-03-01T00:00:00'"; `End` the same with its own example) — to the second,
/// with no zone suffix.
const RESERVATION_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// One purchased reservation, as `PurchaseOffering` stored it.
///
/// The members mirror the model's `Reservation` shape minus the ones this slice
/// derives: `State` comes from the clock (see the module docs), and the resource
/// specification, the term, the prices and the offering description come from the
/// catalog entry [`Self::offering_index`] names, never from a field written at
/// purchase time.
#[derive(Debug, Clone)]
pub struct ReservationRecord {
    pub id: String,
    pub arn: String,
    /// The region the purchase was signed for, which is the region the reserved
    /// capacity is in: `Region` is "AWS region, e.g. 'us-west-2'" and the `Arn`
    /// names that region too.
    pub region: String,
    /// The position in the offering [`catalog`] of the entry this reservation was
    /// purchased from — the one the request's `OfferingId` resolved to.
    offering_index: usize,
    /// `Count`, "Number of reserved resources".
    pub count: i32,
    /// `Name`, "User specified reservation name".
    pub name: Option<String>,
    /// When the lease starts, which the purchase chose and defaults to the
    /// instant of the purchase.
    pub start: SystemTime,
    /// When the lease ends: [`Self::start`] plus the offering's own term, so a
    /// reservation's reported `Duration` and its `End` cannot disagree.
    pub end: SystemTime,
    pub renewal_settings: RenewalSettings,
    /// The request's `Tags`. `Reservation$Tags` reports them and the tag
    /// operations read and write this one map through
    /// [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
}

impl ReservationRecord {
    /// The catalog entry this reservation was purchased from.
    ///
    /// `None` only for an index the catalog does not hold, which a purchase
    /// cannot store (the id is resolved through the catalog before the record is
    /// made) — so the members derived from it are reported for every stored
    /// reservation.
    fn offering(&self) -> Option<&'static OfferingSpec> {
        catalog().get(self.offering_index)
    }

    /// The reservation's `State` as of `now`: `ACTIVE` while the lease runs,
    /// `EXPIRED` from its `End` on.
    pub fn state(&self, now: SystemTime) -> &'static str {
        if now < self.end {
            reservation_state::ACTIVE
        } else {
            reservation_state::EXPIRED
        }
    }

    /// This record as the modelled `Reservation` every reader reports.
    ///
    /// The ONE projection, so the reservation a purchase answers with and the one
    /// a later read reports cannot differ.
    pub fn reservation(&self, now: SystemTime) -> Reservation {
        let offering = self.offering();
        Reservation {
            arn: Some(self.arn.clone()),
            count: Some(self.count),
            currency_code: Some(OFFERING_CURRENCY_CODE.to_string()),
            duration: Some(OFFERING_DURATION),
            duration_units: Some(OFFERING_DURATION_UNITS.to_string()),
            end: Some(reservation_time(self.end)),
            fixed_price: Some(OFFERING_FIXED_PRICE),
            name: self.name.clone(),
            offering_description: offering.map(|spec| spec.description(&self.region)),
            offering_id: Some(offering_id(self.offering_index)),
            offering_type: Some(OFFERING_TYPE.to_string()),
            region: Some(self.region.clone()),
            renewal_settings: Some(self.renewal_settings.clone()),
            reservation_id: Some(self.id.clone()),
            resource_specification: offering.map(OfferingSpec::resource_specification),
            start: Some(reservation_time(self.start)),
            state: Some(self.state(now).to_string()),
            tags: Some(self.tags.clone()),
            usage_price: offering.map(OfferingSpec::usage_price),
        }
    }
}

/// What one `PurchaseOffering` request asks to be reserved: the catalog entry its
/// `OfferingId` resolved to, and the choices the request carried.
///
/// One struct rather than a row of positional arguments on
/// [`ComAmazonawsMedialiveMedialiveState::insert_reservation`], so a caller
/// cannot pass the name where the count belongs.
#[derive(Debug, Clone)]
pub struct NewReservation {
    pub offering_index: usize,
    pub count: i32,
    pub name: Option<String>,
    pub start: SystemTime,
    pub renewal_settings: Option<RenewalSettings>,
    pub tags: HashMap<String, String>,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_reservation`] stored
/// nothing.
#[derive(Debug)]
pub enum ReservationRefusal {
    /// The store already holds [`MAX_RESERVATIONS`] reservations.
    StoreFull,
}

/// Why a requested `Start` is not one this service can lease from.
#[derive(Debug)]
pub enum ReservationStartRefusal {
    /// The value is not a UTC date and time in ISO-8601 format at all.
    Unreadable,
    /// It is a time, but outside the window the member documents. Carries that
    /// window's ends, as the request would have had to spell them.
    OutsideWindow { earliest: String, latest: String },
}

/// A stored instant in the one form this service reports a reservation time in.
pub fn reservation_time(at: SystemTime) -> String {
    DateTime::<Utc>::from(at)
        .format(RESERVATION_TIME_FORMAT)
        .to_string()
}

/// The instant a purchase leases from: the `Start` the request named, or the
/// instant of the purchase itself.
///
/// `Start` is "Requested reservation start time (UTC) in ISO-8601 format. The
/// specified time must be between the first day of the current month and one year
/// from now. If no value is given, the default is now." — so a value outside that
/// window, and a value that is no such time, are both refused rather than
/// leased from, and an absent one takes `now`.
///
/// The calendar arithmetic is the LIBRARY's: the first day of the current month
/// and the same date one year on are month-aware, which a span of seconds is not.
pub fn reservation_start(
    requested: Option<&str>,
    now: SystemTime,
) -> Result<SystemTime, ReservationStartRefusal> {
    let now_utc = DateTime::<Utc>::from(now);
    let Some(requested) = requested else {
        return Ok(whole_seconds(now_utc));
    };
    let start = reservation_instant(requested).ok_or(ReservationStartRefusal::Unreadable)?;
    let earliest = first_of_month(now_utc);
    let latest = one_year_on(now_utc);
    if start < earliest || start > latest {
        return Err(ReservationStartRefusal::OutsideWindow {
            earliest: reservation_time(SystemTime::from(earliest)),
            latest: reservation_time(SystemTime::from(latest)),
        });
    }
    Ok(whole_seconds(start))
}

/// The instant a reservation time names, read as UTC.
///
/// Both the zoned ISO-8601 form (a trailing `Z` or a numeric offset) and the
/// bare one the members' own examples use are read, because both are ISO-8601
/// spellings of one instant; a zoned value is converted to UTC, which is the zone
/// the member says it is in.
fn reservation_instant(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(zoned) = DateTime::parse_from_rfc3339(raw) {
        return Some(zoned.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(raw, RESERVATION_TIME_FORMAT)
        .ok()
        .map(|naive| naive.and_utc())
}

/// Midnight on the first day of the month `now` falls in: the earliest start the
/// `Start` member admits.
fn first_of_month(now: DateTime<Utc>) -> DateTime<Utc> {
    now.date_naive()
        .with_day(1)
        .and_then(|first| first.and_hms_opt(0, 0, 0))
        .map_or(now, |first| first.and_utc())
}

/// One year on from `now`: the latest start the `Start` member admits. A year is
/// [`MONTHS_IN_A_YEAR`] calendar months, so the 29th of February maps to the 28th
/// in a year that has no 29th.
fn one_year_on(now: DateTime<Utc>) -> DateTime<Utc> {
    now.checked_add_months(Months::new(MONTHS_IN_A_YEAR))
        .unwrap_or(now)
}

/// The instant `at` truncated to the whole second this service reports times to,
/// so a stored start and the `Start` read back out of it are the same instant.
fn whole_seconds(at: DateTime<Utc>) -> SystemTime {
    SystemTime::from(at.with_nanosecond(0).unwrap_or(at))
}

/// The end of a lease that starts at `start`: the offering's own term, which
/// every entry of the catalog is for ([`OFFERING_DURATION`] of
/// [`OFFERING_DURATION_UNITS`]).
///
/// Month arithmetic rather than a span of seconds, because the term is in months
/// and months differ in length.
fn reservation_end(start: SystemTime) -> SystemTime {
    let start_utc = DateTime::<Utc>::from(start);
    let term = Months::new(u32::try_from(OFFERING_DURATION).unwrap_or(MONTHS_IN_A_YEAR));
    SystemTime::from(start_utc.checked_add_months(term).unwrap_or(start_utc))
}

/// The renewal settings a purchased reservation carries: the ones the request
/// named, with the automatic-renewal status filled in when it named none.
///
/// `RenewalCount` is left exactly as the request left it — it is "Count for the
/// reservation renewal", so a purchase that named none has no count to report.
fn renewal_settings(requested: Option<RenewalSettings>) -> RenewalSettings {
    let mut settings = requested.unwrap_or_default();
    if settings.automatic_renewal.is_none() {
        settings.automatic_renewal = Some(DEFAULT_AUTOMATIC_RENEWAL.to_string());
    }
    settings
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The next reservation id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    fn mint_reservation_id(&mut self) -> String {
        let id = FIRST_RESERVATION_ID + self.next_reservation_id;
        self.next_reservation_id += 1;
        id.to_string()
    }

    /// Store a reservation purchased in `region` and answer its minted id, or
    /// refuse.
    ///
    /// The store's cap, the mint, the ARN and the lease's end live together here,
    /// so a handler cannot check one of them and write past another; the
    /// handler's job is only to turn a [`ReservationRefusal`] into the
    /// operation's own modelled error.
    pub fn insert_reservation(
        &mut self,
        region: &str,
        new: NewReservation,
    ) -> Result<String, ReservationRefusal> {
        if self.reservations.len() >= MAX_RESERVATIONS {
            return Err(ReservationRefusal::StoreFull);
        }

        let id = self.mint_reservation_id();
        // The ARN form `Reservation$Arn` documents by example
        // ("arn:aws:medialive:us-west-2:123456789012:reservation:1234567").
        let arn = ::engine::emulator_arn("medialive", region, &format!("reservation:{id}"));
        self.reservations.insert(
            id.clone(),
            ReservationRecord {
                id: id.clone(),
                arn,
                region: region.to_string(),
                offering_index: new.offering_index,
                count: new.count,
                name: new.name,
                start: new.start,
                end: reservation_end(new.start),
                renewal_settings: renewal_settings(new.renewal_settings),
                tags: new.tags,
            },
        );
        Ok(id)
    }

    /// Record `token` as having purchased the reservation `reservation_id`,
    /// evicting the oldest token first if that would push the map past
    /// [`MAX_RESERVATION_TOKENS`].
    pub fn record_reservation_token(&mut self, token: String, reservation_id: &str) {
        self.reservation_tokens
            .record(token, reservation_id, MAX_RESERVATION_TOKENS);
    }

    /// The reservation a stored `RequestId` purchased, if that reservation still
    /// exists.
    pub fn reservation_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.reservation_tokens.created(request_id)?;
        self.reservations
            .get(id)
            .map(|reservation| reservation.id.as_str())
    }

    /// The reservation `id` names as of `now`, or `None` when the account holds
    /// none with that id.
    pub fn reservation_description(&self, id: &str, now: SystemTime) -> Option<Reservation> {
        Some(self.reservations.get(id)?.reservation(now))
    }
}
