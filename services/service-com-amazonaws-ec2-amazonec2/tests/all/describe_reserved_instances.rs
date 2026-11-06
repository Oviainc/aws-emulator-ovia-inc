// SPDX-License-Identifier: CC0-1.0

//! What `DescribeReservedInstances` reports out of the Reserved Instances store — end to end
//! over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeReservedInstances`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * The report is every Reserved Instance this account holds, each as the purchase the store
//!   holds it as, narrowed by the ids, the offering class, the offering type and the
//!   documented filters the request names — and nothing else. The rows of the table below
//!   differ along what that rule generalises over: which narrowing member is used, how many
//!   values it carries, which attribute a filter selects on, whether a value is exact or a
//!   wildcard, and how two narrowings combine.
//! * Every member of a stored purchase reads back, including the ones the store derives (the
//!   currency, the tenancy, the scope, the zone id and the instant the term ends).
//! * A value that is not a Reserved Instances id, an id this account holds no purchase under,
//!   an undocumented filter and a request past this crate's request-controlled ceilings are
//!   each refused with the code EC2 answers for the member at fault, and nothing is reported.
//! * A dry run reports the permission answer and reads nothing.
//! * The report is a READ: two calls with nothing in between report the same purchases and
//!   leave the store exactly as it was.
//! * What a WRITER stored is what the report answers with: the purchases
//!   `AcceptReservedInstancesExchangeQuote` mints, and the `retired` it leaves the purchase it
//!   exchanged in, are both visible through this operation.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_PARAMETER_VALUE,
    INVALID_RESERVED_INSTANCES_ID_NOT_FOUND, MAX_ECHOED_RESOURCE_ID_CHARS, MAX_FILTER_VALUE_BYTES,
    MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS, RESERVED_INSTANCE_CURRENCY_CODE,
    RESERVED_INSTANCE_SCOPE_REGION, RESERVED_INSTANCE_SCOPE_ZONE, RESERVED_INSTANCE_STATE_ACTIVE,
    RESERVED_INSTANCE_STATE_RETIRED, ReservedInstancesOfferingTerms, ReservedInstancesSpec,
    availability_zones_in_region, encode_reserved_instances_offering_id,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message, filters, indexed};
use super::wire::{Param, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The form-field prefix ec2Query flattens `ReservedInstancesIds` into: the member's
/// `@xmlName` (`smithy_query(query="operation",operation="DescribeReservedInstances")` gives
/// `ReservedInstancesId`).
const IDS: &str = "ReservedInstancesId";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// "The duration of the Reserved Instance (one year or three years), in seconds (`31536000` |
/// `94608000`)" — the two values `DescribeReservedInstancesRequest$Filters` documents for its
/// `duration` entry:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeReservedInstancesRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
const ONE_YEAR_SECONDS: i64 = 31_536_000;

/// The other of those two terms.
const THREE_YEARS_SECONDS: i64 = 94_608_000;

/// One Reserved Instance to seed: its instance type, its Availability Zone (`None` for a
/// purchase of Regional scope), its term in seconds, its offering class and type, its product
/// description, the state the store holds it in, how many reservations it holds, the purchase
/// price of each and the hourly usage price.
#[derive(Debug, Clone, Copy)]
struct PurchaseRow {
    instance_type: &'static str,
    availability_zone: Option<&'static str>,
    duration_seconds: i64,
    offering_class: &'static str,
    offering_type: &'static str,
    product_description: &'static str,
    state: &'static str,
    instance_count: i32,
    fixed_price: f32,
    usage_price: f32,
}

/// The three purchases every table below is read against.
///
/// They differ in every attribute a documented filter selects on — instance type, zone and so
/// scope, term, offering class and type, product description, state and both prices — so no row
/// of a table can pass by matching more than it asked for. The enum members are the modelled
/// catalogs' own: `standard`/`convertible` of `OFFERING_CLASS_TYPE_VALUES`, `All Upfront` /
/// `No Upfront` / `Partial Upfront` of `OFFERING_TYPE_VALUES_VALUES`, `Linux/UNIX` / `Windows`
/// / `SUSE Linux` of `R_I_PRODUCT_DESCRIPTION_VALUES`, and an `InstanceType` of
/// `INSTANCE_TYPE_VALUES`.
const SEEDED: [PurchaseRow; 3] = [
    PurchaseRow {
        instance_type: "m5.large",
        availability_zone: Some("us-east-1a"),
        duration_seconds: ONE_YEAR_SECONDS,
        offering_class: "standard",
        offering_type: "All Upfront",
        product_description: "Linux/UNIX",
        state: RESERVED_INSTANCE_STATE_ACTIVE,
        instance_count: 2,
        fixed_price: 500.0,
        usage_price: 0.0,
    },
    PurchaseRow {
        instance_type: "t3.micro",
        availability_zone: Some("us-east-1b"),
        duration_seconds: THREE_YEARS_SECONDS,
        offering_class: "convertible",
        offering_type: "No Upfront",
        product_description: "Windows",
        state: RESERVED_INSTANCE_STATE_ACTIVE,
        instance_count: 1,
        fixed_price: 9800.0,
        usage_price: 0.84,
    },
    PurchaseRow {
        instance_type: "m5.xlarge",
        availability_zone: None,
        duration_seconds: ONE_YEAR_SECONDS,
        offering_class: "standard",
        offering_type: "Partial Upfront",
        product_description: "SUSE Linux",
        state: RESERVED_INSTANCE_STATE_RETIRED,
        instance_count: 4,
        fixed_price: 250.5,
        usage_price: 0.0,
    },
];

/// The tag the first seeded purchase carries, for the `tag:<key>` and `tag-key` filters.
const TAG_KEY: &str = "Owner";

/// That tag's value.
const TAG_VALUE: &str = "TeamA";

/// One purchase to store, as the spec the one store seam takes.
fn spec(row: PurchaseRow) -> ReservedInstancesSpec {
    ReservedInstancesSpec {
        instance_type: row.instance_type.to_string(),
        availability_zone: row.availability_zone.map(str::to_string),
        region: REGION.to_string(),
        instance_count: row.instance_count,
        duration_seconds: row.duration_seconds,
        offering_class: row.offering_class.to_string(),
        offering_type: row.offering_type.to_string(),
        product_description: row.product_description.to_string(),
        state: row.state.to_string(),
        start: SystemTime::now() - Duration::from_secs(60),
        fixed_price: row.fixed_price,
        usage_price: row.usage_price,
    }
}

/// Seed one purchase and answer the id the store minted for it.
fn seed(engine: &engine::InMemoryEngine, row: PurchaseRow) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(spec(row))
        .expect("the store has room for the few purchases this suite seeds")
}

/// An emulator holding [`SEEDED`], with the first purchase tagged, and the ids the store
/// minted for them in that order.
fn seeded_emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = super::ec2_query::emulator();
    let ids: Vec<String> = SEEDED.iter().map(|row| seed(&engine, *row)).collect();
    // The tags a purchase carries have no member on [`ReservedInstancesSpec`] — nothing in
    // this emulator tags a Reserved Instance yet — so the one the tag filters select on is
    // written onto the stored shape itself, through the same store the handler reads.
    engine
        .state::<Ec2State>()
        .lock()
        .reserved_instances
        .get_mut(&ids[0])
        .expect("the purchase just seeded")
        .tags = Some(vec![Tag {
        key: Some(TAG_KEY.to_string()),
        value: Some(TAG_VALUE.to_string()),
    }]);
    (router, engine, ids)
}

/// The zone id of `zone_name` in this Region, as the zones of a Region are facts of it.
fn zone_id(zone_name: &str) -> String {
    availability_zones_in_region(REGION)
        .into_iter()
        .find(|zone| zone.zone_name == zone_name)
        .map(|zone| zone.zone_id)
        .unwrap_or_else(|| panic!("{REGION} must have the Availability Zone {zone_name}"))
}

/// One `DescribeReservedInstances` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeReservedInstances", params).await
}

/// Every Reserved Instance a response reports, as its members by element name, in the order
/// the response carries them.
fn reported(case: &str, body: &str) -> Vec<BTreeMap<String, String>> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "reservedInstancesSet")
        .into_iter()
        .map(|item| {
            item.children()
                .filter(|node| node.is_element())
                .map(|node| {
                    (
                        node.tag_name().name().to_string(),
                        node.text().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// The `reservedInstancesId` of every Reserved Instance a response reports.
fn reported_ids(case: &str, status: u16, body: &str) -> Vec<String> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported(case, body)
        .into_iter()
        .map(|members| {
            members
                .get("reservedInstancesId")
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// The purchases the store holds, as the id each is keyed by and the whole record under it —
/// what a "nothing moved" assertion compares.
fn purchases(engine: &engine::InMemoryEngine) -> BTreeMap<String, String> {
    engine
        .state::<Ec2State>()
        .lock()
        .reserved_instances
        .iter()
        .map(|(id, purchase)| (id.clone(), format!("{purchase:?}")))
        .collect()
}

/// A well-formed Reserved Instances id — the hyphenated UUID form — that names nothing, built
/// from `n` so each row of a table gets its own.
fn unknown_id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

// ---------------------------------------------------------------------------
// What the report covers, and what narrows it
// ---------------------------------------------------------------------------

/// The rule: the report is every Reserved Instance the account holds, narrowed by the ids, the
/// offering class, the offering type and the documented filters the request names.
///
/// One row per way of narrowing, spanning what the rule generalises over: no narrowing at all,
/// one id, several ids (one of them named twice), each of the two enum members, each documented
/// filter — including both tag forms and both prices, each written the way the report carries
/// it and the way AWS's own documentation writes it — a wildcard value, a filter carrying two
/// values, two filters that must both hold, and narrowings that select nothing.
#[tokio::test]
async fn the_report_is_the_store_narrowed_by_what_the_request_names() {
    let (router, engine, ids) = seeded_emulator();
    let one_a = zone_id("us-east-1a");
    let retired = ids[2].clone();
    let before = purchases(&engine);

    // (case, request fields, the seeded purchases expected back, by position in `SEEDED`)
    let cases: Vec<(&str, Vec<Param>, Vec<usize>)> = vec![
        ("no narrowing at all", Vec::new(), vec![0, 1, 2]),
        (
            "one id",
            indexed(IDS, &[ids[0].as_str()]),
            vec![0],
        ),
        (
            "two ids, one of them named twice",
            indexed(IDS, &[ids[2].as_str(), ids[0].as_str(), ids[2].as_str()]),
            vec![0, 2],
        ),
        (
            "the convertible offering class",
            vec![param("OfferingClass", "convertible")],
            vec![1],
        ),
        (
            "the standard offering class",
            vec![param("OfferingClass", "standard")],
            vec![0, 2],
        ),
        (
            "one offering type",
            vec![param("OfferingType", "Partial Upfront")],
            vec![2],
        ),
        (
            "an offering class and an offering type that hold together",
            vec![
                param("OfferingClass", "standard"),
                param("OfferingType", "All Upfront"),
            ],
            vec![0],
        ),
        (
            "an offering class and an offering type no purchase holds together",
            vec![
                param("OfferingClass", "convertible"),
                param("OfferingType", "All Upfront"),
            ],
            Vec::new(),
        ),
        (
            "the instance-type filter",
            filters(&[("instance-type", &["t3.micro"])]),
            vec![1],
        ),
        (
            "the instance-type filter with a wildcard value",
            filters(&[("instance-type", &["m5.*"])]),
            vec![0, 2],
        ),
        (
            "the instance-type filter carrying two values",
            filters(&[("instance-type", &["m5.large", "t3.micro"])]),
            vec![0, 1],
        ),
        (
            "the availability-zone filter",
            filters(&[("availability-zone", &["us-east-1b"])]),
            vec![1],
        ),
        (
            "the availability-zone-id filter",
            filters(&[("availability-zone-id", &[one_a.as_str()])]),
            vec![0],
        ),
        (
            "the scope filter, for the zonal purchases",
            filters(&[("scope", &[RESERVED_INSTANCE_SCOPE_ZONE])]),
            vec![0, 1],
        ),
        (
            "the scope filter, for the Regional purchase",
            filters(&[("scope", &[RESERVED_INSTANCE_SCOPE_REGION])]),
            vec![2],
        ),
        (
            "the state filter",
            filters(&[("state", &[RESERVED_INSTANCE_STATE_RETIRED])]),
            vec![2],
        ),
        (
            "the product-description filter",
            filters(&[("product-description", &["Windows"])]),
            vec![1],
        ),
        // `offering-class` and `offering-type` are the two filter names this operation's
        // recorded contract serves that the member's own documentation does not list — a
        // recorded call narrowing this report by the offering class of a purchase is SERVED —
        // so each selects on the member of the same name the report already carries.
        (
            "the offering-class filter",
            filters(&[("offering-class", &["convertible"])]),
            vec![1],
        ),
        (
            "the offering-class filter, for the standard purchases",
            filters(&[("offering-class", &["standard"])]),
            vec![0, 2],
        ),
        (
            "the offering-type filter",
            filters(&[("offering-type", &["Partial Upfront"])]),
            vec![2],
        ),
        (
            "the offering-class filter beside the offering-type filter",
            filters(&[
                ("offering-class", &["standard"]),
                ("offering-type", &["All Upfront"]),
            ]),
            vec![0],
        ),
        (
            "the duration filter, for a three-year term",
            filters(&[("duration", &["94608000"])]),
            vec![1],
        ),
        (
            "the duration filter, for a one-year term",
            filters(&[("duration", &["31536000"])]),
            vec![0, 2],
        ),
        (
            "the fixed-price filter, as the report carries the price",
            filters(&[("fixed-price", &["9800"])]),
            vec![1],
        ),
        (
            "the fixed-price filter, as the documentation writes the price",
            filters(&[("fixed-price", &["9800.0"])]),
            vec![1],
        ),
        (
            "the fixed-price filter, for a price with cents",
            filters(&[("fixed-price", &["250.5"])]),
            vec![2],
        ),
        (
            "the usage-price filter",
            filters(&[("usage-price", &["0.84"])]),
            vec![1],
        ),
        (
            "the reserved-instances-id filter",
            filters(&[("reserved-instances-id", &[retired.as_str()])]),
            vec![2],
        ),
        (
            "the tag:<key> filter",
            filters(&[(&format!("tag:{TAG_KEY}"), &[TAG_VALUE])]),
            vec![0],
        ),
        (
            "the tag-key filter",
            filters(&[("tag-key", &[TAG_KEY])]),
            vec![0],
        ),
        (
            "two filters joined with AND",
            filters(&[
                ("instance-type", &["m5.*"]),
                ("state", &[RESERVED_INSTANCE_STATE_ACTIVE]),
            ]),
            vec![0],
        ),
        (
            "two filters no purchase satisfies together",
            filters(&[
                ("instance-type", &["t3.micro"]),
                ("state", &[RESERVED_INSTANCE_STATE_RETIRED]),
            ]),
            Vec::new(),
        ),
        (
            "an id and a filter that disagree",
            {
                let mut params = indexed(IDS, &[ids[0].as_str()]);
                params.extend(filters(&[(
                    "state",
                    &[RESERVED_INSTANCE_STATE_RETIRED],
                )]));
                params
            },
            Vec::new(),
        ),
        (
            "a filter value no purchase holds",
            filters(&[("instance-type", &["c6g.metal"])]),
            Vec::new(),
        ),
    ];

    for (case, params, expected_positions) in cases {
        let expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        let (status, body) = describe(&router, &params).await;
        let actual = reported_ids(case, status, &body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the purchases {expected:?}, got {actual:?}"
        );
    }

    assert_eq!(
        purchases(&engine),
        before,
        "a report must leave every purchase as it was"
    );
}

/// The rule's other half: each purchase is reported as the store holds it, including the
/// members the store derives rather than takes from a writer — the currency, the tenancy, the
/// scope, the Availability Zone's id and the instant the term ends.
#[tokio::test]
async fn every_member_of_a_stored_purchase_reads_back() {
    let (router, _engine, ids) = seeded_emulator();
    let (status, body) = describe(&router, &[]).await;
    let case = "every member of a stored purchase";
    let items = reported(case, &body);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        items.len(),
        SEEDED.len(),
        "case {case:?}: expected {} purchases, got {body}",
        SEEDED.len()
    );

    for (position, row) in SEEDED.iter().enumerate() {
        let case = format!("the purchase {position} ({})", row.instance_type);
        let members = &items[position];
        let read = |name: &str| -> String { members.get(name).cloned().unwrap_or_default() };
        let expected: Vec<(&str, String)> = vec![
            ("reservedInstancesId", ids[position].clone()),
            ("instanceType", row.instance_type.to_string()),
            ("instanceCount", row.instance_count.to_string()),
            ("duration", row.duration_seconds.to_string()),
            ("offeringClass", row.offering_class.to_string()),
            ("offeringType", row.offering_type.to_string()),
            ("productDescription", row.product_description.to_string()),
            ("state", row.state.to_string()),
            ("fixedPrice", row.fixed_price.to_string()),
            ("usagePrice", row.usage_price.to_string()),
            (
                "currencyCode",
                RESERVED_INSTANCE_CURRENCY_CODE.to_string(),
            ),
            (
                "scope",
                if row.availability_zone.is_some() {
                    RESERVED_INSTANCE_SCOPE_ZONE.to_string()
                } else {
                    RESERVED_INSTANCE_SCOPE_REGION.to_string()
                },
            ),
            (
                "availabilityZone",
                row.availability_zone.unwrap_or_default().to_string(),
            ),
            (
                "availabilityZoneId",
                row.availability_zone.map(zone_id).unwrap_or_default(),
            ),
        ];
        for (name, want) in expected {
            assert_eq!(
                read(name),
                want,
                "case {case:?}: expected {name} {want:?}, got {actual:?} in {body}",
                actual = read(name)
            );
        }
        // "The duration of the Reserved Instance, in seconds" is the span between the two
        // instants the purchase is reported with, so the term the report carries is the term
        // the store holds however the two timestamps are rendered.
        let start = as_timestamp(&read("start"))
            .unwrap_or_else(|| panic!("case {case:?}: start must be a timestamp, got {body}"));
        let end = as_timestamp(&read("end"))
            .unwrap_or_else(|| panic!("case {case:?}: end must be a timestamp, got {body}"));
        let term = end - start;
        let expected_term = row.duration_seconds as f64;
        assert!(
            (term - expected_term).abs() < 1.0,
            "case {case:?}: expected a term of {expected_term} seconds between start and end, \
             got {term}"
        );
    }
}

/// A timestamp filter compares against the instant the report carries, whatever that instant
/// is: the value is read back off the response and handed straight to the filter, so the row
/// pins the rule rather than one run's clock.
#[tokio::test]
async fn a_timestamp_filter_selects_the_purchase_the_report_carries_that_instant_for() {
    let (router, _engine, ids) = seeded_emulator();
    let (status, body) = describe(&router, &[]).await;
    let case = "the instants a report carries";
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let items = reported(case, &body);

    for filter_name in ["start", "end"] {
        // The third purchase shares its term with the first, so the instant asserted on is the
        // SECOND one's: its three-year term gives it an `end` of its own.
        let position = 1;
        let instant = items[position]
            .get(filter_name)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the report must carry {filter_name}: {body}"));
        let case = format!("the {filter_name} filter for {instant}");
        let (status, body) = describe(&router, &filters(&[(filter_name, &[instant.as_str()])])).await;
        let actual = reported_ids(&case, status, &body);
        assert_eq!(
            actual,
            vec![ids[position].clone()],
            "case {case:?}: expected only the purchase that instant belongs to, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the report refuses
// ---------------------------------------------------------------------------

/// The rule: a request this operation cannot answer is refused with the code EC2 answers for
/// the member at fault, rather than reported as an empty result — and nothing is reported.
///
/// The rows span the classes of value it refuses: a value that is not a Reserved Instances id
/// at all (two forms of one), a well-formed id this account holds nothing under, a request
/// mixing a known id with an unknown one, an undocumented filter name and a `tag:` prefix with
/// no key, and each request-controlled ceiling this crate holds a describe to — one id, one
/// filter, one filter value and one value's size past it.
#[tokio::test]
async fn a_request_the_report_cannot_answer_is_refused() {
    let (router, _engine, ids) = seeded_emulator();
    let unknown = unknown_id(1);
    let long_value = "r".repeat(4 * MAX_ECHOED_RESOURCE_ID_CHARS);
    let oversize_value = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let too_many_ids: Vec<String> = (0..=MAX_RESOURCE_IDS).map(unknown_id).collect();
    let too_many_values: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("m5.large.{index}"))
        .collect();

    // (case, request fields, the code expected, the message expected when it is fixed text)
    let cases: Vec<(&str, Vec<Param>, &str, Option<String>)> = vec![
        (
            "a value that is not a Reserved Instances id",
            indexed(IDS, &["not-a-reserved-instances-id"]),
            INVALID_PARAMETER_VALUE,
            Some("Invalid value 'not-a-reserved-instances-id' for ReservedInstancesId.".to_string()),
        ),
        (
            "an id of another EC2 form",
            indexed(IDS, &["ri-0123456789abcdef0"]),
            INVALID_PARAMETER_VALUE,
            Some("Invalid value 'ri-0123456789abcdef0' for ReservedInstancesId.".to_string()),
        ),
        (
            "a value far longer than any id EC2 issues, quoted only as far as the echo is \
             bounded",
            indexed(IDS, &[long_value.as_str()]),
            INVALID_PARAMETER_VALUE,
            Some(format!(
                "Invalid value '{echoed}' for ReservedInstancesId.",
                echoed = "r".repeat(MAX_ECHOED_RESOURCE_ID_CHARS)
            )),
        ),
        (
            "a well-formed id this account holds nothing under",
            indexed(IDS, &[unknown.as_str()]),
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            Some(format!(
                "The reserved-instances ID '{unknown}' does not exist"
            )),
        ),
        (
            "one id this account holds and one it does not",
            indexed(IDS, &[ids[0].as_str(), unknown.as_str()]),
            INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
            Some(format!(
                "The reserved-instances ID '{unknown}' does not exist"
            )),
        ),
        (
            "a filter this operation does not document",
            filters(&[("instance-id", &["i-0123456789abcdef0"])]),
            INVALID_PARAMETER_VALUE,
            Some("The filter 'instance-id' is invalid".to_string()),
        ),
        (
            "the tag prefix with no key after it",
            filters(&[("tag:", &[TAG_VALUE])]),
            INVALID_PARAMETER_VALUE,
            Some("The filter 'tag:' is invalid".to_string()),
        ),
        (
            "one id past the id ceiling",
            indexed(
                IDS,
                &too_many_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>(),
            ),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one filter past the filter ceiling",
            (1..=MAX_FILTERS + 1)
                .flat_map(|index| {
                    [
                        param(&format!("Filter.{index}.Name"), "instance-type"),
                        param(&format!("Filter.{index}.Value.1"), "m5.large"),
                    ]
                })
                .collect(),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one filter value past the value ceiling",
            filters(&[(
                "instance-type",
                &too_many_values
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>(),
            )]),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one byte past the filter value size ceiling",
            filters(&[("instance-type", &[oversize_value.as_str()])]),
            INVALID_PARAMETER_VALUE,
            None,
        ),
    ];

    for (case, params, expected_code, expected_message) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {body}"
            );
        }
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a refused request must report no purchase, got {body}"
        );
    }
}

/// The rule: "checks whether you have the required permissions for the action, without
/// actually making the request … If you have the required permissions, the error response is
/// DryRunOperation" (`$DryRun`) — so a dry run reports HTTP 412 `DryRunOperation` and reads
/// nothing, for a request that would have been answered and for one naming a purchase this
/// account does not hold alike: the probe is about the CALLER's permission, not about the
/// resource. A request this operation cannot read at all is still refused with its own reason.
#[tokio::test]
async fn a_dry_run_reports_the_permission_answer_and_reports_no_purchase() {
    let (router, engine, ids) = seeded_emulator();
    let before = purchases(&engine);
    let unknown = unknown_id(2);
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        ("a dry run of a report that would be answered", Vec::new(), DRY_RUN_STATUS, DRY_RUN_OPERATION),
        (
            "a dry run naming a purchase this account holds",
            indexed(IDS, &[ids[0].as_str()]),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run naming a purchase this account does not hold",
            indexed(IDS, &[unknown.as_str()]),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a request naming a value that is not an id",
            indexed(IDS, &["not-a-reserved-instances-id"]),
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, params, expected_status, expected_code) in cases {
        let mut with_flag = params;
        with_flag.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &with_flag).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a dry run must report no purchase, got {body}"
        );
    }

    assert_eq!(
        purchases(&engine),
        before,
        "a dry run must leave every purchase as it was"
    );
}

/// The rule: the report is a READ. Two identical calls with nothing in between report the same
/// purchases, and the store holds exactly what it held before either of them.
#[tokio::test]
async fn two_reports_with_nothing_in_between_answer_the_same() {
    let (router, engine, _ids) = seeded_emulator();
    let before = purchases(&engine);
    let case = "the same report twice";

    let (first_status, first) = describe(&router, &[]).await;
    let (second_status, second) = describe(&router, &[]).await;
    let first_ids = reported_ids(case, first_status, &first);
    let second_ids = reported_ids(case, second_status, &second);

    assert_eq!(
        first_ids, second_ids,
        "case {case:?}: expected the same purchases, got {first_ids:?} then {second_ids:?}"
    );
    assert_eq!(
        reported(case, &first),
        reported(case, &second),
        "case {case:?}: expected the same members, got {first} then {second}"
    );
    assert_eq!(
        purchases(&engine),
        before,
        "case {case:?}: a report must leave every purchase as it was"
    );
}

// ---------------------------------------------------------------------------
// What a writer stored is what the report answers with
// ---------------------------------------------------------------------------

/// The rule: this report answers out of the store every writer of a purchase writes to, so the
/// purchases `AcceptReservedInstancesExchangeQuote` minted and the `retired` it left the
/// purchase it exchanged in are both visible through it.
///
/// That is what makes the report state-backed rather than a value one call computed: nothing in
/// the describe path knows anything about an exchange.
#[tokio::test]
async fn the_report_answers_with_what_a_writer_stored() {
    let (router, engine) = super::ec2_query::emulator();
    // A Convertible purchase in effect is the one an exchange can act on.
    let exchanged = seed(
        &engine,
        PurchaseRow {
            instance_type: "m5.large",
            availability_zone: Some("us-east-1a"),
            duration_seconds: ONE_YEAR_SECONDS,
            offering_class: "convertible",
            offering_type: "All Upfront",
            product_description: "Linux/UNIX",
            state: RESERVED_INSTANCE_STATE_ACTIVE,
            instance_count: 2,
            fixed_price: 500.0,
            usage_price: 0.0,
        },
    );
    // The offering it is exchanged into, named by the id its own terms derive.
    let target = ReservedInstancesOfferingTerms {
        instance_type: "m5.xlarge".to_string(),
        availability_zone: Some("us-east-1b".to_string()),
        product_description: "Linux/UNIX".to_string(),
        offering_class: "convertible".to_string(),
        offering_type: "All Upfront".to_string(),
        duration_seconds: ONE_YEAR_SECONDS,
        fixed_price: 700.0,
        usage_price: 0.0,
    };
    let case = "an exchange a later report reads back";

    let (status, body) = call(
        &router,
        REGION,
        "AcceptReservedInstancesExchangeQuote",
        &[
            param("ReservedInstanceId.1", &exchanged),
            param(
                "TargetConfiguration.1.OfferingId",
                &encode_reserved_instances_offering_id(&target),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the exchange must be answered, got {status} with body {body}"
    );

    let (status, body) = describe(&router, &[]).await;
    let items = reported(case, &body);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        items.len(),
        2,
        "case {case:?}: expected the exchanged purchase and the one it was exchanged into, \
         got {body}"
    );

    let read = |item: &BTreeMap<String, String>, name: &str| -> String {
        item.get(name).cloned().unwrap_or_default()
    };
    let exchanged_item = items
        .iter()
        .find(|item| read(item, "reservedInstancesId") == exchanged)
        .unwrap_or_else(|| panic!("case {case:?}: the exchanged purchase must be reported: {body}"));
    assert_eq!(
        read(exchanged_item, "state"),
        RESERVED_INSTANCE_STATE_RETIRED,
        "case {case:?}: expected the exchanged purchase to be retired, got {body}"
    );

    let minted = items
        .iter()
        .find(|item| read(item, "reservedInstancesId") != exchanged)
        .unwrap_or_else(|| panic!("case {case:?}: the minted purchase must be reported: {body}"));
    for (name, want) in [
        ("instanceType", target.instance_type.as_str()),
        (
            "availabilityZone",
            target.availability_zone.as_deref().unwrap_or_default(),
        ),
        ("state", RESERVED_INSTANCE_STATE_ACTIVE),
        ("offeringClass", target.offering_class.as_str()),
    ] {
        assert_eq!(
            read(minted, name),
            want,
            "case {case:?}: expected the minted purchase's {name} to be {want:?}, got {actual:?} \
             in {body}",
            actual = read(minted, name)
        );
    }

    // And the report narrows over the minted purchase exactly as it does over a seeded one.
    let minted_id = read(minted, "reservedInstancesId");
    let (status, body) = describe(&router, &indexed(IDS, &[minted_id.as_str()])).await;
    let actual = reported_ids(case, status, &body);
    assert_eq!(
        actual,
        vec![minted_id.clone()],
        "case {case:?}: expected the minted purchase to be nameable by its id, got {actual:?}"
    );
}
