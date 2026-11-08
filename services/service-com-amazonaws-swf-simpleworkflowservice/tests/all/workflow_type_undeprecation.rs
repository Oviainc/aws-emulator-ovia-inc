// SPDX-License-Identifier: CC0-1.0

//! `UndeprecateWorkflowType`, read back through `DescribeWorkflowType` and exercised
//! against `StartWorkflowExecution` and `DeleteWorkflowType`, end to end over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<OperationName>`), dispatched through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `UndeprecateWorkflowType` acts on the ONE stored
//! workflow-type record its `(domain, name, version)` triple addresses and follows that
//! record's own stored `RegistrationStatus` —
//!
//! * a record on `DEPRECATED` returns to `REGISTERED` and drops the `deprecationDate`
//!   deprecation stamped on it (`WorkflowTypeInfo$deprecationDate`: "If the type is in
//!   deprecated state, then it is set to the date when the type was deprecated"),
//!   leaving its identity, description, registration-time defaults and `creationDate`
//!   exactly as registration stored them, and making the type usable for new executions
//!   again ("After a workflow type has been undeprecated, you can create new executions
//!   of that type");
//! * a record already `REGISTERED` is HTTP 400 `TypeAlreadyExistsFault` ("...or if you
//!   undeprecate a type that is currently registered") with nothing written;
//! * a triple naming no stored record is HTTP 400 `UnknownResourceFault`;
//! * the status of the DOMAIN the record lives in never stands in for the record's own
//!   status, and no call addressing one record touches another.
//!
//! Rows vary what the rule generalises over: the form and length of `name`/`version`
//! (plain, digits, symbols, embedded space, non-ASCII, each modelled bound and one past
//! it), which version of one name is addressed, which of two domains holds a same-named
//! type, which registration-time defaults the record carries, how many deprecate /
//! undeprecate cycles are run, and the way a triple fails to name a stored record. No
//! row is privileged.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, merge, register_domain_body, string_at, Emulator,
};

/// The modelled maximum length of `WorkflowType$name`
/// (`smithy_query(query="shape",shape="com.amazonaws.swf#WorkflowType")`: `name` targets
/// `Name`, len 1..256).
const MAX_TYPE_NAME_LEN: usize = 256;

/// The modelled maximum length of `WorkflowType$version` (same query: `version` targets
/// `Version`, len 1..64).
const MAX_TYPE_VERSION_LEN: usize = 64;

/// The task list and child policy the start-capable rows register, so a start needs no
/// overrides of its own.
const TASK_LIST: &str = "primary";
const CHILD_POLICY: &str = "TERMINATE";

/// Amazon SWF's `TypeNotDeprecatedFault` wording for a delete that addresses a record
/// still on `REGISTERED`: one fixed sentence about the state of the record, carrying no
/// value out of the request. The same sentence `delete_workflow_type` pins.
const NOT_DEPRECATED_MESSAGE: &str =
    "The type is currently registered and cannot be deleted in its current state";

/// One stored workflow type the deprecate/undeprecate round trip is driven over. Rows
/// differ in the form and length of the identity and in the registration-time defaults
/// the record carries, so the round trip is checked over a whole record and not just a
/// status member.
struct TypeCase {
    case: &'static str,
    name: String,
    version: String,
    /// Optional `RegisterWorkflowType` members folded into the registration.
    defaults: Value,
}

fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            case: "plain identity, every default",
            name: "orders".to_string(),
            version: "1.0".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        TypeCase {
            // The same name at another version is another record entirely.
            case: "same name, other version, description too",
            name: "orders".to_string(),
            version: "2.0".to_string(),
            defaults: {
                let mut defaults = full_defaults(TASK_LIST, CHILD_POLICY);
                merge(
                    &mut defaults,
                    json!({
                        "description": "the second version",
                        "defaultTaskPriority": "10",
                        "defaultLambdaRole": "arn:aws:iam::123456789012:role/swf-lambda",
                    }),
                );
                defaults
            },
        },
        TypeCase {
            case: "no registered defaults at all",
            name: "reports".to_string(),
            version: "1".to_string(),
            defaults: json!({}),
        },
        TypeCase {
            case: "digits only",
            name: "7".to_string(),
            version: "0001".to_string(),
            defaults: json!({ "defaultTaskStartToCloseTimeout": "NONE" }),
        },
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
        TypeCase {
            case: "identity at the modelled minimum lengths",
            name: "a".to_string(),
            version: "1".to_string(),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
        TypeCase {
            case: "identity at the modelled maximum lengths",
            name: "n".repeat(MAX_TYPE_NAME_LEN),
            version: "v".repeat(MAX_TYPE_VERSION_LEN),
            defaults: full_defaults(TASK_LIST, CHILD_POLICY),
        },
    ]
}

/// The `workflowType` member of a request addressing one type.
fn type_ref(name: &str, version: &str) -> Value {
    json!({ "name": name, "version": version })
}

/// A request body addressing one type in one domain — the input shape
/// `DeprecateWorkflowType`, `UndeprecateWorkflowType`, `DeleteWorkflowType` and
/// `DescribeWorkflowType` share.
fn type_request(domain: &str, name: &str, version: &str) -> Value {
    json!({ "domain": domain, "workflowType": type_ref(name, version) })
}

/// How Amazon SWF names one workflow type in a fault message, built from the requested
/// identity alone.
fn type_reference(name: &str, version: &str) -> String {
    format!("WorkflowType=[name={name}, version={version}]")
}

/// A valid identity component that no registration stored: a value differing from
/// `value` while staying inside the modelled length bound `max`, so a row sitting at
/// that bound is still refused by the handler rather than by the boundary validator.
fn absent_variant(value: &str, max: usize) -> String {
    if value.chars().count() >= max {
        let mut chars: Vec<char> = value.chars().collect();
        chars[0] = 'z';
        chars.into_iter().collect()
    } else {
        format!("{value}-absent")
    }
}

async fn register_domain(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
}

async fn register_type(emulator: &Emulator, case: &TypeCase, domain: &str) {
    let mut body = json!({ "domain": domain, "name": case.name, "version": case.version });
    merge(&mut body, case.defaults.clone());
    emulator.ok(case.case, "RegisterWorkflowType", &body).await;
}

/// Seed one domain holding every row of the table.
async fn seed(emulator: &Emulator, domain: &str, cases: &[TypeCase]) {
    register_domain(emulator, domain, domain).await;
    for case in cases {
        register_type(emulator, case, domain).await;
    }
}

/// The stored record, read back through the implemented reader.
async fn describe_type(emulator: &Emulator, case: &str, domain: &str, ty: &TypeCase) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowType",
            &type_request(domain, &ty.name, &ty.version),
        )
        .await
}

/// Call one operation addressing `ty`, requiring the modelled empty success.
async fn ok_on_type(emulator: &Emulator, case: &str, operation: &str, domain: &str, ty: &TypeCase) {
    let response = emulator
        .ok(case, operation, &type_request(domain, &ty.name, &ty.version))
        .await;
    assert_eq!(
        response,
        json!({}),
        "case {case:?}: {operation}'s modelled output shape is Unit, got {response:?}"
    );
}

/// Call one operation addressing `ty`, requiring HTTP 400 `expected_code`, and return
/// the fault's message.
async fn refused_on_type(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    ty: &TypeCase,
    expected_code: &str,
) -> String {
    let (status, body) = emulator
        .call(operation, &type_request(domain, &ty.name, &ty.version))
        .await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?}: expected {operation} to answer 400 {expected_code}, \
         got {status} {code} with {body:?}"
    );
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"))
        .to_string()
}

/// The modelled `RegistrationStatus` the record reports, checked against the model's own
/// value set so no case can pass on a status the model does not define.
fn modelled_status(case: &str, described: &Value) -> String {
    let status = string_at(described, "/typeInfo/status");
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled RegistrationStatus values \
         {REGISTRATION_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// Assert `described` reports a REGISTERED record with no deprecation stamp.
fn assert_registered(case: &str, described: &Value) {
    assert_eq!(
        modelled_status(case, described),
        "REGISTERED",
        "case {case:?}: a type that is not deprecated reports REGISTERED, got {described:?}"
    );
    assert_eq!(
        described.pointer("/typeInfo/deprecationDate"),
        None,
        "case {case:?}: deprecationDate is reported only while the type is DEPRECATED, \
         got {described:?}"
    );
}

/// Assert `described` reports a DEPRECATED record and return its `deprecationDate`,
/// which must be at or after the record's own `creationDate`.
fn assert_deprecated(case: &str, described: &Value) -> f64 {
    assert_eq!(
        modelled_status(case, described),
        "DEPRECATED",
        "case {case:?}: a deprecated type reports DEPRECATED, got {described:?}"
    );
    let creation = described
        .pointer("/typeInfo/creationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a creationDate in {described:?}"));
    let deprecation = described
        .pointer("/typeInfo/deprecationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a deprecated record carries its stamp, got {described:?}")
        });
    assert!(
        deprecation >= creation,
        "case {case:?}: deprecationDate {deprecation} must be at or after creationDate \
         {creation}, got {described:?}"
    );
    deprecation
}

#[tokio::test]
async fn undeprecation_returns_one_record_to_registered_exactly_as_registered() {
    let emulator = Emulator::new();
    let cases = type_cases();

    // Two domains holding the SAME types, so "the record this triple addresses" is shown
    // to be domain-scoped rather than keyed by name and version alone.
    let domains = ["primary-domain", "other-domain"];
    for domain in domains {
        seed(&emulator, domain, &cases).await;
    }

    for case in &cases {
        let registered = describe_type(&emulator, case.case, domains[0], case).await;
        assert_registered(case.case, &registered);

        // Two full cycles: the round trip is an invariant, not a one-shot.
        for round in 1..=2 {
            let round_case = format!("{} cycle #{round}", case.case);
            ok_on_type(
                &emulator,
                &round_case,
                "DeprecateWorkflowType",
                domains[0],
                case,
            )
            .await;
            let deprecated = describe_type(&emulator, &round_case, domains[0], case).await;
            let stamp = assert_deprecated(&round_case, &deprecated);

            ok_on_type(
                &emulator,
                &round_case,
                "UndeprecateWorkflowType",
                domains[0],
                case,
            )
            .await;
            let undeprecated = describe_type(&emulator, &round_case, domains[0], case).await;
            assert_registered(&round_case, &undeprecated);
            // Everything else the record holds — identity, description, registered
            // configuration, creationDate — is exactly what registration stored.
            assert_eq!(
                undeprecated, registered,
                "case {round_case:?}: an undeprecated record reads back as it was \
                 registered, got {undeprecated:?}"
            );

            // A fresh deprecation stamps again from the record's current state, so the
            // record really did leave DEPRECATED rather than keeping the old stamp.
            ok_on_type(
                &emulator,
                &round_case,
                "DeprecateWorkflowType",
                domains[0],
                case,
            )
            .await;
            let redeprecated = describe_type(&emulator, &round_case, domains[0], case).await;
            let new_stamp = assert_deprecated(&round_case, &redeprecated);
            assert!(
                new_stamp >= stamp,
                "case {round_case:?}: a deprecation after an undeprecation stamps the \
                 instant of that call, got {new_stamp} before {stamp} in {redeprecated:?}"
            );
            ok_on_type(
                &emulator,
                &round_case,
                "UndeprecateWorkflowType",
                domains[0],
                case,
            )
            .await;
            assert_registered(
                &round_case,
                &describe_type(&emulator, &round_case, domains[0], case).await,
            );
        }

        // Every other row, in this domain and in the other, is untouched by this row's
        // cycles.
        for other in &cases {
            for domain in domains {
                if other.case == case.case && domain == domains[0] {
                    continue;
                }
                let described = describe_type(&emulator, other.case, domain, other).await;
                assert_registered(other.case, &described);
            }
        }
    }
}

#[tokio::test]
async fn undeprecating_a_registered_type_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain = "already-registered";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    for case in &cases {
        let before = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &before);

        for round in 1..=3 {
            let round_case = format!("{} refusal #{round}", case.case);
            let message = refused_on_type(
                &emulator,
                &round_case,
                "UndeprecateWorkflowType",
                domain,
                case,
                "TypeAlreadyExistsFault",
            )
            .await;
            assert_eq!(
                message,
                type_reference(&case.name, &case.version),
                "case {round_case:?}: the fault is built from the requested identity, \
                 got {message:?}"
            );
            let after = describe_type(&emulator, &round_case, domain, case).await;
            assert_eq!(
                after, before,
                "case {round_case:?}: a refused undeprecation must not change what a \
                 later caller sees, got {after:?}"
            );
        }

        // And the same refusal returns once the record is back on REGISTERED.
        ok_on_type(&emulator, case.case, "DeprecateWorkflowType", domain, case).await;
        ok_on_type(
            &emulator,
            case.case,
            "UndeprecateWorkflowType",
            domain,
            case,
        )
        .await;
        let message = refused_on_type(
            &emulator,
            case.case,
            "UndeprecateWorkflowType",
            domain,
            case,
            "TypeAlreadyExistsFault",
        )
        .await;
        assert_eq!(
            message,
            type_reference(&case.name, &case.version),
            "case {:?}: the fault is built from the requested identity, got {message:?}",
            case.case
        );
        let after = describe_type(&emulator, case.case, domain, case).await;
        assert_eq!(
            after, before,
            "case {:?}: a completed cycle plus a refusal leaves the registered record, \
             got {after:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_triple_naming_no_stored_workflow_type_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let home = "home-domain";
    let other = "other-domain";
    let cases = type_cases();
    seed(&emulator, home, &cases).await;
    register_domain(&emulator, other, other).await;

    // Deprecate every row in `home`, so a refusal here cannot be the
    // already-registered fault standing in for a missing record.
    for case in &cases {
        ok_on_type(&emulator, case.case, "DeprecateWorkflowType", home, case).await;
    }

    for case in &cases {
        let absent_name = absent_variant(&case.name, MAX_TYPE_NAME_LEN);
        let absent_version = absent_variant(&case.version, MAX_TYPE_VERSION_LEN);
        let missing = vec![
            (
                // A WRITE resolves the domain it writes into first, so an absent domain
                // is reported as the domain; only a request inside an existing domain
                // reports the type.
                "domain never registered",
                "no-such-domain".to_string(),
                case.name.clone(),
                case.version.clone(),
                "Unknown domain: no-such-domain".to_string(),
            ),
            (
                // The lookup is domain-scoped: the type exists, just not here.
                "registered in another domain",
                other.to_string(),
                case.name.clone(),
                case.version.clone(),
                format!("Unknown type: {}", type_reference(&case.name, &case.version)),
            ),
            (
                "name no registration stored",
                home.to_string(),
                absent_name.clone(),
                case.version.clone(),
                format!(
                    "Unknown type: {}",
                    type_reference(&absent_name, &case.version)
                ),
            ),
            (
                "version no registration stored",
                home.to_string(),
                case.name.clone(),
                absent_version.clone(),
                format!(
                    "Unknown type: {}",
                    type_reference(&case.name, &absent_version)
                ),
            ),
        ];

        for (why, domain, name, version, expected_message) in missing {
            let label = format!("{} / {why}", case.case);
            let (status, body) = emulator
                .call(
                    "UndeprecateWorkflowType",
                    &type_request(&domain, &name, &version),
                )
                .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "UnknownResourceFault"),
                "case {label:?}: expected 400 UnknownResourceFault, got {status} {code} \
                 with {body:?}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {label:?}: expected a message, got {body:?}"));
            assert_eq!(
                message, expected_message,
                "case {label:?}: the fault is built from the requested resource, \
                 got {message:?}"
            );
        }

        // None of those refusals touched the stored record: it is still DEPRECATED.
        assert_deprecated(
            case.case,
            &describe_type(&emulator, case.case, home, case).await,
        );
    }

    // One past each modelled length bound is refused at the boundary, with the same
    // status and code the boundary validator answers every unmodellable input with.
    for (label, workflow_type) in [
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
        let (status, body) = emulator
            .call(
                "UndeprecateWorkflowType",
                &json!({ "domain": home, "workflowType": workflow_type }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected 400 ValidationException, got {status} {code} \
             with {body:?}"
        );
    }
}

#[tokio::test]
async fn an_undeprecated_type_accepts_new_executions_and_refuses_deletion_again() {
    // "After a workflow type has been undeprecated, you can create new executions of
    // that type": the effect is read back through the operations that follow the stored
    // status, not just through the reader.
    let emulator = Emulator::new();
    let domain = "undeprecation-effects";
    // Only the rows carrying every registration-time default can start a run with no
    // overrides; a row with no defaults is covered by the tables above instead.
    let cases: Vec<TypeCase> = type_cases()
        .into_iter()
        .filter(|case| case.defaults.get("defaultTaskList").is_some())
        .collect();
    assert!(
        cases.len() >= 4,
        "the start-capable table must span several identity forms, got {} rows",
        cases.len()
    );
    seed(&emulator, domain, &cases).await;

    for (index, case) in cases.iter().enumerate() {
        ok_on_type(&emulator, case.case, "DeprecateWorkflowType", domain, case).await;

        // While DEPRECATED: no new execution, and a deletion is allowed.
        let (status, body) = emulator
            .call(
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("while-deprecated-{index}"),
                    "workflowType": type_ref(&case.name, &case.version),
                }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "TypeDeprecatedFault"),
            "case {:?}: a deprecated type starts no new execution, got {status} {code} \
             with {body:?}",
            case.case
        );

        ok_on_type(
            &emulator,
            case.case,
            "UndeprecateWorkflowType",
            domain,
            case,
        )
        .await;

        // Back on REGISTERED: the same start now succeeds and mints a runId, and the
        // deletion that a DEPRECATED record allows is refused again.
        let started = emulator
            .ok(
                case.case,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("after-undeprecation-{index}"),
                    "workflowType": type_ref(&case.name, &case.version),
                }),
            )
            .await;
        let run_id = string_at(&started, "/runId");
        assert!(
            !run_id.is_empty(),
            "case {:?}: an undeprecated type mints a runId for a new execution, \
             got {started:?}",
            case.case
        );

        let described = emulator
            .ok(
                case.case,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": {
                        "workflowId": format!("after-undeprecation-{index}"),
                        "runId": run_id,
                    },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {:?}: the execution started after undeprecation is stored and OPEN, \
             got {described:?}",
            case.case
        );
        assert_eq!(
            described.pointer("/executionInfo/workflowType"),
            Some(&type_ref(&case.name, &case.version)),
            "case {:?}: the stored run names the undeprecated type, got {described:?}",
            case.case
        );

        let message = refused_on_type(
            &emulator,
            case.case,
            "DeleteWorkflowType",
            domain,
            case,
            "TypeNotDeprecatedFault",
        )
        .await;
        assert_eq!(
            message, NOT_DEPRECATED_MESSAGE,
            "case {:?}: the fault reports the record's state in the one fixed sentence, \
             got {message:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_deprecated_domain_leaves_its_workflow_types_own_status_alone() {
    // Deprecating a DOMAIN moves only that domain's own status, so a type registered in
    // it keeps its own: an undeprecation addressing a REGISTERED type is still refused,
    // and the type's own cycle still runs inside a deprecated domain.
    let emulator = Emulator::new();
    let domain = "deprecated-domain";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    emulator
        .ok(
            "deprecate domain",
            "DeprecateDomain",
            &json!({ "name": domain }),
        )
        .await;
    let domain_record = emulator
        .ok(
            "deprecate domain",
            "DescribeDomain",
            &json!({ "name": domain }),
        )
        .await;
    assert_eq!(
        string_at(&domain_record, "/domainInfo/status"),
        "DEPRECATED",
        "case \"deprecate domain\": the domain itself reports DEPRECATED, \
         got {domain_record:?}"
    );

    for case in &cases {
        let registered = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &registered);

        refused_on_type(
            &emulator,
            case.case,
            "UndeprecateWorkflowType",
            domain,
            case,
            "TypeAlreadyExistsFault",
        )
        .await;

        ok_on_type(&emulator, case.case, "DeprecateWorkflowType", domain, case).await;
        assert_deprecated(
            case.case,
            &describe_type(&emulator, case.case, domain, case).await,
        );
        ok_on_type(
            &emulator,
            case.case,
            "UndeprecateWorkflowType",
            domain,
            case,
        )
        .await;
        let after = describe_type(&emulator, case.case, domain, case).await;
        assert_eq!(
            after, registered,
            "case {:?}: the cycle inside a deprecated domain restores the registered \
             record, got {after:?}",
            case.case
        );
    }
}
