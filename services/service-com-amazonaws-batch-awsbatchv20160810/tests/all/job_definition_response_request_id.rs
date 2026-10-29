// SPDX-License-Identifier: CC0-1.0

//! How a `RegisterJobDefinition` answer NAMES the request it answered, end to end over the
//! wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts and
//! reads the response HEADER an SDK surfaces as that call's request id — the value a
//! `batch` client logs as its response metadata — so what is asserted is what a client can
//! observe and correlate, not an internal call.
//!
//! The rule under test, asserted over a TABLE of registrations rather than the one request
//! the issue recorded: every answer this operation sends names the request it answered in
//! the `x-amzn-requestid` header, and where the answer's own document STATES that id — the
//! `RequestId:` segment of Batch's frontend envelope, which wraps this operation's
//! refusals — the header reports the SAME string, so a client that reconstructs the message
//! from its response metadata gets the message it was sent.
//!
//! The rows vary what the rule generalises over: whether the answer is a refusal or an
//! acceptance; which documented rule the refusal is of (the Fargate execution role, the
//! Fargate reservation catalogue, the `type`/properties combinations, the
//! `RetryStrategy$attempts` range, the `timeout` minimum, the `schedulingPriority` range,
//! the `mainNode`/`numNodes` relation); WHERE the definition carries its container
//! properties (`containerProperties`, `ecsProperties`, `nodeProperties`); and the LAYER
//! that produced the answer (the handler, and the boundary validator for a required member
//! the request omits, whose message states no id at all).

use serde_json::{json, Value};

use crate::job_lifecycle::{error_code, error_message, Batch};
use crate::refusal_contract::{assert_uuid_shaped, enveloped, ENVELOPE_REQUEST_ID};

/// The header AWS reports a response's own request id in, and the name an SDK reads its
/// response metadata off. Written out as the literal a client sees rather than taken from
/// the service's constant, so changing that constant fails this suite instead of moving
/// with it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The only client error this service's model declares, so the code every refusal below
/// carries.
const CLIENT_EXCEPTION: &str = "ClientException";

/// The route this operation binds.
const REGISTER_URI: &str = "/v1/registerjobdefinition";

/// An execution-role ARN a Fargate definition can name.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/ecsTaskExecutionRole";

/// The detail Batch states when a definition that runs on Fargate names no execution role
/// (`executionRoleArn`: "For jobs that run on Fargate resources, you must provide an
/// execution role").
const NO_EXECUTION_ROLE: &str = "executionRoleArn is required for Fargate jobs.";

/// What the answer to one row is, which decides what its document can state about its own
/// request id.
enum Answer {
    /// Refused inside Batch's frontend envelope over this detail: the document states the
    /// id, so the header has to report that same string.
    Enveloped(String),
    /// Refused with a message that states no request id at all, so the header is the only
    /// place a client can read one.
    Bare,
    /// Accepted: a success names its request exactly as a failure does.
    Accepted,
}

/// One registration: the request, the name it registers under (so the store can be read
/// back afterwards) and the answer it is expected to get.
struct Row {
    /// What this row varies; named in every assertion.
    label: &'static str,
    name: &'static str,
    body: Value,
    answer: Answer,
}

/// The container properties of a Fargate job: `role` is the execution role it names (none
/// at all when `None`), `vcpu` and `memory` its reservation.
fn fargate_container(role: Option<&str>, vcpu: &str, memory: &str) -> Value {
    let mut properties = json!({
        "image": "public.ecr.aws/amazonlinux/amazonlinux:latest",
        "resourceRequirements": [
            {"type": "VCPU", "value": vcpu},
            {"type": "MEMORY", "value": memory},
        ],
    });
    if let Some(arn) = role {
        properties["executionRoleArn"] = json!(arn);
    }
    properties
}

/// A container-properties object that breaks no rule, for the rows that are about another
/// member entirely.
fn plain_container() -> Value {
    json!({
        "image": "busybox",
        "resourceRequirements": [
            {"type": "VCPU", "value": "1"},
            {"type": "MEMORY", "value": "512"},
        ],
    })
}

/// Every class of `RegisterJobDefinition` answer: one row per documented rule it refuses
/// on, a refusal the boundary validator words itself, and two acceptances.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded call: a Fargate definition whose containerProperties names no
            // execution role.
            label: "FARGATE containerProperties naming no executionRoleArn",
            name: "jd-rid-fargate-no-role",
            body: json!({
                "jobDefinitionName": "jd-rid-fargate-no-role",
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": fargate_container(None, "0.25", "512"),
            }),
            answer: Answer::Enveloped(NO_EXECUTION_ROLE.to_string()),
        },
        Row {
            // The same rule, stated as an EMPTY role, with another reservation.
            label: "FARGATE containerProperties naming an empty executionRoleArn",
            name: "jd-rid-fargate-empty-role",
            body: json!({
                "jobDefinitionName": "jd-rid-fargate-empty-role",
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": fargate_container(Some(""), "2", "16384"),
            }),
            answer: Answer::Enveloped(NO_EXECUTION_ROLE.to_string()),
        },
        Row {
            // The same rule, on the OTHER place a definition carries container
            // properties: the second `ecsProperties` task names no role.
            label: "FARGATE ecsProperties whose second task names no executionRoleArn",
            name: "jd-rid-fargate-ecs",
            body: json!({
                "jobDefinitionName": "jd-rid-fargate-ecs",
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "ecsProperties": {
                    "taskProperties": [
                        {
                            "executionRoleArn": EXECUTION_ROLE,
                            "containers": [{"name": "first", "image": "busybox"}],
                        },
                        {"containers": [{"name": "second", "image": "busybox"}]},
                    ],
                },
            }),
            answer: Answer::Enveloped(NO_EXECUTION_ROLE.to_string()),
        },
        Row {
            // A reservation outside the catalogue `ResourceRequirement$value` publishes
            // for Fargate: a refusal whose detail is built from the request.
            label: "FARGATE containerProperties reserving a pair Fargate does not publish",
            name: "jd-rid-fargate-pair",
            body: json!({
                "jobDefinitionName": "jd-rid-fargate-pair",
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": fargate_container(Some(EXECUTION_ROLE), "0.25", "4096"),
            }),
            answer: Answer::Enveloped(
                "Fargate resource requirements (0.25 vCPU, 4096 MiB) not valid.".to_string(),
            ),
        },
        Row {
            label: "type container carrying no properties object at all",
            name: "jd-rid-no-properties",
            body: json!({
                "jobDefinitionName": "jd-rid-no-properties",
                "type": "container",
            }),
            answer: Answer::Enveloped(
                "ECS ContainerProperties, ecsProperties, or eksProperties is required when \
                 using type container."
                    .to_string(),
            ),
        },
        Row {
            label: "type multinode carrying no nodeProperties",
            name: "jd-rid-no-node-properties",
            body: json!({
                "jobDefinitionName": "jd-rid-no-node-properties",
                "type": "multinode",
            }),
            answer: Answer::Enveloped("Node properties of multinode job is required.".to_string()),
        },
        Row {
            label: "a multinode definition on FARGATE",
            name: "jd-rid-multinode-fargate",
            body: json!({
                "jobDefinitionName": "jd-rid-multinode-fargate",
                "type": "multinode",
                "platformCapabilities": ["FARGATE"],
                "nodeProperties": {
                    "numNodes": 2,
                    "mainNode": 0,
                    "nodeRangeProperties": [{
                        "targetNodes": "0:1",
                        "container": fargate_container(Some(EXECUTION_ROLE), "0.25", "512"),
                    }],
                },
            }),
            answer: Answer::Enveloped(
                "multinode job definitions aren't supported on Fargate or on Amazon ECS \
                 Managed Instances"
                    .to_string(),
            ),
        },
        Row {
            // One past the maximum `RetryStrategy$attempts` publishes ("You can specify
            // between 1 and 10 attempts").
            label: "retryStrategy.attempts one past its published maximum",
            name: "jd-rid-attempts",
            body: json!({
                "jobDefinitionName": "jd-rid-attempts",
                "type": "container",
                "containerProperties": plain_container(),
                "retryStrategy": {"attempts": 11},
            }),
            answer: Answer::Enveloped("RetryAttempts must be between 1 and 10.".to_string()),
        },
        Row {
            // Below the minimum `timeout` publishes ("The minimum value for the timeout is
            // 60 seconds").
            label: "timeout.attemptDurationSeconds below its documented minimum",
            name: "jd-rid-timeout",
            body: json!({
                "jobDefinitionName": "jd-rid-timeout",
                "type": "container",
                "containerProperties": plain_container(),
                "timeout": {"attemptDurationSeconds": 30},
            }),
            answer: Answer::Enveloped(
                "timeout.attemptDurationSeconds must be at least 60 seconds".to_string(),
            ),
        },
        Row {
            // One past the maximum `schedulingPriority` publishes ("the maximum supported
            // value is 9999").
            label: "schedulingPriority one past its published maximum",
            name: "jd-rid-priority",
            body: json!({
                "jobDefinitionName": "jd-rid-priority",
                "type": "container",
                "containerProperties": plain_container(),
                "schedulingPriority": 10000,
            }),
            answer: Answer::Enveloped(
                "schedulingPriority must be between 0 and 9999".to_string(),
            ),
        },
        Row {
            // `NodeProperties$mainNode`: "This node index value must be fewer than the
            // number of nodes".
            label: "nodeProperties.mainNode at numNodes",
            name: "jd-rid-main-node",
            body: json!({
                "jobDefinitionName": "jd-rid-main-node",
                "type": "multinode",
                "nodeProperties": {
                    "numNodes": 2,
                    "mainNode": 2,
                    "nodeRangeProperties": [
                        {"targetNodes": "0:1", "container": plain_container()}
                    ],
                },
            }),
            answer: Answer::Enveloped(
                "nodeProperties.mainNode 2 must be fewer than nodeProperties.numNodes 2"
                    .to_string(),
            ),
        },
        Row {
            // A required member the request omits: refused by the boundary validator
            // before the handler ran, in words that state no request id, so only the
            // header carries one.
            label: "a request omitting the required jobDefinitionName",
            name: "jd-rid-absent-name",
            body: json!({
                "type": "container",
                "containerProperties": plain_container(),
            }),
            answer: Answer::Bare,
        },
        Row {
            label: "an EC2 container definition the service accepts",
            name: "jd-rid-accepted-ec2",
            body: json!({
                "jobDefinitionName": "jd-rid-accepted-ec2",
                "type": "container",
                "containerProperties": plain_container(),
            }),
            answer: Answer::Accepted,
        },
        Row {
            label: "a FARGATE definition naming its execution role",
            name: "jd-rid-accepted-fargate",
            body: json!({
                "jobDefinitionName": "jd-rid-accepted-fargate",
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": fargate_container(Some(EXECUTION_ROLE), "1", "5120"),
            }),
            answer: Answer::Accepted,
        },
    ]
}

/// The request id this answer reports, read the way a client reads it off the response.
fn reported_request_id(case: &str, headers: &http::HeaderMap, body: &Value) -> String {
    let reported = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the response to name its request in \
                 {REQUEST_ID_HEADER:?}, got the headers {:?} and the body {body}",
                headers.keys().collect::<Vec<_>>()
            )
        })
        .to_string();
    assert_uuid_shaped(case, &reported);
    reported
}

/// How many revisions the registry holds under `name`.
async fn stored_revisions(batch: &Batch, case: &str, name: &str) -> usize {
    let (status, described) = batch
        .call("/v1/describejobdefinitions", json!({"jobDefinitionName": name}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeJobDefinitions, got {status} {described}"
    );
    described["jobDefinitions"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: every `RegisterJobDefinition` answer names the request it answered in the
/// `x-amzn-requestid` header, and an answer whose document states that id states the SAME
/// string the header reports — so the message a client rebuilds from its response metadata
/// is the message it was sent.
#[tokio::test]
async fn every_register_job_definition_answer_names_the_request_it_answered() {
    let batch = Batch::new();

    for row in rows() {
        let case = row.label;
        let (status, headers, body) = batch
            .send_reported("POST", REGISTER_URI, Some(row.body.clone()))
            .await;
        let reported = reported_request_id(case, &headers, &body);

        match &row.answer {
            Answer::Enveloped(detail) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, CLIENT_EXCEPTION,
                    "case {case:?}: expected the modelled {CLIENT_EXCEPTION}, got {code:?}"
                );
                let message = error_message(&body);
                let (stated_detail, stated_id) = enveloped(case, &message);
                assert_eq!(
                    &stated_detail, detail,
                    "case {case:?}: expected the detail {detail:?}, got {stated_detail:?}"
                );
                // The assertion a client makes: the message it rebuilds from the detail and
                // its own response metadata is the message it was sent.
                assert_eq!(
                    stated_id, reported,
                    "case {case:?}: the id the message states and the id the header reports \
                     must be one string, got {stated_id:?} in the message and {reported:?} \
                     in the header"
                );
                assert_eq!(
                    stored_revisions(&batch, case, row.name).await,
                    0,
                    "case {case:?}: a refused registration stores nothing"
                );
            }
            Answer::Bare => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                let message = error_message(&body);
                assert!(
                    !message.contains(ENVELOPE_REQUEST_ID),
                    "case {case:?}: this answer states no request id in its document, got \
                     {message:?}"
                );
                assert_eq!(
                    stored_revisions(&batch, case, row.name).await,
                    0,
                    "case {case:?}: a refused registration stores nothing"
                );
            }
            Answer::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {body}"
                );
                assert_eq!(
                    body["revision"].as_i64(),
                    Some(1),
                    "case {case:?}: the first registration of a name is revision 1, got {body}"
                );
                let message = error_message(&body);
                assert!(
                    !message.contains(ENVELOPE_REQUEST_ID),
                    "case {case:?}: an accepted registration states no request id in its \
                     document, got {message:?}"
                );
                assert_eq!(
                    stored_revisions(&batch, case, row.name).await,
                    1,
                    "case {case:?}: an accepted registration is stored"
                );
            }
        }
    }
}

/// RULE (the id is the CALL's): the id a `RegisterJobDefinition` answer reports is minted
/// per request, so the same call made twice reports two ids — and where the document states
/// the id, each answer's message states its OWN reported id rather than the other's.
#[tokio::test]
async fn two_register_job_definition_answers_never_name_the_same_request() {
    let batch = Batch::new();
    let mut reported: Vec<String> = Vec::new();

    for row in rows() {
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", row.label);
            let (_, headers, body) = batch
                .send_reported("POST", REGISTER_URI, Some(row.body.clone()))
                .await;
            let id = reported_request_id(&case, &headers, &body);
            if let Answer::Enveloped(_) = row.answer {
                let (_, stated_id) = enveloped(&case, &error_message(&body));
                assert_eq!(
                    stated_id, id,
                    "case {case:?}: every answer states the id it reports, got {stated_id:?} \
                     in the message and {id:?} in the header"
                );
            }
            reported.push(id);
        }
    }

    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported.len(),
        "case every answered call: expected one request id per call, got {reported:?}"
    );
}
