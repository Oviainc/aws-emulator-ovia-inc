// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetCapacityManagerMetricDimensions` — "the available dimension
//! values for capacity metrics within a specified time range" — driven over the wire the
//! service speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. CAPACITY MANAGER MUST BE ON: an account that has not enabled it has no capacity
//!    analytics to discover dimensions in, so every request is refused HTTP 400
//!    `CapacityManager.Disabled`, and an account that disables it again is refused again;
//! 2. THE COMBINATIONS ARE THE ACCOUNT'S OWN CAPACITY: the report is the DISTINCT
//!    projections, onto the dimensions the request groups by, of the capacity resources the
//!    account holds — so a combination carries exactly the grouped dimensions, two resources
//!    that agree on them are reported once, and a resource carrying no value for a grouped
//!    dimension yields no combination at all;
//! 3. ONLY CAPACITY WITH DATA IN THE WINDOW: "only dimensions with data in this time range
//!    will be returned", which is an OVERLAP of the resource's own window with the
//!    request's;
//! 4. EACH METRIC IS MEASURED OVER THE CAPACITY IT NAMES: a `reservation-…` metric is
//!    measured over the account's Capacity Reservations, a `spot-…` metric over its Spot
//!    capacity and an `unreserved-…` metric over the capacity no reservation covers;
//! 5. `FilterBy` NARROWS: an `equals` condition keeps the resources whose dimension is its
//!    first value, an `in` condition those matching any of them, and several conditions are
//!    joined with an AND;
//! 6. PAGES: the combinations are cut into `MaxResults`-sized pages in one stable order,
//!    followed by the token the next page is asked for with — and a token minted for one
//!    query is not resumed inside another's;
//! 7. a request outside a documented rule is REFUSED with EC2's own code for it;
//! 8. REPORTING ONLY: asking changes nothing a later caller sees;
//! 9. THE FILTERING IS BOUNDED: every comparison a condition makes is charged to the one
//!    budget a request's whole filtering shares, so the product of stored capacity rows,
//!    conditions and values per condition cannot grow the work without limit.
//!
//! Each table row is a distinct input along what its rule generalises over — which
//! dimensions are grouped by and how many, which metric family, which window, which
//! comparison and values, which page size — so no row is the specification.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ec2::ops::types::CapacityManagerDimension;
use ec2::state::{
    CAPACITY_MANAGER_COMPARISON_EQUALS, CAPACITY_MANAGER_COMPARISON_IN,
    ComAmazonawsEc2Amazonec2State as Ec2State, DIMENSION_INSTANCE_TYPE, MAX_FILTER_VALUES,
    MAX_PAGE_SIZE, MatchBudget, TRANSITION_DELAY, capacity_manager_condition_holds, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, indexed};

/// The Region every call here is signed for. A Capacity Reservation reserves capacity in one
/// Availability Zone of it, so the Region decides which zones a create may name and which
/// Region a reported combination carries.
const REGION: &str = "us-east-1";

/// One metric of each family of the modelled `Metric` catalog
/// (`smithy_query(query="shape",shape="Metric")`), so rule 4 is asserted over the whole
/// catalog rather than over one name: the reserved capacity itself, the usage that capacity
/// covered, Spot capacity, and capacity no reservation covers.
const RESERVATION_METRIC: &str = "reservation-total-capacity-hrs-vcpu";
const RESERVED_USAGE_METRIC: &str = "reserved-total-usage-hrs-inst";
const SPOT_METRIC: &str = "spot-total-usage-hrs-inst";
const UNRESERVED_METRIC: &str = "unreserved-total-usage-hrs-inst";

/// One form field.
fn p(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the
/// service's own state: a launch must name an image the account can resolve, so no id is
/// invented here.
fn catalog_image(windows: bool) -> String {
    let state = Ec2State::default();
    state
        .images
        .iter()
        .find(|(_, image)| image.platform.is_some() == windows)
        .map(|(image_id, _)| image_id.clone())
        .expect("a fresh account must publish a Linux and a Windows AMI")
}

/// `now` shifted by `seconds`, as the ISO 8601 UTC timestamp the wire carries a timestamp as.
fn timestamp_in(seconds: i64) -> String {
    let now = SystemTime::now();
    let shift = Duration::from_secs(seconds.unsigned_abs());
    iso8601_utc(if seconds < 0 { now - shift } else { now + shift })
}

/// An hour in seconds, the unit the windows below are written in.
const HOUR: i64 = 60 * 60;

/// A day in seconds.
const DAY: i64 = 24 * HOUR;

/// The two ends of one window, as the wire carries them, from offsets in seconds from now.
///
/// Rendered ONCE per query rather than per call: a page token names a position in the order
/// of ONE query, and the window is part of that query, so a walk has to keep asking the same
/// question.
fn window(offsets: (i64, i64)) -> (String, String) {
    (timestamp_in(offsets.0), timestamp_in(offsets.1))
}

/// The form fields of one dimension query: what it groups by, which metrics it names, and
/// the window it asks about.
fn query(group_by: &[&str], metrics: &[&str], window: &(String, String)) -> Vec<(String, String)> {
    let mut params = indexed("GroupBy", group_by);
    params.extend(indexed("MetricName", metrics));
    params.push(p("StartTime", window.0.clone()));
    params.push(p("EndTime", window.1.clone()));
    params
}

/// The window that spans this instant: wide enough that the gap between this suite reading
/// the clock and the handler reading it cannot decide a case.
const NOW_WINDOW: (i64, i64) = (-HOUR, HOUR);

/// One `GetCapacityManagerMetricDimensions` call.
async fn dimensions(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(
        router,
        REGION,
        "GetCapacityManagerMetricDimensions",
        params,
    )
    .await
}

/// Turn Capacity Manager on for the account.
async fn enable(router: &MultiServiceRouter) {
    let (status, body) = call(router, REGION, "EnableCapacityManager", &[]).await;
    assert_eq!(
        status, 200,
        "EnableCapacityManager must succeed, got {status} with body {body}"
    );
}

/// What one seeded Capacity Reservation is, and the dimension values it must carry.
struct Reserved {
    zone: &'static str,
    zone_id: &'static str,
    instance_type: &'static str,
    family: &'static str,
    platform: &'static str,
    tenancy: &'static str,
}

/// The reservations rule 2 is asserted over: two in one zone that share a family and a
/// platform, one in another zone with its own family, platform and tenancy — so a grouping
/// folds some rows together and keeps others apart.
const RESERVED: [Reserved; 3] = [
    Reserved {
        zone: "us-east-1a",
        zone_id: "use1-az1",
        instance_type: "m5.large",
        family: "m5",
        platform: "Linux/UNIX",
        tenancy: "default",
    },
    Reserved {
        zone: "us-east-1a",
        zone_id: "use1-az1",
        instance_type: "m5.xlarge",
        family: "m5",
        platform: "Linux/UNIX",
        tenancy: "default",
    },
    Reserved {
        zone: "us-east-1b",
        zone_id: "use1-az2",
        instance_type: "c5.xlarge",
        family: "c5",
        platform: "Windows",
        tenancy: "dedicated",
    },
];

/// Reserve the capacity one fixture names, answering the `CapacityReservationId` the create
/// minted.
async fn reserve(router: &MultiServiceRouter, fixture: &Reserved) -> String {
    let params = vec![
        p("AvailabilityZone", fixture.zone),
        p("InstanceType", fixture.instance_type),
        p("InstancePlatform", fixture.platform),
        p("InstanceCount", "2"),
        p("Tenancy", fixture.tenancy),
    ];
    let (status, body) = call(router, REGION, "CreateCapacityReservation", &params).await;
    assert_eq!(
        status, 200,
        "CreateCapacityReservation for {} must succeed, got {status} with body {body}",
        fixture.instance_type
    );
    element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("the create must report the reservation it minted: {body}"))
}

/// An emulator with Capacity Manager on and every [`RESERVED`] fixture reserved, answering
/// the ids in fixture order.
async fn account_with_reservations() -> (MultiServiceRouter, Vec<String>) {
    let (router, _engine) = emulator();
    enable(&router).await;
    let mut ids = Vec::new();
    for fixture in &RESERVED {
        ids.push(reserve(&router, fixture).await);
    }
    (router, ids)
}

/// Launch one on-demand instance of `instance_type` from `image_id`, answering its id.
async fn launch(
    router: &MultiServiceRouter,
    instance_type: &str,
    image_id: &str,
    zone: &str,
) -> String {
    let params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
        p("InstanceType", instance_type),
        p("Placement.AvailabilityZone", zone),
    ];
    let (status, body) = call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// Every combination a successful report carries, as its member names and values.
fn reported(case: &str, body: &str) -> Vec<BTreeMap<String, String>> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the response must be XML ({error}): {body}"));
    document
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "metricDimensionResultSet")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .map(|item| {
                    item.children()
                        .filter(|member| member.is_element())
                        .map(|member| {
                            (
                                member.tag_name().name().to_string(),
                                member.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect::<BTreeMap<String, String>>()
                })
                .collect::<Vec<BTreeMap<String, String>>>()
        })
        .collect()
}

/// One successful report's combinations, each rendered as the values of `members` in that
/// order — and pinned to carry EXACTLY those members, so a combination is the grouping the
/// request asked for and nothing else.
async fn combinations(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
    members: &[&str],
) -> BTreeSet<Vec<String>> {
    let (status, body) = dimensions(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported(case, &body)
        .into_iter()
        .map(|combination| {
            // The members a combination carries, as a SET: the response writes them in the
            // modelled shape's own order, which is not the order the request grouped by.
            let carried: BTreeSet<String> = combination.keys().cloned().collect();
            let expected: BTreeSet<String> =
                members.iter().map(|name| (*name).to_string()).collect();
            assert_eq!(
                carried, expected,
                "case {case:?}: expected a combination of exactly {expected:?}, got {carried:?} \
                 in {body}"
            );
            members
                .iter()
                .map(|member| combination.get(*member).cloned().unwrap_or_default())
                .collect()
        })
        .collect()
}

/// The combinations as owned rows, for comparing against a table's expectations.
fn rows(values: &[&[&str]]) -> BTreeSet<Vec<String>> {
    values
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect()
}

/// One row of the grouping table: what it covers, the dimensions it groups by, the members
/// a combination then carries, and the combinations the account's capacity must yield.
type GroupingRow = (&'static str, Vec<&'static str>, Vec<&'static str>, Combinations);

/// One row of the condition table: what it covers, the `FilterBy` fields it sends, and the
/// combinations that survive them.
type ConditionRow = (&'static str, Vec<(String, String)>, Combinations);

/// One row of the refusal table: what it covers, the form fields it sends, and the error
/// code and HTTP status it must be answered with.
type RefusalRow = (&'static str, Vec<(String, String)>, &'static str, u16);

/// The combinations one report carries, each as the values of the members it was read by.
type Combinations = BTreeSet<Vec<String>>;

/// RULE 1: whatever a request asks about, an account that has not enabled Capacity Manager
/// is refused `CapacityManager.Disabled` — and the refusal comes back once the account
/// disables it again.
#[tokio::test]
async fn an_account_without_capacity_manager_is_refused() {
    let now = window(NOW_WINDOW);
    // Rows differing along everything the refusal must not depend on: the grouping, the
    // metric family, the window and the page size.
    let rows: [(&str, Vec<(String, String)>); 4] = [
        (
            "grouped by one dimension",
            query(&["instance-type"], &[RESERVATION_METRIC], &now),
        ),
        (
            "grouped by three dimensions",
            query(
                &["resource-region", "availability-zone-id", "instance-family"],
                &[SPOT_METRIC],
                &now,
            ),
        ),
        (
            "over a window in the past",
            query(&["account-id"], &[UNRESERVED_METRIC], &window((-2 * DAY, -DAY))),
        ),
        ("a one-result page", {
            let mut params = query(&["reservation-id"], &[RESERVED_USAGE_METRIC], &now);
            params.push(p("MaxResults", "1"));
            params
        }),
    ];

    for (case, params) in rows {
        let (router, _engine) = emulator();
        let (status, body) = dimensions(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an account that never enabled Capacity \
             Manager, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "CapacityManager.Disabled",
            "case {case:?}: expected CapacityManager.Disabled, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Capacity Manager isn't enabled for this account.",
            "case {case:?}: expected the refusal EC2 words for a Capacity Manager that is \
             off, got {body}"
        );

        // The same request is served once the account enables it, and refused again after a
        // disable — so the refusal tracks the setting rather than the request.
        enable(&router).await;
        let (status, body) = dimensions(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 once Capacity Manager was enabled, got {status} \
             with body {body}"
        );
        let (status, body) = call(&router, REGION, "DisableCapacityManager", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisableCapacityManager must succeed, got {status} with body {body}"
        );
        let (status, body) = dimensions(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 after the disable, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "CapacityManager.Disabled",
            "case {case:?}: expected CapacityManager.Disabled after the disable, got {body}"
        );
    }
}

/// RULE 2: the report is the distinct projections of the account's reservations onto the
/// dimensions the request grouped by.
#[tokio::test]
async fn the_report_is_the_distinct_combinations_of_the_grouped_dimensions() {
    let (router, ids) = account_with_reservations().await;
    let now = window(NOW_WINDOW);

    // `(case, the dimensions grouped by, the members a combination then carries, the
    // combinations the account's capacity must yield)`.
    let table: Vec<GroupingRow> = vec![
        (
            "one dimension every resource carries, two of them sharing it",
            vec!["instance-family"],
            vec!["instanceFamily"],
            rows(&[&[RESERVED[0].family], &[RESERVED[2].family]]),
        ),
        (
            "a dimension no two resources share",
            vec!["instance-type"],
            vec!["instanceType"],
            rows(&[&["m5.large"], &["m5.xlarge"], &["c5.xlarge"]]),
        ),
        (
            "a dimension the whole account agrees on",
            vec!["resource-region"],
            vec!["resourceRegion"],
            rows(&[&["us-east-1"]]),
        ),
        (
            "the zone the capacity is in",
            vec!["availability-zone-id"],
            vec!["availabilityZoneId"],
            rows(&[&["use1-az1"], &["use1-az2"]]),
        ),
        (
            "the platform of the reserved capacity",
            vec!["instance-platform"],
            vec!["instancePlatform"],
            rows(&[&["Linux/UNIX"], &["Windows"]]),
        ),
        (
            "the tenancy of the hardware",
            vec!["tenancy"],
            vec!["tenancy"],
            rows(&[&["default"], &["dedicated"]]),
        ),
        (
            "the state of the reservation",
            vec!["reservation-state"],
            vec!["reservationState"],
            rows(&[&["active"]]),
        ),
        (
            "two dimensions at once",
            vec!["availability-zone-id", "instance-platform"],
            vec!["availabilityZoneId", "instancePlatform"],
            rows(&[&["use1-az1", "Linux/UNIX"], &["use1-az2", "Windows"]]),
        ),
        (
            // The reservations have no end date, so none carries that dimension and none
            // yields a combination of it — a dimension with no value is no combination.
            "a dimension no resource of the account carries",
            vec!["reservation-end-timestamp"],
            vec!["reservationEndTimestamp"],
            BTreeSet::new(),
        ),
        (
            // Nothing in this emulator names an account, which is also what AWS reports
            // for an account whose Capacity Manager has no Organizations access.
            "the account name, which no resource carries",
            vec!["account-name"],
            vec!["accountName"],
            BTreeSet::new(),
        ),
    ];

    for (case, group_by, members, expected) in table {
        let params = query(&group_by, &[RESERVATION_METRIC], &now);
        let reported = combinations(&router, case, &params, &members).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the combinations {expected:?}, got {reported:?}"
        );
    }

    // Grouped by the reservation itself, every reservation of the account is its own
    // combination — the ids the creates minted, not a count this suite wrote.
    let params = query(&["reservation-id"], &[RESERVATION_METRIC], &now);
    let reported = combinations(&router, "grouped by the reservation", &params, &["reservationId"])
        .await;
    let expected: BTreeSet<Vec<String>> = ids.iter().map(|id| vec![id.clone()]).collect();
    assert_eq!(
        reported, expected,
        "case \"grouped by the reservation\": expected one combination per reservation \
         ({expected:?}), got {reported:?}"
    );
}

/// RULE 3: a resource is reported only when its own window overlaps the one the request
/// asked about.
#[tokio::test]
async fn only_capacity_with_data_in_the_window_is_reported() {
    let (router, _engine) = emulator();
    enable(&router).await;
    reserve(&router, &RESERVED[0]).await;

    // `(case, the window, whether the reserved capacity has data in it)`. The reservation
    // was made now and has no end date, so every window that reaches this instant or later
    // holds it and every window that ends before it does not.
    let table: [(&str, (i64, i64), bool); 5] = [
        ("a window spanning now", NOW_WINDOW, true),
        ("a window starting now", (0, DAY), true),
        ("a window entirely in the future", (9 * DAY, 10 * DAY), true),
        (
            "a window entirely before the capacity existed",
            (-10 * DAY, -9 * DAY),
            false,
        ),
        (
            "a window ending an hour before now",
            (-2 * HOUR, -HOUR),
            false,
        ),
    ];

    for (case, offsets, has_data) in table {
        let params = query(&["instance-type"], &[RESERVATION_METRIC], &window(offsets));
        let reported = combinations(&router, case, &params, &["instanceType"]).await;
        let expected = if has_data {
            rows(&[&[RESERVED[0].instance_type]])
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the combinations {expected:?} in the window \
             {offsets:?}, got {reported:?}"
        );
    }
}

/// RULE 4: each metric family is measured over the capacity it names — the account's
/// reservations, its Spot capacity, or the capacity no reservation covers.
#[tokio::test]
async fn each_metric_family_is_measured_over_the_capacity_it_names() {
    let (router, _engine) = emulator();
    let now = window(NOW_WINDOW);
    enable(&router).await;
    // Reserved capacity of one instance type, on-demand capacity of another, and a Spot
    // request for a third — so which capacity a metric is measured over decides which
    // instance types it can report.
    reserve(&router, &RESERVED[0]).await;
    let on_demand = launch(&router, "r6i.large", &catalog_image(false), RESERVED[0].zone).await;
    let spot = vec![
        p("LaunchSpecification.ImageId", catalog_image(true)),
        p("LaunchSpecification.InstanceType", "c6g.medium"),
        p("LaunchSpecification.Placement.AvailabilityZone", RESERVED[2].zone),
    ];
    let (status, body) = call(&router, REGION, "RequestSpotInstances", &spot).await;
    assert_eq!(
        status, 200,
        "RequestSpotInstances must succeed, got {status} with body {body}"
    );
    // The Spot request is fulfilled by the CLOCK, so the Spot capacity exists once its span
    // has passed — whether or not anybody asked in between.
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;

    // `(case, the metric, the instance types that capacity is measured over)`.
    let table: [(&str, &str, &[&str]); 4] = [
        (
            "the reserved capacity itself",
            RESERVATION_METRIC,
            &[RESERVED[0].instance_type],
        ),
        (
            "the usage the reserved capacity covered",
            RESERVED_USAGE_METRIC,
            &[RESERVED[0].instance_type],
        ),
        ("Spot capacity", SPOT_METRIC, &["c6g.medium"]),
        (
            "capacity no reservation covers",
            UNRESERVED_METRIC,
            &["r6i.large"],
        ),
    ];

    for (case, metric, expected_types) in table {
        let params = query(&["instance-type"], &[metric], &now);
        let reported = combinations(&router, case, &params, &["instanceType"]).await;
        let expected: BTreeSet<Vec<String>> = expected_types
            .iter()
            .map(|instance_type| vec![(*instance_type).to_string()])
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the metric {metric:?} to be measured over \
             {expected:?}, got {reported:?}"
        );
    }

    // An instance carries no reservation identity, so grouping instance capacity by one
    // yields no combination at all — while the reserved capacity still does.
    for (case, metric, expected) in [
        ("Spot capacity by reservation", SPOT_METRIC, false),
        (
            "unreserved capacity by reservation",
            UNRESERVED_METRIC,
            false,
        ),
        ("reserved capacity by reservation", RESERVATION_METRIC, true),
    ] {
        let params = query(&["reservation-id"], &[metric], &now);
        let reported = combinations(&router, case, &params, &["reservationId"]).await;
        assert_eq!(
            !reported.is_empty(),
            expected,
            "case {case:?}: expected combinations present={expected}, got {reported:?}"
        );
    }

    // The platform and the zone of instance capacity are the instance's own: the Spot
    // instance was launched from the Windows AMI into the other zone.
    let params = query(
        &["instance-platform", "availability-zone-id"],
        &[SPOT_METRIC],
        &now,
    );
    let reported = combinations(
        &router,
        "the platform and zone of Spot capacity",
        &params,
        &["instancePlatform", "availabilityZoneId"],
    )
    .await;
    assert_eq!(
        reported,
        rows(&[&["Windows", RESERVED[2].zone_id]]),
        "case \"the platform and zone of Spot capacity\": expected the launched instance's \
         own platform and zone, got {reported:?}"
    );

    // A TERMINATED instance holds no capacity any more, so it is reported no dimensions —
    // the account's reserved capacity, which nothing terminated, is still reported.
    let (status, body) = call(
        &router,
        REGION,
        "TerminateInstances",
        &[p("InstanceId.1", on_demand.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "TerminateInstances must succeed, got {status} with body {body}"
    );
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
    let case = "capacity an instance no longer holds";
    let params = query(&["instance-type"], &[UNRESERVED_METRIC], &now);
    let reported = combinations(&router, case, &params, &["instanceType"]).await;
    assert_eq!(
        reported,
        BTreeSet::new(),
        "case {case:?}: expected a terminated instance to be reported no dimensions, got \
         {reported:?}"
    );
    let case = "reserved capacity after the instance was terminated";
    let params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
    let reported = combinations(&router, case, &params, &["instanceType"]).await;
    assert_eq!(
        reported,
        rows(&[&[RESERVED[0].instance_type]]),
        "case {case:?}: expected the reservation still reported, got {reported:?}"
    );
}

/// RULE 5: a `FilterBy` condition narrows the combinations to the resources it holds for.
#[tokio::test]
async fn filter_conditions_narrow_the_combinations() {
    let (router, _engine) = account_with_reservations().await;
    let now = window(NOW_WINDOW);

    /// One condition's form fields, at position `index` counting from 1.
    fn condition(index: usize, dimension: &str, comparison: &str, values: &[&str]) -> Vec<(String, String)> {
        let prefix = format!("FilterBy.{index}.DimensionCondition");
        let mut fields = vec![
            p(&format!("{prefix}.Dimension"), dimension),
            p(&format!("{prefix}.Comparison"), comparison),
        ];
        fields.extend(indexed(&format!("{prefix}.Value"), values));
        fields
    }

    // `(case, the conditions, the instance types that survive them)`.
    let table: Vec<ConditionRow> = vec![
        (
            "equals one instance type",
            condition(1, "instance-type", "equals", &["m5.large"]),
            rows(&[&["m5.large"]]),
        ),
        (
            "equals uses only its first value",
            condition(1, "instance-type", "equals", &["c5.xlarge", "m5.large"]),
            rows(&[&["c5.xlarge"]]),
        ),
        (
            "in matches any of its values",
            condition(1, "instance-type", "in", &["m5.large", "c5.xlarge"]),
            rows(&[&["m5.large"], &["c5.xlarge"]]),
        ),
        (
            "a condition on another dimension entirely",
            condition(1, "availability-zone-id", "equals", &["use1-az1"]),
            rows(&[&["m5.large"], &["m5.xlarge"]]),
        ),
        (
            "two conditions are joined with an AND",
            {
                let mut fields = condition(1, "instance-family", "equals", &["m5"]);
                fields.extend(condition(2, "instance-type", "in", &["m5.xlarge", "c5.xlarge"]));
                fields
            },
            rows(&[&["m5.xlarge"]]),
        ),
        (
            "two conditions nothing satisfies",
            {
                let mut fields = condition(1, "instance-family", "equals", &["m5"]);
                fields.extend(condition(2, "availability-zone-id", "equals", &["use1-az2"]));
                fields
            },
            BTreeSet::new(),
        ),
        (
            "a value no resource carries",
            condition(1, "instance-type", "equals", &["x2gd.metal"]),
            BTreeSet::new(),
        ),
        (
            "a dimension no resource carries a value for",
            condition(1, "account-name", "equals", &["any"]),
            BTreeSet::new(),
        ),
    ];

    for (case, conditions, expected) in table {
        let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
        params.extend(conditions);
        let reported = combinations(&router, case, &params, &["instanceType"]).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the combinations {expected:?}, got {reported:?}"
        );
    }
}

/// RULE 6: the combinations are paged in one stable order, and a token is valid only for the
/// query it was minted for.
#[tokio::test]
async fn the_combinations_are_paged_in_a_stable_order() {
    let (router, _engine) = account_with_reservations().await;
    let now = window(NOW_WINDOW);

    // Every page size from one combination at a time to more than the account holds, plus
    // the request that names none at all.
    for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE)] {
        let case = format!("pages of {page_size:?}");
        let mut seen: Vec<Vec<String>> = Vec::new();
        let mut token: Option<String> = None;
        for step in 0..8 {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            if let Some(page_size) = page_size {
                params.push(p("MaxResults", page_size.to_string()));
            }
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = dimensions(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported(&case, &body);
            if let Some(page_size) = page_size {
                assert!(
                    page.len() <= page_size,
                    "case {case:?} step {step}: expected at most {page_size} combinations, \
                     got {}",
                    page.len()
                );
            }
            seen.extend(
                page.iter()
                    .map(|combination| combination.values().cloned().collect::<Vec<String>>()),
            );
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to end without a token, got {token:?}"
        );
        let walked: BTreeSet<Vec<String>> = seen.iter().cloned().collect();
        assert_eq!(
            walked,
            rows(&[&["m5.large"], &["m5.xlarge"], &["c5.xlarge"]]),
            "case {case:?}: expected the walk to report every combination once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            walked.len(),
            "case {case:?}: expected no combination twice, got {seen:?}"
        );
        let mut ordered = seen.clone();
        ordered.sort();
        assert_eq!(
            seen, ordered,
            "case {case:?}: expected the pages in one ascending order, got {seen:?}"
        );
    }

    // A token names a position in ONE query's order, so it is refused for another query
    // rather than resumed inside its page.
    let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
    params.push(p("MaxResults", "1"));
    let (status, body) = dimensions(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first page must succeed, got {status} with body {body}"
    );
    let token =
        element(&body, "nextToken").unwrap_or_else(|| panic!("a first page of one must carry a token: {body}"));
    for (case, mut other) in [
        (
            "another grouping",
            query(&["instance-family"], &[RESERVATION_METRIC], &now),
        ),
        (
            "another window",
            query(&["instance-type"], &[RESERVATION_METRIC], &window((-DAY, DAY))),
        ),
        (
            "another metric",
            query(&["instance-type"], &[UNRESERVED_METRIC], &now),
        ),
        (
            // A condition decides which combinations exist at all, so it is part of the
            // query the cursor names a position in.
            "the same query with a condition added",
            {
                let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
                params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-family"));
                params.push(p("FilterBy.1.DimensionCondition.Comparison", "equals"));
                params.push(p("FilterBy.1.DimensionCondition.Value.1", RESERVED[0].family));
                params
            },
        ),
    ] {
        other.push(p("NextToken", token.clone()));
        let (status, body) = dimensions(&router, &other).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a token of another query to be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // And a FILTERED query pages on its own token: the walk reports exactly the combinations
    // the condition leaves, so naming the conditions in the token's scope did not make a
    // filtered report unpageable.
    let case = "paging a filtered query";
    let filtered = {
        let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
        params.push(p("MaxResults", "1"));
        params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-family"));
        params.push(p("FilterBy.1.DimensionCondition.Comparison", "equals"));
        params.push(p("FilterBy.1.DimensionCondition.Value.1", RESERVED[0].family));
        params
    };
    let mut walked: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut token: Option<String> = None;
    for step in 0..8 {
        let mut params = filtered.clone();
        if let Some(token) = token.as_deref() {
            params.push(p("NextToken", token));
        }
        let (status, body) = dimensions(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?} step {step}: expected HTTP 200, got {status} with body {body}"
        );
        walked.extend(
            reported(case, &body)
                .iter()
                .map(|combination| combination.values().cloned().collect::<Vec<String>>()),
        );
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked,
        rows(&[
            &[RESERVED[0].instance_type],
            &[RESERVED[1].instance_type]
        ]),
        "case {case:?}: expected the walk to report the filtered combinations, got {walked:?}"
    );
}

/// RULE 7: a request outside a documented rule is refused with EC2's own code, and the
/// account is left exactly as it was.
#[tokio::test]
async fn requests_outside_a_documented_rule_are_refused() {
    let (router, _engine) = account_with_reservations().await;
    let now = window(NOW_WINDOW);

    // `(case, the form fields, the code and the status EC2 answers)`.
    let table: Vec<RefusalRow> = vec![
        (
            // An absent `@required` member is refused by the framework's own constraint
            // walk, before any handler runs: which code EC2 answers for each framework
            // refusal, and how it words it, is one rule for the whole service and has an
            // issue of its own, so this pins what the service answers today rather than
            // restating that rule here.
            "no dimension to group by",
            query(&[], &[RESERVATION_METRIC], &now),
            "MalformedInput",
            400,
        ),
        (
            // The dimension catalog is the model's own, enforced by the framework's
            // constraint walk before any handler runs — like the two rows above, this pins
            // what the service answers for a framework refusal today.
            "a dimension outside the catalog",
            query(&["instance-colour"], &[RESERVATION_METRIC], &now),
            "ValidationError",
            400,
        ),
        (
            // Refused by that same constraint walk, for the same reason as the absent
            // `GroupBy` above.
            "no metric at all",
            query(&["instance-type"], &[], &now),
            "MalformedInput",
            400,
        ),
        (
            // The metric catalog is the model's own too.
            "a metric outside the catalog",
            query(&["instance-type"], &["reservation-total-moons"], &now),
            "ValidationError",
            400,
        ),
        (
            "more metrics than the one this version serves",
            query(
                &["instance-type"],
                &[RESERVATION_METRIC, SPOT_METRIC],
                &now,
            ),
            "InvalidParameterValue",
            400,
        ),
        (
            "a window whose end precedes its start",
            query(&["instance-type"], &[RESERVATION_METRIC], &window((HOUR, -HOUR))),
            "InvalidParameterValue",
            400,
        ),
        (
            // Both ends the SAME instant, so the window has no length rather than merely a
            // short one.
            "a window of no length at all",
            query(&["instance-type"], &[RESERVATION_METRIC], &{
                let instant = timestamp_in(HOUR);
                (instant.clone(), instant)
            }),
            "InvalidParameterValue",
            400,
        ),
        ("a page of no results", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("MaxResults", "0"));
            params
        }, "InvalidParameterValue", 400),
        ("a page past the published ceiling", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("MaxResults", (MAX_PAGE_SIZE + 1).to_string()));
            params
        }, "InvalidParameterValue", 400),
        ("a condition naming more values than the crate's filter ceiling", {
            // The model's own `@length` on `ConditionValueList` (0..10) is far below the
            // crate's shared `MAX_FILTER_VALUES`, so the framework's constraint walk
            // refuses a list this long before the handler's own ceiling is reached — the
            // handler keeps that ceiling so the bound holds whatever the model publishes.
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-type"));
            params.push(p("FilterBy.1.DimensionCondition.Comparison", "in"));
            for index in 1..=MAX_FILTER_VALUES + 1 {
                params.push(p(
                    &format!("FilterBy.1.DimensionCondition.Value.{index}"),
                    format!("m5.size{index}"),
                ));
            }
            params
        }, "ValidationError", 400),
        ("a token this service did not mint", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("NextToken", "not-a-token"));
            params
        }, "InvalidPaginationToken", 400),
        ("a condition naming no dimension", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Comparison", "equals"));
            params.push(p("FilterBy.1.DimensionCondition.Value.1", "m5.large"));
            params
        }, "MissingParameter", 400),
        ("a condition naming no comparison", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-type"));
            params.push(p("FilterBy.1.DimensionCondition.Value.1", "m5.large"));
            params
        }, "MissingParameter", 400),
        ("a condition with no value to match", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-type"));
            params.push(p("FilterBy.1.DimensionCondition.Comparison", "equals"));
            params
        }, "MissingParameter", 400),
        ("a condition on a dimension outside the catalog", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-colour"));
            params.push(p("FilterBy.1.DimensionCondition.Comparison", "equals"));
            params.push(p("FilterBy.1.DimensionCondition.Value.1", "green"));
            params
        }, "ValidationError", 400),
        ("a comparison outside the catalog", {
            let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
            params.push(p("FilterBy.1.DimensionCondition.Dimension", "instance-type"));
            params.push(p("FilterBy.1.DimensionCondition.Comparison", "starts-with"));
            params.push(p("FilterBy.1.DimensionCondition.Value.1", "m5"));
            params
        }, "ValidationError", 400),
    ];

    for (case, params, code, expected_status) in table {
        let (status, body) = dimensions(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // A `DryRun` request reports the permission outcome and reads nothing.
    let mut params = query(&["instance-type"], &[RESERVATION_METRIC], &now);
    params.push(p("DryRun", "true"));
    let (status, body) = dimensions(&router, &params).await;
    assert_eq!(
        status, 412,
        "case \"a dry run\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry run\": expected DryRunOperation, got {body}"
    );
}

/// RULE 8: the report writes nothing — two identical calls agree, every refused request
/// leaves the account where it was, and the settings the report read are untouched.
#[tokio::test]
async fn the_report_writes_nothing() {
    let (router, ids) = account_with_reservations().await;
    let now = window(NOW_WINDOW);
    let params = query(
        &["instance-type", "availability-zone-id"],
        &[RESERVATION_METRIC],
        &now,
    );
    let members = ["instanceType", "availabilityZoneId"];

    let first = combinations(&router, "the first read", &params, &members).await;
    let second = combinations(&router, "the second read", &params, &members).await;
    assert_eq!(
        second, first,
        "case \"two reads in a row\": expected the same combinations, got {second:?} after \
         {first:?}"
    );

    // The reservations the report read are still there, with the states they had.
    let (status, body) = call(&router, REGION, "DescribeCapacityReservations", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeCapacityReservations must succeed, got {status} with body {body}"
    );
    for id in &ids {
        assert!(
            body.contains(id.as_str()),
            "case \"the reservations after a read\": expected {id} still reported, got {body}"
        );
    }

    // And the Capacity Manager settings the refusal is decided on are untouched.
    let (status, body) = call(&router, REGION, "GetCapacityManagerAttributes", &[]).await;
    assert_eq!(
        status, 200,
        "GetCapacityManagerAttributes must succeed, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "capacityManagerStatus").as_deref(),
        Some("enabled"),
        "case \"the settings after a read\": expected Capacity Manager still enabled, got {body}"
    );
}

/// RULE 9: every comparison a `FilterBy` condition makes is charged to the one budget a
/// request's whole filtering shares, and a budget that cannot pay answers "no match" rather
/// than doing the work — which is what keeps the product of stored capacity rows, conditions
/// and values finite. Asserted on the condition reader itself, because the bound belongs to
/// it rather than to one caller, and a request large enough to spend the service's whole
/// budget is not something to send over a socket.
#[test]
fn every_comparison_a_condition_makes_is_charged_to_the_budget() {
    let row = CapacityManagerDimension {
        instance_type: Some("m5.large".to_string()),
        ..CapacityManagerDimension::default()
    };
    // `(case, the comparison, its values, the steps the condition costs, whether it holds)` —
    // rows differing along what the charge must cover: reading the row's own value, a single
    // `equals` comparison, a match found at the front of an `in` list and one found at its
    // end, and a list nothing matches.
    let table: [(&str, &str, &[&str], u64, bool); 5] = [
        (
            "equals, matching",
            CAPACITY_MANAGER_COMPARISON_EQUALS,
            &["m5.large"],
            2,
            true,
        ),
        (
            "equals, not matching",
            CAPACITY_MANAGER_COMPARISON_EQUALS,
            &["c5.xlarge"],
            2,
            false,
        ),
        (
            "in, matching the first value",
            CAPACITY_MANAGER_COMPARISON_IN,
            &["m5.large", "c5.xlarge", "r6i.large"],
            2,
            true,
        ),
        (
            "in, matching the last value",
            CAPACITY_MANAGER_COMPARISON_IN,
            &["c5.xlarge", "r6i.large", "m5.large"],
            4,
            true,
        ),
        (
            "in, matching nothing",
            CAPACITY_MANAGER_COMPARISON_IN,
            &["c5.xlarge", "r6i.large"],
            3,
            false,
        ),
    ];

    for (case, comparison, values, cost, expected) in table {
        let values: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();

        // With exactly what it costs, the condition is answered and the budget is spent to
        // nothing — so no comparison was made for free.
        let mut budget = MatchBudget::new(cost);
        let held = capacity_manager_condition_holds(
            &row,
            DIMENSION_INSTANCE_TYPE,
            comparison,
            &values,
            &mut budget,
        );
        assert_eq!(
            held, expected,
            "case {case:?}: expected the condition to hold={expected} on a budget of \
             {cost}, got {held}"
        );
        assert!(
            budget.exhausted(),
            "case {case:?}: expected a budget of {cost} to be spent to nothing, got \
             {budget:?}"
        );

        // One step short of that, the answer is "no match" and the caller is told the budget
        // ran out rather than being served an incomplete report.
        let mut short = MatchBudget::new(cost - 1);
        let held = capacity_manager_condition_holds(
            &row,
            DIMENSION_INSTANCE_TYPE,
            comparison,
            &values,
            &mut short,
        );
        assert!(
            !held,
            "case {case:?}: expected no match on a budget of {}, got {held}",
            cost - 1
        );
        assert!(
            short.exhausted(),
            "case {case:?}: expected a budget of {} to run out, got {short:?}",
            cost - 1
        );

        // A dimension the row carries no value for costs the one step it takes to find that
        // out, whatever the condition names.
        let mut absent = MatchBudget::new(1);
        let held = capacity_manager_condition_holds(
            &row,
            "account-name",
            comparison,
            &values,
            &mut absent,
        );
        assert!(
            !held,
            "case {case:?}: expected no match on a dimension the row does not carry, got \
             {held}"
        );
        assert!(
            absent.exhausted(),
            "case {case:?}: expected reading an absent dimension to cost a step, got \
             {absent:?}"
        );
    }
}
