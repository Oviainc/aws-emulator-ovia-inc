// SPDX-License-Identifier: CC0-1.0

//! ListRouterInputs, end to end over HTTP: `POST /v1/routerInputs`.
//!
//! The subject is the rule ListRouterInputs follows: it "retrieves a list of router inputs",
//! a page at a time in the store's own order, each entry the STORED router input record
//! projected into the modelled `ListedRouterInput` summary
//! (`smithy_query(query="shape",shape="ListedRouterInput")`), narrowed to the inputs that
//! satisfy EVERY element of `Filters` — an element names one attribute of a router input and
//! the values accepted for it. Nothing is fixed in the handler: every member asserted below
//! is compared against what the store holds, and an input the store does not hold is not
//! listed.
//!
//! The rows therefore vary what that rule generalises over: which `RouterInputConfiguration`
//! variant (and so which input type and whether there is a network interface) an input
//! carries, which interface it ingests on, its Region, routing scope and name, how many
//! router outputs are routed to it, whether a maintenance window has been scheduled for it,
//! which member of the `RouterInputFilter` union a request filters on (every one the model
//! declares, including several at once and a value nothing matches), the page size across the
//! modelled `MaxResults` range and its absence, and the form of the pagination token.
//!
//! Every call goes over the wire through the same router the emulator's front door uses, on
//! the harness `flow_lifecycle` owns, so what these tests assert is what a MediaConnect
//! client sees: routing, restJson1 deserialization, constraint validation, handler,
//! serialization. The create helpers, the configuration table and the store-seeding helper
//! are the ones the other router input suites own, so the router input suites cannot drift
//! apart.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{configuration_rows, create_body, create_router_input};
use crate::router_network_interfaces::{create_network_interface, public_configuration};
use crate::router_outputs::{
    create_router_output, minted_flow_source, minted_network_interface, standard_with_protocol,
};

/// The Region the fixture's cross-Region input is created in — a Region other than the one
/// every other fixture lands in ([`REGION`]), so a `RegionNames` filter has something to
/// separate.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough for a transition the clock owes to be due: twice `mc::TRANSITION_SPAN`, so a
/// settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Send one ListRouterInputs request: `POST /v1/routerInputs`, with `MaxResults` and
/// `NextToken` bound to the query string and `Filters` carried in the body
/// (`smithy_query(query="operation",operation="ListRouterInputs")`).
async fn list_router_inputs(
    emulator: &Emulator,
    query: &str,
    filters: Option<Value>,
) -> (u16, Option<String>, Value) {
    let body = filters.map(|filters| json!({ "filters": filters }));
    emulator
        .send("POST", &format!("/v1/routerInputs{query}"), body)
        .await
}

/// A 200 listing, asserted to be answered at all.
async fn page(emulator: &Emulator, case: &str, query: &str, filters: Option<Value>) -> Value {
    let (status, code, body) = list_router_inputs(emulator, query, filters.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing {query:?} with {filters:?} expected 200, got {status} {code:?}: \
         {body}"
    );
    body
}

/// The `RouterInputs` member of a ListRouterInputs response.
fn listed(body: &Value) -> Vec<Value> {
    body["routerInputs"].as_array().cloned().unwrap_or_default()
}

/// The ARNs of the `RouterInputs` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|summary| summary["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every router input ARN the store holds, in the order the store holds them — the order a
/// listing must report, derived from the store rather than written out here.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store.router_inputs.keys().cloned().collect()
}

/// The whole router input map as the store holds it, for comparing a before and an after: a
/// listing must leave every record exactly as it was.
fn stored_router_inputs(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.router_inputs)
}

/// What the STORED record holds for the members a summary reports, so the response is
/// compared against the persisted record rather than against a literal.
struct StoredSummary {
    name: String,
    id: String,
    input_type: String,
    state: String,
    routed_outputs: i32,
    region_name: String,
    availability_zone: String,
    maximum_bitrate: i64,
    routing_scope: String,
    message_count: usize,
    network_interface_arn: Option<String>,
    created_at: SystemTime,
    updated_at: SystemTime,
    has_maintenance_schedule: bool,
}

/// Read [`StoredSummary`] for `arn` straight out of the store.
///
/// `RoutedOutputs` is "the number of router outputs that are associated with this router
/// input", a relation that lives on the OUTPUT records, so it is counted from them here the
/// way the store's own counter does.
fn stored_summary(emulator: &Emulator, arn: &str) -> StoredSummary {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    let routed_outputs = i32::try_from(
        store
            .router_outputs
            .values()
            .filter(|output| output.routed_input_arn.as_deref() == Some(arn))
            .count(),
    )
    .unwrap_or(i32::MAX);
    let input = store
        .router_inputs
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture router input {arn} is stored"));
    StoredSummary {
        name: input.name.clone(),
        id: input.id.clone(),
        input_type: mc::router_input_type(&input.configuration).to_string(),
        state: input.state.clone(),
        routed_outputs,
        region_name: input.region_name.clone(),
        availability_zone: input.availability_zone.clone(),
        maximum_bitrate: input.maximum_bitrate,
        routing_scope: input.routing_scope.clone(),
        message_count: input.messages().len(),
        network_interface_arn: mc::router_input_network_interface_arn(&input.configuration)
            .map(str::to_string),
        created_at: input.created_at,
        updated_at: input.updated_at,
        has_maintenance_schedule: input.maintenance_schedule.is_some(),
    }
}

/// Route the stored router output `output_arn` to the router input `input_arn`.
///
/// `RouterOutput$RoutedInputArn` is set by TakeRouterInput, which is still scaffolded, so the
/// relation `ListedRouterInput$RoutedOutputs` counts is recorded through the store's own
/// out-of-band channel — the way the router output listing suite records the same relation
/// from the other side.
fn record_route(emulator: &Emulator, output_arn: &str, input_arn: &str) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let output = store
        .router_outputs
        .get_mut(output_arn)
        .expect("the router output the create minted is in the store");
    output.routed_input_arn = Some(input_arn.to_string());
}

/// One router input of the fixture store, and the attributes a filter selects on.
struct Fixture {
    case: String,
    arn: String,
    input_type: &'static str,
    region_name: String,
    routing_scope: &'static str,
    /// The router network interface the input ingests on, for the configurations that name
    /// one.
    network_interface_arn: Option<String>,
}

/// The fixture store, and the resources its inputs and the routed outputs name.
struct Fixtures {
    inputs: Vec<Fixture>,
    /// The interface the configuration table's inputs ingest on.
    interface_one: String,
    /// A second interface, so a `NetworkInterfaceArns` filter has two to separate.
    interface_two: String,
}

/// `create_body` with the extra request members `extra` names — the Region and routing scope
/// a fixture needs beside the minimal create body the other router input suites use.
fn create_body_with(name: &str, configuration: Value, extra: &[(&str, Value)]) -> Value {
    let mut body = create_body(name, configuration);
    for (member, value) in extra {
        body[*member] = value.clone();
    }
    body
}

/// Create one router input over the wire and answer the ARN the service minted for it.
async fn minted_input(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, code, created) = create_router_input(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterInput expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        mc::is_mintable_router_input_arn(&arn),
        "case {case:?}: the create mints a RouterInputArn, got {arn:?}"
    );
    arn
}

/// Mint a second router network interface, so a `NetworkInterfaceArns` filter has two
/// interfaces to separate.
async fn second_interface(emulator: &Emulator) -> String {
    let (status, code, created) = create_network_interface(
        emulator,
        json!({ "name": "rni-ri-listing-two", "configuration": public_configuration(1) }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the second interface is minted, got {status} {code:?}: {created}"
    );
    created["routerNetworkInterface"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The fixture store every filtering case runs against: one router input per
/// `RouterInputConfiguration` member the model declares (through the configuration table the
/// router input suites share), plus one STANDARD input on a SECOND network interface with a
/// GLOBAL routing scope and one input in ANOTHER Region, so no filter can pass by matching
/// everything.
async fn fixtures(emulator: &Emulator) -> Fixtures {
    let interface_one = minted_network_interface(emulator, "rni-ri-listing-one").await;
    let (flow_arn, _) = minted_flow_source(emulator, "flow-ri-listing").await;
    let interface_two = second_interface(emulator).await;

    let mut inputs: Vec<Fixture> = Vec::new();
    for (kind, _, _, configuration, expected_input_type) in
        configuration_rows(&interface_one, &flow_arn)
    {
        let name = format!("ri-listing-{kind}").replace('/', "-");
        let arn = minted_input(
            emulator,
            kind,
            create_body(&name, configuration.clone()),
        )
        .await;
        // Which interface an input ingests on follows from its configuration: the STANDARD,
        // FAILOVER and MERGE members name one, the two that take their content from another
        // AWS resource do not.
        let network_interface_arn = match expected_input_type {
            mc::ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW | mc::ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL => {
                None
            }
            _ => Some(interface_one.clone()),
        };
        inputs.push(Fixture {
            case: kind.to_string(),
            arn,
            input_type: expected_input_type,
            region_name: REGION.to_string(),
            routing_scope: "REGIONAL",
            network_interface_arn,
        });
    }

    // A STANDARD input on the OTHER interface, routed globally: the row that separates the
    // two interfaces and the two routing scopes.
    let (_, _, _, standard_on_two, _) = configuration_rows(&interface_two, &flow_arn)
        .into_iter()
        .next()
        .expect("the configuration table is not empty");
    let arn = minted_input(
        emulator,
        "standard/second-interface",
        create_body_with(
            "ri-listing-second-interface",
            standard_on_two,
            &[("routingScope", json!("GLOBAL"))],
        ),
    )
    .await;
    inputs.push(Fixture {
        case: "standard/second-interface".to_string(),
        arn,
        input_type: mc::ROUTER_INPUT_TYPE_STANDARD,
        region_name: REGION.to_string(),
        routing_scope: "GLOBAL",
        network_interface_arn: Some(interface_two.clone()),
    });

    // An input in another Region. A MEDIALIVE CHANNEL input takes its content from another
    // service's resource rather than from an interface of this account, so the cross-Region
    // row needs nothing minted in that Region.
    let medialive = json!({
        "MediaLiveChannel": {
            "mediaLiveChannelArn": "arn:aws:medialive:eu-west-1:000000000000:channel:7654321",
            "mediaLivePipelineId": "PIPELINE_1",
            "sourceTransitDecryption": {
                "encryptionKeyType": "AUTOMATIC",
                "encryptionKeyConfiguration": { "Automatic": {} },
            },
        }
    });
    let arn = minted_input(
        emulator,
        "cross-region",
        create_body_with(
            "ri-listing-cross-region",
            medialive,
            &[("regionName", json!(OTHER_REGION))],
        ),
    )
    .await;
    inputs.push(Fixture {
        case: "cross-region".to_string(),
        arn,
        input_type: mc::ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL,
        region_name: OTHER_REGION.to_string(),
        routing_scope: "REGIONAL",
        network_interface_arn: None,
    });

    Fixtures {
        inputs,
        interface_one,
        interface_two,
    }
}

/// The ARNs of the fixtures `cases` names, in the order the STORE holds them — which is the
/// order a listing must report them in.
fn expected_arns(fixtures: &[Fixture], cases: &[&str]) -> Vec<String> {
    let mut arns: Vec<String> = fixtures
        .iter()
        .filter(|fixture| cases.contains(&fixture.case.as_str()))
        .map(|fixture| fixture.arn.clone())
        .collect();
    arns.sort();
    arns
}

/// The members a `ListedRouterInput` declares (`smithy_query(query="shape",
/// shape="ListedRouterInput")`), by their wire names: the thirteen required ones.
const REQUIRED_MEMBERS: [&str; 13] = [
    "name",
    "arn",
    "id",
    "inputType",
    "state",
    "routedOutputs",
    "regionName",
    "availabilityZone",
    "maximumBitrate",
    "routingScope",
    "createdAt",
    "updatedAt",
    "messageCount",
];

/// The members a `RouterInput` carries that its summary does NOT declare, so a summary
/// reporting one of them would be reporting a member the model does not put there.
const MEMBERS_OF_THE_FULL_RESOURCE: [&str; 11] = [
    "configuration",
    "streamDetails",
    "ipAddress",
    "tier",
    "tags",
    "messages",
    "transitEncryption",
    "maintenanceType",
    "maintenanceConfiguration",
    "contentQualityAnalysisType",
    "contentQualityAnalysisConfiguration",
];

/// Create one router output over the wire on `interface_arn` and answer its ARN — a resource
/// whose `RoutedInputArn` is what `ListedRouterInput$RoutedOutputs` counts.
async fn minted_output(emulator: &Emulator, name: &str, port: i64) -> String {
    let body = json!({
        "name": name,
        "configuration": standard_with_protocol(
            &minted_network_interface(emulator, &format!("rni-{name}")).await,
            json!({ "Rtp": { "destinationAddress": "198.51.100.20", "destinationPort": port } }),
            "RTP",
        ),
        "maximumBitrate": 20_000_000_i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    });
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture router output {name:?} is minted, got {status} {code:?}: {created}"
    );
    created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The listing reports every stored router input, projected from its own record, whatever
/// kind of input it is — and it stores nothing.
#[tokio::test]
async fn every_stored_router_input_is_listed_from_its_own_record() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;
    let inputs = &fixtures.inputs;

    // Router outputs routed to two of the inputs — two to the first and one to the second —
    // so `RoutedOutputs` is a count that differs per input rather than a constant.
    for (index, port) in [5100_i64, 5101].into_iter().enumerate() {
        let output = minted_output(&emulator, &format!("ro-ri-listing-{index}"), port).await;
        record_route(&emulator, &output, &inputs[0].arn);
    }
    let output = minted_output(&emulator, "ro-ri-listing-2", 5102).await;
    record_route(&emulator, &output, &inputs[1].arn);

    // Past the span the clock owes a new input, so every record is settled and a listing is
    // free of state the clock is still moving.
    tokio::time::sleep(past_transition_span()).await;

    let body = page(&emulator, "every kind at once", "?maxResults=100", None).await;
    // Snapshotted AFTER that first listing: materialising a state change the clock had
    // already made due is the one write a listing performs, and that span has now passed, so
    // from here on a further listing must leave every record exactly as it is.
    let before = stored_router_inputs(&emulator);
    let expected = stored_arns(&emulator);
    assert_eq!(
        expected.len(),
        inputs.len(),
        "every fixture input must be stored, got {expected:?}"
    );
    assert_eq!(
        listed_arns(&body),
        expected,
        "every stored router input is listed, in the store's own order, got {body}"
    );

    let mut routed_counts: Vec<i64> = Vec::new();
    for fixture in inputs {
        let case = fixture.case.as_str();
        let stored = stored_summary(&emulator, &fixture.arn);
        let summary = listed(&body)
            .into_iter()
            .find(|summary| summary["arn"].as_str() == Some(fixture.arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the stored input is listed: {body}"));

        assert_eq!(
            (
                summary["name"].as_str(),
                summary["id"].as_str(),
                summary["inputType"].as_str(),
                summary["state"].as_str(),
                summary["regionName"].as_str(),
                summary["availabilityZone"].as_str(),
                summary["routingScope"].as_str(),
                summary["maximumBitrate"].as_i64(),
                summary["messageCount"].as_i64(),
                summary["routedOutputs"].as_i64(),
            ),
            (
                Some(stored.name.as_str()),
                Some(stored.id.as_str()),
                Some(stored.input_type.as_str()),
                Some(stored.state.as_str()),
                Some(stored.region_name.as_str()),
                Some(stored.availability_zone.as_str()),
                Some(stored.routing_scope.as_str()),
                Some(stored.maximum_bitrate),
                Some(stored.message_count as i64),
                Some(i64::from(stored.routed_outputs)),
            ),
            "case {case:?}: the summary is the stored record, got {summary}"
        );
        // The attributes the fixture asked for, so a projection reading the wrong record's
        // field could not pass by being self-consistent.
        assert_eq!(
            (
                summary["inputType"].as_str(),
                summary["regionName"].as_str(),
                summary["routingScope"].as_str(),
            ),
            (
                Some(fixture.input_type),
                Some(fixture.region_name.as_str()),
                Some(fixture.routing_scope),
            ),
            "case {case:?}: the summary reports what the create asked for, got {summary}"
        );
        assert_eq!(
            summary["networkInterfaceArn"].as_str(),
            stored.network_interface_arn.as_deref(),
            "case {case:?}: the interface is reported exactly for an input that ingests on \
             one, got {summary}"
        );
        assert_eq!(
            summary["networkInterfaceArn"].as_str(),
            fixture.network_interface_arn.as_deref(),
            "case {case:?}: and it is the interface the configuration names, got {summary}"
        );
        assert_eq!(
            (
                summary["maintenanceSchedule"].is_null(),
                summary["maintenanceScheduleType"].is_null(),
            ),
            (
                !stored.has_maintenance_schedule,
                !stored.has_maintenance_schedule
            ),
            "case {case:?}: a window is reported exactly when the record holds one, got \
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
        for (member, instant) in [("createdAt", stored.created_at), ("updatedAt", stored.updated_at)]
        {
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
        routed_counts.push(summary["routedOutputs"].as_i64().unwrap_or_default());
    }
    assert_eq!(
        (routed_counts[0], routed_counts[1]),
        (2, 1),
        "the routed output count is the number of outputs routed to each input, got \
         {routed_counts:?}"
    );
    assert!(
        routed_counts[2..].iter().all(|count| *count == 0),
        "an input nothing is routed to counts no routed outputs, got {routed_counts:?}"
    );

    // A read does not change what a later caller sees: the second listing is the same
    // document and the store is untouched.
    let again = page(&emulator, "repeated listing", "?maxResults=100", None).await;
    assert_eq!(again, body, "listing twice reports the same page, got {again}");
    let after = stored_router_inputs(&emulator);
    assert_eq!(
        after, before,
        "listing router inputs must store nothing; expected {before}, got {after}"
    );
}

/// One filtering case: the `Filters` a request carries and the fixtures it must report.
struct FilterRow {
    case: &'static str,
    /// Which fixture [`Fixture::case`] names the filter must select, in any order: the
    /// expectation is sorted into the store's order before it is compared.
    expected: &'static [&'static str],
}

/// `Filters` narrows the listing to the inputs that satisfy EVERY element, and a request
/// carrying none lists them all.
///
/// The rows span the `RouterInputFilter` union: every member the model declares, a member
/// naming several accepted values, several members at once, a value no stored input carries,
/// and a member naming no value at all.
#[tokio::test]
async fn filters_report_only_the_router_inputs_that_satisfy_them() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;
    let interface_one = fixtures.interface_one.clone();
    let interface_two = fixtures.interface_two.clone();
    let inputs = &fixtures.inputs;

    tokio::time::sleep(past_transition_span()).await;
    // One listing materialises the state changes the clock has already made due — its only
    // write — so the snapshot every filtered listing below is compared against is taken
    // after it.
    page(&emulator, "settling listing", "?maxResults=100", None).await;
    let before = stored_router_inputs(&emulator);

    // Every fixture case, for the rows that must report them all.
    const EVERY_CASE: &[&str] = &[
        "standard/rtp",
        "standard/srt-listener",
        "failover",
        "merge",
        "mediaconnect-flow",
        "medialive-channel",
        "standard/second-interface",
        "cross-region",
    ];

    let cases: Vec<(FilterRow, Value)> = vec![
        (
            FilterRow {
                case: "no filter at all",
                expected: EVERY_CASE,
            },
            json!([]),
        ),
        (
            FilterRow {
                case: "RegionNames naming the cross-Region input's Region",
                expected: &["cross-region"],
            },
            json!([{ "RegionNames": [OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "RegionNames naming both Regions",
                expected: EVERY_CASE,
            },
            json!([{ "RegionNames": [REGION, OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceArns naming one interface",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "failover",
                    "merge",
                ],
            },
            json!([{ "NetworkInterfaceArns": [&interface_one] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceArns naming the second interface",
                expected: &["standard/second-interface"],
            },
            json!([{ "NetworkInterfaceArns": [&interface_two] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceArns naming both interfaces",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "failover",
                    "merge",
                    "standard/second-interface",
                ],
            },
            json!([{ "NetworkInterfaceArns": [&interface_one, &interface_two] }]),
        ),
        (
            FilterRow {
                case: "RoutingScopes naming GLOBAL",
                expected: &["standard/second-interface"],
            },
            json!([{ "RoutingScopes": ["GLOBAL"] }]),
        ),
        (
            FilterRow {
                case: "RoutingScopes naming REGIONAL",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "failover",
                    "merge",
                    "mediaconnect-flow",
                    "medialive-channel",
                    "cross-region",
                ],
            },
            json!([{ "RoutingScopes": ["REGIONAL"] }]),
        ),
        (
            FilterRow {
                case: "InputTypes naming one type",
                expected: &["merge"],
            },
            json!([{ "InputTypes": [mc::ROUTER_INPUT_TYPE_MERGE] }]),
        ),
        (
            FilterRow {
                case: "InputTypes naming two types",
                expected: &["failover", "mediaconnect-flow"],
            },
            json!([{ "InputTypes": [
                mc::ROUTER_INPUT_TYPE_FAILOVER,
                mc::ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW,
            ] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment one name carries",
                expected: &["standard/srt-listener"],
            },
            json!([{ "NameContains": ["srt-listener"] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment every name carries",
                expected: EVERY_CASE,
            },
            json!([{ "NameContains": ["ri-listing-"] }]),
        ),
        (
            FilterRow {
                case: "NameContains two fragments",
                expected: &["merge", "cross-region"],
            },
            json!([{ "NameContains": ["merge", "cross-region"] }]),
        ),
        (
            FilterRow {
                case: "two members at once, both satisfied by one input",
                expected: &["standard/second-interface"],
            },
            json!([
                { "RoutingScopes": ["GLOBAL"] },
                { "InputTypes": [mc::ROUTER_INPUT_TYPE_STANDARD] },
            ]),
        ),
        (
            FilterRow {
                case: "two members at once that no input satisfies together",
                expected: &[],
            },
            json!([
                { "RoutingScopes": ["GLOBAL"] },
                { "InputTypes": [mc::ROUTER_INPUT_TYPE_MERGE] },
            ]),
        ),
        (
            FilterRow {
                case: "a Region no stored input is in",
                expected: &[],
            },
            json!([{ "RegionNames": ["ap-south-1"] }]),
        ),
        (
            FilterRow {
                case: "a member naming no value at all",
                expected: &[],
            },
            json!([{ "RegionNames": [] }]),
        ),
    ];

    for (FilterRow { case, expected }, filters) in cases {
        let expected = expected_arns(inputs, expected);
        let body = page(&emulator, case, "?maxResults=100", Some(filters.clone())).await;
        assert_eq!(
            listed_arns(&body),
            expected,
            "case {case:?}: the filter {filters} reports exactly the matching inputs, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: a page holding every match carries no token, got {body}"
        );
    }

    // Filtering is a read like any other: it stores nothing.
    let after = stored_router_inputs(&emulator);
    assert_eq!(
        after, before,
        "a filtered listing must store nothing; expected {before}, got {after}"
    );
}

/// The listing pages every matching router input exactly once, whatever `MaxResults` is, and
/// the token is the ARN the next page resumes at. With no `MaxResults` the page holds as many
/// as the modelled range allows.
#[tokio::test]
async fn pages_partition_every_router_input_exactly_once() {
    let emulator = Emulator::new();
    // More than the largest page the modelled `MaxResults` range allows, so the page a
    // request naming no size gets is observably bounded. The store ceiling is a bound on what
    // is STORED, so these records go in through the store's own channel; the calls under test
    // still go over the wire.
    for index in 0..130 {
        crate::router_input_deletion::seed_router_input(
            &emulator,
            &format!("seeded-ri-listing-{index}"),
            mc::ROUTER_INPUT_STATE_STANDBY,
        );
    }
    let all = stored_arns(&emulator);
    assert_eq!(all.len(), 130, "every seeded input is stored, got {all:?}");

    // `MaxResults` is "the maximum number of router inputs to return in the response" and the
    // model documents no smaller default, so a request naming none is answered with the
    // maximum of the modelled range (1..100).
    let default_page = page(&emulator, "the default page size", "", None).await;
    assert_eq!(
        listed_arns(&default_page),
        all[..100].to_vec(),
        "the default page holds the first hundred inputs, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(all[100].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled range: its lower bound, sizes between, and its upper
    // bound — each walked with no filter and under one, because filtering and paging compose.
    // The filter keeps every seeded input, so the two walks must agree.
    let filter = json!([{ "NameContains": ["seeded-ri-listing-"] }]);
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
                "case {case:?}: the pages must partition the stored inputs once, in order"
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

/// An account that holds no router input is listed empty — not a 404 and not a page of
/// invented inputs.
#[tokio::test]
async fn an_account_that_holds_no_router_input_is_listed_empty() {
    let emulator = Emulator::new();
    for (case, filters) in [
        ("no filter", None),
        ("a filter", Some(json!([{ "RoutingScopes": ["REGIONAL"] }]))),
    ] {
        let body = page(&emulator, case, "", filters).await;
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: an account holding no router input lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );
    }
}

/// The listed `State` and maintenance window are the CURRENT ones: a listing materialises
/// only the change the clock already made due, so an input whose creation is still in flight
/// is listed `CREATING`, the transition span settles it into `STANDBY`, and starting it is
/// what schedules the window the summary then reports.
#[tokio::test]
async fn the_listed_state_is_the_one_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ri-listing-clock").await;
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-ri-listing-clock").await;
    let (_, _, _, configuration, _) = configuration_rows(&interface_arn, &flow_arn)
        .into_iter()
        .next()
        .expect("the configuration table is not empty");
    let arn = minted_input(
        &emulator,
        "clock",
        create_body("ri-listing-clock", configuration),
    )
    .await;

    let body = page(&emulator, "inside the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_INPUT_STATE_CREATING),
        "inside the span the creation is still in flight, got {body}"
    );
    assert!(
        listed(&body)
            .first()
            .is_some_and(|summary| summary["maintenanceSchedule"].is_null()),
        "nothing has scheduled a window for an input that was never started, got {body}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "past the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "once the span has passed the creation is settled, got {body}"
    );

    // Starting the input schedules its maintenance window, and the summary reports that
    // window and the one type the `MaintenanceScheduleType` enum declares beside it.
    let (status, code, started) = emulator
        .send("POST", &format!("/v1/routerInput/start/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "StartRouterInput accepts the start, got {status} {code:?}: {started}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "a started input", "", None).await;
    let summary = listed(&body)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the started input is listed: {body}"));
    assert_eq!(
        (
            summary["state"].as_str(),
            summary["maintenanceScheduleType"].as_str(),
        ),
        (
            Some(mc::ROUTER_INPUT_STATE_ACTIVE),
            Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
        ),
        "a started input is listed ACTIVE with the window type it was given, got {summary}"
    );
    assert_eq!(
        summary["maintenanceSchedule"], started["maintenanceSchedule"],
        "the listed window is the one the start answered, got {summary}"
    );

    // A deleted router input leaves the store once the clock completes the delete, and the
    // listing stops reporting it rather than reporting a record nothing holds.
    let (status, code, deleting) = emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DeleteRouterInput accepts the delete, got {status} {code:?}: {deleting}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "a completed delete", "", None).await;
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "an input the store no longer holds is not listed, got {body}"
    );
}

/// One refused ListRouterInputs request.
struct RefusalCase {
    case: &'static str,
    query: &'static str,
}

/// ListRouterInputs refuses a page size outside the modelled range and a pagination token it
/// could not have issued — with the modelled `BadRequestException` (400), never with a 501
/// and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    // The modelled `MaxResults` range is 1..100, so each bound is usable (the paging suite
    // above lists with 1 and with 100) and each value past it is refused. A `NextToken` is the
    // router input ARN the listing resumes at, so a token the service could not have minted is
    // refused; the rows are the ways a token can fail that form.
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
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:routerInput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose account id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:routerInput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose input id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerInput:a1b2c3d4e5f",
        },
        RefusalCase {
            case: "a nextToken whose input id carries an upper-case character",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerInput:A1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose input id is empty",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerInput:",
        },
    ];

    for RefusalCase { case, query } in CASES {
        let (status, code, body) = list_router_inputs(&emulator, query, None).await;
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

    // `Filters` is bounded as well: every stored input is tested against every filter and
    // every value one names, so a request past either ceiling is refused rather than
    // answered. Each ceiling is asserted usable at its bound and refused one past it.
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
        let (status, code, body) = list_router_inputs(&emulator, "", Some(filters)).await;
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

    // A token the service COULD have minted is accepted even when no such input is stored:
    // paging must survive a delete between pages.
    let unknown = "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:ffffffffffff";
    let body = page(
        &emulator,
        "a well-formed token for an unknown input",
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
