// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteStateMachineVersion`, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. the call removes EXACTLY the one version record its qualified ARN names: every
//!    other version of that state machine, and the state machine's own current
//!    configuration, still answer afterwards, and the version it removed no longer
//!    does — whatever the number of versions published and whichever of them is
//!    named;
//! 2. the operation models no resource-absent error, so a well-formed version ARN the
//!    store holds nothing for is a SUCCESS: a repeat of one delete, a number never
//!    published and a version of a state machine that is gone each answer 200;
//! 3. "You can't delete a state machine version currently referenced by one or more
//!    aliases": a version an alias' routing configuration still names is
//!    `ConflictException` (HTTP 409) and nothing is removed, while a version of the
//!    same state machine that no alias names still deletes;
//! 4. the ARN is judged BEFORE the lookup: text that is not a state machine VERSION
//!    ARN of this service in the endpoint's region — malformed, another service's,
//!    another region's, unqualified, Distributed Map labeled, or qualified with an
//!    alias name — is the modelled `InvalidArn`, and nothing is removed;
//! 5. "Deleting a state machine version won't terminate its in-progress executions":
//!    an execution started through the version still reports, attributed to the ARN
//!    it ran, and reaches the outcome its definition computes.
//!
//! The harness (`Emulator`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds, and the published versions come from
//! `state_machine_versions`' own `machine_with_versions`; `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator, ACCOUNT,
    HELLO_DEFINITION, PAST_THE_DELETE_DELAY, PAST_THE_EXECUTION_DELAY, REGION,
};
use crate::state_machine_versions::machine_with_versions;

/// `DeleteStateMachineVersion` on `version_arn`, returning `(status, body)`.
async fn delete_version(emulator: &Emulator, version_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": version_arn }),
        )
        .await
}

/// `DeleteStateMachineVersion` on `version_arn`, which must answer 200 with the
/// modelled empty output.
async fn delete_version_ok(emulator: &Emulator, case: &str, version_arn: &str) {
    let (status, body) = delete_version(emulator, version_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting {version_arn} must answer 200, got {status} with body {body:?}"
    );
    assert!(
        body.as_object().is_none_or(serde_json::Map::is_empty),
        "case {case:?}: the output models no members, got {body:?}"
    );
}

/// The version ARNs `ListStateMachineVersions` reports for `machine_arn`, in the
/// order the response carries them.
async fn listed_versions(emulator: &Emulator, machine_arn: &str) -> Vec<String> {
    let body = emulator
        .ok(
            "ListStateMachineVersions",
            &json!({ "stateMachineArn": machine_arn, "maxResults": 1000 }),
        )
        .await;
    body.get("stateMachineVersions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a stateMachineVersions list in {body:?}"))
        .iter()
        .map(|item| string(item, "stateMachineVersionArn"))
        .collect()
}

/// `(status, errorCode)` of `DescribeStateMachine` on `arn`, which is how a version
/// record is read back: the version's own ARN while it exists, and the modelled
/// `StateMachineDoesNotExist` once it is gone.
async fn describe(emulator: &Emulator, arn: &str) -> (u16, Value) {
    emulator
        .call("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await
}

/// Assert that `version_arn` is readable through `DescribeStateMachine` and reports
/// itself.
async fn assert_describes(emulator: &Emulator, case: &str, version_arn: &str) {
    let (status, body) = describe(emulator, version_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {version_arn} must still describe, got {status} with body {body:?}"
    );
    assert_eq!(
        string(&body, "stateMachineArn"),
        version_arn,
        "case {case:?}: the version must report its own ARN, got {body:?}"
    );
}

/// Assert that `version_arn` names no record any more.
///
/// The refusal names the resource the ARN referred to: a version-qualified ARN the
/// store holds nothing for is the VERSION condition. RECORDED text, spelled out here
/// rather than imported from the handler's constant, so an edit to that constant
/// cannot keep this test green.
async fn assert_absent(emulator: &Emulator, case: &str, version_arn: &str) {
    let (status, body) = describe(emulator, version_arn).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDoesNotExist"),
        "case {case:?}: {version_arn} must be gone, got {status} {code} with body {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(format!("State Machine Version Does Not Exist: '{version_arn}'").as_str()),
        "case {case:?}: the refusal must name the version condition, got {body:?}"
    );
}

/// One entry of an alias' routing configuration.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// Create an alias over `routing` and return its ARN.
async fn create_alias(emulator: &Emulator, name: &str, routing: &[Value]) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({ "name": name, "routingConfiguration": routing }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

// ---------------------------------------------------------------------------
// What a delete removes
// ---------------------------------------------------------------------------

/// A delete removes exactly the version its ARN names: the listing loses that one
/// entry, the version no longer describes, and every other version and the state
/// machine itself are untouched.
///
/// The rows vary along what the rule generalises over: how many versions the state
/// machine holds, and WHICH of them is named — the first, one in the middle and the
/// newest.
#[tokio::test]
async fn a_delete_removes_exactly_the_version_it_names() {
    // (published versions, the version number deleted).
    let cases: Vec<(u32, u32)> = vec![(1, 1), (2, 1), (2, 2), (3, 2), (5, 1), (5, 3), (5, 5)];

    for (versions, target) in cases {
        let emulator = Emulator::new();
        let case = format!("{target} of {versions} published version(s)");
        let machine =
            machine_with_versions(&emulator, &format!("delete-{versions}-{target}"), versions)
                .await;
        let deleted = format!("{machine}:{target}");
        let kept: Vec<String> = (1..=versions)
            .filter(|number| *number != target)
            .map(|number| format!("{machine}:{number}"))
            .collect();

        assert_describes(&emulator, &case, &deleted).await;
        delete_version_ok(&emulator, &case, &deleted).await;

        // Read back through the implemented readers: the listing and the version's
        // own describe.
        assert_absent(&emulator, &case, &deleted).await;
        let listed = listed_versions(&emulator, &machine).await;
        let expected: Vec<String> = kept.iter().rev().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the remaining versions newest first, got {listed:?}"
        );
        for version_arn in &kept {
            assert_describes(&emulator, &case, version_arn).await;
        }

        // The state machine itself keeps its current configuration.
        let (status, body) = describe(&emulator, &machine).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the state machine must be untouched, got {status} with body {body:?}"
        );
        assert_eq!(
            string(&body, "stateMachineArn"),
            machine,
            "case {case:?}: the state machine must report its own ARN, got {body:?}"
        );
    }
}

/// A version number is never handed out twice: publishing after a delete mints the
/// NEXT number, not the number the delete freed.
#[tokio::test]
async fn a_deleted_version_number_is_not_reused() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "counter", 2).await;

    delete_version_ok(&emulator, "the newest version", &format!("{machine}:2")).await;
    // A published version pins the revision it snapshots, so the state machine is
    // updated before each publish to make the next one a new revision.
    for expected in [3_u32, 4] {
        emulator
            .ok(
                "UpdateStateMachine",
                &json!({
                    "stateMachineArn": &machine,
                    "definition": format!(
                        r#"{{"StartAt":"V{expected}","States":{{"V{expected}":{{"Type":"Pass","End":true}}}}}}"#
                    ),
                }),
            )
            .await;
        let published = emulator
            .ok(
                "PublishStateMachineVersion",
                &json!({ "stateMachineArn": &machine }),
            )
            .await;
        assert_eq!(
            string(&published, "stateMachineVersionArn"),
            format!("{machine}:{expected}"),
            "case {expected}: the publish must mint the next number, got {published:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A version the store does not hold
// ---------------------------------------------------------------------------

/// This operation models no resource-absent error, so a well-formed version ARN the
/// store holds nothing for is a success — over a table varying WHY nothing is there.
#[tokio::test]
async fn deleting_a_version_the_store_does_not_hold_is_a_success() {
    let emulator = Emulator::new();
    let present = machine_with_versions(&emulator, "present", 2).await;
    let unpublished = machine_with_versions(&emulator, "unpublished", 0).await;
    let removed = machine_with_versions(&emulator, "removed-later", 1).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": &removed }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let never = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // Delete one version of the state machine that does hold versions, twice: the
    // repeat is the "already deleted" row.
    delete_version_ok(&emulator, "the first delete", &format!("{present}:1")).await;

    for (case, version_arn) in [
        ("a version already deleted", format!("{present}:1")),
        ("a number never published", format!("{present}:7")),
        (
            "the highest number a version takes",
            format!("{present}:{}", u32::MAX),
        ),
        (
            "a state machine created without publish",
            format!("{unpublished}:1"),
        ),
        (
            "a state machine the clock has since removed",
            format!("{removed}:1"),
        ),
        ("a state machine that never existed", format!("{never}:1")),
    ] {
        delete_version_ok(&emulator, case, &version_arn).await;
    }

    // None of those successes removed anything else: the one surviving version of
    // the state machine that holds versions is still there.
    let listed = listed_versions(&emulator, &present).await;
    assert_eq!(
        listed,
        vec![format!("{present}:2")],
        "the untouched version must survive every absent-version delete, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A version an alias still routes to
// ---------------------------------------------------------------------------

/// "You can't delete a state machine version currently referenced by one or more
/// aliases": the delete is `ConflictException` (HTTP 409) and the version stays.
///
/// The rows vary along HOW the alias names the version — the only entry of a routing
/// configuration, one entry of two, at a weight of zero, and named by two aliases at
/// once — and include the version no alias names, which still deletes.
#[tokio::test]
async fn a_version_an_alias_routes_to_cannot_be_deleted() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "aliased", 5).await;
    let version = |number: u32| format!("{machine}:{number}");

    // Version 1: the whole weight of one alias, and an entry of a second.
    create_alias(&emulator, "PROD", &[route(&version(1), 100)]).await;
    create_alias(
        &emulator,
        "rollout",
        &[route(&version(1), 60), route(&version(2), 40)],
    )
    .await;
    // Version 3: named at a weight of zero, which is still a reference.
    create_alias(
        &emulator,
        "staged",
        &[route(&version(4), 100), route(&version(3), 0)],
    )
    .await;

    for (case, number) in [
        ("the whole weight of one alias and an entry of another", 1),
        ("one entry of a two-version routing configuration", 2),
        ("an entry carrying no weight", 3),
        ("the whole weight of one alias", 4),
    ] {
        let version_arn = version(number);
        let (status, body) = delete_version(&emulator, &version_arn).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException for {version_arn}, got {status} {code} \
             with body {body:?}"
        );
        // Nothing was removed.
        assert_describes(&emulator, case, &version_arn).await;
    }

    // The version no alias names deletes, and every referenced version survives.
    delete_version_ok(&emulator, "a version no alias names", &version(5)).await;
    let listed = listed_versions(&emulator, &machine).await;
    assert_eq!(
        listed,
        vec![version(4), version(3), version(2), version(1)],
        "only the unreferenced version may be removed, got {listed:?}"
    );
}

/// An alias belongs to ONE state machine, so an alias of one machine does not block
/// the same version number of another machine.
#[tokio::test]
async fn an_alias_only_blocks_the_versions_of_its_own_state_machine() {
    let emulator = Emulator::new();
    let blocked = machine_with_versions(&emulator, "blocked", 1).await;
    let free = machine_with_versions(&emulator, "free", 1).await;
    create_alias(&emulator, "PROD", &[route(&format!("{blocked}:1"), 100)]).await;

    let (status, body) = delete_version(&emulator, &format!("{blocked}:1")).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "the referenced version must be refused, got {status} {code} with body {body:?}"
    );

    delete_version_ok(&emulator, "another machine's version 1", &format!("{free}:1")).await;
    assert_absent(&emulator, "another machine's version 1", &format!("{free}:1")).await;
    assert_describes(&emulator, "the referenced version", &format!("{blocked}:1")).await;
}

// ---------------------------------------------------------------------------
// What a deleted version's ARN then answers
// ---------------------------------------------------------------------------

/// A resource-absent refusal names the resource the ARN REFERRED to: after the
/// delete, every operation that resolves a state machine ARN reports the version
/// condition for the deleted version's ARN, and the state machine condition for an
/// unqualified ARN that names nothing.
///
/// The rows vary along the operation doing the resolving and along the ARN's form,
/// which is what the rule generalises over.
#[tokio::test]
async fn an_absent_version_is_reported_as_a_version() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "conditions", 1).await;
    let version_arn = format!("{machine}:1");
    delete_version_ok(&emulator, "the only version", &version_arn).await;
    let absent_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, operation, the request body, the message). RECORDED text: the condition
    // in spaced Title Case, then the ARN of the request, quoted.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "DescribeStateMachine on the deleted version",
            "DescribeStateMachine",
            json!({ "stateMachineArn": &version_arn }),
            format!("State Machine Version Does Not Exist: '{version_arn}'"),
        ),
        (
            "StartExecution on the deleted version",
            "StartExecution",
            json!({ "stateMachineArn": &version_arn, "name": "after-the-delete" }),
            format!("State Machine Version Does Not Exist: '{version_arn}'"),
        ),
        (
            "ListExecutions on the deleted version",
            "ListExecutions",
            json!({ "stateMachineArn": &version_arn }),
            format!("State Machine Version Does Not Exist: '{version_arn}'"),
        ),
        (
            "DescribeStateMachine on a number never published",
            "DescribeStateMachine",
            json!({ "stateMachineArn": format!("{machine}:9") }),
            format!("State Machine Version Does Not Exist: '{machine}:9'"),
        ),
        (
            "DescribeStateMachine on an unqualified ARN that names nothing",
            "DescribeStateMachine",
            json!({ "stateMachineArn": &absent_machine }),
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
        (
            "StartExecution on an unqualified ARN that names nothing",
            "StartExecution",
            json!({ "stateMachineArn": &absent_machine, "name": "orphan" }),
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
    ];

    for (case, operation, body, message) in &cases {
        let (status, answered) = emulator.call(operation, body).await;
        let code = error_code(&answered);
        assert_eq!(
            (status, code.as_str()),
            (400, "StateMachineDoesNotExist"),
            "case {case:?}: expected 400 StateMachineDoesNotExist, got {status} {code} with body \
             {answered:?}"
        );
        assert_eq!(
            answered.get("message").and_then(Value::as_str),
            Some(message.as_str()),
            "case {case:?}: expected the message {message:?}, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ARN is judged before the lookup
// ---------------------------------------------------------------------------

/// Text that is not a state machine VERSION ARN of this service in the endpoint's
/// region is the modelled `InvalidArn`, whose message names the first component that
/// failed inside the service's `Invalid Arn: '...'` envelope — and nothing is
/// removed.
#[tokio::test]
async fn an_arn_that_names_no_version_is_invalid_arn() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "arn-rule", 2).await;
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // (case, the ARN sent, the diagnosis the message must carry). The rows vary
    // along WHICH part of the ARN is not a version ARN: the ARN syntax itself, the
    // service, the region, the resource type, and the qualifier — absent, a
    // Distributed Map label, or an alias name rather than a version number.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "text that is not an ARN",
            "not-a-valid-arn".to_string(),
            "Invalid ARN prefix: not-a-valid-arn".to_string(),
        ),
        (
            "an ARN with no resource field",
            prefix.clone(),
            format!("Invalid ARN: expected 6 colon-delimited fields in {prefix}"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:arn-rule:1"),
            "Invalid ARN service: lambda, expected states".to_string(),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-rule:1"),
            format!(
                "Expected the ARN arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-rule:1 to \
                 be within region ({REGION})."
            ),
        ),
        (
            "an execution ARN",
            format!("{prefix}:execution:arn-rule:run-1"),
            "Invalid ARN resource type: execution, expected stateMachine".to_string(),
        ),
        (
            "an activity ARN",
            format!("{prefix}:activity:an-activity"),
            "Invalid ARN resource type: activity, expected stateMachine".to_string(),
        ),
        (
            "a state machine ARN with no name",
            format!("{prefix}:stateMachine::1"),
            format!("Invalid ARN stateMachine name: {prefix}:stateMachine::1"),
        ),
        (
            "an unqualified state machine ARN",
            machine.clone(),
            format!("Invalid ARN stateMachine version: {machine}"),
        ),
        (
            "a Distributed Map labeled ARN",
            format!("{machine}/mapStateLabel"),
            format!("Invalid ARN stateMachine version: {machine}/mapStateLabel"),
        ),
        (
            "a labeled ARN qualified with a version",
            format!("{machine}/mapStateLabel:1"),
            format!("Invalid ARN stateMachine version: {machine}/mapStateLabel:1"),
        ),
        (
            "an alias ARN",
            format!("{machine}:PROD"),
            format!("Invalid ARN stateMachine version: {machine}:PROD"),
        ),
        (
            "a qualifier that only starts with digits",
            format!("{machine}:1-beta"),
            format!("Invalid ARN stateMachine version: {machine}:1-beta"),
        ),
        (
            "a qualifier past the numbers a version takes",
            format!("{machine}:{}", u64::from(u32::MAX) + 1),
            format!(
                "Invalid ARN stateMachine version: {machine}:{}",
                u64::from(u32::MAX) + 1
            ),
        ),
        (
            "a version qualified again",
            format!("{machine}:1:2"),
            format!("Invalid ARN stateMachine version: {machine}:1:2"),
        ),
    ];

    for (case, value, diagnosis) in &cases {
        let (status, body) = delete_version(&emulator, value).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {case:?}: expected 400 InvalidArn, got {status} {code} with body {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(format!("Invalid Arn: '{diagnosis}'").as_str()),
            "case {case:?}: unexpected message in {body:?}"
        );
    }

    // None of those refusals removed a version.
    let listed = listed_versions(&emulator, &machine).await;
    assert_eq!(
        listed,
        vec![format!("{machine}:2"), format!("{machine}:1")],
        "a refused ARN must remove nothing, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Executions outlive the version they ran
// ---------------------------------------------------------------------------

/// "Deleting a state machine version won't terminate its in-progress executions": the
/// execution still reports, still attributed to the version ARN it was started
/// through, and reaches the outcome its definition computes.
#[tokio::test]
async fn deleting_a_version_leaves_its_executions_alone() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "outliving", HELLO_DEFINITION).await;
    let published = emulator
        .ok(
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": &machine }),
        )
        .await;
    let version_arn = string(&published, "stateMachineVersionArn");

    let execution = start_execution(&emulator, &version_arn, "through-the-version", "{}").await;
    delete_version_ok(&emulator, "the version an execution is running", &version_arn).await;
    assert_absent(&emulator, "the version an execution is running", &version_arn).await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        string(&described, "stateMachineVersionArn"),
        version_arn,
        "the execution must stay attributed to the version it ran, got {described:?}"
    );
    assert_eq!(
        string(&described, "status"),
        "SUCCEEDED",
        "the execution must reach the outcome its definition computes, got {described:?}"
    );
}
