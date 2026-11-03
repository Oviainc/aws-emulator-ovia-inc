// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request the MODEL refuses, before any handler ran:
//! the error code MSK declares for bad client input, and the ONE fixed sentence
//! it words such a refusal in.
//!
//! The rule under test is service-wide, so the table below spans the violation
//! classes the model can produce (a value above a `range` maximum, one below its
//! minimum, a string shorter and longer than a `length` bound, an absent
//! required scalar and an absent required structure, and two constraint
//! violations in one request) ACROSS operations (`ListConfigurations`,
//! `ListClusters`, `CreateConfiguration`, `CreateCluster`) — every one of them is
//! answered with the same text, which is what makes it the service's sentence
//! rather than a report of the request. Every bound and member path is taken
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
const CLUSTER_NAME_MAX_LENGTH: usize = 64;
const BROKER_NODES_MIN: i64 = 1;
const BROKER_NODES_MAX: i64 = 15;

/// The sentence Amazon MSK refuses a parameter it will not accept with: fixed
/// text end to end, naming neither the member at fault, nor the bound it broke,
/// nor the value submitted.
///
/// Shared with the other suites of this binary, so one wording serves every
/// operation's pre-handler refusal rather than one copy per suite.
pub(crate) const PARAMETERS_NOT_VALID: &str = "One or more of the parameters are not valid.";

/// What a row submitted: the member paths and the values, so an assertion can
/// check the refusal states NONE of them.
struct Submitted {
    paths: Vec<&'static str>,
    values: Vec<String>,
}

/// A `CreateCluster` body whose required members are all present and legal, so a
/// row that edits ONE member exercises that member's constraint alone.
fn valid_cluster_body(name: &str) -> Value {
    ClusterRequest::named(name).to_json()
}

/// One row of the table: a request the model refuses, plus the member paths and
/// submitted values its refusal must state NOTHING of.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    submitted: Submitted,
}

/// RULE: a request the model refuses before any handler runs is answered with
/// HTTP 400 and `BadRequestException` — the client-input error every MSK
/// operation declares, MSK's model declaring no `ValidationException` anywhere —
/// and with the service's ONE fixed sentence for a parameter it will not accept
/// ([`PARAMETERS_NOT_VALID`]): the same text for every violation class, naming
/// neither the member at fault nor the bound it broke nor the value submitted,
/// however many constraints one request breaks.
#[tokio::test]
async fn a_refused_request_is_answered_with_the_services_one_parameter_sentence() {
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
    // Two constraint violations in ONE request: the answer is still the one
    // sentence, so a refusal cannot be read as a count of what went wrong.
    let mut two_violations = valid_cluster_body("placeholder");
    two_violations["clusterName"] = json!(too_long_name);
    two_violations["numberOfBrokerNodes"] = json!(BROKER_NODES_MAX + 1);

    let rows = vec![
        Row {
            case: "ListConfigurations maxResults above the modelled maximum",
            method: "GET",
            uri: format!("/v1/configurations?maxResults={}", MAX_RESULTS_MAX + 1),
            body: None,
            submitted: Submitted {
                paths: vec!["maxResults"],
                values: vec![(MAX_RESULTS_MAX + 1).to_string()],
            },
        },
        Row {
            case: "ListConfigurations maxResults below the modelled minimum",
            method: "GET",
            uri: format!("/v1/configurations?maxResults={}", MAX_RESULTS_MIN - 1),
            body: None,
            submitted: Submitted {
                paths: vec!["maxResults"],
                values: vec![(MAX_RESULTS_MIN - 1).to_string()],
            },
        },
        Row {
            // The same bound on another operation: the rule is the service's,
            // not one list's.
            case: "ListClusters maxResults above the modelled maximum",
            method: "GET",
            uri: format!("/v1/clusters?maxResults={}", MAX_RESULTS_MAX + 1),
            body: None,
            submitted: Submitted {
                paths: vec!["maxResults"],
                values: vec![(MAX_RESULTS_MAX + 1).to_string()],
            },
        },
        Row {
            case: "ListClusters maxResults far below the modelled minimum",
            method: "GET",
            uri: "/v1/clusters?maxResults=-7".to_string(),
            body: None,
            submitted: Submitted {
                paths: vec!["maxResults"],
                values: vec!["-7".to_string()],
            },
        },
        Row {
            case: "CreateConfiguration without its required name",
            method: "POST",
            uri: "/v1/configurations".to_string(),
            body: Some(json!({ "serverProperties": "" })),
            submitted: Submitted {
                paths: vec!["name"],
                values: Vec::new(),
            },
        },
        Row {
            case: "CreateConfiguration without its required serverProperties",
            method: "POST",
            uri: "/v1/configurations".to_string(),
            body: Some(json!({ "name": "no-properties" })),
            submitted: Submitted {
                paths: vec!["serverProperties"],
                values: Vec::new(),
            },
        },
        Row {
            // A required member of another operation again, this one a
            // structure rather than a scalar.
            case: "CreateCluster without its required brokerNodeGroupInfo",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(no_broker_group),
            submitted: Submitted {
                paths: vec!["brokerNodeGroupInfo"],
                values: Vec::new(),
            },
        },
        Row {
            // Two violations in one request: still the one sentence.
            case: "CreateCluster violating two modelled constraints at once",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(two_violations),
            submitted: Submitted {
                paths: vec!["clusterName", "numberOfBrokerNodes"],
                values: vec![too_long_name.clone(), (BROKER_NODES_MAX + 1).to_string()],
            },
        },
        Row {
            case: "CreateCluster clusterName shorter than the modelled minimum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(short_name_body),
            submitted: Submitted {
                paths: vec!["clusterName"],
                // The submitted value is the empty string, which every text
                // contains, so only the member path is checked for this row.
                values: Vec::new(),
            },
        },
        Row {
            case: "CreateCluster clusterName longer than the modelled maximum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(long_name_body),
            submitted: Submitted {
                paths: vec!["clusterName"],
                values: vec![too_long_name.clone()],
            },
        },
        Row {
            case: "CreateCluster numberOfBrokerNodes above the modelled maximum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(too_many_brokers),
            submitted: Submitted {
                paths: vec!["numberOfBrokerNodes"],
                values: vec![(BROKER_NODES_MAX + 1).to_string()],
            },
        },
        Row {
            case: "CreateCluster numberOfBrokerNodes below the modelled minimum",
            method: "POST",
            uri: "/v1/clusters".to_string(),
            body: Some(too_few_brokers),
            submitted: Submitted {
                paths: vec!["numberOfBrokerNodes"],
                values: vec![(BROKER_NODES_MIN - 1).to_string()],
            },
        },
    ];

    for row in &rows {
        let reply = emulator.call(row.method, &row.uri, row.body.clone()).await;
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
        assert_eq!(
            message, PARAMETERS_NOT_VALID,
            "case {}: expected the refusal message {PARAMETERS_NOT_VALID:?}, got {message:?}",
            row.case
        );
        for path in &row.submitted.paths {
            assert!(
                !message.contains(path),
                "case {}: the refusal must not name the member {path:?}, got {message:?}",
                row.case
            );
        }
        for value in &row.submitted.values {
            assert!(
                !message.contains(value.as_str()),
                "case {}: the refusal must not echo the submitted {value:?}, got {message:?}",
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
