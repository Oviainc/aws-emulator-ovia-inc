// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDS this service's control plane refuses a request
//! in, driven over its own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes in `$.__type` and `$.message`.
//!
//! The rules under test, each asserted over a table that spans the class rather
//! than over the one request that exposed it:
//!
//! 1. a value outside a MODELLED constraint is refused as that constraint
//!    violation — the standard validation envelope, with the error code every
//!    other modelled constraint of this service answers — including the one
//!    constraint the boundary cannot enforce (a state machine alias `name`'s
//!    `@pattern`, whose lookahead is outside the regex dialect the centralized
//!    walk compiles);
//! 2. the clause of that envelope echoes the rejected value, EXCEPT for a member
//!    the model marks `@sensitive`, whose value does not travel back out at all
//!    (WHICH members those are is the shared reading's own decision, pinned
//!    against this model by `src/service/validation.rs`'s unit tests);
//! 3. a refusal about a routing configuration states the rule the configuration
//!    broke and the weights the request submitted;
//! 4. an ARN that an operation needs a version qualifier on and that carries none
//!    is refused for the one thing it is missing;
//! 5. a state machine object's fields are judged against the Amazon States
//!    Language's CLOSED set: the required fields it does not carry are reported
//!    together in one diagnostic at the object, and each field the language does
//!    not declare is reported as unsupported at the object;
//! 6. `TestState` words the defects of its OWN members: a `definition` that is
//!    not JSON in one fixed sentence about the state definition, and an `input`
//!    that is not JSON in the parse diagnostic alone, which withholds the
//!    `@sensitive` document it was reading;
//! 7. `UpdateStateMachine` refuses only a request that names NO member that would
//!    change the state machine, and states the alternatives it accepts;
//! 8. `UpdateStateMachineAlias` words the resource-absent condition for WHICH
//!    resource its ARN resolved nothing for, and names the alias in `resourceName`
//!    either way.
//!
//! The harness (`Emulator`, `create_machine`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds, `failure` and `MODEL` are `error_messages`'
//! own, the published versions come from `state_machine_versions`'
//! `machine_with_versions` and the routing entries from `state_machine_aliases`'
//! `route`; `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    missing_version_qualifier_message, pattern_violation_message, resource_not_found_message,
    routing_weight_sum_message, ALIAS_NAME_PATTERN, RESOURCE_DOES_NOT_EXIST,
    UPDATE_NAMES_NO_MEMBER,
};

use crate::error_messages::{failure, MODEL};
use crate::state_machine_aliases::route;
use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};
use crate::state_machine_versions::machine_with_versions;

/// The modelled maximum of `definition` (`@length` 1..1048576), which the oversized
/// row below is one character past.
const MAX_DEFINITION_CHARS: usize = 1_048_576;

/// The modelled maximum of the `SensitiveData` members (`@length` 0..262144).
const MAX_SENSITIVE_DATA_CHARS: usize = 262_144;

/// The modelled maximum of a description (`@length` 0..256).
const MAX_DESCRIPTION_CHARS: usize = 256;

/// A well-formed state machine definition of `characters` characters, so a row that
/// probes the `@length` of `definition` is refused for its LENGTH and not for its
/// shape.
fn definition_of(characters: usize) -> String {
    const WRAPPER: &str = r#"{"Comment": ""}"#;
    format!(
        r#"{{"Comment": "{}"}}"#,
        "x".repeat(characters.saturating_sub(WRAPPER.len()))
    )
}

/// The validation envelope for one violation whose clause names the rejected value.
fn envelope_naming(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The validation envelope for one violation whose member is `@sensitive`, so the
/// clause names no value at all.
fn envelope_without_value(path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The `Member must have length less than or equal to <max>` requirement.
fn at_most(characters: usize) -> String {
    format!("Member must have length less than or equal to {characters}")
}

/// The `Member must have length greater than or equal to <min>` requirement.
fn at_least(characters: usize) -> String {
    format!("Member must have length greater than or equal to {characters}")
}

/// The requirement a member whose own documentation states its bound as a payload
/// SIZE carries (`StartExecution$input`: "Length constraints apply to the payload
/// size, and are expressed as bytes in UTF-8 encoding").
fn at_most_bytes(bytes: usize) -> String {
    format!("Member must have size less than or equal to {bytes} bytes in UTF-8 encoding")
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the modelled-constraint envelope, and the sensitive member
// ---------------------------------------------------------------------------

/// A rejected value is echoed in the clause, and a `@sensitive` member's is not.
///
/// The rows differ along the member (seven of the model's sensitive members, at a
/// top-level path and at a nested one), along the operation that carries it, along
/// WHICH bound of the `@length` was broken (the maximum and the minimum), and they
/// end with the control rows: members the model does NOT mark sensitive, whose
/// rejected value the clause still names.
#[tokio::test]
async fn a_sensitive_members_rejected_value_is_not_echoed() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "sensitive-rule", HELLO_DEFINITION).await;
    let oversized_definition = definition_of(MAX_DEFINITION_CHARS + 1);
    let oversized_data = "y".repeat(MAX_SENSITIVE_DATA_CHARS + 1);
    let long_description = "d".repeat(MAX_DESCRIPTION_CHARS + 1);
    let long_name = "n".repeat(81);

    // (case, operation, request body, the whole message the refusal must carry)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            // The recorded row: an oversized definition names the constraint and
            // NOT the definition.
            "CreateStateMachine with a definition past its maximum",
            "CreateStateMachine",
            json!({
                "name": "sensitive-oversized",
                "definition": &oversized_definition,
                "roleArn": ROLE_ARN,
            }),
            envelope_without_value("definition", &at_most(MAX_DEFINITION_CHARS)),
        ),
        (
            "UpdateStateMachine with a definition past its maximum",
            "UpdateStateMachine",
            json!({ "stateMachineArn": &machine, "definition": &oversized_definition }),
            envelope_without_value("definition", &at_most(MAX_DEFINITION_CHARS)),
        ),
        (
            "TestState with a definition under its minimum",
            "TestState",
            json!({ "definition": "" }),
            envelope_without_value("definition", &at_least(1)),
        ),
        (
            // `input`'s bound is a payload SIZE, which its own documentation states
            // in UTF-8 bytes, so the requirement is the one about that size.
            "StartExecution with an input past its maximum",
            "StartExecution",
            json!({ "stateMachineArn": &machine, "input": &oversized_data }),
            envelope_without_value("input", &at_most_bytes(MAX_SENSITIVE_DATA_CHARS)),
        ),
        (
            "TestState with variables past their maximum",
            "TestState",
            json!({ "definition": HELLO_DEFINITION, "variables": &oversized_data }),
            envelope_without_value("variables", &at_most(MAX_SENSITIVE_DATA_CHARS)),
        ),
        (
            "TestState with a stateName under its minimum",
            "TestState",
            json!({ "definition": HELLO_DEFINITION, "stateName": "" }),
            envelope_without_value("stateName", &at_least(1)),
        ),
        (
            // A member one structure deep: the path names the structure and the
            // member, and the value is still withheld.
            "TestState with a mocked result past its maximum",
            "TestState",
            json!({
                "definition": HELLO_DEFINITION,
                "mock": { "result": &oversized_data },
            }),
            envelope_without_value("mock.result", &at_most(MAX_SENSITIVE_DATA_CHARS)),
        ),
        (
            "CreateStateMachineAlias with a description past its maximum",
            "CreateStateMachineAlias",
            json!({
                "name": "PROD",
                "description": &long_description,
                "routingConfiguration": [route(&format!("{machine}:1"), 100)],
            }),
            envelope_without_value("description", &at_most(MAX_DESCRIPTION_CHARS)),
        ),
        (
            "UpdateStateMachine with a versionDescription past its maximum",
            "UpdateStateMachine",
            json!({
                "stateMachineArn": &machine,
                "definition": HELLO_DEFINITION,
                "publish": true,
                "versionDescription": &long_description,
            }),
            envelope_without_value("versionDescription", &at_most(MAX_DESCRIPTION_CHARS)),
        ),
        // The controls: members the model does not mark sensitive.
        (
            "CreateStateMachine with a name past its maximum",
            "CreateStateMachine",
            json!({ "name": &long_name, "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN }),
            envelope_naming(&long_name, "name", &at_most(80)),
        ),
        (
            "CreateActivity with a name past its maximum",
            "CreateActivity",
            json!({ "name": &long_name }),
            envelope_naming(&long_name, "name", &at_most(80)),
        ),
    ];

    for (case, operation, body, expected) in &cases {
        let (status, code, message) = failure(&emulator, case, operation, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            &message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // No sensitive value reached any of those messages, however long it was: the
    // rule is that the value does not travel back out, not that one clause was
    // reworded.
    for (case, operation, body, _) in &cases {
        let (_, _, message) = failure(&emulator, case, operation, body).await;
        for (value, what) in [
            (oversized_definition.as_str(), "the definition"),
            (oversized_data.as_str(), "the data"),
            (long_description.as_str(), "the description"),
        ] {
            assert!(
                !message.contains(value),
                "case {case:?}: expected {what} to be withheld, got a message of {} characters",
                message.len()
            );
        }
    }
}

/// A state machine alias `name` outside the modelled `@pattern` is refused as that
/// constraint violation, and the pattern the refusal quotes is the MODEL's own.
///
/// The rows differ along why the name misses the pattern — all digits, each class of
/// character the set does not admit, a non-ASCII letter — and end with the names the
/// pattern admits, which must be created rather than refused.
#[tokio::test]
async fn an_alias_name_outside_the_modelled_pattern_is_a_constraint_violation() {
    // The pattern is the model's, read off the model here so the constant and the
    // model cannot drift apart.
    let model: Value = serde_json::from_str(MODEL).expect("the Smithy model parses as JSON");
    let declared = model
        .get("shapes")
        .and_then(|shapes| shapes.get("com.amazonaws.sfn#CharacterRestrictedName"))
        .and_then(|shape| shape.get("traits"))
        .and_then(|traits| traits.get("smithy.api#pattern"))
        .and_then(Value::as_str)
        .expect("the model declares a pattern on CharacterRestrictedName");
    assert_eq!(
        ALIAS_NAME_PATTERN, declared,
        "expected the quoted pattern to be the model's {declared:?}, got {ALIAS_NAME_PATTERN:?}"
    );

    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "pattern-rule", 1).await;
    let routing = vec![route(&format!("{machine}:1"), 100)];

    for name in ["9", "1234567890", "has space", "PROD!", "a/b", "a:1", "caf\u{e9}", "~"] {
        let (status, code, message) = failure(
            &emulator,
            name,
            "CreateStateMachineAlias",
            &json!({ "name": name, "routingConfiguration": &routing }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}"
        );
        let expected = pattern_violation_message("name", ALIAS_NAME_PATTERN, name);
        assert_eq!(
            message, expected,
            "case {name:?}: expected the message {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            envelope_naming(
                name,
                "name",
                &format!("Member must satisfy regular expression pattern: {declared}")
            ),
            "case {name:?}: expected the standard constraint envelope, got {message:?}"
        );
    }

    // The control: a name the pattern admits is created, so the rule is about the
    // pattern and not about refusing names.
    for name in ["PROD", "a", "_", "-", ".", "v1-beta", "release.2024"] {
        let created = emulator
            .ok(
                "CreateStateMachineAlias",
                &json!({ "name": name, "routingConfiguration": &routing }),
            )
            .await;
        assert_eq!(
            string(&created, "stateMachineAliasArn"),
            format!("{machine}:{name}"),
            "case {name:?}: expected the alias to be created under its own ARN"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the routing configuration's weights
// ---------------------------------------------------------------------------

/// A routing configuration whose weights do not add up states the rule and lists the
/// weights the request submitted.
///
/// The rows differ along the number of entries, along how far the sum misses 100
/// (under, over, nothing at all) and along the operation that read the
/// configuration, and end with the configurations that DO add up, which are applied.
#[tokio::test]
async fn the_routing_weight_refusal_states_the_rule_and_the_submitted_weights() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "weight-rule", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");
    let alias = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": "PROD",
                "routingConfiguration": [route(&first, 100)],
            }),
        )
        .await;
    let alias = string(&alias, "stateMachineAliasArn");

    // (case, the weights of the entries the request names)
    let cases: Vec<(&str, Vec<i64>)> = vec![
        ("one entry short of the total", vec![42]),
        ("another single entry short of it", vec![40]),
        ("one entry at nothing", vec![0]),
        ("two entries short of the total", vec![60, 30]),
        ("two entries past it", vec![100, 1]),
        ("two entries at nothing", vec![0, 0]),
    ];

    for (case, weights) in &cases {
        let routing: Vec<Value> = weights
            .iter()
            .zip([&first, &second])
            .map(|(weight, version)| route(version, *weight))
            .collect();
        let submitted: Vec<i32> = weights.iter().map(|weight| *weight as i32).collect();
        let expected = routing_weight_sum_message(&submitted);
        let listed = weights
            .iter()
            .map(i64::to_string)
            .collect::<Vec<String>>()
            .join(", ");
        assert_eq!(
            expected,
            format!("Sum of routing configuration weights must equal 100. Received: [{listed}]"),
            "case {case:?}: unexpected construction of the refusal"
        );

        for (operation, body) in [
            (
                "CreateStateMachineAlias",
                json!({ "name": "CANARY", "routingConfiguration": &routing }),
            ),
            (
                "UpdateStateMachineAlias",
                json!({ "stateMachineAliasArn": &alias, "routingConfiguration": &routing }),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {code} with \
                 message {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }

    // The controls: configurations whose weights add up are applied.
    for (case, routing) in [
        ("one entry at the total", vec![route(&first, 100)]),
        (
            "two entries that add up to it",
            vec![route(&first, 90), route(&second, 10)],
        ),
        (
            "two entries, one of them at nothing",
            vec![route(&first, 100), route(&second, 0)],
        ),
    ] {
        emulator
            .ok(
                "UpdateStateMachineAlias",
                &json!({ "stateMachineAliasArn": &alias, "routingConfiguration": routing }),
            )
            .await;
        let described = emulator
            .ok(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": &alias }),
            )
            .await;
        assert_eq!(
            string(&described, "stateMachineAliasArn"),
            alias,
            "case {case:?}: expected the alias to answer for itself, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an ARN that names no version
// ---------------------------------------------------------------------------

/// A well-formed state machine ARN carrying NO qualifier, where a version ARN is
/// wanted, is refused for the one thing it is missing — and an ARN that carries a
/// qualifier which is not a version keeps the service's ARN envelope, because what
/// is wrong with it is not a missing qualifier.
#[tokio::test]
async fn an_arn_with_no_version_qualifier_names_the_missing_qualifier() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "qualifier-rule", 1).await;
    let other = machine_with_versions(&emulator, "qualifier-rule-other", 1).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // Rows differ along the state machine the ARN names: one with versions, another
    // one, and one the store holds nothing for — the ARN is judged before anything
    // is looked up, so all three are refused the same way — and along whether the
    // ARN carries a Distributed Map label, which is no version qualifier either.
    let labeled = format!("{machine}/mapStateLabel");
    for arn in [&machine, &other, &absent, &labeled] {
        let (status, code, message) = failure(
            &emulator,
            arn,
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": arn }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {arn:?}: expected 400 InvalidArn, got {status} {code} with message {message:?}"
        );
        let expected = missing_version_qualifier_message(arn);
        assert_eq!(
            message, expected,
            "case {arn:?}: expected the message {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            format!("Arn missing version qualifier: {arn}"),
            "case {arn:?}: unexpected construction of the refusal"
        );
    }

    // The controls: a qualifier that is present and is not a version number is a
    // defect in the ARN itself, stated inside the envelope every other ARN
    // diagnosis of this service carries.
    for qualified in [
        format!("{machine}:PROD"),
        format!("{machine}:1-beta"),
        format!("{machine}/mapStateLabel:1"),
    ] {
        let (status, code, message) = failure(
            &emulator,
            &qualified,
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": &qualified }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {qualified:?}: expected 400 InvalidArn, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            message,
            format!("Invalid Arn: 'Invalid ARN stateMachine version: {qualified}'"),
            "case {qualified:?}: expected the ARN envelope, got {message:?}"
        );
    }

    // And the control that this is about the QUALIFIER and not about the operation:
    // the version ARN the publish minted is deleted.
    emulator
        .ok(
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": format!("{machine}:1") }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// Rule 5: the state machine object's fields
// ---------------------------------------------------------------------------

/// The diagnostics a definition's `InvalidDefinition` quotes, in order.
async fn definition_subjects(emulator: &Emulator, case: &str, definition: &str) -> String {
    let (status, code, message) = failure(
        emulator,
        case,
        "CreateStateMachine",
        &json!({
            "name": format!("fields-{}", case.len()),
            "definition": definition,
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidDefinition"),
        "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
         {message:?}"
    );
    message
        .strip_prefix("Invalid State Machine Definition: '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the definition envelope, got {message:?}")
        })
        .to_string()
}

/// A state machine object owes the language's required fields and may carry no field
/// the language does not declare; both are reported AT THE OBJECT.
///
/// The rows differ along which required field is missing (both, each on its own),
/// along whether an undeclared field is there too, along how many undeclared fields
/// there are, along WHERE the object sits (the definition itself, a `Parallel`
/// branch, a `Map`'s `ItemProcessor`) — which is also what decides whether
/// `ProcessorConfig` is a field the language declares there. The last rows are the
/// objects that carry every declared field and nothing else, which must be created.
#[tokio::test]
async fn a_state_machine_object_reports_its_required_and_unsupported_fields() {
    let emulator = Emulator::new();

    // (case, the definition, the subjects the refusal must quote)
    let cases: Vec<(&str, String, String)> = vec![
        (
            // The recorded row.
            "an object carrying one field the language does not declare",
            json!({ "invalid": true }).to_string(),
            "SCHEMA_VALIDATION_FAILED: These fields are required: [States, StartAt] at /, \
             SCHEMA_VALIDATION_FAILED: Field 'invalid' is not supported at /"
                .to_string(),
        ),
        (
            "the empty object",
            "{}".to_string(),
            "SCHEMA_VALIDATION_FAILED: These fields are required: [States, StartAt] at /"
                .to_string(),
        ),
        (
            "an object carrying States alone",
            json!({ "States": { "A": { "Type": "Succeed" } } }).to_string(),
            "SCHEMA_VALIDATION_FAILED: These fields are required: [StartAt] at /".to_string(),
        ),
        (
            "an object carrying StartAt alone",
            json!({ "StartAt": "A" }).to_string(),
            "SCHEMA_VALIDATION_FAILED: These fields are required: [States] at /".to_string(),
        ),
        (
            "a complete object carrying two fields the language does not declare",
            json!({
                "StartAt": "A",
                "States": { "A": { "Type": "Succeed" } },
                "Retry": [],
                "zzz": 1,
            })
            .to_string(),
            "SCHEMA_VALIDATION_FAILED: Field 'Retry' is not supported at /, \
             SCHEMA_VALIDATION_FAILED: Field 'zzz' is not supported at /"
                .to_string(),
        ),
        (
            // The admissible set is keyed on WHERE the object sits: a `Map`
            // state's processor is the one position the language declares
            // `ProcessorConfig` at, so a definition carrying it carries a field
            // the language does not declare there.
            "a definition carrying a field only a Map's processor declares",
            json!({
                "StartAt": "A",
                "States": { "A": { "Type": "Succeed" } },
                "ProcessorConfig": { "Mode": "INLINE" },
            })
            .to_string(),
            "SCHEMA_VALIDATION_FAILED: Field 'ProcessorConfig' is not supported at /".to_string(),
        ),
        (
            "a Parallel branch that carries neither required field",
            json!({
                "StartAt": "P",
                "States": { "P": { "Type": "Parallel", "Branches": [{}], "End": true } },
            })
            .to_string(),
            "SCHEMA_VALIDATION_FAILED: These fields are required: [States, StartAt] at \
             /States/P/Branches[0]"
                .to_string(),
        ),
        (
            "a Map ItemProcessor carrying a field the language does not declare",
            json!({
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": { "I": { "Type": "Pass", "End": true } },
                            "Nope": true,
                        },
                        "End": true,
                    },
                },
            })
            .to_string(),
            "SCHEMA_VALIDATION_FAILED: Field 'Nope' is not supported at \
             /States/M/ItemProcessor"
                .to_string(),
        ),
    ];

    for (case, definition, expected) in &cases {
        let subjects = definition_subjects(&emulator, case, definition).await;
        assert_eq!(
            &subjects, expected,
            "case {case:?}: expected the subjects {expected:?}, got {subjects:?}"
        );

        // What a create refuses the definition for is what the validation reports
        // for it, so the two cannot disagree.
        let reported = emulator
            .ok(
                "ValidateStateMachineDefinition",
                &json!({ "definition": definition }),
            )
            .await;
        assert_eq!(
            reported.get("result").and_then(Value::as_str),
            Some("FAIL"),
            "case {case:?}: expected the validation to fail the definition, got {reported:?}"
        );
    }

    // The controls: every field the language declares, at the definition and inside
    // a nested machine, is carried without complaint.
    let complete = json!({
        "Comment": "every declared field",
        "QueryLanguage": "JSONPath",
        "TimeoutSeconds": 60,
        "Version": "1.0",
        "StartAt": "M",
        "States": {
            "M": {
                "Type": "Map",
                "ItemProcessor": {
                    "Comment": "nested",
                    "ProcessorConfig": { "Mode": "INLINE" },
                    "StartAt": "I",
                    "States": { "I": { "Type": "Pass", "End": true } },
                },
                "End": true,
            },
        },
    })
    .to_string();
    let reported = emulator
        .ok(
            "ValidateStateMachineDefinition",
            &json!({ "definition": &complete }),
        )
        .await;
    assert_eq!(
        reported.get("result").and_then(Value::as_str),
        Some("OK"),
        "case the complete definition: expected it to verify, got {reported:?}"
    );
    create_machine(&emulator, "fields-complete", &complete).await;
}

// ---------------------------------------------------------------------------
// Rule 6: what `TestState` says about its own members
// ---------------------------------------------------------------------------

/// A `definition` this operation cannot read as JSON is refused in ONE fixed
/// sentence about the state definition, carrying nothing of the document.
///
/// The rows differ along the KIND of malformation and along whether the request
/// names a `stateName` — a state machine definition and a single state alike are
/// refused for not being JSON before either shape is judged — and end with a
/// document that IS JSON, refused (or served) for something else.
#[tokio::test]
async fn test_state_refuses_a_definition_that_is_not_json_in_one_sentence() {
    const NOT_JSON: &str = "The state definition must be valid JSON";
    let emulator = Emulator::new();

    for document in [
        r#"{"Type":"Pass","End":true"#,
        "{ this is not json",
        "not-json",
        "[",
        r#"{"Type":"Pass",}"#,
        "\u{feff}{}",
    ] {
        for state_name in [None, Some("A")] {
            let case = format!("{document:?} with stateName {state_name:?}");
            let mut body = json!({ "definition": document });
            if let Some(state_name) = state_name {
                body["stateName"] = json!(state_name);
            }
            let (status, code, message) = failure(&emulator, &case, "TestState", &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
                 {message:?}"
            );
            assert_eq!(
                message, NOT_JSON,
                "case {case:?}: expected the message {NOT_JSON:?}, got {message:?}"
            );
        }
    }

    // The control: a definition that IS JSON is read, so the sentence is about a
    // document that could not be parsed and not about refusing definitions.
    let tested = emulator
        .ok(
            "TestState",
            &json!({ "definition": r#"{"Type":"Pass","Result":{"ok":true},"End":true}"# }),
        )
        .await;
    assert_eq!(
        tested.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case a definition that is JSON: expected the state to be tested, got {tested:?}"
    );
}

/// `TestState`'s refusal about the input a state is tested on names where the parse
/// stopped and, for a document that ran out of input inside a container, the START
/// MARKER of that container — and withholds the document itself, whose member the
/// model marks `@sensitive`, from every clause it states.
///
/// RECORDED text: the service answers `Unexpected end-of-input: expected close marker
/// for Object (start marker at [Source: REDACTED (…); line: 1, column: 1])` for an
/// input that ends inside an object, and the unquoted-word reading alone, with no
/// clause after it, for an input that opens on a bare word.
///
/// The rows differ along the fault class — a document that ends inside each kind of
/// container, a container opened on a later line, a character where something else
/// was expected, an unquoted word, the empty document — and end with documents long
/// enough that quoting one whole would amplify the request, which no clause may do.
#[tokio::test]
async fn test_state_names_where_the_input_stopped_and_withholds_the_input() {
    let emulator = Emulator::new();
    let pass_state = r#"{"Type":"Pass","End":true}"#;

    /// The source a clause names where the document is withheld. Spelled out here
    /// rather than read off the service: this is the recorded text, so the suite
    /// fails if it is reworded.
    const REDACTED: &str = "REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled)";

    /// The clause for a container left open at `line`:`column`.
    fn left_open(container: &str, line: usize, column: usize) -> String {
        format!(
            "Unexpected end-of-input: expected close marker for {container} (start marker at \
             [Source: {REDACTED}; line: {line}, column: {column}])"
        )
    }

    // (case, the input, the whole diagnostic the refusal quotes)
    let cases: Vec<(&str, String, String)> = vec![
        (
            // The recorded row: an object left open names the mark that opened it.
            "an object that ends early",
            r#"{"probe": 1"#.to_string(),
            left_open("Object", 1, 1),
        ),
        (
            "an array that ends early",
            "[1,2".to_string(),
            left_open("Array", 1, 1),
        ),
        (
            "an inner object that ends early",
            r#"{"a": {"b": 1"#.to_string(),
            left_open("Object", 1, 7),
        ),
        (
            "an object opened on the second line",
            "{\n  \"a\": [1".to_string(),
            left_open("Array", 2, 8),
        ),
        (
            "a character where a colon was expected",
            r#"{"a" 1}"#.to_string(),
            "Unexpected character ('1' (code 49)): was expecting a colon to separate field name \
             and value"
                .to_string(),
        ),
        (
            // The recorded row: a word is named WHOLE, and no clause follows it.
            "an unquoted word where a value belongs",
            "not-json".to_string(),
            "Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or \
             token 'null', 'true' or 'false')"
                .to_string(),
        ),
        (
            "the empty document",
            String::new(),
            "Unexpected end-of-input: expected a value".to_string(),
        ),
    ];

    for (case, document, diagnostic) in &cases {
        let (status, code, message) = failure(
            &emulator,
            case,
            "TestState",
            &json!({ "definition": pass_state, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        let expected = format!("Invalid State Machine Execution Input: '{diagnostic}'");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The input is `@sensitive`: no clause quotes the document the parse was reading,
    // so however long the input is the refusal cannot be made to grow with it or to
    // leak it into a log. The one caller value a reading does name — the unquoted
    // word the parse stopped on — is clipped, which is what keeps it bounded.
    for characters in [600_usize, 60_000] {
        for (shape, document) in [
            ("a bare word", "a".repeat(characters)),
            (
                "an object left open",
                format!("{{\"a\": \"{}\"", "a".repeat(characters)),
            ),
        ] {
            let case = format!("{shape} of {characters} characters");
            let (_, code, message) = failure(
                &emulator,
                &case,
                "TestState",
                &json!({ "definition": pass_state, "input": &document }),
            )
            .await;
            assert_eq!(
                code, "InvalidExecutionInput",
                "case {case:?}: expected InvalidExecutionInput, got {code} with {message:?}"
            );
            assert!(
                !message.contains("[Source: (String)"),
                "case {case:?}: expected no clause to quote the input it read, got {message:?}"
            );
            assert!(
                message.len() < 300,
                "case {case:?}: expected the refusal to stay bounded, got {} characters",
                message.len()
            );
        }
    }

    // The control: an input that IS JSON is read.
    let tested = emulator
        .ok(
            "TestState",
            &json!({ "definition": pass_state, "input": r#"{"probe": 1}"# }),
        )
        .await;
    assert_eq!(
        tested.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case an input that is JSON: expected the state to be tested, got {tested:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 7: what an update must name
// ---------------------------------------------------------------------------

/// An update naming ANY member that would change the state machine is served, and
/// one naming none of them is the modelled `MissingRequiredParameter` stating the
/// alternatives.
///
/// The rows differ along WHICH member the update names — each of the five on its own
/// — and the refused rows along what a request that changes nothing carries beside
/// the ARN.
#[tokio::test]
async fn an_update_naming_one_member_it_can_change_is_served() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "update-members", HELLO_DEFINITION).await;

    for (case, member) in [
        ("the definition", json!({ "definition": HELLO_DEFINITION })),
        ("the role ARN", json!({ "roleArn": ROLE_ARN })),
        (
            "the logging configuration",
            json!({ "loggingConfiguration": { "level": "OFF", "includeExecutionData": false } }),
        ),
        (
            "the tracing configuration",
            json!({ "tracingConfiguration": { "enabled": true } }),
        ),
        (
            "the encryption configuration",
            json!({ "encryptionConfiguration": { "type": "AWS_OWNED_KEY" } }),
        ),
    ] {
        let mut body = json!({ "stateMachineArn": &machine });
        for (name, value) in member.as_object().expect("a member object") {
            body[name] = value.clone();
        }
        let updated = emulator.ok("UpdateStateMachine", &body).await;
        assert!(
            updated.get("revisionId").and_then(Value::as_str).is_some(),
            "case {case:?}: an applied update mints a revision, got {updated:?}"
        );
        // The update is readable back: the record answers for what it wrote.
        let described = emulator
            .ok(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": &machine }),
            )
            .await;
        assert_eq!(
            described.get("revisionId"),
            updated.get("revisionId"),
            "case {case:?}: expected the record to answer for the revision the update minted, \
             got {described:?}"
        );
    }

    // A request that names no member it could change asks for nothing, whatever
    // else it carries.
    for (case, body) in [
        ("the ARN alone", json!({ "stateMachineArn": &machine })),
        (
            "the ARN and publish",
            json!({ "stateMachineArn": &machine, "publish": true }),
        ),
    ] {
        let (status, code, message) = failure(&emulator, case, "UpdateStateMachine", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "MissingRequiredParameter"),
            "case {case:?}: expected 400 MissingRequiredParameter, got {status} {code} with \
             message {message:?}"
        );
        assert_eq!(
            message, UPDATE_NAMES_NO_MEMBER,
            "case {case:?}: expected the message {UPDATE_NAMES_NO_MEMBER:?}, got {message:?}"
        );
        assert_eq!(
            message,
            "Either the definition, the role ARN, the LoggingConfiguration, or the \
             TracingConfiguration must be specified",
            "case {case:?}: unexpected construction of the refusal"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 8: the alias update's resource-absent wording
// ---------------------------------------------------------------------------

/// `UpdateStateMachineAlias` on an alias the store does not hold words the condition
/// for WHICH resource its ARN resolved nothing for — the state machine it qualifies,
/// or the alias name inside it — and names the alias in `resourceName` either way.
///
/// An alias ARN resolves in two steps, so a request that resolves neither is told the
/// step that failed: a state machine the store does not hold is reported by the ARN
/// the request named, and a state machine that IS held but carries no such alias is
/// the condition stated as a sentence of its own, which carries no value of the
/// request at all.
///
/// The rows differ along whether the alias's state machine exists, along which member
/// the update names, and along the alias name — none of which appears in the fixed
/// sentence, which is what shows nothing is interpolated into it.
#[tokio::test]
async fn an_update_of_an_absent_alias_words_which_resource_was_not_found() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "absent-alias", 1).await;
    let absent_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, the alias ARN, the member the update names, the message it earns)
    let cases: Vec<(&str, String, Value, String)> = vec![
        (
            "an alias of an existing state machine, repointed",
            format!("{machine}:BETA"),
            json!({ "routingConfiguration": [route(&format!("{machine}:1"), 100)] }),
            RESOURCE_DOES_NOT_EXIST.to_string(),
        ),
        (
            "an alias of an existing state machine, re-described",
            format!("{machine}:PROD"),
            json!({ "description": "new" }),
            RESOURCE_DOES_NOT_EXIST.to_string(),
        ),
        (
            // The state machine resolves nothing, so the ARN the request named is
            // what the refusal reports — the ARN that identifies the dangling alias.
            "an alias of a state machine that does not exist either",
            format!("{absent_machine}:PROD"),
            json!({ "description": "new" }),
            format!("Resource not found: '{absent_machine}:PROD'"),
        ),
        (
            "an alias of a state machine that does not exist, repointed",
            format!("{absent_machine}:BETA"),
            json!({ "routingConfiguration": [route(&format!("{absent_machine}:1"), 100)] }),
            format!("Resource not found: '{absent_machine}:BETA'"),
        ),
    ];

    for (case, arn, member, expected) in &cases {
        let mut body = json!({ "stateMachineAliasArn": arn });
        for (name, value) in member.as_object().expect("a member object") {
            body[name] = value.clone();
        }
        let (status, parsed) = emulator.call("UpdateStateMachineAlias", &body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {code} with body \
             {parsed:?}"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {parsed:?}"
        );
        assert_eq!(
            parsed.get("resourceName").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the refused alias to be named in resourceName, got \
             {parsed:?}"
        );
    }

    // The wording of the ARN-naming row is the ONE construction every refusal about
    // an ARN the store resolves nothing for reads, so the two cannot drift apart.
    assert_eq!(
        resource_not_found_message(&format!("{absent_machine}:PROD")),
        format!("Resource not found: '{absent_machine}:PROD'"),
        "the ARN-naming refusal is the service's one construction for it"
    );
}
