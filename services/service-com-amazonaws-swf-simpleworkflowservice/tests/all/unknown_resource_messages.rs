// SPDX-License-Identifier: CC0-1.0

//! The `UnknownResourceFault` message form, over every operation of this service that
//! reports a missing execution or a missing registered type, end to end over the
//! service's own wire protocol through the same `MultiServiceRouter` the emulator mounts
//! in production. The harness is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: the fault's message is built from the resource the
//! lookup failed on and from nothing else — the requested execution as
//! `Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]`, the
//! requested type as `Unknown type: <Kind>=[name=<name>, version=<version>]`, and a
//! domain no record is stored under as `Unknown domain: <name>`. The domain the lookup
//! was SCOPED to is never part of an execution or type clause, so the same request
//! against two domains yields the same message, and no message carries the domain name
//! at all.
//!
//! Rows vary what the rule generalises over: the operation reporting the fault, the
//! resource kind, how the lookup misses (unknown id, wrong run, a record that exists
//! only in another domain, a record in a deprecated domain), the shape of the ids
//! (plain, non-ASCII, punctuated, long) and the shape of the domain name.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, register_domain_body, seeded, string_at, Emulator,
};

/// The workflow type every domain in this suite registers, with every default set so a
/// start needs no overrides.
const TYPE_NAME: &str = "orders";
const TYPE_VERSION: &str = "1.0";

/// The expected clause for a missing execution: the requested `(workflowId, runId)` and
/// nothing else.
fn unknown_execution_message(workflow_id: &str, run_id: &str) -> String {
    format!("Unknown execution: WorkflowExecution=[workflowId={workflow_id}, runId={run_id}]")
}

/// The expected clause for a missing registered type: the requested identity and nothing
/// else. `kind` is the modelled shape name, `WorkflowType` or `ActivityType`.
fn unknown_type_message(kind: &str, name: &str, version: &str) -> String {
    format!("Unknown type: {kind}=[name={name}, version={version}]")
}

/// Call one operation, require HTTP 400 `UnknownResourceFault`, and return its message.
async fn unknown_resource(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = error_code(&parsed);
    assert_eq!(
        (status, code.as_str()),
        (400, "UnknownResourceFault"),
        "case {case:?}: expected {operation} to answer 400 UnknownResourceFault, \
         got {status} {code} with {parsed:?}"
    );
    parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed:?}"))
        .to_string()
}

/// One way an `(domain, execution)` pair can fail to name a stored run.
struct ExecutionCase {
    case: &'static str,
    workflow_id: String,
    run_id: String,
}

/// Every execution-scoped reader: both resolve the pair the same way, so both must
/// report a miss the same way.
const EXECUTION_READERS: [&str; 2] = ["DescribeWorkflowExecution", "GetWorkflowExecutionHistory"];

#[tokio::test]
async fn an_unknown_execution_fault_names_the_requested_execution_only() {
    let emulator = Emulator::new();
    // Two domains of different name shapes, each holding a run of its own, so a message
    // that leaked the scoping domain would differ between them.
    let domains = ["home.domain_1", "second-domain"];
    let mut started: Vec<(String, String)> = Vec::new();
    for (index, domain) in domains.iter().enumerate() {
        seeded(
            &emulator,
            domain,
            domain,
            (TYPE_NAME, TYPE_VERSION),
            full_defaults("tasks", "TERMINATE"),
        )
        .await;
        // The id carries no part of the domain name, so an assertion that the message
        // omits the domain cannot pass by accident.
        let workflow_id = format!("run-{index}");
        let answer = emulator
            .ok(
                domain,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": workflow_id,
                    "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
                }),
            )
            .await;
        started.push((workflow_id, string_at(&answer, "/runId")));
    }

    for (index, domain) in domains.iter().enumerate() {
        let (stored_workflow_id, stored_run_id) = &started[index];
        let (other_workflow_id, other_run_id) = &started[(index + 1) % domains.len()];
        let cases = vec![
            ExecutionCase {
                case: "a workflowId nothing was started with",
                workflow_id: "never-started".to_string(),
                run_id: "bogus-run-id".to_string(),
            },
            ExecutionCase {
                // The pair is the key: a stored workflowId with a run that is not its own
                // names no execution.
                case: "a stored workflowId with a wrong runId",
                workflow_id: stored_workflow_id.clone(),
                run_id: "bogus-run-id".to_string(),
            },
            ExecutionCase {
                // The lookup is domain-scoped: this run exists, just not here.
                case: "a run of the other domain",
                workflow_id: other_workflow_id.clone(),
                run_id: other_run_id.clone(),
            },
            ExecutionCase {
                case: "non-ASCII and punctuated ids",
                workflow_id: "注文-ワークフロー#1".to_string(),
                run_id: "実行 id".to_string(),
            },
            ExecutionCase {
                case: "long ids",
                workflow_id: "w".repeat(256),
                run_id: "r".repeat(64),
            },
        ];

        for case in &cases {
            for operation in EXECUTION_READERS {
                let label = format!("{domain} / {} / {operation}", case.case);
                let message = unknown_resource(
                    &emulator,
                    &label,
                    operation,
                    &json!({
                        "domain": domain,
                        "execution": {
                            "workflowId": case.workflow_id,
                            "runId": case.run_id,
                        },
                    }),
                )
                .await;
                assert_eq!(
                    message,
                    unknown_execution_message(&case.workflow_id, &case.run_id),
                    "case {label:?}: the fault names the requested execution and nothing \
                     else, got {message:?}"
                );
                assert!(
                    !message.contains(domain),
                    "case {label:?}: the scoping domain is not part of the clause, \
                     got {message:?}"
                );
            }
        }

        // The run that IS stored here is still readable, so the refusals above are about
        // the requested pair and not about the domain.
        let described = emulator
            .ok(
                domain,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": stored_workflow_id, "runId": stored_run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {domain:?}: the stored run survives every refusal, got {described:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_type_fault_names_the_requested_type_only() {
    let emulator = Emulator::new();
    // One domain holds the types; the other holds none, so the SAME request differs only
    // in the domain it is scoped to and must still produce the identical message.
    let home = "home.domain_1";
    let elsewhere = "elsewhere-2";
    for domain in [home, elsewhere] {
        emulator
            .ok(domain, "RegisterDomain", &register_domain_body(domain, "7"))
            .await;
    }
    for (operation, extra) in [
        ("RegisterWorkflowType", full_defaults("tasks", "TERMINATE")),
        ("RegisterActivityType", json!({})),
    ] {
        let mut body = json!({ "domain": home, "name": TYPE_NAME, "version": TYPE_VERSION });
        crate::workflow_execution_lifecycle::merge(&mut body, extra);
        emulator.ok("seed", operation, &body).await;
    }

    // (operation, kind, the member carrying the identity).
    let operations = [
        ("DescribeWorkflowType", "WorkflowType", "workflowType"),
        ("DeprecateWorkflowType", "WorkflowType", "workflowType"),
        ("DeleteWorkflowType", "WorkflowType", "workflowType"),
        ("DescribeActivityType", "ActivityType", "activityType"),
        ("DeprecateActivityType", "ActivityType", "activityType"),
        ("UndeprecateActivityType", "ActivityType", "activityType"),
        ("DeleteActivityType", "ActivityType", "activityType"),
    ];

    let identities = [
        ("missing-type".to_string(), "9.9".to_string()),
        ("注文タイプ".to_string(), "版 1".to_string()),
        ("t".repeat(256), "v".repeat(64)),
        // Registered in `home` only: against `elsewhere` the identity names nothing.
        (TYPE_NAME.to_string(), TYPE_VERSION.to_string()),
    ];

    for (name, version) in &identities {
        for (operation, kind, member) in operations {
            // The identity that IS registered in `home` is only a miss in `elsewhere`.
            let domains: Vec<&str> = if name == TYPE_NAME && version == TYPE_VERSION {
                vec![elsewhere]
            } else {
                vec![home, elsewhere]
            };
            for domain in domains {
                let label = format!("{operation} on {name}/{version} in {domain}");
                let message = unknown_resource(
                    &emulator,
                    &label,
                    operation,
                    &json!({
                        "domain": domain,
                        member: { "name": name, "version": version },
                    }),
                )
                .await;
                assert_eq!(
                    message,
                    unknown_type_message(kind, name, version),
                    "case {label:?}: the fault names the requested type and nothing else, \
                     got {message:?}"
                );
                assert!(
                    !message.contains(domain),
                    "case {label:?}: the scoping domain is not part of the clause, \
                     got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_domain_no_record_is_stored_under_is_named_by_the_fault() {
    // When the lookup fails on the DOMAIN, the domain is the resource the fault names —
    // the same rule, applied one level up, over every operation that resolves a domain.
    let emulator = Emulator::new();
    let absent = "no-such-domain";

    let requests: Vec<(&str, Value)> = vec![
        (
            "DescribeDomain",
            json!({ "name": absent }),
        ),
        ("DeprecateDomain", json!({ "name": absent })),
        (
            "DescribeWorkflowExecution",
            json!({
                "domain": absent,
                "execution": { "workflowId": "wf", "runId": "run" },
            }),
        ),
        (
            "GetWorkflowExecutionHistory",
            json!({
                "domain": absent,
                "execution": { "workflowId": "wf", "runId": "run" },
            }),
        ),
        (
            "DescribeWorkflowType",
            json!({
                "domain": absent,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "DescribeActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "DeleteActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "DeprecateActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "UndeprecateActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
    ];

    for (operation, body) in &requests {
        let message = unknown_resource(&emulator, operation, operation, body).await;
        assert_eq!(
            message,
            format!("Unknown domain: {absent}"),
            "case {operation:?}: the fault names the requested domain, got {message:?}"
        );
    }
}
