// SPDX-License-Identifier: CC0-1.0

//! One rule about a mission profile's dataflow graph, asserted end to end over HTTP
//! through [`crate::scheduling_loop`]'s front door:
//!
//! an entry of `dataflowEdges` is ONE dataflow of a contact, so the two configs it names
//! are type-checked and not merely resolved — the source's capability must be one that
//! originates a dataflow and the destination's one that receives data from that source.
//! An edge outside those pairings is refused with `InvalidParameterException` (HTTP 431)
//! naming the capability types the end at fault admits, and nothing is stored or changed.
//!
//! The tables below sweep EVERY ordered pair of the model's eight `ConfigCapabilityType`
//! values (64 rows), not only the pairs an issue cited, plus the forms an edge can take
//! that the pairing rule is indifferent to: the same config at both ends, an end naming
//! one output NODE of a config rather than the config whole, an illegal edge beside legal
//! ones in a longer list, and the same graph arriving through `UpdateMissionProfile`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, harness, send, Reply,
};

/// Every value of the model's `ConfigCapabilityType`.
///
/// Source: `smithy_query(query="shape", shape="ConfigCapabilityType",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — the eight capability tokens, which
/// are also the eight members of `ConfigTypeData`.
const CAPABILITY_TYPES: &[&str] = &[
    "antenna-downlink",
    "antenna-downlink-demod-decode",
    "antenna-uplink",
    "dataflow-endpoint",
    "s3-recording",
    "telemetry-sink",
    "tracking",
    "uplink-echo",
];

/// The ordered `(source, destination)` capability pairs that form a dataflow.
///
/// Written out here rather than read from the service so the suite states the rule
/// independently of the code under test. Downlinked data — raw (`antenna-downlink`) or
/// demodulated and decoded by the service first — is carried to a dataflow endpoint of
/// the customer's VPC or written to their bucket; data the customer hands to a dataflow
/// endpoint is radiated by the antenna; and the echo of an uplink is carried back to a
/// dataflow endpoint. The capability types that take part at all are the model's own:
/// `Source$configDetails` is "Additional details for a Config, if type is
/// dataflow-endpoint or antenna-downlink-demod-decode", `Destination$configDetails` says
/// the same for a destination, and `ConfigDetails` carries `s3RecordingDetails`, "Details
/// for an S3 recording Config in a contact" (`smithy_query(query="shape",
/// shape="Source,Destination,ConfigDetails")`). The two types in no pair — `tracking` and
/// `telemetry-sink` — are the two a profile wires up through a member of its own
/// (`trackingConfigArn`, `telemetrySinkConfigArn`), never through the graph.
const DATAFLOWS: &[(&str, &str)] = &[
    ("antenna-downlink", "dataflow-endpoint"),
    ("antenna-downlink", "s3-recording"),
    ("antenna-downlink-demod-decode", "dataflow-endpoint"),
    ("antenna-downlink-demod-decode", "s3-recording"),
    ("dataflow-endpoint", "antenna-uplink"),
    ("uplink-echo", "dataflow-endpoint"),
];

/// The trailing segment a `ConfigArn` may carry after a config's uuid — one output node of
/// that config (`^arn:aws:groundstation:…:config/…/<uuid>(/.{1,256})?$`, the pattern on
/// `DataflowEdge`'s member).
const OUTPUT_NODE: &str = "I-Egress";

/// Whether `source` into `destination` is a dataflow, per [`DATAFLOWS`].
fn is_dataflow(source: &str, destination: &str) -> bool {
    DATAFLOWS
        .iter()
        .any(|(from, to)| *from == source && *to == destination)
}

/// The capability types a dataflow may be destined for from `source`, in [`DATAFLOWS`]
/// order.
fn destinations_from(source: &str) -> Vec<&'static str> {
    DATAFLOWS
        .iter()
        .filter(|(from, _)| *from == source)
        .map(|(_, to)| *to)
        .collect()
}

/// The capability types a dataflow can start at, in [`DATAFLOWS`] order.
fn dataflow_sources() -> Vec<&'static str> {
    let mut sources: Vec<&'static str> = Vec::new();
    for (from, _) in DATAFLOWS {
        if !sources.contains(from) {
            sources.push(from);
        }
    }
    sources
}

/// `types` quoted and joined the way a refusal lists them.
fn quoted(types: &[&str]) -> String {
    let quoted: Vec<String> = types.iter().map(|token| format!("\"{token}\"")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} or {last}", leading.join(", ")),
    }
}

/// The refusal an edge out of `source` earns: the end at fault is the SOURCE when its
/// capability starts no dataflow, otherwise the destination, and the text names the types
/// that end admits.
fn expected_refusal(source: &str) -> String {
    let admissible = match destinations_from(source) {
        empty if empty.is_empty() => dataflow_sources(),
        destinations => destinations,
    };
    format!(
        "Invalid ARN provided. Config Type must be of type {}.",
        quoted(&admissible)
    )
}

/// One config of every capability type, keyed by that type, over a router whose account
/// is provisioned for uplink.
async fn configs_of_every_type(router: &MultiServiceRouter) -> BTreeMap<&'static str, String> {
    // `uplinkEchoConfig` references an antenna-uplink config by ARN, so one exists first.
    let (_, _, antenna_uplink_arn) = create_config(
        router,
        "edge-rules echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    let mut arns: BTreeMap<&'static str, String> = BTreeMap::new();
    for (config_type, config_data) in config_data_table(&antenna_uplink_arn) {
        let (_, created_type, arn) = create_config(
            router,
            &format!("edge-rules {config_type}"),
            config_data,
            None,
        )
        .await;
        assert_eq!(
            created_type, config_type,
            "setup {config_type}: CreateConfig reported another configType, got {created_type:?}"
        );
        arns.insert(config_type, arn);
    }
    assert_eq!(
        arns.len(),
        CAPABILITY_TYPES.len(),
        "setup: one config per capability type is needed, got {:?}",
        arns.keys()
    );
    arns
}

/// `CreateMissionProfile` carrying `edges`, against `tracking_arn`.
async fn create_profile(
    router: &MultiServiceRouter,
    name: &str,
    tracking_arn: &str,
    edges: Value,
) -> Reply {
    send(
        router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": name,
            "minimumViableContactDurationSeconds": 60,
            "dataflowEdges": edges,
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await
}

/// Assert `reply` is this service's refusal of an edge out of a config of `source_type`.
fn assert_edge_refused(case: &str, reply: &Reply, source_type: &str) {
    assert_eq!(
        reply.status, 431,
        "case {case}: an edge that is no dataflow must answer 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        expected_refusal(source_type),
        "case {case}: the refusal must name the capability types the end at fault admits, \
         got {}",
        reply.field("message")
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!("dataflowEdges"),
        "case {case}: the refusal must name the member at fault, got {}",
        reply.field("parameterName")
    );
    assert!(
        reply.field("missionProfileId").is_null(),
        "case {case}: a refused create must mint no profile, got {}",
        reply.body
    );
}

/// The ids `ListMissionProfiles` holds, sorted.
async fn listed_profile_ids(router: &MultiServiceRouter) -> Vec<String> {
    let list = send(router, "GET", "/missionprofile", None).await;
    assert_eq!(
        list.status, 200,
        "ListMissionProfiles: expected 200, got {} {}",
        list.status, list.body
    );
    let mut ids: Vec<String> = list
        .field("missionProfileList")
        .as_array()
        .expect("missionProfileList is a list")
        .iter()
        .map(|item| {
            item.get("missionProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn every_capability_pair_is_admitted_as_a_dataflow_edge_only_if_it_is_a_dataflow() {
    let router = harness();
    let arns = configs_of_every_type(&router).await;
    let tracking_arn = arns["tracking"].clone();

    let mut created: Vec<String> = Vec::new();
    for source_type in CAPABILITY_TYPES {
        for destination_type in CAPABILITY_TYPES {
            let case = format!("{source_type} -> {destination_type}");
            let edges = json!([[arns[*source_type], arns[*destination_type]]]);
            let reply = create_profile(
                &router,
                &format!("edge {source_type} to {destination_type}"),
                &tracking_arn,
                edges.clone(),
            )
            .await;

            if !is_dataflow(source_type, destination_type) {
                assert_edge_refused(&case, &reply, source_type);
                continue;
            }

            assert_eq!(
                reply.status, 200,
                "case {case}: a dataflow must be accepted, got {} {}",
                reply.status, reply.body
            );
            let id = reply.string("missionProfileId");
            created.push(id.clone());

            // Round trip: the graph a read reports is the one that was created.
            let read = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected GetMissionProfile 200, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                read.field("dataflowEdges"),
                &edges,
                "case {case}: the stored graph must be the one created, got {}",
                read.field("dataflowEdges")
            );
        }
    }

    assert_eq!(
        created.len(),
        DATAFLOWS.len(),
        "case sweep: exactly the dataflow pairings may be stored, got {created:?}"
    );
    created.sort();
    assert_eq!(
        listed_profile_ids(&router).await,
        created,
        "case sweep: a refused create must leave no profile behind"
    );
}

#[tokio::test]
async fn an_edge_naming_one_config_at_both_ends_is_never_a_dataflow() {
    let router = harness();
    let arns = configs_of_every_type(&router).await;
    let tracking_arn = arns["tracking"].clone();

    for config_type in CAPABILITY_TYPES {
        let case = format!("self edge {config_type}");
        let arn = arns[*config_type].clone();
        let reply = create_profile(
            &router,
            &format!("self edge {config_type}"),
            &tracking_arn,
            json!([[arn, arn]]),
        )
        .await;
        assert_edge_refused(&case, &reply, config_type);
    }

    // The two instances the issue cited, with their text spelled out rather than derived:
    // the config a profile wires through `trackingConfigArn` is no dataflow node at all,
    // and two downlink configs are a source with no destination.
    let tracking_self = create_profile(
        &router,
        "tracking at both ends",
        &tracking_arn,
        json!([[tracking_arn, tracking_arn]]),
    )
    .await;
    assert_eq!(
        tracking_self.string("message"),
        "Invalid ARN provided. Config Type must be of type \"antenna-downlink\", \
         \"antenna-downlink-demod-decode\", \"dataflow-endpoint\" or \"uplink-echo\".",
        "case tracking-at-both-ends: unexpected refusal text, got {}",
        tracking_self.body
    );
    let downlink_arn = arns["antenna-downlink"].clone();
    let downlink_self = create_profile(
        &router,
        "downlink at both ends",
        &tracking_arn,
        json!([[downlink_arn, downlink_arn]]),
    )
    .await;
    assert_eq!(
        downlink_self.string("message"),
        "Invalid ARN provided. Config Type must be of type \"dataflow-endpoint\" or \
         \"s3-recording\".",
        "case downlink-at-both-ends: unexpected refusal text, got {}",
        downlink_self.body
    );

    assert!(
        listed_profile_ids(&router).await.is_empty(),
        "case self-edges: no profile may have been stored"
    );
}

#[tokio::test]
async fn the_pairing_rule_holds_whatever_form_the_edge_list_takes() {
    let router = harness();
    let arns = configs_of_every_type(&router).await;
    let tracking_arn = arns["tracking"].clone();
    let demod_arn = arns["antenna-downlink-demod-decode"].clone();
    let downlink_arn = arns["antenna-downlink"].clone();
    let endpoint_arn = arns["dataflow-endpoint"].clone();
    let recording_arn = arns["s3-recording"].clone();
    let uplink_arn = arns["antenna-uplink"].clone();

    /// One row: the case, the `dataflowEdges` it sends, and the capability type of the
    /// source of the edge at fault (`None` when every edge is a dataflow).
    struct Row {
        case: &'static str,
        edges: Value,
        refused_source: Option<&'static str>,
    }

    // Rows differ along the number of edges, which edge in the list is at fault, and
    // whether an end names a config whole or one output NODE of it.
    let rows = vec![
        Row {
            case: "an empty graph",
            edges: json!([]),
            refused_source: None,
        },
        Row {
            case: "one dataflow in each direction, plus a recording",
            edges: json!([
                [downlink_arn, endpoint_arn],
                [endpoint_arn, uplink_arn],
                [demod_arn, recording_arn],
            ]),
            refused_source: None,
        },
        Row {
            case: "an output node of a demod-decode config into a dataflow endpoint",
            edges: json!([[format!("{demod_arn}/{OUTPUT_NODE}"), endpoint_arn]]),
            refused_source: None,
        },
        Row {
            case: "an output node of a config that is no dataflow node",
            edges: json!([[format!("{tracking_arn}/{OUTPUT_NODE}"), endpoint_arn]]),
            refused_source: Some("tracking"),
        },
        Row {
            case: "an output node of a downlink config into another downlink config",
            edges: json!([[format!("{demod_arn}/{OUTPUT_NODE}"), downlink_arn]]),
            refused_source: Some("antenna-downlink-demod-decode"),
        },
        Row {
            case: "an illegal edge after two legal ones",
            edges: json!([
                [downlink_arn, endpoint_arn],
                [endpoint_arn, uplink_arn],
                [recording_arn, endpoint_arn],
            ]),
            refused_source: Some("s3-recording"),
        },
        Row {
            case: "an illegal edge before a legal one",
            edges: json!([
                [endpoint_arn, recording_arn],
                [downlink_arn, endpoint_arn],
            ]),
            refused_source: Some("dataflow-endpoint"),
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for row in &rows {
        let reply = create_profile(
            &router,
            &format!("form {}", created.len()),
            &tracking_arn,
            row.edges.clone(),
        )
        .await;
        match row.refused_source {
            Some(source_type) => assert_edge_refused(row.case, &reply, source_type),
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {}: a graph of dataflows must be accepted, got {} {}",
                    row.case, reply.status, reply.body
                );
                let id = reply.string("missionProfileId");
                let read = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
                assert_eq!(
                    read.field("dataflowEdges"),
                    &row.edges,
                    "case {}: the stored graph must be the one created, got {}",
                    row.case,
                    read.field("dataflowEdges")
                );
                created.push(id);
            }
        }
    }

    created.sort();
    assert_eq!(
        listed_profile_ids(&router).await,
        created,
        "case forms: only the graphs made of dataflows may be stored"
    );
}

#[tokio::test]
async fn an_update_cannot_leave_a_profile_holding_a_graph_a_create_would_refuse() {
    let router = harness();
    let arns = configs_of_every_type(&router).await;
    let tracking_arn = arns["tracking"].clone();
    let downlink_arn = arns["antenna-downlink"].clone();
    let endpoint_arn = arns["dataflow-endpoint"].clone();
    let recording_arn = arns["s3-recording"].clone();
    let uplink_arn = arns["antenna-uplink"].clone();

    let legal_graph = json!([[downlink_arn, endpoint_arn]]);
    let created = create_profile(
        &router,
        "update subject",
        &tracking_arn,
        legal_graph.clone(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");

    // Rows differ along which end of the edge is at fault and how many edges the update
    // carries; each must leave the stored graph exactly as it was.
    let refusals: Vec<(&str, &str, Value)> = vec![
        (
            "a config that is no dataflow node",
            "tracking",
            json!([[tracking_arn, tracking_arn]]),
        ),
        (
            "two downlinks",
            "antenna-downlink",
            json!([[downlink_arn, downlink_arn]]),
        ),
        (
            "a recording as the source",
            "s3-recording",
            json!([[recording_arn, endpoint_arn]]),
        ),
        (
            "an illegal edge beside a legal one",
            "antenna-uplink",
            json!([[downlink_arn, endpoint_arn], [uplink_arn, endpoint_arn]]),
        ),
    ];

    for (case, source_type, edges) in &refusals {
        let reply = send(
            &router,
            "PUT",
            &format!("/missionprofile/{id}"),
            Some(json!({"dataflowEdges": edges})),
        )
        .await;
        assert_edge_refused(case, &reply, source_type);

        let read = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
        assert_eq!(
            read.field("dataflowEdges"),
            &legal_graph,
            "case {case}: a refused update must leave the stored graph alone, got {}",
            read.field("dataflowEdges")
        );
    }

    // An update whose graph IS made of dataflows goes through, so the refusals above are
    // about the pairing and not about updating the member at all.
    let accepted_graph = json!([[endpoint_arn, uplink_arn], [downlink_arn, recording_arn]]);
    let accepted = send(
        &router,
        "PUT",
        &format!("/missionprofile/{id}"),
        Some(json!({"dataflowEdges": accepted_graph})),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case accepted-update: expected 200, got {} {}",
        accepted.status, accepted.body
    );
    let read = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
    assert_eq!(
        read.field("dataflowEdges"),
        &accepted_graph,
        "case accepted-update: the stored graph must be the one the update carried, got {}",
        read.field("dataflowEdges")
    );
}
