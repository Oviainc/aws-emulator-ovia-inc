// SPDX-License-Identifier: CC0-1.0

//! `DeleteWorkflowType`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//! The harness is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `DeleteWorkflowType` removes the ONE stored
//! workflow-type record its `(domain, name, version)` triple addresses, and only once
//! that record is `DEPRECATED`, so afterwards every reader of the identity answers
//! HTTP 400 `UnknownResourceFault`, a fresh registration of the same identity stores a
//! new record, and every other stored type and every already-started execution is left
//! exactly as it was; a triple naming no stored record is HTTP 400
//! `UnknownResourceFault`, and a record still on `REGISTERED` is HTTP 400
//! `TypeNotDeprecatedFault` with nothing removed. The tables below vary what that rule
//! generalises over: the form and length of `name`/`version` (plain, symbols, non-ASCII,
//! each modelled bound and one past it), which version of one name is addressed, which
//! of two domains holds a same-named type, which registration-time defaults the record
//! carries, whether the domain holding it is itself deprecated, and the way a triple
//! fails to name a stored record.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, merge, register_domain_body, string_at, whole_history, Emulator,
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

/// Amazon SWF's `TypeNotDeprecatedFault` wording for a delete that addresses a record
/// still on `REGISTERED`: one fixed sentence about the state of the record, carrying no
/// value out of the request.
const NOT_DEPRECATED_MESSAGE: &str =
    "The type is currently registered and cannot be deleted in its current state";

/// The `workflowType` member of a request addressing one type.
fn type_ref(name: &str, version: &str) -> Value {
    json!({ "name": name, "version": version })
}

/// A request addressing one `(domain, name, version)` triple.
fn type_request(domain: &str, name: &str, version: &str) -> Value {
    json!({ "domain": domain, "workflowType": type_ref(name, version) })
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

/// Deprecate `(name, version)` in `domain`: the state `DeleteWorkflowType` requires.
async fn deprecate_type(emulator: &Emulator, case: &str, domain: &str, name: &str, version: &str) {
    emulator
        .ok(
            case,
            "DeprecateWorkflowType",
            &type_request(domain, name, version),
        )
        .await;
}

/// Delete `(name, version)` in `domain`, requiring the modelled empty success.
async fn delete_type(emulator: &Emulator, case: &str, domain: &str, name: &str, version: &str) {
    let response = emulator
        .ok(
            case,
            "DeleteWorkflowType",
            &type_request(domain, name, version),
        )
        .await;
    assert_eq!(
        response,
        json!({}),
        "case {case:?}: DeleteWorkflowType's modelled output shape is Unit, got {response:?}"
    );
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
        .ok(case, "DescribeWorkflowType", &type_request(domain, name, version))
        .await
}

/// Assert the identity names no stored record any more: its reader answers the modelled
/// fault, naming the type it was asked for, rather than a record.
async fn assert_type_absent(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    name: &str,
    version: &str,
) {
    let request = type_request(domain, name, version);
    let (status, body) = emulator.call("DescribeWorkflowType", &request).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "UnknownResourceFault"),
        "case {case:?}: a deleted type is unknown to its reader, got {status} {code} \
         with {body:?}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"));
    assert_eq!(
        message,
        format!("Unknown type: WorkflowType=[name={name}, version={version}]"),
        "case {case:?}: the fault names the requested type, got {message:?}"
    );
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

/// Assert the record `described` reports is DEPRECATED.
fn assert_deprecated(case: &str, described: &Value) {
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
    assert!(
        described.pointer("/typeInfo/deprecationDate").is_some(),
        "case {case:?}: a deprecated record carries its deprecation stamp, got {described:?}"
    );
}

/// One stored workflow type a deletion addresses. Rows differ in the form and length of
/// the identity and in which registration-time defaults the record carries.
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
        // The same NAME at another version is a different record, so deleting one
        // version must leave the other stored.
        TypeCase {
            case: "plain name, second version",
            name: "orders".to_string(),
            version: "2.0".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        // A record carrying NO defaults at all: deletion is about the record's
        // existence, not about its configuration.
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
async fn deletion_removes_only_the_addressed_deprecated_record() {
    let emulator = Emulator::new();
    let cases = type_cases();

    // Two domains holding the SAME deprecated types, so "the record this triple
    // addresses" is shown to be domain-scoped rather than keyed by name and version
    // alone.
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
            deprecate_type(&emulator, case.case, domain, &case.name, &case.version).await;
        }
    }

    // What each record looked like before anything was deleted, per domain: two
    // domains' records of one identity are distinct records, each with its own stamps.
    let mut before: Vec<Vec<Value>> = Vec::new();
    for domain in domains {
        let mut per_domain = Vec::new();
        for case in &cases {
            let described =
                describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
            assert_deprecated(case.case, &described);
            per_domain.push(described);
        }
        before.push(per_domain);
    }

    for (index, case) in cases.iter().enumerate() {
        delete_type(&emulator, case.case, domains[0], &case.name, &case.version).await;

        // The identity now names no stored record in that domain.
        assert_type_absent(&emulator, case.case, domains[0], &case.name, &case.version).await;

        // Every record deleted earlier is still gone, and every record not yet
        // addressed is still stored, byte for byte as it was.
        for (other_index, other) in cases.iter().enumerate() {
            if other_index <= index {
                assert_type_absent(&emulator, other.case, domains[0], &other.name, &other.version)
                    .await;
            } else {
                let still_there =
                    describe_type(&emulator, other.case, domains[0], &other.name, &other.version)
                        .await;
                assert_eq!(
                    still_there, before[0][other_index],
                    "case {:?}: deleting another record must not change this one, \
                     got {still_there:?}",
                    other.case
                );
            }
        }

        // The same (name, version) in the other domain is a different record.
        let foreign =
            describe_type(&emulator, case.case, domains[1], &case.name, &case.version).await;
        assert_eq!(
            foreign, before[1][index],
            "case {:?}: deletion is scoped to the addressed domain, got {foreign:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_deleted_identity_can_be_registered_again_as_a_new_record() {
    // "After a workflow type has been deleted, you cannot create new executions of that
    // type" — the identity is unregistered, so a fresh registration stores a NEW record
    // rather than resurrecting the deleted one.
    let emulator = Emulator::new();
    let domain = "re-registration";
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
        deprecate_type(&emulator, case.case, domain, &case.name, &case.version).await;
        delete_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_type_absent(&emulator, case.case, domain, &case.name, &case.version).await;

        // Registering the identity again carries DIFFERENT defaults, so what the reader
        // reports can only come from the new record.
        let fresh_defaults = json!({
            "description": "the record registered after the deletion",
            "defaultTaskList": { "name": "rebuilt-tasks" },
            "defaultTaskStartToCloseTimeout": "30",
            "defaultExecutionStartToCloseTimeout": "600",
            "defaultChildPolicy": "ABANDON",
        });
        register_type(
            &emulator,
            case.case,
            domain,
            &case.name,
            &case.version,
            fresh_defaults,
        )
        .await;
        let described =
            describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_registered(case.case, &described);
        assert_eq!(
            described.pointer("/configuration/defaultTaskList/name"),
            Some(&json!("rebuilt-tasks")),
            "case {:?}: the re-registered record reports its OWN defaults, got {described:?}",
            case.case
        );
        assert_eq!(
            described.pointer("/typeInfo/description"),
            Some(&json!("the record registered after the deletion")),
            "case {:?}: the re-registered record reports its OWN description, \
             got {described:?}",
            case.case
        );

        // And it is a usable type again: a start resolves against the new record.
        let started = emulator
            .ok(
                case.case,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("wf-{}", case.case),
                    "workflowType": type_ref(&case.name, &case.version),
                }),
            )
            .await;
        assert!(
            started.pointer("/runId").is_some(),
            "case {:?}: a re-registered type starts executions again, got {started:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn deleting_a_registered_type_is_refused_and_removes_nothing() {
    // "Prior to deletion, workflow types must first be deprecated."
    let emulator = Emulator::new();
    let domain = "not-deprecated";
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
        let before = describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_registered(case.case, &before);

        let request = type_request(domain, &case.name, &case.version);
        for round in 1..=3 {
            let repeated = format!("{} attempt #{round}", case.case);
            let (status, body) = emulator.call("DeleteWorkflowType", &request).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "TypeNotDeprecatedFault"),
                "case {repeated:?}: a type still REGISTERED cannot be deleted, \
                 got {status} {code} with {body:?}"
            );
            // Amazon SWF's own wording for this state, naming nothing out of the
            // request, so it is identical for every row of this table.
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(NOT_DEPRECATED_MESSAGE),
                "case {repeated:?}: the fault reports the state of the record, \
                 got {body:?}"
            );
            let after =
                describe_type(&emulator, &repeated, domain, &case.name, &case.version).await;
            assert_eq!(
                after, before,
                "case {repeated:?}: a refused deletion must not change what a later \
                 caller sees, got {after:?}"
            );
        }

        // Once the record IS deprecated, the very same request succeeds: the refusal
        // followed the stored status and nothing else.
        deprecate_type(&emulator, case.case, domain, &case.name, &case.version).await;
        delete_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_type_absent(&emulator, case.case, domain, &case.name, &case.version).await;
    }
}

#[tokio::test]
async fn a_type_in_a_deprecated_domain_still_needs_its_own_deprecation() {
    // Deprecating the DOMAIN leaves the types registered in it REGISTERED, so a delete
    // addressed at one of them is still refused until that type itself is deprecated:
    // the rule follows the type's STORED status, which only a call addressing the type
    // moves.
    let emulator = Emulator::new();
    let domain = "deprecated-domain";
    register_domain(&emulator, domain, domain).await;
    let cases = type_cases();
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

    emulator
        .ok(
            "deprecate domain",
            "DeprecateDomain",
            &json!({ "name": domain }),
        )
        .await;

    for case in &cases {
        let after_domain =
            describe_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_registered(case.case, &after_domain);

        let (status, body) = emulator
            .call(
                "DeleteWorkflowType",
                &type_request(domain, &case.name, &case.version),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "TypeNotDeprecatedFault"),
            "case {:?}: a type in a deprecated domain is still REGISTERED and cannot be \
             deleted, got {status} {code} with {body:?}",
            case.case
        );

        // Its own deprecation is what makes the identical delete succeed.
        deprecate_type(&emulator, case.case, domain, &case.name, &case.version).await;
        delete_type(&emulator, case.case, domain, &case.name, &case.version).await;
        assert_type_absent(&emulator, case.case, domain, &case.name, &case.version).await;
    }
}

/// One way a `(domain, workflowType)` triple can fail to address a stored record.
/// `expected_message` is `Some` only where the message is built from the request and is
/// therefore a requirement; otherwise only the status and the error code are.
struct UnknownCase {
    case: &'static str,
    domain: String,
    workflow_type: Value,
    expected_message: Option<String>,
}

#[tokio::test]
async fn a_triple_naming_no_stored_record_is_an_unknown_resource() {
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
    deprecate_type(&emulator, "seed", domain, "orders", "1.0").await;

    // A type already deleted is one more way a triple names nothing: deletion is not
    // idempotent, so the second call is the unknown-resource fault.
    register_type(
        &emulator,
        "seed deleted",
        domain,
        "gone",
        "1.0",
        full_defaults(TASK_LIST, CHILD_POLICY),
    )
    .await;
    deprecate_type(&emulator, "seed deleted", domain, "gone", "1.0").await;
    delete_type(&emulator, "seed deleted", domain, "gone", "1.0").await;

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
        // The record is stored, but in another domain: the fault is documented for a
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
        UnknownCase {
            case: "record already deleted",
            domain: domain.to_string(),
            workflow_type: type_ref("gone", "1.0"),
            expected_message: Some(
                "Unknown type: WorkflowType=[name=gone, version=1.0]".to_string(),
            ),
        },
    ];

    for case in &cases {
        let request = json!({ "domain": &case.domain, "workflowType": &case.workflow_type });
        emulator
            .refused(case.case, "DeleteWorkflowType", &request, "UnknownResourceFault")
            .await;
        if let Some(expected) = &case.expected_message {
            let (status, body) = emulator.call("DeleteWorkflowType", &request).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "UnknownResourceFault"),
                "case {:?}: an unknown triple stays unknown however often it is asked, \
                 got {status} {code} with {body:?}",
                case.case
            );
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
        (
            "name one past the modelled maximum",
            type_ref(&"n".repeat(MAX_TYPE_NAME_LEN + 1), "1.0"),
        ),
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
                "DeleteWorkflowType",
                &json!({ "domain": domain, "workflowType": workflow_type }),
                "ValidationException",
            )
            .await;
    }

    // None of those refusals removed anything: the one deprecated record is still
    // stored, and still deletable.
    let described = describe_type(&emulator, "after refusals", domain, "orders", "1.0").await;
    assert_deprecated("after refusals", &described);
    delete_type(&emulator, "after refusals", domain, "orders", "1.0").await;
}

#[tokio::test]
async fn an_unknown_type_fault_names_the_requested_type_and_nothing_else() {
    // The message every operation of this service answers a missing registered type
    // with: `Unknown type: <Kind>=[name=<name>, version=<version>]`, built from the
    // request alone. The rows span the operation reporting it (the writer this issue
    // implements, the one beside it, and the two type readers) and the resource kind
    // (`WorkflowType` / `ActivityType`), with identities of several forms.
    let emulator = Emulator::new();
    let domain = "unknown-type-messages";
    register_domain(&emulator, domain, domain).await;

    let identities = [
        ("missing-wf".to_string(), "1.0".to_string()),
        ("注文ワークフロー".to_string(), "版1".to_string()),
        ("w".repeat(MAX_TYPE_NAME_LEN), "v".repeat(MAX_TYPE_VERSION_LEN)),
    ];

    for (name, version) in &identities {
        let (name, version) = (name.as_str(), version.as_str());
        for (operation, kind, request) in [
            (
                "DeleteWorkflowType",
                "WorkflowType",
                type_request(domain, name, version),
            ),
            (
                "DeprecateWorkflowType",
                "WorkflowType",
                type_request(domain, name, version),
            ),
            (
                "DescribeWorkflowType",
                "WorkflowType",
                type_request(domain, name, version),
            ),
            (
                "DescribeActivityType",
                "ActivityType",
                json!({ "domain": domain, "activityType": type_ref(name, version) }),
            ),
        ] {
            let case = format!("{operation} on {name}/{version}");
            let (status, body) = emulator.call(operation, &request).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "UnknownResourceFault"),
                "case {case:?}: a missing type is HTTP 400 UnknownResourceFault, \
                 got {status} {code} with {body:?}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"));
            assert_eq!(
                message,
                format!("Unknown type: {kind}=[name={name}, version={version}]"),
                "case {case:?}: the fault names the requested type and nothing else, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn deletion_refuses_new_starts_but_leaves_running_executions_alone() {
    // "After a workflow type has been deleted, you cannot create new executions of that
    // type. Executions that started before the type was deleted will continue to run."
    let emulator = Emulator::new();
    let domain = "deletion-effects";
    register_domain(&emulator, domain, domain).await;

    // Two types, so the effects are shown to follow the deleted record and not the
    // domain. The rows vary the identity form the same way the first table does.
    let deleted = ("orders", "1.0");
    let untouched = ("注文ワークフロー", "版1");
    for (name, version) in [deleted, untouched] {
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

    // One run of each type, started while both types are registered.
    let mut runs = Vec::new();
    for (index, (name, version)) in [deleted, untouched].iter().enumerate() {
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
        runs.push((workflow_id, string_at(&started, "/runId")));
    }

    // What those runs reported before the deletion.
    let mut before_histories = Vec::new();
    for (workflow_id, run_id) in &runs {
        before_histories.push(whole_history(&emulator, domain, workflow_id, run_id).await);
    }

    deprecate_type(&emulator, "deprecate", domain, deleted.0, deleted.1).await;
    delete_type(&emulator, "delete", domain, deleted.0, deleted.1).await;

    // Every fresh `workflowId` is refused, so the refusal is about the deleted record
    // and not about a colliding open run.
    for attempt in 0..3 {
        emulator
            .refused(
                "start after deletion",
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("wf-after-{attempt}"),
                    "workflowType": type_ref(deleted.0, deleted.1),
                }),
                "UnknownResourceFault",
            )
            .await;
    }

    // The type that was NOT deleted still starts.
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
        "case \"start untouched type\": a stored type still mints a runId, got {started:?}"
    );

    // Both runs started before the deletion continue to run, still reporting the
    // identity and the configuration they resolved at start, and still carrying their
    // whole event log.
    for (index, (workflow_id, run_id)) in runs.iter().enumerate() {
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
            "case {workflow_id:?}: a run started before the deletion keeps running, \
             got {described:?}"
        );
        let expected_type = if index == 0 { deleted } else { untouched };
        assert_eq!(
            described.pointer("/executionInfo/workflowType"),
            Some(&type_ref(expected_type.0, expected_type.1)),
            "case {workflow_id:?}: a run reports the identity it was started with, \
             got {described:?}"
        );
        assert_eq!(
            described.pointer("/executionConfiguration/taskList/name"),
            Some(&json!(TASK_LIST)),
            "case {workflow_id:?}: a run keeps the configuration it resolved at start, \
             got {described:?}"
        );
        let history = whole_history(&emulator, domain, workflow_id, run_id).await;
        assert_eq!(
            history, before_histories[index],
            "case {workflow_id:?}: the deletion must not touch a run's event log, \
             got {history:?}"
        );
    }

    // And the untouched type's own record is exactly as it was.
    let still_there = describe_type(&emulator, "untouched record", domain, untouched.0, untouched.1)
        .await;
    assert_registered("untouched record", &still_there);
}
