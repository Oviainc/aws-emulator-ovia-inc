// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `PurchaseCapacityBlock`, with the
//! `DescribeCapacityBlockOfferings` that sells the offering and the
//! `DescribeCapacityBlocks` / `DescribeCapacityReservations` that read the purchase back,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=PurchaseCapacityBlock`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! Three rules are under test:
//!
//! 1. A purchase BUYS THE OFFERING ITS ID NAMES and persists it: every member of the
//!    Capacity Block it stores and of the Capacity Reservation that holds that block's
//!    capacity is derived from the terms the offering was reported with plus the platform
//!    the purchase names, the two name each other, and both come back out of the describes.
//! 2. The purchase is PAID FOR BEFORE THE CAPACITY IS DELIVERED: both halves are
//!    `payment-pending` until the payment settles, after which each is in the state its
//!    reserved window puts it in — `scheduled` ahead of the window, `active` inside it,
//!    `expired` past it.
//! 3. A value that is NOT an offering id this service reported is refused with
//!    `InvalidCapacityBlockOfferingIdMalformed` (HTTP 400), and nothing is stored.
//!
//! Every table row is a distinct input along what those rules generalise over — the Region
//! and so the zone, the instance type (or a search that names none at all), the instance
//! count including the published maximum, the duration, UltraServers instead of instances,
//! the platform, the tags, and the several ways a value can fail to be an offering id — so
//! no single recorded purchase is the specification.
//!
//! Every response is read with the XML parser rather than by substring, so a member of the
//! block is never confused with one of the same name inside its Capacity Reservation id set
//! or its tags.

use std::time::{Duration, SystemTime};

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{iso8601_utc, system_time_to_date_time, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region a case is signed for unless its row says otherwise.
const DEFAULT_REGION: &str = "us-east-1";

/// One form field.
fn field(name: &str, value: impl ToString) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// Long enough past [`TRANSITION_DELAY`] that the payment a purchase started has certainly
/// settled, with margin for a loaded test machine.
fn past_the_payment() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(400)
}

/// One `PurchaseCapacityBlock` call signed for `region`, as its HTTP status and XML body.
async fn purchase(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "PurchaseCapacityBlock", params).await
}

/// One `DescribeCapacityBlocks` call signed for `region`.
async fn describe_blocks(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeCapacityBlocks", params).await
}

/// One `DescribeCapacityReservations` call signed for `region`.
async fn describe_reservations(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeCapacityReservations", params).await
}

/// The first element named `name` anywhere in `body`'s document.
fn element_named<'a, 'i>(
    case: &str,
    body: &str,
    document: &'a roxmltree::Document<'i>,
    name: &str,
) -> roxmltree::Node<'a, 'i> {
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{name}> in the body {body}"))
}

/// The one Capacity Block a `PurchaseCapacityBlock` or `DescribeCapacityBlocks` response
/// carries: the first item of its `capacityBlockSet`.
fn block_of<'a, 'i>(
    case: &str,
    body: &str,
    document: &'a roxmltree::Document<'i>,
) -> roxmltree::Node<'a, 'i> {
    let set = element_named(case, body, document, "capacityBlockSet");
    child(set, "item")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Capacity Block in the body {body}"))
}

/// The one Capacity Reservation a `DescribeCapacityReservations` response carries.
fn described_reservation_of<'a, 'i>(
    case: &str,
    body: &str,
    document: &'a roxmltree::Document<'i>,
) -> roxmltree::Node<'a, 'i> {
    let set = element_named(case, body, document, "capacityReservationSet");
    child(set, "item").unwrap_or_else(|| {
        panic!("case {case:?}: expected a Capacity Reservation in the body {body}")
    })
}

/// The text of `node`'s `name` member, or a failure naming the case.
fn required(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child_text(node, name).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a <{name}> on the <{}> the service reported",
            node.tag_name().name()
        )
    })
}

/// A timestamp the wire carries, as seconds since the Unix epoch. Two members naming one
/// instant are compared AS instants, because the model types some of them
/// `MillisecondDateTime` and others `DateTime`.
fn epoch_seconds(case: &str, name: &str, rendered: &str) -> f64 {
    as_timestamp(rendered).unwrap_or_else(|| {
        panic!("case {case:?}: expected a timestamp in {name}, got {rendered:?}")
    })
}

/// The tags of `node`, as the pairs it reported them.
fn tags_of(node: roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    set_items(node, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The terms one offering was reported with — what a purchase of it must turn into.
#[derive(Debug, Clone)]
struct Offering {
    id: String,
    instance_type: Option<String>,
    availability_zone: String,
    instance_count: Option<String>,
    ultraserver_type: Option<String>,
    ultraserver_count: Option<String>,
    start_date: String,
    end_date: String,
}

impl Offering {
    /// The instance type the Capacity Reservation of a purchase of this offering reserves
    /// capacity for: the offering's own, or — for an offering of EC2 UltraServers, which
    /// names no instance type — the UltraServer type it sells.
    fn reserved_type(&self) -> String {
        self.instance_type
            .clone()
            .or_else(|| self.ultraserver_type.clone())
            .unwrap_or_default()
    }

    /// The number of units the purchase reserves: the offering's instance count, or its
    /// UltraServer count when it is UltraServers that are on sale.
    fn reserved_count(&self) -> String {
        self.instance_count
            .clone()
            .or_else(|| self.ultraserver_count.clone())
            .unwrap_or_default()
    }
}

/// The offering a search reports first, which the rows below buy.
///
/// Every search asks for a window at least an hour out (`StartDateRange`), so a block a row
/// buys is `scheduled` rather than racing the moment its window opens, and for ONE offering
/// (`MaxResults`), so the row buys a single known offering however many zones and instance
/// types its search spans.
async fn offered(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    search: &[(String, String)],
) -> Offering {
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
    let parsed = document(case, &body);
    let set = element_named(case, &body, &parsed, "capacityBlockOfferingSet");
    let item = child(set, "item")
        .unwrap_or_else(|| panic!("case {case:?}: expected an offering in the body {body}"));
    Offering {
        id: required(case, item, "capacityBlockOfferingId"),
        instance_type: child_text(item, "instanceType"),
        availability_zone: required(case, item, "availabilityZone"),
        instance_count: child_text(item, "instanceCount"),
        ultraserver_type: child_text(item, "ultraserverType"),
        ultraserver_count: child_text(item, "ultraserverCount"),
        start_date: required(case, item, "startDate"),
        end_date: required(case, item, "endDate"),
    }
}

/// One row of the purchase table: the search whose offering it buys, the platform it
/// reserves capacity for, and the tags it puts on the block.
struct PurchaseCase {
    case: &'static str,
    region: &'static str,
    search: Vec<(String, String)>,
    platform: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// The purchases the assertions below are made over.
///
/// The rows differ along everything rule 1 generalises over: the Region (and so the zone and
/// its id), the instance type — entries of the modelled `InstanceType` catalog
/// (`smithy_query(query="shape",shape="InstanceType")`) — including a search that names none
/// at all, the instance count including the published maximum of 64
/// (`DescribeCapacityBlockOfferingsRequest$InstanceCount`), the duration in both published
/// increments, a block of EC2 UltraServers instead of instances, the platform (several
/// members of `CapacityReservationInstancePlatform`, including one whose name carries
/// spaces), and the tags (none, one, two).
fn purchase_cases() -> Vec<PurchaseCase> {
    vec![
        PurchaseCase {
            case: "one instance for one day",
            region: "us-east-1",
            search: vec![
                field("InstanceType", "p5.48xlarge"),
                field("InstanceCount", 1),
                field("CapacityDurationHours", 24),
            ],
            platform: "Linux/UNIX",
            tags: Vec::new(),
        },
        PurchaseCase {
            case: "eight instances for a week, another Region",
            region: "eu-west-2",
            search: vec![
                field("InstanceType", "trn2.48xlarge"),
                field("InstanceCount", 8),
                field("CapacityDurationHours", 168),
            ],
            platform: "Red Hat Enterprise Linux",
            tags: vec![("Workload", "training")],
        },
        PurchaseCase {
            case: "a full block of 64 for fourteen days",
            region: "us-west-2",
            search: vec![
                field("InstanceType", "p4d.24xlarge"),
                field("InstanceCount", 64),
                field("CapacityDurationHours", 336),
            ],
            platform: "Ubuntu Pro",
            tags: vec![("Name", "fleet"), ("Team", "ml")],
        },
        PurchaseCase {
            case: "two UltraServers for two days",
            region: "ap-southeast-1",
            search: vec![
                field("UltraserverType", "u-p6e-gb200x72"),
                field("UltraserverCount", 2),
                field("CapacityDurationHours", 48),
            ],
            platform: "Windows",
            tags: vec![("Name", "ultra")],
        },
        PurchaseCase {
            case: "a search that names no instance type",
            region: "eu-central-1",
            search: vec![field("CapacityDurationHours", 504)],
            platform: "RHEL with HA and SQL Server Standard",
            tags: vec![("Name", "any type")],
        },
    ]
}

/// The form fields of one purchase: the offering, the platform, and a `capacity-block` tag
/// specification carrying `tags`.
fn purchase_params(
    offering_id: &str,
    platform: &str,
    tags: &[(&'static str, &'static str)],
) -> Vec<(String, String)> {
    let mut params = vec![
        field("CapacityBlockOfferingId", offering_id),
        field("InstancePlatform", platform),
    ];
    if !tags.is_empty() {
        params.push(field(
            "TagSpecification.1.ResourceType",
            ec2::state::RESOURCE_TYPE_CAPACITY_BLOCK,
        ));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(field(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key,
            ));
            params.push(field(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    params
}

/// How many Capacity Blocks and Capacity Reservations the store holds.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (
        guard.capacity_blocks.len(),
        guard.capacity_reservations.len(),
    )
}

/// The `CapacityBlockResourceState` a describe reports for the one block it is narrowed to.
async fn block_state(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[(String, String)],
) -> String {
    let (status, body) = describe_blocks(router, DEFAULT_REGION, ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing the block must succeed, got {status} with {body}"
    );
    let parsed = document(case, &body);
    required(case, block_of(case, &body, &parsed), "state")
}

/// The `CapacityReservationState` a describe reports for the one reservation it is narrowed
/// to.
async fn reservation_state(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[(String, String)],
) -> String {
    let (status, body) = describe_reservations(router, DEFAULT_REGION, ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing the reservation must succeed, got {status} with {body}"
    );
    let parsed = document(case, &body);
    required(case, described_reservation_of(case, &body, &parsed), "state")
}

// ---------------------------------------------------------------------------
// A purchase buys the offering its id names
// ---------------------------------------------------------------------------

/// Every member of the block and of its Capacity Reservation is the offering's terms and the
/// purchase's platform, the two name each other, and both read back through the describes.
#[tokio::test]
async fn a_purchase_buys_the_offering_its_id_names_and_stores_both_halves() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = purchase_cases();
    let row_count = rows.len();

    for row in rows {
        let case = row.case;
        let offering = offered(&router, row.region, case, &row.search).await;
        let params = purchase_params(&offering.id, row.platform, &row.tags);
        let (status, body) = purchase(&router, row.region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let answer = document(case, &body);

        // The block: the offering's zone, window and UltraServer type, in the state a
        // purchase whose payment has not settled is in.
        let block = block_of(case, &body, &answer);
        let block_id = required(case, block, "capacityBlockId");
        assert!(
            block_id.starts_with("cb-"),
            "case {case:?}: expected a cb- Capacity Block id, got {block_id:?}"
        );
        for (name, expected) in [
            ("availabilityZone", offering.availability_zone.clone()),
            ("state", "payment-pending".to_string()),
        ] {
            let actual = required(case, block, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the block's {name} to be {expected:?}, got {actual:?}"
            );
        }
        // The reserved window the offering was reported with, to the instant.
        for (name, expected) in [
            ("startDate", offering.start_date.as_str()),
            ("endDate", offering.end_date.as_str()),
        ] {
            let actual = required(case, block, name);
            assert_eq!(
                epoch_seconds(case, name, &actual),
                epoch_seconds(case, "the offering's", expected),
                "case {case:?}: expected the block's {name} to be the offering's \
                 {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            child_text(block, "ultraserverType"),
            offering.ultraserver_type,
            "case {case:?}: expected the block's ultraserverType to be the offering's, \
             got {body}"
        );
        assert!(
            child_text(block, "createDate").is_some(),
            "case {case:?}: expected the block to report when it was created, got {body}"
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let reported_tags = tags_of(block);
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the block's tags to be {expected_tags:?}, \
             got {reported_tags:?}"
        );

        // The Capacity Reservation that holds the block's capacity: the offering's counts,
        // zone and window, the purchase's platform, and the link back to the block.
        let reservation = element_named(case, &body, &answer, "capacityReservation");
        let reservation_id = required(case, reservation, "capacityReservationId");
        let named_reservations: Vec<String> = set_items(block, "capacityReservationIdSet")
            .into_iter()
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            named_reservations,
            vec![reservation_id.clone()],
            "case {case:?}: expected the block to name {reservation_id:?} as its Capacity \
             Reservation, got {named_reservations:?}"
        );
        for (name, expected) in [
            ("capacityBlockId", block_id.clone()),
            ("reservationType", "capacity-block".to_string()),
            ("instanceType", offering.reserved_type()),
            ("instancePlatform", row.platform.to_string()),
            ("availabilityZone", offering.availability_zone.clone()),
            ("totalInstanceCount", offering.reserved_count()),
            ("availableInstanceCount", offering.reserved_count()),
            ("instanceMatchCriteria", "targeted".to_string()),
            ("tenancy", "default".to_string()),
            ("state", "payment-pending".to_string()),
            ("endDateType", "limited".to_string()),
        ] {
            let actual = required(case, reservation, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the reservation's {name} to be {expected:?}, \
                 got {actual:?}"
            );
        }
        // It holds the block's capacity, so it holds it for the block's window.
        for (name, expected) in [
            ("startDate", offering.start_date.as_str()),
            ("endDate", offering.end_date.as_str()),
        ] {
            let actual = required(case, reservation, name);
            assert_eq!(
                epoch_seconds(case, name, &actual),
                epoch_seconds(case, "the offering's", expected),
                "case {case:?}: expected the reservation's {name} to be the offering's \
                 {expected:?}, got {actual:?}"
            );
        }
        let arn = required(case, reservation, "capacityReservationArn");
        assert!(
            arn.contains(row.region) && arn.ends_with(&reservation_id),
            "case {case:?}: expected an ARN of {reservation_id:?} in {}, got {arn:?}",
            row.region
        );

        // Both halves read back: a describe reports what the purchase wrote.
        let (block_status, described_block_body) = describe_blocks(
            &router,
            row.region,
            &indexed("CapacityBlockId", &[&block_id]),
        )
        .await;
        assert_eq!(
            block_status, 200,
            "case {case:?}: describing {block_id:?} must succeed, got {described_block_body}"
        );
        let described_block_document = document(case, &described_block_body);
        let described_block = block_of(case, &described_block_body, &described_block_document);
        for name in [
            "capacityBlockId",
            "availabilityZone",
            "availabilityZoneId",
            "startDate",
            "endDate",
            "state",
        ] {
            let described = required(case, described_block, name);
            let answered = required(case, block, name);
            assert_eq!(
                described, answered,
                "case {case:?}: expected the describe to report the block's {name} as the \
                 purchase did ({answered:?}), got {described:?}"
            );
        }
        let (reservation_status, described_reservation_body) = describe_reservations(
            &router,
            row.region,
            &indexed("CapacityReservationId", &[&reservation_id]),
        )
        .await;
        assert_eq!(
            reservation_status, 200,
            "case {case:?}: describing {reservation_id:?} must succeed, \
             got {described_reservation_body}"
        );
        let described_reservation_document = document(case, &described_reservation_body);
        let described_reservation = described_reservation_of(
            case,
            &described_reservation_body,
            &described_reservation_document,
        );
        for name in [
            "capacityReservationId",
            "capacityBlockId",
            "reservationType",
            "instanceType",
            "instancePlatform",
            "totalInstanceCount",
            "state",
        ] {
            let described = required(case, described_reservation, name);
            let answered = required(case, reservation, name);
            assert_eq!(
                described, answered,
                "case {case:?}: expected the describe to report the reservation's {name} as \
                 the purchase did ({answered:?}), got {described:?}"
            );
        }
    }

    // One block and one reservation per purchase, and not one more: every answer above came
    // out of what was stored.
    let (blocks, reservations) = stored(&engine);
    assert_eq!(
        (blocks, reservations),
        (row_count, row_count),
        "expected {row_count} Capacity Blocks and as many Capacity Reservations after \
         {row_count} purchases, got {blocks} and {reservations}"
    );
}

/// Two purchases of the SAME offering buy two Capacity Blocks: the purchase carries no
/// idempotency token, so each call reserves capacity of its own.
#[tokio::test]
async fn purchasing_one_offering_twice_buys_two_blocks() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "the same offering, bought twice";
    let offering = offered(
        &router,
        DEFAULT_REGION,
        case,
        &[
            field("InstanceType", "p5e.48xlarge"),
            field("InstanceCount", 2),
            field("CapacityDurationHours", 24),
        ],
    )
    .await;
    let params = purchase_params(&offering.id, "Linux/UNIX", &[]);

    let mut ids = Vec::new();
    for attempt in 1..=2 {
        let (status, body) = purchase(&router, DEFAULT_REGION, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: purchase {attempt} expected HTTP 200, got {status} with body {body}"
        );
        let answer = document(case, &body);
        let block = block_of(case, &body, &answer);
        ids.push(required(case, block, "capacityBlockId"));
    }
    assert_ne!(
        ids[0], ids[1],
        "case {case:?}: expected two distinct Capacity Blocks, got {ids:?}"
    );
    let (blocks, reservations) = stored(&engine);
    assert_eq!(
        (blocks, reservations),
        (2, 2),
        "case {case:?}: expected two blocks and two reservations, got {blocks} and \
         {reservations}"
    );
}

// ---------------------------------------------------------------------------
// The payment settles, and the window then decides the state
// ---------------------------------------------------------------------------

/// A purchase answers `payment-pending` on both halves; once the payment has settled each is
/// in the state its reserved window puts it in — `scheduled` ahead of the window, `active`
/// inside it, `expired` past it — and reading it never changes it.
#[tokio::test]
async fn the_payment_settles_and_the_reserved_window_then_decides_the_state() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a purchase whose payment has still to settle";
    let offering = offered(
        &router,
        DEFAULT_REGION,
        case,
        &[
            field("InstanceType", "m5.large"),
            field("InstanceCount", 4),
            field("CapacityDurationHours", 24),
        ],
    )
    .await;
    let (status, body) = purchase(
        &router,
        DEFAULT_REGION,
        &purchase_params(&offering.id, "Linux/UNIX", &[]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let (block_id, reservation_id) = {
        let answer = document(case, &body);
        let block = block_of(case, &body, &answer);
        let reservation = element_named(case, &body, &answer, "capacityReservation");
        (
            required(case, block, "capacityBlockId"),
            required(case, reservation, "capacityReservationId"),
        )
    };
    let block_ids = indexed("CapacityBlockId", &[&block_id]);
    let reservation_ids = indexed("CapacityReservationId", &[&reservation_id]);

    // Before the payment falls due, a caller acting straight after the purchase still sees
    // the transient state.
    let early_block = block_state(&router, case, &block_ids).await;
    assert_eq!(
        early_block, "payment-pending",
        "case {case:?}: expected the block to be payment-pending before the payment \
         settles, got {early_block:?}"
    );
    let early_reservation = reservation_state(&router, case, &reservation_ids).await;
    assert_eq!(
        early_reservation, "payment-pending",
        "case {case:?}: expected the reservation to be payment-pending before the payment \
         settles, got {early_reservation:?}"
    );

    tokio::time::sleep(past_the_payment()).await;

    // The window is still ahead (the search above asked for one at least an hour out), so
    // the settled purchase is scheduled — twice over, because a read does not consume the
    // move.
    for attempt in 1..=2 {
        let settled_block = block_state(&router, case, &block_ids).await;
        assert_eq!(
            settled_block, "scheduled",
            "case {case:?}: read {attempt} expected the block to be scheduled once the \
             payment settled, got {settled_block:?}"
        );
        let settled_reservation = reservation_state(&router, case, &reservation_ids).await;
        assert_eq!(
            settled_reservation, "scheduled",
            "case {case:?}: read {attempt} expected the reservation to be scheduled once \
             the payment settled, got {settled_reservation:?}"
        );
    }

    // The rest of the window's walk. The window opens at least an hour out and closes a day
    // beyond that, which no test can wait for, so the stored records' own window — the two
    // values the rule reads — is moved, and both halves are read back over the wire.
    let move_window = |starts_in: i64, ends_in: i64| {
        let at = |seconds: i64| {
            let shift = Duration::from_secs(seconds.unsigned_abs());
            let now = SystemTime::now();
            system_time_to_date_time(if seconds < 0 { now - shift } else { now + shift })
        };
        let (start, end) = (at(starts_in), at(ends_in));
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        if let Some(record) = guard.capacity_blocks.get_mut(&block_id) {
            record.block.start_date = Some(start);
            record.block.end_date = Some(end);
        }
        if let Some(record) = guard.capacity_reservations.get_mut(&reservation_id) {
            record.view.start_date = Some(start);
            record.view.end_date = Some(end);
        }
    };

    // Inside the window: the capacity has been delivered.
    move_window(-60, 3_600);
    let open_block = block_state(&router, case, &block_ids).await;
    assert_eq!(
        open_block, "active",
        "a block inside its reserved window is active, got {open_block:?}"
    );
    let open_reservation = reservation_state(&router, case, &reservation_ids).await;
    assert_eq!(
        open_reservation, "active",
        "the Capacity Reservation of an open block is active, got {open_reservation:?}"
    );

    // Past the window: "When a Capacity Block expires, all instances in the Capacity Block
    // are terminated" (`CapacityBlock$EndDate`).
    move_window(-7_200, -60);
    let closed_block = block_state(&router, case, &block_ids).await;
    assert_eq!(
        closed_block, "expired",
        "a block past its reserved window is expired, got {closed_block:?}"
    );
    let closed_reservation = reservation_state(&router, case, &reservation_ids).await;
    assert_eq!(
        closed_reservation, "expired",
        "the Capacity Reservation of an expired block is expired, got {closed_reservation:?}"
    );
}

// ---------------------------------------------------------------------------
// A value that is not an offering id
// ---------------------------------------------------------------------------

/// Every value that is not an offering id this service reported is refused as malformed,
/// with EC2's code and wording for it, and nothing is stored.
#[tokio::test]
async fn a_value_that_is_not_an_offering_id_is_refused_as_malformed() {
    let (router, engine) = super::ec2_query::emulator();
    let real = offered(
        &router,
        DEFAULT_REGION,
        "a real offering to tamper with",
        &[
            field("InstanceType", "p5.48xlarge"),
            field("InstanceCount", 1),
            field("CapacityDurationHours", 24),
        ],
    )
    .await;

    // Rows: free text, the bare prefix with and without its separator, a payload that is not
    // hex, hex this service never minted, a value carrying this service's marker with a
    // checksum that does not match its terms, a real offering id with its last digit
    // changed, a real one with that digit dropped, the id of another resource kind, a real id
    // whose case was changed, and two correctly checksummed ids whose terms are terms no
    // offering of this service carries.
    let forged = ::hex::encode("ec2-cbo-1:0000000000000000:0|3600|1||6161|6262||");
    let flipped: String = {
        let mut characters: Vec<char> = real.id.chars().collect();
        let last = characters.len() - 1;
        characters[last] = if characters[last] == 'a' { 'b' } else { 'a' };
        characters.into_iter().collect()
    };
    let shortened = real.id[..real.id.len() - 1].to_string();
    // Two ids this suite MINTS through the service's own encoder, so each carries a checksum
    // that matches its terms and is read as this service's id — but whose terms are terms no
    // offering it reports has: one naming neither a count nor a type, and one reserving no
    // units at all. The encoder is `crate::state::encode_capacity_block_offering_id`, the one
    // `DescribeCapacityBlockOfferings` mints an offering id with.
    let terms = |instance_count: Option<i32>, instance_type: Option<&str>| {
        ec2::state::encode_capacity_block_offering_id(&ec2::state::CapacityBlockOfferingTerms {
            instance_type: instance_type.map(str::to_string),
            instance_count,
            ultraserver_type: None,
            ultraserver_count: None,
            availability_zone: "us-east-1a".to_string(),
            availability_zone_id: "use1-az1".to_string(),
            start_date_secs: 2_000_000_000,
            end_date_secs: 2_000_086_400,
        })
    };
    let cases = [
        "not-a-real-offering-id".to_string(),
        "cbo".to_string(),
        "cbo-".to_string(),
        "cbo-not-hex".to_string(),
        "cbo-6161".to_string(),
        format!("cbo-{forged}"),
        flipped,
        shortened,
        "cb-0123456789abcdef0".to_string(),
        real.id.to_uppercase(),
        terms(None, None),
        terms(Some(0), Some("p5.48xlarge")),
    ];
    for offering_id in cases {
        let (status, body) = purchase(
            &router,
            DEFAULT_REGION,
            &purchase_params(&offering_id, "Linux/UNIX", &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {offering_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidCapacityBlockOfferingIdMalformed",
            "case {offering_id:?}: expected InvalidCapacityBlockOfferingIdMalformed, \
             got {code:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Capacity Block Offering Id '{offering_id}' is malformed."),
            "case {offering_id:?}: unexpected wording {message:?}"
        );
    }

    // The real id still buys, so the rows above were refused for what they carried rather
    // than because nothing can be bought at all.
    let (status, body) = purchase(
        &router,
        DEFAULT_REGION,
        &purchase_params(&real.id, "Linux/UNIX", &[]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the offering this suite tampered with must itself be purchasable, got {status} \
         with body {body}"
    );
    let (blocks, reservations) = stored(&engine);
    assert_eq!(
        (blocks, reservations),
        (1, 1),
        "expected only the one real purchase to have been stored, got {blocks} blocks and \
         {reservations} reservations"
    );
}

// ---------------------------------------------------------------------------
// DryRun and the tag specification
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and buys nothing;
/// a request that would have been refused is refused with THAT reason instead, and
/// `DryRun=false` is an ordinary purchase.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_buying() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "DryRun";
    let offering = offered(
        &router,
        DEFAULT_REGION,
        case,
        &[
            field("InstanceType", "c6g.medium"),
            field("CapacityDurationHours", 4_368),
        ],
    )
    .await;

    let mut params = purchase_params(&offering.id, "Linux/UNIX", &[]);
    params.push(field("DryRun", "true"));
    let (status, body) = purchase(&router, DEFAULT_REGION, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored(&engine),
        (0, 0),
        "case {case:?}: a dry run must buy nothing"
    );

    // A dry run of a request that would have been refused anyway is refused with that
    // reason, not with the permitted outcome.
    let mut refused = purchase_params("not-an-offering-id", "Linux/UNIX", &[]);
    refused.push(field("DryRun", "true"));
    let (status, body) = purchase(&router, DEFAULT_REGION, &refused).await;
    assert_eq!(
        status, 400,
        "a dry run of a malformed id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityBlockOfferingIdMalformed",
        "a dry run of a malformed id: expected the refusal's own reason, got {body}"
    );

    // `DryRun=false` is an ordinary purchase.
    let mut performed = purchase_params(&offering.id, "Linux/UNIX", &[]);
    performed.push(field("DryRun", "false"));
    let (status, body) = purchase(&router, DEFAULT_REGION, &performed).await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored(&engine),
        (1, 1),
        "DryRun=false: expected the purchase to have been stored"
    );
}

/// "The tags to apply to the Capacity Block during launch": a tag specification naming any
/// other resource type is refused rather than accepted and discarded, and nothing is stored.
#[tokio::test]
async fn a_tag_specification_may_only_name_the_capacity_block() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a tag specification for another resource type";
    let offering = offered(
        &router,
        DEFAULT_REGION,
        case,
        &[
            field("InstanceType", "p5.48xlarge"),
            field("CapacityDurationHours", 168),
        ],
    )
    .await;

    // Rows: the Capacity Reservation the purchase also creates, an unrelated resource type,
    // and a specification that names no resource type at all.
    for resource_type in ["capacity-reservation", "instance", ""] {
        let mut params = vec![
            field("CapacityBlockOfferingId", &offering.id),
            field("InstancePlatform", "Linux/UNIX"),
            field("TagSpecification.1.Tag.1.Key", "Name"),
            field("TagSpecification.1.Tag.1.Value", "mislabelled"),
        ];
        if !resource_type.is_empty() {
            params.push(field("TagSpecification.1.ResourceType", resource_type));
        }
        let (status, body) = purchase(&router, DEFAULT_REGION, &params).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            stored(&engine),
            (0, 0),
            "case {resource_type:?}: a refused purchase must store nothing"
        );
    }

    // The resource type the purchase does tag is accepted, and the tag reaches the block.
    let (status, body) = purchase(
        &router,
        DEFAULT_REGION,
        &purchase_params(&offering.id, "Linux/UNIX", &[("Name", "labelled")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "a capacity-block tag specification: expected HTTP 200, got {status} with body {body}"
    );
    let answer = document(case, &body);
    let reported = tags_of(block_of(case, &body, &answer));
    assert_eq!(
        reported,
        vec![("Name".to_string(), "labelled".to_string())],
        "expected the tag on the block, got {reported:?}"
    );
}
