// SPDX-License-Identifier: CC0-1.0

//! One rule about a flow source's ingest endpoint, end to end over HTTP, on every path that
//! stores a source.
//!
//! A source reached through another account's entitlement is fed by that account's flow, so
//! this service assigns it no ingest endpoint of its own; an owned source listens on both
//! halves of the one the service assigned it. `Source$IngestIp` is "the IP address that the
//! flow will be listening on for incoming content" and `Source$IngestPort` "the port that
//! the flow will be listening on for incoming content"
//! (`smithy_query(query="shape",shape="Source")`), so the two halves are ONE endpoint: a
//! source either reports both or reports neither, keyed on the `EntitlementArn` the same
//! `ListedFlow$SourceType` is read off — `ENTITLED` "when the flow ingests through an
//! entitlement granted by another account", `OWNED` otherwise
//! (`smithy_query(query="shape",shape="ListedFlow")`).
//!
//! The rows vary along what the rule generalises over: which path stored the source
//! (CreateFlow's `Source`, AddFlowSources, UpdateFlowSource), which direction the update
//! moves the source in (owned onto an entitlement, an entitled source asked for a port),
//! which protocol the source is on (including a Zixi one, whose port the model fixes, and
//! none at all), whether the request carries the entitlement alone or beside other
//! settings, and whether the source already held an endpoint.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_sources::{add_flow_sources, create_flow_from, described_sources, failover_active};
use crate::update_flow_source::update_flow_source;

/// An entitlement ARN of another account, well formed against the modelled
/// `EntitlementArn` pattern `^arn:.+:mediaconnect.+:entitlement:.+$`, so the request reaches
/// the handler.
const OTHER_ACCOUNT_ENTITLEMENT: &str =
    "arn:aws:mediaconnect:us-east-1:999988887777:entitlement:1-0000beef:shared";

/// A port a source may ask for, outside the one the model fixes for a Zixi source.
const REQUESTED_PORT: i64 = 5101;

/// One row of the endpoint table: the source an add path stores, the UpdateFlowSource body
/// then applied to it, and whether the source is reached through an entitlement once those
/// calls are applied.
struct EndpointCase {
    case: &'static str,
    /// The `SetSourceRequest` CreateFlow's `Source` and AddFlowSources both store.
    source: Value,
    /// The UpdateFlowSource body applied to that stored source, or `None` for a row that
    /// asserts only what the add paths store.
    update: Option<Value>,
    /// Whether the stored source carries an `EntitlementArn` after the row's calls — the
    /// marker the rule is keyed on, stated by the row so a row cannot pass vacuously.
    entitled: bool,
}

fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "an owned rtp source no update touches",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: None,
            entitled: false,
        },
        EndpointCase {
            case: "an owned source given an entitlement",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: Some(json!({ "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT })),
            entitled: true,
        },
        EndpointCase {
            case: "an owned source that asked for a port, given an entitlement",
            source: json!({
                "name": "src",
                "protocol": "rtp",
                "whitelistCidr": "10.0.0.0/16",
                "ingestPort": REQUESTED_PORT
            }),
            update: Some(json!({ "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT })),
            entitled: true,
        },
        EndpointCase {
            case: "an owned source given an entitlement and a port in one request",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: Some(json!({
                "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT,
                "ingestPort": REQUESTED_PORT
            })),
            entitled: true,
        },
        EndpointCase {
            // The port the model fixes for a Zixi source is still a port of THIS account,
            // so an entitled Zixi source holds none of it either.
            case: "an owned zixi-push source given an entitlement",
            source: json!({
                "name": "src",
                "protocol": "zixi-push",
                "whitelistCidr": "10.0.0.0/16"
            }),
            update: Some(json!({ "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT })),
            entitled: true,
        },
        EndpointCase {
            case: "an owned source given an entitlement and a new protocol",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: Some(json!({
                "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT,
                "protocol": "srt-listener"
            })),
            entitled: true,
        },
        EndpointCase {
            case: "an entitled source asked for a port",
            source: json!({
                "name": "src",
                "protocol": "rtp",
                "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT
            }),
            update: Some(json!({ "ingestPort": REQUESTED_PORT })),
            entitled: true,
        },
        EndpointCase {
            // `Transport$Protocol` is required, so a source that names no protocol holds no
            // transport at all — and the rule reads the entitlement, not the transport.
            case: "an entitled source on no protocol at all, asked for a port",
            source: json!({ "name": "src", "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT }),
            update: Some(json!({ "ingestPort": REQUESTED_PORT })),
            entitled: true,
        },
        EndpointCase {
            case: "an entitled source given a description",
            source: json!({
                "name": "src",
                "protocol": "rtp",
                "entitlementArn": OTHER_ACCOUNT_ENTITLEMENT
            }),
            update: Some(json!({ "description": "the shared feed" })),
            entitled: true,
        },
        EndpointCase {
            case: "an owned source whose port an update moves",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: Some(json!({ "ingestPort": REQUESTED_PORT })),
            entitled: false,
        },
        EndpointCase {
            case: "an owned source whose protocol an update moves onto Zixi",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            update: Some(json!({ "protocol": "zixi-pull" })),
            entitled: false,
        },
    ]
}

/// The rule itself, read off the source the service reports: an entitled source reports
/// NEITHER half of an ingest endpoint, an owned one reports BOTH.
fn assert_ingest_endpoint(case: &str, where_from: &str, source: &Value, entitled: bool) {
    assert_eq!(
        source["entitlementArn"].is_string(),
        entitled,
        "case {case:?} ({where_from}): the source is entitled={entitled}, got {source}"
    );
    let ip = &source["ingestIp"];
    let port = &source["ingestPort"];
    if entitled {
        assert!(
            ip.is_null() && port.is_null(),
            "case {case:?} ({where_from}): a source reached through another account's \
             entitlement holds no ingest endpoint of its own, got ingestIp {ip} and \
             ingestPort {port}"
        );
    } else {
        assert!(
            ip.is_string() && port.is_number(),
            "case {case:?} ({where_from}): an owned source holds both halves of its ingest \
             endpoint, got ingestIp {ip} and ingestPort {port}"
        );
    }
}

/// The `SourceType` ListFlows reports for the flow `flow_arn`.
///
/// Every flow the test created at once, so the listing asks for a page large enough to hold
/// them (the default page is 10 flows).
async fn listed_source_type(emulator: &Emulator, case: &str, flow_arn: &str) -> String {
    let (status, code, listing) = emulator.list_flows("?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: ListFlows must answer 200, got {status} {code:?}: {listing}"
    );
    listing["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|flow| flow["flowArn"].as_str() == Some(flow_arn))
        .and_then(|flow| flow["sourceType"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: ListFlows reports the flow under test"))
        .to_string()
}

/// A stored source holds an ingest endpoint of this account exactly when it is NOT reached
/// through another account's entitlement — on CreateFlow's own source, on the source
/// AddFlowSources appends, and after an UpdateFlowSource that moves either member of the
/// relation.
#[tokio::test]
async fn a_source_holds_an_ingest_endpoint_exactly_when_it_is_not_entitled() {
    let emulator = Emulator::new();

    for (index, EndpointCase { case, source, update, entitled }) in
        endpoint_cases().into_iter().enumerate()
    {
        // CreateFlow's own `Source`: the flow under test holds this source alone, so the
        // `SourceType` it is listed under is this source's.
        let flow_arn = create_flow_from(
            &emulator,
            case,
            json!({ "name": format!("flow-endpoint-{index}"), "source": source.clone() }),
        )
        .await;
        let created = described_sources(&emulator, case, &flow_arn).await;
        let stored = created
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the created flow holds its source"));
        // Before the update, the source is entitled iff its own request named an
        // entitlement — which is the row's end state for every row the update does not move
        // the relation in.
        let entitled_on_create = source["entitlementArn"].is_string();
        assert_ingest_endpoint(case, "CreateFlow", &stored, entitled_on_create);

        let source_arn = stored["sourceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the stored source carries an ARN"))
            .to_string();

        if let Some(body) = update.clone() {
            let (status, code, answered) =
                update_flow_source(&emulator, &flow_arn, &source_arn, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: UpdateFlowSource must answer 202, got {status} {code:?}: \
                 {answered}"
            );
            assert_ingest_endpoint(case, "UpdateFlowSource", &answered["source"], entitled);
            // The round trip: the 202 body is the stored record DescribeFlow reports.
            let after = described_sources(&emulator, case, &flow_arn).await;
            assert_eq!(
                after.first(), Some(&answered["source"]),
                "case {case:?}: DescribeFlow reports what the update answered, got {after:?}"
            );
            assert_ingest_endpoint(case, "DescribeFlow", &after[0], entitled);
        }

        // The flow's `SourceType` is read off the same marker, so a flow listed ENTITLED
        // never holds a source still advertising an endpoint of this account.
        let expected_type = if entitled {
            mc::SOURCE_TYPE_ENTITLED
        } else {
            mc::SOURCE_TYPE_OWNED
        };
        assert_eq!(
            listed_source_type(&emulator, case, &flow_arn).await,
            expected_type,
            "case {case:?}: ListFlows reports the flow's source type"
        );

        // AddFlowSources: the same request shape appended to a flow that already holds a
        // plain source, updated the same way, must store the same relation.
        let host_arn = create_flow_from(
            &emulator,
            case,
            json!({
                "name": format!("flow-endpoint-added-{index}"),
                "sourceFailoverConfig": failover_active(),
                "source": { "name": "host", "protocol": "rtp", "whitelistCidr": "10.9.0.0/16" },
            }),
        )
        .await;
        let mut appended = source.clone();
        appended["name"] = json!("appended");
        let (status, code, added) =
            add_flow_sources(&emulator, &host_arn, json!([appended])).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: AddFlowSources must answer 201, got {status} {code:?}: {added}"
        );
        assert_ingest_endpoint(case, "AddFlowSources", &added["sources"][0], entitled_on_create);

        if let Some(body) = update {
            let added_arn = added["sources"][0]["sourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the appended source carries an ARN"))
                .to_string();
            let (status, code, answered) =
                update_flow_source(&emulator, &host_arn, &added_arn, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the appended source must update, got {status} {code:?}: \
                 {answered}"
            );
            assert_ingest_endpoint(
                case,
                "UpdateFlowSource on the appended source",
                &answered["source"],
                entitled,
            );
            // The flow's other source is owned and keeps its own endpoint whole: the
            // relation is per source, not per flow.
            let after = described_sources(&emulator, case, &host_arn).await;
            let host = after
                .iter()
                .find(|stored| stored["name"].as_str() == Some("host"))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: the flow still holds its host source"));
            assert_ingest_endpoint(case, "the owned sibling", &host, false);
        }
    }
}
