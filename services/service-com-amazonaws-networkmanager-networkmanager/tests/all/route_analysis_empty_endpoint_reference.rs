// SPDX-License-Identifier: CC0-1.0

//! The rule this crate gives every OPTIONAL reference member, pinned on the two endpoints
//! of a route analysis: the empty string names no resource, so it resolves nothing and is
//! held as none — exactly as omitting the member is — while any non-empty value stays a
//! reference that must name an attachment of the same global network.
//!
//! `RouteAnalysisEndpointOptionsSpecification$TransitGatewayAttachmentArn` is modelled
//! `length {min: 0, max: 500}` over `^[\s\S]*$` (`smithy_query query="shape"
//! shape="RouteAnalysisEndpointOptionsSpecification"`), so the empty string satisfies the
//! constraint walk and reaches the handler. The table below therefore drives each request
//! TWICE — once with the endpoint's ARN spelled empty and once with it omitted — and
//! requires the two analyses to agree member for member: the same completion reason code,
//! the same (absent) reason context, and endpoint options reporting no attachment ARN at
//! all. `IpAddress` is not probed this way: it is modelled `length {min: 1, max: 50}`, so
//! the empty string never reaches the handler on that member.
//!
//! The second test is the other half of the rule: a NON-EMPTY ARN — a blank one included,
//! which is a value rather than the absence of one — is still a reference, reported back
//! verbatim and named in the reason context of the path it stopped.
//!
//! The rows span what the rule generalises over: which endpoint carries the empty string
//! (the source, the destination, both), what the OTHER endpoint names (nothing, an ARN no
//! global network holds, an attachment this global network does hold), whether an IP
//! address is given, and the two flags. Nothing reaches into `crate::state`: every analysis
//! is read back through `GetRouteAnalysis`.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::empty_optional_reference::without;
use crate::global_network_lifecycle::{expected_arn, Emulator, PAST_SETTLE};

/// The Region the fixture's core network configures an edge location in, and so the Region
/// of the attached VPC.
const REGION: &str = "us-west-2";

/// The endpoint member a reason context keys its entry under
/// (`<member>.TransitGatewayAttachmentArn`).
const SOURCE: &str = "Source";
/// See [`SOURCE`].
const DESTINATION: &str = "Destination";

/// The members two analyses of the same request never share: the minted id and the instant
/// the call ran at.
const VOLATILE: &[&str] = &["RouteAnalysisId", "StartTimestamp"];

/// An Amazon VPC transit gateway attachment ARN. Nothing registers a transit gateway with a
/// global network here, so no such ARN names a resource any global network holds.
fn transit_gateway_attachment_arn(index: usize) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:transit-gateway-attachment/tgw-attach-{index:017x}",
        engine::emulator_account_id()
    )
}

/// The `StartRouteAnalysis` URI of `network`.
fn analyses_uri(network: &str) -> String {
    format!("/global-networks/{network}/route-analyses")
}

/// The `GetRouteAnalysis` URI of `analysis` under `network`.
fn analysis_uri(network: &str, analysis: &str) -> String {
    format!("/global-networks/{network}/route-analyses/{analysis}")
}

/// An endpoint body naming only an IP address — no attachment ARN member at all.
fn address_only(address: &str) -> Value {
    json!({ "IpAddress": address })
}

/// An endpoint body naming an attachment ARN and no IP address.
fn at(arn: &str) -> Value {
    json!({ "TransitGatewayAttachmentArn": arn })
}

/// `endpoint` with `TransitGatewayAttachmentArn` spelled as the empty string.
fn with_empty_arn(endpoint: &Value) -> Value {
    let mut spelled = endpoint.clone();
    spelled["TransitGatewayAttachmentArn"] = json!("");
    spelled
}

/// One row: the request as it reads with the empty ARN OMITTED, which endpoints spell it
/// empty in the other request, and the completion both must report.
struct Row {
    /// Names the case in every assertion message.
    case: &'static str,
    source: Value,
    destination: Value,
    /// The endpoints whose `TransitGatewayAttachmentArn` the first request spells as the
    /// empty string.
    empty: &'static [&'static str],
    /// Extra top-level request members (the two flags), or an empty object for none.
    extra: Value,
    /// The `RouteAnalysisCompletion$ReasonCode` the completed forward path must carry.
    reason: &'static str,
}

impl Row {
    /// The request body of this row, with the ARN of each endpoint in `empty` spelled as the
    /// empty string when `spell_empty`, and omitted when not.
    fn body(&self, spell_empty: bool) -> Value {
        let endpoint = |member: &str, base: &Value| {
            if spell_empty && self.empty.contains(&member) {
                with_empty_arn(base)
            } else {
                base.clone()
            }
        };
        let mut body = json!({
            SOURCE: endpoint(SOURCE, &self.source),
            DESTINATION: endpoint(DESTINATION, &self.destination),
        });
        for (member, value) in self.extra.as_object().expect("the extras are an object") {
            body[member] = value.clone();
        }
        body
    }
}

/// Start one analysis and answer `(id, the analysis as the start reported it)`.
async fn start(emulator: &Emulator, case: &str, network: &str, body: Value) -> (String, Value) {
    let (status, answer) = emulator.post(&analyses_uri(network), body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartRouteAnalysis({body}) expected 200, got {status} with {answer}"
    );
    let analysis = answer["RouteAnalysis"].clone();
    let id = analysis["RouteAnalysisId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RouteAnalysisId in {answer}"))
        .to_string();
    (id, analysis)
}

/// Read one analysis back through `GetRouteAnalysis`.
async fn read(emulator: &Emulator, case: &str, network: &str, analysis: &str) -> Value {
    let (status, body) = emulator.get(&analysis_uri(network, analysis)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRouteAnalysis({analysis}) expected 200, got {status} with {body}"
    );
    body["RouteAnalysis"].clone()
}

/// The global network every analysis here is started on, plus the ARN of a Cloud WAN
/// attachment it holds — an attachment of this global network that is not one on a transit
/// gateway.
async fn fixture(emulator: &Emulator) -> (String, String) {
    let (network, core_network) =
        core_network_at(emulator, "route-analysis-empty-reference", &[REGION]).await;
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(REGION, 0x5201),
            "SubnetArns": [subnet_arn(REGION, 0x5201)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment for the fixture: expected 200, got {status} with {created}"
    );
    let attachment = attachment_id("fixture", &created["VpcAttachment"]);
    (network, expected_arn("attachment", &attachment))
}

fn rows(held_attachment_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "the destination's ARN is the empty string, the source names an unheld one",
            source: at(&transit_gateway_attachment_arn(1)),
            destination: address_only("10.0.1.11"),
            empty: &[DESTINATION],
            extra: json!({}),
            // The endpoint the path runs TOWARDS names no attachment ARN, so there is no
            // destination to analyse a path to.
            reason: "NO_DESTINATION_ARN_PROVIDED",
        },
        Row {
            case: "the destination's ARN is the empty string and neither endpoint names an address",
            source: json!({}),
            destination: json!({}),
            empty: &[DESTINATION],
            extra: json!({}),
            reason: "NO_DESTINATION_ARN_PROVIDED",
        },
        Row {
            case: "the destination's ARN is the empty string, the source an attachment this global network holds",
            source: at(held_attachment_arn),
            destination: address_only("10.0.2.13"),
            empty: &[DESTINATION],
            extra: json!({}),
            reason: "NO_DESTINATION_ARN_PROVIDED",
        },
        Row {
            case: "both endpoints' ARNs are the empty string",
            source: address_only("10.0.3.15"),
            destination: address_only("10.0.4.17"),
            empty: &[SOURCE, DESTINATION],
            extra: json!({ "IncludeReturnPath": true }),
            reason: "NO_DESTINATION_ARN_PROVIDED",
        },
        Row {
            case: "the source's ARN is the empty string, the destination names an unheld one",
            source: address_only("10.0.5.19"),
            destination: at(&transit_gateway_attachment_arn(2)),
            empty: &[SOURCE],
            extra: json!({}),
            // The path starts at the source, which names no attachment of this global
            // network, so it cannot leave it.
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
        },
        Row {
            case: "the source's ARN is the empty string, the destination an attachment this global network holds",
            source: json!({}),
            destination: at(held_attachment_arn),
            empty: &[SOURCE],
            extra: json!({ "IncludeReturnPath": true, "UseMiddleboxes": true }),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
        },
    ]
}

#[tokio::test]
async fn an_empty_endpoint_attachment_arn_reads_as_no_attachment_arn() {
    let emulator = Emulator::new();
    let (network, held_attachment_arn) = fixture(&emulator).await;

    // Every analysis is started first, so the whole table waits once for the transition the
    // clock owes rather than once per row.
    let table = rows(&held_attachment_arn);
    let mut started: Vec<(&Row, String, String)> = Vec::new();
    for row in &table {
        let (empty_id, empty) = start(&emulator, row.case, &network, row.body(true)).await;
        let (omitted_id, omitted) = start(&emulator, row.case, &network, row.body(false)).await;

        for member in row.empty {
            assert_eq!(
                empty[*member]["TransitGatewayAttachmentArn"],
                Value::Null,
                "case {:?}: the {member} ARN spelled empty must name no attachment, got {empty}",
                row.case
            );
            assert_eq!(
                omitted[*member]["TransitGatewayAttachmentArn"],
                Value::Null,
                "case {:?}: the {member} ARN omitted must name no attachment, got {omitted}",
                row.case
            );
        }
        assert_eq!(
            without(&empty, VOLATILE),
            without(&omitted, VOLATILE),
            "case {:?}: the empty spelling of {:?} must answer what omitting it answers, got \
             {empty} against {omitted}",
            row.case,
            row.empty
        );
        started.push((row, empty_id, omitted_id));
    }

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    for (row, empty_id, omitted_id) in &started {
        let empty = read(&emulator, row.case, &network, empty_id).await;
        let omitted = read(&emulator, row.case, &network, omitted_id).await;

        // ROUND TRIP: the STORED analysis is what the reader answers, so the empty spelling
        // resolved nothing and was held as none rather than only looking that way in the
        // start's answer.
        for member in row.empty {
            assert_eq!(
                empty[*member]["TransitGatewayAttachmentArn"],
                Value::Null,
                "case {:?}: the stored {member} must hold no attachment ARN, got {empty}",
                row.case
            );
        }
        let completion = &empty["ForwardPath"]["CompletionStatus"];
        assert_eq!(
            completion["ResultCode"],
            json!("NOT_CONNECTED"),
            "case {:?}: expected NOT_CONNECTED, got {empty}",
            row.case
        );
        assert_eq!(
            completion["ReasonCode"],
            json!(row.reason),
            "case {:?}: expected reason {}, got {empty}",
            row.case,
            row.reason
        );
        // An endpoint that named no ARN at all has nothing further to report, so the
        // completion carries no reason context: every row above stops at an endpoint whose
        // ARN is the empty string or absent.
        assert_eq!(
            completion["ReasonContext"],
            Value::Null,
            "case {:?}: an endpoint naming no ARN must leave no reason context, got {empty}",
            row.case
        );
        assert_eq!(
            without(&empty, VOLATILE),
            without(&omitted, VOLATILE),
            "case {:?}: the stored empty spelling of {:?} must read back as the stored \
             omission, got {empty} against {omitted}",
            row.case,
            row.empty
        );
    }
}

/// One row of the non-empty table: the ARN the source names, and the completion the path
/// then stops with.
struct ReferenceRow {
    case: &'static str,
    arn: String,
    reason: &'static str,
}

#[tokio::test]
async fn a_non_empty_endpoint_attachment_arn_is_still_a_reference() {
    let emulator = Emulator::new();
    let (network, held_attachment_arn) = fixture(&emulator).await;

    // The rows span the ways a non-empty ARN is still a reference: one nothing is stored
    // under, a blank one — which is a value rather than the absence of one — and one naming
    // an attachment this global network does hold, which is no attachment on a transit
    // gateway.
    let table = vec![
        ReferenceRow {
            case: "an ARN no global network holds",
            arn: transit_gateway_attachment_arn(3),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
        },
        ReferenceRow {
            case: "a blank ARN, which is a value and not the absence of one",
            arn: " ".to_string(),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
        },
        ReferenceRow {
            case: "an attachment this global network holds, not one on a transit gateway",
            arn: held_attachment_arn.clone(),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY",
        },
    ];

    let mut started: Vec<(&ReferenceRow, String)> = Vec::new();
    for row in &table {
        let (id, analysis) = start(
            &emulator,
            row.case,
            &network,
            json!({
                SOURCE: at(&row.arn),
                DESTINATION: at(&transit_gateway_attachment_arn(4)),
            }),
        )
        .await;
        assert_eq!(
            analysis[SOURCE]["TransitGatewayAttachmentArn"],
            json!(row.arn),
            "case {:?}: a non-empty ARN must be reported as the request named it, got {analysis}",
            row.case
        );
        started.push((row, id));
    }

    std::thread::sleep(PAST_SETTLE);

    for (row, id) in &started {
        let analysis = read(&emulator, row.case, &network, id).await;
        assert_eq!(
            analysis[SOURCE]["TransitGatewayAttachmentArn"],
            json!(row.arn),
            "case {:?}: the stored source must hold the ARN the request named, got {analysis}",
            row.case
        );
        let completion = &analysis["ForwardPath"]["CompletionStatus"];
        assert_eq!(
            completion["ReasonCode"],
            json!(row.reason),
            "case {:?}: expected reason {}, got {analysis}",
            row.case,
            row.reason
        );
        assert_eq!(
            completion["ReasonContext"],
            json!({ format!("{SOURCE}.TransitGatewayAttachmentArn"): row.arn }),
            "case {:?}: the reason context must name the ARN the path could not resolve, got \
             {analysis}",
            row.case
        );
    }
}
