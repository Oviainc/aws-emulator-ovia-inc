// SPDX-License-Identifier: CC0-1.0

//! The `UnknownResourceFault` message form, over every operation of this service that
//! reports a missing execution or a missing registered type, end to end over the
//! service's own wire protocol through the same `MultiServiceRouter` the emulator mounts
//! in production. The harness is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: the fault's message is built from the resource
//! whose lookup FAILED and from nothing else — the requested execution as
//! `Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]` where
//! a run was addressed and as `Unknown execution, workflowId = <workflowId>` where a
//! `workflowId` was addressed alone, the requested type as
//! `Unknown type: <Kind>=[name=<name>, version=<version>]`, and the domain as
//! `Unknown domain: <name>`. The domain the lookup was SCOPED to is never part of an
//! execution or type clause, so the same request against two domains yields the same
//! message; and which lookup fails first follows what the operation does — a read of a
//! leaf resolves the leaf directly (so it names the leaf even when the domain is the
//! missing record), while a write resolves the domain it writes into first.
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

/// The expected clause for a missing execution a request addressed by `workflowId`
/// alone: that id, with no `runId` the request never supplied.
fn unknown_execution_by_workflow_id_message(workflow_id: &str) -> String {
    format!("Unknown execution, workflowId = {workflow_id}")
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

/// Every execution-scoped operation that resolves a `(workflowId, runId)` pair: each
/// resolves it the same way, so each must report a miss the same way. `GetHistory` and
/// `Describe` take the pair in an `execution` member; `SignalWorkflowExecution` takes
/// the two ids flat, which [`execution_body`] is what hides.
const EXECUTION_OPERATIONS: [&str; 3] = [
    "DescribeWorkflowExecution",
    "GetWorkflowExecutionHistory",
    "SignalWorkflowExecution",
];

/// The request addressing `(workflow_id, run_id)` in `domain`, in the shape
/// `operation` takes it.
fn execution_body(operation: &str, domain: &str, workflow_id: &str, run_id: &str) -> Value {
    if operation == "SignalWorkflowExecution" {
        json!({
            "domain": domain,
            "workflowId": workflow_id,
            "runId": run_id,
            "signalName": "a-signal",
        })
    } else {
        json!({
            "domain": domain,
            "execution": { "workflowId": workflow_id, "runId": run_id },
        })
    }
}

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
            for operation in EXECUTION_OPERATIONS {
                let label = format!("{domain} / {} / {operation}", case.case);
                let message = unknown_resource(
                    &emulator,
                    &label,
                    operation,
                    &execution_body(operation, domain, &case.workflow_id, &case.run_id),
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
async fn an_execution_addressed_by_workflow_id_alone_is_named_by_that_id_alone() {
    // The three operations that take an OPTIONAL `runId` resolve "the current open
    // workflow execution with the specified workflowId" when none is given, so the
    // fault names the id the request supplied and renders no `runId` it never asked
    // about. One row per shape of the id, in a registered domain holding a run of its
    // own, so the refusal is about the addressed execution and not about the domain.
    let emulator = Emulator::new();
    let domain = "addressed-by-workflow-id";
    seeded(
        &emulator,
        domain,
        domain,
        (TYPE_NAME, TYPE_VERSION),
        full_defaults("tasks", "TERMINATE"),
    )
    .await;
    emulator
        .ok(
            "a stored run",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "stored-run",
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        )
        .await;

    let operations: [(&str, Value); 3] = [
        (
            "SignalWorkflowExecution",
            json!({ "signalName": "a-signal" }),
        ),
        ("RequestCancelWorkflowExecution", json!({})),
        ("TerminateWorkflowExecution", json!({})),
    ];
    let workflow_ids = [
        "never-started".to_string(),
        "注文-ワークフロー".to_string(),
        "orders#2026-01-01".to_string(),
        "w".repeat(256),
    ];

    for workflow_id in &workflow_ids {
        for (operation, extra) in &operations {
            let label = format!("{operation} / {workflow_id}");
            let mut body = json!({ "domain": domain, "workflowId": workflow_id });
            crate::workflow_execution_lifecycle::merge(&mut body, extra.clone());
            let message = unknown_resource(&emulator, &label, operation, &body).await;
            assert_eq!(
                message,
                unknown_execution_by_workflow_id_message(workflow_id),
                "case {label:?}: the fault names the workflowId the request supplied, \
                 with no runId clause, got {message:?}"
            );
            assert!(
                !message.contains("runId"),
                "case {label:?}: a runId the request never supplied is not rendered, \
                 got {message:?}"
            );
        }
    }

    // The two cancellation operations report this form whichever way the execution was
    // addressed: the recorded answers name the `workflowId` alone even for a request
    // that DID supply a `runId`, where `SignalWorkflowExecution` carries the pair (the
    // row for that is in `an_unknown_execution_fault_names_the_requested_execution_only`).
    for operation in ["RequestCancelWorkflowExecution", "TerminateWorkflowExecution"] {
        for run_id in ["bogus-run-id", "stored-run"] {
            let label = format!("{operation} / with runId {run_id}");
            let message = unknown_resource(
                &emulator,
                &label,
                operation,
                &json!({
                    "domain": domain,
                    "workflowId": "never-started",
                    "runId": run_id,
                }),
            )
            .await;
            assert_eq!(
                message,
                unknown_execution_by_workflow_id_message("never-started"),
                "case {label:?}: this operation reports the workflowId form whichever \
                 way the execution was addressed, got {message:?}"
            );
        }
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
async fn an_absent_domain_is_reported_as_the_resource_whose_lookup_failed() {
    // The rule one level up: the fault names the resource whose lookup failed, and which
    // lookup runs first follows what the operation does. An operation that only READS a
    // leaf inside a domain (describing one type, describing or paging one execution)
    // resolves that leaf directly, so it names the leaf even when it is the DOMAIN that
    // is absent; an operation that WRITES — registering, deprecating, undeprecating or
    // deleting a type, starting, signalling, cancelling or terminating a run — resolves
    // the domain it writes into first, so an absent domain is named as the domain, as is
    // a domain an operation merely works inside (a listing, a poll) or addresses
    // directly (a domain lifecycle call).
    let emulator = Emulator::new();
    let absent = "no-such-domain";

    // The reads: the message is the leaf clause, whichever kind of leaf is named.
    let leaf_requests: Vec<(&str, Value, String)> = vec![
        (
            "DescribeWorkflowType",
            json!({
                "domain": absent,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
            unknown_type_message("WorkflowType", TYPE_NAME, TYPE_VERSION),
        ),
        (
            "DescribeActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
            unknown_type_message("ActivityType", TYPE_NAME, TYPE_VERSION),
        ),
        (
            "DescribeWorkflowExecution",
            json!({
                "domain": absent,
                "execution": { "workflowId": "wf", "runId": "run" },
            }),
            unknown_execution_message("wf", "run"),
        ),
        (
            "GetWorkflowExecutionHistory",
            json!({
                "domain": absent,
                "execution": { "workflowId": "wf", "runId": "run" },
            }),
            unknown_execution_message("wf", "run"),
        ),
    ];

    for (operation, body, expected) in &leaf_requests {
        let label = format!("{operation} / {expected}");
        let message = unknown_resource(&emulator, &label, operation, body).await;
        assert_eq!(
            &message, expected,
            "case {label:?}: the fault names the leaf resource the request addressed, \
             got {message:?}"
        );
        assert!(
            !message.contains(absent),
            "case {label:?}: the absent domain is not part of a leaf clause, \
             got {message:?}"
        );
    }

    // Domain-addressing, domain-scoped and domain-writing operations: the domain IS the
    // resource that could not be resolved, so the message names it.
    let domain_requests: Vec<(&str, Value)> = vec![
        ("DescribeDomain", json!({ "name": absent })),
        ("DeprecateDomain", json!({ "name": absent })),
        ("UndeprecateDomain", json!({ "name": absent })),
        ("ListActivityTypes", json!({ "domain": absent, "registrationStatus": "REGISTERED" })),
        (
            "ListOpenWorkflowExecutions",
            json!({ "domain": absent, "startTimeFilter": { "oldestDate": 0 } }),
        ),
        (
            "PollForDecisionTask",
            json!({ "domain": absent, "taskList": { "name": "tasks" } }),
        ),
        (
            "PollForActivityTask",
            json!({ "domain": absent, "taskList": { "name": "tasks" } }),
        ),
        (
            // A registration writes INTO the domain, so the domain is what must resolve.
            "RegisterWorkflowType",
            json!({ "domain": absent, "name": TYPE_NAME, "version": TYPE_VERSION }),
        ),
        (
            "RegisterActivityType",
            json!({ "domain": absent, "name": TYPE_NAME, "version": TYPE_VERSION }),
        ),
        (
            // A type's lifecycle writes to the record in the domain, for the same reason.
            "DeprecateWorkflowType",
            json!({
                "domain": absent,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "UndeprecateWorkflowType",
            json!({
                "domain": absent,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            "DeleteWorkflowType",
            json!({
                "domain": absent,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
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
        (
            "DeleteActivityType",
            json!({
                "domain": absent,
                "activityType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            // A start writes a new run into the domain, for the same reason.
            "StartWorkflowExecution",
            json!({
                "domain": absent,
                "workflowId": "wf",
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        ),
        (
            // So do the three writes addressing one run.
            "SignalWorkflowExecution",
            json!({ "domain": absent, "workflowId": "wf", "signalName": "a-signal" }),
        ),
        (
            "RequestCancelWorkflowExecution",
            json!({ "domain": absent, "workflowId": "wf" }),
        ),
        (
            "TerminateWorkflowExecution",
            json!({ "domain": absent, "workflowId": "wf" }),
        ),
    ];

    for (operation, body) in &domain_requests {
        let message = unknown_resource(&emulator, operation, operation, body).await;
        assert_eq!(
            message,
            format!("Unknown domain: {absent}"),
            "case {operation:?}: the fault names the requested domain, got {message:?}"
        );
    }
}
