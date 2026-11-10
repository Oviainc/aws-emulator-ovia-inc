// SPDX-License-Identifier: CC0-1.0

//! `EnableAvailabilityZonesForLoadBalancer`: the zones a load balancer is in, named
//! directly rather than through a subnet.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * The operation adds the requested zones to the set the load balancer is in and answers
//!   "the updated list of Availability Zones for the load balancer" — the WHOLE stored set,
//!   which a later `DescribeLoadBalancers` then reports: a zone it is not in yet joins the
//!   set, a zone it is already in re-states the same membership, and a zone named twice in
//!   one request is enabled once.
//! * A name the request's region holds no load balancer for earns the operation's one
//!   modelled failure (`LoadBalancerNotFound`, HTTP 400), and a request that names no zone,
//!   or more zones than a load balancer can be in, is refused under the code this service
//!   answers values it cannot accept with. Each refusal leaves the stored set exactly as it
//!   was.
//! * Which VPC a load balancer is in is EC2's to say, and this crate mounts no EC2, so
//!   nothing is known here to refuse the call on: the zone-enable of a load balancer placed
//!   in subnets is accepted. The other side of that rule — a load balancer EC2 reports in a
//!   VPC of the account's own, which this operation does not apply to — needs both services
//!   on one engine and is pinned in
//!   `core/router/tests/all/elb_classic_zone_enable_turns_on_the_vpc.rs`.

use super::wire::{call, emulator, error_message, find, member_texts, param, parse, Wire};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The error code this service answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The number of placements one load balancer's stored set is bounded by
/// (`state::MAX_PLACEMENTS_PER_LOAD_BALANCER`), which the over-bound refusals below are
/// stated over.
const MAX_PLACEMENTS: usize = 50;

/// Create `name` with one HTTP 80 listener and the placement in `extra`.
async fn create(router: &MultiServiceRouter, name: &str, extra: &[(String, String)]) -> Wire {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "80"),
    ];
    params.extend_from_slice(extra);
    let created = call(router, REGION, "CreateLoadBalancer", &params).await;
    assert_eq!(
        created.status, 200,
        "the fixture {name} must be created, got {} with {}",
        created.status, created.body
    );
    created
}

/// `EnableAvailabilityZonesForLoadBalancer` with one `AvailabilityZones` member per entry
/// of `zones` — a zone repeated in the list is sent twice, which is what the
/// deduplication row drives.
fn enable_params(name: &str, zones: &[&str]) -> Vec<(String, String)> {
    let mut params = vec![param("LoadBalancerName", name)];
    for (index, zone) in zones.iter().enumerate() {
        params.push(param(
            &format!("AvailabilityZones.member.{}", index + 1),
            zone,
        ));
    }
    params
}

/// The `AvailabilityZones` a success envelope answers, in document order.
fn answered_zones(answer: &Wire) -> Vec<String> {
    let doc = parse(&answer.body);
    member_texts(answer.result(&doc), "AvailabilityZones")
}

/// The `AvailabilityZones` a later `DescribeLoadBalancers` reports for `name`.
async fn described_zones(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let described = call(
        router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    assert_eq!(
        described.status, 200,
        "the load balancer {name} must be describable, got {}",
        described.body
    );
    let doc = parse(&described.body);
    let description = find(described.result(&doc), "LoadBalancerDescriptions")
        .and_then(|list| list.children().next())
        .unwrap_or_else(|| panic!("no description in {}", described.body));
    member_texts(description, "AvailabilityZones")
}

fn owned(zones: &[&str]) -> Vec<String> {
    zones.iter().map(|zone| (*zone).to_string()).collect()
}

/// The zone set FOLLOWS the requests that enable zones, and the answer is the whole stored
/// set — which is what the next read reports.
///
/// The rows walk one load balancer, differing along what the rule generalises over: how
/// many zones one request names, whether the load balancer is already in them, and a zone
/// named twice in the same request.
#[tokio::test]
async fn enabling_zones_adds_them_to_the_stored_set() {
    let router = emulator();
    let name = "zone-enable-elb";
    create(
        &router,
        name,
        &[param("AvailabilityZones.member.1", "us-west-2a")],
    )
    .await;

    // One row: the zones the request names, and the set the load balancer is then in.
    struct Row<'a> {
        case: &'a str,
        requested: Vec<&'a str>,
        expected: Vec<&'a str>,
    }
    let rows: [Row<'_>; 6] = [
        Row {
            case: "a zone the load balancer is not in yet",
            requested: vec!["us-west-2b"],
            expected: vec!["us-west-2a", "us-west-2b"],
        },
        Row {
            case: "the same zone again re-states the same membership",
            requested: vec!["us-west-2b"],
            expected: vec!["us-west-2a", "us-west-2b"],
        },
        Row {
            case: "a zone it is already in named beside one it is not",
            requested: vec!["us-west-2b", "us-west-2c"],
            expected: vec!["us-west-2a", "us-west-2b", "us-west-2c"],
        },
        Row {
            case: "one zone named twice in one request is enabled once",
            requested: vec!["us-west-2d", "us-west-2d"],
            expected: vec!["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"],
        },
        Row {
            case: "every zone of the request already held",
            requested: vec!["us-west-2a", "us-west-2d"],
            expected: vec!["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"],
        },
        Row {
            case: "three new zones in one request",
            requested: vec!["us-west-2e", "us-west-2f", "us-west-2g"],
            expected: vec![
                "us-west-2a",
                "us-west-2b",
                "us-west-2c",
                "us-west-2d",
                "us-west-2e",
                "us-west-2f",
                "us-west-2g",
            ],
        },
    ];

    for Row {
        case,
        requested,
        expected,
    } in rows
    {
        let answered = call(
            &router,
            REGION,
            "EnableAvailabilityZonesForLoadBalancer",
            &enable_params(name, &requested),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: the zone-enable must be accepted, got {} with {}",
            answered.status, answered.body
        );
        let answered_set = answered_zones(&answered);
        assert_eq!(
            answered_set,
            owned(&expected),
            "case {case:?}: expected the answer to carry the stored set {:?}, got \
             {answered_set:?}",
            owned(&expected)
        );
        let reported = described_zones(&router, name).await;
        assert_eq!(
            reported,
            owned(&expected),
            "case {case:?}: expected a later describe to report {:?}, got {reported:?}",
            owned(&expected)
        );
    }
}

/// Each refusal the operation answers, and the stored set each of them leaves alone.
#[tokio::test]
async fn a_refused_zone_enable_changes_nothing() {
    let router = emulator();
    let name = "zone-enable-refusal-elb";
    create(
        &router,
        name,
        &[param("AvailabilityZones.member.1", "us-west-2a")],
    )
    .await;

    // One row: the load balancer the request names, the zones it names, and the status and
    // code it must be refused with.
    struct Row<'a> {
        case: &'a str,
        named: &'a str,
        zones: Vec<&'a str>,
        status: u16,
        code: &'a str,
    }
    let too_many: Vec<String> = (0..=MAX_PLACEMENTS)
        .map(|index| format!("us-west-2-{index}"))
        .collect();
    let rows: Vec<Row<'_>> = vec![
        Row {
            case: "a name the region holds no load balancer for",
            named: "no-such-classic-elb",
            zones: vec!["us-west-2b"],
            status: 400,
            code: "LoadBalancerNotFound",
        },
        Row {
            // `AvailabilityZones` is a required member, so a request carrying none of it
            // is refused by the boundary validator — before the handler, under the code
            // the framework answers a request that does not satisfy the MODEL with.
            case: "a request that names no zone at all",
            named: name,
            zones: vec![],
            status: 400,
            code: "MalformedInput",
        },
        Row {
            case: "one zone more than a load balancer can be in",
            named: name,
            zones: too_many.iter().map(String::as_str).collect(),
            status: 400,
            code: VALIDATION_ERROR,
        },
    ];

    for Row {
        case,
        named,
        zones,
        status,
        code,
    } in rows
    {
        let answered = call(
            &router,
            REGION,
            "EnableAvailabilityZonesForLoadBalancer",
            &enable_params(named, &zones),
        )
        .await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {} with {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert!(
            !error_message(&answered).is_empty(),
            "case {case:?}: a refusal must state why, got {}",
            answered.body
        );
        let reported = described_zones(&router, name).await;
        assert_eq!(
            reported,
            owned(&["us-west-2a"]),
            "case {case:?}: a refused request must leave the stored set alone, got \
             {reported:?}"
        );
    }

    // The name the first row asked about is still held by nothing: a refusal invents no
    // record.
    let described = call(
        &router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", "no-such-classic-elb")],
    )
    .await;
    assert_eq!(
        (described.status, described.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a refused zone-enable must leave no record behind, got {}",
        described.body
    );
}

/// With no EC2 to ask, this service learned nothing about the load balancer's network, so
/// nothing is refused on that silence: the zone-enable of a load balancer placed in subnets
/// is accepted and the zone joins the stored set.
#[tokio::test]
async fn nothing_is_refused_where_there_was_no_ec2_to_ask() {
    let router = emulator();
    let name = "zone-enable-silent-network-elb";
    create(
        &router,
        name,
        &[param("Subnets.member.1", "subnet-0f1e2d3c4b5a69780")],
    )
    .await;

    let answered = call(
        &router,
        REGION,
        "EnableAvailabilityZonesForLoadBalancer",
        &enable_params(name, &["us-west-2b"]),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "nothing is refused on silence, got {} with {}",
        answered.status, answered.body
    );
    let reported = described_zones(&router, name).await;
    assert_eq!(
        reported,
        owned(&["us-west-2b"]),
        "the enabled zone must join the stored set, got {reported:?}"
    );
}
