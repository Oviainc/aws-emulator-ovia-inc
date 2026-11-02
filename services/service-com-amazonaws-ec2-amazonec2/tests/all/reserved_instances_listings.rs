// SPDX-License-Identifier: CC0-1.0

//! The Reserved Instance Marketplace listings of this account's purchases: what
//! `CreateReservedInstancesListing` puts up for sale, what `CancelReservedInstancesListing`
//! withdraws, what `DescribeReservedInstancesListings` reports back, and the values each
//! refuses — end to end over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rules under test:
//!
//! * A request naming an active Standard Reserved Instance the account holds, offering at
//!   most the reservations that purchase holds, CREATES a listing: it is stored and answered
//!   `active` — the status AWS answers a listing it has just put up for sale — with every
//!   listed reservation `available` and none `sold`, `cancelled` or `pending`, carrying the
//!   price schedules it was given and the `ClientToken` that created it, and
//!   `DescribeReservedInstancesListings` reports exactly that listing back.
//! * Of those price schedules exactly one is in effect: the one whose `Term` is the smallest
//!   that still covers the months remaining in the term, or the largest when none reaches
//!   that far ("A specific price schedule is always in effect, but only one price schedule
//!   can be active at any time … schedule 1, covering the first three months of the
//!   remaining term, will be active during months 5, 4, and 3" —
//!   `smithy_query(query="doc",shape="PriceSchedule",member="Active")`).
//! * Cancelling a listing moves the reservations that were still for sale to `cancelled` and
//!   the listing's own status with them, leaves the purchase itself untouched, is visible to
//!   a later caller through the describe, and frees the purchase to be listed again.
//! * A `listingId` this service cannot act on — a value that is not one of this account's
//!   listing ids, whatever form it takes, or one naming a listing that is already withdrawn
//!   — is refused with HTTP 400 `InvalidInput` and the one sentence EC2 words that refusal
//!   with, and nothing is written. The same error class names the member at fault for the
//!   create's own members, with the reason after it when there is one to give.
//! * A repeated `ClientToken` answers with the listing the first call created rather than
//!   creating a second, and the same token carrying a different request is refused as an
//!   idempotency mismatch.

use std::time::SystemTime;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IDEMPOTENT_PARAMETER_MISMATCH, INVALID_INPUT,
    INVALID_PARAMETER_VALUE, MAX_RESERVED_INSTANCES_PRICE_SCHEDULES, MISSING_PARAMETER,
    RESERVED_INSTANCE_STATE_ACTIVE, RESERVED_INSTANCE_STATE_QUEUED, RESERVED_INSTANCE_STATE_RETIRED,
    RESERVED_INSTANCES_LISTING_STATE_AVAILABLE, RESERVED_INSTANCES_LISTING_STATE_CANCELLED,
    RESERVED_INSTANCES_LISTING_STATE_PENDING, RESERVED_INSTANCES_LISTING_STATE_SOLD,
    RESERVED_INSTANCES_LISTING_STATUS_ACTIVE, RESERVED_INSTANCES_LISTING_STATUS_CANCELLED,
    ReservedInstancesSpec, reserved_instances_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The form-field prefix ec2Query flattens `PriceSchedules` into: the member's
/// `@ec2QueryName` (`smithy_query(query="operation",operation="CreateReservedInstancesListing")`
/// gives `PriceSchedules`), which is the key the ec2Query spec resolves for it.
const SCHEDULES: &str = "PriceSchedules";

/// A one-year Reserved Instance term, in seconds — the units `ReservedInstances$Duration` is
/// reported in ("the duration of the Reserved Instance, in seconds").
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// The months in the one-year term of [`ONE_YEAR_SECONDS`]: how many months a price schedule
/// has to cover to be in effect for a purchase listed the moment it started.
const ONE_YEAR_MONTHS: i64 = 12;

/// The only currency "for transacting the Reserved Instance resale"
/// (`PriceSchedule$CurrencyCode`), and the one member of the modelled `CurrencyCodeValues`
/// enum — so the currency a listing reports whether or not the request named one.
const CURRENCY: &str = "USD";

/// A value in the hyphenated UUID form a listing id takes that names no listing of this
/// account: the store mints its ids from its own counter
/// (`ComAmazonawsEc2Amazonec2State::mint_reserved_instances_listing_id`), so nothing it holds
/// can read like this.
const UNKNOWN_UUID: &str = "11111111-2222-4333-8444-555555555555";

/// One price schedule a row asks for: the months it covers, the price for that term, and the
/// currency it names (`None` leaves the member out, which the store fills in).
type Schedule = (i64, f64, Option<&'static str>);

/// One row of the create table: the case, how many reservations the purchase holds, how many
/// of them the listing offers, how long the term is, and the price schedules to ask for.
type CreateCase = (&'static str, i32, i32, i64, &'static [Schedule]);

/// One row of the price-schedule table: the case, how long the term is, the months priced,
/// and the month whose schedule must be the one in effect.
type ActiveCase = (&'static str, i64, &'static [i64], i64);

/// One row of the price-schedule refusal table: the case, the `PriceSchedules` form fields to
/// send exactly as written, and the code and message the refusal must carry.
type ScheduleRefusalCase = (&'static str, Vec<Param>, &'static str, String);

/// One row of the narrowing table: the case, the request's narrowing fields, and which of the
/// suite's listings it must report, by their creation order.
type NarrowCase = (&'static str, Vec<Param>, &'static [usize]);

/// One listing as a response reports it — the members these tests assert.
#[derive(Debug, Clone, PartialEq)]
struct ReportedListing {
    listing_id: String,
    reserved_instances_id: String,
    client_token: String,
    status: String,
    status_message: String,
    /// The `instanceCounts` entries, in the order the response carries them.
    counts: Vec<(String, i32)>,
    /// The `priceSchedules` entries: the term, the price, the currency and whether that
    /// schedule is the one in effect.
    schedules: Vec<(i64, f64, String, bool)>,
    create_date: Option<f64>,
    update_date: Option<f64>,
}

/// The form fields for a whole `PriceSchedules` list, as ec2Query flattens it:
/// `PriceSchedules.<N>.<Member>`, each member under its own `@ec2QueryName`.
fn schedule_params(schedules: &[Schedule]) -> Vec<Param> {
    schedules
        .iter()
        .enumerate()
        .flat_map(|(index, (term, price, currency))| {
            let position = index + 1;
            let mut fields = vec![
                param(&format!("{SCHEDULES}.{position}.Term"), &term.to_string()),
                param(&format!("{SCHEDULES}.{position}.Price"), &price.to_string()),
            ];
            if let Some(currency) = currency {
                fields.push(param(
                    &format!("{SCHEDULES}.{position}.CurrencyCode"),
                    currency,
                ));
            }
            fields
        })
        .collect()
}

/// One `CreateReservedInstancesListing` call, as its HTTP status and XML body.
async fn create(
    router: &MultiServiceRouter,
    reserved_instances_id: &str,
    instance_count: i32,
    schedules: &[Schedule],
    client_token: &str,
) -> (u16, String) {
    let mut params = vec![
        param("ReservedInstancesId", reserved_instances_id),
        param("InstanceCount", &instance_count.to_string()),
        param("ClientToken", client_token),
    ];
    params.extend(schedule_params(schedules));
    super::ec2_query::call(router, REGION, "CreateReservedInstancesListing", &params).await
}

/// One `CreateReservedInstancesListing` call whose `PriceSchedules` fields are given exactly
/// as written, so a row can leave a member of a schedule out.
async fn create_with_fields(
    router: &MultiServiceRouter,
    reserved_instances_id: &str,
    instance_count: i32,
    schedule_fields: &[Param],
    client_token: &str,
) -> (u16, String) {
    let mut params = vec![
        param("ReservedInstancesId", reserved_instances_id),
        param("InstanceCount", &instance_count.to_string()),
        param("ClientToken", client_token),
    ];
    params.extend_from_slice(schedule_fields);
    super::ec2_query::call(router, REGION, "CreateReservedInstancesListing", &params).await
}

/// One `CancelReservedInstancesListing` call naming `listing_id`.
async fn cancel(router: &MultiServiceRouter, listing_id: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "CancelReservedInstancesListing",
        &[param("ReservedInstancesListingId", listing_id)],
    )
    .await
}

/// One `DescribeReservedInstancesListings` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeReservedInstancesListings", params).await
}

/// Every listing a response reports, in the order it carries them.
fn reported(case: &str, body: &str) -> Vec<ReportedListing> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "reservedInstancesListingsSet")
        .into_iter()
        .map(|item| {
            let number = |name: &str| -> Option<f64> {
                child_text(item, name).and_then(|text| super::ec2_query::as_timestamp(&text))
            };
            ReportedListing {
                listing_id: child_text(item, "reservedInstancesListingId").unwrap_or_default(),
                reserved_instances_id: child_text(item, "reservedInstancesId").unwrap_or_default(),
                client_token: child_text(item, "clientToken").unwrap_or_default(),
                status: child_text(item, "status").unwrap_or_default(),
                status_message: child_text(item, "statusMessage").unwrap_or_default(),
                counts: set_items(item, "instanceCounts")
                    .into_iter()
                    .map(|entry| {
                        (
                            child_text(entry, "state").unwrap_or_default(),
                            child_text(entry, "instanceCount")
                                .and_then(|text| text.parse::<i32>().ok())
                                .unwrap_or_default(),
                        )
                    })
                    .collect(),
                schedules: set_items(item, "priceSchedules")
                    .into_iter()
                    .map(|entry| {
                        (
                            child_text(entry, "term")
                                .and_then(|text| text.parse::<i64>().ok())
                                .unwrap_or_default(),
                            child_text(entry, "price")
                                .and_then(|text| text.parse::<f64>().ok())
                                .unwrap_or_default(),
                            child_text(entry, "currencyCode").unwrap_or_default(),
                            child_text(entry, "active").as_deref() == Some("true"),
                        )
                    })
                    .collect(),
                create_date: number("createDate"),
                update_date: number("updateDate"),
            }
        })
        .collect()
}

/// The ONE listing a successful create or cancel reports, having first pinned the status.
fn one_listing(case: &str, status: u16, body: &str) -> ReportedListing {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let listings = reported(case, body);
    assert_eq!(
        listings.len(),
        1,
        "case {case:?}: the answer must carry exactly one listing, got {listings:?}"
    );
    listings.into_iter().next().expect("one listing")
}

/// The instance counts a listing carries when `available` of its reservations are still for
/// sale and `cancelled` of them have been withdrawn: one entry per member of the modelled
/// `ListingState` catalog, in the order the enum declares them.
fn expected_counts(available: i32, cancelled: i32) -> Vec<(String, i32)> {
    vec![
        (
            RESERVED_INSTANCES_LISTING_STATE_AVAILABLE.to_string(),
            available,
        ),
        (RESERVED_INSTANCES_LISTING_STATE_SOLD.to_string(), 0),
        (
            RESERVED_INSTANCES_LISTING_STATE_CANCELLED.to_string(),
            cancelled,
        ),
        (RESERVED_INSTANCES_LISTING_STATE_PENDING.to_string(), 0),
    ]
}

/// A Reserved Instance purchase to seed: `instance_count` reservations of one offering, in
/// `state` and of `offering_class`, whose term is `duration_seconds` long and starts now.
///
/// The enum members are the modelled catalogs' own: `standard` / `convertible` of
/// `OFFERING_CLASS_TYPE_VALUES`, `All Upfront` of `OFFERING_TYPE_VALUES_VALUES`,
/// `Linux/UNIX` of `R_I_PRODUCT_DESCRIPTION_VALUES`, and an `InstanceType` of
/// `INSTANCE_TYPE_VALUES`.
fn purchase(
    state: &str,
    offering_class: &str,
    instance_count: i32,
    duration_seconds: i64,
) -> ReservedInstancesSpec {
    ReservedInstancesSpec {
        instance_type: "t3.micro".to_string(),
        availability_zone: Some("us-east-1a".to_string()),
        region: REGION.to_string(),
        instance_count,
        duration_seconds,
        offering_class: offering_class.to_string(),
        offering_type: "All Upfront".to_string(),
        product_description: "Linux/UNIX".to_string(),
        state: state.to_string(),
        start: SystemTime::now(),
        fixed_price: 500.0,
        usage_price: 0.0,
    }
}

/// Seed one Reserved Instance and answer the id the store minted for it.
fn seed(
    engine: &engine::InMemoryEngine,
    state: &str,
    offering_class: &str,
    instance_count: i32,
    duration_seconds: i64,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(purchase(
            state,
            offering_class,
            instance_count,
            duration_seconds,
        ))
        .expect("the store has room for the few purchases this suite seeds")
}

/// One `active` Standard Reserved Instance of a one-year term holding `instance_count`
/// reservations: what every row that is not about the purchase itself seeds.
fn seed_standard(engine: &engine::InMemoryEngine, instance_count: i32) -> String {
    seed(
        engine,
        RESERVED_INSTANCE_STATE_ACTIVE,
        "standard",
        instance_count,
        ONE_YEAR_SECONDS,
    )
}

/// One listing as the STORE holds it, as a "nothing was written" assertion compares it: its
/// id, its status, and its instance counts.
type StoredListing = (String, String, Vec<(String, i32)>);

/// The status and instance counts the store holds for every listing, by id — what a
/// "nothing was written" assertion compares.
fn stored_listings(engine: &engine::InMemoryEngine) -> Vec<StoredListing> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .reserved_instances_listings
        .iter()
        .map(|(listing_id, record)| {
            (
                listing_id.clone(),
                record.view.status.clone().unwrap_or_default(),
                record
                    .view
                    .instance_counts
                    .iter()
                    .flatten()
                    .map(|entry| {
                        (
                            entry.state.clone().unwrap_or_default(),
                            entry.instance_count.unwrap_or_default(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

/// The state and reservation count the store holds for one Reserved Instance.
fn stored_purchase(
    engine: &engine::InMemoryEngine,
    reserved_instances_id: &str,
) -> Option<(String, i32)> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .reserved_instances
        .get(reserved_instances_id)
        .map(|held| {
            (
                held.state.clone().unwrap_or_default(),
                held.instance_count.unwrap_or_default(),
            )
        })
}

// ---------------------------------------------------------------------------
// What a listing is created as
// ---------------------------------------------------------------------------

/// The rule: a create of an active Standard Reserved Instance the account holds stores and
/// answers one listing, `active`, with every reservation it offers `available` and none in
/// any other state, carrying the price schedules and `ClientToken` it was given — and the
/// describe reports that same listing back.
///
/// The rows differ along what the rule generalises over: how many of the purchase's
/// reservations are listed (all of them, some of them, one), how many price schedules the
/// listing carries, whether a schedule names the currency or leaves it to the service, and
/// how long the term is.
#[tokio::test]
async fn a_create_lists_the_reservations_it_offers_and_the_describe_reports_them_back() {
    // (case, reservations held, reservations listed, term seconds, price schedules)
    let cases: &[CreateCase] = &[
        (
            "every reservation of a one-year purchase, one schedule",
            1,
            1,
            ONE_YEAR_SECONDS,
            &[(ONE_YEAR_MONTHS, 300.0, Some(CURRENCY))],
        ),
        (
            "some of the reservations, a schedule per remaining month",
            4,
            2,
            ONE_YEAR_SECONDS,
            &[
                (12, 300.0, Some(CURRENCY)),
                (6, 150.5, Some(CURRENCY)),
                (1, 25.25, Some(CURRENCY)),
            ],
        ),
        (
            "a schedule that leaves the currency to the service",
            2,
            2,
            ONE_YEAR_SECONDS,
            &[(12, 0.0, None)],
        ),
        (
            "a three-year term listed one reservation at a time",
            3,
            1,
            3 * ONE_YEAR_SECONDS,
            &[(36, 900.0, Some(CURRENCY)), (12, 200.0, None)],
        ),
    ];

    for (case, held, listed, duration_seconds, schedules) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let reserved_instances_id = seed(
            &engine,
            RESERVED_INSTANCE_STATE_ACTIVE,
            "standard",
            *held,
            *duration_seconds,
        );
        let token = format!("token-for-{case}");

        let (status, body) = create(
            &router,
            &reserved_instances_id,
            *listed,
            schedules,
            &token,
        )
        .await;
        let listing = one_listing(case, status, &body);

        assert!(
            reserved_instances_id_is_well_formed(&listing.listing_id),
            "case {case:?}: the answered listing id must be a UUID, got {:?}",
            listing.listing_id
        );
        assert_eq!(
            (
                listing.reserved_instances_id.as_str(),
                listing.client_token.as_str(),
                listing.status.as_str(),
                listing.status_message.as_str()
            ),
            (
                reserved_instances_id.as_str(),
                token.as_str(),
                RESERVED_INSTANCES_LISTING_STATUS_ACTIVE,
                RESERVED_INSTANCES_LISTING_STATUS_ACTIVE
            ),
            "case {case:?}: a new listing names the purchase and the token it was created \
             with and is on sale, got {listing:?}"
        );
        assert_eq!(
            listing.counts,
            expected_counts(*listed, 0),
            "case {case:?}: every listed reservation starts available and none is sold, \
             cancelled or pending, got {:?}",
            listing.counts
        );
        let asked: Vec<(i64, f64, String)> = schedules
            .iter()
            .map(|(term, price, currency)| {
                (
                    *term,
                    *price,
                    currency.unwrap_or(CURRENCY).to_string(),
                )
            })
            .collect();
        let got: Vec<(i64, f64, String)> = listing
            .schedules
            .iter()
            .map(|(term, price, currency, _)| (*term, *price, currency.clone()))
            .collect();
        assert_eq!(
            got, asked,
            "case {case:?}: the listing prices the months it was given, in that order and in \
             the one supported currency, got {:?}",
            listing.schedules
        );
        assert_eq!(
            listing
                .schedules
                .iter()
                .filter(|(_, _, _, active)| *active)
                .count(),
            1,
            "case {case:?}: exactly one price schedule is in effect, got {:?}",
            listing.schedules
        );
        assert!(
            listing.create_date.is_some() && listing.create_date == listing.update_date,
            "case {case:?}: a listing nothing has changed yet was created and updated at the \
             one instant, got {:?} / {:?}",
            listing.create_date,
            listing.update_date
        );

        // Round trip: the describe reports the listing the create stored, and narrowing it to
        // the listing's own id reports that one listing.
        let (described_status, described_body) =
            describe(&router, &[param("ReservedInstancesListingId", &listing.listing_id)]).await;
        assert_eq!(
            described_status, 200,
            "case {case:?}: the describe must succeed, got {described_status}: {described_body}"
        );
        assert_eq!(
            reported(case, &described_body),
            vec![listing.clone()],
            "case {case:?}: the describe must report the listing the create stored, got {}",
            described_body
        );

        // The purchase itself is untouched: listing reservations offers them for sale, it does
        // not spend them.
        assert_eq!(
            stored_purchase(&engine, &reserved_instances_id),
            Some((RESERVED_INSTANCE_STATE_ACTIVE.to_string(), *held)),
            "case {case:?}: the purchase must still be held as it was, got {:?}",
            stored_purchase(&engine, &reserved_instances_id)
        );
    }
}

/// The rule: of a listing's price schedules exactly one is in effect — the one whose `Term`
/// is the smallest that still covers the months remaining in the term, and the one with the
/// largest term when none of them reaches that far.
///
/// The rows differ along what the rule generalises over: the number of months remaining (a
/// one-year and a three-year term), how many schedules there are, whether one covers the
/// whole remainder exactly, whether several do, and whether none does.
#[tokio::test]
async fn the_price_schedule_in_effect_is_the_smallest_term_that_covers_the_months_remaining() {
    // (case, term seconds, the terms priced, the term expected to be in effect)
    let cases: &[ActiveCase] = &[
        (
            "one schedule covering the whole remaining term",
            ONE_YEAR_SECONDS,
            &[12],
            12,
        ),
        (
            "the exact remainder among longer and shorter schedules",
            ONE_YEAR_SECONDS,
            &[1, 6, 12, 24],
            12,
        ),
        (
            "no schedule reaching the remainder takes the longest one",
            ONE_YEAR_SECONDS,
            &[1, 3, 6],
            6,
        ),
        (
            "the smallest schedule that still covers a three-year remainder",
            3 * ONE_YEAR_SECONDS,
            &[12, 36, 48],
            36,
        ),
        (
            "a single schedule shorter than the remainder is still in effect",
            3 * ONE_YEAR_SECONDS,
            &[2],
            2,
        ),
    ];

    for (case, duration_seconds, terms, expected_active) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let reserved_instances_id = seed(
            &engine,
            RESERVED_INSTANCE_STATE_ACTIVE,
            "standard",
            1,
            *duration_seconds,
        );
        let schedules: Vec<Schedule> = terms
            .iter()
            .map(|term| (*term, 10.0, Some(CURRENCY)))
            .collect();

        let (status, body) =
            create(&router, &reserved_instances_id, 1, &schedules, case).await;
        let listing = one_listing(case, status, &body);
        let active: Vec<i64> = listing
            .schedules
            .iter()
            .filter(|(_, _, _, active)| *active)
            .map(|(term, _, _, _)| *term)
            .collect();
        assert_eq!(
            active,
            vec![*expected_active],
            "case {case:?}: the schedule for month {expected_active} must be the one in \
             effect, got {:?}",
            listing.schedules
        );
    }
}

// ---------------------------------------------------------------------------
// What cancelling a listing withdraws
// ---------------------------------------------------------------------------

/// The rule: cancelling a listing moves the reservations that were still for sale to
/// `cancelled` and the listing's own status with them, stamps the listing's update date,
/// leaves the purchase untouched, and is what every later reader of the listing sees.
///
/// The rows differ along what the rule generalises over: how many reservations were on sale,
/// how many price schedules the listing carries, and the length of the term.
#[tokio::test]
async fn cancelling_a_listing_withdraws_the_reservations_that_were_for_sale() {
    // (case, reservations held, reservations listed, term seconds, price schedules)
    let cases: &[CreateCase] = &[
        (
            "a listing of one reservation",
            1,
            1,
            ONE_YEAR_SECONDS,
            &[(12, 300.0, Some(CURRENCY))],
        ),
        (
            "a listing of several reservations priced month by month",
            5,
            3,
            ONE_YEAR_SECONDS,
            &[(12, 300.0, Some(CURRENCY)), (4, 90.0, Some(CURRENCY))],
        ),
        (
            "a listing on a three-year term",
            2,
            2,
            3 * ONE_YEAR_SECONDS,
            &[(36, 800.0, None)],
        ),
    ];

    for (case, held, listed, duration_seconds, schedules) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let reserved_instances_id = seed(
            &engine,
            RESERVED_INSTANCE_STATE_ACTIVE,
            "standard",
            *held,
            *duration_seconds,
        );
        let (status, body) = create(
            &router,
            &reserved_instances_id,
            *listed,
            schedules,
            &format!("token-for-{case}"),
        )
        .await;
        let created = one_listing(case, status, &body);

        let (cancel_status, cancel_body) = cancel(&router, &created.listing_id).await;
        let withdrawn = one_listing(case, cancel_status, &cancel_body);

        assert_eq!(
            (
                withdrawn.listing_id.as_str(),
                withdrawn.status.as_str(),
                withdrawn.status_message.as_str()
            ),
            (
                created.listing_id.as_str(),
                RESERVED_INSTANCES_LISTING_STATUS_CANCELLED,
                RESERVED_INSTANCES_LISTING_STATUS_CANCELLED
            ),
            "case {case:?}: the cancel answers the listing it withdrew, cancelled, got \
             {withdrawn:?}"
        );
        assert_eq!(
            withdrawn.counts,
            expected_counts(0, *listed),
            "case {case:?}: the reservations that were for sale must be cancelled and none \
             left available, got {:?}",
            withdrawn.counts
        );
        assert_eq!(
            withdrawn.create_date, created.create_date,
            "case {case:?}: withdrawing a listing does not re-create it, got {:?} for {:?}",
            withdrawn.create_date, created.create_date
        );
        assert!(
            withdrawn.update_date >= created.update_date,
            "case {case:?}: the withdrawal is the listing's last modification, got {:?} \
             before {:?}",
            withdrawn.update_date,
            created.update_date
        );
        assert_eq!(
            withdrawn.schedules, created.schedules,
            "case {case:?}: withdrawing a listing does not re-price it, got {:?}",
            withdrawn.schedules
        );

        // What a LATER caller sees: the describe reports the withdrawn listing, and the
        // purchase is still held exactly as it was, free to be listed again.
        let (described_status, described_body) = describe(&router, &[]).await;
        assert_eq!(
            described_status, 200,
            "case {case:?}: the describe must succeed, got {described_status}: \
             {described_body}"
        );
        assert_eq!(
            reported(case, &described_body),
            vec![withdrawn.clone()],
            "case {case:?}: a later caller must see the withdrawn listing, got \
             {described_body}"
        );
        assert_eq!(
            stored_purchase(&engine, &reserved_instances_id),
            Some((RESERVED_INSTANCE_STATE_ACTIVE.to_string(), *held)),
            "case {case:?}: the purchase must be untouched by the withdrawal, got {:?}",
            stored_purchase(&engine, &reserved_instances_id)
        );

        let (relisted_status, relisted_body) = create(
            &router,
            &reserved_instances_id,
            *listed,
            schedules,
            &format!("second-token-for-{case}"),
        )
        .await;
        let relisted = one_listing(case, relisted_status, &relisted_body);
        assert_ne!(
            relisted.listing_id, created.listing_id,
            "case {case:?}: listing the purchase again must create a second listing, got \
             {relisted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a listing id this operation cannot act on is answered with
// ---------------------------------------------------------------------------

/// The rule: a `reservedInstancesListingId` this service cannot act on is refused with HTTP
/// 400 `InvalidInput` and the one sentence EC2 words that refusal with, and nothing is
/// written.
///
/// The rows differ along what the rule generalises over: the FORM of the value (a well-formed
/// listing id that names nothing, a value that is not one at all, an id of another kind of
/// resource, an over-long value) and the state of a listing that does exist but has nothing
/// left to withdraw.
#[tokio::test]
async fn a_listing_id_this_operation_cannot_act_on_is_refused_as_an_invalid_listing_id() {
    /// What a row names as the `listingId`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Named {
        /// A literal value, whatever form it takes.
        Literal(&'static str),
        /// The id of the Reserved Instance the row seeded — an id of another kind.
        ThePurchase,
        /// The id of a listing the row created and then cancelled.
        AWithdrawnListing,
    }

    let cases: &[(&str, Named)] = &[
        (
            "a listing id in the right form that names nothing",
            Named::Literal(UNKNOWN_UUID),
        ),
        (
            "a value that is not a listing id at all",
            Named::Literal("not-a-listing-id"),
        ),
        ("an empty-looking placeholder", Named::Literal("-")),
        (
            "an over-long value",
            Named::Literal(
                "0123456789012345678901234567890123456789012345678901234567890123456789\
                 0123456789012345678901234567890123456789012345678901234567890123456789",
            ),
        ),
        ("the Reserved Instance's own id", Named::ThePurchase),
        (
            "a listing that is already withdrawn",
            Named::AWithdrawnListing,
        ),
    ];

    for (case, named) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let reserved_instances_id = seed_standard(&engine, 2);
        let schedules: &[Schedule] = &[(12, 300.0, Some(CURRENCY))];

        let listing_id = match named {
            Named::Literal(value) => (*value).to_string(),
            Named::ThePurchase => reserved_instances_id.clone(),
            Named::AWithdrawnListing => {
                let (status, body) =
                    create(&router, &reserved_instances_id, 2, schedules, case).await;
                let created = one_listing(case, status, &body);
                let (cancel_status, cancel_body) = cancel(&router, &created.listing_id).await;
                one_listing(case, cancel_status, &cancel_body);
                created.listing_id
            }
        };
        let before = stored_listings(&engine);

        let (status, body) = cancel(&router, &listing_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Invalid value for 'listingId'",
            "case {case:?}: expected the sentence EC2 refuses a listingId with, got {body}"
        );
        assert_eq!(
            stored_listings(&engine),
            before,
            "case {case:?}: a refused cancel must write nothing, got {:?}",
            stored_listings(&engine)
        );
    }
}

/// The rule: a create whose member the service can read but cannot act on is refused with
/// the error class that names that member — HTTP 400 `InvalidInput` naming the member, with
/// the reason after it when there is one to give — and nothing is listed.
///
/// The rows differ along what the rule generalises over: which member is at fault
/// (`reservedInstancesId`, `instanceCount`), and why — a purchase the account does not hold,
/// one in each state that has no term to resell, a Convertible one, a count above the
/// reservations held, a count below one, and a purchase that is already on sale.
#[tokio::test]
async fn a_create_member_the_service_cannot_act_on_is_refused_naming_that_member() {
    /// One row: what to seed, what to ask for, and the refusal it must get. `{id}`, `{held}`
    /// and `{listing}` in `message` stand for the seeded purchase, its reservation count and
    /// the listing already on sale.
    #[derive(Debug, Clone, Copy)]
    struct Case {
        case: &'static str,
        state: &'static str,
        offering_class: &'static str,
        held: i32,
        already_listed: bool,
        unknown_id: bool,
        asked: i32,
        code: &'static str,
        message: &'static str,
    }

    /// The row every case starts from: an active Standard purchase of two reservations, both
    /// of them listed.
    const BASE: Case = Case {
        case: "",
        state: RESERVED_INSTANCE_STATE_ACTIVE,
        offering_class: "standard",
        held: 2,
        already_listed: false,
        unknown_id: false,
        asked: 2,
        code: INVALID_INPUT,
        message: "",
    };

    let cases: &[Case] = &[
        Case {
            case: "a Reserved Instance the account does not hold",
            unknown_id: true,
            message: "Invalid value for 'reservedInstancesId'",
            ..BASE
        },
        Case {
            case: "a purchase that has not been made yet",
            state: RESERVED_INSTANCE_STATE_QUEUED,
            message: "Invalid value for 'reservedInstancesId'. Reserved Instance {id} is in \
                      the 'queued' state.",
            ..BASE
        },
        Case {
            case: "a purchase whose term is over",
            state: RESERVED_INSTANCE_STATE_RETIRED,
            message: "Invalid value for 'reservedInstancesId'. Reserved Instance {id} is in \
                      the 'retired' state.",
            ..BASE
        },
        Case {
            case: "a Convertible purchase, which cannot be sold",
            offering_class: "convertible",
            message: "Invalid value for 'reservedInstancesId'. Reserved Instance {id} is \
                      Convertible, and only Standard Reserved Instances can be sold in the \
                      Reserved Instance Marketplace.",
            ..BASE
        },
        Case {
            case: "a purchase that is already on sale",
            already_listed: true,
            message: "Invalid value for 'reservedInstancesId'. Reserved Instance {id} is \
                      already listed in the Reserved Instance Marketplace as {listing}.",
            ..BASE
        },
        Case {
            case: "more reservations than the purchase holds",
            held: 3,
            asked: 4,
            message: "Invalid value for 'instanceCount'. Reserved Instance {id} holds {held} \
                      reservations.",
            ..BASE
        },
        Case {
            case: "no reservations at all",
            asked: 0,
            code: INVALID_PARAMETER_VALUE,
            message: "Invalid value '0' for instanceCount. It must be at least 1",
            ..BASE
        },
    ];

    for row in cases {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(
            &engine,
            row.state,
            row.offering_class,
            row.held,
            ONE_YEAR_SECONDS,
        );
        let schedules: &[Schedule] = &[(12, 300.0, Some(CURRENCY))];
        let mut already = String::new();
        if row.already_listed {
            let (status, body) = create(&router, &seeded, 1, schedules, "the-first-listing").await;
            already = one_listing(case, status, &body).listing_id;
        }
        let named = if row.unknown_id {
            UNKNOWN_UUID.to_string()
        } else {
            seeded.clone()
        };
        let before = stored_listings(&engine);

        let (status, body) = create(&router, &named, row.asked, schedules, case).await;
        let expected = row
            .message
            .replace("{id}", &named)
            .replace("{held}", &row.held.to_string())
            .replace("{listing}", &already);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {}, got {body}",
            row.code
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to name the member at fault, got {body}"
        );
        assert_eq!(
            stored_listings(&engine),
            before,
            "case {case:?}: a refused create must list nothing, got {:?}",
            stored_listings(&engine)
        );
    }
}

/// The rule: a price schedule the service cannot price a month of the term with is refused —
/// `MissingParameter` for a member the schedule leaves out and `InvalidParameterValue` for a
/// value it cannot take — and nothing is listed.
///
/// The rows differ along what the rule generalises over: which member is missing or wrong
/// (the list itself, `term`, `price`), and how (absent, below the first month, the same month
/// twice, negative, more schedules than one listing may carry).
#[tokio::test]
async fn a_price_schedule_the_service_cannot_take_is_refused() {
    // (case, the PriceSchedules form fields, expected code, expected message)
    let too_many: Vec<Param> = schedule_params(
        &(1..=i64::try_from(MAX_RESERVED_INSTANCES_PRICE_SCHEDULES + 1).unwrap_or(i64::MAX))
            .map(|term| (term, 1.0, Some(CURRENCY)))
            .collect::<Vec<Schedule>>(),
    );
    let cases: &[ScheduleRefusalCase] = &[
        (
            "no price schedule at all",
            Vec::new(),
            // `PriceSchedules` is `@required`, so a request that carries no schedule at all
            // is refused by the BOUNDARY validator before this operation runs: the model
            // itself enforces that the member is there, and this operation enforces what the
            // schedules in it have to say.
            "MalformedInput",
            "1 validation error detected: Value null at 'priceSchedules' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a schedule that names no month",
            vec![param(&format!("{SCHEDULES}.1.Price"), "300")],
            MISSING_PARAMETER,
            "The request must contain the parameter term".to_string(),
        ),
        (
            "a schedule that names no price",
            vec![param(&format!("{SCHEDULES}.1.Term"), "12")],
            MISSING_PARAMETER,
            "The request must contain the parameter price".to_string(),
        ),
        (
            "a month before the first month of a term",
            schedule_params(&[(0, 300.0, Some(CURRENCY))]),
            INVALID_PARAMETER_VALUE,
            "Invalid value '0' for term. It must be at least 1".to_string(),
        ),
        (
            "a negative month",
            schedule_params(&[(-3, 300.0, Some(CURRENCY))]),
            INVALID_PARAMETER_VALUE,
            "Invalid value '-3' for term. It must be at least 1".to_string(),
        ),
        (
            "the same month priced twice",
            schedule_params(&[(6, 300.0, Some(CURRENCY)), (6, 100.0, Some(CURRENCY))]),
            INVALID_PARAMETER_VALUE,
            "Invalid value '6' for term. The request prices that month more than once"
                .to_string(),
        ),
        (
            "a negative price",
            schedule_params(&[(12, -1.5, Some(CURRENCY))]),
            INVALID_PARAMETER_VALUE,
            "Invalid value '-1.5' for price. It must be an amount that is not negative"
                .to_string(),
        ),
        (
            "more schedules than one listing may carry",
            too_many,
            INVALID_PARAMETER_VALUE,
            format!(
                "PriceSchedules carries {} schedules, above the maximum of \
                 {MAX_RESERVED_INSTANCES_PRICE_SCHEDULES}",
                MAX_RESERVED_INSTANCES_PRICE_SCHEDULES + 1
            ),
        ),
    ];

    for (case, fields, code, message) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let reserved_instances_id = seed_standard(&engine, 2);

        let (status, body) =
            create_with_fields(&router, &reserved_instances_id, 2, fields, case).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got {body}"
        );
        assert_eq!(
            &error_message(&body),
            message,
            "case {case:?}: expected the refusal to name what the schedule got wrong, got \
             {body}"
        );
        assert!(
            stored_listings(&engine).is_empty(),
            "case {case:?}: a refused create must list nothing, got {:?}",
            stored_listings(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// The rule: a repeated `ClientToken` answers with the listing the first call created rather
/// than creating a second, and the same token carrying a different request is refused as an
/// idempotency mismatch.
///
/// The rows differ in what the second request changes: nothing (the repeat), the count it
/// lists, the months it prices, a price, and the purchase it lists.
#[tokio::test]
async fn a_repeated_client_token_answers_the_first_listing_and_a_changed_request_is_a_mismatch() {
    /// What the second call changes about the request.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Change {
        /// Nothing: the same request again.
        Nothing,
        /// A different `InstanceCount`.
        Count,
        /// A different set of months priced.
        Terms,
        /// A different price for the same month.
        Price,
        /// Another Reserved Instance of the account.
        Purchase,
    }

    let cases: &[(&str, Change)] = &[
        ("the same request again", Change::Nothing),
        ("a different number of reservations", Change::Count),
        ("a different set of months", Change::Terms),
        ("a different price for the same month", Change::Price),
        ("a different Reserved Instance", Change::Purchase),
    ];

    for (case, change) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let first_purchase = seed_standard(&engine, 4);
        let second_purchase = seed_standard(&engine, 4);
        let token = "one-token";
        let schedules: &[Schedule] = &[(12, 300.0, Some(CURRENCY)), (6, 100.0, Some(CURRENCY))];

        let (status, body) = create(&router, &first_purchase, 2, schedules, token).await;
        let created = one_listing(case, status, &body);

        let (purchase_id, asked, repeat_schedules): (&str, i32, Vec<Schedule>) = match change {
            Change::Nothing => (first_purchase.as_str(), 2, schedules.to_vec()),
            Change::Count => (first_purchase.as_str(), 3, schedules.to_vec()),
            Change::Terms => (
                first_purchase.as_str(),
                2,
                vec![(12, 300.0, Some(CURRENCY)), (5, 100.0, Some(CURRENCY))],
            ),
            Change::Price => (
                first_purchase.as_str(),
                2,
                vec![(12, 300.0, Some(CURRENCY)), (6, 110.0, Some(CURRENCY))],
            ),
            Change::Purchase => (second_purchase.as_str(), 2, schedules.to_vec()),
        };

        let (repeat_status, repeat_body) =
            create(&router, purchase_id, asked, &repeat_schedules, token).await;
        if *change == Change::Nothing {
            let repeated = one_listing(case, repeat_status, &repeat_body);
            assert_eq!(
                repeated.listing_id, created.listing_id,
                "case {case:?}: the repeat must answer the listing the first call created, \
                 got {repeated:?}"
            );
        } else {
            assert_eq!(
                repeat_status, 400,
                "case {case:?}: expected HTTP 400, got {repeat_status} with body {repeat_body}"
            );
            assert_eq!(
                error_code(&repeat_body),
                IDEMPOTENT_PARAMETER_MISMATCH,
                "case {case:?}: expected the code {IDEMPOTENT_PARAMETER_MISMATCH}, got \
                 {repeat_body}"
            );
        }
        assert_eq!(
            stored_listings(&engine).len(),
            1,
            "case {case:?}: one token must not create a second listing, got {:?}",
            stored_listings(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe narrows by
// ---------------------------------------------------------------------------

/// The rule: the describe reports this account's listings in id order, narrowed by the ids
/// the request names and by every documented filter it carries, and refuses a filter name it
/// does not document.
///
/// The rows differ along what the rule generalises over: which member or filter narrows the
/// report (listing id, Reserved Instances id, status, status message), whether it selects
/// one, several or none of the listings, and whether two of them are combined.
#[tokio::test]
async fn the_describe_narrows_by_the_ids_and_the_documented_filters() {
    let (router, engine) = super::ec2_query::emulator();
    let first_purchase = seed_standard(&engine, 4);
    let second_purchase = seed_standard(&engine, 4);
    let schedules: &[Schedule] = &[(12, 300.0, Some(CURRENCY))];

    // Two listings of the first purchase — the first of them withdrawn before the second was
    // created, since only one listing of a purchase may be on sale at a time, so a status
    // filter has something to tell apart — and one of the second purchase.
    let (status, body) = create(&router, &first_purchase, 3, schedules, "token-3").await;
    let withdrawn = one_listing("seed", status, &body);
    let (status, body) = cancel(&router, &withdrawn.listing_id).await;
    one_listing("seed", status, &body);
    let (status, body) = create(&router, &first_purchase, 1, schedules, "token-1").await;
    let on_sale = one_listing("seed", status, &body);
    let (status, body) = create(&router, &second_purchase, 2, schedules, "token-2").await;
    let other_purchase = one_listing("seed", status, &body);
    let listings = [
        on_sale.listing_id.clone(),
        other_purchase.listing_id.clone(),
        withdrawn.listing_id.clone(),
    ];

    let cases: &[NarrowCase] = &[
        ("no narrowing at all reports every listing", Vec::new(), &[0, 1, 2]),
        (
            "the listing id member",
            vec![param("ReservedInstancesListingId", &listings[1])],
            &[1],
        ),
        (
            "the Reserved Instances id member",
            vec![param("ReservedInstancesId", &first_purchase)],
            &[0, 2],
        ),
        (
            "a listing id that names nothing",
            vec![param("ReservedInstancesListingId", UNKNOWN_UUID)],
            &[],
        ),
        (
            "the reserved-instances-listing-id filter",
            super::ec2_query::filters(&[(
                "reserved-instances-listing-id",
                &[listings[0].as_str()],
            )]),
            &[0],
        ),
        (
            "the reserved-instances-id filter naming both purchases",
            super::ec2_query::filters(&[(
                "reserved-instances-id",
                &[first_purchase.as_str(), second_purchase.as_str()],
            )]),
            &[0, 1, 2],
        ),
        (
            "the status filter on the listings still for sale",
            super::ec2_query::filters(&[(
                "status",
                &[RESERVED_INSTANCES_LISTING_STATUS_ACTIVE],
            )]),
            &[0, 1],
        ),
        (
            "the status filter on the withdrawn listing",
            super::ec2_query::filters(&[(
                "status",
                &[RESERVED_INSTANCES_LISTING_STATUS_CANCELLED],
            )]),
            &[2],
        ),
        (
            "the status-message filter",
            super::ec2_query::filters(&[(
                "status-message",
                &[RESERVED_INSTANCES_LISTING_STATUS_CANCELLED],
            )]),
            &[2],
        ),
        (
            "a status filter and an id member joined with AND",
            {
                let mut params =
                    vec![param("ReservedInstancesId", &first_purchase)];
                params.extend(super::ec2_query::filters(&[(
                    "status",
                    &[RESERVED_INSTANCES_LISTING_STATUS_ACTIVE],
                )]));
                params
            },
            &[0],
        ),
        (
            "a filter value no listing carries",
            super::ec2_query::filters(&[("status", &["closed"])]),
            &[],
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = describe(&router, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let got: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|listing| listing.listing_id)
            .collect();
        let mut want: Vec<String> = expected
            .iter()
            .map(|index| listings[*index].clone())
            .collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: the describe must report the listings the request narrows to, in \
             id order, got {got:?}"
        );
    }

    // A filter name this operation does not document is refused rather than read as "no
    // results": `tag:` filters a listing's tags are not among the four names the request
    // documents.
    let (status, body) = describe(
        &router,
        &super::ec2_query::filters(&[("tag:Name", &["anything"])]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an undocumented filter must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "an undocumented filter must be refused with {INVALID_PARAMETER_VALUE}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "The filter 'tag:Name' is invalid",
        "an undocumented filter must be named in the refusal, got {body}"
    );
}
