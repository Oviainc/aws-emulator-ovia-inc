// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's pipeline lifecycle: `CreatePipeline`,
//! `DescribePipeline` and `DeletePipeline`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a
//! pipeline definition is accepted exactly when it declares the published schema
//! version and a step list whose names are unique and whose `DependsOn` graph resolves
//! and is acyclic; an accepted pipeline round-trips through `DescribePipeline`; and the
//! delete removes the pipeline, so the reader refuses it from that call on.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The only pipeline-definition schema version SageMaker Pipelines publishes.
pub(crate) const DEFINITION_VERSION: &str = "2020-12-01";

/// A `ClientRequestToken` built from `seed`, padded to the modelled minimum length of
/// 32 characters (`IdempotencyToken`, len 32..128).
pub(crate) fn token(seed: &str) -> String {
    format!("{seed:-<32}")
}

/// A pipeline definition carrying `steps`, in the published schema version.
pub(crate) fn definition(steps: Value) -> String {
    json!({ "Version": DEFINITION_VERSION, "Metadata": {}, "Steps": steps }).to_string()
}

/// A `Lambda` step named `name`, depending on `depends_on`.
///
/// A Lambda step is a step another step may wait on: a `Fail` step ends the run and a
/// `Condition` step hands control to a branch, so neither may be the target of a
/// `DependsOn` edge (`pipeline_definition_diagnosis` pins those two refusals).
pub(crate) fn step(name: &str, depends_on: &[&str]) -> Value {
    let mut built = json!({
        "Name": name,
        "Type": "Lambda",
        "Arguments": {
            "FunctionArn": format!("arn:aws:lambda:us-east-1:{ACCOUNT}:function:{name}"),
        },
    });
    if !depends_on.is_empty() {
        built["DependsOn"] = json!(depends_on);
    }
    built
}

/// A `Condition` step whose branches hold steps of the same pipeline, nested under
/// `Arguments` the way SageMaker's own definitions nest them.
pub(crate) fn condition_step(name: &str, if_steps: Value, else_steps: Value) -> Value {
    json!({
        "Name": name,
        "Type": "Condition",
        "Arguments": {
            "Conditions": [{ "Type": "Equals", "LeftValue": 1, "RightValue": 1 }],
            "IfSteps": if_steps,
            "ElseSteps": else_steps,
        },
    })
}

/// A minimal `CreatePipeline` request body.
pub(crate) fn create_body(name: &str, definition: &str, request_token: &str) -> Value {
    json!({
        "PipelineName": name,
        "PipelineDefinition": definition,
        "ClientRequestToken": request_token,
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role"),
    })
}

/// RULE: a pipeline definition is ACCEPTED when it declares the published schema
/// version and a non-empty `Steps` list whose step names are unique and whose
/// `DependsOn` entries all name steps the same definition declares, forming an acyclic
/// graph — whatever the step count, the declaration order, or the nesting of a
/// condition step's branches.
#[tokio::test]
async fn a_definition_whose_step_graph_resolves_and_is_acyclic_is_accepted() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: step count, declaration order
    // relative to the dependency direction, graph shape (chain, fan-out, fan-in,
    // diamond) and whether the steps are nested in a condition step's branches.
    let cases: Vec<(&str, Value)> = vec![
        ("single-step", json!([step("Only", &[])])),
        (
            "two-independent-steps",
            json!([step("Left", &[]), step("Right", &[])]),
        ),
        (
            "chain-declared-in-order",
            json!([
                step("First", &[]),
                step("Second", &["First"]),
                step("Third", &["Second"])
            ]),
        ),
        (
            "chain-declared-in-reverse",
            json!([
                step("Last", &["Middle"]),
                step("Middle", &["Head"]),
                step("Head", &[])
            ]),
        ),
        (
            "diamond",
            json!([
                step("Fan", &[]),
                step("LeftArm", &["Fan"]),
                step("RightArm", &["Fan"]),
                step("Join", &["LeftArm", "RightArm"])
            ]),
        ),
        (
            "condition-branches",
            json!([
                step("Prepare", &[]),
                condition_step(
                    "Check",
                    json!([step("WhenTrue", &["Prepare"])]),
                    json!([step("WhenFalse", &[])]),
                )
            ]),
        ),
        (
            "top-level-step-depends-on-a-branch-step",
            json!([
                condition_step("Gate", json!([step("Branch", &[])]), json!([])),
                step("After", &["Branch"])
            ]),
        ),
        (
            // A step that is not a condition step carries its own payload: a list that
            // merely happens to be named `Steps` in it is the step's own argument, not
            // nested pipeline steps, so it is accepted rather than walked.
            "non-condition-step-whose-arguments-carry-a-steps-list",
            json!([{
                "Name": "Invoke",
                "Type": "Lambda",
                "Arguments": {
                    "FunctionArn": format!("arn:aws:lambda:us-east-1:{ACCOUNT}:function:f"),
                    "Steps": [{ "Opaque": "payload" }],
                },
            }]),
        ),
    ];

    for (case, steps) in cases {
        let created = emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &definition(steps), &token(case)),
            )
            .await;
        assert_eq!(
            str_field(&created, "PipelineArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/{case}"),
            "case {case}: PipelineArn, got {created:?}"
        );
    }

    // The same rule across step COUNTS: an n-step chain resolves for every n, and the
    // same n steps closed into a cycle is refused (the next test's rule, pinned here
    // over the same sizes so one axis is varied at a time).
    for length in 2..=8_usize {
        let names: Vec<String> = (0..length).map(|index| format!("Step{index}")).collect();
        let chain: Vec<Value> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let parents: Vec<&str> = if index == 0 {
                    Vec::new()
                } else {
                    vec![names[index - 1].as_str()]
                };
                step(name, &parents)
            })
            .collect();
        let case = format!("chain-of-{length}");
        let created = emulator
            .ok(
                "CreatePipeline",
                &create_body(&case, &definition(json!(chain)), &token(&case)),
            )
            .await;
        assert_eq!(
            str_field(&created, "PipelineArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/{case}"),
            "case {case}: PipelineArn, got {created:?}"
        );

        let cycle: Vec<Value> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let parent = &names[(index + length - 1) % length];
                step(name, &[parent.as_str()])
            })
            .collect();
        let case = format!("cycle-of-{length}");
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(&case, &definition(json!(cycle)), &token(&case)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a cycle of {length} steps must be refused, got {status} {body:?}"
        );
    }
}

/// RULE: a definition SageMaker cannot resolve is refused with 400
/// `ValidationException` and nothing is persisted — whether it is not JSON, declares no
/// supported schema version, declares no steps, declares a step twice, names a step it
/// does not declare, or closes its `DependsOn` edges into a cycle.
#[tokio::test]
async fn an_unresolvable_definition_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();

    // Rows differ along the KIND of unresolvability, each an instance of the rule.
    let cases: Vec<(&str, String)> = vec![
        ("not-json", "{ this is not json".to_string()),
        ("json-but-not-an-object", "[]".to_string()),
        (
            "missing-version",
            json!({ "Steps": [step("Only", &[])] }).to_string(),
        ),
        (
            "unsupported-version",
            json!({ "Version": "2019-01-01", "Steps": [step("Only", &[])] }).to_string(),
        ),
        (
            "missing-steps",
            json!({ "Version": DEFINITION_VERSION }).to_string(),
        ),
        ("empty-steps", definition(json!([]))),
        ("step-is-not-an-object", definition(json!(["Only"]))),
        (
            "step-without-name",
            definition(json!([{ "Type": "Fail", "Arguments": {} }])),
        ),
        (
            "step-without-type",
            definition(json!([{ "Name": "Only", "Arguments": {} }])),
        ),
        (
            "duplicate-step-name",
            definition(json!([step("Twice", &[]), step("Twice", &[])])),
        ),
        (
            "dependency-not-declared",
            definition(json!([step("Only", &["Absent"])])),
        ),
        (
            "dependency-not-declared-in-a-branch",
            definition(json!([condition_step(
                "Gate",
                json!([step("Branch", &["Absent"])]),
                json!([]),
            )])),
        ),
        (
            "dependency-is-not-a-step-name",
            definition(json!([{ "Name": "Only", "Type": "Fail", "DependsOn": [7] }])),
        ),
        (
            "depends-on-is-not-a-list",
            definition(json!([{ "Name": "Only", "Type": "Fail", "DependsOn": "Other" }])),
        ),
        (
            "self-cycle",
            definition(json!([step("Loop", &["Loop"])])),
        ),
        (
            "two-step-cycle",
            definition(json!([step("Ping", &["Pong"]), step("Pong", &["Ping"])])),
        ),
        (
            "cycle-reached-from-an-acyclic-prefix",
            definition(json!([
                step("Entry", &["Ring1"]),
                step("Ring1", &["Ring2"]),
                step("Ring2", &["Ring1"])
            ])),
        ),
        (
            "cycle-across-a-condition-branch",
            definition(json!([
                condition_step("Gate", json!([step("Branch", &["Tail"])]), json!([])),
                step("Tail", &["Branch"])
            ])),
        ),
    ];

    for (case, definition) in cases {
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(case, &definition, &token(case)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {body:?}"
        );

        // Nothing was persisted: the name is still unknown to the reader.
        let (status, body) = emulator
            .call("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: a refused create must persist nothing, got {status} {body:?}"
        );
    }
}

/// RULE: exactly one definition source is acted on — the inline `PipelineDefinition` is
/// read, neither source or both is refused with 400 `ValidationException`, and the
/// `PipelineDefinitionS3Location` form is not implemented, so it answers the scaffold's
/// 501 rather than persisting a pipeline with no definition.
#[tokio::test]
async fn the_definition_source_decides_what_the_create_does() {
    let emulator = Emulator::new();
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role");
    let inline = definition(json!([step("Only", &[])]));
    let s3 = json!({ "Bucket": "pipeline-definitions", "ObjectKey": "demo/definition.json" });

    // Rows: inline only, S3 only, both, neither.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "inline-only",
            json!({
                "PipelineName": "inline-only",
                "PipelineDefinition": inline,
                "ClientRequestToken": token("inline-only"),
                "RoleArn": role,
            }),
            200,
            "",
        ),
        (
            "s3-only",
            json!({
                "PipelineName": "s3-only",
                "PipelineDefinitionS3Location": s3,
                "ClientRequestToken": token("s3-only"),
                "RoleArn": role,
            }),
            501,
            "NotImplemented",
        ),
        (
            "both-sources",
            json!({
                "PipelineName": "both-sources",
                "PipelineDefinition": inline,
                "PipelineDefinitionS3Location": s3,
                "ClientRequestToken": token("both-sources"),
                "RoleArn": role,
            }),
            400,
            "ValidationException",
        ),
        (
            "no-source",
            json!({
                "PipelineName": "no-source",
                "ClientRequestToken": token("no-source"),
                "RoleArn": role,
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let (status, parsed) = emulator.call("CreatePipeline", &body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} {parsed:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&parsed),
                expected_code,
                "case {case}: error code, got {parsed:?}"
            );
            let (status, parsed) = emulator
                .call("DescribePipeline", &json!({ "PipelineName": case }))
                .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ResourceNotFound"),
                "case {case}: a refused create must persist nothing, got {status} {parsed:?}"
            );
        }
    }
}

/// RULE: `DescribePipeline` replays every member `CreatePipeline` stored, present
/// members and absent ones alike, reports the pipeline `Active`, and answers the same
/// record whether the caller names it by name or by ARN.
#[tokio::test]
async fn describe_replays_what_create_stored() {
    let emulator = Emulator::new();
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role");
    let definition = definition(json!([step("Prepare", &[]), step("Train", &["Prepare"])]));

    // Rows differ along which OPTIONAL members the create carried.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required-members-only",
            json!({
                "PipelineName": "required-members-only",
                "PipelineDefinition": definition,
                "ClientRequestToken": token("required-members-only"),
                "RoleArn": role,
            }),
        ),
        (
            "with-display-name-and-description",
            json!({
                "PipelineName": "with-display-name-and-description",
                "PipelineDisplayName": "Shown-To-Humans",
                "PipelineDescription": "a two-step pipeline",
                "PipelineDefinition": definition,
                "ClientRequestToken": token("with-display-name"),
                "RoleArn": role,
            }),
        ),
        (
            "with-parallelism-and-tags",
            json!({
                "PipelineName": "with-parallelism-and-tags",
                "PipelineDefinition": definition,
                "ParallelismConfiguration": { "MaxParallelExecutionSteps": 4 },
                "Tags": [{ "Key": "owner", "Value": "science" }],
                "ClientRequestToken": token("with-parallelism"),
                "RoleArn": role,
            }),
        ),
    ];

    for (case, body) in cases {
        let created = emulator.ok("CreatePipeline", &body).await;
        let arn = str_field(&created, "PipelineArn");

        for reference in [case.to_string(), arn.clone()] {
            let described = emulator
                .ok(
                    "DescribePipeline",
                    &json!({ "PipelineName": reference.clone() }),
                )
                .await;
            assert_eq!(
                str_field(&described, "PipelineArn"),
                arn,
                "case {case} by {reference}: PipelineArn, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "PipelineName"),
                case,
                "case {case} by {reference}: PipelineName, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "PipelineDefinition"),
                definition,
                "case {case} by {reference}: PipelineDefinition must be the stored text, \
                 got {described:?}"
            );
            assert_eq!(
                str_field(&described, "RoleArn"),
                role,
                "case {case} by {reference}: RoleArn, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "PipelineStatus"),
                "Active",
                "case {case} by {reference}: a created pipeline is Active, got {described:?}"
            );
            for (member, expected) in [
                ("PipelineDescription", body.get("PipelineDescription")),
                (
                    "ParallelismConfiguration",
                    body.get("ParallelismConfiguration"),
                ),
            ] {
                assert_eq!(
                    described.get(member),
                    expected,
                    "case {case} by {reference}: {member} must replay what the create \
                     carried, got {described:?}"
                );
            }
            // The display name is the one the create stated, or the pipeline's own name
            // when it stated none (pinned in `service_resolved_members`).
            assert_eq!(
                described.get("PipelineDisplayName").and_then(Value::as_str),
                Some(
                    body.get("PipelineDisplayName")
                        .and_then(Value::as_str)
                        .unwrap_or(case)
                ),
                "case {case} by {reference}: PipelineDisplayName, got {described:?}"
            );
            assert_eq!(
                described.get("CreationTime"),
                described.get("LastModifiedTime"),
                "case {case} by {reference}: an untouched pipeline was last modified when \
                 it was created, got {described:?}"
            );
            assert!(
                described.get("CreationTime").and_then(Value::as_f64) > Some(0.0),
                "case {case} by {reference}: CreationTime must be the instant of the \
                 create, got {described:?}"
            );
            assert!(
                described.get("LastRunTime").is_none(),
                "case {case} by {reference}: a pipeline that never ran reports no \
                 LastRunTime, got {described:?}"
            );
        }
    }

    // A pipeline nobody created, and a version id no pipeline minted, are both refused
    // with the modelled not-found rather than an empty success.
    let cases: Vec<(&str, Value)> = vec![
        ("unknown-name", json!({ "PipelineName": "never-created" })),
        (
            "unknown-arn",
            json!({ "PipelineName": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/never-created") }),
        ),
        (
            "unknown-version",
            json!({ "PipelineName": "required-members-only", "PipelineVersionId": 2 }),
        ),
    ];
    for (case, body) in cases {
        let (status, parsed) = emulator.call("DescribePipeline", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: expected 400 ResourceNotFound, got {status} {parsed:?}"
        );
    }
    // The one version a created pipeline carries is the one the create minted.
    let described = emulator
        .ok(
            "DescribePipeline",
            &json!({ "PipelineName": "required-members-only", "PipelineVersionId": 1 }),
        )
        .await;
    assert_eq!(
        str_field(&described, "PipelineName"),
        "required-members-only",
        "case version-1: the created pipeline's only version is 1, got {described:?}"
    );
}

/// RULE: every ARN this service mints for a pipeline is
/// `arn:aws:sagemaker:<signed region>:<account>:pipeline/<name>`, whatever the name's
/// shape and whatever region the caller signed for; a name outside the modelled length
/// bound (`PipelineName`, len 1..256) is refused.
#[tokio::test]
async fn minted_pipeline_arns_name_the_signed_region_and_the_stored_name() {
    let emulator = Emulator::new();
    let definition = definition(json!([step("Only", &[])]));

    // Rows differ along region and name shape, including the modelled maximum length.
    let longest = format!("p{}", "0".repeat(255));
    let names = [
        "p".to_string(),
        "pipeline-with-hyphens".to_string(),
        "Pipeline123".to_string(),
        longest.clone(),
    ];
    for (index, region) in ["us-east-1", "eu-west-2", "ap-southeast-1"]
        .into_iter()
        .enumerate()
    {
        for (offset, name) in names.iter().enumerate() {
            // Names are unique per row so each create is a first create.
            let name = if name == &longest {
                format!("{}{index}{offset}", &longest[..longest.len() - 2])
            } else {
                format!("{name}-{index}{offset}")
            };
            let (status, body) = emulator
                .call_in(
                    region,
                    "CreatePipeline",
                    &create_body(&name, &definition, &token(&format!("{index}{offset}"))),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {region}/{name}: CreatePipeline, got {status} {body:?}"
            );
            assert_eq!(
                str_field(&body, "PipelineArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:pipeline/{name}"),
                "case {region}/{name}: PipelineArn, got {body:?}"
            );
        }
    }

    // One past each published bound of `PipelineName`: empty, and 257 characters.
    for (case, name) in [
        ("empty-name", String::new()),
        ("name-one-over-the-maximum", format!("p{}", "0".repeat(256))),
    ] {
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(&name, &definition, &token(case)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a name outside the modelled bound must be refused, got {status} \
             {body:?}"
        );
    }
}

/// RULE: a pipeline name is unique within an account and region — a second create under
/// a name in use is refused and changes nothing — while a RETRY of one create, which
/// repeats its `ClientRequestToken`, completes no more than once: it answers the first
/// call's ARN and leaves the stored pipeline as that call wrote it.
#[tokio::test]
async fn a_repeated_create_is_idempotent_and_a_second_create_is_refused() {
    let emulator = Emulator::new();
    let first_definition = definition(json!([step("Only", &[])]));
    let other_definition = definition(json!([step("Different", &[])]));
    let request_token = token("one-create");

    let created = emulator
        .ok(
            "CreatePipeline",
            &create_body("twice", &first_definition, &request_token),
        )
        .await;
    let arn = str_field(&created, "PipelineArn");
    let described = emulator
        .ok("DescribePipeline", &json!({ "PipelineName": "twice" }))
        .await;
    let created_at = described.get("CreationTime").cloned();

    // Rows: the retry of the same call (same token), the retry carrying a different
    // definition, and a genuinely second create (a different token).
    let cases: Vec<(&str, String, String, u16)> = vec![
        (
            "same-token-same-definition",
            request_token.clone(),
            first_definition.clone(),
            200,
        ),
        (
            "same-token-different-definition",
            request_token.clone(),
            other_definition.clone(),
            200,
        ),
        (
            "different-token",
            token("another-create"),
            first_definition.clone(),
            400,
        ),
    ];

    for (case, request_token, definition, expected_status) in cases {
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body("twice", &definition, &request_token),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if expected_status == 200 {
            assert_eq!(
                str_field(&body, "PipelineArn"),
                arn,
                "case {case}: a retry answers the first call's ARN, got {body:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: error code, got {body:?}"
            );
        }

        // Whatever the answer, the stored pipeline is still the one the first call
        // wrote: no retry and no refused create rewrote it.
        let described = emulator
            .ok("DescribePipeline", &json!({ "PipelineName": "twice" }))
            .await;
        assert_eq!(
            str_field(&described, "PipelineDefinition"),
            first_definition,
            "case {case}: the stored definition must be the first call's, got {described:?}"
        );
        assert_eq!(
            described.get("CreationTime").cloned(),
            created_at,
            "case {case}: the stored CreationTime must be the first call's, got {described:?}"
        );
    }
}

/// RULE: `DeletePipeline` removes the pipeline — it answers the ARN the pipeline
/// carried, and from that call on the reader refuses the name with the modelled
/// not-found, a second delete of it is refused the same way, and the name is free to
/// create again — and a delete of a pipeline nobody created is refused with that same
/// not-found.
#[tokio::test]
async fn delete_makes_the_pipeline_unreadable_and_frees_its_name() {
    let emulator = Emulator::new();
    let definition = definition(json!([step("Only", &[])]));

    // Rows differ along how many deletes the caller issues: one, and a repeat, which
    // must be refused rather than succeed a second time.
    for (case, deletes) in [("single-delete", 1_usize), ("repeated-delete", 2)] {
        let created = emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &definition, &token(case)),
            )
            .await;
        let arn = str_field(&created, "PipelineArn");

        for attempt in 0..deletes {
            let (status, body) = emulator
                .call(
                    "DeletePipeline",
                    &json!({
                        "PipelineName": case,
                        "ClientRequestToken": token(&format!("{case}{attempt}")),
                    }),
                )
                .await;
            if attempt == 0 {
                assert_eq!(
                    (status, str_field(&body, "PipelineArn")),
                    (200, arn.clone()),
                    "case {case}/attempt {attempt}: DeletePipeline answers the \
                     pipeline's ARN, got {status} {body:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFound"),
                    "case {case}/attempt {attempt}: a pipeline already deleted is gone, \
                     got {status} {body:?}"
                );
            }

            // Straight after the call the record is gone: no transient status, no
            // readable leftover.
            let (status, body) = emulator
                .call("DescribePipeline", &json!({ "PipelineName": case }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFound"),
                "case {case}/attempt {attempt}: a deleted pipeline is refused by the \
                 reader, got {status} {body:?}"
            );
            assert_eq!(
                message(&body),
                format!("Pipeline '{arn}' does not exist."),
                "case {case}/attempt {attempt}: the reader names the resolved ARN in \
                 single quotes, got {body:?}"
            );
        }

        // And the name is free at once: a create under it is a first create again.
        let recreated = emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &definition, &token(&format!("{case}-again"))),
            )
            .await;
        assert_eq!(
            str_field(&recreated, "PipelineArn"),
            arn,
            "case {case}: the name is reusable once the pipeline is deleted, \
             got {recreated:?}"
        );
        assert_eq!(
            str_field(
                &emulator
                    .ok("DescribePipeline", &json!({ "PipelineName": case }))
                    .await,
                "PipelineStatus"
            ),
            "Active",
            "case {case}: the recreated pipeline is Active"
        );
    }

    let (status, body) = emulator
        .call(
            "DeletePipeline",
            &json!({ "PipelineName": "never-created", "ClientRequestToken": token("absent") }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFound"),
        "case unknown-name: expected 400 ResourceNotFound, got {status} {body:?}"
    );
    assert_eq!(
        message(&body),
        format!(
            "Pipeline 'arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/never-created' \
             does not exist."
        ),
        "case unknown-name: the delete names the resolved ARN in single quotes, \
         got {body:?}"
    );
}

/// RULE: one definition declares at most the published maximum number of steps and of
/// `DependsOn` edges between them — the two counts resolving the graph is the product of
/// — so a definition AT each bound is accepted and one past it is refused with 400
/// `ValidationException`, whatever the megabyte the modelled `PipelineDefinition` length
/// admits is filled with.
#[tokio::test]
async fn a_definition_past_the_step_or_dependency_bound_is_refused() {
    // The bounds the validator enforces, read from the module that states them rather
    // than repeated as literals here.
    let (max_steps, max_dependencies) =
        service_com_amazonaws_sagemaker_sagemaker::state::pipeline_definition_bounds();
    let emulator = Emulator::new();

    let independent_steps = |count: usize| -> Value {
        json!((0..count)
            .map(|index| step(&format!("Step{index}"), &[]))
            .collect::<Vec<Value>>())
    };

    // Rows: at the step bound, one past it, at the dependency bound, one past it. The
    // two dependency rows land on EXACTLY the bound and the bound plus one: the edges
    // are spread over the steps the step bound allows, and the remainder is padded onto
    // the last step, so an off-by-one in the edge accounting shows up here.
    let hub_and_spokes = |edges: usize| -> Value {
        let spokes = max_steps - 1;
        let per_spoke = edges / spokes;
        let remainder = edges % spokes;
        let mut steps = vec![step("Hub", &[])];
        for index in 0..spokes {
            let count = per_spoke + usize::from(index < remainder);
            let parents: Vec<&str> = vec!["Hub"; count];
            steps.push(step(&format!("Spoke{index}"), &parents));
        }
        json!(steps)
    };

    let cases: Vec<(String, Value, u16)> = vec![
        (
            format!("at-the-step-bound-{max_steps}"),
            independent_steps(max_steps),
            200,
        ),
        (
            format!("one-step-past-the-bound-{}", max_steps + 1),
            independent_steps(max_steps + 1),
            400,
        ),
        (
            format!("at-the-dependency-bound-{max_dependencies}"),
            hub_and_spokes(max_dependencies),
            200,
        ),
        (
            format!("one-dependency-past-the-bound-{}", max_dependencies + 1),
            hub_and_spokes(max_dependencies + 1),
            400,
        ),
    ];

    for (index, (case, steps, expected_status)) in cases.into_iter().enumerate() {
        let name = format!("bounds{index}");
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(&name, &definition(steps), &token(&name)),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: error code, got {body:?}"
            );
        }
    }
}
