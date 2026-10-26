// SPDX-License-Identifier: CC0-1.0

//! What the clock does to a long-term pricing contract when it reaches the end of its term,
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of contracts rather than over a single one: a contract
//! bought with automatic renewal is renewed for another whole term each time it reaches the
//! end of the one it is running, so it is reported running the first term whose end is still
//! ahead of the call and keeps the status the service holds it under; a contract bought
//! without automatic renewal keeps the end date it was stored with and is reported expired
//! from that instant on. Renewing moves nothing but the end date: the contract's start, term,
//! device type and auto-renew choice read back unchanged.
//!
//! The rows differ along what the rule generalises over: the `LongTermPricingType` (every
//! value of the modelled catalog, so every term length), the `SnowballType` (several values
//! of the modelled catalog), whether the contract renews itself, and where the clock stands
//! against its term (inside it, just past one term's end, and several terms past it, so a
//! contract nobody read for several terms is as many terms on).
//!
//! The renewed instant itself is pinned separately, against a hand-written calendar table
//! with an explicit reading of the clock, since no request can move this process's clock
//! across a month boundary — the split `long_term_pricing_listing` already makes for the
//! term arithmetic a create applies.
//!
//! The harness (`router_with_engine`, `ok`, `string`, `entries`, `epoch_seconds`,
//! `long_term_pricing_id`, `term_months`) is the one this crate's other suites drive the
//! service with: every call goes through the same front door a client uses. A contract whose
//! term has already run out can only be stood up by writing it into the service's own store —
//! a create always starts its term now — so the rows are seeded through the insertion point
//! `CreateLongTermPricing` writes through, and everything the rule is about still travels over
//! the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The calendar arithmetic a term is measured with, read from the crate rather than restated,
// so a row cannot drift from the behaviour it exercises.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_long_term_pricing::plus_calendar_months;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::LONG_TERM_PRICING_TYPE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    service_state, StoredLongTermPricing, LONG_TERM_PRICING_STATUS_ACTIVE,
    LONG_TERM_PRICING_STATUS_EXPIRED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_creation::long_term_pricing_id;
use super::job_lifecycle::{ok, router_with_engine, string};
use super::long_term_pricing_listing::{entries, epoch_seconds, term_months};

/// Seconds in a day, the unit the rows below age a contract by.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The longest a calendar month can be, in days — the calendar's own bound, used only to age
/// a seeded contract far enough back that a whole number of its terms has certainly run out.
const LONGEST_MONTH_DAYS: u64 = 31;

/// How far a reported instant may sit from the instant the row seeded: the protocol puts a
/// timestamp on the wire as epoch seconds, and every seeded instant is a whole second.
const TIMESTAMP_TOLERANCE_SECONDS: f64 = 1.0;

/// Several values of the modelled `SnowballType` catalog
/// (`shared_types::SNOWBALL_TYPE_VALUES`), cycled through the rows so the rule is exercised
/// on contracts covering different devices: a small Snowcone, two Snowball Edge devices and a
/// Snowblade, none of which the issue this suite pins cited.
const SEEDED_DEVICES: &[&str] = &["EDGE", "EDGE_C", "SNC1_HDD", "V3_5S"];

/// Where the clock stands against a seeded contract's first term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// The term the contract was bought for still has time to run.
    InsideFirstTerm,
    /// That term ran out some terms ago: `1` is just past its end, more than one is a
    /// contract nobody read across several terms.
    TermsPast(u64),
}

/// One seeded contract: what it was bought for, whether it renews itself, and where the clock
/// stands against its term.
struct Row {
    case: String,
    long_term_pricing_id: String,
    pricing_type: &'static str,
    snowball_type: &'static str,
    auto_renew: bool,
    standing: Standing,
    /// The term this contract names, in whole calendar months.
    months: u32,
    start: SystemTime,
    /// The end of the term the contract was bought for, which is what a create stores.
    end: SystemTime,
}

impl Row {
    /// Whether the term this contract was bought for has run out by now.
    fn lapsed(&self) -> bool {
        self.standing != Standing::InsideFirstTerm
    }
}

/// `instant` as the epoch seconds the protocol puts a timestamp on the wire as.
fn epoch_of(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// The instant `seconds` after the epoch.
fn instant_of(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Every seeded row: each `LongTermPricingType` the modelled catalog admits, against a
/// contract that renews itself and one that does not, against each standing of the clock.
fn rows(now: SystemTime) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut devices = SEEDED_DEVICES.iter().cycle();
    for pricing_type in LONG_TERM_PRICING_TYPE_VALUES {
        let months = term_months(pricing_type)
            .unwrap_or_else(|| panic!("the crate's term table covers {pricing_type:?}"));
        for auto_renew in [false, true] {
            for standing in [
                Standing::InsideFirstTerm,
                Standing::TermsPast(1),
                Standing::TermsPast(3),
            ] {
                let snowball_type = devices.next().expect("the device cycle never ends");
                // A term of `months` calendar months is at most `31 * months` days long, so a
                // contract started that many days ago — plus a day, so no boundary lands on
                // this instant — has certainly run that term out, and `terms` of them.
                let days_back = match standing {
                    Standing::InsideFirstTerm => 1,
                    Standing::TermsPast(terms) => LONGEST_MONTH_DAYS * u64::from(months) * terms + 1,
                };
                let start = now - Duration::from_secs(days_back * SECONDS_PER_DAY);
                let end = plus_calendar_months(start, months)
                    .expect("a term inside the range a timestamp can represent");
                let standing_label = match standing {
                    Standing::InsideFirstTerm => "inside the term it was bought for".to_string(),
                    Standing::TermsPast(terms) => format!("{terms} term(s) past that term's end"),
                };
                rows.push(Row {
                    case: format!(
                        "a {pricing_type} contract on {snowball_type} with auto-renew \
                         {auto_renew}, {standing_label}"
                    ),
                    long_term_pricing_id: long_term_pricing_id(rows.len() + 1),
                    pricing_type,
                    snowball_type,
                    auto_renew,
                    standing,
                    months,
                    start,
                    end,
                });
            }
        }
    }
    rows
}

/// Write `rows` into the service's own store, through the same insertion point
/// `CreateLongTermPricing` writes through, so what is stood up is the record that operation
/// stores.
fn seed(engine: &Arc<engine::InMemoryEngine>, rows: &[Row]) {
    let state = service_state(&**engine).expect("this service's store is on the engine");
    let mut store = state.lock();
    for row in rows {
        assert!(
            store.insert_long_term_pricing(StoredLongTermPricing {
                long_term_pricing_id: row.long_term_pricing_id.clone(),
                long_term_pricing_type: row.pricing_type.to_string(),
                snowball_type: row.snowball_type.to_string(),
                is_long_term_pricing_auto_renew: row.auto_renew,
                long_term_pricing_start_date: row.start,
                long_term_pricing_end_date: row.end,
                // The status a create stores: a contract whose upfront payment has been taken
                // is one the service is holding to.
                long_term_pricing_status: LONG_TERM_PRICING_STATUS_ACTIVE.to_string(),
                replacement_job: None,
            }),
            "case {:?}: expected the store to hold the seeded contract",
            row.case
        );
    }
}

/// The contract the store holds under `pricing_id`.
fn stored(
    engine: &Arc<engine::InMemoryEngine>,
    pricing_id: &str,
    case: &str,
) -> StoredLongTermPricing {
    let state = service_state(&**engine).expect("this service's store is on the engine");
    let store = state.lock();
    store
        .long_term_pricing
        .get(pricing_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a contract stored under {pricing_id:?}"))
}

/// Every entry `ListLongTermPricing` reports, keyed by the contract it is about.
async fn listed_by_id(router: &MultiServiceRouter, case: &str) -> Vec<(String, Value)> {
    let listed = ok(
        router,
        "ListLongTermPricing",
        json!({ "MaxResults": 100 }),
        case,
    )
    .await;
    entries(&listed)
        .into_iter()
        .map(|entry| (string(&entry, "LongTermPricingId", case), entry))
        .collect()
}

/// The entry reported for `pricing_id`.
fn entry_for(listed: &[(String, Value)], pricing_id: &str, case: &str) -> Value {
    listed
        .iter()
        .find(|(id, _)| id == pricing_id)
        .map(|(_, entry)| entry.clone())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an entry for {pricing_id:?}, got {:?}",
                listed.iter().map(|(id, _)| id).collect::<Vec<_>>()
            )
        })
}

#[tokio::test]
async fn a_contract_that_renews_itself_runs_a_fresh_term_instead_of_expiring() {
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();
    let rows = rows(now);
    seed(&engine, &rows);

    let listed = listed_by_id(&router, "every seeded contract, read over the wire").await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case \"every seeded contract\": expected one entry per seeded contract, got {}",
        listed.len()
    );

    for row in &rows {
        let case = row.case.clone();
        let entry = entry_for(&listed, &row.long_term_pricing_id, &case);

        // What the contract was bought for is not what a renewal touches.
        assert_eq!(
            string(&entry, "LongTermPricingType", &case),
            row.pricing_type,
            "case {case:?}: expected the term the contract was bought for, got {entry}"
        );
        assert_eq!(
            string(&entry, "SnowballType", &case),
            row.snowball_type,
            "case {case:?}: expected the device type the contract covers, got {entry}"
        );
        assert_eq!(
            entry
                .get("IsLongTermPricingAutoRenew")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            row.auto_renew,
            "case {case:?}: expected the auto-renew choice the contract carries, got {entry}"
        );
        let reported_start = epoch_seconds(&entry, "LongTermPricingStartDate", &case);
        assert!(
            (reported_start - epoch_of(row.start)).abs() < TIMESTAMP_TOLERANCE_SECONDS,
            "case {case:?}: expected the contract's own start date {}, got {reported_start}",
            epoch_of(row.start)
        );

        let reported_end = epoch_seconds(&entry, "LongTermPricingEndDate", &case);
        let reported_status = string(&entry, "LongTermPricingStatus", &case);
        let stored_after = stored(&engine, &row.long_term_pricing_id, &case);
        match (row.lapsed(), row.auto_renew) {
            // A term with time left to run is the term being reported, whatever the
            // auto-renew choice: there is nothing for the clock to have done yet.
            (false, _) => {
                assert!(
                    (reported_end - epoch_of(row.end)).abs() < TIMESTAMP_TOLERANCE_SECONDS,
                    "case {case:?}: expected the term's own end {}, got {reported_end}",
                    epoch_of(row.end)
                );
                assert_eq!(
                    reported_status, LONG_TERM_PRICING_STATUS_ACTIVE,
                    "case {case:?}: expected a contract inside its term to be reported active, \
                     got {entry}"
                );
            }
            // A contract that does not renew itself is over at its end date, which stands.
            (true, false) => {
                assert!(
                    (reported_end - epoch_of(row.end)).abs() < TIMESTAMP_TOLERANCE_SECONDS,
                    "case {case:?}: expected the end date the contract was stored with {}, got \
                     {reported_end}",
                    epoch_of(row.end)
                );
                assert_eq!(
                    reported_status, LONG_TERM_PRICING_STATUS_EXPIRED,
                    "case {case:?}: expected a lapsed contract that does not renew itself to be \
                     reported expired, got {entry}"
                );
                assert_eq!(
                    stored_after.long_term_pricing_status, LONG_TERM_PRICING_STATUS_EXPIRED,
                    "case {case:?}: expected the store to hold the expiry its end date made \
                     due, got {:?}",
                    stored_after.long_term_pricing_status
                );
            }
            // A contract that renews itself is running a later term: one that has not ended
            // yet, and no further out than one whole term from now, so it is the FIRST
            // boundary still ahead rather than some arbitrary future instant.
            (true, true) => {
                let one_term_ahead = plus_calendar_months(now, row.months)
                    .expect("a term inside the range a timestamp can represent");
                assert!(
                    reported_end > epoch_of(now)
                        && reported_end <= epoch_of(one_term_ahead) + TIMESTAMP_TOLERANCE_SECONDS,
                    "case {case:?}: expected a renewed term ending after {} and no later than \
                     {}, got {reported_end}",
                    epoch_of(now),
                    epoch_of(one_term_ahead)
                );
                assert!(
                    reported_end > epoch_of(row.end),
                    "case {case:?}: expected the renewal to move the end date on from {}, got \
                     {reported_end}",
                    epoch_of(row.end)
                );
                assert_eq!(
                    reported_status, LONG_TERM_PRICING_STATUS_ACTIVE,
                    "case {case:?}: expected a contract that renews itself to still be held to, \
                     got {entry}"
                );
                assert!(
                    (epoch_of(stored_after.long_term_pricing_end_date) - reported_end).abs()
                        < TIMESTAMP_TOLERANCE_SECONDS,
                    "case {case:?}: expected the store to hold the renewed term ending \
                     {reported_end}, got {}",
                    epoch_of(stored_after.long_term_pricing_end_date)
                );
            }
        }
    }

    // Reading the listing again reports exactly the same window and status: no boundary falls
    // between two calls moments apart, so what one caller is told is what the next sees.
    let again = listed_by_id(&router, "the same listing, read a second time").await;
    for row in &rows {
        let case = format!("{} read a second time", row.case);
        let first = entry_for(&listed, &row.long_term_pricing_id, &case);
        let second = entry_for(&again, &row.long_term_pricing_id, &case);
        assert_eq!(
            second, first,
            "case {case:?}: expected the second read to report the same entry as the first, got \
             {second} against {first}"
        );
    }
}

/// One calendar row: a contract's stored term end, its term, whether it renews itself and the
/// status the store holds it under, against an explicit reading of the clock — and the end
/// date and status that reading leaves it with.
struct CalendarRow {
    case: &'static str,
    /// The end of the term the contract holds, as an instant spelled in UTC.
    stored_end_label: &'static str,
    stored_end: u64,
    pricing_type: &'static str,
    auto_renew: bool,
    stored_status: &'static str,
    now: u64,
    expected_end_label: &'static str,
    expected_end: u64,
    expected_status: &'static str,
}

/// The calendar a renewal follows, written out by hand.
///
/// Source: the proleptic Gregorian calendar, the one the terms of this service are measured
/// in (`long_term_pricing_listing::CALENDAR_ROWS` writes the same calendar out for the term a
/// create measures). Each row names the instants in UTC beside their epoch seconds: a renewal
/// runs the term's whole calendar months on from the end of the term before, clamping a day
/// the target month does not have, and keeps the time of day the term runs to; renewals chain
/// until one lands ahead of the clock, so a contract nobody read across several terms is as
/// many terms on.
const CALENDAR_ROWS: &[CalendarRow] = &[
    // A one-month term that ran out yesterday is renewed into the month it had reached: the
    // 31st clamps to the 28th of a 28-day February.
    CalendarRow {
        case: "a one-month term that ran out yesterday, renewing",
        stored_end_label: "2026-01-31T00:00:00Z",
        stored_end: 1_769_817_600,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_769_904_000, // 2026-02-01T00:00:00Z
        expected_end_label: "2026-02-28T00:00:00Z",
        expected_end: 1_772_236_800,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // The same contract read two months later: the renewals chain 2026-01-31 -> 2026-02-28 ->
    // 2026-03-28 -> 2026-04-28, each a whole month on from the one before.
    CalendarRow {
        case: "a one-month term renewed across several months, the clamped day carried on",
        stored_end_label: "2026-01-31T00:00:00Z",
        stored_end: 1_769_817_600,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_775_779_200, // 2026-04-10T00:00:00Z
        expected_end_label: "2026-04-28T00:00:00Z",
        expected_end: 1_777_334_400,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // Renewal happens "before the long-term pricing contract expires", so a term reaching its
    // end this very instant is renewed rather than run out.
    CalendarRow {
        case: "a one-month term reaching its end this instant, renewing",
        stored_end_label: "2026-02-28T00:00:00Z",
        stored_end: 1_772_236_800,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_772_236_800, // the end date itself
        expected_end_label: "2026-03-28T00:00:00Z",
        expected_end: 1_774_656_000,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // The same instant on a contract that does not renew itself: that is where it ends.
    CalendarRow {
        case: "a one-month term reaching its end this instant, not renewing",
        stored_end_label: "2026-02-28T00:00:00Z",
        stored_end: 1_772_236_800,
        pricing_type: "OneMonth",
        auto_renew: false,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_772_236_800,
        expected_end_label: "2026-02-28T00:00:00Z",
        expected_end: 1_772_236_800,
        expected_status: LONG_TERM_PRICING_STATUS_EXPIRED,
    },
    // A one-year term bought on a leap day: 2024-02-29 -> 2025-02-28 -> 2026-02-28.
    CalendarRow {
        case: "a one-year term ending on a leap day, renewed twice",
        stored_end_label: "2024-02-29T00:00:00Z",
        stored_end: 1_709_164_800,
        pricing_type: "OneYear",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_767_225_600, // 2026-01-01T00:00:00Z
        expected_end_label: "2026-02-28T00:00:00Z",
        expected_end: 1_772_236_800,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // Four one-year renewals on: 2025-02-28 -> 2026-02-28 -> 2027-02-28 -> 2028-02-28 ->
    // 2029-02-28, the first of them ahead of the clock.
    CalendarRow {
        case: "a one-year term renewed four times",
        stored_end_label: "2025-02-28T00:00:00Z",
        stored_end: 1_740_700_800,
        pricing_type: "OneYear",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_861_920_000, // 2029-01-01T00:00:00Z
        expected_end_label: "2029-02-28T00:00:00Z",
        expected_end: 1_866_931_200,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // A three-year term renews for another three years: 2027-02-28 -> 2030-02-28.
    CalendarRow {
        case: "a three-year term renewed once",
        stored_end_label: "2027-02-28T00:00:00Z",
        stored_end: 1_803_772_800,
        pricing_type: "ThreeYear",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_861_920_000, // 2029-01-01T00:00:00Z
        expected_end_label: "2030-02-28T00:00:00Z",
        expected_end: 1_898_467_200,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // A renewal keeps the time of day the term runs to.
    CalendarRow {
        case: "a one-month term renewed a second after it ran out, keeping its time of day",
        stored_end_label: "2026-04-15T12:34:56Z",
        stored_end: 1_776_256_496,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_776_256_497,
        expected_end_label: "2026-05-15T12:34:56Z",
        expected_end: 1_778_848_496,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // A term with time left to run is nobody's to move, either way the flag is set.
    CalendarRow {
        case: "a term still running, renewing",
        stored_end_label: "2026-04-15T00:00:00Z",
        stored_end: 1_776_211_200,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_773_532_800, // 2026-03-15T00:00:00Z
        expected_end_label: "2026-04-15T00:00:00Z",
        expected_end: 1_776_211_200,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    CalendarRow {
        case: "a three-year term still running, not renewing",
        stored_end_label: "2029-03-15T00:00:00Z",
        stored_end: 1_868_227_200,
        pricing_type: "ThreeYear",
        auto_renew: false,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_773_532_800, // 2026-03-15T00:00:00Z
        expected_end_label: "2029-03-15T00:00:00Z",
        expected_end: 1_868_227_200,
        expected_status: LONG_TERM_PRICING_STATUS_ACTIVE,
    },
    // A contract that does not renew itself keeps the end date it ran out on, however long ago
    // that was.
    CalendarRow {
        case: "a one-month term that ran out months ago, not renewing",
        stored_end_label: "2026-01-31T00:00:00Z",
        stored_end: 1_769_817_600,
        pricing_type: "OneMonth",
        auto_renew: false,
        stored_status: LONG_TERM_PRICING_STATUS_ACTIVE,
        now: 1_775_779_200, // 2026-04-10T00:00:00Z
        expected_end_label: "2026-01-31T00:00:00Z",
        expected_end: 1_769_817_600,
        expected_status: LONG_TERM_PRICING_STATUS_EXPIRED,
    },
    // A contract the service has already stopped holding to has no term left to renew: asking
    // for renewal afterwards does not put it back on one.
    CalendarRow {
        case: "a contract already expired, carrying the auto-renew flag afterwards",
        stored_end_label: "2026-01-31T00:00:00Z",
        stored_end: 1_769_817_600,
        pricing_type: "OneMonth",
        auto_renew: true,
        stored_status: LONG_TERM_PRICING_STATUS_EXPIRED,
        now: 1_775_779_200, // 2026-04-10T00:00:00Z
        expected_end_label: "2026-01-31T00:00:00Z",
        expected_end: 1_769_817_600,
        expected_status: LONG_TERM_PRICING_STATUS_EXPIRED,
    },
];

#[test]
fn a_renewal_runs_a_whole_term_on_from_the_term_before() {
    for row in CALENDAR_ROWS {
        let case = row.case;
        let now = instant_of(row.now);
        let months = term_months(row.pricing_type)
            .unwrap_or_else(|| panic!("case {case:?}: the term table covers {}", row.pricing_type));
        let mut contract = StoredLongTermPricing {
            long_term_pricing_id: long_term_pricing_id(1),
            long_term_pricing_type: row.pricing_type.to_string(),
            snowball_type: SEEDED_DEVICES[0].to_string(),
            is_long_term_pricing_auto_renew: row.auto_renew,
            // The start is not what a renewal reads; it is set a term's worth of days before
            // the end so the record is one a create could have left.
            long_term_pricing_start_date: instant_of(
                row.stored_end - u64::from(months) * 28 * SECONDS_PER_DAY,
            ),
            long_term_pricing_end_date: instant_of(row.stored_end),
            long_term_pricing_status: row.stored_status.to_string(),
            replacement_job: None,
        };

        let derived_end = contract.end_date_at(now);
        assert_eq!(
            derived_end,
            instant_of(row.expected_end),
            "case {case:?}: expected a {} term ending {} to be running to {} ({}) at {}, got {}",
            row.pricing_type,
            row.stored_end_label,
            row.expected_end_label,
            row.expected_end,
            row.now,
            epoch_of(derived_end)
        );
        assert_eq!(
            contract.status_at(now),
            row.expected_status,
            "case {case:?}: expected the status {} at {}, got {}",
            row.expected_status,
            row.now,
            contract.status_at(now)
        );

        // Writing the change the boundary already made due leaves the record saying exactly
        // what the derivation reports, and doing it again changes nothing further.
        contract.settle_term_at(now);
        assert_eq!(
            contract.long_term_pricing_end_date,
            instant_of(row.expected_end),
            "case {case:?}: expected the settled record to hold the term ending {} ({}), got {}",
            row.expected_end_label,
            row.expected_end,
            epoch_of(contract.long_term_pricing_end_date)
        );
        assert_eq!(
            contract.long_term_pricing_status, row.expected_status,
            "case {case:?}: expected the settled record to hold the status {}, got {}",
            row.expected_status, contract.long_term_pricing_status
        );
        let settled_once = contract.clone();
        contract.settle_term_at(now);
        assert_eq!(
            (
                contract.long_term_pricing_end_date,
                contract.long_term_pricing_status.clone()
            ),
            (
                settled_once.long_term_pricing_end_date,
                settled_once.long_term_pricing_status.clone()
            ),
            "case {case:?}: expected settling the same instant twice to change nothing, got {} \
             and {}",
            epoch_of(contract.long_term_pricing_end_date),
            contract.long_term_pricing_status
        );
    }
}

#[tokio::test]
async fn switching_the_auto_renew_choice_lands_on_the_term_the_clock_has_the_contract_in() {
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();

    // A one-month contract whose first term ran out over a month ago: one that renews itself,
    // and one that does not.
    let months = term_months("OneMonth").expect("the term table covers OneMonth");
    let start = now - Duration::from_secs(40 * SECONDS_PER_DAY);
    let end = plus_calendar_months(start, months).expect("a term inside the timestamp range");
    let renewing = Row {
        case: "a lapsed contract that renews itself, switched to not renewing".to_string(),
        long_term_pricing_id: long_term_pricing_id(101),
        pricing_type: "OneMonth",
        snowball_type: SEEDED_DEVICES[0],
        auto_renew: true,
        standing: Standing::TermsPast(1),
        months,
        start,
        end,
    };
    let lapsed = Row {
        case: "a lapsed contract that does not renew itself, switched to renewing".to_string(),
        long_term_pricing_id: long_term_pricing_id(102),
        pricing_type: "OneMonth",
        snowball_type: SEEDED_DEVICES[1],
        auto_renew: false,
        standing: Standing::TermsPast(1),
        months,
        start,
        end,
    };
    let rows = vec![renewing, lapsed];
    seed(&engine, &rows);

    let before = listed_by_id(&router, "the two lapsed contracts, before the update").await;
    let renewed_entry = entry_for(&before, &rows[0].long_term_pricing_id, &rows[0].case);
    let renewed_end = epoch_seconds(&renewed_entry, "LongTermPricingEndDate", &rows[0].case);
    assert_eq!(
        string(&renewed_entry, "LongTermPricingStatus", &rows[0].case),
        LONG_TERM_PRICING_STATUS_ACTIVE,
        "case {:?}: expected the renewing contract to be held to before the update, got \
         {renewed_entry}",
        rows[0].case
    );
    let expired_entry = entry_for(&before, &rows[1].long_term_pricing_id, &rows[1].case);
    assert_eq!(
        string(&expired_entry, "LongTermPricingStatus", &rows[1].case),
        LONG_TERM_PRICING_STATUS_EXPIRED,
        "case {:?}: expected the non-renewing contract to be expired before the update, got \
         {expired_entry}",
        rows[1].case
    );

    // Switch each contract's auto-renew choice the other way.
    for (row, choice) in [(&rows[0], false), (&rows[1], true)] {
        ok(
            &router,
            "UpdateLongTermPricing",
            json!({
                "LongTermPricingId": row.long_term_pricing_id,
                "IsLongTermPricingAutoRenew": choice,
            }),
            &row.case,
        )
        .await;
    }

    let after = listed_by_id(&router, "the two contracts, after the update").await;
    // A term already renewed is a term bought: switching renewal off inside it does not end
    // it early.
    let case = rows[0].case.clone();
    let entry = entry_for(&after, &rows[0].long_term_pricing_id, &case);
    assert_eq!(
        string(&entry, "LongTermPricingStatus", &case),
        LONG_TERM_PRICING_STATUS_ACTIVE,
        "case {case:?}: expected the renewed term to still be held to, got {entry}"
    );
    let end_after = epoch_seconds(&entry, "LongTermPricingEndDate", &case);
    assert!(
        (end_after - renewed_end).abs() < TIMESTAMP_TOLERANCE_SECONDS,
        "case {case:?}: expected the renewed term to keep ending {renewed_end}, got {end_after}"
    );
    assert!(
        !entry
            .get("IsLongTermPricingAutoRenew")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "case {case:?}: expected the update's auto-renew choice to be reported, got {entry}"
    );

    // A contract whose last term has run out has nothing left to renew.
    let case = rows[1].case.clone();
    let entry = entry_for(&after, &rows[1].long_term_pricing_id, &case);
    assert_eq!(
        string(&entry, "LongTermPricingStatus", &case),
        LONG_TERM_PRICING_STATUS_EXPIRED,
        "case {case:?}: expected a contract whose term ran out to stay expired, got {entry}"
    );
    let end_after = epoch_seconds(&entry, "LongTermPricingEndDate", &case);
    assert!(
        (end_after - epoch_of(rows[1].end)).abs() < TIMESTAMP_TOLERANCE_SECONDS,
        "case {case:?}: expected the end date it ran out on {}, got {end_after}",
        epoch_of(rows[1].end)
    );
}
