// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HTTP STATUS this service answers a refused request
//! with, and for the two refusal sentences whose text is not derived from one
//! value of the request, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`, so what is asserted is what a
//! signed AWS SDK request observes on the status line and in `$.message`.
//!
//! The rules under test:
//!
//! 1. every modelled CLIENT error of this service is answered with ONE status —
//!    HTTP 400 — whatever status that error's shape declares in its `@httpError`
//!    trait: `awsJson1_0` names the error in the body's `__type`, which is where a
//!    caller reads which refusal it was. The table below spans the declared
//!    statuses this model carries for a client error (404, 409, 402 and 400, both
//!    explicit and defaulted), the operations that raise them, and the read and the
//!    write path;
//! 2. a `ResourceNotFound` about an ARN the store holds no resource under states
//!    the condition and then that ARN in quotes, the same sentence whichever
//!    operation resolved the ARN and whichever resource kind it named;
//! 3. the two `ConflictException`s whose message is NOT about one value of the
//!    request state their own rule: a create colliding with an existing alias
//!    states it with no variable part at all, and a version an alias still routes
//!    to lists the aliases that reference it, by NAME, in a bracketed list read
//!    from the store.
//!
//! The harness (`Emulator`, `create_machine`, `string`, `failure`) and the alias
//! and version helpers are the ones the sibling suites already build;
//! `tests/all.rs` is this binary's crate root, so they are reached through
//! `crate::`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::service::wire::{error_status, CLIENT_ERROR_STATUS};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    resource_not_found_message, version_referenced_by_aliases_message, ALIAS_ROUTING_CONFLICT,
    MAX_ALIASES_PER_STATE_MACHINE,
};

use crate::error_messages::{failure, MODEL};
use crate::state_machine_aliases::{create_alias, create_request, route};
use crate::state_machine_lifecycle::{create_machine, Emulator, ACCOUNT, HELLO_DEFINITION, REGION};
use crate::state_machine_versions::machine_with_versions;

/// The status an error shape of this model DECLARES, with the error class it is
/// declared in.
///
/// Read off the model itself (`error_messages::MODEL`, the one copy this test
/// binary embeds): a `smithy.api#httpError` trait when the shape carries one, and
/// otherwise the status Smithy defaults an error of that class to — which is what
/// makes the rule's input the model's own declaration rather than a list written
/// here.
fn declared_error(error_code: &str) -> (String, u16) {
    let model: Value = serde_json::from_str(MODEL).expect("the Smithy model parses as JSON");
    let shape_id = format!("com.amazonaws.sfn#{error_code}");
    let traits = model
        .get("shapes")
        .and_then(|shapes| shapes.get(&shape_id))
        .and_then(|shape| shape.get("traits"))
        .unwrap_or_else(|| panic!("the model declares {shape_id} with traits"));
    let class = traits
        .get("smithy.api#error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected {shape_id} to declare an error class, got {traits:?}"))
        .to_string();
    let declared = traits
        .get("smithy.api#httpError")
        .and_then(Value::as_u64)
        .map_or_else(
            // Smithy defaults a client error to 400 and a server error to 500.
            || if class == "client" { 400 } else { 500 },
            |status| u16::try_from(status).expect("an HTTP status fits in a u16"),
        );
    (class, declared)
}

/// A Map Run ARN inside the modelled `LongArn` length that no record answers for.
///
/// The label and the UUID differ per case, so the rows below are not one ARN
/// written four times.
fn absent_map_run(label: &str, uuid: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:mapRun:{label}/{uuid}:0")
}

// ---------------------------------------------------------------------------
// Rule 1: one status for the whole client-error class
// ---------------------------------------------------------------------------

/// Every modelled client error of this service is answered with HTTP 400,
/// whatever status its shape's `@httpError` declares.
///
/// The rows differ along the DECLARED status (404, 409, 402, 400 explicit and 400
/// defaulted), along the operation and along whether the call reads or writes —
/// which is what the rule generalises over — and the loop re-derives each row's
/// declared status from the model, so a row whose shape the model declares
/// differently is still judged by the rule.
#[tokio::test]
async fn every_modelled_client_error_answers_one_status() {
    let emulator = Emulator::new();

    let machine = machine_with_versions(&emulator, "status-rule", 3).await;
    let version = |number: u32| format!("{machine}:{number}");
    create_alias(&emulator, "PROD", None, &[route(&version(1), 100)]).await;

    // A second state machine, filled to the per-state-machine alias quota, so the
    // one declared 402 of this model is reachable.
    let full = machine_with_versions(&emulator, "status-rule-quota", 1).await;
    let full_routing = vec![route(&format!("{full}:1"), 100)];
    for number in 0..MAX_ALIASES_PER_STATE_MACHINE {
        create_alias(&emulator, &format!("alias-{number}"), None, &full_routing).await;
    }

    let absent_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");
    let absent_activity = format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:never-created");

    // (case, operation, request body, the error code the refusal must carry)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a Map Run ARN no record answers for, on the read path",
            "DescribeMapRun",
            json!({ "mapRunArn": absent_map_run("status-rule", "0e6e0d0e-0000-4000-8000-00000000abcd") }),
            "ResourceNotFound",
        ),
        (
            "a Map Run ARN no record answers for, on the write path",
            "UpdateMapRun",
            json!({
                "mapRunArn": absent_map_run("status-rule", "11111111-2222-4333-8444-555555555555"),
                "maxConcurrency": 1,
            }),
            "ResourceNotFound",
        ),
        (
            "tagging an ARN the store holds no resource under",
            "TagResource",
            json!({
                "resourceArn": &absent_activity,
                "tags": [{ "key": "team", "value": "orders" }],
            }),
            "ResourceNotFound",
        ),
        (
            "an alias that was never created",
            "DescribeStateMachineAlias",
            json!({ "stateMachineAliasArn": format!("{machine}:BETA") }),
            "ResourceNotFound",
        ),
        (
            "a routing configuration naming a version that was never published",
            "CreateStateMachineAlias",
            create_request("beta", None, &[route(&version(9), 100)]),
            "ResourceNotFound",
        ),
        (
            "a create colliding with an alias of a different routing configuration",
            "CreateStateMachineAlias",
            create_request("PROD", None, &[route(&version(2), 100)]),
            "ConflictException",
        ),
        (
            "deleting a version an alias still routes to",
            "DeleteStateMachineVersion",
            json!({ "stateMachineVersionArn": version(1) }),
            "ConflictException",
        ),
        (
            "publishing against a revision the state machine does not hold",
            "PublishStateMachineVersion",
            json!({ "stateMachineArn": &machine, "revisionId": "no-such-revision" }),
            "ConflictException",
        ),
        (
            "one alias past the per-state-machine quota",
            "CreateStateMachineAlias",
            create_request("one-too-many", None, &full_routing),
            "ServiceQuotaExceededException",
        ),
        (
            "text that is not an ARN",
            "DescribeMapRun",
            json!({ "mapRunArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "a routing configuration whose weights do not add up to 100",
            "CreateStateMachineAlias",
            create_request("half", None, &[route(&version(1), 50)]),
            "ValidationException",
        ),
        (
            "a state machine that was never created",
            "DescribeStateMachine",
            json!({ "stateMachineArn": &absent_machine }),
            "StateMachineDoesNotExist",
        ),
    ];

    let mut declared_statuses = BTreeSet::new();
    for (case, operation, body, expected_code) in &cases {
        let (status, code, message) = failure(&emulator, case, operation, body).await;
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} with message \
             {message:?}"
        );
        let (class, declared) = declared_error(&code);
        assert_eq!(
            class, "client",
            "case {case:?}: expected {code} to be a client error, got {class:?}"
        );
        declared_statuses.insert(declared);
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: expected HTTP {CLIENT_ERROR_STATUS} for {code} (the model declares \
             {declared}), got {status} with message {message:?}"
        );
        assert_eq!(
            status,
            error_status(declared),
            "case {case:?}: expected the status this service answers a declared {declared} with, \
             got {status}"
        );
    }

    // The table really did span the declared statuses the rule generalises over,
    // so none of the rows above passed by declaring 400 already.
    for declared in [400, 402, 404, 409] {
        assert!(
            declared_statuses.contains(&declared),
            "expected a row whose error shape declares {declared}, got {declared_statuses:?}"
        );
    }

    // None of the refusals above changed anything: the alias and the version they
    // were refused about still answer.
    let described = emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": format!("{machine}:PROD") }),
        )
        .await;
    assert_eq!(
        described
            .get("stateMachineAliasArn")
            .and_then(Value::as_str),
        Some(format!("{machine}:PROD").as_str()),
        "a refused call must leave the alias where it was, got {described:?}"
    );
    let still_there = emulator
        .ok(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": version(1) }),
        )
        .await;
    assert_eq!(
        still_there
            .get("stateMachineArn")
            .and_then(Value::as_str),
        Some(version(1).as_str()),
        "a refused delete must leave the version where it was, got {still_there:?}"
    );
}

/// A SERVER status is not folded into the client one: the status rule reads the
/// class the model declares, so only a `4xx` becomes this service's one client
/// status.
#[tokio::test]
async fn a_server_status_keeps_its_own() {
    for declared in [400, 402, 404, 409, 429, 499] {
        let answered = error_status(declared);
        assert_eq!(
            answered, CLIENT_ERROR_STATUS,
            "case {declared}: expected a client status to be answered as \
             {CLIENT_ERROR_STATUS}, got {answered}"
        );
    }
    for declared in [500, 501, 503] {
        let answered = error_status(declared);
        assert_eq!(
            answered, declared,
            "case {declared}: expected a server status to be answered as it is, got {answered}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: `Resource not found: '<the ARN the request named>'`
// ---------------------------------------------------------------------------

/// A `ResourceNotFound` about an ARN the store holds no resource under states the
/// condition and then that ARN in quotes.
///
/// The rows differ along the operation that resolved the ARN, along the RESOURCE
/// KIND the ARN names (a Map Run, an activity, a state machine, an alias), along
/// whether the call reports or writes, and along whether the resource's parent
/// exists — which is what the rule generalises over.
#[tokio::test]
async fn a_resource_absent_refusal_quotes_the_arn_the_request_named() {
    // RECORDED text: the wire recordings of this API answer
    // `Resource not found: '<arn>'` for this error class, whichever operation
    // raised it (`states-map-run-inspection-and-lookup-errors`,
    // `states-resource-tagging-workflow`).
    const RECORDED_CONDITION: &str = "Resource not found";

    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "absent-rule", HELLO_DEFINITION).await;
    let absent_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    let map_run = absent_map_run("absent-rule", "22222222-3333-4444-8555-666666666666");
    let other_map_run = absent_map_run("absent-rule/inner", "33333333-4444-4555-8666-777777777777");
    let activity = format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:never-created");
    let alias = format!("{machine}:BETA");
    let foreign_alias = format!("{absent_machine}:PROD");

    // (case, operation, request body, the ARN the message must quote)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "DescribeMapRun on a Map Run ARN that names nothing",
            "DescribeMapRun",
            json!({ "mapRunArn": &map_run }),
            &map_run,
        ),
        (
            "UpdateMapRun on a Map Run ARN that names nothing",
            "UpdateMapRun",
            json!({ "mapRunArn": &map_run, "maxConcurrency": 1 }),
            &map_run,
        ),
        (
            "ListExecutions of a Map Run with a labelled ARN that names nothing",
            "ListExecutions",
            json!({ "mapRunArn": &other_map_run }),
            &other_map_run,
        ),
        (
            "TagResource on an activity that was never created",
            "TagResource",
            json!({ "resourceArn": &activity, "tags": [{ "key": "k", "value": "v" }] }),
            &activity,
        ),
        (
            "UntagResource on a state machine that was never created",
            "UntagResource",
            json!({ "resourceArn": &absent_machine, "tagKeys": ["k"] }),
            &absent_machine,
        ),
        (
            "TagResource on a state machine that was never created",
            "TagResource",
            json!({ "resourceArn": &absent_machine, "tags": [{ "key": "k", "value": "v" }] }),
            &absent_machine,
        ),
        (
            "DescribeStateMachineAlias on an alias of an existing state machine",
            "DescribeStateMachineAlias",
            json!({ "stateMachineAliasArn": &alias }),
            &alias,
        ),
        (
            "UpdateStateMachineAlias on an alias whose state machine is absent too",
            "UpdateStateMachineAlias",
            json!({ "stateMachineAliasArn": &foreign_alias, "description": "x" }),
            &foreign_alias,
        ),
    ];

    for (case, operation, body, arn) in &cases {
        let (status, code, message) = failure(&emulator, case, operation, body).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR_STATUS, "ResourceNotFound"),
            "case {case:?}: expected {CLIENT_ERROR_STATUS} ResourceNotFound, got {status} {code} \
             with message {message:?}"
        );
        // Spelled literally, so a reworded constructor fails here.
        let expected = format!("{RECORDED_CONDITION}: '{arn}'");
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            resource_not_found_message(arn),
            "case {case:?}: expected the refusal to read the service's one constructor, got \
             {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the conflicts whose message is not about one request value
// ---------------------------------------------------------------------------

/// A create naming an alias that already exists under other parameters states the
/// conflict rule, with NO variable part: no ARN, no alias name and no version of
/// the request appears in it.
///
/// The rows differ along WHICH parameter of the idempotency check differs — the
/// description, the version, the weighting, the order of the entries — and along
/// the alias the collision is about.
#[tokio::test]
async fn a_create_colliding_with_an_existing_alias_states_the_conflict_rule() {
    // RECORDED text (`states-state-machine-alias-lifecycle#task#8`).
    const RECORDED: &str = "Failed to create alias because an alias with the same name and a \
                            different routing configuration already exists.";
    assert_eq!(
        ALIAS_ROUTING_CONFLICT, RECORDED,
        "expected the conflict sentence to be {RECORDED:?}, got {ALIAS_ROUTING_CONFLICT:?}"
    );

    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "conflict-rule", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");
    create_alias(&emulator, "PROD", None, &[route(&first, 100)]).await;
    create_alias(
        &emulator,
        "canary",
        Some("a tenth of the traffic"),
        &[route(&first, 90), route(&second, 10)],
    )
    .await;

    // (case, the alias name the create collides with, the description, the routing)
    let cases: Vec<(&str, &str, Option<&str>, Vec<Value>)> = vec![
        (
            "a description where the alias has none",
            "PROD",
            Some("new"),
            vec![route(&first, 100)],
        ),
        ("another version", "PROD", None, vec![route(&second, 100)]),
        (
            "another weighting",
            "PROD",
            None,
            vec![route(&first, 90), route(&second, 10)],
        ),
        (
            "the same entries in the other order",
            "canary",
            Some("a tenth of the traffic"),
            vec![route(&second, 10), route(&first, 90)],
        ),
        (
            "no description where the alias has one",
            "canary",
            None,
            vec![route(&first, 90), route(&second, 10)],
        ),
    ];

    for (case, name, description, routing) in &cases {
        let (status, code, message) = failure(
            &emulator,
            case,
            "CreateStateMachineAlias",
            &create_request(name, *description, routing),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR_STATUS, "ConflictException"),
            "case {case:?}: expected {CLIENT_ERROR_STATUS} ConflictException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, RECORDED,
            "case {case:?}: expected message {RECORDED:?}, got {message:?}"
        );
        for interpolated in [&machine, &first, &second, &(*name).to_string()] {
            assert!(
                !message.contains(interpolated.as_str()),
                "case {case:?}: the sentence carries no value of the request, but {interpolated:?} \
                 is in {message:?}"
            );
        }
    }
}

/// A version one or more aliases still route to is refused with the rule and the
/// list of the aliases that reference it, by NAME, in the order the store holds
/// them.
///
/// The rows differ along the NUMBER of aliases referencing the version (one, two,
/// three), along how each one names it (the whole weight, one entry of two, an
/// entry carrying no weight) and along the version — which is what the list
/// generalises over.
#[tokio::test]
async fn a_version_an_alias_routes_to_lists_those_aliases_by_name() {
    const OPENING: &str = "Version to be deleted must not be referenced by an alias. Current list \
                           of aliases referencing this version: [";

    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "referenced-rule", 3).await;
    let version = |number: u32| format!("{machine}:{number}");

    // Created in this order, which is the order the refusal lists them in.
    create_alias(&emulator, "PROD", None, &[route(&version(1), 100)]).await;
    create_alias(
        &emulator,
        "rollout",
        None,
        &[route(&version(1), 60), route(&version(2), 40)],
    )
    .await;
    create_alias(&emulator, "staged", None, &[route(&version(3), 100)]).await;
    create_alias(
        &emulator,
        "canary",
        None,
        &[route(&version(3), 0), route(&version(2), 100)],
    )
    .await;
    create_alias(&emulator, "third", None, &[route(&version(3), 100)]).await;

    // (case, version number, the alias names the refusal must list)
    let cases: Vec<(&str, u32, Vec<&str>)> = vec![
        ("two aliases, one by its whole weight", 1, vec!["PROD", "rollout"]),
        ("two aliases, one of them by an entry of two", 2, vec!["rollout", "canary"]),
        (
            "three aliases, one of them by an entry carrying no weight",
            3,
            vec!["staged", "canary", "third"],
        ),
    ];

    for (case, number, names) in &cases {
        let version_arn = version(*number);
        let (status, code, message) = failure(
            &emulator,
            case,
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": &version_arn }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR_STATUS, "ConflictException"),
            "case {case:?}: expected {CLIENT_ERROR_STATUS} ConflictException, got {status} {code} \
             with message {message:?}"
        );
        let expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            message,
            format!("{OPENING}{}]", expected.join(", ")),
            "case {case:?}: expected the rule and the aliases {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            version_referenced_by_aliases_message(&expected),
            "case {case:?}: expected the refusal to read the service's one constructor, got \
             {message:?}"
        );
        // The list carries NAMES: no ARN of the request or of an alias is in it.
        assert!(
            !message.contains(&machine),
            "case {case:?}: expected the list to name aliases rather than ARNs, got {message:?}"
        );
    }

    // The version no alias names still deletes, so the refusals above are about the
    // references and nothing else.
    emulator
        .ok(
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": &machine }),
        )
        .await;
    emulator
        .ok(
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": version(4) }),
        )
        .await;
}
