// SPDX-License-Identifier: CC0-1.0

//! What the ROUTING SCOPES and the REGIONS of a router input and a router output admit,
//! end to end over HTTP.
//!
//! The rule, stated from both sides by the model: a REGIONAL router input "connects only to
//! outputs in same Region" (`CreateRouterInputRequest$RoutingScope`) and a REGIONAL router
//! output "can only take inputs from same Region" (`CreateRouterOutputRequest$RoutingScope`),
//! while GLOBAL puts no such bound on the side that carries it. So a pair in ONE Region may
//! be associated whatever the two scopes are, and a pair that spans two Regions may be
//! associated only when NEITHER side is REGIONAL.
//!
//! 1. TakeRouterInput records an association only for a pair the rule admits, and refuses one
//!    it does not with the modelled `BadRequestException` (400) in this service's
//!    semantic-refusal envelope. A refusal persists nothing: the output stays `UNROUTED` and
//!    the input keeps counting no routed output.
//!
//! 2. The same rule bounds what an update may store: narrowing a `RoutingScope` to REGIONAL
//!    is refused while the record is part of an association across Regions that scope
//!    forbids, and accepted once that association is gone — or when the pair is in one Region
//!    to begin with.
//!
//! The rows vary what the rule generalises over: each `RoutingScope` member on each side,
//! the Region each side lives in (the signing Region and two others), whether the request
//! names a Region at all, and which side of the pair is the one bound to its Region.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine) and every
//! request goes through the helpers the router suites own, so this suite cannot drift from
//! the wire shape they pin.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ROUTING_SCOPE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, past_transition_span,
};
use crate::router_output_configuration_targets::update_router_output;
use crate::router_output_take_input::take_router_input;
use crate::router_outputs::{create_router_output, minted_network_interface, standard};
use crate::update_router_input::update_router_input;

/// Two Regions that are not the one the test harness signs for, so a row can put either
/// side of a pair outside the signing Region and a row can span two Regions neither of
/// which is it.
const OTHER_REGION: &str = "eu-west-1";
const THIRD_REGION: &str = "ap-south-1";

/// A `RoutingScope` that binds a resource to its own Region, and the one that does not —
/// the two members `RoutingScope` declares
/// (`smithy_query(query="shape",shape="RoutingScope")`), asserted against the catalog the
/// crate generates from the model in [`the_rows_span_every_routing_scope`].
const REGIONAL: &str = "REGIONAL";
const GLOBAL: &str = "GLOBAL";

/// Send one GetRouterOutput request.
async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// `(RoutedState, RoutedInputArn)` as GetRouterOutput reports them.
fn reported_route(body: &Value) -> (&str, Option<&str>) {
    let output = body.get("routerOutput").unwrap_or(&Value::Null);
    (
        output
            .get("routedState")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        output.get("routedInputArn").and_then(Value::as_str),
    )
}

/// `RouterInput$RoutedOutputs` as GetRouterInput reports it — the other side of the same
/// association.
fn reported_routed_outputs(body: &Value) -> i64 {
    body.get("routerInput")
        .and_then(|router_input| router_input.get("routedOutputs"))
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// `RoutingScope` as a read reports it for a router input.
async fn input_routing_scope(emulator: &Emulator, arn: &str) -> String {
    let (_, _, body) = get_router_input(emulator, arn).await;
    body["routerInput"]["routingScope"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `RoutingScope` as a read reports it for a router output.
async fn output_routing_scope(emulator: &Emulator, arn: &str) -> String {
    let (_, _, body) = get_router_output(emulator, arn).await;
    body["routerOutput"]["routingScope"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create one router input with `routing_scope`, in `region` when the row names one, and
/// answer its ARN. The configuration is the smallest CreateRouterInput accepts, over a
/// router network interface this account holds.
async fn create_input(
    emulator: &Emulator,
    case: &str,
    name: &str,
    interface_arn: &str,
    routing_scope: &str,
    region: Option<&str>,
) -> String {
    let mut body = create_body(
        name,
        json!({
            "Standard": {
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": { "Rtp": { "port": 5000 } },
            }
        }),
    );
    body["routingScope"] = json!(routing_scope);
    if let Some(region) = region {
        body["regionName"] = json!(region);
    }
    let (status, code, created) = create_router_input(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterInput answers 201, got {status} {code:?} with {created:?}"
    );
    created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create one router output with `routing_scope`, in `region` when the row names one, and
/// answer its ARN.
async fn create_output(
    emulator: &Emulator,
    case: &str,
    name: &str,
    interface_arn: &str,
    routing_scope: &str,
    region: Option<&str>,
) -> String {
    let mut body = json!({
        "name": name,
        "configuration": standard(
            interface_arn,
            json!({ "Rtp": { "destinationAddress": "203.0.113.40", "destinationPort": 5004 } }),
        ),
        "maximumBitrate": 20_000_000_i64,
        "routingScope": routing_scope,
        "tier": "OUTPUT_20",
    });
    if let Some(region) = region {
        body["regionName"] = json!(region);
    }
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterOutput answers 201, got {status} {code:?} with {created:?}"
    );
    created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row: the scope and the Region each side of the pair carries, and whether the rule
/// admits their association. `None` for a Region is a request that names none, which takes
/// the Region the call was signed for.
struct Row {
    case: &'static str,
    input_scope: &'static str,
    input_region: Option<&'static str>,
    output_scope: &'static str,
    output_region: Option<&'static str>,
    admitted: bool,
}

/// The table: every `RoutingScope` member on each side, crossed with pairs that share a
/// Region and pairs that do not — including one spanning two Regions neither of which is
/// the signing Region, and one whose Region the request leaves to the signing Region.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both REGIONAL, neither naming a Region",
            input_scope: REGIONAL,
            input_region: None,
            output_scope: REGIONAL,
            output_region: None,
            admitted: true,
        },
        Row {
            case: "both REGIONAL in the same named Region",
            input_scope: REGIONAL,
            input_region: Some(OTHER_REGION),
            output_scope: REGIONAL,
            output_region: Some(OTHER_REGION),
            admitted: true,
        },
        Row {
            case: "both GLOBAL in the signing Region",
            input_scope: GLOBAL,
            input_region: None,
            output_scope: GLOBAL,
            output_region: None,
            admitted: true,
        },
        Row {
            case: "GLOBAL input, REGIONAL output, one Region",
            input_scope: GLOBAL,
            input_region: Some(THIRD_REGION),
            output_scope: REGIONAL,
            output_region: Some(THIRD_REGION),
            admitted: true,
        },
        Row {
            case: "REGIONAL input, GLOBAL output, one Region",
            input_scope: REGIONAL,
            input_region: Some(THIRD_REGION),
            output_scope: GLOBAL,
            output_region: Some(THIRD_REGION),
            admitted: true,
        },
        Row {
            case: "both GLOBAL across two Regions",
            input_scope: GLOBAL,
            input_region: None,
            output_scope: GLOBAL,
            output_region: Some(OTHER_REGION),
            admitted: true,
        },
        Row {
            case: "both REGIONAL across two Regions",
            input_scope: REGIONAL,
            input_region: None,
            output_scope: REGIONAL,
            output_region: Some(OTHER_REGION),
            admitted: false,
        },
        Row {
            case: "REGIONAL input, GLOBAL output in another Region",
            input_scope: REGIONAL,
            input_region: None,
            output_scope: GLOBAL,
            output_region: Some(OTHER_REGION),
            admitted: false,
        },
        Row {
            case: "GLOBAL input, REGIONAL output in another Region",
            input_scope: GLOBAL,
            input_region: None,
            output_scope: REGIONAL,
            output_region: Some(OTHER_REGION),
            admitted: false,
        },
        Row {
            case: "both REGIONAL, in two Regions neither of them the signing one",
            input_scope: REGIONAL,
            input_region: Some(OTHER_REGION),
            output_scope: REGIONAL,
            output_region: Some(THIRD_REGION),
            admitted: false,
        },
    ]
}

/// The two scopes the rows name are exactly the `RoutingScope` members the model declares,
/// read from the catalog the crate GENERATES from that model, and each appears on each side
/// of the pair — so the table cannot assert a value the API does not have, nor leave a
/// member of it untested.
#[test]
fn the_rows_span_every_routing_scope() {
    let mut named = [REGIONAL, GLOBAL];
    named.sort_unstable();
    let mut declared = ROUTING_SCOPE_VALUES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        named.to_vec(),
        declared,
        "the suite names every RoutingScope the model declares, got {ROUTING_SCOPE_VALUES:?}"
    );
    for scope in ROUTING_SCOPE_VALUES {
        assert!(
            rows().iter().any(|row| row.input_scope == *scope),
            "scope {scope:?} must appear on the router input side of the table"
        );
        assert!(
            rows().iter().any(|row| row.output_scope == *scope),
            "scope {scope:?} must appear on the router output side of the table"
        );
    }
    assert!(
        rows().iter().any(|row| row.admitted) && rows().iter().any(|row| !row.admitted),
        "the table must hold both admitted and refused pairs"
    );
}

/// A take records the association only for a pair the two sides' scopes and Regions admit,
/// and refuses one they do not — persisting nothing on either side.
#[tokio::test]
async fn a_take_associates_only_a_pair_the_scopes_and_regions_admit() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-region-scope").await;
    let rows = rows();

    // Every pair is created first and the clock is given one span to settle them, so a take
    // below is judged on the Regions and the scopes rather than on a create still in flight.
    let mut pairs: Vec<(&Row, String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let input_arn = create_input(
            &emulator,
            row.case,
            &format!("region-scope-input-{index}"),
            &interface_arn,
            row.input_scope,
            row.input_region,
        )
        .await;
        let output_arn = create_output(
            &emulator,
            row.case,
            &format!("region-scope-output-{index}"),
            &interface_arn,
            row.output_scope,
            row.output_region,
        )
        .await;
        pairs.push((row, input_arn, output_arn));
    }
    tokio::time::sleep(past_transition_span()).await;

    for (row, input_arn, output_arn) in &pairs {
        let case = row.case;
        let (status, code, body) = take_router_input(&emulator, output_arn, Some(input_arn)).await;
        if row.admitted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the pair is admitted, so the take answers 202, got {status} \
                 {code:?} with {body:?}"
            );
            assert_eq!(
                body.get("routedState").and_then(Value::as_str),
                Some(mc::ROUTED_STATE_ROUTING),
                "case {case:?}: an accepted take answers the transient ROUTING, got {body:?}"
            );
            continue;
        }

        // The refusal: the modelled `BadRequestException` (400), carrying the one sentence
        // the rule owns, with both sides' scope and Region in it.
        let detail = mc::cross_region_association_detail(
            input_arn,
            row.input_scope,
            row.input_region.unwrap_or(REGION),
            output_arn,
            row.output_scope,
            row.output_region.unwrap_or(REGION),
        );
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the pair is not admitted, so the take is refused, got {status} \
             {code:?} with {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(mc::bad_request_message(&detail).as_str()),
            "case {case:?}: the refusal states the rule over the two records, got {body:?}"
        );

        // And it persisted nothing on either side of the pair it refused.
        let (status, code, output) = get_router_output(&emulator, output_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_route(&output)),
            (200, None, (mc::ROUTED_STATE_UNROUTED, None)),
            "case {case:?}: the refused take left the output unrouted, got {status} {code:?} \
             with {output:?}"
        );
        let (status, code, read_input) = get_router_input(&emulator, input_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_routed_outputs(&read_input)),
            (200, None, 0),
            "case {case:?}: the refused take left the input counting no routed output, got \
             {status} {code:?} with {read_input:?}"
        );
    }

    // The accepted associations settle, and both sides report the same one association.
    tokio::time::sleep(past_transition_span()).await;
    for (row, input_arn, output_arn) in &pairs {
        let case = row.case;
        let (_, _, output) = get_router_output(&emulator, output_arn).await;
        let (_, _, read_input) = get_router_input(&emulator, input_arn).await;
        let expected = if row.admitted {
            (mc::ROUTED_STATE_ROUTED, Some(input_arn.as_str()), 1)
        } else {
            (mc::ROUTED_STATE_UNROUTED, None, 0)
        };
        assert_eq!(
            (
                reported_route(&output).0,
                reported_route(&output).1,
                reported_routed_outputs(&read_input),
            ),
            expected,
            "case {case:?}: both sides report the association the rule admits, got {output:?} \
             and {read_input:?}"
        );
    }
}

/// The same rule bounds an UPDATE: a `RoutingScope` cannot be narrowed to REGIONAL while the
/// record is part of an association across Regions that scope forbids, on either side — and
/// the same request is accepted once the association is gone, or when the pair never spanned
/// two Regions at all.
#[tokio::test]
async fn a_routing_scope_cannot_be_narrowed_under_a_cross_region_association() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-narrowing").await;

    // A pair spanning two Regions, which only two GLOBAL scopes admit, and a pair in one
    // Region, which any scope admits.
    let crossing_input = create_input(
        &emulator,
        "crossing",
        "narrowing-crossing-input",
        &interface_arn,
        GLOBAL,
        None,
    )
    .await;
    let crossing_output = create_output(
        &emulator,
        "crossing",
        "narrowing-crossing-output",
        &interface_arn,
        GLOBAL,
        Some(OTHER_REGION),
    )
    .await;
    let local_input = create_input(
        &emulator,
        "one Region",
        "narrowing-local-input",
        &interface_arn,
        GLOBAL,
        None,
    )
    .await;
    let local_output = create_output(
        &emulator,
        "one Region",
        "narrowing-local-output",
        &interface_arn,
        GLOBAL,
        None,
    )
    .await;
    tokio::time::sleep(past_transition_span()).await;

    for (case, output_arn, input_arn) in [
        ("crossing", &crossing_output, &crossing_input),
        ("one Region", &local_output, &local_input),
    ] {
        let (status, code, body) = take_router_input(&emulator, output_arn, Some(input_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: both sides are GLOBAL, so the take is accepted, got {status} \
             {code:?} with {body:?}"
        );
    }
    tokio::time::sleep(past_transition_span()).await;

    // The sentence the rule owns, for the pair the narrowing would leave illegal — the
    // refusal states it whichever side the request narrows.
    let narrowed_input_detail = mc::cross_region_association_detail(
        &crossing_input,
        REGIONAL,
        REGION,
        &crossing_output,
        GLOBAL,
        OTHER_REGION,
    );
    let narrowed_output_detail = mc::cross_region_association_detail(
        &crossing_input,
        GLOBAL,
        REGION,
        &crossing_output,
        REGIONAL,
        OTHER_REGION,
    );

    let (status, code, refusal) = update_router_input(
        &emulator,
        &crossing_input,
        json!({ "routingScope": REGIONAL }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "narrowing a routed input's RoutingScope across Regions is refused, got {status} \
         {code:?} with {refusal:?}"
    );
    let message = refusal["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::bad_request_message(&narrowed_input_detail).as_str()),
        "case \"input narrowed\": the refusal states the rule over the standing association, \
         got {message:?}"
    );
    assert_eq!(
        input_routing_scope(&emulator, &crossing_input).await,
        GLOBAL,
        "case \"input narrowed\": the refused update left the stored RoutingScope alone"
    );

    let (status, code, refusal) = update_router_output(
        &emulator,
        &crossing_output,
        json!({ "routingScope": REGIONAL }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "narrowing a routed output's RoutingScope across Regions is refused, got {status} \
         {code:?} with {refusal:?}"
    );
    let message = refusal["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::bad_request_message(&narrowed_output_detail).as_str()),
        "case \"output narrowed\": the refusal states the rule over the standing association, \
         got {message:?}"
    );
    assert_eq!(
        output_routing_scope(&emulator, &crossing_output).await,
        GLOBAL,
        "case \"output narrowed\": the refused update left the stored RoutingScope alone"
    );
    let (_, _, output) = get_router_output(&emulator, &crossing_output).await;
    assert_eq!(
        reported_route(&output),
        (mc::ROUTED_STATE_ROUTED, Some(crossing_input.as_str())),
        "the refused updates left the association standing, got {output:?}"
    );

    // The pair that lives in ONE Region takes the same narrowing: a shared Region admits
    // either scope.
    for (case, arn, narrowed) in [("input", &local_input, true), ("output", &local_output, false)] {
        let (status, code, answered) = if narrowed {
            update_router_input(&emulator, arn, json!({ "routingScope": REGIONAL })).await
        } else {
            update_router_output(&emulator, arn, json!({ "routingScope": REGIONAL })).await
        };
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a pair in one Region takes the narrowing, got {status} {code:?} \
             with {answered:?}"
        );
    }

    // And the crossing pair takes it too, once the association it would have broken is gone.
    let (status, code, cleared) = take_router_input(&emulator, &crossing_output, None).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the clearing take is accepted, got {status} {code:?} with {cleared:?}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, answered) = update_router_input(
        &emulator,
        &crossing_input,
        json!({ "routingScope": REGIONAL }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "an unrouted input takes the narrowing, got {status} {code:?} with {answered:?}"
    );
    assert_eq!(
        input_routing_scope(&emulator, &crossing_input).await,
        REGIONAL,
        "the accepted update stored the narrower RoutingScope"
    );
    let (status, code, answered) = update_router_output(
        &emulator,
        &crossing_output,
        json!({ "routingScope": REGIONAL }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "an unrouted output takes the narrowing, got {status} {code:?} with {answered:?}"
    );
    assert_eq!(
        output_routing_scope(&emulator, &crossing_output).await,
        REGIONAL,
        "the accepted update stored the narrower RoutingScope on the output"
    );

    // With both sides REGIONAL in two Regions, the association cannot be recorded again.
    let (status, code, refused) =
        take_router_input(&emulator, &crossing_output, Some(&crossing_input)).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "the narrowed pair can no longer be associated, got {status} {code:?} with {refused:?}"
    );
}
