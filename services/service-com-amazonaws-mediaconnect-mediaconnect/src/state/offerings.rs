// SPDX-License-Identifier: CC0-1.0

//! The reservation side of this service: the offering catalogue it publishes, the terms an
//! entry is offered on, and the reservation record a purchase of one stores.

use super::*;

/// The `CurrencyCode` every offering and reservation is priced in: "the currencyCode used
/// for all reservations is US dollars"
/// (`smithy_query(query="shape",shape="Offering",member="CurrencyCode")`).
pub const CURRENCY_CODE_USD: &str = "USD";

/// The only value the modelled `PriceUnits` enum carries
/// (`smithy_query(query="shape",shape="PriceUnits")`: HOURLY), so `PricePerUnit` is the
/// cost of ONE HOUR of the bandwidth an offering reserves.
pub const PRICE_UNITS_HOURLY: &str = "HOURLY";

/// The only value the modelled `DurationUnits` enum carries
/// (`smithy_query(query="shape",shape="DurationUnits")`: MONTHS), so `Duration` is a
/// number of months.
pub const DURATION_UNITS_MONTHS: &str = "MONTHS";

/// The only value the modelled `ResourceType` enum carries
/// (`smithy_query(query="shape",shape="ResourceType")`: Mbps_Outbound_Bandwidth), so what
/// an offering reserves is outbound bandwidth counted in Mbps — which is also what
/// `ResourceSpecification$ReservedBitrate`, "the amount of outbound bandwidth that is
/// discounted in the offering", carries.
pub const RESOURCE_TYPE_MBPS_OUTBOUND_BANDWIDTH: &str = "Mbps_Outbound_Bandwidth";

/// The term every offering in this service's catalogue runs for, in
/// [`DURATION_UNITS_MONTHS`] — `Offering$Duration`, "the length of time that your
/// reservation would be active".
///
/// One term for the whole catalogue: the catalogue below varies only along the bandwidth
/// it reserves, so a purchase picks an amount rather than a length.
pub const OFFERING_DURATION_MONTHS: i32 = 12;

/// The scale a reservation price is published at: thousandths of a
/// [`CURRENCY_CODE_USD`] dollar.
///
/// `Offering$PricePerUnit` is a decimal STRING ("the cost of a single unit",
/// `smithy_query(query="shape",shape="Offering",member="PricePerUnit")`), and the published
/// per-hour price of a reservation is quoted to the thousandth of a dollar — the same
/// rendering [`offering_description`] reads the price back out of. Amounts are therefore
/// held as an integer count of thousandths, so a rendered price carries no rounding of its
/// own.
const PRICE_THOUSANDTHS_PER_DOLLAR: i64 = 1_000;

/// One entry of the reservation catalogue this service publishes.
#[derive(Debug, Clone, Copy)]
pub struct PublishedOffering {
    /// `ResourceSpecification$ReservedBitrate`, "the amount of outbound bandwidth that is
    /// discounted in the offering", in Mbps.
    pub reserved_bitrate: i32,
    /// What one hour of that much reserved outbound bandwidth costs, in
    /// [`PRICE_THOUSANDTHS_PER_DOLLAR`] of a dollar — `Offering$PricePerUnit` read with
    /// `Offering$PriceUnits` ([`PRICE_UNITS_HOURLY`]).
    pub price_per_hour: i64,
}

/// The reservation catalogue this service publishes: one offering per entry, each
/// reserving that much outbound bandwidth for [`OFFERING_DURATION_MONTHS`].
///
/// An offering is the SERVICE's own catalogue entry rather than a resource a caller creates
/// (ListOfferings "displays a list of all offerings that are available to this account",
/// and nothing in the model creates one), and the model carries no price list — it fixes
/// only what an entry MEANS: outbound bandwidth in Mbps, priced per hour
/// ([`PRICE_UNITS_HOURLY`]) in US dollars ([`CURRENCY_CODE_USD`]). The amounts below are
/// therefore this emulator's catalogue, as a published reservation price list is shaped:
/// each entry prices its own amount of bandwidth, with the rate per Mbps falling as the
/// amount rises (a reservation is a volume commitment), so no entry's price is a
/// multiplication of another's.
///
/// Ordered by the amount reserved, which [`offering_id`] also makes the order of their
/// ARNs.
pub const OFFERING_CATALOGUE: &[PublishedOffering] = &[
    PublishedOffering { reserved_bitrate: 50, price_per_hour: 1_200 },
    PublishedOffering { reserved_bitrate: 100, price_per_hour: 2_300 },
    PublishedOffering { reserved_bitrate: 200, price_per_hour: 4_400 },
    PublishedOffering { reserved_bitrate: 300, price_per_hour: 6_300 },
    PublishedOffering { reserved_bitrate: 500, price_per_hour: 10_000 },
    PublishedOffering { reserved_bitrate: 750, price_per_hour: 14_250 },
    PublishedOffering { reserved_bitrate: 1_000, price_per_hour: 18_000 },
    PublishedOffering { reserved_bitrate: 1_500, price_per_hour: 25_500 },
    PublishedOffering { reserved_bitrate: 2_000, price_per_hour: 32_000 },
    PublishedOffering { reserved_bitrate: 3_000, price_per_hour: 45_000 },
    PublishedOffering { reserved_bitrate: 5_000, price_per_hour: 70_000 },
    PublishedOffering { reserved_bitrate: 10_000, price_per_hour: 130_000 },
];

/// `Offering$PricePerUnit` for a price held in thousandths of a dollar
/// ([`PRICE_THOUSANDTHS_PER_DOLLAR`]): the decimal string the member is modelled as
/// (`PricePerUnit` targets `String`), quoted at the scale the catalogue publishes.
///
/// The ONE rendering of a price, so the price a response carries and the price
/// [`offering_description`] states cannot differ.
pub fn offering_price_per_unit(price_per_hour: i64) -> String {
    format!(
        "{}.{:03}",
        price_per_hour / PRICE_THOUSANDTHS_PER_DOLLAR,
        (price_per_hour % PRICE_THOUSANDTHS_PER_DOLLAR).abs()
    )
}

/// `Offering$OfferingDescription`, "a description of the offering": what the offering
/// reserves and what it costs, in the wording this service publishes a reservation
/// offering under — `Reserved outbound bandwidth of <N>Mbps at $<price>/hour`.
///
/// Both variable parts are the entry's own terms, rendered from the same values the
/// response's `ResourceSpecification$ReservedBitrate` and `PricePerUnit` carry, so a
/// description cannot state terms the offering is not offered on. The one wording of an
/// offering's description, so the catalogue and a reservation bought from it
/// (`Reservation$OfferingDescription`) cannot describe one offering two ways.
pub fn offering_description(reserved_bitrate: i32, price_per_unit: &str) -> String {
    format!("Reserved outbound bandwidth of {reserved_bitrate}Mbps at ${price_per_unit}/hour")
}

/// The offering id an offering ARN ends in: the digits that name one catalogue entry.
///
/// An offering id is NUMERIC, and it is minted from the entry's own terms — the term in
/// months, then the amount of bandwidth reserved, each zero-padded — so it is stable: an
/// offering is a catalogue entry rather than a record a request mints, so its id must be
/// the same for every caller and in every process, which is also what lets a `NextToken`
/// naming one resume a listing. The fixed width keeps the ids', and so the ARNs',
/// lexicographic order the ascending order of the amounts they reserve.
pub fn offering_id(reserved_bitrate: i32, duration_months: i32) -> String {
    format!("{duration_months:02}{reserved_bitrate:06}")
}

/// The resource fragment of an offering ARN: `offering:<offering id>`.
///
/// The model puts no pattern on `OfferingArn`
/// (`smithy_query(query="shape",shape="OfferingArn")`: a plain string carrying an
/// `@arnReference` to `AWS::MediaConnect::Offering`), so the form is the one this service
/// publishes its catalogue under: the resource type, then the numeric [`offering_id`].
pub fn offering_resource_fragment(reserved_bitrate: i32, duration_months: i32) -> String {
    format!(
        "{OFFERING_RESOURCE_TYPE}:{}",
        offering_id(reserved_bitrate, duration_months)
    )
}

/// The resource type of an offering ARN.
pub const OFFERING_RESOURCE_TYPE: &str = "offering";

/// `Offering$OfferingArn`, "the Amazon Resource Name (ARN) that MediaConnect assigns to
/// the offering", for the entry `resource_fragment` names in `region`:
/// `arn:aws:mediaconnect:<region>:offering:<offering id>`.
///
/// An offering ARN carries NO account-id field — the catalogue belongs to the service, not
/// to one account, and every account in the Region is offered the same entries — so
/// `::engine::emulator_arn`, which always writes `::engine::emulator_account_id()` into
/// that field, cannot mint this form; the partition and the service namespace are the ones
/// it mints every other ARN of this service with ([`SERVICE_NAMESPACE`]). A client that
/// parses an offering ARN therefore reads the Region out of the fourth field and the
/// offering id out of the last.
pub fn offering_arn(region: &str, resource_fragment: &str) -> String {
    format!("arn:aws:{SERVICE_NAMESPACE}:{region}:{resource_fragment}")
}

/// Whether `arn` is an offering ARN this service could have minted: the account-less
/// prefix [`offering_arn`] mints — the `arn` prefix, an `aws*` partition, this service's
/// namespace, a region token and the `offering` resource type — then a numeric
/// [`offering_id`].
///
/// ListOfferings' `NextToken` is the ARN its next page resumes at, so a token the service
/// never minted is a semantic error in the request rather than a key to page from. The
/// offering it names need NOT be in the catalogue: paging must survive a catalogue that
/// does not hold that entry.
pub fn is_mintable_offering_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, resource_type, offering_id] = segments.as_slice()
    else {
        return false;
    };
    prefix == &"arn"
        && is_aws_partition(partition)
        && service == &SERVICE_NAMESPACE
        && is_region_token(region)
        && resource_type == &OFFERING_RESOURCE_TYPE
        && !offering_id.is_empty()
        && offering_id.bytes().all(|b| b.is_ascii_digit())
}

/// This service's whole offering catalogue in `region`, in ARN order: one `Offering` per
/// entry of [`OFFERING_CATALOGUE`], with the terms that entry is offered on.
///
/// The ONE projection of the catalogue, so the operations that read an offering — the
/// listing, and a purchase copying its terms onto the reservation it mints — cannot report
/// one entry differently. The Region is a field of every offering ARN, so the catalogue is
/// projected for the Region the request was signed for.
pub fn published_offerings(region: &str) -> Vec<Offering> {
    let mut offerings: Vec<Offering> = OFFERING_CATALOGUE
        .iter()
        .map(|entry| {
            let price_per_unit = offering_price_per_unit(entry.price_per_hour);
            Offering {
                offering_arn: offering_arn(
                    region,
                    &offering_resource_fragment(entry.reserved_bitrate, OFFERING_DURATION_MONTHS),
                ),
                offering_description: offering_description(entry.reserved_bitrate, &price_per_unit),
                currency_code: CURRENCY_CODE_USD.to_string(),
                price_per_unit,
                price_units: PRICE_UNITS_HOURLY.to_string(),
                duration: OFFERING_DURATION_MONTHS,
                duration_units: DURATION_UNITS_MONTHS.to_string(),
                resource_specification: ResourceSpecification {
                    reserved_bitrate: Some(entry.reserved_bitrate),
                    resource_type: RESOURCE_TYPE_MBPS_OUTBOUND_BANDWIDTH.to_string(),
                },
            }
        })
        .collect();
    // The listing pages from an ARN, so the order it reports the catalogue in is the order
    // of those ARNs — sorted here rather than assumed of the ladder above.
    offerings.sort_by(|left, right| left.offering_arn.cmp(&right.offering_arn));
    offerings
}

/// The published offering `offering_arn` addresses in `region`, or `None` when this
/// service's catalogue publishes no offering under that ARN.
///
/// The ONE place an `OfferingArn` is resolved, and it resolves it against
/// [`published_offerings`] — the same projection the listing reports — so the ARN a client
/// read from a listing is exactly the ARN that resolves here, and a described offering
/// cannot drift from a listed one. An ARN naming another Region's catalogue, another
/// account, another service, another resource type, or an entry this catalogue does not
/// hold, addresses no offering: the Region and the entry are both part of an offering's
/// identity.
pub fn published_offering(region: &str, offering_arn: &str) -> Option<Offering> {
    published_offerings(region)
        .into_iter()
        .find(|offering| offering.offering_arn == offering_arn)
}

/// The documented wire form of `Reservation$Start` and `Reservation$End`: "Use the
/// following format: `YYYY-MM-DDTHH:mm:SSZ`, where `T` and `Z` are literal characters"
/// (`smithy_query(query="doc",shape="PurchaseOfferingRequest",member="Start")`).
///
/// Both members are modelled as plain strings rather than timestamps, so the service
/// renders them itself. A reservation record keeps the two as instants
/// ([`StoredReservation::start_at`], [`StoredReservation::end_at`]) because the clock
/// decides when the reservation has expired; this is the one place they become the string
/// a response carries, truncated to the second, which is all that form holds.
pub fn format_reservation_instant(instant: SystemTime) -> String {
    let epoch_seconds = match instant.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        // Before the epoch: the error carries the distance back to the instant.
        Err(before_epoch) => -i64::try_from(before_epoch.duration().as_secs()).unwrap_or(i64::MAX),
    };
    ::aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // An instant no RFC-3339 date can hold is reported as the empty string rather
        // than failing the read; no instant this service stores is one.
        .unwrap_or_default()
}

/// A reservation the account holds: "a pricing agreement for a discounted rate for a
/// specific outbound bandwidth" (`smithy_query(query="shape",shape="Reservation")`).
///
/// Every term of the offering — its duration, price, description and resource
/// specification — is COPIED onto the record when the offering is purchased, because
/// "MediaConnect defines this value in the offering"
/// (`smithy_query(query="shape",shape="Reservation",member="Duration")`): the reservation
/// is the agreement that was struck, so a later change to the offering catalogue cannot
/// rewrite a reservation already bought.
#[derive(Debug, Clone)]
pub struct StoredReservation {
    /// `Reservation$ReservationArn`, the ARN this service minted for the reservation and
    /// the identity DescribeReservation addresses it by (`@httpLabel ReservationArn`).
    pub reservation_arn: String,
    /// `Reservation$ReservationName`, "the name that you assigned to the reservation when
    /// you purchased the offering".
    pub reservation_name: String,
    /// `Reservation$OfferingArn`, the offering this reservation was bought from.
    pub offering_arn: String,
    /// `Reservation$OfferingDescription`, that offering's description.
    pub offering_description: String,
    /// `Reservation$CurrencyCode`.
    pub currency_code: String,
    /// `Reservation$PricePerUnit`, which with [`Self::price_units`] "makes up the rate".
    pub price_per_unit: String,
    /// `Reservation$PriceUnits` — a `PriceUnits` value.
    pub price_units: String,
    /// `Reservation$Duration`, "the length of time that this reservation is active", in
    /// [`Self::duration_units`].
    pub duration: i32,
    /// `Reservation$DurationUnits` — a `DurationUnits` value.
    pub duration_units: String,
    /// `Reservation$ResourceSpecification`, "a definition of the amount of outbound
    /// bandwidth" the reservation covers.
    pub resource_specification: ResourceSpecification,
    /// When the reservation becomes active — `Reservation$Start`, "you set this value when
    /// you purchase the offering". Rendered by [`format_reservation_instant`].
    pub start_at: SystemTime,
    /// When the reservation expires — `Reservation$End`, "calculated based on the start
    /// date and time that you set and the offering's duration". Rendered by
    /// [`format_reservation_instant`], and the instant from which [`Self::settle`] reports
    /// the reservation `EXPIRED`.
    pub end_at: SystemTime,
    /// `Reservation$ReservationState` — one of the [`RESERVATION_STATE_PROCESSING`] values.
    pub reservation_state: String,
    /// The reservation-state change the clock owes, or `None` when it owes none — the
    /// same mechanism every other record in this store uses (see [`PendingStatus`]).
    pub pending: Option<PendingStatus>,
}

impl StoredReservation {
    /// Materialise the reservation-state change the clock already made due, if it is due
    /// at `now`.
    ///
    /// Two changes are clock-owed, and neither waits on anybody calling: the state a
    /// purchase settles into one [`TRANSITION_SPAN`] after it was accepted, and EXPIRY —
    /// `End` is "the day and time that this reservation expires"
    /// (`smithy_query(query="shape",shape="Reservation",member="End")`), so from that
    /// instant on the reservation is expired whether or not it is read. Both terminal
    /// states stay put: an expired reservation does not come back, and a canceled one is
    /// never expired on top.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.reservation_state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
        let runs_out = self.reservation_state == RESERVATION_STATE_PROCESSING
            || self.reservation_state == RESERVATION_STATE_ACTIVE;
        if runs_out && now >= self.end_at {
            self.reservation_state = RESERVATION_STATE_EXPIRED.to_string();
            // Expiry is terminal, so a settle still owed would otherwise move the
            // reservation back out of it.
            self.pending = None;
        }
    }

    /// The modelled `Reservation` for this record — the one projection every reservation
    /// operation answers from, so no two of them can report one reservation differently.
    pub fn to_reservation(&self) -> Reservation {
        Reservation {
            currency_code: self.currency_code.clone(),
            duration: self.duration,
            duration_units: self.duration_units.clone(),
            end: format_reservation_instant(self.end_at),
            offering_arn: self.offering_arn.clone(),
            offering_description: self.offering_description.clone(),
            price_per_unit: self.price_per_unit.clone(),
            price_units: self.price_units.clone(),
            reservation_arn: self.reservation_arn.clone(),
            reservation_name: self.reservation_name.clone(),
            reservation_state: self.reservation_state.clone(),
            resource_specification: self.resource_specification.clone(),
            start: format_reservation_instant(self.start_at),
        }
    }
}
