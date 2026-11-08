// SPDX-License-Identifier: CC0-1.0

//! The EC2 tag filter family on the describes of a TAGGABLE resource, driven over the wire
//! the service speaks (`ec2Query`) through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a describe whose reported shape carries the modelled `Tags` member
//! answers the tag filter family — `tag:<key>`, `tag-key` and `tag-value`, the three names
//! the model spells out for a taggable resource
//! (`smithy_query(query="doc",shape="DescribeInstanceEventWindowsRequest",member="Filters")`)
//! — and selects on the tags the resource really holds, whether or not that operation's own
//! `Filters` member enumerates them. A name that is neither documented by the operation nor
//! one of the family is still refused, so an unsupported filter can never read as "no
//! results".
//!
//! `DescribeCapacityReservations` (`CapacityReservation$Tags`, `tagSet`) and
//! `DescribeCapacityReservationFleets` (`CapacityReservationFleet$Tags`, `tagSet`) are the
//! two describes asserted here, each over a table whose rows differ along what the rule
//! generalises over: which name of the family, which key, one value or two, an exact value
//! or an EC2 wildcard, the wrong case, a key no resource carries, the family beside a
//! documented filter, and a resource with no tags at all.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    RESOURCE_TYPE_CAPACITY_RESERVATION, RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET, TAG_KEY_FILTER,
    TAG_VALUE_FILTER,
};

use super::ec2_query::{call, elements, error_code, filters};

/// The Region every call here is signed for: a Capacity Reservation is reported in the
/// Region of the zone it reserves in, so the seeds below reserve in a zone of this one.
const REGION: &str = "us-west-2";

/// One row of the filter table: what it covers, the `Filters` entries it sends, and the
/// indexes into the seeded resources the filter must select.
type FilterRow = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<usize>,
);

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The form fields of a tag specification carrying `tags` for `resource_type`.
///
/// `prefix` is the member's own wire name, which differs per operation:
/// `CreateCapacityReservation` carries `TagSpecifications.N`, `CreateCapacityReservationFleet`
/// `TagSpecification.N`.
fn tag_specification(
    prefix: &str,
    resource_type: &str,
    tags: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut params = vec![p(&format!("{prefix}.1.ResourceType"), resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(
            &format!("{prefix}.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(p(
            &format!("{prefix}.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// The first Availability Zone of [`REGION`], read off `DescribeAvailabilityZones` rather
/// than written down here.
async fn first_zone(router: &MultiServiceRouter) -> String {
    let (status, body) = api(router, "DescribeAvailabilityZones", &[]).await;
    assert_eq!(
        status, 200,
        "the Region must report its zones, got {status} with body {body}"
    );
    elements(&body, "zoneName")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the Region must report at least one zone: {body}"))
}

/// The tags each seeded resource carries. The rows below select over exactly these: two keys
/// on the first, one shared key with another value on the second, and NO tags on the third.
const SEEDED_TAGS: [&[(&str, &str)]; 3] = [
    &[("Name", "alpha"), ("Owner", "TeamA")],
    &[("Name", "beta")],
    &[],
];

/// Create one Capacity Reservation per entry of [`SEEDED_TAGS`] and answer their ids, in
/// the order they were created.
async fn seed_reservations(router: &MultiServiceRouter) -> Vec<String> {
    let zone = first_zone(router).await;
    let mut ids = Vec::new();
    for (index, tags) in SEEDED_TAGS.iter().enumerate() {
        let mut params = vec![
            p("AvailabilityZone", &zone),
            p("InstanceType", "m5.large"),
            p("InstancePlatform", "Linux/UNIX"),
            p("InstanceCount", "1"),
        ];
        if !tags.is_empty() {
            params.extend(tag_specification(
                "TagSpecifications",
                RESOURCE_TYPE_CAPACITY_RESERVATION,
                tags,
            ));
        }
        let (status, body) = api(router, "CreateCapacityReservation", &params).await;
        assert_eq!(
            status, 200,
            "seeding reservation {index}: expected HTTP 200, got {status} with body {body}"
        );
        ids.push(
            elements(&body, "capacityReservationId")
                .into_iter()
                .next()
                .unwrap_or_else(|| {
                    panic!("seeding reservation {index}: no id in body {body}")
                }),
        );
    }
    ids
}

/// Create one Capacity Reservation Fleet per entry of [`SEEDED_TAGS`] and answer their ids,
/// in the order they were created.
async fn seed_fleets(router: &MultiServiceRouter) -> Vec<String> {
    let zone = first_zone(router).await;
    let mut ids = Vec::new();
    for (index, tags) in SEEDED_TAGS.iter().enumerate() {
        let mut params = vec![
            p("TotalTargetCapacity", "2"),
            p("InstanceTypeSpecification.1.InstanceType", "m5.large"),
            p("InstanceTypeSpecification.1.InstancePlatform", "Linux/UNIX"),
            p("InstanceTypeSpecification.1.AvailabilityZone", &zone),
            p("InstanceTypeSpecification.1.Weight", "1.0"),
        ];
        if !tags.is_empty() {
            params.extend(tag_specification(
                "TagSpecification",
                RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET,
                tags,
            ));
        }
        let (status, body) = api(router, "CreateCapacityReservationFleet", &params).await;
        assert_eq!(
            status, 200,
            "seeding fleet {index}: expected HTTP 200, got {status} with body {body}"
        );
        ids.push(
            elements(&body, "capacityReservationFleetId")
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("seeding fleet {index}: no id in body {body}")),
        );
    }
    ids
}

/// The rows every describe of a taggable resource is asserted over, written against
/// [`SEEDED_TAGS`]: `documented` is a filter the operation's own `Filters` member lists,
/// which every seeded resource matches, so the conjunction row can join the family with it.
fn rows(documented: (&'static str, &'static [&'static str])) -> Vec<FilterRow> {
    vec![
        (
            "tag:<key> on the value one resource carries",
            vec![("tag:Name", &["alpha"])],
            vec![0],
        ),
        (
            "tag:<key> on the value another carries",
            vec![("tag:Name", &["beta"])],
            vec![1],
        ),
        (
            "tag:<key> with two values, joined with an OR",
            vec![("tag:Name", &["alpha", "beta"])],
            vec![0, 1],
        ),
        (
            "tag:<key> with an EC2 wildcard",
            vec![("tag:Name", &["*a"])],
            vec![0, 1],
        ),
        (
            "tag:<key> in the wrong case selects nothing, filter values being \
             case-sensitive",
            vec![("tag:Name", &["ALPHA"])],
            Vec::new(),
        ),
        (
            "tag:<key> for a key only one resource carries",
            vec![("tag:Owner", &["TeamA"])],
            vec![0],
        ),
        (
            "tag:<key> for a key no resource carries",
            vec![("tag:Phase", &["alpha"])],
            Vec::new(),
        ),
        (
            "tag-key selects every resource carrying that key, whatever its value",
            vec![(TAG_KEY_FILTER, &["Name"])],
            vec![0, 1],
        ),
        (
            "tag-key for a key only one resource carries",
            vec![(TAG_KEY_FILTER, &["Owner"])],
            vec![0],
        ),
        (
            "tag-key for a key no resource carries",
            vec![(TAG_KEY_FILTER, &["Phase"])],
            Vec::new(),
        ),
        (
            "tag-value selects on the value whatever the key is",
            vec![(TAG_VALUE_FILTER, &["TeamA"])],
            vec![0],
        ),
        (
            "tag-value no resource carries",
            vec![(TAG_VALUE_FILTER, &["TeamZ"])],
            Vec::new(),
        ),
        (
            "a tag filter and a documented filter, joined with an AND",
            vec![("tag:Name", &["beta"]), documented],
            vec![1],
        ),
        (
            "a documented filter alone still selects the untagged resource too",
            vec![documented],
            vec![0, 1, 2],
        ),
    ]
}

/// The ids `action` reports for `params`, as the set they are: the rows above assert WHICH
/// resources a filter selects, which is not an assertion about the order they are paged in.
async fn selected(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    id_element: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    let mut ids = elements(&body, id_element);
    ids.sort();
    ids
}

/// The ids of `indexes`, sorted, as [`selected`] answers them.
fn expected_ids(ids: &[String], indexes: &[usize]) -> Vec<String> {
    let mut expected: Vec<String> = indexes.iter().map(|index| ids[*index].clone()).collect();
    expected.sort();
    expected
}

/// `DescribeCapacityReservations` answers the tag filter family over the tags its
/// reservations really hold.
#[tokio::test]
async fn describe_capacity_reservations_answers_the_tag_filter_family() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_reservations(&router).await;

    for (case, entries, indexes) in rows(("instance-type", &["m5.large"])) {
        let params = filters(&entries);
        let reported = selected(
            case,
            &router,
            "DescribeCapacityReservations",
            "capacityReservationId",
            &params,
        )
        .await;
        let expected = expected_ids(&ids, &indexes);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reservations {expected:?}, got {reported:?}"
        );
    }
}

/// `DescribeCapacityReservationFleets` answers the same family over its Fleets' tags.
#[tokio::test]
async fn describe_capacity_reservation_fleets_answers_the_tag_filter_family() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_fleets(&router).await;

    for (case, entries, indexes) in rows(("tenancy", &["default"])) {
        let params = filters(&entries);
        let reported = selected(
            case,
            &router,
            "DescribeCapacityReservationFleets",
            "capacityReservationFleetId",
            &params,
        )
        .await;
        let expected = expected_ids(&ids, &indexes);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the Fleets {expected:?}, got {reported:?}"
        );
    }
}

/// A name that is neither a filter the operation documents nor one of the tag filter family
/// is still refused: a `tag:` prefix with no key names no tag, and neither does a name of
/// the family in the wrong case.
#[tokio::test]
async fn a_name_outside_the_documented_set_and_the_family_is_still_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let _reservations = seed_reservations(&router).await;
    let _fleets = seed_fleets(&router).await;

    let undocumented = [
        "tag:",
        "TAG-KEY",
        "Tag-Key",
        "tag_key",
        "tags",
        "reservation-name",
        "",
    ];
    for action in [
        "DescribeCapacityReservations",
        "DescribeCapacityReservationFleets",
    ] {
        for name in undocumented {
            let case = format!("{action} with the filter {name:?}");
            let (status, body) = api(&router, action, &filters(&[(name, &["anything"])])).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {code:?} in body {body}"
            );
        }
    }
}
