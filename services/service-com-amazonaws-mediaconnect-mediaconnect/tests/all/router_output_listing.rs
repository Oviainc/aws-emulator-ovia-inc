// SPDX-License-Identifier: CC0-1.0

//! ListRouterOutputs, end to end over HTTP: `POST /v1/routerOutputs`.
//!
//! The subject is the rule ListRouterOutputs follows: it "retrieves a list of router
//! outputs", a page at a time in the store's own order, each entry the STORED router output
//! record projected into the modelled `ListedRouterOutput` summary
//! (`smithy_query(query="shape",shape="ListedRouterOutput")`), narrowed to the outputs that
//! satisfy EVERY element of `Filters` — an element names one attribute of a router output
//! and the values accepted for it. Nothing is fixed in the handler: every member asserted
//! below is compared against what the store holds, and an output the store does not hold is
//! not listed.
//!
//! The rows therefore vary what that rule generalises over: which `RouterOutputConfiguration`
//! variant (and so which output type and whether there is a network interface) an output
//! carries, its Region, routing scope, tier, bitrate and name, whether a router input is
//! routed to it, how many messages its record holds, whether a maintenance window has been
//! scheduled for it, which member of the `RouterOutputFilter` union a request filters on
//! (every one the model declares, including several at once and a value nothing matches),
//! the page size across the modelled `MaxResults` range and its absence, and the form of the
//! pagination token.
//!
//! Every call goes over the wire through the same router the emulator's front door uses, on
//! the harness `flow_lifecycle` owns, so what these tests assert is what a MediaConnect
//! client sees: routing, restJson1 deserialization, constraint validation, handler,
//! serialization. The create helper, the minted configuration targets and the store-seeding
//! helper are the ones `router_outputs` owns, so the router output suites cannot drift
//! apart.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::RouterOutputMessage;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{
    create_body as router_input_body, create_router_input, minted_configuration_rows,
};
use crate::router_network_interfaces::{create_network_interface, public_configuration};
use crate::router_outputs::{
    create_router_output, minted_configuration_targets, seed_router_outputs,
    standard_with_protocol, ConfigurationTargets, MEDIALIVE_INPUT_ARN, ROLE_ARN, SECRET_ARN,
};

/// The Region the fixture's cross-Region output is created in — a Region other than the one
/// every other fixture lands in ([`REGION`]), so a `RegionNames` filter has something to
/// separate.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough for a transition the clock owes to be due: twice `mc::TRANSITION_SPAN`, so a
/// settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Send one ListRouterOutputs request: `POST /v1/routerOutputs`, with `MaxResults` and
/// `NextToken` bound to the query string and `Filters` carried in the body
/// (`smithy_query(query="operation",operation="ListRouterOutputs")`).
async fn list_router_outputs(
    emulator: &Emulator,
    query: &str,
    filters: Option<Value>,
) -> (u16, Option<String>, Value) {
    let body = filters.map(|filters| json!({ "filters": filters }));
    emulator
        .send("POST", &format!("/v1/routerOutputs{query}"), body)
        .await
}

/// A 200 listing, asserted to be answered at all.
async fn page(emulator: &Emulator, case: &str, query: &str, filters: Option<Value>) -> Value {
    let (status, code, body) = list_router_outputs(emulator, query, filters.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing {query:?} with {filters:?} expected 200, got {status} {code:?}: \
         {body}"
    );
    body
}

/// The `RouterOutputs` member of a ListRouterOutputs response.
fn listed(body: &Value) -> Vec<Value> {
    body["routerOutputs"].as_array().cloned().unwrap_or_default()
}

/// The ARNs of the `RouterOutputs` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|summary| summary["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every router output ARN the store holds, in the order the store holds them — the order a
/// listing must report, derived from the store rather than written out here.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store.router_outputs.keys().cloned().collect()
}

/// The whole router output map as the store holds it, for comparing a before and an after: a
/// listing must leave every record exactly as it was.
fn stored_router_outputs(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.router_outputs)
}

/// What the STORED record holds for the members a summary reports, so the response is
/// compared against the persisted record rather than against a literal.
struct StoredSummary {
    name: String,
    id: String,
    output_type: String,
    state: String,
    routed_state: String,
    region_name: String,
    availability_zone: String,
    maximum_bitrate: i64,
    routing_scope: String,
    message_count: usize,
    routed_input_arn: Option<String>,
    network_interface_arn: Option<String>,
    created_at: SystemTime,
    updated_at: SystemTime,
    has_maintenance_schedule: bool,
}

/// Read [`StoredSummary`] for `arn` straight out of the store.
fn stored_summary(emulator: &Emulator, arn: &str) -> StoredSummary {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    let output = store
        .router_outputs
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture router output {arn} is stored"));
    StoredSummary {
        name: output.name.clone(),
        id: output.id.clone(),
        output_type: output.output_type().to_string(),
        state: output.state.clone(),
        routed_state: output.routed_state().to_string(),
        region_name: output.region_name.clone(),
        availability_zone: output.availability_zone.clone(),
        maximum_bitrate: output.maximum_bitrate,
        routing_scope: output.routing_scope.clone(),
        message_count: output.messages.len(),
        routed_input_arn: output.routed_input_arn.clone(),
        network_interface_arn: mc::router_output_network_interface_arn(&output.configuration)
            .map(str::to_string),
        created_at: output.created_at,
        updated_at: output.updated_at,
        has_maintenance_schedule: output.maintenance_schedule.is_some(),
    }
}

/// Route `input_arn` to the stored router output `arn`, and record `messages` on it.
///
/// `RouterOutput$RoutedInputArn` is set by TakeRouterInput and the operations that record a
/// message are their own; both are still scaffolded, so the two members a summary reports
/// from them are set through the store's own out-of-band channel — the way the gateway
/// instance suites register an instance.
fn record_route_and_messages(emulator: &Emulator, arn: &str, input_arn: &str, messages: usize) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let output = store
        .router_outputs
        .get_mut(arn)
        .expect("the router output the create minted is in the store");
    output.routed_input_arn = Some(input_arn.to_string());
    output.messages = (0..messages)
        .map(|index| RouterOutputMessage {
            code: format!("ROUTER_OUTPUT_NOTICE_{index}"),
            message: format!("notice {index} about {arn}"),
        })
        .collect();
}

/// One router output of the fixture store, and the attributes a filter selects on.
struct Fixture {
    case: &'static str,
    arn: String,
    output_type: &'static str,
    region_name: String,
    routing_scope: &'static str,
    /// The router network interface the output egresses from, for the configurations that
    /// name one.
    network_interface_arn: Option<String>,
}

/// Mint a second router network interface, so a `NetworkInterfaceArns` filter has two
/// interfaces to separate.
async fn second_interface(emulator: &Emulator) -> String {
    let (status, code, created) = create_network_interface(
        emulator,
        json!({ "name": "rni-ro-listing-two", "configuration": public_configuration(1) }),
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

/// Create one router output over the wire and answer the ARN the service minted for it.
async fn minted_output(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterOutput expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        mc::is_mintable_router_output_arn(&arn),
        "case {case:?}: the create mints a RouterOutputArn, got {arn:?}"
    );
    arn
}

/// The fixture store every filtering case runs against: one router output per
/// `RouterOutputConfiguration` member the model declares (`Standard`, `MediaConnectFlow`,
/// `MediaLiveInput`), two `Standard` ones on DIFFERENT network interfaces and with different
/// protocol configurations, differing also in Region, routing scope, tier, bitrate and name,
/// so no filter can pass by matching everything.
async fn fixtures(emulator: &Emulator) -> Vec<Fixture> {
    let targets: ConfigurationTargets = minted_configuration_targets(emulator, "ro-listing").await;
    let other_interface = second_interface(emulator).await;

    let rtp = minted_output(
        emulator,
        "standard/rtp",
        json!({
            "name": "ro-listing-standard-rtp",
            "configuration": standard_with_protocol(&targets.interface_arn, json!({
                "Rtp": { "destinationAddress": "203.0.113.30", "destinationPort": 5004 }
            }), "RTP"),
            "maximumBitrate": 20_000_000_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
        }),
    )
    .await;
    let srt = minted_output(
        emulator,
        "standard/srt-listener",
        json!({
            "name": "ro-listing-standard-srt",
            "configuration": standard_with_protocol(&other_interface, json!({
                "SrtListener": { "port": 3200, "minimumLatencyMilliseconds": 120 }
            }), "SRT_LISTENER"),
            "maximumBitrate": 100_000_000_i64,
            "routingScope": "GLOBAL",
            "tier": "OUTPUT_100",
            "availabilityZone": "us-east-1c",
            "tags": { "team": "broadcast" },
        }),
    )
    .await;
    let flow = minted_output(
        emulator,
        "mediaconnect-flow",
        json!({
            "name": "ro-listing-flow",
            "configuration": {
                "MediaConnectFlow": {
                    "flowArn": targets.flow_arn,
                    "flowSourceArn": targets.flow_source_arn,
                    "destinationTransitEncryption": {
                        "encryptionKeyType": "SECRETS_MANAGER",
                        "encryptionKeyConfiguration": {
                            "SecretsManager": { "secretArn": SECRET_ARN, "roleArn": ROLE_ARN }
                        },
                    },
                }
            },
            "maximumBitrate": 500_000_000_i64,
            "routingScope": "GLOBAL",
            "tier": "OUTPUT_500",
            "regionName": OTHER_REGION,
            "availabilityZone": format!("{OTHER_REGION}b"),
        }),
    )
    .await;
    let medialive = minted_output(
        emulator,
        "medialive-input",
        json!({
            "name": "ro-listing-medialive",
            "configuration": {
                "MediaLiveInput": {
                    "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                    "mediaLivePipelineId": "PIPELINE_1",
                    "destinationTransitEncryption": {
                        "encryptionKeyType": "AUTOMATIC",
                        "encryptionKeyConfiguration": { "Automatic": {} },
                    },
                }
            },
            "maximumBitrate": 250_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_250",
        }),
    )
    .await;

    vec![
        Fixture {
            case: "standard/rtp",
            arn: rtp,
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            region_name: REGION.to_string(),
            routing_scope: "REGIONAL",
            network_interface_arn: Some(targets.interface_arn.clone()),
        },
        Fixture {
            case: "standard/srt-listener",
            arn: srt,
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            region_name: REGION.to_string(),
            routing_scope: "GLOBAL",
            network_interface_arn: Some(other_interface),
        },
        Fixture {
            case: "mediaconnect-flow",
            arn: flow,
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
            region_name: OTHER_REGION.to_string(),
            routing_scope: "GLOBAL",
            network_interface_arn: None,
        },
        Fixture {
            case: "medialive-input",
            arn: medialive,
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT,
            region_name: REGION.to_string(),
            routing_scope: "REGIONAL",
            network_interface_arn: None,
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

/// The members a `ListedRouterOutput` declares (`smithy_query(query="shape",
/// shape="ListedRouterOutput")`), by their wire names: the thirteen required ones, then the
/// four optional ones.
const REQUIRED_MEMBERS: [&str; 13] = [
    "name",
    "arn",
    "id",
    "outputType",
    "state",
    "routedState",
    "regionName",
    "availabilityZone",
    "maximumBitrate",
    "routingScope",
    "createdAt",
    "updatedAt",
    "messageCount",
];

/// The members a `RouterOutput` carries that its summary does NOT declare, so a summary
/// reporting one of them would be reporting a member the model does not put there.
const MEMBERS_OF_THE_FULL_RESOURCE: [&str; 8] = [
    "configuration",
    "streamDetails",
    "ipAddress",
    "tier",
    "tags",
    "messages",
    "maintenanceConfiguration",
    "fabricConfiguration",
];

/// The listing reports every stored router output, projected from its own record, whatever
/// kind of output it is — and it stores nothing.
#[tokio::test]
async fn every_stored_router_output_is_listed_from_its_own_record() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;

    // A router input of this account, routed to one of the outputs, and messages recorded on
    // it: the two members a summary reports that a create never sets.
    let rows = minted_configuration_rows(&emulator, "ro-listing-input").await;
    let (_, _, _, input_configuration, _) = rows
        .into_iter()
        .next()
        .expect("the router input configuration rows are not empty");
    let (status, code, created_input) = create_router_input(
        &emulator,
        router_input_body("ri-ro-listing", input_configuration),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture router input is created, got {status} {code:?}: {created_input}"
    );
    let input_arn = created_input["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    record_route_and_messages(&emulator, &fixtures[0].arn, &input_arn, 2);

    // Past the span the clock owes a new output, so every record is settled and a listing is
    // free of state the clock is still moving.
    tokio::time::sleep(past_transition_span()).await;

    let body = page(&emulator, "every kind at once", "?maxResults=100", None).await;
    // Snapshotted AFTER that first listing: materialising a state change the clock had
    // already made due is the one write a listing performs, and that span has now passed,
    // so from here on a further listing must leave every record exactly as it is.
    let before = stored_router_outputs(&emulator);
    let expected = stored_arns(&emulator);
    assert_eq!(
        expected.len(),
        fixtures.len(),
        "every fixture output must be stored, got {expected:?}"
    );
    assert_eq!(
        listed_arns(&body),
        expected,
        "every stored router output is listed, in the store's own order, got {body}"
    );

    for fixture in &fixtures {
        let case = fixture.case;
        let stored = stored_summary(&emulator, &fixture.arn);
        let summary = listed(&body)
            .into_iter()
            .find(|summary| summary["arn"].as_str() == Some(fixture.arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the stored output is listed: {body}"));

        assert_eq!(
            (
                summary["name"].as_str(),
                summary["id"].as_str(),
                summary["outputType"].as_str(),
                summary["state"].as_str(),
                summary["routedState"].as_str(),
                summary["regionName"].as_str(),
                summary["availabilityZone"].as_str(),
                summary["routingScope"].as_str(),
                summary["maximumBitrate"].as_i64(),
                summary["messageCount"].as_i64(),
            ),
            (
                Some(stored.name.as_str()),
                Some(stored.id.as_str()),
                Some(stored.output_type.as_str()),
                Some(stored.state.as_str()),
                Some(stored.routed_state.as_str()),
                Some(stored.region_name.as_str()),
                Some(stored.availability_zone.as_str()),
                Some(stored.routing_scope.as_str()),
                Some(stored.maximum_bitrate),
                Some(stored.message_count as i64),
            ),
            "case {case:?}: the summary is the stored record, got {summary}"
        );
        // The attributes the fixture asked for, so a projection reading the wrong record's
        // field could not pass by being self-consistent.
        assert_eq!(
            (
                summary["outputType"].as_str(),
                summary["regionName"].as_str(),
                summary["routingScope"].as_str(),
            ),
            (
                Some(fixture.output_type),
                Some(fixture.region_name.as_str()),
                Some(fixture.routing_scope),
            ),
            "case {case:?}: the summary reports what the create asked for, got {summary}"
        );
        assert_eq!(
            summary["networkInterfaceArn"].as_str(),
            stored.network_interface_arn.as_deref(),
            "case {case:?}: the interface is reported exactly for an output that egresses \
             from one, got {summary}"
        );
        assert_eq!(
            summary["networkInterfaceArn"].as_str(),
            fixture.network_interface_arn.as_deref(),
            "case {case:?}: and it is the interface the configuration names, got {summary}"
        );
        assert_eq!(
            summary["routedInputArn"].as_str(),
            stored.routed_input_arn.as_deref(),
            "case {case:?}: the routed input is the one the record holds, got {summary}"
        );
        assert_eq!(
            stored.routed_state,
            if stored.routed_input_arn.is_some() {
                mc::ROUTED_STATE_ROUTED
            } else {
                mc::ROUTED_STATE_UNROUTED
            },
            "case {case:?}: the routed state follows from the routed input, got {summary}"
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
                "case {case:?}: {member} is the instant the record holds, expected \
                 {expected}, got {reported}"
            );
        }
    }

    // A read does not change what a later caller sees: the second listing is the same
    // document and the store is untouched.
    let again = page(&emulator, "repeated listing", "?maxResults=100", None).await;
    assert_eq!(again, body, "listing twice reports the same page, got {again}");
    let after = stored_router_outputs(&emulator);
    assert_eq!(
        after, before,
        "listing router outputs must store nothing; expected {before}, got {after}"
    );
}

/// One filtering case: the `Filters` a request carries and the fixtures it must report.
struct FilterRow {
    case: &'static str,
    /// Which fixture [`Fixture::case`] names the filter must select, in any order: the
    /// expectation is sorted into the store's order before it is compared.
    expected: &'static [&'static str],
}

/// `Filters` narrows the listing to the outputs that satisfy EVERY element, and a request
/// carrying none lists them all.
///
/// The rows span the `RouterOutputFilter` union: every member the model declares, a member
/// naming several accepted values, several members at once, a value no stored output carries,
/// and a member naming no value at all.
#[tokio::test]
async fn filters_report_only_the_router_outputs_that_satisfy_them() {
    let emulator = Emulator::new();
    let fixtures = fixtures(&emulator).await;
    let interface_arn = fixtures[0]
        .network_interface_arn
        .clone()
        .expect("the standard fixture egresses from an interface");
    let other_interface_arn = fixtures[1]
        .network_interface_arn
        .clone()
        .expect("the second standard fixture egresses from an interface");

    // A routed output, so the `RoutedInputArns` member has something to select.
    let rows = minted_configuration_rows(&emulator, "ro-filter-input").await;
    let (_, _, _, input_configuration, _) = rows
        .into_iter()
        .next()
        .expect("the router input configuration rows are not empty");
    let (status, _, created_input) = create_router_input(
        &emulator,
        router_input_body("ri-ro-filter", input_configuration),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture router input is created, got {created_input}"
    );
    let input_arn = created_input["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let unrouted_input_arn =
        "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:ffffffffffff".to_string();
    record_route_and_messages(&emulator, &fixtures[2].arn, &input_arn, 0);

    tokio::time::sleep(past_transition_span()).await;
    // One listing materialises the state changes the clock has already made due — its only
    // write — so the snapshot every filtered listing below is compared against is taken
    // after it.
    page(&emulator, "settling listing", "?maxResults=100", None).await;
    let before = stored_router_outputs(&emulator);

    let cases: Vec<(FilterRow, Value)> = vec![
        (
            FilterRow {
                case: "no filter at all",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "mediaconnect-flow",
                    "medialive-input",
                ],
            },
            json!([]),
        ),
        (
            FilterRow {
                case: "RegionNames naming the cross-Region output's Region",
                expected: &["mediaconnect-flow"],
            },
            json!([{ "RegionNames": [OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "RegionNames naming both Regions",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "mediaconnect-flow",
                    "medialive-input",
                ],
            },
            json!([{ "RegionNames": [REGION, OTHER_REGION] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceArns naming one interface",
                expected: &["standard/rtp"],
            },
            json!([{ "NetworkInterfaceArns": [&interface_arn] }]),
        ),
        (
            FilterRow {
                case: "NetworkInterfaceArns naming both interfaces",
                expected: &["standard/rtp", "standard/srt-listener"],
            },
            json!([{ "NetworkInterfaceArns": [&interface_arn, &other_interface_arn] }]),
        ),
        (
            FilterRow {
                case: "RoutingScopes naming GLOBAL",
                expected: &["standard/srt-listener", "mediaconnect-flow"],
            },
            json!([{ "RoutingScopes": ["GLOBAL"] }]),
        ),
        (
            FilterRow {
                case: "RoutingScopes naming REGIONAL",
                expected: &["standard/rtp", "medialive-input"],
            },
            json!([{ "RoutingScopes": ["REGIONAL"] }]),
        ),
        (
            FilterRow {
                case: "OutputTypes naming one type",
                expected: &["medialive-input"],
            },
            json!([{ "OutputTypes": [mc::ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT] }]),
        ),
        (
            FilterRow {
                case: "OutputTypes naming two types",
                expected: &["standard/rtp", "standard/srt-listener", "mediaconnect-flow"],
            },
            json!([{ "OutputTypes": [
                mc::ROUTER_OUTPUT_TYPE_STANDARD,
                mc::ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
            ] }]),
        ),
        (
            FilterRow {
                case: "RoutedInputArns naming the routed input",
                expected: &["mediaconnect-flow"],
            },
            json!([{ "RoutedInputArns": [&input_arn] }]),
        ),
        (
            FilterRow {
                case: "RoutedInputArns naming an input nothing is routed from",
                expected: &[],
            },
            json!([{ "RoutedInputArns": [&unrouted_input_arn] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment one name carries",
                expected: &["standard/srt-listener"],
            },
            json!([{ "NameContains": ["standard-srt"] }]),
        ),
        (
            FilterRow {
                case: "NameContains a fragment every name carries",
                expected: &[
                    "standard/rtp",
                    "standard/srt-listener",
                    "mediaconnect-flow",
                    "medialive-input",
                ],
            },
            json!([{ "NameContains": ["ro-listing-"] }]),
        ),
        (
            FilterRow {
                case: "NameContains two fragments",
                expected: &["standard/rtp", "medialive-input"],
            },
            json!([{ "NameContains": ["standard-rtp", "medialive"] }]),
        ),
        (
            FilterRow {
                case: "two members at once, both satisfied by one output",
                expected: &["standard/srt-listener"],
            },
            json!([
                { "RoutingScopes": ["GLOBAL"] },
                { "OutputTypes": [mc::ROUTER_OUTPUT_TYPE_STANDARD] },
            ]),
        ),
        (
            FilterRow {
                case: "two members at once that no output satisfies together",
                expected: &[],
            },
            json!([
                { "RoutingScopes": ["REGIONAL"] },
                { "OutputTypes": [mc::ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW] },
            ]),
        ),
        (
            FilterRow {
                case: "a Region no stored output is in",
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
        let expected = expected_arns(&fixtures, expected);
        let body = page(&emulator, case, "?maxResults=100", Some(filters.clone())).await;
        assert_eq!(
            listed_arns(&body),
            expected,
            "case {case:?}: the filter {filters} reports exactly the matching outputs, got \
             {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: a page holding every match carries no token, got {body}"
        );
    }

    // Filtering is a read like any other: it stores nothing.
    let after = stored_router_outputs(&emulator);
    assert_eq!(
        after, before,
        "a filtered listing must store nothing; expected {before}, got {after}"
    );
}

/// The listing pages every matching router output exactly once, whatever `MaxResults` is,
/// and the token is the ARN the next page resumes at. With no `MaxResults` the page holds as
/// many as the modelled range allows.
#[tokio::test]
async fn pages_partition_every_router_output_exactly_once() {
    let emulator = Emulator::new();
    // More than the largest page the modelled `MaxResults` range allows, so the page a
    // request naming no size gets is observably bounded. The store ceiling is a bound on
    // what is STORED, so these records go in through the store's own channel; the calls
    // under test still go over the wire.
    seed_router_outputs(&emulator, 130);
    let all = stored_arns(&emulator);
    assert_eq!(all.len(), 130, "every seeded output is stored, got {all:?}");

    // `MaxResults` is "the maximum number of router outputs to return in the response" and
    // the model documents no smaller default, so a request naming none is answered with the
    // maximum of the modelled range (1..100).
    let default_page = page(&emulator, "the default page size", "", None).await;
    assert_eq!(
        listed_arns(&default_page),
        all[..100].to_vec(),
        "the default page holds the first hundred outputs, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(all[100].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled range: its lower bound, sizes between, and its upper
    // bound — each walked with no filter and under one, because filtering and paging
    // compose. The filter keeps every seeded output, so the two walks must agree.
    let filter = json!([{ "NameContains": ["seeded-router-output-"] }]);
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
                "case {case:?}: the pages must partition the stored outputs once, in order"
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

/// An account that holds no router output is listed empty — not a 404 and not a page of
/// invented outputs.
#[tokio::test]
async fn an_account_that_holds_no_router_output_is_listed_empty() {
    let emulator = Emulator::new();
    for (case, filters) in [
        ("no filter", None),
        ("a filter", Some(json!([{ "RoutingScopes": ["REGIONAL"] }]))),
    ] {
        let body = page(&emulator, case, "", filters).await;
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: an account holding no router output lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );
    }
}

/// The listed `State` and maintenance window are the CURRENT ones: a listing materialises
/// only the change the clock already made due, so an output whose creation is still in
/// flight is listed `CREATING`, the transition span settles it into `STANDBY`, and starting
/// it is what schedules the window the summary then reports.
#[tokio::test]
async fn the_listed_state_is_the_one_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let targets = minted_configuration_targets(&emulator, "ro-listing-clock").await;
    let arn = minted_output(
        &emulator,
        "clock",
        json!({
            "name": "ro-listing-clock",
            "configuration": standard_with_protocol(&targets.interface_arn, json!({
                "Rtp": { "destinationAddress": "203.0.113.40", "destinationPort": 5006 }
            }), "RTP"),
            "maximumBitrate": 20_000_000_i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
        }),
    )
    .await;

    let body = page(&emulator, "inside the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_OUTPUT_STATE_CREATING),
        "inside the span the creation is still in flight, got {body}"
    );
    assert!(
        listed(&body)
            .first()
            .is_some_and(|summary| summary["maintenanceSchedule"].is_null()),
        "nothing has scheduled a window for an output that was never started, got {body}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "past the transition span", "", None).await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["state"].as_str()),
        Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        "once the span has passed the creation is settled, got {body}"
    );

    // Starting the output schedules its maintenance window, and the summary reports that
    // window and the variant of the schedule union beside it.
    let (status, code, started) = emulator
        .send("POST", &format!("/v1/routerOutput/start/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "StartRouterOutput accepts the start, got {status} {code:?}: {started}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "a started output", "", None).await;
    let summary = listed(&body)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the started output is listed: {body}"));
    assert_eq!(
        (
            summary["state"].as_str(),
            summary["maintenanceScheduleType"].as_str(),
        ),
        (
            Some(mc::ROUTER_OUTPUT_STATE_ACTIVE),
            Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
        ),
        "a started output is listed ACTIVE with the window type it was given, got {summary}"
    );
    assert_eq!(
        summary["maintenanceSchedule"], started["maintenanceSchedule"],
        "the listed window is the one the start answered, got {summary}"
    );

    // A deleted router output leaves the store once the clock completes the delete, and the
    // listing stops reporting it rather than reporting a record nothing holds.
    let (status, code, deleting) = emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DeleteRouterOutput accepts the delete, got {status} {code:?}: {deleting}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let body = page(&emulator, "a completed delete", "", None).await;
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "an output the store no longer holds is not listed, got {body}"
    );
}

/// One refused ListRouterOutputs request.
struct RefusalCase {
    case: &'static str,
    query: &'static str,
}

/// ListRouterOutputs refuses a page size outside the modelled range and a pagination token
/// it could not have issued — with the modelled `BadRequestException` (400), never with a
/// 501 and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    // The modelled `MaxResults` range is 1..100, so each bound is usable (the paging suite
    // above lists with 1 and with 100) and each value past it is refused. A `NextToken` is
    // the router output ARN the listing resumes at, so a token the service could not have
    // minted is refused; the rows are the ways a token can fail that form.
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
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:routerOutput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerInput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose account id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:routerOutput:a1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose output id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:a1b2c3d4e5f",
        },
        RefusalCase {
            case: "a nextToken whose output id carries an upper-case character",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:A1b2c3d4e5f6",
        },
        RefusalCase {
            case: "a nextToken whose output id is empty",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:",
        },
    ];

    for RefusalCase { case, query } in CASES {
        let (status, code, body) = list_router_outputs(&emulator, query, None).await;
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

    // `Filters` is bounded as well: every stored output is tested against every filter and
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
        let (status, code, body) = list_router_outputs(&emulator, "", Some(filters)).await;
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

    // A token the service COULD have minted is accepted even when no such output is stored:
    // paging must survive a delete between pages.
    let unknown = "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff";
    let body = page(
        &emulator,
        "a well-formed token for an unknown output",
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
