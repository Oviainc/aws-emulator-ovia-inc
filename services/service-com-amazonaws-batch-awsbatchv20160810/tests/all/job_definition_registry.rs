// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the AWS Batch job-definition registry.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `batch` SDK client sees — not a handler called directly.
//! The rules under test:
//!
//! * a job definition that runs on Fargate must name an execution role in EVERY
//!   container-properties object it carries (`executionRoleArn`: "For jobs that
//!   run on Fargate resources, you must provide an execution role"); one that
//!   does not is refused with `ClientException` and nothing is stored;
//! * a registered revision is a stored resource: `DescribeJobDefinitions`
//!   answers from it, by name, by `name:revision` and by ARN;
//! * each registration of a name takes the next revision, and its ARN carries
//!   that revision;
//! * the bounds the model documents for `jobDefinitionName`,
//!   `schedulingPriority`, `timeout.attemptDurationSeconds` and `maxResults`;
//! * the `type` / properties-object combinations the model documents;
//! * a describe page is `maxResults` items plus a cursor to the next one, and a
//!   describe changes nothing a later describe sees.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::job_lifecycle::{error_code, error_message};

/// A SigV4 `Authorization` naming this service's signing name (`batch`) — how
/// the router picks the service — and the region in its credential scope, which
/// is the region every ARN the service mints carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/batch/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The detail Batch states when a Fargate job definition names no execution
/// role.
const NO_EXECUTION_ROLE: &str = "executionRoleArn is required for Fargate jobs.";

/// The envelope every `ClientException` this service raises is worded in.
const REFUSAL_PREFIX: &str = "Error executing request, Exception : ";

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null`
/// for an empty body).
async fn call(router: &MultiServiceRouter, path: &str, body: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://localhost{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::copy_from_slice(body.as_bytes()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn register(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "/v1/registerjobdefinition", body).await
}

async fn describe(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "/v1/describejobdefinitions", body).await
}

fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn int_at(body: &Value, member: &str) -> i64 {
    body.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// The `jobDefinitions` of a describe response.
fn described(body: &Value) -> Vec<Value> {
    body.get("jobDefinitions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `name:revision` of each described job definition, in response order.
fn described_keys(body: &Value) -> Vec<String> {
    described(body)
        .iter()
        .map(|definition| {
            format!(
                "{}:{}",
                string_at(definition, "jobDefinitionName"),
                int_at(definition, "revision")
            )
        })
        .collect()
}

/// The container properties of a Fargate job, with or without an execution role.
fn fargate_container(execution_role: Option<&str>) -> String {
    let role = match execution_role {
        Some(arn) => format!(r#","executionRoleArn":"{arn}""#),
        None => String::new(),
    };
    format!(
        r#"{{"image":"public.ecr.aws/amazonlinux/amazonlinux:latest",
             "resourceRequirements":[{{"type":"VCPU","value":"0.25"}},
                                     {{"type":"MEMORY","value":"512"}}]{role}}}"#
    )
}

/// An `ecsProperties` with `tasks` task objects; the task at index
/// `task_without_role`, if any, names no execution role.
fn ecs_properties(tasks: usize, task_without_role: Option<usize>) -> String {
    let task_objects: Vec<String> = (0..tasks)
        .map(|index| {
            let role = if Some(index) == task_without_role {
                String::new()
            } else {
                r#","executionRoleArn":"arn:aws:iam::000000000000:role/ecsTaskExecutionRole""#
                    .to_string()
            };
            format!(
                r#"{{"containers":[{{"name":"c{index}","image":"busybox"}}]{role}}}"#
            )
        })
        .collect();
    format!(r#"{{"taskProperties":[{}]}}"#, task_objects.join(","))
}

/// A `nodeProperties` for a two-node multi-node definition.
fn node_properties(execution_role: Option<&str>) -> String {
    format!(
        r#"{{"numNodes":2,"mainNode":0,
             "nodeRangeProperties":[{{"targetNodes":"0:1","container":{}}}]}}"#,
        fargate_container(execution_role)
    )
}

/// The one rule this issue was filed about, over the SET of places a job
/// definition can carry container properties and the set of platform
/// capabilities: an execution role is required exactly when the definition runs
/// on Fargate.
#[tokio::test]
async fn fargate_job_definition_requires_an_execution_role_in_every_container_object() {
    let role = "arn:aws:iam::000000000000:role/ecsTaskExecutionRole";
    // (case, platformCapabilities JSON, properties member, properties JSON,
    //  accepted?)
    let cases: Vec<(&str, &str, &str, String, bool)> = vec![
        (
            "FARGATE containerProperties with an execution role",
            r#"["FARGATE"]"#,
            "containerProperties",
            fargate_container(Some(role)),
            true,
        ),
        (
            "FARGATE containerProperties with no execution role",
            r#"["FARGATE"]"#,
            "containerProperties",
            fargate_container(None),
            false,
        ),
        (
            "FARGATE containerProperties with an empty execution role",
            r#"["FARGATE"]"#,
            "containerProperties",
            fargate_container(Some("")),
            false,
        ),
        (
            "FARGATE named beside EC2, containerProperties with no execution role",
            r#"["EC2","FARGATE"]"#,
            "containerProperties",
            fargate_container(None),
            false,
        ),
        (
            "EC2 containerProperties with no execution role",
            r#"["EC2"]"#,
            "containerProperties",
            fargate_container(None),
            true,
        ),
        (
            "no platform capability named, containerProperties with no execution role",
            "",
            "containerProperties",
            fargate_container(None),
            true,
        ),
        (
            "FARGATE ecsProperties, its one task with an execution role",
            r#"["FARGATE"]"#,
            "ecsProperties",
            ecs_properties(1, None),
            true,
        ),
        (
            "FARGATE ecsProperties, its one task with no execution role",
            r#"["FARGATE"]"#,
            "ecsProperties",
            ecs_properties(1, Some(0)),
            false,
        ),
        (
            "FARGATE ecsProperties, three tasks, the last with no execution role",
            r#"["FARGATE"]"#,
            "ecsProperties",
            ecs_properties(3, Some(2)),
            false,
        ),
        (
            "FARGATE ecsProperties, three tasks, all with an execution role",
            r#"["FARGATE"]"#,
            "ecsProperties",
            ecs_properties(3, None),
            true,
        ),
        (
            "EC2 ecsProperties, its one task with no execution role",
            r#"["EC2"]"#,
            "ecsProperties",
            ecs_properties(1, Some(0)),
            true,
        ),
    ];

    let router = router();
    for (index, (case, capabilities, member, properties, accepted)) in cases.iter().enumerate() {
        let name = format!("execrole{index}");
        let capabilities = if capabilities.is_empty() {
            String::new()
        } else {
            format!(r#","platformCapabilities":{capabilities}"#)
        };
        let body = format!(
            r#"{{"jobDefinitionName":"{name}","type":"container",
                 "{member}":{properties}{capabilities}}}"#
        );
        let (status, response) = register(&router, &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to be accepted, got {status} {response:?}"
            );
            assert_eq!(
                int_at(&response, "revision"),
                1,
                "case {case:?}: expected revision 1, got {response:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the registration to be refused, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ClientException",
                "case {case:?}: expected a ClientException, got {response:?}"
            );
            assert_eq!(
                error_message(&response),
                format!("{REFUSAL_PREFIX}{NO_EXECUTION_ROLE}, RequestId: None"),
                "case {case:?}: expected the Fargate execution-role refusal, got {response:?}"
            );
            // Nothing is stored by a refused registration.
            let (_, listed) = describe(
                &router,
                &format!(r#"{{"jobDefinitionName":"{name}"}}"#),
            )
            .await;
            assert!(
                described(&listed).is_empty(),
                "case {case:?}: a refused registration must store nothing, got {listed:?}"
            );
        }
    }
}

/// A registered revision is a stored resource: every reader form finds the same
/// record, with the members the request carried.
#[tokio::test]
async fn a_registered_revision_is_read_back_by_name_by_revision_and_by_arn() {
    let router = router();
    let role = "arn:aws:iam::000000000000:role/ecsTaskExecutionRole";
    let body = format!(
        r#"{{"jobDefinitionName":"roundtrip","type":"container",
             "platformCapabilities":["FARGATE"],
             "containerProperties":{},
             "parameters":{{"shard":"7"}},
             "tags":{{"team":"batch"}},
             "propagateTags":true,
             "schedulingPriority":42,
             "timeout":{{"attemptDurationSeconds":120}},
             "retryStrategy":{{"attempts":3}}}}"#,
        fargate_container(Some(role))
    );
    let (status, registered) = register(&router, &body).await;
    assert_eq!(status, 200, "register: expected 200, got {status} {registered:?}");
    let expected_arn = format!("arn:aws:batch:{REGION}:{ACCOUNT}:job-definition/roundtrip:1");
    assert_eq!(
        string_at(&registered, "jobDefinitionArn"),
        expected_arn,
        "register: expected the revision's ARN, got {registered:?}"
    );

    // A second registration of the same name takes the next revision.
    let (status, second) = register(&router, &body).await;
    assert_eq!(status, 200, "re-register: expected 200, got {status} {second:?}");
    assert_eq!(
        int_at(&second, "revision"),
        2,
        "re-register: expected revision 2, got {second:?}"
    );

    // (case, describe request, the `name:revision` keys it must return)
    let reads: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "by jobDefinitionName",
            r#"{"jobDefinitionName":"roundtrip"}"#.to_string(),
            vec!["roundtrip:1", "roundtrip:2"],
        ),
        (
            "by name:revision",
            r#"{"jobDefinitions":["roundtrip:2"]}"#.to_string(),
            vec!["roundtrip:2"],
        ),
        (
            "by ARN",
            format!(r#"{{"jobDefinitions":["{expected_arn}"]}}"#),
            vec!["roundtrip:1"],
        ),
        (
            "by bare name",
            r#"{"jobDefinitions":["roundtrip"]}"#.to_string(),
            vec!["roundtrip:1", "roundtrip:2"],
        ),
        (
            "by status ACTIVE",
            r#"{"jobDefinitionName":"roundtrip","status":"ACTIVE"}"#.to_string(),
            vec!["roundtrip:1", "roundtrip:2"],
        ),
        (
            "by status INACTIVE",
            r#"{"jobDefinitionName":"roundtrip","status":"INACTIVE"}"#.to_string(),
            vec![],
        ),
        (
            "by a name nothing was registered under",
            r#"{"jobDefinitionName":"absent"}"#.to_string(),
            vec![],
        ),
        (
            "by a revision that was never registered",
            r#"{"jobDefinitions":["roundtrip:9"]}"#.to_string(),
            vec![],
        ),
    ];
    for (case, request, expected) in reads {
        let (status, response) = describe(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            described_keys(&response),
            expected,
            "case {case:?}: expected {expected:?}, got {response:?}"
        );
    }

    // The stored record carries what the request asked for.
    let (_, response) = describe(&router, r#"{"jobDefinitions":["roundtrip:1"]}"#).await;
    let definition = described(&response).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        string_at(&definition, "status"),
        "ACTIVE",
        "read back: expected an ACTIVE revision, got {definition:?}"
    );
    assert_eq!(
        string_at(&definition, "type"),
        "container",
        "read back: expected the registered type, got {definition:?}"
    );
    assert_eq!(
        string_at(&definition, "containerOrchestrationType"),
        "ECS",
        "read back: expected ECS orchestration, got {definition:?}"
    );
    assert_eq!(
        int_at(&definition, "schedulingPriority"),
        42,
        "read back: expected the registered schedulingPriority, got {definition:?}"
    );
    assert_eq!(
        definition
            .get("containerProperties")
            .map(|container| string_at(container, "executionRoleArn")),
        Some(role.to_string()),
        "read back: expected the registered executionRoleArn, got {definition:?}"
    );
    assert_eq!(
        definition
            .get("parameters")
            .map(|parameters| string_at(parameters, "shard")),
        Some("7".to_string()),
        "read back: expected the registered parameters, got {definition:?}"
    );
    assert_eq!(
        definition
            .get("timeout")
            .map(|timeout| int_at(timeout, "attemptDurationSeconds")),
        Some(120),
        "read back: expected the registered timeout, got {definition:?}"
    );
    assert_eq!(
        definition.get("propagateTags").and_then(Value::as_bool),
        Some(true),
        "read back: expected the registered propagateTags, got {definition:?}"
    );

    // A describe is a read: a second one sees exactly what the first did.
    let (_, repeat) = describe(&router, r#"{"jobDefinitionName":"roundtrip"}"#).await;
    assert_eq!(
        described_keys(&repeat),
        vec!["roundtrip:1", "roundtrip:2"],
        "a describe must not change what a later describe sees, got {repeat:?}"
    );
}

/// Each registration of a name takes the next revision, and the ARN carries it.
#[tokio::test]
async fn revisions_of_one_name_are_numbered_from_one_upwards() {
    let router = router();
    // One row per registration count the rule spans.
    for registrations in [1_usize, 2, 5] {
        let name = format!("revseq{registrations}");
        let case = format!("{registrations} registration(s) of {name}");
        for expected_revision in 1..=registrations {
            let body = format!(
                r#"{{"jobDefinitionName":"{name}","type":"container",
                     "containerProperties":{}}}"#,
                fargate_container(None)
            );
            let (status, response) = register(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            assert_eq!(
                int_at(&response, "revision"),
                expected_revision as i64,
                "case {case:?}: expected revision {expected_revision}, got {response:?}"
            );
            assert_eq!(
                string_at(&response, "jobDefinitionArn"),
                format!(
                    "arn:aws:batch:{REGION}:{ACCOUNT}:job-definition/{name}:{expected_revision}"
                ),
                "case {case:?}: expected the ARN to carry the revision, got {response:?}"
            );
        }
        let (_, listed) = describe(&router, &format!(r#"{{"jobDefinitionName":"{name}"}}"#)).await;
        let expected: Vec<String> = (1..=registrations)
            .map(|revision| format!("{name}:{revision}"))
            .collect();
        assert_eq!(
            described_keys(&listed),
            expected,
            "case {case:?}: expected every revision to be readable, got {listed:?}"
        );
    }
}

/// The `jobDefinitionName` alphabet and length the member's doc states: "up to
/// 128 letters long. It can contain uppercase and lowercase letters, numbers,
/// hyphens (-), and underscores (_)".
#[tokio::test]
async fn job_definition_names_outside_the_documented_form_are_refused() {
    let router = router();
    let longest = "a".repeat(128);
    let too_long = "a".repeat(129);
    // (case, name, accepted?) — the published length bound and one past it.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("one letter", "a", true),
        ("letters, digits, hyphen and underscore", "Job_def-9", true),
        ("the longest name (128)", longest.as_str(), true),
        ("one past the longest (129)", too_long.as_str(), false),
        ("empty", "", false),
        ("a space", "job def", false),
        ("a dot", "job.def", false),
        ("a slash", "job/def", false),
        ("a colon", "job:def", false),
    ];
    for (case, name, accepted) in cases {
        let body = format!(
            r#"{{"jobDefinitionName":"{name}","type":"container",
                 "containerProperties":{}}}"#,
            fargate_container(None)
        );
        let (status, response) = register(&router, &body).await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&response),
                "ClientException",
                "case {case:?}: expected a ClientException, got {response:?}"
            );
        }
    }
}

/// The numeric bounds the model documents: `schedulingPriority` 0..=9999 and
/// `timeout.attemptDurationSeconds` >= 60 — each published bound, and one past
/// it.
#[tokio::test]
async fn documented_numeric_bounds_are_enforced() {
    let router = router();
    // (case, request members, accepted?)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "schedulingPriority at its minimum (0)",
            r#""schedulingPriority":0"#.to_string(),
            true,
        ),
        (
            "schedulingPriority at its maximum (9999)",
            r#""schedulingPriority":9999"#.to_string(),
            true,
        ),
        (
            "schedulingPriority one below the minimum (-1)",
            r#""schedulingPriority":-1"#.to_string(),
            false,
        ),
        (
            "schedulingPriority one above the maximum (10000)",
            r#""schedulingPriority":10000"#.to_string(),
            false,
        ),
        (
            "timeout at its minimum (60s)",
            r#""timeout":{"attemptDurationSeconds":60}"#.to_string(),
            true,
        ),
        (
            "timeout above the minimum (61s)",
            r#""timeout":{"attemptDurationSeconds":61}"#.to_string(),
            true,
        ),
        (
            "timeout one below the minimum (59s)",
            r#""timeout":{"attemptDurationSeconds":59}"#.to_string(),
            false,
        ),
        (
            "timeout of zero",
            r#""timeout":{"attemptDurationSeconds":0}"#.to_string(),
            false,
        ),
    ];
    for (index, (case, members, accepted)) in cases.iter().enumerate() {
        let body = format!(
            r#"{{"jobDefinitionName":"bounds{index}","type":"container",
                 "containerProperties":{},{members}}}"#,
            fargate_container(None)
        );
        let (status, response) = register(&router, &body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&response),
                "ClientException",
                "case {case:?}: expected a ClientException, got {response:?}"
            );
        }
    }
}

/// The `type` / properties-object combinations the model documents: a container
/// definition needs one of the three container-shaped members, a multinode one
/// needs `nodeProperties`, and the members are mutually exclusive.
#[tokio::test]
async fn the_documented_type_and_properties_combinations_are_enforced() {
    let router = router();
    let eks = r#"{"podProperties":{"containers":[{"image":"busybox"}]}}"#;
    // (case, request members, accepted?)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "container with containerProperties",
            format!(
                r#""type":"container","containerProperties":{}"#,
                fargate_container(None)
            ),
            true,
        ),
        (
            "container with ecsProperties",
            format!(r#""type":"container","ecsProperties":{}"#, ecs_properties(1, Some(0))),
            true,
        ),
        (
            "container with eksProperties",
            format!(r#""type":"container","eksProperties":{eks}"#),
            true,
        ),
        (
            "container with none of them",
            r#""type":"container""#.to_string(),
            false,
        ),
        (
            "multinode with nodeProperties",
            format!(r#""type":"multinode","nodeProperties":{}"#, node_properties(None)),
            true,
        ),
        (
            "multinode with no nodeProperties",
            format!(
                r#""type":"multinode","containerProperties":{}"#,
                fargate_container(None)
            ),
            false,
        ),
        (
            "multinode on Fargate",
            format!(
                r#""type":"multinode","platformCapabilities":["FARGATE"],"nodeProperties":{}"#,
                node_properties(Some("arn:aws:iam::000000000000:role/r"))
            ),
            false,
        ),
        (
            "containerProperties and nodeProperties together",
            format!(
                r#""type":"container","containerProperties":{},"nodeProperties":{}"#,
                fargate_container(None),
                node_properties(None)
            ),
            false,
        ),
        (
            "containerProperties and ecsProperties together",
            format!(
                r#""type":"container","containerProperties":{},"ecsProperties":{}"#,
                fargate_container(None),
                ecs_properties(1, Some(0))
            ),
            false,
        ),
        (
            "eksProperties with platformCapabilities",
            format!(r#""type":"container","platformCapabilities":["EC2"],"eksProperties":{eks}"#),
            false,
        ),
        (
            "MANAGED_INSTANCES with containerProperties",
            format!(
                r#""type":"container","platformCapabilities":["MANAGED_INSTANCES"],
                   "containerProperties":{}"#,
                fargate_container(None)
            ),
            false,
        ),
        (
            "MANAGED_INSTANCES with ecsProperties",
            format!(
                r#""type":"container","platformCapabilities":["MANAGED_INSTANCES"],
                   "ecsProperties":{}"#,
                ecs_properties(1, Some(0))
            ),
            true,
        ),
    ];
    for (index, (case, members, accepted)) in cases.iter().enumerate() {
        let body = format!(r#"{{"jobDefinitionName":"combo{index}",{members}}}"#);
        let (status, response) = register(&router, &body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&response),
                "ClientException",
                "case {case:?}: expected a ClientException, got {response:?}"
            );
        }
    }

    // An EKS definition reports EKS orchestration; every other one reports ECS.
    let (_, response) = describe(&router, r#"{"jobDefinitionName":"combo2"}"#).await;
    let definition = described(&response).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        string_at(&definition, "containerOrchestrationType"),
        "EKS",
        "an eksProperties definition must report EKS orchestration, got {definition:?}"
    );
}

/// A describe page is `maxResults` items plus a cursor to the next one, and the
/// pages together are every stored revision, exactly once.
#[tokio::test]
async fn describe_pages_through_every_stored_revision() {
    let router = router();
    let registrations = 5;
    for _ in 0..registrations {
        let body = format!(
            r#"{{"jobDefinitionName":"paged","type":"container",
                 "containerProperties":{}}}"#,
            fargate_container(None)
        );
        let (status, response) = register(&router, &body).await;
        assert_eq!(status, 200, "register: expected 200, got {status} {response:?}");
    }

    for page_size in [1_usize, 2, 5, 100] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=registrations {
            let request = match token.as_deref() {
                Some(cursor) => format!(
                    r#"{{"jobDefinitionName":"paged","maxResults":{page_size},
                         "nextToken":"{cursor}"}}"#
                ),
                None => format!(
                    r#"{{"jobDefinitionName":"paged","maxResults":{page_size}}}"#
                ),
            };
            let (status, response) = describe(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            let page = described_keys(&response);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} items, got {page:?}"
            );
            seen.extend(page);
            token = response
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging must terminate, got a cursor {token:?} after every page"
        );
        let expected: Vec<String> = (1..=registrations)
            .map(|revision| format!("paged:{revision}"))
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every revision exactly once, got {seen:?}"
        );
    }

    // (case, describe request, accepted?) — the published `maxResults` bounds,
    // one past each, and a cursor this service never minted.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("maxResults at its minimum (1)", r#"{"maxResults":1}"#, true),
        ("maxResults at its maximum (100)", r#"{"maxResults":100}"#, true),
        ("maxResults below the minimum (0)", r#"{"maxResults":0}"#, false),
        (
            "maxResults above the maximum (101)",
            r#"{"maxResults":101}"#,
            false,
        ),
        (
            "a nextToken this service never minted",
            r#"{"nextToken":"not-a-cursor"}"#,
            false,
        ),
    ];
    for (case, request, accepted) in cases {
        let (status, response) = describe(&router, request).await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response:?}"
        );
        if !accepted {
            assert_eq!(
                error_code(&response),
                "ClientException",
                "case {case:?}: expected a ClientException, got {response:?}"
            );
        }
    }
}
