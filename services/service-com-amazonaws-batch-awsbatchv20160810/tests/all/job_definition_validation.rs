// SPDX-License-Identifier: CC0-1.0

//! What `RegisterJobDefinition` will and will not STORE, end to end over the wire.
//!
//! The rule under test: **a `RegisterJobDefinition` request carrying a value or a
//! combination of values the model's own documentation declares illegal is refused with
//! `ClientException` (HTTP 400) and nothing is stored, while one inside those bounds is
//! stored and read back unchanged — except that a variable stated with an EMPTY value says
//! nothing about the name it carries, so AWS Batch keeps no such variable at all.**
//!
//! Three documented bounds and one representation rule are asserted here, each over a
//! TABLE of inputs rather than the one case that found the defect:
//!
//! * `RetryStrategy$attempts` — "The number of times to move a job to the `RUNNABLE`
//!   status. You can specify between 1 and 10 attempts"; rows at each published bound and
//!   one past each of them;
//! * `NodeProperties$mainNode` — "This node index value must be fewer than the number of
//!   nodes"; rows either side of the relation, at several node counts;
//! * `ResourceRequirement$value` for a Fargate definition — "value must match one of the
//!   supported values and the `MEMORY` values must be one of the values supported for that
//!   `VCPU` value", over the catalogue that member's own documentation enumerates
//!   ([`FARGATE_PAIRS`]);
//! * an `environment` entry whose `value` is the empty string, over each of the places a
//!   job definition can carry a `KeyValuePair` environment.
//!
//! Each refusal's STATUS, CODE and MESSAGE are checked, not just that the call failed: the
//! three are the contract a client branches on.

use serde_json::{json, Value};

use crate::job_lifecycle::{error_code, error_message, Batch};

/// The wire error code every client-side refusal of this service carries: the only
/// client error its model declares.
const CLIENT_EXCEPTION: &str = "ClientException";

/// The envelope AWS Batch's frontend wraps THIS operation's refusals in, up to the detail
/// and the request id it ends with (`state::client_exception_message`).
const ENVELOPE_PREFIX: &str = "Error executing request, Exception : ";

/// A container-properties object that satisfies every OTHER rule, so each row below is
/// refused (or accepted) only for the reason it is about.
fn container() -> Value {
    json!({
        "image": "busybox",
        "resourceRequirements": [
            {"type": "VCPU", "value": "1"},
            {"type": "MEMORY", "value": "512"},
        ],
    })
}

/// Register `body` and answer the status, the wire code and the message.
async fn register(batch: &Batch, body: Value) -> (u16, String, String) {
    let (status, answered) = batch.call("/v1/registerjobdefinition", body).await;
    (
        status,
        error_code(&answered),
        error_message(&answered),
    )
}

/// Assert that `body` is refused with the modelled `ClientException` (HTTP 400) whose
/// detail is `detail`, inside the envelope this operation's refusals carry, and that
/// nothing was stored for `name`.
async fn refused(batch: &Batch, case: &str, name: &str, body: Value, detail: &str) {
    let (status, code, message) = register(batch, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {message:?}"
    );
    assert_eq!(
        code, CLIENT_EXCEPTION,
        "case {case:?}: expected the modelled {CLIENT_EXCEPTION}, got {code:?}"
    );
    let expected = format!("{ENVELOPE_PREFIX}{detail}, RequestId: ");
    assert!(
        message.starts_with(&expected),
        "case {case:?}: expected the message to start {expected:?}, got {message:?}"
    );
    let (status, described) = batch
        .call(
            "/v1/describejobdefinitions",
            json!({"jobDefinitionName": name}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeJobDefinitions, got {described}"
    );
    assert_eq!(
        described["jobDefinitions"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: a refused registration stores nothing, got {described}"
    );
}

/// Assert that `body` is ACCEPTED, and answer the revision it was stored as.
async fn accepted(batch: &Batch, case: &str, body: Value) -> i64 {
    let (status, answered) = batch.call("/v1/registerjobdefinition", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered}"
    );
    answered["revision"].as_i64().unwrap_or_default()
}

/// RULE: `RetryStrategy$attempts` is accepted inside the range its own doc publishes ("You
/// can specify between 1 and 10 attempts") and refused outside it; a request with no retry
/// strategy at all states no attempts and is accepted.
///
/// Rows: each published bound, one past each bound, a value far past the maximum, and the
/// absent case.
#[tokio::test]
async fn retry_strategy_attempts_is_held_to_the_documented_range() {
    let batch = Batch::new();
    // (case, the `attempts` the request states, whether AWS accepts it)
    let rows: &[(&str, Option<i64>, bool)] = &[
        ("no retryStrategy at all", None, true),
        ("the published minimum, 1", Some(1), true),
        ("inside the range, 5", Some(5), true),
        ("the published maximum, 10", Some(10), true),
        ("one below the minimum, 0", Some(0), false),
        ("one past the maximum, 11", Some(11), false),
        ("far past the maximum, 1000", Some(1000), false),
        ("negative", Some(-3), false),
    ];

    for (index, (case, attempts, admitted)) in rows.iter().enumerate() {
        let name = format!("jd-attempts-{index}");
        let mut body = json!({
            "jobDefinitionName": &name,
            "type": "container",
            "containerProperties": container(),
        });
        if let Some(attempts) = attempts {
            body["retryStrategy"] = json!({"attempts": attempts});
        }
        if *admitted {
            let revision = accepted(&batch, case, body).await;
            assert_eq!(
                revision, 1,
                "case {case:?}: the first registration of a name is revision 1, got {revision}"
            );
            let (_, described) = batch
                .call(
                    "/v1/describejobdefinitions",
                    json!({"jobDefinitionName": &name}),
                )
                .await;
            assert_eq!(
                described["jobDefinitions"][0]["retryStrategy"]["attempts"].as_i64(),
                *attempts,
                "case {case:?}: the stored revision reports what was registered, got {described}"
            );
        } else {
            refused(
                &batch,
                case,
                &name,
                body,
                "RetryAttempts must be between 1 and 10.",
            )
            .await;
        }
    }
}

/// RULE: a multi-node definition's `mainNode` must be "fewer than the number of nodes"
/// (`NodeProperties$mainNode`), so a request stating an index at or past `numNodes` is
/// refused and nothing is stored.
///
/// Rows span several node counts on both sides of the relation, including the first index
/// past each count.
#[tokio::test]
async fn a_multi_node_definition_main_node_must_be_fewer_than_num_nodes() {
    let batch = Batch::new();
    // (case, numNodes, mainNode, whether AWS accepts it)
    let rows: &[(&str, i64, i64, bool)] = &[
        ("one node, the only index", 1, 0, true),
        ("two nodes, the first", 2, 0, true),
        ("two nodes, the last", 2, 1, true),
        ("ten nodes, the last", 10, 9, true),
        ("one node, the first index past it", 1, 1, false),
        ("two nodes, the first index past them", 2, 2, false),
        ("two nodes, far past them", 2, 7, false),
        ("ten nodes, the first index past them", 10, 10, false),
    ];

    for (index, (case, num_nodes, main_node, admitted)) in rows.iter().enumerate() {
        let name = format!("jd-mainnode-{index}");
        let body = json!({
            "jobDefinitionName": &name,
            "type": "multinode",
            "nodeProperties": {
                "numNodes": num_nodes,
                "mainNode": main_node,
                "nodeRangeProperties": [
                    {"targetNodes": format!("0:{}", num_nodes - 1), "container": container()}
                ],
            },
        });
        if *admitted {
            accepted(&batch, case, body).await;
            let (_, described) = batch
                .call(
                    "/v1/describejobdefinitions",
                    json!({"jobDefinitionName": &name}),
                )
                .await;
            let stored = &described["jobDefinitions"][0]["nodeProperties"];
            assert_eq!(
                (stored["numNodes"].as_i64(), stored["mainNode"].as_i64()),
                (Some(*num_nodes), Some(*main_node)),
                "case {case:?}: the stored revision reports what was registered, got {described}"
            );
        } else {
            refused(
                &batch,
                case,
                &name,
                body,
                &format!(
                    "nodeProperties.mainNode {main_node} must be fewer than \
                     nodeProperties.numNodes {num_nodes}"
                ),
            )
            .await;
        }
    }
}

/// The vCPU/memory pairs Fargate supports, from `ResourceRequirement$value`'s own
/// documentation under `type="VCPU"` (`smithy_query(query="doc",
/// shape="ResourceRequirement", member="value")`): one entry per supported vCPU count
/// ("The supported values are 0.25, 0.5, 1, 2, 4, 8, 16, and 32") with a MEMORY value that
/// doc lists for it, and one it does not.
///
/// Every row is a pair the issue did not cite, and the two published ENDS of the catalogue
/// (the smallest reservation, 0.25 vCPU with 512 MiB, and the largest, 32 vCPU with 249856
/// MiB) are here beside a value one step past each of them.
const FARGATE_PAIRS: &[(&str, &str, &str, bool)] = &[
    ("the smallest published pair", "0.25", "512", true),
    ("0.5 vCPU with its largest memory", "0.5", "4096", true),
    ("1 vCPU with a middle memory", "1", "5120", true),
    ("2 vCPU with its largest memory", "2", "16384", true),
    ("4 vCPU with a memory only it admits", "4", "30720", true),
    ("8 vCPU with a memory only it admits", "8", "36864", true),
    ("16 vCPU with its largest memory", "16", "122880", true),
    ("the largest published pair", "32", "249856", true),
    // Past the published bounds, and inside them but wrongly paired.
    ("0.25 vCPU with memory it does not admit", "0.25", "4096", false),
    ("0.5 vCPU one step below its smallest", "0.5", "512", false),
    ("16 vCPU past the largest memory", "16", "249856", false),
    ("32 vCPU one step past the catalogue", "32", "262144", false),
    ("a vCPU count Fargate does not publish", "0.3", "2048", false),
    ("a vCPU count past the published maximum", "64", "122880", false),
    ("a memory that is not a multiple it admits", "2", "4097", false),
];

/// RULE: for a definition that runs on Fargate, the `VCPU` and `MEMORY` reservations must
/// be one of the pairs `ResourceRequirement$value` publishes; a pair outside that catalogue
/// is refused and nothing is stored. The same reservation on an EC2 definition is not held
/// to the catalogue at all — the pairing is documented "for jobs that are running on
/// Fargate resources".
#[tokio::test]
async fn a_fargate_definition_is_held_to_the_published_vcpu_memory_pairs() {
    let batch = Batch::new();

    for (index, (case, vcpu, memory, admitted)) in FARGATE_PAIRS.iter().enumerate() {
        let name = format!("jd-fargate-{index}");
        let properties = json!({
            "image": "busybox",
            "executionRoleArn": "arn:aws:iam::000000000000:role/ecsTaskExecutionRole",
            "resourceRequirements": [
                {"type": "VCPU", "value": vcpu},
                {"type": "MEMORY", "value": memory},
            ],
        });
        let body = json!({
            "jobDefinitionName": &name,
            "type": "container",
            "platformCapabilities": ["FARGATE"],
            "containerProperties": properties,
        });
        if *admitted {
            accepted(&batch, case, body).await;
        } else {
            refused(
                &batch,
                case,
                &name,
                body,
                &format!(
                    "Fargate resource requirements ({vcpu} vCPU, {memory} MiB) not valid."
                ),
            )
            .await;
        }

        // The SAME reservation on an EC2 definition (the default when no
        // `platformCapabilities` is stated) is accepted whatever the pair: the rule is
        // Fargate's.
        let ec2_name = format!("jd-ec2-{index}");
        accepted(
            &batch,
            case,
            json!({
                "jobDefinitionName": &ec2_name,
                "type": "container",
                "containerProperties": {
                    "image": "busybox",
                    "resourceRequirements": [
                        {"type": "VCPU", "value": vcpu},
                        {"type": "MEMORY", "value": memory},
                    ],
                },
            }),
        )
        .await;
    }
}

/// RULE: an `environment` entry whose `value` is the EMPTY STRING says nothing about the
/// name it carries, so AWS Batch stores no such variable and a read of the revision cannot
/// tell that name apart from one the request never mentioned. Every other value is stored
/// exactly as sent.
///
/// Rows differ along the value (empty, a single space, `"0"`, a long one, non-ASCII) and
/// along WHERE the environment is carried: the top-level `containerProperties`, a
/// multi-node range's `container`, and an `ecsProperties` task container.
#[tokio::test]
async fn an_empty_valued_environment_variable_is_not_stored() {
    let batch = Batch::new();
    // (case, the value the request states, whether the name survives the write)
    let rows: &[(&str, &str, bool)] = &[
        ("the empty string", "", false),
        ("a single space", " ", true),
        ("the digit zero", "0", true),
        ("the word false", "false", true),
        ("a long value", "x", true),
        ("a non-ASCII value", "naïve ✅", true),
    ];

    for (index, (case, value, kept)) in rows.iter().enumerate() {
        let environment = json!([
            {"name": "KEPT", "value": "always"},
            {"name": "UNDER_TEST", "value": value},
        ]);

        // 1. The top-level containerProperties.
        let name = format!("jd-env-container-{index}");
        accepted(
            &batch,
            case,
            json!({
                "jobDefinitionName": &name,
                "type": "container",
                "containerProperties": {"image": "busybox", "environment": environment},
            }),
        )
        .await;
        let (_, described) = batch
            .call(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": &name}),
            )
            .await;
        let stored = &described["jobDefinitions"][0]["containerProperties"]["environment"];
        assert_names(case, "containerProperties", stored, value, *kept);

        // 2. A multi-node range's own container.
        let node_name = format!("jd-env-node-{index}");
        accepted(
            &batch,
            case,
            json!({
                "jobDefinitionName": &node_name,
                "type": "multinode",
                "nodeProperties": {
                    "numNodes": 2,
                    "mainNode": 0,
                    "nodeRangeProperties": [{
                        "targetNodes": "0:1",
                        "container": {"image": "busybox", "environment": environment},
                    }],
                },
            }),
        )
        .await;
        let (_, described) = batch
            .call(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": &node_name}),
            )
            .await;
        let stored = &described["jobDefinitions"][0]["nodeProperties"]
            ["nodeRangeProperties"][0]["container"]["environment"];
        assert_names(case, "a node range's container", stored, value, *kept);

        // 3. An ecsProperties task container.
        let ecs_name = format!("jd-env-ecs-{index}");
        accepted(
            &batch,
            case,
            json!({
                "jobDefinitionName": &ecs_name,
                "type": "container",
                "ecsProperties": {
                    "taskProperties": [{
                        "containers": [{
                            "image": "busybox",
                            "name": "main",
                            "environment": environment,
                        }],
                    }],
                },
            }),
        )
        .await;
        let (_, described) = batch
            .call(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": &ecs_name}),
            )
            .await;
        let stored = &described["jobDefinitions"][0]["ecsProperties"]["taskProperties"][0]
            ["containers"][0]["environment"];
        assert_names(case, "an ecsProperties container", stored, value, *kept);
    }
}

/// Assert what a stored `environment` list reports: the name whose value was non-empty is
/// always there with the value that was sent, and `UNDER_TEST` is there exactly when `kept`.
fn assert_names(case: &str, place: &str, stored: &Value, value: &str, kept: bool) {
    let pairs: Vec<(String, String)> = stored
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|pair| {
            (
                pair["name"].as_str().unwrap_or_default().to_string(),
                pair["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        pairs.iter().any(|(name, held)| name == "KEPT" && held == "always"),
        "case {case:?} in {place}: the non-empty variable must survive, got {pairs:?}"
    );
    let under_test = pairs.iter().find(|(name, _)| name == "UNDER_TEST");
    if kept {
        assert_eq!(
            under_test.map(|(_, held)| held.as_str()),
            Some(value),
            "case {case:?} in {place}: expected the value {value:?} stored, got {pairs:?}"
        );
    } else {
        assert!(
            under_test.is_none(),
            "case {case:?} in {place}: an empty value stores no variable at all, got {pairs:?}"
        );
    }
}
