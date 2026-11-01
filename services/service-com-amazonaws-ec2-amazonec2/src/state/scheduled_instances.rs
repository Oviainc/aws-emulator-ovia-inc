// SPDX-License-Identifier: CC0-1.0

//! Scheduled Instances: the capacity a Region has set aside for them, and the schedule
//! arithmetic every operation that reads that capacity shares.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! [`super::reserved_instances`] gives: the store, the words a schedule is described in and
//! the arithmetic that turns a recurrence into hours are one subject, and keeping them
//! together leaves `state.rs` readable. Everything here is re-exported by `crate::state`, so
//! a handler reaches it as `crate::state::scheduled_instance_occurrences_per_year` exactly as
//! it reaches the helpers that live in the parent module.
//!
//! ## Capacity is stored, not derived
//!
//! A Region's Availability Zones and the instance types it offers are DERIVED from the Region
//! name (see [`crate::state`], "Availability Zones are facts of a Region"), because AWS
//! reports the zones and types a Region *has*. Scheduled Instance capacity is not of that
//! kind: it is capacity AWS sets ASIDE for term purchase, so whether a Region holds any is a
//! fact about that Region's inventory rather than about its name — and nothing in the EC2 API
//! sets capacity aside, exactly as nothing in it racks the Outpost behind
//! [`ComAmazonawsEc2Amazonec2State::local_gateways`]. So it is a STORE
//! ([`ComAmazonawsEc2Amazonec2State::scheduled_instance_capacity`]) that starts EMPTY, and a
//! search of an account whose Region holds none is answered with no schedules at all — which
//! is what the recorded `DescribeScheduledInstanceAvailability` response carries: HTTP 200
//! with an empty `scheduledInstanceAvailabilitySet`.
//!
//! The schedule a search reports is then computed from the capacity record and the request:
//! the recurrence says how often a slot comes round
//! ([`scheduled_instance_occurrences_per_year`]), which is what fixes the smallest slot that
//! still meets the 1,200-hour-per-year minimum
//! ([`scheduled_instance_minimum_slot_hours`]), and the record says which zone, instance
//! type and platform the capacity is in and what an hour of it costs.

use super::*;

// ---------------------------------------------------------------------------
// The words a Scheduled Instance schedule is described in
// ---------------------------------------------------------------------------

/// A schedule that comes round every day: the `Daily` frequency of
/// `ScheduledInstanceRecurrenceRequest$Frequency` — "The frequency (`Daily`, `Weekly`, or
/// `Monthly`)".
pub const SCHEDULED_INSTANCE_FREQUENCY_DAILY: &str = "Daily";

/// A schedule that comes round every week: that member's `Weekly` frequency.
pub const SCHEDULED_INSTANCE_FREQUENCY_WEEKLY: &str = "Weekly";

/// A schedule that comes round every month: that member's `Monthly` frequency.
pub const SCHEDULED_INSTANCE_FREQUENCY_MONTHLY: &str = "Monthly";

/// Every frequency a schedule can recur at, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_FREQUENCIES: [&str; 3] = [
    SCHEDULED_INSTANCE_FREQUENCY_DAILY,
    SCHEDULED_INSTANCE_FREQUENCY_WEEKLY,
    SCHEDULED_INSTANCE_FREQUENCY_MONTHLY,
];

/// Occurrence days counted as days of the WEEK: the `DayOfWeek` unit of
/// `ScheduledInstanceRecurrenceRequest$OccurrenceUnit` — "The unit for `OccurrenceDays`
/// (`DayOfWeek` or `DayOfMonth`)".
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_WEEK: &str = "DayOfWeek";

/// Occurrence days counted as days of the MONTH: that member's `DayOfMonth` unit.
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_MONTH: &str = "DayOfMonth";

/// Both units, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNITS: [&str; 2] = [
    SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_WEEK,
    SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_MONTH,
];

/// The platform of a Scheduled Instance that runs Linux: one of the two values
/// `ScheduledInstanceAvailability$Platform` names — "The platform (`Linux/UNIX` or
/// `Windows`)" — and one of the two the `platform` filter selects on.
pub const SCHEDULED_INSTANCE_PLATFORM_LINUX: &str = "Linux/UNIX";

/// The platform of a Scheduled Instance that runs Windows: the other value of that member.
pub const SCHEDULED_INSTANCE_PLATFORM_WINDOWS: &str = "Windows";

/// Both platforms, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_PLATFORMS: [&str; 2] = [
    SCHEDULED_INSTANCE_PLATFORM_LINUX,
    SCHEDULED_INSTANCE_PLATFORM_WINDOWS,
];

/// The network a Scheduled Instance of a Region that has only VPCs runs in:
/// `ScheduledInstanceAvailability$NetworkPlatform` ("The network platform") spelled the way
/// EC2 spells a network platform elsewhere in this API — the `EC2-VPC` of
/// `AccountAttribute`'s `supported-platforms` — since EC2-Classic is gone.
pub const SCHEDULED_INSTANCE_NETWORK_PLATFORM_VPC: &str = "EC2-VPC";

/// The term a Scheduled Instance is bought for, in days: "The minimum term. The only possible
/// value is 365 days" (`ScheduledInstanceAvailability$MinTermDurationInDays`), and the same
/// sentence on `MaxTermDurationInDays` — so one constant answers both members.
pub const SCHEDULED_INSTANCE_TERM_DURATION_DAYS: i32 = 365;

/// The hours a Scheduled Instance schedule must add up to over its term: "You must meet the
/// minimum required duration of 1,200 hours per year"
/// (`DescribeScheduledInstanceAvailability`).
pub const SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR: i32 = 1_200;

/// The duration a slot must stay BELOW, in hours: "This value must be greater than
/// MinSlotDurationInHours and less than 1,720"
/// (`DescribeScheduledInstanceAvailabilityRequest$MaxSlotDurationInHours`) — so the bound is
/// exclusive.
pub const SCHEDULED_INSTANCE_SLOT_DURATION_LIMIT_HOURS: i32 = 1_720;

/// The days one term covers, which is what [`scheduled_instance_occurrences_per_year`] counts
/// a DAILY schedule's slots over: the term is [`SCHEDULED_INSTANCE_TERM_DURATION_DAYS`] days.
pub const SCHEDULED_INSTANCE_DAYS_PER_TERM: i32 = SCHEDULED_INSTANCE_TERM_DURATION_DAYS;

/// The whole weeks one term covers, which a WEEKLY schedule's slots are counted over.
pub const SCHEDULED_INSTANCE_WEEKS_PER_TERM: i32 = SCHEDULED_INSTANCE_DAYS_PER_TERM / 7;

/// The months one term covers, which a MONTHLY schedule's slots are counted over.
pub const SCHEDULED_INSTANCE_MONTHS_PER_TERM: i32 = 12;

/// The days of the week an occurrence day may name: "for a weekly schedule, this is one or
/// more days of the week (1-7, where 1 is Sunday)"
/// (`ScheduledInstanceRecurrenceRequest$OccurrenceDays`).
pub const SCHEDULED_INSTANCE_DAYS_OF_WEEK: ::std::ops::RangeInclusive<i32> = 1..=7;

/// The days of the month an occurrence day may name: "For a monthly schedule, this is one or
/// more days of the month (1-31)" (the same member's doc).
pub const SCHEDULED_INSTANCE_DAYS_OF_MONTH: ::std::ops::RangeInclusive<i32> = 1..=31;

/// How long a purchase token stays good for, in seconds: "The purchase token. This token
/// expires in two hours" (`ScheduledInstanceAvailability$PurchaseToken`).
pub const SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS: i64 = 2 * 60 * 60;

/// The prefix a purchase token carries, so a token this service minted is told from a value a
/// caller assembled before any of its fields are read.
const SCHEDULED_INSTANCE_PURCHASE_TOKEN_PREFIX: &str = "sipt";

/// The marker inside a purchase token saying which kind of thing its fields describe — see
/// [`encode_scheduled_instance_purchase_token`].
const SCHEDULED_INSTANCE_PURCHASE_TOKEN_VERSION: &str = "ec2-sipt-1";

// ---------------------------------------------------------------------------
// The capacity a Region has set aside
// ---------------------------------------------------------------------------

/// One block of Scheduled Instance capacity: the instances of one type, on one platform, in
/// one Availability Zone that AWS has set aside for schedules, and what an hour of one costs.
///
/// It holds what a SEARCH cannot compute — which zone, type and platform have capacity, how
/// many instances are left in it and the published price — and nothing a search computes: the
/// slot, its first start and the term's hours all follow from the request's recurrence and
/// window, so storing them would let one zone's record disagree with another's about
/// arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledInstanceCapacityRecord {
    /// The Availability Zone the capacity sits in, as
    /// [`availability_zones_in_region`] names one (`us-west-2a`). A record in a zone the
    /// request's Region does not have is no capacity that request can be answered with.
    pub availability_zone: String,
    /// The instance type the capacity is for. "You can specify one of the C3, C4, M4, or R3
    /// instance types" (`ScheduledInstanceAvailability$InstanceType`) is what AWS offered
    /// schedules for; the store keeps whatever type its writer set aside rather than a second
    /// catalog of its own.
    pub instance_type: String,
    /// `Linux/UNIX` or `Windows` — see [`SCHEDULED_INSTANCE_PLATFORMS`].
    pub platform: String,
    /// The network platform an instance launched from this capacity runs in — see
    /// [`SCHEDULED_INSTANCE_NETWORK_PLATFORM_VPC`].
    pub network_platform: String,
    /// "The number of available instances" (`ScheduledInstanceAvailability$AvailableInstanceCount`):
    /// what is left of this block, which a purchase draws down.
    pub available_instance_count: i32,
    /// "The hourly price for a single instance"
    /// (`ScheduledInstanceAvailability$HourlyPrice`), as the decimal amount that member
    /// carries. Held rather than derived: a Scheduled Instance price is AWS's published term
    /// price for the capacity, which neither the model nor a request carries.
    pub hourly_price: String,
}

/// The key one capacity record is stored under: its zone, platform and instance type, joined
/// by a space.
///
/// Capacity is one block per zone, platform and instance type — those three are what a
/// `DescribeScheduledInstanceAvailability` filter selects on — so the key is exactly the
/// identity of a block, and a second record for the same three replaces the first instead of
/// being reported twice. The separator sorts ahead of every character a zone, platform or
/// instance type can carry, so one value being a prefix of another cannot reorder two keys —
/// the same construction [`offering_page_cursor`] uses, and the order a page of availability
/// is reported and resumed in.
pub fn scheduled_instance_capacity_key(
    availability_zone: &str,
    platform: &str,
    instance_type: &str,
) -> String {
    format!("{availability_zone} {platform} {instance_type}")
}

impl ComAmazonawsEc2Amazonec2State {
    /// Set `record`'s capacity aside, and answer the key it is stored under.
    ///
    /// The ONE writer of [`Self::scheduled_instance_capacity`], so every block is keyed the
    /// one way ([`scheduled_instance_capacity_key`]) and a reader can page over the store's
    /// own order. Nothing in the EC2 API calls it — AWS sets Scheduled Instance capacity
    /// aside itself, as it racks the Outpost behind [`Self::local_gateways`] — so it is how
    /// an account's inventory is seeded, and it charges nothing against
    /// [`Self::stored_bytes`]: that charge bounds what a REQUEST can leave in the store, and
    /// no request reaches this.
    pub fn insert_scheduled_instance_capacity(
        &mut self,
        record: ScheduledInstanceCapacityRecord,
    ) -> String {
        let key = scheduled_instance_capacity_key(
            &record.availability_zone,
            &record.platform,
            &record.instance_type,
        );
        self.scheduled_instance_capacity
            .insert(key.clone(), record);
        key
    }
}

// ---------------------------------------------------------------------------
// What a recurrence adds up to over one term
// ---------------------------------------------------------------------------

/// How many slots a schedule of `frequency` every `interval` units, on `occurrence_days`
/// days, comes round for in one term; `None` when it comes round for none at all.
///
/// A daily schedule has one slot per day of the term, a weekly one per named day of each
/// week it falls in and a monthly one per named day of each month — divided by the interval,
/// which is "the interval quantity. The interval unit depends on the value of `frequency`"
/// (`ScheduledInstanceRecurrence$Interval`). A schedule that names no occurrence day comes
/// round once per period, which is what a daily schedule always is ("You can't specify this
/// value with a daily schedule") and what a weekly or monthly one that leaves the days to AWS
/// is.
///
/// An interval longer than the term leaves no slot inside it, and that is `None` rather than
/// zero: there is no schedule to report at all, so no slot duration could make it meet the
/// 1,200 hours its term needs.
pub fn scheduled_instance_occurrences_per_year(
    frequency: &str,
    interval: i32,
    occurrence_days: usize,
) -> Option<i32> {
    let interval = interval.max(1);
    let days = i32::try_from(occurrence_days.max(1)).unwrap_or(i32::MAX);
    let periods = match frequency {
        SCHEDULED_INSTANCE_FREQUENCY_DAILY => SCHEDULED_INSTANCE_DAYS_PER_TERM / interval,
        SCHEDULED_INSTANCE_FREQUENCY_WEEKLY => SCHEDULED_INSTANCE_WEEKS_PER_TERM / interval,
        SCHEDULED_INSTANCE_FREQUENCY_MONTHLY => SCHEDULED_INSTANCE_MONTHS_PER_TERM / interval,
        // Unreachable through a validated request: a frequency outside
        // [`SCHEDULED_INSTANCE_FREQUENCIES`] names no schedule, so it comes round never
        // rather than being counted as one of the three.
        _ => 0,
    };
    // A daily schedule names no day, so its count is the periods themselves; for the other
    // two the named days multiply the periods.
    let occurrences = periods.saturating_mul(days);
    (occurrences > 0).then_some(occurrences)
}

/// The shortest slot, in hours, a schedule that comes round `occurrences_per_term` times may
/// have: the smallest whole number of hours that still adds up to
/// [`SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR`] over the term.
///
/// This is the rule "You must meet the minimum required duration of 1,200 hours per year" as
/// arithmetic, and the operation's own examples are what it answers: 4 hours for a daily
/// schedule (365 slots), 24 for a weekly one (52 slots) and 100 for a monthly one (12 slots).
pub fn scheduled_instance_minimum_slot_hours(occurrences_per_term: i32) -> Option<i32> {
    let occurrences = u32::try_from(occurrences_per_term)
        .ok()
        .filter(|occurrences| *occurrences > 0)?;
    let minimum = u32::try_from(SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR).unwrap_or(0);
    // Rounded UP: a slot one hour shorter would leave the term below the minimum. And never
    // to nothing — a slot runs for some whole number of hours, however often it comes round.
    i32::try_from(minimum.div_ceil(occurrences).max(1)).ok()
}

// ---------------------------------------------------------------------------
// The token a purchase is made with
// ---------------------------------------------------------------------------

/// What one reported schedule would be purchased as: the capacity it draws on, the schedule
/// itself, and the instant the token naming it stops being good.
///
/// A schedule is not a stored resource — it is what a search computes from stored capacity
/// and the request — so the terms travel INSIDE the purchase token
/// ([`encode_scheduled_instance_purchase_token`]) rather than in a map keyed by it, exactly
/// as a Capacity Block offering's terms travel inside its offering id
/// ([`CapacityBlockOfferingTerms`]). That is what lets the search report a token without
/// writing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledInstancePurchaseTerms {
    /// The Availability Zone the capacity sits in.
    pub availability_zone: String,
    /// The instance type the schedule is for.
    pub instance_type: String,
    /// The platform those instances run.
    pub platform: String,
    /// When the first slot of the schedule starts, in seconds since the Unix epoch.
    pub first_slot_start_secs: i64,
    /// How many hours one slot runs for.
    pub slot_duration_in_hours: i32,
    /// When the token stops being good, in seconds since the Unix epoch:
    /// [`SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS`] after it was minted.
    pub expires_at_secs: i64,
}

/// The `PurchaseToken` for `terms`: the one derived-id construction this service reports every
/// derived resource under ([`encode_derived_id`]), over the terms, a marker saying they are a
/// Scheduled Instance purchase and a checksum.
///
/// Deriving the token FROM the terms is what makes it resolvable without a store, and the
/// expiry it carries is what makes it expire: a token is good for
/// [`SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS`] from the call that minted it, so two
/// searches a while apart report different tokens for the same schedule — the token is a
/// per-call credential for a purchase, not a name for the schedule.
pub fn encode_scheduled_instance_purchase_token(terms: &ScheduledInstancePurchaseTerms) -> String {
    super::encode_derived_id(
        SCHEDULED_INSTANCE_PURCHASE_TOKEN_PREFIX,
        SCHEDULED_INSTANCE_PURCHASE_TOKEN_VERSION,
        &[
            terms.first_slot_start_secs.to_string(),
            terms.slot_duration_in_hours.to_string(),
            terms.expires_at_secs.to_string(),
            super::derived_id_text_field(Some(terms.availability_zone.as_str())),
            super::derived_id_text_field(Some(terms.instance_type.as_str())),
            super::derived_id_text_field(Some(terms.platform.as_str())),
        ],
    )
}
