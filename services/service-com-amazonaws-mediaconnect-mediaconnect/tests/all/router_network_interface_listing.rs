// SPDX-License-Identifier: CC0-1.0

//! ListRouterNetworkInterfaces, end to end over HTTP: `POST /v1/routerNetworkInterfaces`.
//!
//! The subject is the rule ListRouterNetworkInterfaces follows: it "retrieves a list of
//! router network interfaces", a page at a time in the store's own order, each entry the
//! STORED interface record projected into the modelled `ListedRouterNetworkInterface` summary
//! (`smithy_query(query="shape",shape="ListedRouterNetworkInterface")`), narrowed to the
//! interfaces that satisfy EVERY element of `Filters` — an element names one attribute of an
//! interface and the values accepted for it. Nothing is fixed in the handler: every member
//! asserted below is compared against what the store holds, and an interface the store does
//! not hold is not listed.
//!
//! The rows therefore vary what that rule generalises over: which
//! `RouterNetworkInterfaceConfiguration` variant (and so which network interface type) an
//! interface carries, its Region, the form of its name, how many router inputs and outputs
//! name it (the two association counts), which member of the `RouterNetworkInterfaceFilter`
//! union a request filters on (every one the model declares, including several at once and a
//! value nothing matches), the page size across the modelled `MaxResults` range and its
//! absence, and the form of the pagination token.
//!
//! Every call goes over the wire through the same router the emulator's front door uses, on
//! the harness `flow_lifecycle` owns, so what these tests assert is what a MediaConnect
//! client sees: routing, restJson1 deserialization, constraint validation, handler,
//! serialization. The create helper and the two configuration fixtures are the ones
//! `router_network_interfaces` owns, so the router network interface suites cannot drift
//! apart.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    PublicRouterNetworkInterfaceConfiguration, RouterNetworkInterfaceConfiguration,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{configuration_rows, create_body, create_router_input};
use crate::router_network_interfaces::{
    create_network_interface, public_configuration, vpc_configuration,
};
use crate::router_outputs::{create_router_output, minted_flow_source, standard};

/// The Region the fixture's cross-Region interfaces are created in — a Region other than the
/// one every other fixture lands in ([`REGION`]), so a `RegionNames` filter has something to
/// separate.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough for a transition the clock owes to be due: twice `mc::TRANSITION_SPAN`, so a
/// settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Send one ListRouterNetworkInterfaces request: `POST /v1/routerNetworkInterfaces`, with
/// `MaxResults` and `NextToken` bound to the query string and `Filters` carried in the body
/// (`smithy_query(query="operation",operation="ListRouterNetworkInterfaces")`).
async fn list_interfaces(
    emulator: &Emulator,
    query: &str,
    filters: Option<Value>,
) -> (u16, Option<String>, Value) {
    let body = filters.map(|filters| json!({ "filters": filters }));
    emulator
        .send(
            "POST",
            &format!("/v1/routerNetworkInterfaces{query}"),
            body,
        )
        .await
}

/// A 200 listing, asserted to be answered at all.
async fn page(emulator: &Emulator, case: &str, query: &str, filters: Option<Value>) -> Value {
    let (status, code, body) = list_interfaces(emulator, query, filters.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing {query:?} with {filters:?} expected 200, got {status} {code:?}: \
         {body}"
    );
    body
}

/// The `RouterNetworkInterfaces` member of a ListRouterNetworkInterfaces response.
fn listed(body: &Value) -> Vec<Value> {
    body["routerNetworkInterfaces"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARNs of the `RouterNetworkInterfaces` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|summary| summary["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every router network interface ARN the store holds, in the order the store holds them —
/// the order a listing must report, derived from the store rather than written out here.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store.router_network_interfaces.keys().cloned().collect()
}

/// The whole router network interface map as the store holds it, for comparing a before and
/// an after: a listing must leave every record exactly as it was.
fn stored_interfaces(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.router_network_interfaces)
}

/// What the STORED record holds for the members a summary reports, so the response is
/// compared against the persisted record rather than against a literal.
struct StoredSummary {
    name: String,
    id: String,
    network_interface_type: String,
    state: String,
    region_name: String,
    associated_input_count: i32,
    associated_output_count: i32,
    created_at: SystemTime,
    updated_at: SystemTime,
}

/// Read [`StoredSummary`] for `arn` straight out of the store, with the two association
/// counts counted from the router inputs and outputs that name the interface — where the
/// relation lives.
fn stored_summary(emulator: &Emulator, arn: &str) -> StoredSummary {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    let interface = store
        .router_network_interfaces
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture router network interface {arn} is stored"));
    StoredSummary {
        name: interface.name.clone(),
        id: interface.id.clone(),
        network_interface_type: interface.network_interface_type().to_string(),
        state: interface.state.clone(),
        region_name: interface.region_name.clone(),
        associated_input_count: store.associated_router_input_count(arn),
        associated_output_count: store.associated_router_output_count(arn),
        created_at: interface.created_at,
        updated_at: interface.updated_at,
    }
}

/// Create one router network interface over the wire and answer the ARN the service minted
/// for it.
async fn minted_interface(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, code, created) = create_network_interface(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterNetworkInterface expected 201, got {status} {code:?}: \
         {created}"
    );
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        mc::is_mintable_router_network_interface_arn(&arn),
        "case {case:?}: the create mints a RouterNetworkInterfaceArn, got {arn:?}"
    );
    arn
}

/// One router network interface of the fixture store, and the attributes a filter selects on.
struct Fixture {
    case: &'static str,
    arn: String,
    network_interface_type: &'static str,
    region_name: String,
    name: &'static str,
}

/// The fixture store every filtering case runs against: one interface per
/// `RouterNetworkInterfaceConfiguration` member the model declares (`Public`, `Vpc`), each in
/// both Regions, and names of different forms, so no filter can pass by matching everything.
async fn fixtures(emulator: &Emulator) -> Vec<Fixture> {
    let public_here = minted_interface(
        emulator,
        "public/here",
        json!({
            "name": "rni-listing-public-here",
            "configuration": public_configuration(1),
            "regionName": REGION,
        }),
    )
    .await;
    let public_there = minted_interface(
        emulator,
        "public/there",
        json!({
            "name": "rni-listing-public-there",
            "configuration": public_configuration(0),
            "regionName": OTHER_REGION,
        }),
    )
    .await;
    let vpc_here = minted_interface(
        emulator,
        "vpc/here",
        json!({
            "name": "rni_listing-vpc-here",
            "configuration": vpc_configuration(1),
            "tags": { "env": "test" },
        }),
    )
    .await;
    let vpc_there = minted_interface(
        emulator,
        "vpc/there",
        json!({
            "name": "RNI0listing",
            "configuration": vpc_configuration(5),
            "regionName": OTHER_REGION,
        }),
    )
    .await;

    vec![
        Fixture {
            case: "public/here",
            arn: public_here,
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            region_name: REGION.to_string(),
            name: "rni-listing-public-here",
        },
        Fixture {
            case: "public/there",
            arn: public_there,
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            region_name: OTHER_REGION.to_string(),
            name: "rni-listing-public-there",
        },
        Fixture {
            case: "vpc/here",
            arn: vpc_here,
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            region_name: REGION.to_string(),
            name: "rni_listing-vpc-here",
        },
        Fixture {
            case: "vpc/there",
            arn: vpc_there,
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            region_name: OTHER_REGION.to_string(),
            name: "RNI0listing",
        },
    ]
}

/// The ARNs of the fixtures `cases` names, in the order the STORE holds them — which is the
/// order a listing must report them in.
fn expected_arns(fixtures: &[Fixture], cases: &[&str]) -> Vec<String> {
    let mut arns: Vec<String> = fixtures
        .iter()
        .filter(|fixture| cases.contains(&fixture.case))
        .map(|fixture| fixture.arn.clone())
        .collect();
    arns.sort();
    arns
}

/// Create `inputs` router inputs and `outputs` router outputs that name the interface `arn`,
/// so the two association counts a summary reports have something to count.
async fn associate(emulator: &Emulator, arn: &str, prefix: &str, inputs: usize, outputs: usize) {
    let (flow_arn, _) = minted_flow_source(emulator, &format!("flow-{prefix}")).await;
    let rows = configuration_rows(arn, &flow_arn);
    let (_, _, _, standard_input_configuration, _) = rows
        .into_iter()
        .next()
        .expect("the router input configuration rows are not empty");
    for index in 0..inputs {
        let (status, code, created) = create_router_input(
            emulator,
            create_body(
                &format!("ri-{prefix}-{index}"),
                standard_input_configuration.clone(),
            ),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "the associated router input {index} is created, got {status} {code:?}: {created}"
        );
    }
    for index in 0..outputs {
        let (status, code, created) = create_router_output(
            emulator,
            json!({
                "name": format!("ro-{prefix}-{index}"),
                "configuration": standard(arn, json!({
                    "Rtp": {
                        "destinationAddress": "203.0.113.50",
                        "destinationPort": 5004 + index,
                    }
                })),
                "maximumBitrate": 20_000_000_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "the associated router output {index} is created, got {status} {code:?}: {created}"
        );
    }
}

/// The members a `ListedRouterNetworkInterface` declares
/// (`smithy_query(query="shape",shape="ListedRouterNetworkInterface")`), by their wire names:
/// all ten are required.
const REQUIRED_MEMBERS: [&str; 10] = [
    "name",
    "arn",
    "id",
    "networkInterfaceType",
    "associatedOutputCount",
    "associatedInputCount",
    "state",
    "regionName",
    "createdAt",
    "updatedAt",
];

/// The members a `RouterNetworkInterface` carries that its summary does NOT declare, so a
/// summary reporting one of them would be reporting a member the model does not put there.
const MEMBERS_OF_THE_FULL_RESOURCE: [&str; 2] = ["configuration", "tags"];

/// The listing reports every stored router network interface, projected from its own record,
/// whatever kind of interface it is — and it stores nothing.
#[tokio::test]
async fn every_stored_interface_is_listed_from_its_own_record() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;
    // Two router inputs and one router output name the first fixture, so the two association
    // counts a summary reports differ from each other and from the zero every other fixture
    // carries.
    associate(&emulator, &fixtures[0].arn, "rni-listing", 2, 1).await;

    // Past the span the clock owes a new interface, so every record is settled and a listing
    // is free of state the clock is still moving.
    tokio::time::sleep(past_transition_span()).await;

    let body = page(&emulator, "every kind at once", "?maxResults=100", None).await;
    // Snapshotted AFTER that first listing: materialising a state change the clock had
    // already made due is the one write a listing performs, and that span has now passed, so
    // from here on a further listing must leave every record exactly as it is.
    let before = stored_interfaces(&emulator);
    let expected = stored_arns(&emulator);
    assert_eq!(
        expected.len(),
        fixtures.len(),
        "every fixture interface must be stored, got {expected:?}"
    );
    assert_eq!(
        listed_arns(&body),
        expected,
        "every stored router network interface is listed, in the store's own order, got {body}"
    );

    for fixture in &fixtures {
        let case = fixture.case;
        let stored = stored_summary(&emulator, &fixture.arn);
        let summary = listed(&body)
            .into_iter()
            .find(|summary| summary["arn"].as_str() == Some(fixture.arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the stored interface is listed: {body}"));

        assert_eq!(
            (
                summary["name"].as_str(),
                summary["id"].as_str(),
                summary["networkInterfaceType"].as_str(),
                summary["state"].as_str(),
                summary["regionName"].as_str(),
                summary["associatedInputCount"].as_i64(),
                summary["associatedOutputCount"].as_i64(),
            ),
            (
                Some(stored.name.as_str()),
                Some(stored.id.as_str()),
                Some(stored.network_interface_type.as_str()),
                Some(stored.state.as_str()),
                Some(stored.region_name.as_str()),
                Some(i64::from(stored.associated_input_count)),
                Some(i64::from(stored.associated_output_count)),
            ),
            "case {case:?}: the summary is the stored record, got {summary}"
        );
        // The attributes the fixture asked for, so a projection reading the wrong record's
        // field could not pass by being self-consistent.
        assert_eq!(
            (
                summary["name"].as_str(),
                summary["networkInterfaceType"].as_str(),
                summary["regionName"].as_str(),
            ),
            (
                Some(fixture.name),
                Some(fixture.network_interface_type),
                Some(fixture.region_name.as_str()),
            ),
            "case {case:?}: the summary reports what the create asked for, got {summary}"
        );
        // The counts are the resources that NAME this interface: two inputs and one output
        // for the associated fixture, none for the others.
        let expected_counts = if fixture.case == fixtures[0].case {
            (Some(2_i64), Some(1_i64))
        } else {
            (Some(0_i64), Some(0_i64))
        };
        assert_eq!(
            (
                summary["associatedInputCount"].as_i64(),
                summary["associatedOutputCount"].as_i64(),
            ),
            expected_counts,
            "case {case:?}: the counts are the inputs and outputs naming the interface, got \
             {summary}"
        );
        for member in REQUIRED_MEMBERS {
            assert!(
                !summary[member].is_null(),
                "case {case:?}: the required member {member} is reported, got {summary}"
            );
        }
        for member in MEMBERS_OF_THE_FULL_RESOURCE {
            assert!(
                summary.get(member).is_none(),
                "case {case:?}: a summary does not carry {member}, got {summary}"
            );
        }
        // The timestamps are the record's own, reported in the form this emulator's codec
        // renders every timestamp in (epoch seconds), so they are compared as instants.
        for (member, instant) in [
            ("createdAt", stored.created_at),
            ("updatedAt", stored.updated_at),
        ] {
            let reported = summary[member]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: {member} is a timestamp: {summary}"));
            let expected = instant
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|since| since.as_secs_f64())
                .unwrap_or_default();
            assert!(
                (reported - expected).abs() < 1.0,
                "case {case:?}: {member} is the instant the record holds, expected {expected}, \
                 got {reported}"
            );
        }
    }

    // A read does not change what a later caller sees: the second listing is the same
    // document and the store is untouched.
    let again = page(&emulator, "repeated listing", "?maxResults=100", None).await;
    assert_eq!(again, body, "listing twice reports the same page, got {again}");
    let after = stored_interfaces(&emulator);
    assert_eq!(
        after, before,
        "listing router network interfaces must store nothing; expected {before}, got {after}"
    );
}

/// One filtering case: the fixtures the `Filters` of a request must report.
struct FilterRow {
    case: &'static str,
    /// Which fixture [`Fixture::case`] names the filter must select, in any order: the
    /// expectation is sorted into the store's order before it is compared.
    expected: &'static [&'static str],
}

/// `Filters` narrows the listing to the interfaces that satisfy EVERY element, and a request
/// carrying none lists them all.
///
/// The rows span the `RouterNetworkInterfaceFilter` union: every member the model declares, a
/// member naming several accepted values, several members at once, a value no stored interface
/// carries, and a member naming no value at all.
#[tokio::test]
async fn filters_report_only_the_interfaces_that_satisfy_them() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;

    tokio::time::sleep(past_transition_span()).await;
    // One listing materialises the state changes the clock has already made due — its only
    // write — so the snapshot every filtered listing below is compared against is taken after
    // it.
    page(&emulator, "settling listing", "?maxResults=100", None).await;
    let before = stored_interfaces(&emulator);

    let cases: Vec<(FilterRow, Value)> = vec![
        (
            FilterRow {
                case: "no filter at all",
                expected: &["public/here", "public/there", "vpc/here", "vpc/there"],
            },
            json!([]),
        ),
        (
            FilterRow {
                case: "RegionNames naming the cross-Region interfaces' Region",
                expected: &["public/there", "vpc/there"],
            },
            json!([{ "RegionNames": [OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "RegionNames naming both Regions",
                expected: &["public/here", "public/there", "vpc/here", "vpc/there"],
            },
            json!([{ "RegionNames": [REGION, OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceTypes naming PUBLIC",
                expected: &["public/here", "public/there"],
            },
            json!([{ "NetworkInterfaceTypes": [mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceTypes naming VPC",
                expected: &["vpc/here", "vpc/there"],
            },
            json!([{ "NetworkInterfaceTypes": [mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceTypes naming both types",
                expected: &["public/here", "public/there", "vpc/here", "vpc/there"],
            },
            json!([{ "NetworkInterfaceTypes": [
                mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
                mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            ] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment one name carries",
                expected: &["vpc/here"],
            },
            json!([{ "NameContains": ["rni_listing"] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment three names carry",
                expected: &["public/here", "public/there", "vpc/here"],
            },
            json!([{ "NameContains": ["rni"] }]),
        ),
        (
            FilterRow {
                case: "NameContains two fragments",
                expected: &["vpc/here", "vpc/there"],
            },
            json!([{ "NameContains": ["rni_listing", "RNI0"] }]),
        ),
        (
            FilterRow {
                case: "NameContains is case sensitive",
                expected: &[],
            },
            json!([{ "NameContains": ["rni0listing"] }]),
        ),
        (
            FilterRow {
                case: "two members at once, both satisfied by one interface",
                expected: &["vpc/there"],
            },
            json!([
                { "RegionNames": [OTHER_REGION] },
                { "NetworkInterfaceTypes": [mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC] },
            ]),
        ),
        (
            FilterRow {
                case: "three members at once, satisfied by one interface",
                expected: &["public/here"],
            },
            json!([
                { "RegionNames": [REGION] },
                { "NetworkInterfaceTypes": [mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC] },
                { "NameContains": ["public"] },
            ]),
        ),
        (
            FilterRow {
                case: "a Region no stored interface is in",
                expected: &[],
            },
            json!([{ "RegionNames": ["ap-south-1"] }]),
        ),
        (
            FilterRow {
                case: "a member naming no value at all",
                expected: &[],
            },
            json!([{ "NetworkInterfaceTypes": [] }]),
        ),
    ];

    for (FilterRow { case, expected }, filters) in cases {
        let expected = expected_arns(&fixtures, expected);
        let body = page(&emulator, case, "?maxResults=100", Some(filters.clone())).await;
        assert_eq!(
            listed_arns(&body),
            expected,
            "case {case:?}: the filter {filters} reports exactly the matching interfaces, got \
             {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: a page holding every match carries no token, got {body}"
        );
    }

    // Filtering is a read like any other: it stores nothing.
    let after = stored_interfaces(&emulator);
    assert_eq!(
        after, before,
        "a filtered listing must store nothing; expected {before}, got {after}"
    );
}

/// Put `count` router network interfaces straight into the store, the way
/// CreateRouterNetworkInterface stores one: the ARN is minted through the crate's own
/// [`mc::router_network_interface_resource_fragment`] and the record is live and settled, like
/// one whose creation the clock has finished.
fn seed_interfaces(emulator: &Emulator, count: usize) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    for _ in 0..count {
        let suffix = store.next_suffix();
        let id = mc::router_resource_id(suffix);
        let arn = engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            REGION,
            &mc::router_network_interface_resource_fragment(&id),
        );
        let now = SystemTime::now();
        store.router_network_interfaces.insert(
            arn.clone(),
            mc::StoredRouterNetworkInterface {
                arn: arn.clone(),
                id,
                name: format!("seeded-router-network-interface-{suffix}"),
                configuration: RouterNetworkInterfaceConfiguration::Public(
                    PublicRouterNetworkInterfaceConfiguration {
                        allow_rules: Vec::new(),
                    },
                ),
                region_name: REGION.to_string(),
                tags: BTreeMap::new(),
                state: mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE.to_string(),
                pending: None,
                client_token: None,
                created_at: now,
                updated_at: now,
                removal_due_at: None,
            },
        );
    }
}

/// The listing pages every matching interface exactly once, whatever `MaxResults` is, and the
/// token is the ARN the next page resumes at. With no `MaxResults` the page holds as many as
/// the modelled range allows.
#[tokio::test]
async fn pages_partition_every_interface_exactly_once() {
    let emulator = Emulator::new();
    // More than the largest page the modelled `MaxResults` range allows, so the page a
    // request naming no size gets is observably bounded. The store ceiling is a bound on what
    // is STORED, so these records go in through the store's own channel; the calls under test
    // still go over the wire.
    seed_interfaces(&emulator, 130);
    let all = stored_arns(&emulator);
    assert_eq!(
        all.len(),
        130,
        "every seeded interface is stored, got {all:?}"
    );

    // `MaxResults` is "the maximum number of router network interfaces to return in the
    // response" and the model documents no smaller default, so a request naming none is
    // answered with the maximum of the modelled range (1..100).
    let default_page = page(&emulator, "the default page size", "", None).await;
    assert_eq!(
        listed_arns(&default_page),
        all[..100].to_vec(),
        "the default page holds the first hundred interfaces, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(all[100].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled range: its lower bound, sizes between, and its upper
    // bound — each walked with no filter and under one, because filtering and paging compose.
    // The filter keeps every seeded interface, so the two walks must agree.
    let filter = json!([{ "NameContains": ["seeded-router-network-interface-"] }]);
    for page_size in [1_usize, 7, 100] {
        for filters in [None, Some(filter.clone())] {
            let case = format!("page size {page_size}, filters {filters:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut query = format!("?maxResults={page_size}");
                if let Some(token) = &token {
                    query.push_str(&format!("&nextToken={token}"));
                }
                let body = page(&emulator, &case, &query, filters.clone()).await;
                let listed_page = listed_arns(&body);
                token = body["nextToken"].as_str().map(ToString::to_string);
                if token.is_some() {
                    assert_eq!(
                        listed_page.len(),
                        page_size,
                        "case {case:?}: a page that carries a token is full, got {body}"
                    );
                }
                seen.extend(listed_page);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, all,
                "case {case:?}: the pages must partition the stored interfaces once, in order"
            );
        }
    }

    // The token is the ARN the listing RESUMES AT, so a page resumed from it starts there.
    let first = page(&emulator, "resuming", "?maxResults=3", None).await;
    let token = first["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let resumed = page(
        &emulator,
        "resuming",
        &format!("?maxResults=100&nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        listed_arns(&resumed),
        all[3..103].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// An account that holds no router network interface is listed empty — not a 404 and not a
/// page of invented interfaces.
#[tokio::test]
async fn an_account_that_holds_no_interface_is_listed_empty() {
    let emulator = Emulator::new();
    for (case, filters) in [
        ("no filter", None),
        (
            "a filter",
            Some(json!([{ "NetworkInterfaceTypes": [mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC] }])),
        ),
    ] {
        let body = page(&emulator, case, "", filters).await;
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: an account holding no interface lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );
    }
}

/// The listed `State` is the CURRENT one: a listing materialises only the change the clock
/// already made due, so an interface whose creation is still in flight is listed `CREATING`,
/// the transition span settles it into `ACTIVE`, a delete puts it in `DELETING`, and once the
/// clock completes that delete the listing stops reporting it.
#[tokio::test]
async fn the_listed_state_is_the_one_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let arn = minted_interface(
        &emulator,
        "clock",
        json!({
            "name": "rni-listing-clock",
            "configuration": public_configuration(1),
        }),
    )
    .await;

    let body = page(&emulator, "inside the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING),
        "inside the span the creation is still in flight, got {body}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "past the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
        "once the span has passed the creation is settled, got {body}"
    );

    let (status, code, deleting) = emulator
        .send("DELETE", &format!("/v1/routerNetworkInterface/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DeleteRouterNetworkInterface accepts the delete, got {status} {code:?}: {deleting}"
    );
    let body = page(&emulator, "a delete in flight", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING),
        "an interface on its way out is listed DELETING, got {body}"
    );

    // A deleted interface leaves the store once the clock completes the delete, and the
    // listing stops reporting it rather than reporting a record nothing holds.
    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "a completed delete", "", None).await;
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "an interface the store no longer holds is not listed, got {body}"
    );
}

/// One refused ListRouterNetworkInterfaces request.
struct RefusalCase {
    case: &'static str,
    query: &'static str,
}

/// ListRouterNetworkInterfaces refuses a page size outside the modelled range and a
/// pagination token it could not have issued — with the modelled `BadRequestException` (400),
/// never with a 501 and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    // The modelled `MaxResults` range is 1..100, so each bound is usable (the paging suite
    // above lists with 1 and with 100) and each value past it is refused. A `NextToken` is the
    // router network interface ARN the listing resumes at, so a token the service could not
    // have minted is refused; the rows are the ways a token can fail that form.
    const CASES: &[RefusalCase] = &[
        RefusalCase {
            case: "MaxResults below the modelled range",
            query: "?maxResults=0",
        },
        RefusalCase {
            case: "MaxResults above the modelled range",
            query: "?maxResults=101",
        },
        RefusalCase {
            case: "a nextToken that is not an ARN",
            query: "?nextToken=not-a-token",
        },
        RefusalCase {
            case: "a nextToken from another service",
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:routerNetworkInterface:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose account id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:routerNetworkInterface:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose interface id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:a1b2c3d4e5f",
        },
        RefusalCase {
            case: "a nextToken whose interface id carries an upper-case character",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:A1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose interface id is empty",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:",
        },
    ];

    for RefusalCase { case, query } in CASES {
        let (status, code, body) = list_interfaces(&emulator, query, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
    }

    // `Filters` is bounded as well: every stored interface is tested against every filter and
    // every value one names, so a request past either ceiling is refused rather than answered.
    // Each ceiling is asserted usable at its bound and refused one past it.
    let region_values = |count: usize| -> Value {
        (0..count)
            .map(|index| Value::String(format!("us-east-{index}")))
            .collect()
    };
    let filters_of = |count: usize, values: usize| -> Value {
        (0..count)
            .map(|_| json!({ "RegionNames": region_values(values) }))
            .collect()
    };
    for (case, filters, expected_status) in [
        ("as many filters as the ceiling allows", filters_of(10, 1), 200),
        ("one filter past the ceiling", filters_of(11, 1), 400),
        (
            "as many values as the ceiling allows",
            filters_of(1, 100),
            200,
        ),
        ("one value past the ceiling", filters_of(1, 101), 400),
    ] {
        let (status, code, body) = list_interfaces(&emulator, "", Some(filters)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {code:?}: {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                code.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: the refusal names the modelled error, got {code:?}: {body}"
            );
            assert!(
                body["message"].as_str().is_some_and(|m| !m.is_empty()),
                "case {case:?}: the refusal carries a message, got {body}"
            );
        }
    }

    // A token the service COULD have minted is accepted even when no such interface is
    // stored: paging must survive a delete between pages.
    let unknown =
        "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:ffffffffffff";
    let body = page(
        &emulator,
        "a well-formed token for an unknown interface",
        &format!("?nextToken={unknown}"),
        None,
    )
    .await;
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "nothing is stored past that key, got {body}"
    );
}
