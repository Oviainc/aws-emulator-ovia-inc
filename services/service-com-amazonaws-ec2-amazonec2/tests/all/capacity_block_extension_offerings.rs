// SPDX-License-Identifier: CC0-1.0

//! `DescribeCapacityBlockExtensionOfferings` end-to-end, over the wire the service actually
//! speaks: an `application/x-www-form-urlencoded` ec2Query POST carrying
//! `Action=DescribeCapacityBlockExtensionOfferings`, dispatched through the same
//! `MultiServiceRouter` the emulator serves with and signed for a Region, which is where the
//! service reads the Region it is being asked about.
//!
//! The rules under test:
//!
//! 1. The extension is COMPUTED FROM THE RESERVATION it would extend: the capacity, the
//!    zone, the tenancy and the block's own start date are the stored reservation's, the
//!    extension starts at that reservation's end date and closes exactly
//!    `CapacityBlockExtensionDurationHours` later, and the fee is the reserved units times
//!    those hours at the one rate this service prices a Capacity Block with.
//! 2. An extension offering is DERIVED, NOT STORED: its id is a function of its own terms, so
//!    the same request answers the same id, the search writes nothing, and the id decodes
//!    back to the window, zone and counts it was reported with. A reservation with no
//!    Capacity Block capacity left to extend is answered with no offering at all.
//! 3. Each refusal answers EC2's own code for the rule the request broke: an id of the
//!    resource-id shape that names no reservation of the account in this Region is not found,
//!    a value that is no Capacity Reservation id at all is malformed, a duration that extends
//!    nothing or runs past the longest term a Capacity Block is sold for is refused, a dry run
//!    performs nothing, and a token this operation never issued is refused.
//!
//! Every table row is a distinct input along what those rules generalise over — the Region
//! and so the zone, the instance type including a block of EC2 UltraServers, the instance
//! count, the block's own duration and the extension's, including both published bounds of
//! the extension and one past the upper one — so no single recorded search is the
//! specification.

use std::time::{Duration, SystemTime};

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    availability_zones_in_region, capacity_block_upfront_fee, decode_capacity_block_offering_id,
    iso8601_utc, CapacityBlockPurchaseSpec, CapacityBlockSpec, CapacityReservationSpec,
    CAPACITY_BLOCK_OFFERING_CURRENCY_CODE, CAPACITY_BLOCK_OFFERING_TENANCY,
    CAPACITY_RESERVATION_STATE_ACTIVE, INSTANCE_MATCH_CRITERIA_TARGETED,
    ZONE_TYPE_AVAILABILITY_ZONE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, error_code, error_message};
use super::wire::{child_text, document};

/// The Region a case is signed for unless its row says otherwise.
const DEFAULT_REGION: &str = "us-east-1";

/// The seconds in one hour, the unit `CapacityBlockExtensionDurationHours` is given in.
const SECONDS_PER_HOUR: f64 = 3_600.0;

/// The longest term a Capacity Block is sold for, in hours: 182 days in 7-day increments
/// ("in 7-day increments up to 182 days",
/// `DescribeCapacityBlockOfferingsRequest$CapacityDurationHours`), which is the ceiling the
/// handler bounds an extension's duration by.
const MAX_TERM_HOURS: i32 = 24 * 182;

/// One form field.
fn field(name: &str, value: impl ToString) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `DescribeCapacityBlockExtensionOfferings` call signed for `region`, as its HTTP status
/// and XML body.
async fn describe_extensions(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(
        router,
        region,
        "DescribeCapacityBlockExtensionOfferings",
        params,
    )
    .await
}

/// The Capacity Reservation a row extends, as the service reported it: what every member of
/// the extension offering must be derived from.
#[derive(Debug, Clone)]
struct Reservation {
    id: String,
    instance_type: Option<String>,
    instance_count: Option<String>,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    tenancy: Option<String>,
    start_date: String,
    end_date: String,
}

/// Buy one Capacity Block in `region` for the search `search` names, and answer the Capacity
/// Reservation that holds its capacity.
///
/// The window is asked for two hours out (`StartDateRange`), so the block a row extends is
/// still ahead of its own end date however long the test takes, and for ONE offering
/// (`MaxResults`), so the row buys a single known offering however many zones and instance
/// types its search spans.
async fn block_reservation(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    search: &[(String, String)],
    platform: &str,
) -> Reservation {
    let mut params = search.to_vec();
    params.push(field(
        "StartDateRange",
        iso8601_utc(SystemTime::now() + Duration::from_secs(2 * 60 * 60)),
    ));
    params.push(field("MaxResults", 1));
    let (status, body) =
        super::ec2_query::call(router, region, "DescribeCapacityBlockOfferings", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the search for an offering must succeed, got {status} with body {body}"
    );
    let offered = document(case, &body);
    let item = offered
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, "capacityBlockOfferingId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected an offering in the body {body}"));
    let offering_id = child_text(item, "capacityBlockOfferingId").unwrap_or_default();

    let purchase = vec![
        field("CapacityBlockOfferingId", &offering_id),
        field("InstancePlatform", platform),
    ];
    let (status, body) =
        super::ec2_query::call(router, region, "PurchaseCapacityBlock", &purchase).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the purchase must succeed, got {status} with body {body}"
    );
    let bought = document(case, &body);
    let reservation = bought
        .descendants()
        .find(|node| node.is_element() && child_text(*node, "capacityReservationId").is_some())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Capacity Reservation in the body {body}")
        });
    let required = |name: &str| {
        child_text(reservation, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in the body {body}"))
    };
    Reservation {
        id: required("capacityReservationId"),
        instance_type: child_text(reservation, "instanceType"),
        instance_count: child_text(reservation, "totalInstanceCount"),
        availability_zone: child_text(reservation, "availabilityZone"),
        availability_zone_id: child_text(reservation, "availabilityZoneId"),
        tenancy: child_text(reservation, "tenancy"),
        start_date: required("startDate"),
        end_date: required("endDate"),
    }
}

/// One extension offering of a response. Every member is read as text, so a member the
/// offering does not carry reads as `None` rather than as a default.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Offering {
    offering_id: String,
    instance_type: Option<String>,
    instance_count: Option<String>,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    start_date: Option<String>,
    extension_start_date: Option<String>,
    extension_end_date: Option<String>,
    duration_hours: Option<String>,
    upfront_fee: Option<String>,
    currency_code: Option<String>,
    tenancy: Option<String>,
    zone_type: Option<String>,
}

/// Every extension offering a response carries, in the order it reported them: the `<item>`s
/// of its `capacityBlockExtensionOfferingSet`. Read through the XML document rather than by
/// substring, so a member of the offering is never confused with one of the same name
/// elsewhere in the envelope.
fn offerings(case: &str, body: &str) -> Vec<Offering> {
    let parsed = document(case, body);
    let Some(set) = parsed.descendants().find(|node| {
        node.is_element() && node.tag_name().name() == "capacityBlockExtensionOfferingSet"
    }) else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| Offering {
            offering_id: child_text(item, "capacityBlockExtensionOfferingId")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an offering with no id in the body {body}")
                }),
            instance_type: child_text(item, "instanceType"),
            instance_count: child_text(item, "instanceCount"),
            availability_zone: child_text(item, "availabilityZone"),
            availability_zone_id: child_text(item, "availabilityZoneId"),
            start_date: child_text(item, "startDate"),
            extension_start_date: child_text(item, "capacityBlockExtensionStartDate"),
            extension_end_date: child_text(item, "capacityBlockExtensionEndDate"),
            duration_hours: child_text(item, "capacityBlockExtensionDurationHours"),
            upfront_fee: child_text(item, "upfrontFee"),
            currency_code: child_text(item, "currencyCode"),
            tenancy: child_text(item, "tenancy"),
            zone_type: child_text(item, "zoneType"),
        })
        .collect()
}

/// Whether the body carries a `nextToken` at all.
fn has_next_token(case: &str, body: &str) -> bool {
    document(case, body)
        .descendants()
        .any(|node| node.is_element() && node.tag_name().name() == "nextToken")
}

/// A timestamp the wire carries, as seconds since the Unix epoch.
fn epoch_seconds(case: &str, name: &str, rendered: &str) -> f64 {
    as_timestamp(rendered)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp in {name}, got {rendered:?}"))
}

/// How many Capacity Blocks and Capacity Reservations the store holds: what a report must
/// leave exactly as it found.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (
        guard.capacity_blocks.len(),
        guard.capacity_reservations.len(),
    )
}

/// One row of the extension table: the block it buys and the extension it then asks for.
struct ExtensionCase {
    case: &'static str,
    region: &'static str,
    search: Vec<(String, String)>,
    platform: &'static str,
    extension_hours: i32,
}

/// The extensions the assertions below are made over.
///
/// The rows differ along everything rule 1 generalises over: the Region (and so the zone and
/// its id), the instance type — entries of the modelled `InstanceType` catalog
/// (`smithy_query(query="shape",shape="InstanceType")`) — including a search that names none
/// at all and one for EC2 UltraServers instead of instances, the instance count including the
/// published maximum of 64 (`DescribeCapacityBlockOfferingsRequest$InstanceCount`), the
/// block's own duration in both published increments, and the extension's duration from the
/// shortest there is to the longest term a Capacity Block is sold for.
fn extension_cases() -> Vec<ExtensionCase> {
    vec![
        ExtensionCase {
            case: "one instance for a day, extended by a day",
            region: "us-east-1",
            search: vec![
                field("InstanceType", "p5.48xlarge"),
                field("InstanceCount", 1),
                field("CapacityDurationHours", 24),
            ],
            platform: "Linux/UNIX",
            extension_hours: 24,
        },
        ExtensionCase {
            case: "eight instances for a week, extended by a week",
            region: "eu-west-2",
            search: vec![
                field("InstanceType", "trn2.48xlarge"),
                field("InstanceCount", 8),
                field("CapacityDurationHours", 168),
            ],
            platform: "Red Hat Enterprise Linux",
            extension_hours: 168,
        },
        ExtensionCase {
            case: "a full block of 64, extended by the shortest extension there is",
            region: "us-west-2",
            search: vec![
                field("InstanceType", "p4d.24xlarge"),
                field("InstanceCount", 64),
                field("CapacityDurationHours", 336),
            ],
            platform: "Ubuntu Pro",
            extension_hours: 1,
        },
        ExtensionCase {
            case: "two UltraServers for two days, extended by two days",
            region: "ap-southeast-1",
            search: vec![
                field("UltraserverType", "u-p6e-gb200x72"),
                field("UltraserverCount", 2),
                field("CapacityDurationHours", 48),
            ],
            platform: "Windows",
            extension_hours: 48,
        },
        ExtensionCase {
            case: "a block of any instance type, extended by the longest term there is",
            region: "eu-central-1",
            search: vec![field("CapacityDurationHours", 504)],
            platform: "RHEL with HA and SQL Server Standard",
            extension_hours: MAX_TERM_HOURS,
        },
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: the extension is computed from the reservation it would extend
// ---------------------------------------------------------------------------

/// Every member of the offering is the stored reservation's capacity, zone, tenancy and
/// window plus the duration the request asked for.
#[tokio::test]
async fn the_extension_offered_continues_the_block_it_would_extend() {
    let (router, engine) = super::ec2_query::emulator();

    for row in extension_cases() {
        let case = row.case;
        let reservation =
            block_reservation(&router, row.region, case, &row.search, row.platform).await;
        let before = stored(&engine);
        let params = vec![
            field("CapacityReservationId", &reservation.id),
            field("CapacityBlockExtensionDurationHours", row.extension_hours),
        ];
        let (status, body) = describe_extensions(&router, row.region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = offerings(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one extension of the block, got {reported:?}"
        );
        let offering = &reported[0];

        assert_eq!(
            offering.instance_type, reservation.instance_type,
            "case {case:?}: expected the reservation's instance type {:?}, got {:?}",
            reservation.instance_type, offering.instance_type
        );
        assert_eq!(
            offering.instance_count, reservation.instance_count,
            "case {case:?}: expected the reservation's instance count {:?}, got {:?}",
            reservation.instance_count, offering.instance_count
        );
        assert_eq!(
            offering.availability_zone, reservation.availability_zone,
            "case {case:?}: expected the reservation's zone {:?}, got {:?}",
            reservation.availability_zone, offering.availability_zone
        );
        assert_eq!(
            offering.availability_zone_id, reservation.availability_zone_id,
            "case {case:?}: expected the reservation's zone id {:?}, got {:?}",
            reservation.availability_zone_id, offering.availability_zone_id
        );
        assert_eq!(
            offering.tenancy, reservation.tenancy,
            "case {case:?}: expected the reservation's tenancy {:?}, got {:?}",
            reservation.tenancy, offering.tenancy
        );
        assert_eq!(
            offering.tenancy.as_deref(),
            Some(CAPACITY_BLOCK_OFFERING_TENANCY),
            "case {case:?}: a Capacity Block is sold at one tenancy, got {:?}",
            offering.tenancy
        );
        assert_eq!(
            offering.currency_code.as_deref(),
            Some(CAPACITY_BLOCK_OFFERING_CURRENCY_CODE),
            "case {case:?}: expected the currency every offering is priced in, got {:?}",
            offering.currency_code
        );
        assert_eq!(
            offering.zone_type.as_deref(),
            Some(ZONE_TYPE_AVAILABILITY_ZONE),
            "case {case:?}: expected the type of the zone the capacity sits in, got {:?}",
            offering.zone_type
        );
        assert_eq!(
            offering.duration_hours.as_deref(),
            Some(row.extension_hours.to_string().as_str()),
            "case {case:?}: expected the duration the request asked for, got {:?}",
            offering.duration_hours
        );

        // The window: the block's own start date, an extension opening where the block
        // closes, and closing exactly the requested hours after that.
        let block_start = epoch_seconds(case, "the reservation's startDate", &reservation.start_date);
        let block_end = epoch_seconds(case, "the reservation's endDate", &reservation.end_date);
        let reported_start = offering
            .start_date
            .as_deref()
            .map(|rendered| epoch_seconds(case, "startDate", rendered));
        let extension_start = offering
            .extension_start_date
            .as_deref()
            .map(|rendered| epoch_seconds(case, "capacityBlockExtensionStartDate", rendered));
        let extension_end = offering
            .extension_end_date
            .as_deref()
            .map(|rendered| epoch_seconds(case, "capacityBlockExtensionEndDate", rendered));
        assert_eq!(
            reported_start,
            Some(block_start),
            "case {case:?}: expected the block's own start date {block_start}, got {reported_start:?}"
        );
        assert_eq!(
            extension_start,
            Some(block_end),
            "case {case:?}: an extension starts where the block ends ({block_end}), got \
             {extension_start:?}"
        );
        let expected_end = block_end + f64::from(row.extension_hours) * SECONDS_PER_HOUR;
        assert_eq!(
            extension_end,
            Some(expected_end),
            "case {case:?}: expected the extension to close at {expected_end}, got \
             {extension_end:?}"
        );

        // The fee is the reserved units times the reserved hours at this service's one rate,
        // so an extension costs what the same capacity costs for the same hours.
        let units: u64 = reservation
            .instance_count
            .as_deref()
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the reservation's instance count must be a number ({error}), \
                     got {:?}",
                    reservation.instance_count
                )
            });
        let expected_fee =
            capacity_block_upfront_fee(units, u64::from(row.extension_hours.unsigned_abs()));
        assert_eq!(
            offering.upfront_fee.as_deref(),
            Some(expected_fee.as_str()),
            "case {case:?}: expected the fee for {units} units for {} hours, got {:?}",
            row.extension_hours,
            offering.upfront_fee
        );

        // Rule 2: the id carries the extension's own terms, the search answers the same id
        // again, and nothing was stored by either read.
        let terms = decode_capacity_block_offering_id(&offering.offering_id).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the offering id must carry its own terms, got {:?}",
                offering.offering_id
            )
        });
        assert_eq!(
            (
                terms.start_date_secs as f64,
                terms.end_date_secs as f64,
                terms.availability_zone.clone(),
                terms.availability_zone_id.clone(),
                terms.instance_type.clone(),
            ),
            (
                block_end,
                expected_end,
                reservation.availability_zone.clone().unwrap_or_default(),
                reservation.availability_zone_id.clone().unwrap_or_default(),
                reservation.instance_type.clone(),
            ),
            "case {case:?}: the id must decode to the terms it was reported with, got {terms:?}"
        );
        assert!(
            !has_next_token(case, &body),
            "case {case:?}: one offering is the whole answer, so no page follows it: {body}"
        );

        let (status, again) = describe_extensions(&router, row.region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second search must succeed, got {status} with body {again}"
        );
        let repeated = offerings(case, &again);
        assert_eq!(
            repeated, reported,
            "case {case:?}: the same search must answer the same offering, got {repeated:?}"
        );
        let after = stored(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: a search stores nothing, so the store must be unchanged \
             (was {before:?}, is {after:?})"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a reservation with no Capacity Block capacity to extend is offered nothing
// ---------------------------------------------------------------------------

/// A Capacity Reservation that is not a Capacity Block, and a Capacity Block whose reserved
/// window has already closed, are both answered with no offering rather than with an error or
/// an invented extension.
#[tokio::test]
async fn a_reservation_with_no_block_left_to_extend_is_offered_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    // A plain Capacity Reservation: created directly, so it holds no Capacity Block.
    let case = "a Capacity Reservation that is not a Capacity Block";
    let create = vec![
        field("InstanceType", "m5.large"),
        field("InstancePlatform", "Linux/UNIX"),
        field("InstanceCount", 2),
        field("AvailabilityZone", format!("{DEFAULT_REGION}a")),
    ];
    let (status, body) =
        super::ec2_query::call(&router, DEFAULT_REGION, "CreateCapacityReservation", &create).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} with body {body}"
    );
    let created = document(case, &body);
    let plain_id = created
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "capacityReservationId")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the create must report an id, got {body}"))
        .to_string();

    // A Capacity Block whose window closed an hour ago: seeded through the one store writer a
    // purchase goes through, because no search sells a window in the past.
    let expired_case = "a Capacity Block whose reserved window has closed";
    let expired_id = seed_expired_block(&engine, DEFAULT_REGION);

    for (case, reservation_id) in [(case, plain_id), (expired_case, expired_id)] {
        let params = vec![
            field("CapacityReservationId", &reservation_id),
            field("CapacityBlockExtensionDurationHours", 24),
        ];
        let (status, body) = describe_extensions(&router, DEFAULT_REGION, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = offerings(case, &body);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected no extension offering, got {reported:?}"
        );
    }
}

/// Store one Capacity Block whose reserved window has already closed, and answer the id of
/// the Capacity Reservation that holds its capacity.
fn seed_expired_block(engine: &engine::InMemoryEngine, region: &str) -> String {
    let zone = availability_zones_in_region(region)
        .into_iter()
        .next()
        .expect("every Region this emulator serves has a zone");
    let now = SystemTime::now();
    let start_date = now - Duration::from_secs(48 * 60 * 60);
    let end_date = now - Duration::from_secs(60 * 60);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let (_, capacity_reservation_id) = guard
        .insert_purchased_capacity_block(CapacityBlockPurchaseSpec {
            block: CapacityBlockSpec {
                ultraserver_type: None,
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                capacity_reservation_ids: Vec::new(),
                start_date,
                end_date,
                create_date: start_date,
                pinned_state: None,
                payment_settles_at: None,
                tags: Vec::new(),
            },
            reservation: CapacityReservationSpec {
                instance_type: "p5.48xlarge".to_string(),
                instance_platform: "Linux/UNIX".to_string(),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: region.to_string(),
                tenancy: CAPACITY_BLOCK_OFFERING_TENANCY.to_string(),
                instance_count: 1,
                ebs_optimized: false,
                ephemeral_storage: false,
                end_date: Some(end_date),
                instance_match_criteria: INSTANCE_MATCH_CRITERIA_TARGETED.to_string(),
                created_at: start_date,
                start_date: Some(start_date),
                commitment: None,
                delivery_preference: None,
                state: CAPACITY_RESERVATION_STATE_ACTIVE,
                tags: Vec::new(),
                interruption_info: None,
                zero_size_preference: None,
                pending: None,
            },
        })
        .expect("the store has room for one seeded Capacity Block");
    capacity_reservation_id
}

// ---------------------------------------------------------------------------
// Rule 3: each refusal answers EC2's own code for the rule the request broke
// ---------------------------------------------------------------------------

/// One refusal row: the case, the Region it is signed for, the fields it sends, and the code,
/// status and (where the wording is the contract) the message it must be answered with.
struct RefusalCase {
    case: &'static str,
    region: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
    status: u16,
    message: Option<String>,
}

/// The id-shaped values and durations the refusals are asserted over, plus the two flags.
#[tokio::test]
async fn the_search_refuses_what_it_cannot_answer() {
    let (router, _engine) = super::ec2_query::emulator();
    let search = vec![
        field("InstanceType", "p5.48xlarge"),
        field("InstanceCount", 1),
        field("CapacityDurationHours", 24),
    ];
    let reservation = block_reservation(
        &router,
        DEFAULT_REGION,
        "the block the flag rows name",
        &search,
        "Linux/UNIX",
    )
    .await;

    // A request that names the stored reservation and a duration it is extendable by: what
    // the rows below break exactly one rule of.
    let good = |extra: &[(String, String)]| {
        let mut params = vec![
            field("CapacityReservationId", &reservation.id),
            field("CapacityBlockExtensionDurationHours", 24),
        ];
        params.extend_from_slice(extra);
        params
    };
    // A value of the resource-id SHAPE — `cr-` and 8 or 17 lowercase hexadecimal digits —
    // that names no reservation of the account is reported not found, the all-zero
    // placeholder included; anything outside that shape is malformed.
    let named = |id: &str| {
        vec![
            field("CapacityReservationId", id),
            field("CapacityBlockExtensionDurationHours", 24),
        ]
    };
    let not_found = |id: &str| {
        Some(format!(
            "The capacity-reservation ID '{id}' does not exist"
        ))
    };

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "an id no reservation of the account has",
            region: DEFAULT_REGION,
            params: named("cr-01234567890abcdef"),
            code: "InvalidCapacityReservationId.NotFound",
            status: 400,
            message: not_found("cr-01234567890abcdef"),
        },
        RefusalCase {
            case: "the all-zero id every client reaches for as a placeholder",
            region: DEFAULT_REGION,
            params: named("cr-00000000000000000"),
            code: "InvalidCapacityReservationId.NotFound",
            status: 400,
            message: not_found("cr-00000000000000000"),
        },
        RefusalCase {
            case: "a short-form id of the kind that names nothing",
            region: DEFAULT_REGION,
            params: named("cr-0abc1234"),
            code: "InvalidCapacityReservationId.NotFound",
            status: 400,
            message: not_found("cr-0abc1234"),
        },
        RefusalCase {
            case: "a stored reservation of ANOTHER Region",
            region: "eu-west-2",
            params: named(&reservation.id),
            code: "InvalidCapacityReservationId.NotFound",
            status: 400,
            message: not_found(&reservation.id),
        },
        RefusalCase {
            case: "a value that is not a Capacity Reservation id at all",
            region: DEFAULT_REGION,
            params: named("capacity-reservation-7"),
            code: "InvalidCapacityReservationId.Malformed",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "an id of neither published id length",
            region: DEFAULT_REGION,
            params: named("cr-0123456789"),
            code: "InvalidCapacityReservationId.Malformed",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "an id carrying a digit outside hexadecimal",
            region: DEFAULT_REGION,
            params: named("cr-0123456789abcdefg"),
            code: "InvalidCapacityReservationId.Malformed",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "an extension of no hours at all",
            region: DEFAULT_REGION,
            params: vec![
                field("CapacityReservationId", &reservation.id),
                field("CapacityBlockExtensionDurationHours", 0),
            ],
            code: "InvalidParameterValue",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "an extension of negative hours",
            region: DEFAULT_REGION,
            params: vec![
                field("CapacityReservationId", &reservation.id),
                field("CapacityBlockExtensionDurationHours", -24),
            ],
            code: "InvalidParameterValue",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "an extension one hour past the longest term a Capacity Block is sold for",
            region: DEFAULT_REGION,
            params: vec![
                field("CapacityReservationId", &reservation.id),
                field("CapacityBlockExtensionDurationHours", MAX_TERM_HOURS + 1),
            ],
            code: "InvalidParameterValue",
            status: 400,
            message: None,
        },
        RefusalCase {
            case: "a dry run of a search that would have succeeded",
            region: DEFAULT_REGION,
            params: good(&[field("DryRun", "true")]),
            code: "DryRunOperation",
            status: 412,
            message: None,
        },
        RefusalCase {
            case: "a token this operation never issued",
            region: DEFAULT_REGION,
            params: good(&[field("NextToken", "not-a-token")]),
            code: "InvalidPaginationToken",
            status: 400,
            message: None,
        },
    ];

    for row in cases {
        let case = row.case;
        let (status, body) = describe_extensions(&router, row.region, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.code,
            "case {case:?}: expected the error code {:?}, got {code:?}",
            row.code
        );
        if let Some(expected) = row.message {
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }
}
