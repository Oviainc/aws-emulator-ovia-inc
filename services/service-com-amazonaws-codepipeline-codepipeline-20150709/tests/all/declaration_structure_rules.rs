// SPDX-License-Identifier: CC0-1.0

//! The structural laws a `PipelineDeclaration` is held to, and how the input-constraint
//! violations of one request are counted — end to end over the wire the service speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test, each asserted over a SET of inputs and over EVERY operation it
//! reaches, because one law may not be worded two ways:
//!
//! * a pipeline carries at least [`MIN_STAGES_PER_PIPELINE`] stages, and a declaration
//!   below that floor is refused as a malformed STRUCTURE, stating the count it declared;
//! * a pipeline acts on the revision its source stage brings in, so a declaration
//!   carrying no action outside the `Source` category — including one whose only other
//!   stage declares no action at all — is refused as a malformed structure with the fixed
//!   sentence that defect class carries;
//! * an action is addressed by its name within a stage, so a stage declaring two actions
//!   under one name is refused as a malformed ACTION declaration, naming the repeated
//!   action and its stage — while the same name in two different stages is accepted;
//! * a declaration that names no artifact store at all is refused over the INPUT, with
//!   the fixed sentence that requirement carries, rather than as a structure defect;
//! * a `@pattern` is not evaluated against an EMPTY value that already violated a
//!   `@length` minimum the model declares: the floor is the one violation such a value
//!   has;
//! * a `resourceArn` and an approval `token` are held to the length floor their own
//!   `@pattern` implies and the model leaves unstated, reported after the clause for the
//!   constraint the model does declare.
//!
//! The harness comes from `tests/all/pipeline_lifecycle.rs`, so every assertion goes
//! through the same router the production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    duplicate_action_name_message, too_few_stages_message, APPROVAL_TOKEN_MIN_LENGTH,
    MIN_STAGES_PER_PIPELINE, MISSING_ARTIFACT_STORE_MESSAGE, NO_NON_SOURCE_ACTION_MESSAGE,
    RESOURCE_ARN_MIN_LENGTH,
};

use crate::pipeline_lifecycle::{build_stage, source_stage, Emulator, ROLE_ARN};

/// The two operations that take a whole `PipelineDeclaration`, so every declaration law
/// is asserted on both.
const DECLARATION_OPERATIONS: [&str; 2] = ["CreatePipeline", "UpdatePipeline"];

/// The name every row's fixture pipeline carries: `UpdatePipeline` replaces the structure
/// of a pipeline that already exists, so each row creates this one first.
const FIXTURE: &str = "demo";

/// `DisabledReason`'s modelled bounds
/// (`smithy_query(query="shape", shape="DisabledReason")`: `length 1..300`).
const REASON_BOUNDS: (usize, usize) = (1, 300);

/// `DisabledReason`'s modelled `@pattern`, as a refusal quotes it: the expression
/// without the `^`/`$` anchors the model spells it with.
const REASON_PATTERN: &str = r"[a-zA-Z0-9!@ \(\)\.\*\?\-]+";

/// `ResourceArn`'s modelled pattern, as a refusal quotes it: the expression without the
/// `^`/`$` anchors the model spells it with
/// (`smithy_query(query="shape", shape="ResourceArn")`).
const RESOURCE_ARN_PATTERN: &str = r"arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+";

/// `ApprovalToken`'s modelled pattern, quoted the same way
/// (`smithy_query(query="shape", shape="ApprovalToken")`).
const APPROVAL_TOKEN_PATTERN: &str =
    r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

/// How a row builds the request one ARN-addressed operation carries.
type ArnRequest = fn(&str) -> Value;

/// A declaration named `name` over `stages`.
fn pipeline(name: &str, stages: Value) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// One stage carrying `actions`, which may be empty — `StageDeclaration$actions` is
/// `@required`, which an empty list satisfies.
fn stage(name: &str, actions: Vec<Value>) -> Value {
    json!({"name": name, "actions": actions})
}

/// One action of `category`, with the `runOrder` it declares.
///
/// The provider is one of the categories' own (`state::BUILT_IN_ACTION_TYPES`) and the
/// owner is `Custom`, so no action here is work the engine performs while a case is
/// asserting about the declaration it was refused or stored by.
fn action(name: &str, category: &str, run_order: i64) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": category,
            "owner": "Custom",
            "provider": "MyProvider",
            "version": "1",
        },
        "runOrder": run_order,
    })
}

/// A `Source` action, for a first stage under test.
fn source_action(name: &str, run_order: i64) -> Value {
    action(name, "Source", run_order)
}

/// The `(status, code, message)` of a refusal. The awsJson `__type` may carry the
/// model's namespace, so only the shape name is compared.
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, response) = emulator.call(operation, body).await;
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// An emulator holding the fixture pipeline, so a row drives `UpdatePipeline` against a
/// stored structure and can read that structure back after a refusal.
async fn emulator_with_fixture() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline(
                FIXTURE,
                json!([source_stage("Source"), build_stage("Build", 1)]),
            )}),
        )
        .await;
    emulator
}

/// The stage names the stored fixture declares, so a refused call can be shown to have
/// changed nothing.
async fn stored_stages(emulator: &Emulator) -> Value {
    emulator.ok("GetPipeline", &json!({"name": FIXTURE})).await["pipeline"]["stages"].clone()
}

/// RULE: a declaration carrying fewer than [`MIN_STAGES_PER_PIPELINE`] stages describes
/// nothing the service could run, and is refused as a malformed structure stating the
/// count the request declared — whatever those stages hold.
#[tokio::test]
async fn a_declaration_below_the_stage_floor_is_refused_as_a_structure_defect() {
    // The rows differ along what the rule generalises over: the stage COUNT below the
    // floor, and what the stages below it hold (nothing, one action, several, and a stage
    // whose action is not even a source action — which the stage count is refused ahead
    // of).
    let rows: Vec<(&str, Value, usize)> = vec![
        ("no stage at all", json!([]), 0),
        ("one source stage", json!([source_stage("Source")]), 1),
        (
            "one stage carrying several source actions",
            json!([stage(
                "Source",
                vec![
                    source_action("Src", 1),
                    source_action("Mirror", 2),
                    source_action("Tag", 3),
                ],
            )]),
            1,
        ),
        (
            "one stage carrying no source action",
            json!([stage("Build", vec![action("Compile", "Build", 1)])]),
            1,
        ),
    ];

    for operation in DECLARATION_OPERATIONS {
        for (case, stages, count) in &rows {
            let emulator = emulator_with_fixture().await;
            let before = stored_stages(&emulator).await;
            let name = if operation == "UpdatePipeline" {
                FIXTURE
            } else {
                "fresh"
            };
            let (status, code, message) = refusal(
                &emulator,
                operation,
                &json!({"pipeline": pipeline(name, stages.clone())}),
            )
            .await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (
                    400,
                    "InvalidStructureException",
                    too_few_stages_message(*count).as_str()
                ),
                "case {case:?} on {operation}: expected the stage-floor refusal, got \
                 {status} {code} {message:?}"
            );
            assert_eq!(
                stored_stages(&emulator).await,
                before,
                "case {case:?} on {operation}: a refused declaration changes nothing"
            );
            if operation == "CreatePipeline" {
                let (status, code, _) =
                    refusal(&emulator, "GetPipeline", &json!({"name": name})).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "PipelineNotFoundException"),
                    "case {case:?}: a refused create stores no pipeline"
                );
            }
        }
    }

    // The control: a declaration AT the floor is accepted, and so is one above it, so
    // what the rows pin is the floor and not a blanket refusal.
    for extra in 0..3 {
        let emulator = Emulator::new();
        let mut stages = vec![source_stage("Source")];
        for index in 1..MIN_STAGES_PER_PIPELINE + extra {
            stages.push(stage(
                &format!("Act{index}"),
                vec![action("Compile", "Build", 1)],
            ));
        }
        let declared = stages.len();
        let answered = emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": pipeline("accepted", Value::Array(stages))}),
            )
            .await;
        assert_eq!(
            answered["pipeline"]["stages"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            declared,
            "a declaration of {declared} stages is accepted and stored whole, got {answered}"
        );
    }
}

/// RULE: a pipeline acts on the revision its source stage brings in, so a declaration
/// carrying no action outside the `Source` category is refused as a malformed structure
/// with the fixed sentence that defect class carries.
#[tokio::test]
async fn a_declaration_with_no_acting_action_is_refused_as_a_structure_defect() {
    // The rows differ along HOW the declaration manages to carry no acting action: a
    // later stage with no action at all, later stages carrying only source actions, and
    // mixtures of the two over different stage counts.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the second stage declares no action",
            json!([source_stage("Source"), stage("Gate", Vec::new())]),
        ),
        (
            "every action the declaration carries is a source action",
            json!([
                source_stage("Source"),
                stage("Mirror", vec![source_action("Again", 1)]),
            ]),
        ),
        (
            "three stages, two of them empty",
            json!([
                source_stage("Source"),
                stage("Gate", Vec::new()),
                stage("Ship", Vec::new()),
            ]),
        ),
        (
            "a later stage carrying several source actions at different run orders",
            json!([
                stage("Source", vec![source_action("Src", 1)]),
                stage(
                    "More",
                    vec![source_action("Second", 1), source_action("Third", 2)],
                ),
            ]),
        ),
    ];

    for operation in DECLARATION_OPERATIONS {
        for (case, stages) in &rows {
            let emulator = emulator_with_fixture().await;
            let before = stored_stages(&emulator).await;
            let name = if operation == "UpdatePipeline" {
                FIXTURE
            } else {
                "fresh"
            };
            let (status, code, message) = refusal(
                &emulator,
                operation,
                &json!({"pipeline": pipeline(name, stages.clone())}),
            )
            .await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (
                    400,
                    "InvalidStructureException",
                    NO_NON_SOURCE_ACTION_MESSAGE
                ),
                "case {case:?} on {operation}: expected the acting-action refusal, got \
                 {status} {code} {message:?}"
            );
            assert_eq!(
                stored_stages(&emulator).await,
                before,
                "case {case:?} on {operation}: a refused declaration changes nothing"
            );
        }
    }

    // The control: the SAME stage counts with ONE action outside the Source category are
    // accepted, so what the refusal turns on is the action's category.
    for (case, stages) in [
        (
            "an acting action in the second stage",
            json!([
                source_stage("Source"),
                stage("Gate", vec![action("Approve", "Approval", 1)]),
            ]),
        ),
        (
            "an acting action in the third stage, the second one empty of its own",
            json!([
                source_stage("Source"),
                stage("Gate", vec![action("Approve", "Approval", 1)]),
                stage("Ship", vec![action("Deploy", "Deploy", 1)]),
            ]),
        ),
    ] {
        let emulator = Emulator::new();
        let answered = emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": pipeline("accepted", stages)}),
            )
            .await;
        assert_eq!(
            answered["pipeline"]["name"],
            json!("accepted"),
            "case {case:?}: a declaration that acts on its revision is accepted, got \
             {answered}"
        );
    }
}

/// RULE: an action is addressed by its name WITHIN a stage, so one stage cannot declare
/// two actions under one name — refused as a malformed action declaration naming the
/// repeated action and the stage that repeats it — while the same name in two different
/// stages is unambiguous and accepted.
#[tokio::test]
async fn a_stage_declaring_one_action_name_twice_is_refused() {
    // The rows differ along what the rule generalises over: WHICH stage repeats a name,
    // how many actions the stage declares, whether the repeats share a `runOrder`, what
    // else the repeated actions declare (an output artifact each, which the artifact law
    // would otherwise refuse first), and the name's own alphabet and length — the whole
    // character class `ActionName`'s pattern allows (`^[A-Za-z0-9.@\-_]+$`) and its
    // modelled 100-character maximum.
    let long_name = "a".repeat(100);
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the source stage repeats its only action's name",
            json!([
                stage("Source", vec![source_action("Src", 1), source_action("Src", 2)]),
                stage("Build", vec![action("Compile", "Build", 1)]),
            ]),
            "Src",
            "Source",
        ),
        (
            "a later stage repeats a name at one run order",
            json!([
                source_stage("Source"),
                stage(
                    "Build",
                    vec![
                        action("Compile", "Build", 1),
                        action("Test", "Test", 1),
                        action("Compile", "Build", 1),
                    ],
                ),
            ]),
            "Compile",
            "Build",
        ),
        (
            "the third stage repeats a punctuated name",
            json!([
                source_stage("Source"),
                stage("Build", vec![action("Compile", "Build", 1)]),
                stage(
                    "Ship.It@once",
                    vec![
                        action("a.b@c-d_e", "Deploy", 1),
                        action("Other", "Deploy", 2),
                        action("a.b@c-d_e", "Deploy", 3),
                    ],
                ),
            ]),
            "a.b@c-d_e",
            "Ship.It@once",
        ),
        (
            "a repeated name at the modelled length maximum",
            json!([
                source_stage("Source"),
                stage(
                    "Build",
                    vec![
                        action(&long_name, "Build", 1),
                        action(&long_name, "Build", 2),
                    ],
                ),
            ]),
            &long_name,
            "Build",
        ),
        (
            "the repeats each declare the same output artifact",
            json!([
                stage(
                    "Source",
                    vec![
                        json!({
                            "name": "Src",
                            "actionTypeId": {
                                "category": "Source", "owner": "Custom",
                                "provider": "MyProvider", "version": "1",
                            },
                            "outputArtifacts": [{"name": "SourceOut"}],
                            "runOrder": 1,
                        }),
                        json!({
                            "name": "Src",
                            "actionTypeId": {
                                "category": "Source", "owner": "Custom",
                                "provider": "MyProvider", "version": "1",
                            },
                            "outputArtifacts": [{"name": "SourceOut"}],
                            "runOrder": 2,
                        }),
                    ],
                ),
                stage("Build", vec![action("Compile", "Build", 1)]),
            ]),
            "Src",
            "Source",
        ),
    ];

    for operation in DECLARATION_OPERATIONS {
        for (case, stages, action_name, stage_name) in &rows {
            let emulator = emulator_with_fixture().await;
            let before = stored_stages(&emulator).await;
            let name = if operation == "UpdatePipeline" {
                FIXTURE
            } else {
                "fresh"
            };
            let (status, code, message) = refusal(
                &emulator,
                operation,
                &json!({"pipeline": pipeline(name, stages.clone())}),
            )
            .await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (
                    400,
                    "InvalidActionDeclarationException",
                    duplicate_action_name_message(action_name, stage_name).as_str()
                ),
                "case {case:?} on {operation}: expected the repeated-name refusal, got \
                 {status} {code} {message:?}"
            );
            assert_eq!(
                stored_stages(&emulator).await,
                before,
                "case {case:?} on {operation}: a refused declaration changes nothing"
            );
        }
    }

    // The controls: one name per stage is accepted, and the SAME name in two different
    // stages is accepted too — the rule is a stage's own, so it does not reach across
    // stages. Both are read back through `GetPipeline`, so what is asserted is the
    // stored structure rather than the answer alone.
    for (case, stages) in [
        (
            "distinct names in one stage",
            json!([
                source_stage("Source"),
                stage(
                    "Build",
                    vec![action("Compile", "Build", 1), action("Test", "Test", 2)],
                ),
            ]),
        ),
        (
            "one name used once in each of two stages",
            json!([
                stage("Source", vec![source_action("Run", 1)]),
                stage("Build", vec![action("Run", "Build", 1)]),
                stage("Ship", vec![action("Run", "Deploy", 1)]),
            ]),
        ),
    ] {
        let emulator = Emulator::new();
        let answered = emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": pipeline("accepted", stages.clone())}),
            )
            .await;
        let read = emulator
            .ok("GetPipeline", &json!({"name": "accepted"}))
            .await;
        assert_eq!(
            read["pipeline"]["stages"], answered["pipeline"]["stages"],
            "case {case:?}: the accepted structure is the stored one, got {read}"
        );
        let names: Vec<Value> = read["pipeline"]["stages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|stage| stage["actions"].as_array().into_iter().flatten())
            .map(|action| action["name"].clone())
            .collect();
        let declared: Vec<Value> = stages
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|stage| stage["actions"].as_array().into_iter().flatten())
            .map(|action| action["name"].clone())
            .collect();
        assert_eq!(
            names, declared,
            "case {case:?}: every declared action is stored under the name it declared, \
             got {read}"
        );
    }
}

/// RULE: a declaration that names no artifact store at all cannot be stored, and the
/// requirement is stated over the INPUT — `ValidationException` with the fixed sentence
/// that requirement carries — rather than as a defect of the pipeline's structure.
#[tokio::test]
async fn a_declaration_naming_no_artifact_store_is_refused_as_an_input_requirement() {
    // The rows differ along HOW the declaration manages to name no store — the member
    // absent, or present as an empty map — and along the structure beside it, which the
    // requirement does not depend on.
    let rows: Vec<(&str, Value)> = vec![
        (
            "neither member present",
            json!({
                "name": FIXTURE,
                "roleArn": ROLE_ARN,
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
        ),
        (
            "an empty artifactStores map",
            json!({
                "name": FIXTURE,
                "roleArn": ROLE_ARN,
                "artifactStores": {},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
        ),
        (
            "no store, and a structure that is refusable in its own right",
            json!({
                "name": FIXTURE,
                "roleArn": ROLE_ARN,
                "stages": [source_stage("Source")],
            }),
        ),
    ];

    for operation in DECLARATION_OPERATIONS {
        for (case, mut declaration) in rows.clone() {
            let emulator = emulator_with_fixture().await;
            let before = stored_stages(&emulator).await;
            if operation == "CreatePipeline" {
                declaration["name"] = json!("fresh");
            }
            let (status, code, message) =
                refusal(&emulator, operation, &json!({"pipeline": declaration})).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, "ValidationException", MISSING_ARTIFACT_STORE_MESSAGE),
                "case {case:?} on {operation}: expected the artifact-store requirement, \
                 got {status} {code} {message:?}"
            );
            assert_eq!(
                stored_stages(&emulator).await,
                before,
                "case {case:?} on {operation}: a refused declaration changes nothing"
            );
        }
    }

    // The control: either member on its own is accepted, so the requirement is that one
    // of them is named and not that a particular one is.
    for (case, store) in [
        ("artifactStore", json!({"artifactStore": {"type": "S3", "location": "my-bucket"}})),
        (
            "artifactStores",
            json!({"artifactStores": {"us-east-1": {"type": "S3", "location": "east"}}}),
        ),
    ] {
        let emulator = Emulator::new();
        let mut declaration = json!({
            "name": "accepted",
            "roleArn": ROLE_ARN,
            "stages": [source_stage("Source"), build_stage("Build", 1)],
        });
        for (member, value) in store.as_object().cloned().unwrap_or_default() {
            declaration[member] = value;
        }
        let answered = emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        assert_eq!(
            answered["pipeline"]["name"],
            json!("accepted"),
            "case {case:?}: a declaration naming one store is accepted, got {answered}"
        );
    }
}

/// RULE: a `@pattern` is not evaluated against an EMPTY value that already violated a
/// `@length` minimum the MODEL declares — the floor is the one violation such a value
/// has — while a non-empty value is reported against every constraint it does violate,
/// and a member whose floor only the service states keeps both clauses.
#[tokio::test]
async fn an_empty_value_is_reported_against_its_modelled_length_floor_only() {
    let emulator = emulator_with_fixture().await;
    let (min, max) = REASON_BOUNDS;

    // The rows differ along the VALUE and the member carrying it: an empty value for a
    // member whose model declares both a length floor and a pattern, an empty value for
    // a member whose floor the model does NOT declare, and non-empty values that violate
    // one constraint each.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an empty reason",
            "DisableStageTransition",
            json!({
                "pipelineName": FIXTURE,
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "",
            }),
            format!(
                "1 validation error detected: Value at 'reason' failed to satisfy \
                 constraint: Member must have length greater than or equal to {min}"
            ),
        ),
        (
            "an empty pipeline name",
            "GetPipeline",
            json!({"name": ""}),
            "1 validation error detected: Value at 'name' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "an empty resourceArn, whose floor the model does not declare",
            "ListTagsForResource",
            json!({"resourceArn": ""}),
            format!(
                "2 validation errors detected: Value at 'resourceArn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {RESOURCE_ARN_PATTERN}; Value at 'resourceArn' failed to satisfy \
                 constraint: Member must have length greater than or equal to \
                 {RESOURCE_ARN_MIN_LENGTH}"
            ),
        ),
        (
            "a non-empty reason outside the pattern",
            "DisableStageTransition",
            json!({
                "pipelineName": FIXTURE,
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "held/pending#review",
            }),
            format!(
                "1 validation error detected: Value at 'reason' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {REASON_PATTERN}"
            ),
        ),
        (
            "a reason one character past the length maximum",
            "DisableStageTransition",
            json!({
                "pipelineName": FIXTURE,
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "a".repeat(max + 1),
            }),
            format!(
                "1 validation error detected: Value at 'reason' failed to satisfy \
                 constraint: Member must have length less than or equal to {max}"
            ),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, code, message) = refusal(&emulator, operation, &body).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: unexpected refusal, got {status} {code} {message:?}"
        );
    }

    // The control: a reason the constraints admit is accepted, at both ends of the
    // modelled length, so what the rows pin is the violation set and not a blanket
    // refusal.
    for reason in ["a".repeat(min), "a".repeat(max)] {
        emulator
            .ok(
                "DisableStageTransition",
                &json!({
                    "pipelineName": FIXTURE,
                    "stageName": "Build",
                    "transitionType": "Inbound",
                    "reason": reason,
                }),
            )
            .await;
    }
}

/// RULE: a `resourceArn` is held to a length floor the model leaves unstated, reported
/// AFTER the clause for the constraint the model does declare — so a value below the
/// floor states both clauses and one above it states only the pattern's.
#[tokio::test]
async fn a_resource_arn_states_the_length_floor_the_model_leaves_unstated() {
    let emulator = emulator_with_fixture().await;

    // The rows differ along the length and the shape of the malformed ARN: well under the
    // floor, one character under it, exactly at it, and above it — and over every
    // operation that addresses a resource by its ARN.
    let rows: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN whose account is one digit short",
            "arn:aws:codepipeline:us-east-1:00000000000:p".to_string(),
        ),
        (
            "one character under the floor",
            format!("arn:aws:codepipeline:us-east-1:{}", "0".repeat(5)),
        ),
        (
            "exactly at the floor, and malformed",
            format!("arn:aws:codepipeline:us-east-1:{}", "0".repeat(6)),
        ),
        (
            "well above the floor, but another service's ARN",
            format!(
                "arn:aws:codebuild:us-east-1:000000000000:project/{}",
                "p".repeat(20)
            ),
        ),
    ];

    // Each operation that carries `resourceArn`, with the other members it requires.
    let operations: [(&str, ArnRequest); 3] = [
        ("ListTagsForResource", |arn| json!({"resourceArn": arn})),
        ("TagResource", |arn| {
            json!({"resourceArn": arn, "tags": [{"key": "team", "value": "build"}]})
        }),
        ("UntagResource", |arn| {
            json!({"resourceArn": arn, "tagKeys": ["team"]})
        }),
    ];

    for (operation, body) in operations {
        for (case, arn) in &rows {
            let pattern_clause = format!(
                "Value at 'resourceArn' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: {RESOURCE_ARN_PATTERN}"
            );
            let length_clause = format!(
                "Value at 'resourceArn' failed to satisfy constraint: Member must have \
                 length greater than or equal to {RESOURCE_ARN_MIN_LENGTH}"
            );
            // The rule itself: the floor is stated for a value below it, after the
            // pattern clause the model's own constraint states.
            let expected = if i64::try_from(arn.chars().count()).expect("a short value")
                < RESOURCE_ARN_MIN_LENGTH
            {
                format!("2 validation errors detected: {pattern_clause}; {length_clause}")
            } else {
                format!("1 validation error detected: {pattern_clause}")
            };
            let (status, code, message) = refusal(&emulator, operation, &body(arn)).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, "ValidationException", expected.as_str()),
                "case {case:?} on {operation}: unexpected refusal, got {status} {code} \
                 {message:?}"
            );
        }
    }

    // The control: the ARN the service minted for the fixture pipeline satisfies both
    // constraints, so the listing answers from state rather than refusing.
    let arn = emulator.ok("GetPipeline", &json!({"name": FIXTURE})).await["metadata"]
        ["pipelineArn"]
        .as_str()
        .expect("a stored pipeline carries its ARN")
        .to_string();
    assert!(
        i64::try_from(arn.chars().count()).expect("a short ARN") >= RESOURCE_ARN_MIN_LENGTH,
        "the ARN this service mints is at or above the floor it enforces, got {arn:?}"
    );
    let listed = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    assert!(
        listed["tags"].is_null() || listed["tags"].is_array(),
        "a well-formed ARN is served from state, got {listed}"
    );
}

/// RULE: the same floor rule on the other member whose pattern implies one — an approval
/// `token` below the length its pattern admits states the floor clause AFTER the pattern
/// clause the model declares, and a value at or above that length states only the
/// pattern's.
#[tokio::test]
async fn an_approval_token_states_the_length_floor_the_model_leaves_unstated() {
    let emulator = emulator_with_fixture().await;

    let approval = |token: &str| {
        json!({
            "pipelineName": FIXTURE,
            "stageName": "Build",
            "actionName": "Build0",
            "result": {"summary": "looks good", "status": "Approved"},
            "token": token,
        })
    };

    // The rows differ along the LENGTH and the shape of the malformed token: empty, well
    // under the floor, one character under it, exactly at it, and above it.
    let rows: Vec<(&str, String)> = vec![
        ("an empty token", String::new()),
        ("not a token at all", "nope".to_string()),
        (
            "one character under the floor",
            "1".repeat(usize::try_from(APPROVAL_TOKEN_MIN_LENGTH - 1).expect("a small floor")),
        ),
        (
            "exactly at the floor, and malformed",
            "z".repeat(usize::try_from(APPROVAL_TOKEN_MIN_LENGTH).expect("a small floor")),
        ),
        (
            "well above the floor, and malformed",
            "0123456789".repeat(8),
        ),
    ];

    for (case, token) in &rows {
        let pattern_clause = format!(
            "Value at 'token' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: {APPROVAL_TOKEN_PATTERN}"
        );
        let length_clause = format!(
            "Value at 'token' failed to satisfy constraint: Member must have length greater \
             than or equal to {APPROVAL_TOKEN_MIN_LENGTH}"
        );
        let expected = if i64::try_from(token.chars().count()).expect("a short value")
            < APPROVAL_TOKEN_MIN_LENGTH
        {
            format!("2 validation errors detected: {pattern_clause}; {length_clause}")
        } else {
            format!("1 validation error detected: {pattern_clause}")
        };
        let (status, code, message) = refusal(&emulator, "PutApprovalResult", &approval(token)).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: unexpected refusal, got {status} {code} {message:?}"
        );
    }

    // The control: a token the pattern admits is past validation entirely, so the refusal
    // comes from the handler resolving it against state instead.
    let (status, code, message) = refusal(
        &emulator,
        "PutApprovalResult",
        &approval("11111111-2222-4333-8444-555555555555"),
    )
    .await;
    assert_ne!(
        code.as_str(),
        "ValidationException",
        "case \"a well-formed token\": the constraints admit it, got {status} {code} {message:?}"
    );
}
