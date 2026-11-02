// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `StartRouteAnalysis` and `GetRouteAnalysis`, driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: `StartRouteAnalysis` records a route analysis of the addressed
//! global network, analysing the path between its two endpoints against what that global
//! network holds at the instant of the call — each endpoint resolved against the global
//! network's own network-resource inventory — and reports it `RUNNING` until the
//! transition the clock owes falls due, after which `GetRouteAnalysis` reads back the same
//! analysis with the completion the call computed: `NOT_CONNECTED` plus the reason code for
//! the endpoint the path could not get past, and no return path, because the forward path
//! did not reach the destination.
//!
//! A second table pins the reporting side of that rule on its own: `GetRouteAnalysis`
//! reports the status the clock owes the analysis — `RUNNING` until that analysis's own due
//! instant, `COMPLETED` after it — so reading it, however many times, moves nothing a later
//! caller sees.
//!
//! The table below spans that class: rows differing in which endpoint carries an attachment
//! ARN at all, in whether that ARN names a resource of this global network, of another one,
//! or of nothing at all, in what kind of resource it names, in whether an IP address is
//! given, and in the two flags. No attachment on a transit gateway can be in the inventory
//! yet — a transit gateway's attachments are learned from its registration with the global
//! network, and `RegisterTransitGateway` is still a scaffold stub with its own issue — so
//! the rows cover exactly the resolutions the store can hold. Nothing reaches into
//! `crate::state`: every analysis is read back through `GetRouteAnalysis`.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE};

/// The resource kind a route analysis is: the id prefix `StartRouteAnalysis` mints and the
/// `ResourceType` a not-found refusal reports (`crate::state::ROUTE_ANALYSIS_KIND`).
const ROUTE_ANALYSIS: &str = "route-analysis";

/// The Region the core networks here configure an edge location in, and so the Region of
/// the attached VPCs.
const REGION: &str = "us-west-2";

/// How many route analyses one global network may hold: the bound
/// `crate::state::MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK` fixes, which is what keeps
/// repeated starts from growing the store without end.
const MAX_ANALYSES: usize = 100;

/// An Amazon VPC transit gateway attachment ARN: the kind of ARN a route analysis endpoint
/// names ("The ARN of the transit gateway attachment", `smithy_query query="doc"
/// shape="RouteAnalysisEndpointOptions" member="TransitGatewayAttachmentArn"`). Nothing
/// registers a transit gateway with a global network yet, so no such ARN names a resource
/// any global network here holds.
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

/// One endpoint of a request: the attachment ARN and the IP address it names, either of
/// which may be absent.
#[derive(Debug, Clone, Default)]
struct Endpoint {
    attachment_arn: Option<String>,
    ip_address: Option<&'static str>,
}

impl Endpoint {
    /// The request body of this endpoint, carrying only the members it names.
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(arn) = &self.attachment_arn {
            body["TransitGatewayAttachmentArn"] = json!(arn);
        }
        if let Some(address) = self.ip_address {
            body["IpAddress"] = json!(address);
        }
        body
    }
}

/// An endpoint naming an attachment ARN and no IP address.
fn at(arn: &str) -> Endpoint {
    Endpoint {
        attachment_arn: Some(arn.to_string()),
        ip_address: None,
    }
}

/// An endpoint naming an attachment ARN and an IP address.
fn at_address(arn: &str, address: &'static str) -> Endpoint {
    Endpoint {
        attachment_arn: Some(arn.to_string()),
        ip_address: Some(address),
    }
}

/// An endpoint naming only an IP address — no attachment ARN at all.
fn address_only(address: &'static str) -> Endpoint {
    Endpoint {
        attachment_arn: None,
        ip_address: Some(address),
    }
}

/// One row of the table: a request, and the completion the analysis must report.
#[derive(Debug, Clone)]
struct AnalysisCase {
    /// Names the case in every assertion message.
    case: &'static str,
    source: Endpoint,
    destination: Endpoint,
    /// `IncludeReturnPath` as the request spells it, or `None` for a request that names it
    /// nowhere (the modelled `@default` of `false`).
    include_return_path: Option<bool>,
    /// `UseMiddleboxes`; see [`AnalysisCase::include_return_path`].
    use_middleboxes: Option<bool>,
    /// The `RouteAnalysisCompletion$ReasonCode` the forward path must carry.
    reason: &'static str,
    /// Which endpoint's attachment ARN the `ReasonContext` must name (`Source` or
    /// `Destination`), or `None` for a completion that carries no context at all.
    context_endpoint: Option<&'static str>,
}

impl AnalysisCase {
    /// The request body for this row.
    fn body(&self) -> Value {
        let mut body = json!({
            "Source": self.source.body(),
            "Destination": self.destination.body(),
        });
        if let Some(flag) = self.include_return_path {
            body["IncludeReturnPath"] = json!(flag);
        }
        if let Some(flag) = self.use_middleboxes {
            body["UseMiddleboxes"] = json!(flag);
        }
        body
    }

    /// The `ReasonContext` this row's completion must carry: the request member the
    /// analysis could not get past, keyed as `<member>.TransitGatewayAttachmentArn`.
    fn expected_context(&self) -> Value {
        match self.context_endpoint {
            None => Value::Null,
            Some(member) => {
                let endpoint = if member == "Source" {
                    &self.source
                } else {
                    &self.destination
                };
                let arn = endpoint
                    .attachment_arn
                    .clone()
                    .unwrap_or_else(|| panic!("case {:?}: the context endpoint names no ARN", self.case));
                json!({ format!("{member}.TransitGatewayAttachmentArn"): arn })
            }
        }
    }

    /// What the endpoint options of `member` must read as: the ARN and the IP address the
    /// request named, and never a `TransitGatewayArn` — no endpoint here resolves to an
    /// attachment on a transit gateway, so there is no gateway to report.
    fn expected_endpoint(&self, member: &str) -> Value {
        let endpoint = if member == "Source" {
            &self.source
        } else {
            &self.destination
        };
        let mut expected = json!({});
        if let Some(arn) = &endpoint.attachment_arn {
            expected["TransitGatewayAttachmentArn"] = json!(arn);
        }
        if let Some(address) = endpoint.ip_address {
            expected["IpAddress"] = json!(address);
        }
        expected
    }
}

/// The resources the table's rows name: one global network with a core network, a Cloud WAN
/// attachment and a device, plus a SECOND global network holding an attachment of its own.
struct Fixture {
    /// The global network every analysis below is started on.
    network: String,
    /// The ARN of a Cloud WAN attachment of [`Self::network`] — an attachment the global
    /// network holds, but not one on a transit gateway.
    attachment_arn: String,
    /// The ARN of a device of [`Self::network`] — a resource of the global network that is
    /// no attachment at all.
    device_arn: String,
    /// The ARN of a Cloud WAN attachment of ANOTHER global network.
    other_network_attachment_arn: String,
}

/// Build the fixture through the implemented operations only.
async fn fixture(emulator: &Emulator) -> Fixture {
    let (network, core_network) = core_network_at(emulator, "route-analysis", &[REGION]).await;
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(REGION, 0x5101),
            "SubnetArns": [subnet_arn(REGION, 0x5101)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment for the fixture: expected 200, got {status} with {created}"
    );
    let attachment = attachment_id("fixture", &created["VpcAttachment"]);
    let device = create_device_ok(
        emulator,
        &network,
        json!({ "Description": "route-analysis fixture" }),
    )
    .await;

    let (other_network, other_core_network) =
        core_network_at(emulator, "route-analysis-other", &[REGION]).await;
    let _ = other_network;
    let (status, other_created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": other_core_network,
            "VpcArn": vpc_arn(REGION, 0x5102),
            "SubnetArns": [subnet_arn(REGION, 0x5102)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment in the other global network: expected 200, got {status} with {other_created}"
    );
    let other_attachment = attachment_id("fixture-other", &other_created["VpcAttachment"]);

    Fixture {
        network,
        attachment_arn: expected_arn("attachment", &attachment),
        device_arn: expected_arn("device", &device),
        other_network_attachment_arn: expected_arn("attachment", &other_attachment),
    }
}

/// The table: rows differing along everything the rule generalises over — which endpoint
/// carries an attachment ARN, what that ARN names (nothing at all, an attachment of this
/// global network, an attachment of another one, a resource that is no attachment), whether
/// an IP address is given, and the two flags.
fn analysis_cases(fixture: &Fixture) -> Vec<AnalysisCase> {
    vec![
        AnalysisCase {
            case: "neither endpoint's attachment is held anywhere",
            source: at(&transit_gateway_attachment_arn(1)),
            destination: at(&transit_gateway_attachment_arn(2)),
            include_return_path: None,
            use_middleboxes: None,
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
            context_endpoint: Some("Source"),
        },
        AnalysisCase {
            case: "both endpoints also name an IP address, and both flags are set",
            source: at_address(&transit_gateway_attachment_arn(3), "10.0.1.7"),
            destination: at_address(&transit_gateway_attachment_arn(4), "10.0.2.9"),
            include_return_path: Some(true),
            use_middleboxes: Some(true),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
            context_endpoint: Some("Source"),
        },
        AnalysisCase {
            case: "the destination names no attachment ARN, only an address",
            source: at(&transit_gateway_attachment_arn(5)),
            destination: address_only("10.0.3.11"),
            include_return_path: Some(true),
            use_middleboxes: None,
            reason: "NO_DESTINATION_ARN_PROVIDED",
            context_endpoint: None,
        },
        AnalysisCase {
            case: "neither endpoint names an attachment ARN",
            source: address_only("10.0.4.13"),
            destination: address_only("10.0.5.15"),
            include_return_path: None,
            use_middleboxes: Some(true),
            reason: "NO_DESTINATION_ARN_PROVIDED",
            context_endpoint: None,
        },
        AnalysisCase {
            case: "the source is an attachment of this global network, not one on a transit gateway",
            source: at(&fixture.attachment_arn),
            destination: at(&transit_gateway_attachment_arn(6)),
            include_return_path: None,
            use_middleboxes: None,
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY",
            context_endpoint: Some("Source"),
        },
        AnalysisCase {
            case: "the destination is an attachment of this global network, the source nothing",
            source: at(&transit_gateway_attachment_arn(7)),
            destination: at(&fixture.attachment_arn),
            include_return_path: None,
            use_middleboxes: None,
            // The path starts at the source, so the endpoint it cannot leave is the one
            // reported, whatever the destination resolves to.
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
            context_endpoint: Some("Source"),
        },
        AnalysisCase {
            case: "the source is a device of this global network, which is no attachment",
            source: at(&fixture.device_arn),
            destination: at(&transit_gateway_attachment_arn(8)),
            include_return_path: Some(false),
            use_middleboxes: Some(false),
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
            context_endpoint: Some("Source"),
        },
        AnalysisCase {
            case: "the source is an attachment of ANOTHER global network",
            source: at(&fixture.other_network_attachment_arn),
            destination: at(&transit_gateway_attachment_arn(9)),
            include_return_path: None,
            use_middleboxes: None,
            reason: "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND",
            context_endpoint: Some("Source"),
        },
    ]
}

/// The identity and request members an analysis must report, whatever its completion.
fn assert_identity(case: &AnalysisCase, network: &str, analysis: &Value) {
    assert!(
        is_minted_id(
            analysis["RouteAnalysisId"].as_str().unwrap_or_default(),
            ROUTE_ANALYSIS
        ),
        "case {:?}: expected a minted {ROUTE_ANALYSIS} id, got {analysis}",
        case.case
    );
    assert_eq!(
        analysis["GlobalNetworkId"],
        json!(network),
        "case {:?}: expected the addressed global network, got {analysis}",
        case.case
    );
    assert_eq!(
        analysis["OwnerAccountId"],
        json!(engine::emulator_account_id()),
        "case {:?}: expected the emulator's account, got {analysis}",
        case.case
    );
    assert!(
        analysis["StartTimestamp"].as_f64().unwrap_or_default() > 0.0,
        "case {:?}: expected a start timestamp, got {analysis}",
        case.case
    );
    for member in ["Source", "Destination"] {
        assert_eq!(
            analysis[member],
            case.expected_endpoint(member),
            "case {:?}: expected the {member} the request named, got {analysis}",
            case.case
        );
    }
    assert_eq!(
        analysis["IncludeReturnPath"],
        json!(case.include_return_path.unwrap_or(false)),
        "case {:?}: expected IncludeReturnPath as requested, got {analysis}",
        case.case
    );
    assert_eq!(
        analysis["UseMiddleboxes"],
        json!(case.use_middleboxes.unwrap_or(false)),
        "case {:?}: expected UseMiddleboxes as requested, got {analysis}",
        case.case
    );
}

/// The completion a COMPLETED analysis of `case` must report.
fn assert_completion(case: &AnalysisCase, analysis: &Value) {
    let forward = &analysis["ForwardPath"];
    assert_eq!(
        forward["CompletionStatus"]["ResultCode"],
        json!("NOT_CONNECTED"),
        "case {:?}: expected NOT_CONNECTED, got {analysis}",
        case.case
    );
    assert_eq!(
        forward["CompletionStatus"]["ReasonCode"],
        json!(case.reason),
        "case {:?}: expected reason {}, got {analysis}",
        case.case,
        case.reason
    );
    assert_eq!(
        forward["CompletionStatus"]["ReasonContext"],
        case.expected_context(),
        "case {:?}: expected the reason context naming the unresolved endpoint, got {analysis}",
        case.case
    );
    assert_eq!(
        forward["Path"],
        json!([]),
        "case {:?}: a path that stopped at an endpoint traverses no component, got {analysis}",
        case.case
    );
    // "The return path is not analyzed if the forward path analysis does not succeed"
    // (`smithy_query query="doc" shape="RouteAnalysis" member="IncludeReturnPath"`), which
    // holds for every row here, including the ones that asked for a return path.
    assert_eq!(
        analysis["ReturnPath"],
        Value::Null,
        "case {:?}: expected no return path for a forward path that did not connect, got {analysis}",
        case.case
    );
}

#[tokio::test]
async fn an_analysis_reports_the_path_the_global_network_holds_for_its_endpoints() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let cases = analysis_cases(&fixture);

    // Every analysis is started first, so the whole table waits once for the transition the
    // clock owes rather than once per row.
    let mut started: Vec<(AnalysisCase, String)> = Vec::new();
    for case in cases {
        let (status, body) = emulator
            .post(&analyses_uri(&fixture.network), case.body())
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: StartRouteAnalysis expected 200, got {status} with {body}",
            case.case
        );
        let analysis = &body["RouteAnalysis"];
        assert_identity(&case, &fixture.network, analysis);
        assert_eq!(
            analysis["Status"],
            json!("RUNNING"),
            "case {:?}: a caller acting immediately must still see RUNNING, got {body}",
            case.case
        );
        assert_eq!(
            analysis["ForwardPath"],
            Value::Null,
            "case {:?}: a running analysis has no completion to report yet, got {body}",
            case.case
        );
        let id = analysis["RouteAnalysisId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no RouteAnalysisId in {body}", case.case))
            .to_string();
        assert!(
            started.iter().all(|(_, held)| held != &id),
            "case {:?}: expected a fresh analysis id, got the already minted {id}",
            case.case
        );
        started.push((case, id));
    }

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    for (case, id) in &started {
        let (status, body) = emulator.get(&analysis_uri(&fixture.network, id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetRouteAnalysis expected 200, got {status} with {body}",
            case.case
        );
        let analysis = &body["RouteAnalysis"];
        assert_eq!(
            analysis["RouteAnalysisId"],
            json!(id),
            "case {:?}: expected the analysis that was started, got {body}",
            case.case
        );
        assert_identity(case, &fixture.network, analysis);
        assert_eq!(
            analysis["Status"],
            json!("COMPLETED"),
            "case {:?}: expected COMPLETED after the span the clock owes, got {body}",
            case.case
        );
        assert_completion(case, analysis);

        // A read changes nothing a later caller sees: the second read answers the first,
        // member for member.
        let (status, again) = emulator.get(&analysis_uri(&fixture.network, id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second GetRouteAnalysis expected 200, got {status} with {again}",
            case.case
        );
        assert_eq!(
            again, body,
            "case {:?}: repeated reads must report the identical analysis",
            case.case
        );
    }
}

/// One row of the transition table: the analysis to start, and how many times it is read
/// back BEFORE the span the clock owes falls due. The rows differ in that count because the
/// rule under test is that the reported status follows the clock and the analysis's own due
/// instant — never how often, or whether, anyone read it.
#[derive(Debug, Clone)]
struct TransitionRow {
    analysis: AnalysisCase,
    reads_before_due: usize,
}

/// The transition table: one row per completion the table above spans — so the rows differ
/// in the reason code the stored analysis carries as well as in the number of early reads —
/// each read a different number of times while still `RUNNING`.
fn transition_rows(fixture: &Fixture) -> Vec<TransitionRow> {
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut rows: Vec<TransitionRow> = Vec::new();
    for analysis in analysis_cases(fixture) {
        if reasons.contains(&analysis.reason) {
            continue;
        }
        reasons.push(analysis.reason);
        rows.push(TransitionRow {
            analysis,
            reads_before_due: rows.len() + 1,
        });
    }
    rows
}

/// `GetRouteAnalysis` reports the status the clock owes, and reading never moves it: an
/// analysis read any number of times before its due instant is still `RUNNING`, with no
/// completion to report, and is `COMPLETED` after that instant whether it was read before
/// or not.
#[tokio::test]
async fn a_read_reports_the_status_the_clock_owes_and_never_moves_it() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let rows = transition_rows(&fixture);
    assert!(
        rows.len() > 1,
        "the transition table must span more than one completion, got {} rows",
        rows.len()
    );

    // Start each row and take its early reads straight away, so every read below lands
    // inside the span the clock owes rather than racing it.
    let mut started: Vec<(TransitionRow, String, Value)> = Vec::new();
    for row in rows {
        let case = &row.analysis;
        let (status, body) = emulator
            .post(&analyses_uri(&fixture.network), case.body())
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: StartRouteAnalysis expected 200, got {status} with {body}",
            case.case
        );
        let id = body["RouteAnalysis"]["RouteAnalysisId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no RouteAnalysisId in {body}", case.case))
            .to_string();

        let mut running: Option<Value> = None;
        for read in 1..=row.reads_before_due {
            let (status, early) = emulator.get(&analysis_uri(&fixture.network, &id)).await;
            assert_eq!(
                status, 200,
                "case {:?}: read {read} before the due instant expected 200, got {status} with {early}",
                case.case
            );
            let analysis = &early["RouteAnalysis"];
            assert_identity(case, &fixture.network, analysis);
            assert_eq!(
                analysis["Status"],
                json!("RUNNING"),
                "case {:?}: read {read} before the due instant must still be RUNNING, got {early}",
                case.case
            );
            assert_eq!(
                analysis["ForwardPath"],
                Value::Null,
                "case {:?}: read {read} of a running analysis has no completion to report, got {early}",
                case.case
            );
            assert_eq!(
                analysis["ReturnPath"],
                Value::Null,
                "case {:?}: read {read} of a running analysis has no return path to report, got {early}",
                case.case
            );
            match &running {
                None => running = Some(early.clone()),
                Some(first) => assert_eq!(
                    &early, first,
                    "case {:?}: read {read} before the due instant must report the identical running analysis",
                    case.case
                ),
            }
        }
        started.push((row, id, body));
    }

    // The transition is owed by the clock, not by the reads above: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    for (row, id, start_body) in &started {
        let case = &row.analysis;
        let (status, body) = emulator.get(&analysis_uri(&fixture.network, id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetRouteAnalysis after the due instant expected 200, got {status} with {body}",
            case.case
        );
        let analysis = &body["RouteAnalysis"];
        assert_eq!(
            analysis["Status"],
            json!("COMPLETED"),
            "case {:?}: expected COMPLETED after the span the clock owes, read {} times before it, got {body}",
            case.case,
            row.reads_before_due
        );
        assert_identity(case, &fixture.network, analysis);
        assert_completion(case, analysis);
        // The early reads changed nothing about the analysis itself: the identity members
        // the start reported are the ones read back.
        assert_eq!(
            analysis["StartTimestamp"],
            start_body["RouteAnalysis"]["StartTimestamp"],
            "case {:?}: expected the start timestamp the analysis was recorded with, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn starting_past_the_quota_is_refused_and_records_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("route-analysis-quota").await;
    let body = json!({
        "Source": { "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x31) },
        "Destination": { "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x32) },
    });

    let mut first = String::new();
    for index in 0..MAX_ANALYSES {
        let (status, started) = emulator.post(&analyses_uri(&network), body.clone()).await;
        assert_eq!(
            status, 200,
            "analysis {index}: expected 200 below the quota, got {status} with {started}"
        );
        if index == 0 {
            first = started["RouteAnalysis"]["RouteAnalysisId"]
                .as_str()
                .unwrap_or_else(|| panic!("no RouteAnalysisId in {started}"))
                .to_string();
        }
    }

    let (status, refused) = emulator.post(&analyses_uri(&network), body).await;
    assert_eq!(
        status, 400,
        "at the quota: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "at the quota: expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["Fields"][0]["Name"],
        json!("globalNetworkId"),
        "at the quota: expected the refusal to name the global network, got {refused}"
    );

    // The refusal recorded nothing and took nothing away: the first analysis is still there.
    let (status, held) = emulator.get(&analysis_uri(&network, &first)).await;
    assert_eq!(
        status, 200,
        "the analyses below the quota must survive the refusal, got {status} with {held}"
    );
}

#[tokio::test]
async fn an_analysis_is_only_found_through_the_global_network_it_was_started_on() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("route-analysis-owner").await;
    let other = emulator.create_global_network("route-analysis-stranger").await;
    let body = json!({
        "Source": { "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x21) },
        "Destination": { "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x22) },
    });
    let (status, started) = emulator.post(&analyses_uri(&network), body.clone()).await;
    assert_eq!(
        status, 200,
        "StartRouteAnalysis: expected 200, got {status} with {started}"
    );
    let id = started["RouteAnalysis"]["RouteAnalysisId"]
        .as_str()
        .unwrap_or_else(|| panic!("no RouteAnalysisId in {started}"))
        .to_string();

    // Each row: the request, the status and the kind the refusal must name.
    let unknown_network = "global-network-00000000000000fff";
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "read under a global network the store does not hold",
            analysis_uri(unknown_network, &id),
            "GET",
            "global-network",
        ),
        (
            "read an analysis id nothing minted",
            analysis_uri(&network, "route-analysis-0000000000000beef"),
            "GET",
            ROUTE_ANALYSIS,
        ),
        (
            "read this analysis through another global network",
            analysis_uri(&other, &id),
            "GET",
            ROUTE_ANALYSIS,
        ),
    ];

    for (case, uri, method, kind) in cases {
        let (status, refused) = if method == "POST" {
            emulator.post(&uri, body.clone()).await
        } else {
            emulator.get(&uri).await
        };
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"],
            json!(kind),
            "case {case:?}: expected the refusal to name {kind}, got {refused}"
        );
    }

    // Starting an analysis is a different refusal: the analysis is a NEW record stored
    // under the global network the path addresses, so the parent reference is validated
    // before anything is stored and an id naming no stored global network is the field
    // failure the creates under a global network answer — not the missing-resource answer
    // the reads above give.
    let (status, refused) = emulator.post(&analyses_uri(unknown_network), body.clone()).await;
    assert_eq!(
        status, 400,
        "case start on a global network the store does not hold: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case start on a global network the store does not hold: expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case start on a global network the store does not hold: expected Reason FieldValidationFailed, got {refused}"
    );
    assert_eq!(
        refused["Fields"],
        json!([{ "Name": "globalNetworkId", "Message": "Global network not found." }]),
        "case start on a global network the store does not hold: expected one Fields entry naming globalNetworkId, got {refused}"
    );

    // The refusals stored nothing: the one analysis the owner holds is still readable, and
    // still the only one.
    let (status, held) = emulator.get(&analysis_uri(&network, &id)).await;
    assert_eq!(
        status, 200,
        "the started analysis must survive the refusals, got {status} with {held}"
    );
}
