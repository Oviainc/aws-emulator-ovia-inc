// SPDX-License-Identifier: CC0-1.0

//! `DeprecateWorkflowType`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//! The harness is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `DeprecateWorkflowType` moves the ONE stored
//! workflow-type record its `(domain, name, version)` triple addresses from
//! `REGISTERED` to `DEPRECATED` and stamps that record's `deprecationDate`, leaving its
//! registered configuration, every other stored type and every already-started
//! execution exactly as they were; a triple naming no stored type is HTTP 400
//! `UnknownResourceFault`, and a record that is already `DEPRECATED` is HTTP 400
//! `TypeDeprecatedFault` with nothing written. The tables below vary what that rule
//! generalises over: the form and length of `name`/`version` (plain, symbols,
//! non-ASCII, each modelled bound and one past it), which version of one name is
//! addressed, which of two domains holds a same-named type, which registration-time
//! defaults the type carries, and the way a triple fails to name a stored type.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{
    full_defaults, merge, register_domain_body, string_at, Emulator,
};

/// The modelled maximum length of `WorkflowType$name`
/// (`smithy_query(query="shape",shape="WorkflowType")`: `name` targets `Name`, len
/// 1..256).
const MAX_TYPE_NAME_LEN: usize = 256;

/// The modelled maximum length of `WorkflowType$version` (same query: `version` targets
/// `Version`, len 1..64).
const MAX_TYPE_VERSION_LEN: usize = 64;

/// The task list and child policy every type registered here carries, so a start needs
/// no overrides of its own.
const TASK_LIST: &str = "primary";
const CHILD_POLICY: &str = "TERMINATE";

/// The `workflowType` member of a request addressing one type.
fn type_ref(name: &str, version: &str) -> Value {
    json!({ "name": name, "version": version })
}

/// Register a domain with nothing in it.
async fn register_domain(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
}

/// Register `(name, version)` in `domain`, folding `defaults` into the request.
async fn register_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    name: &str,
    version: &str,
    defaults: Value,
) {
    let mut body = json!({ "domain": domain, "name": name, "version": version });
    merge(&mut body, defaults);
    emulator.ok(case, "RegisterWorkflowType", &body).await;
}

/// The stored record, read back through the implemented reader.
async fn describe_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    name: &str,
    version: &str,
) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowType",
            &json!({ "domain": domain, "workflowType": type_ref(name, version) }),
        )
        .await
}

/// A timestamp member of a response, in the epoch-seconds form the protocol carries.
fn seconds_at(case: &str, body: &Value, pointer: &str) -> f64 {
    body.pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp at {pointer} of {body:?}"))
}

/// Assert the record `described` reports is REGISTERED and carries no deprecation
/// stamp.
fn assert_registered(case: &str, described: &Value) {
    let status = string_at(described, "/typeInfo/status");
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled RegistrationStatus values \
         {REGISTRATION_STATUS_VALUES:?}, got {status:?}"
    );
    assert_eq!(
        status, "REGISTERED",
        "case {case:?}: an un-deprecated type is REGISTERED, got {described:?}"
    );
    assert_eq!(
        described.pointer("/typeInfo/deprecationDate"),
        None,
        "case {case:?}: deprecationDate is set only once the type is deprecated, \
         got {described:?}"
    );
}

/// Assert the record `described` reports is DEPRECATED and return its
/// `deprecationDate`.
fn assert_deprecated(case: &str, described: &Value) -> f64 {
    let status = string_at(described, "/typeInfo/status");
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled RegistrationStatus values \
         {REGISTRATION_STATUS_VALUES:?}, got {status:?}"
    );
    assert_eq!(
        status, "DEPRECATED",
        "case {case:?}: a deprecated type reports DEPRECATED, got {described:?}"
    );
    let deprecated_at = seconds_at(case, described, "/typeInfo/deprecationDate");
    let created_at = seconds_at(case, described, "/typeInfo/creationDate");
    assert!(
        deprecated_at >= created_at,
        "case {case:?}: the deprecation stamp cannot precede the creation stamp, \
         got {deprecated_at} before {created_at} in {described:?}"
    );
    deprecated_at
}

/// One stored workflow type a deprecation addresses. Rows differ in the form and length
/// of the identity and in which registration-time defaults the record carries.
struct TypeCase {
    case: &'static str,
    name: String,
    version: String,
    defaults: Value,
}

fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            case: "plain name, first version",
            name: "orders".to_string(),
            version: "1.0".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        // The same NAME at another version is a different record, so deprecating one
        // version must leave the other REGISTERED.
        TypeCase {
            case: "plain name, second version",
            name: "orders".to_string(),
            version: "2.0".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        // A type carrying NO defaults at all: deprecation is about the status, not the
        // configuration.
        TypeCase {
            case: "no registered defaults",
            name: "reports".to_string(),
            version: "1".to_string(),
            defaults: json!({}),
        },
        // The characters SWF's naming rule allows (everything but `:`, `/`, `|` and
        // control characters), a class the issue's own case never exercised.
        TypeCase {
            case: "symbols and spaces",
            name: "order.flow_v2-final+beta (eu)".to_string(),
            version: "2025.01.01 rc@1".to_string(),
            defaults: json!({
                "description": "a type whose identity is full of symbols",
                "defaultTaskList": { "name": TASK_LIST },
                "defaultTaskStartToCloseTimeout": "NONE",
                "defaultExecutionStartToCloseTimeout": "31536000",
                "defaultTaskPriority": "-1",
                "defaultChildPolicy": "ABANDON",
            }),
        },
        TypeCase {
            case: "non-ASCII identity",
            name: "注文ワークフロー".to_string(),
            version: "版1".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        // Each modelled length bound: the minimum (1) and the maximum on each member.
        TypeCase {
            case: "identity at the modelled minimum",
            name: "a".to_string(),
            version: "1".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        TypeCase {
            case: "name at the modelled maximum",
            name: "n".repeat(MAX_TYPE_NAME_LEN),
            version: "1.0".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        TypeCase {
            case: "version at the modelled maximum",
            name: "bounded-version".to_string(),
            version: "v".repeat(MAX_TYPE_VERSION_LEN),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
    ]
}

#[tokio::test]
async fn deprecation_moves_only_the_addressed_stored_type_to_deprecated() {
    let emulator = Emulator::new();
    let cases = type_cases();

    // Two domains holding the SAME types, so "the record this triple addresses" is
    // shown to be domain-scoped rather than keyed by name and version alone.
    let domains = ["primary-domain", "other-domain"];
    for domain in domains {
        register_domain(&emulator, domain, domain).await;
        for case in &cases {
            register_type(
                &emulator,
                case.case,
                domain,
                &case.name,
                &case.version,
                case.defaults.clone(),
            )
            .await;
        }
    }

    // What each record looked like before anything was deprecated.
    let mut before: Vec<Value> = Vec::new();
    for case in &cases {
        let described = describe_type(&emulator, case.case, domains[0], &case.name, &case.version)
            .await;
        assert_registered(case.case, &described);
        before.push(described);
    }

    let mut stamps: Vec<f64> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let response = emulator
            .ok(
                case.case,
                "DeprecateWorkflowType",
                &json!({
                    "domain": domains[0],
                    "workflowType": type_ref(&case.name, &case.version),
                }),
            )
            .await;
        assert_eq!(
            response,
            json!({}),
            "case {:?}: DeprecateWorkflowType's modelled output shape is Unit, \
             got {response:?}",
            case.case
        );

        let described =
            describe_type(&emulator, case.case, domains[0], &case.name, &case.version).await;
        stamps.push(assert_deprecated(case.case, &described));

        // The identity and the registered configuration survive the status move.
        assert_eq!(
            described.pointer("/typeInfo/workflowType"),
            before[index].pointer("/typeInfo/workflowType"),
            "case {:?}: deprecation must not change the stored identity, got {described:?}",
            case.case
        );
        assert_eq!(
            described.get("configuration"),
            before[index].get("configuration"),
            "case {:?}: deprecation must not change the registered defaults, \
             got {described:?}",
            case.case
        );
        assert_eq!(
            described.pointer("/typeInfo/description"),
            before[index].pointer("/typeInfo/description"),
            "case {:?}: deprecation must not change the stored description, \
             got {described:?}",
            case.case
        );
        assert_eq!(
            described.pointer("/typeInfo/creationDate"),
            before[index].pointer("/typeInfo/creationDate"),
            "case {:?}: deprecation must not restamp the creation date, got {described:?}",
            case.case
        );

        // Every record deprecated earlier keeps the stamp it was given, and every
        // record not yet addressed is still REGISTERED.
        for (other_index, other) in cases.iter().enumerate() {
            let other_described =
                describe_type(&emulator, other.case, domains[0], &other.name, &other.version).await;
            if other_index <= index {
                let stamp = assert_deprecated(other.case, &other_described);
                assert_eq!(
                    stamp, stamps[other_index],
                    "case {:?}: a later deprecation of another type must not restamp this \
                     record, got {other_described:?}",
                    other.case
                );
            } else {
                assert_registered(other.case, &other_described);
            }
        }

        // The same (name, version) in the other domain is a different record.
        let foreign =
            describe_type(&emulator, case.case, domains[1], &case.name, &case.version).await;
        assert_registered(case.case, &foreign);
    }
}

#[tokio::test]
async fn deprecating_an_already_deprecated_type_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let domain = "already-deprecated";
    register_domain(&emulator, domain, domain).await;

    for case in type_cases() {
        register_type(
            &emulator,
            case.case,
            domain,
            &case.name,
            &case.version,
            case.defaults.clone(),
        )
        .await;
        let request = json!({
            "domain": domain,
            "workflowType": type_ref(&case.name, &case.version),
        });
        emulator
            .ok(case.case, "DeprecateWorkflowType", &request)
            .await;
        let first = describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        let stamp = assert_deprecated(case.case, &first);

        // Deprecation is not idempotent: every repeat is the modelled fault, and none
        // of them touches the record.
        for round in 1..=3 {
            let repeated = format!("{} repeat #{round}", case.case);
            emulator
                .refused(
                    &repeated,
                    "DeprecateWorkflowType",
                    &request,
                    "TypeDeprecatedFault",
                )
                .await;
            let after = describe_type(&emulator, &repeated, domain, &case.name, &case.version).await;
            assert_eq!(
                assert_deprecated(&repeated, &after),
                stamp,
                "case {repeated:?}: a refused deprecation must not restamp the record, \
                 got {after:?}"
            );
            assert_eq!(
                after, first,
                "case {repeated:?}: a refused deprecation must not change what a later \
                 caller sees, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_type_in_a_deprecated_domain_still_deprecates_on_its_own() {
    // Deprecating the DOMAIN does not move the status of the types registered in it, so
    // a type there is still REGISTERED and the first `DeprecateWorkflowType` addressing
    // it succeeds; only a second one is the modelled fault. The rule is about the STORED
    // status of the type, which only a call addressing that type moves.
    let emulator = Emulator::new();
    let domain = "deprecated-domain";
    register_domain(&emulator, domain, domain).await;
    for case in type_cases() {
        register_type(
            &emulator,
            case.case,
            domain,
            &case.name,
            &case.version,
            case.defaults.clone(),
        )
        .await;
    }

    emulator
        .ok("deprecate domain", "DeprecateDomain", &json!({ "name": domain }))
        .await;

    for case in type_cases() {
        let after_domain =
            describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_registered(case.case, &after_domain);

        let request = json!({
            "domain": domain,
            "workflowType": type_ref(&case.name, &case.version),
        });
        emulator
            .ok(case.case, "DeprecateWorkflowType", &request)
            .await;
        let deprecated =
            describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        let stamp = assert_deprecated(case.case, &deprecated);

        emulator
            .refused(
                case.case,
                "DeprecateWorkflowType",
                &request,
                "TypeDeprecatedFault",
            )
            .await;
        let after = describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_eq!(
            assert_deprecated(case.case, &after),
            stamp,
            "case {:?}: a refused deprecation must not restamp the record, got {after:?}",
            case.case
        );
    }
}

/// One way a `(domain, workflowType)` triple can fail to address a stored type.
/// `expected_message` is `Some` only where the message is built from the request and is
/// therefore a requirement; otherwise only the status and the error code are.
struct UnknownCase {
    case: &'static str,
    domain: String,
    workflow_type: Value,
    expected_message: Option<String>,
}

#[tokio::test]
async fn a_triple_naming_no_stored_type_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let domain = "unknown-triples";
    let other_domain = "elsewhere";
    register_domain(&emulator, domain, domain).await;
    register_domain(&emulator, other_domain, other_domain).await;
    register_type(
        &emulator,
        "seed",
        domain,
        "orders",
        "1.0",
        full_defaults(TASK_LIST, CHILD_POLICY),
    )
    .await;

    let cases = vec![
        UnknownCase {
            // A WRITE resolves the domain it writes into first, so an absent domain is
            // reported as the domain; only a request inside an existing domain reports
            // the type.
            case: "domain never registered",
            domain: "no-such-domain".to_string(),
            workflow_type: type_ref("orders", "1.0"),
            expected_message: Some("Unknown domain: no-such-domain".to_string()),
        },
        UnknownCase {
            case: "type name never registered",
            domain: domain.to_string(),
            workflow_type: type_ref("no-such-type", "1.0"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=no-such-type, version=1.0]".to_string(),
            ),
        },
        UnknownCase {
            case: "stored name at a version never registered",
            domain: domain.to_string(),
            workflow_type: type_ref("orders", "9.9"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=orders, version=9.9]".to_string(),
            ),
        },
        // The type is stored, but in another domain: the fault is documented for a
        // resource outside the scope of this operation, "region or domain".
        UnknownCase {
            case: "stored type, other domain",
            domain: other_domain.to_string(),
            workflow_type: type_ref("orders", "1.0"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=orders, version=1.0]".to_string(),
            ),
        },
        // Case differs, and SWF names are matched exactly.
        UnknownCase {
            case: "stored name in another case",
            domain: domain.to_string(),
            workflow_type: type_ref("Orders", "1.0"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=Orders, version=1.0]".to_string(),
            ),
        },
        // The identity swapped round addresses nothing.
        UnknownCase {
            case: "name and version transposed",
            domain: domain.to_string(),
            workflow_type: type_ref("1.0", "orders"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=1.0, version=orders]".to_string(),
            ),
        },
    ];

    for case in &cases {
        emulator
            .refused(
                case.case,
                "DeprecateWorkflowType",
                &json!({ "domain": &case.domain, "workflowType": &case.workflow_type }),
                "UnknownResourceFault",
            )
            .await;
        if let Some(expected) = &case.expected_message {
            let (_, body) = emulator
                .call(
                    "DeprecateWorkflowType",
                    &json!({ "domain": &case.domain, "workflowType": &case.workflow_type }),
                )
                .await;
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {:?}: expected a message, got {body:?}", case.case));
            assert_eq!(
                message, expected,
                "case {:?}: the fault message is built from the requested resource, \
                 got {message:?}",
                case.case
            );
        }
    }

    // One past each modelled length bound is refused at the boundary, with the same
    // status and code the boundary validator answers every unmodellable input with.
    for (case, workflow_type) in [
        ("name one past the modelled maximum", type_ref(&"n".repeat(MAX_TYPE_NAME_LEN + 1), "1.0")),
        (
            "version one past the modelled maximum",
            type_ref("orders", &"v".repeat(MAX_TYPE_VERSION_LEN + 1)),
        ),
        ("name below the modelled minimum", type_ref("", "1.0")),
        ("version below the modelled minimum", type_ref("orders", "")),
    ] {
        emulator
            .refused(
                case,
                "DeprecateWorkflowType",
                &json!({ "domain": domain, "workflowType": workflow_type }),
                "ValidationException",
            )
            .await;
    }

    // None of those refusals wrote anything: the one stored type is still REGISTERED.
    let described = describe_type(&emulator, "after refusals", domain, "orders", "1.0").await;
    assert_registered("after refusals", &described);
}

#[tokio::test]
async fn a_deprecated_type_refuses_new_starts_but_leaves_running_executions_alone() {
    let emulator = Emulator::new();
    let domain = "deprecation-effects";
    register_domain(&emulator, domain, domain).await;

    // Two types, so the refusal is shown to follow the deprecated record and not the
    // domain. Rows vary the identity form the same way the first table does.
    let deprecated = ("orders", "1.0");
    let untouched = ("注文ワークフロー", "版1");
    for (name, version) in [deprecated, untouched] {
        register_type(
            &emulator,
            name,
            domain,
            name,
            version,
            full_defaults(TASK_LIST, CHILD_POLICY),
        )
        .await;
    }

    // One run of each type, started while both types are REGISTERED.
    let mut run_ids = Vec::new();
    for (index, (name, version)) in [deprecated, untouched].iter().enumerate() {
        let workflow_id = format!("wf-before-{index}");
        let started = emulator
            .ok(
                name,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": &workflow_id,
                    "workflowType": type_ref(name, version),
                }),
            )
            .await;
        run_ids.push((workflow_id, string_at(&started, "/runId")));
    }

    emulator
        .ok(
            "deprecate",
            "DeprecateWorkflowType",
            &json!({
                "domain": domain,
                "workflowType": type_ref(deprecated.0, deprecated.1),
            }),
        )
        .await;

    // "After a workflow type has been deprecated, you cannot create new executions of
    // that type." Every fresh `workflowId` is refused, so the refusal is about the type
    // and not about a colliding open run.
    for attempt in 0..3 {
        emulator
            .refused(
                "start after deprecation",
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("wf-after-{attempt}"),
                    "workflowType": type_ref(deprecated.0, deprecated.1),
                }),
                "TypeDeprecatedFault",
            )
            .await;
    }

    // The type that was NOT deprecated still starts.
    let started = emulator
        .ok(
            "start untouched type",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-after-untouched",
                "workflowType": type_ref(untouched.0, untouched.1),
            }),
        )
        .await;
    assert!(
        !string_at(&started, "/runId").is_empty(),
        "case \"start untouched type\": a REGISTERED type still mints a runId, got {started:?}"
    );

    // "Executions that were started before the type was deprecated continues to run."
    for (workflow_id, run_id) in &run_ids {
        let described = emulator
            .ok(
                workflow_id,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": workflow_id, "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {workflow_id:?}: a run started before deprecation keeps running, \
             got {described:?}"
        );
    }

    // "A deprecated workflow type may still be used when calling visibility actions."
    let listed = emulator
        .ok(
            "visibility after deprecation",
            "ListOpenWorkflowExecutions",
            &json!({
                "domain": domain,
                "startTimeFilter": { "oldestDate": 0.0 },
                "typeFilter": type_ref(deprecated.0, deprecated.1),
            }),
        )
        .await;
    let reported: Vec<String> = listed
        .pointer("/executionInfos")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case \"visibility after deprecation\": expected executionInfos in {listed:?}")
        })
        .iter()
        .map(|info| string_at(info, "/execution/workflowId"))
        .collect();
    assert_eq!(
        reported,
        vec![run_ids[0].0.clone()],
        "case \"visibility after deprecation\": a visibility action still reports the \
         deprecated type's open run, got {listed:?}"
    );

    // And the type reader — also a visibility action — still answers from the record.
    let described = describe_type(
        &emulator,
        "visibility after deprecation",
        domain,
        deprecated.0,
        deprecated.1,
    )
    .await;
    assert_deprecated("visibility after deprecation", &described);
    assert_eq!(
        described.pointer("/configuration/defaultTaskList/name"),
        Some(&json!(TASK_LIST)),
        "case \"visibility after deprecation\": the deprecated record still reports its \
         registered defaults, got {described:?}"
    );
}
