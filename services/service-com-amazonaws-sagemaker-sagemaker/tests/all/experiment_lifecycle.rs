// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's experiment-management surface:
//! `CreateExperiment` and the `DescribeExperiment` that reads it back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `tests/all/inference_deployment_lifecycle.rs` owns.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region; the create/describe round trip for every
//! optional member, including each published length bound; the case-insensitive
//! uniqueness of the name; and the refusals for a duplicate name, a name that names no
//! experiment, and a name the modelled constraints reject.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// `ExperimentEntityName` is `len 1..120` with pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`
/// (smithy_query operation=CreateExperiment), so this name sits exactly on the upper bound.
fn name_at_max_length() -> String {
    format!("a{}", "b".repeat(119))
}

/// `ExperimentDescription` is `len 0..3072` with pattern `.*`
/// (smithy_query operation=CreateExperiment): this description sits on the upper bound.
fn description_at_max_length() -> String {
    "d".repeat(3072)
}

/// RULE: a created experiment's ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:experiment/<ExperimentName>`, for every
/// name the model admits and whatever region the caller signed for (the emulator default
/// when the request is unsigned), and `DescribeExperiment` reports that same ARN.
#[tokio::test]
async fn created_experiment_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape and length: the 1-character lower bound, digits,
    // embedded hyphens, and the 120-character upper bound.
    let max_name = name_at_max_length();
    let names = ["a", "exp1", "my-experiment-2026", max_name.as_str()];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name-len={}", name.len());
            let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:experiment/{name}");

            let (status, created) = emulator
                .call_in(region, "CreateExperiment", &json!({ "ExperimentName": name }))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateExperiment, got {status} with {created}"
            );
            assert_eq!(
                str_field(&created, "ExperimentArn"),
                expected,
                "case {case}: ExperimentArn, got {created:?}"
            );

            let (status, described) = emulator
                .call_in(region, "DescribeExperiment", &json!({ "ExperimentName": name }))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeExperiment, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "ExperimentArn"),
                expected,
                "case {case}: DescribeExperiment ExperimentArn, got {described:?}"
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_unsigned("CreateExperiment", &json!({ "ExperimentName": "unsigned-exp" }))
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: CreateExperiment, got {status} with {created}"
    );
    assert_eq!(
        str_field(&created, "ExperimentArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/unsigned-exp"),
        "case unsigned: ExperimentArn must fall back to the default region, got {created:?}"
    );
}

/// RULE: `DescribeExperiment` replays every member `CreateExperiment` stored, reports
/// `ExperimentName` as the `DisplayName` when the request supplied none, carries equal
/// `CreationTime` and `LastModifiedTime` for an experiment nothing has modified, and is a
/// read: a second describe answers exactly what the first did.
#[tokio::test]
async fn describe_experiment_replays_exactly_what_create_experiment_stored() {
    let long_description = description_at_max_length();
    // Rows differ along which optional members the create carried, the description's
    // length (absent, short, and the published 3072-character maximum), and tag count.
    let cases: Vec<(&str, Value)> = vec![
        ("name-only", json!({})),
        ("display-name-only", json!({ "DisplayName": "DisplayOnly" })),
        ("description-only", json!({ "Description": "a short description" })),
        (
            "display-name-and-description",
            json!({ "DisplayName": "Both-Members", "Description": "both supplied" }),
        ),
        (
            "description-at-max-length",
            json!({ "Description": long_description.clone() }),
        ),
        (
            "tags-and-display-name",
            // `Tags` the create carries are reported by `ListTags`, which the
            // resource_tagging suite asserts over the whole tag surface; this row keeps
            // the create path exercised with tags present.
            json!({
                "DisplayName": "Tagged",
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = json!({ "ExperimentName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateExperiment", &body).await;
        let described = emulator
            .ok("DescribeExperiment", &json!({ "ExperimentName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ExperimentName"),
            case,
            "case {case}: ExperimentName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ExperimentArn"),
            str_field(&created, "ExperimentArn"),
            "case {case}: DescribeExperiment must report the ARN the create minted, got \
             {described:?}"
        );
        // The model documents that `ExperimentName` is displayed when `DisplayName` is
        // absent, so the member is always present on the describe.
        let expected_display = body
            .get("DisplayName")
            .and_then(Value::as_str)
            .unwrap_or(case);
        assert_eq!(
            str_field(&described, "DisplayName"),
            expected_display,
            "case {case}: DisplayName, got {described:?}"
        );
        assert_eq!(
            described.get("Description"),
            body.get("Description"),
            "case {case}: Description must replay what the create stored, got {described:?}"
        );

        let creation_time = described
            .get("CreationTime")
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described:?}"));
        assert_eq!(
            described.get("LastModifiedTime"),
            Some(creation_time),
            "case {case}: nothing has modified the experiment, so LastModifiedTime must \
             equal CreationTime, got {described:?}"
        );

        // The identity the create was signed with, stored then and replayed now. The
        // harness signs with `Credential=AKIDEXAMPLE/...`, and the identity carries both
        // its ARN and the unique id AWS gives that identity, which is a function of the
        // ARN (`service_common::caller_principal::aws_unique_id`).
        let identity_arn = format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE");
        let expected_identity = Some(json!({
            "IamIdentity": {
                "Arn": identity_arn,
                "PrincipalId": service_common::caller_principal::aws_unique_id(
                    "AIDA",
                    &identity_arn,
                ),
            },
        }));
        assert_eq!(
            described.get("CreatedBy").cloned(),
            expected_identity,
            "case {case}: CreatedBy, got {described:?}"
        );
        assert_eq!(
            described.get("LastModifiedBy").cloned(),
            expected_identity,
            "case {case}: the creation is the last modification, so LastModifiedBy must \
             name the creating identity, got {described:?}"
        );

        // A read changes nothing a later caller sees.
        let again = emulator
            .ok("DescribeExperiment", &json!({ "ExperimentName": case }))
            .await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeExperiment must answer what the first did, got \
             {again:?}"
        );
    }
}

/// RULE: an experiment name is unique per account and not case-sensitive, so a create
/// whose name differs from a stored one only in case is refused with HTTP 400
/// `ValidationException` naming that name, the stored experiment is left untouched, and a
/// describe in any spelling resolves to it and reports the spelling the create used.
#[tokio::test]
async fn experiment_names_are_unique_per_account_ignoring_case() {
    // Rows differ along where the case differs and what else the name carries: all
    // lowercase, all uppercase, mixed case with digits and a hyphen.
    let cases = [
        ("myexperiment", "MyExperiment"),
        ("UPPERCASE", "uppercase"),
        ("MiXeD-123", "mixed-123"),
        ("Exp", "eXP"),
    ];

    for (created_as, other_case) in cases {
        let emulator = Emulator::new();
        let case = format!("created={created_as} other={other_case}");

        let created = emulator
            .ok(
                "CreateExperiment",
                &json!({ "ExperimentName": created_as, "DisplayName": "First", "Description": "first" }),
            )
            .await;

        // Both the identical name and the case variant are duplicates.
        for duplicate in [created_as, other_case] {
            let (status, body) = emulator
                .call(
                    "CreateExperiment",
                    &json!({ "ExperimentName": duplicate, "DisplayName": "Second" }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case} duplicate={duplicate}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} duplicate={duplicate}: expected ValidationException, got {body:?}"
            );
            assert!(
                message(&body).contains(duplicate),
                "case {case} duplicate={duplicate}: the message must name the rejected \
                 ExperimentName, got {:?}",
                message(&body)
            );
        }

        // The refused creates left the stored experiment exactly as it was, and either
        // spelling reads it back.
        for spelling in [created_as, other_case] {
            let described = emulator
                .ok("DescribeExperiment", &json!({ "ExperimentName": spelling }))
                .await;
            assert_eq!(
                str_field(&described, "ExperimentName"),
                created_as,
                "case {case} spelling={spelling}: DescribeExperiment must report the name \
                 the create used, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "ExperimentArn"),
                str_field(&created, "ExperimentArn"),
                "case {case} spelling={spelling}: ExperimentArn, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "DisplayName"),
                "First",
                "case {case} spelling={spelling}: a refused duplicate must not overwrite \
                 the stored DisplayName, got {described:?}"
            );
        }
    }
}

/// RULE: `DescribeExperiment` for a name that names no experiment is refused with the
/// modelled `ResourceNotFound` (HTTP 400), never a 200 carrying empty members.
#[tokio::test]
async fn describe_experiment_refuses_a_name_that_names_no_experiment() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": "the-one-that-exists" }))
        .await;

    // Rows differ along name shape and length, including the 1-character lower bound and
    // the 120-character upper bound, plus a name that is a prefix of the stored one.
    let max_name = name_at_max_length();
    let missing = [
        "x",
        "never-created",
        "the-one",
        "the-one-that-exists2",
        max_name.as_str(),
    ];

    for name in missing {
        let (status, body) = emulator
            .call("DescribeExperiment", &json!({ "ExperimentName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case missing={name}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case missing={name}: expected ResourceNotFound, got {body:?}"
        );
        assert!(
            message(&body).contains(name),
            "case missing={name}: the message must name the ExperimentName the request \
             supplied, got {:?}",
            message(&body)
        );
    }
}

/// RULE: a name outside `ExperimentEntityName`'s modelled constraints (`len 1..120`,
/// pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`) is refused with HTTP 400
/// `ValidationException` and no experiment is created.
#[tokio::test]
async fn malformed_experiment_names_are_refused_and_persist_nothing() {
    let one_past_max = "a".repeat(121);
    let one_past_description = "d".repeat(3073);
    // Rows differ along which part of the constraint the value breaks: the length's two
    // published bounds, each by one; the leading and trailing character class; and a
    // character the pattern admits nowhere.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("empty-name", json!({ "ExperimentName": "" }), false),
        (
            "name-one-past-max-length",
            json!({ "ExperimentName": one_past_max.clone() }),
            false,
        ),
        ("leading-hyphen", json!({ "ExperimentName": "-leading" }), false),
        (
            "trailing-hyphen",
            json!({ "ExperimentName": "trailing-" }),
            false,
        ),
        (
            "underscore",
            json!({ "ExperimentName": "under_score" }),
            false,
        ),
        ("space", json!({ "ExperimentName": "two words" }), false),
        (
            "description-one-past-max-length",
            json!({ "ExperimentName": "valid-name", "Description": one_past_description.clone() }),
            true,
        ),
        (
            "display-name-malformed",
            json!({ "ExperimentName": "valid-name", "DisplayName": "bad display name!" }),
            true,
        ),
    ];

    for (case, body, name_is_well_formed) in cases {
        let emulator = Emulator::new();
        let (status, refusal) = emulator.call("CreateExperiment", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case}: expected ValidationException, got {refusal:?}"
        );

        // Nothing was persisted: the name the refused request carried names no experiment.
        let name = body
            .get("ExperimentName")
            .and_then(Value::as_str)
            .expect("every case names an ExperimentName");
        let (status, described) = emulator
            .call("DescribeExperiment", &json!({ "ExperimentName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create must persist nothing, got {status} with {described}"
        );
        // Where the NAME itself is well formed, the describe reaches the handler, so the
        // refusal is the absence of a stored record rather than the name's own
        // constraint: that is what makes this probe load-bearing.
        if name_is_well_formed {
            assert_eq!(
                error_code(&described),
                "ResourceNotFound",
                "case {case}: a refused create must leave the name naming no experiment, got \
                 {described:?}"
            );
        }
    }

    // The two rows above that break only an optional member or the length bound by one
    // character succeed at the bound itself, so the refusals are the constraint and not a
    // blanket rejection.
    let emulator = Emulator::new();
    let at_bounds = json!({
        "ExperimentName": name_at_max_length(),
        "DisplayName": name_at_max_length(),
        "Description": description_at_max_length(),
    });
    let created = emulator.ok("CreateExperiment", &at_bounds).await;
    assert_eq!(
        str_field(&created, "ExperimentArn"),
        format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/{}",
            name_at_max_length()
        ),
        "case at-published-bounds: ExperimentArn, got {created:?}"
    );
}
