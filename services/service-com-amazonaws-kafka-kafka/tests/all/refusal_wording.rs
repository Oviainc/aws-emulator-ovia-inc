// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request the MODEL refuses, before any handler ran:
//! the error code MSK declares for bad client input, and the AWS
//! constraint-validation message built from that request.
//!
//! The rule under test is service-wide, so the table below spans the violation
//! classes the model can produce (a value above a `range` maximum, one below its
//! minimum, a string shorter and longer than a `length` bound, an absent
//! required scalar and an absent required structure, and two constraint
//! violations in one request) ACROSS operations (`ListConfigurations`,
//! `ListClusters`, `CreateConfiguration`, `CreateCluster`) — a page-size bound is
//! the same rule whichever list carries it. Every bound and member path is taken
//! from the model, cited on the constant that states it.

use serde_json::{json, Value};

use super::kafka_harness::{ClusterRequest, Emulator};

/// The modelled bounds the rows below violate, each from
/// `smithy_query(query="operation",operation="<op>")` on this crate's
/// `smithy-model/com-amazonaws-kafka-kafka.json`:
///
/// * `MaxResults` (`ListConfigurations`, `ListClusters`) — `range` 1..100.
/// * `ClusterName` (`CreateCluster`) — `length` 1..64.
/// * `NumberOfBrokerNodes` (`CreateCluster`) — `range` 1..15.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;
const CLUSTER_NAME_MIN_LENGTH: usize = 1;
const CLUSTER_NAME_MAX_LENGTH: usize = 64;
const BROKER_NODES_MIN: i64 = 1;
const BROKER_NODES_MAX: i64 = 15;

/// One clause of the AWS constraint-validation message: the value the caller
/// submitted (`None` for a member it omitted), the member path the violation is
/// reported on, and the requirement that value failed.
fn clause(submitted: Option<&str>, path: &str, requirement: &str) -> String {
    match submitted {
        Some(value) => {
            format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
        }
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
    }
}

/// The envelope the clauses of ONE refusal are stated in, which counts them.
fn envelope_prefix(clauses: usize) -> String {
    match clauses {
        1 => "1 validation error detected: ".to_string(),
        n => format!("{n} validation errors detected: "),
    }
}

/// A `CreateCluster` body whose required members are all present and legal, so a
/// row that edits ONE member exercises that member's constraint alone.
fn valid_cluster_body(name: &str) -> Value {
    ClusterRequest::named(name).to_json()
}

/// One row of the table: a request the model refuses, and every clause its
/// refusal must state.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    clauses: Vec<String>,
}

/// RULE: a request the model refuses before any handler runs is answered with
/// HTTP 400 and `BadRequestException` — the client-input error every MSK
/// operation declares, MSK's model declaring no `ValidationException` anywhere —
/// and with the AWS constraint-validation message built from that request: one
/// clause per violation, each naming the submitted value, the member path and the
/// requirement it failed, inside an envelope that counts them.
#[tokio::test]
async fn a_refused_request_states_every_violation_in_the_validation_envelope() {
    let emulator = Emulator::new();

    let too_long_name = "n".repeat(CLUSTER_NAME_MAX_LENGTH + 1);
    let mut short_name_body = valid_cluster_body("placeholder");
    short_name_body["clusterName"] = json!("");
    let mut long_name_body = valid_cluster_body("placeholder");
    long_name_body["clusterName"] = json!(too_long_name);
    let mut too_many_brokers = valid_cluster_body("too-many-brokers");
    too_many_brokers["numberOfBrokerNodes"] = json!(BROKER_NODES_MAX + 1);
    let mut too_few_brokers = valid_cluster_body("too-few-brokers");
    too_few_brokers["numberOfBrokerNodes"] = json!(BROKER_NODES_MIN - 1);
    let mut no_broker_group = valid_cluster_body("no-broker-group");
    let _ = no_broker_group
        .as_object_mut()
        .expect("the CreateCluster body is a JSON object")
        .remove("brokerNodeGroupInfo");
    // Two constraint violations in ONE request, so the envelope has to count
    // them: constraint clauses accumulate across members.
    let mut two_violations = valid_cluster_body("placeholder");
    two_violations["clusterName"] = json!(too_long_name);
    two_violations["numberOfBrokerNodes"] = json!(BROKER_NODES_MAX + 1);

    let rows = vec![
        Row {
            case: "ListConfigurations maxResults above the modelled maximum",
            method: "GET",
            uri: format!("/v1/configurations?maxResults={}", MAX_RESULTS_MAX + 1),
            body: None,
            clauses: vec![clause(
                Some(&(MAX_RESULTS_MAX + 1).to_string()),
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
            )],
        },
        Row {
            case: "ListConfigurations maxResults below the modelled minimum",
            method: "GET",
            uri: format!("/v1/configurations?maxResults={}", MAX_RESULTS_MIN - 1),
            body: None,
            clauses: vec![clause(
                Some(&(MAX_RESULTS_MIN - 1).to_string()),
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
            )],
        },
        Row {
            // The same bound on another operation: the rule is the model's, not
            // one list's.
            case: "ListClusters maxResults above the modelled maximum",
            method: "GET",
            uri: format!("/v1/clusters?maxResults={}", MAX_RESULTS_MAX + 1),
            body: None,
            clauses: vec![clause(
                Some(&(MAX_RESULTS_MAX + 1).to_string()),
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
            )],
        },
        Row {
            case: "ListClusters maxResults far below the modelled minimum",
            method: "GET",
            uri: "/v1/clusters?maxResults=-7".to_string(),
            body: None,
            clauses: vec![clause(
                Some("-7"),
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
            )],
        },
        Row {
            case: "CreateConfiguration without its required name",
            method: "POST",
            uri: "/v1/configurations".to_string(),
            body: Some(json!({ "serverProperties": "" })),
            clauses: vec![clause(None, "name", "Member must not be null")],
        },
        Row {
            case: "CreateConfiguration without its required serverProperties",
            method: "POST",
            uri: "/v1/configurations".to_string(),
            body: Some(json!({ "name": "no-properties" })),
            clauses: vec![clause(None, "serverProperties", "Member must not be null")],
        },
        Row {
            // A required member of another operation again, this one a
            // structure rather than a scalar.
            case: "CreateCluster without its required brokerNodeGroupInfo",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(no_broker_group),
            clauses: vec![clause(
                None,
                "brokerNodeGroupInfo",
                "Member must not be null",
            )],
        },
        Row {
            // Two violations in one request: the envelope counts them and
            // states every one.
            case: "CreateCluster violating two modelled constraints at once",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(two_violations),
            clauses: vec![
                clause(
                    Some(&too_long_name),
                    "clusterName",
                    &format!(
                        "Member must have length less than or equal to {CLUSTER_NAME_MAX_LENGTH}"
                    ),
                ),
                clause(
                    Some(&(BROKER_NODES_MAX + 1).to_string()),
                    "numberOfBrokerNodes",
                    &format!("Member must have value less than or equal to {BROKER_NODES_MAX}"),
                ),
            ],
        },
        Row {
            case: "CreateCluster clusterName shorter than the modelled minimum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(short_name_body),
            clauses: vec![clause(
                Some(""),
                "clusterName",
                &format!(
                    "Member must have length greater than or equal to {CLUSTER_NAME_MIN_LENGTH}"
                ),
            )],
        },
        Row {
            case: "CreateCluster clusterName longer than the modelled maximum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(long_name_body),
            clauses: vec![clause(
                Some(&too_long_name),
                "clusterName",
                &format!("Member must have length less than or equal to {CLUSTER_NAME_MAX_LENGTH}"),
            )],
        },
        Row {
            case: "CreateCluster numberOfBrokerNodes above the modelled maximum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(too_many_brokers),
            clauses: vec![clause(
                Some(&(BROKER_NODES_MAX + 1).to_string()),
                "numberOfBrokerNodes",
                &format!("Member must have value less than or equal to {BROKER_NODES_MAX}"),
            )],
        },
        Row {
            case: "CreateCluster numberOfBrokerNodes below the modelled minimum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(too_few_brokers),
            clauses: vec![clause(
                Some(&(BROKER_NODES_MIN - 1).to_string()),
                "numberOfBrokerNodes",
                &format!("Member must have value greater than or equal to {BROKER_NODES_MIN}"),
            )],
        },
    ];

    for row in &rows {
        let reply = emulator
            .call(row.method, &row.uri, row.body.clone())
            .await;
        assert_eq!(
            reply.status, 400,
            "case {}: expected HTTP 400, got {} {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {}: expected the modelled BadRequestException, got {}",
            row.case,
            reply.body
        );
        let message = reply.body["message"].as_str().unwrap_or_default();
        let prefix = envelope_prefix(row.clauses.len());
        assert!(
            message.starts_with(&prefix),
            "case {}: expected the refusal message to open with {prefix:?}, got {message:?}",
            row.case
        );
        for expected in &row.clauses {
            assert!(
                message.contains(expected.as_str()),
                "case {}: expected the refusal message to state {expected:?}, got {message:?}",
                row.case
            );
        }
    }
}

/// RULE: a refusal happens before the handler, so it persists nothing — neither
/// the resource the refused request named nor any other change a later reader
/// could see.
#[tokio::test]
async fn a_refused_request_persists_nothing() {
    let emulator = Emulator::new();

    // One legal resource of each kind, so the readers below have something to
    // report and a stray write would show as a change in their counts.
    let kept = emulator.create_ok(&ClusterRequest::named("kept-cluster")).await;
    let configuration = emulator
        .call(
            "POST",
            "/v1/configurations",
            Some(json!({
                "name": "kept-configuration",
                "serverProperties": ::aws_smithy_types::base64::encode("auto.create.topics.enable=true\n"),
            })),
        )
        .await;
    assert_eq!(
        configuration.status, 200,
        "expected CreateConfiguration to succeed, got {} {}",
        configuration.status, configuration.body
    );

    // Rows: a refused create of each kind, and a refused read of each list.
    let mut oversized = valid_cluster_body("refused-cluster");
    oversized["numberOfBrokerNodes"] = json!(BROKER_NODES_MAX + 1);
    let refused: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateCluster with too many broker nodes",
            "POST",
            "/v1/clusters".to_string(),
            Some(oversized),
        ),
        (
            "CreateConfiguration without serverProperties",
            "POST",
            "/v1/configurations".to_string(),
            Some(json!({ "name": "refused-configuration" })),
        ),
        (
            "ListClusters with an illegal page size",
            "GET",
            format!("/v1/clusters?maxResults={}", MAX_RESULTS_MAX + 1),
            None,
        ),
        (
            "ListConfigurations with an illegal page size",
            "GET",
            format!("/v1/configurations?maxResults={}", MAX_RESULTS_MIN - 1),
            None,
        ),
    ];
    for (case, method, uri, body) in refused {
        let reply = emulator.call(method, &uri, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
    }

    let clusters = emulator.list("").await;
    let listed: Vec<&str> = clusters.body["clusterInfoList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["clusterArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![kept.as_str()],
        "a refused request must leave the cluster registry alone, got {}",
        clusters.body
    );

    let configurations = emulator.call("GET", "/v1/configurations", None).await;
    let names: Vec<&str> = configurations.body["configurations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["kept-configuration"],
        "a refused request must leave the configuration registry alone, got {}",
        configurations.body
    );
}
