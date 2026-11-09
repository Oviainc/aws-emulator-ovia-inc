// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's experiment-tracking entities:
//! `CreateTrialComponent` and the `DescribeTrialComponent` that reads one back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region; the create/describe round trip for every
//! optional member (status, both parameter-union arms, artifacts with and without a
//! media type, metadata properties, timestamps at second and millisecond precision,
//! non-ASCII keys and values); the name-or-ARN resolution a reader modelled
//! `ExperimentEntityNameOrArn` owes; the refusal of a duplicate name; and the modelled
//! `ResourceNotFound` for a component that was never created.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialComponentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`,
/// smithy_query operation=CreateTrialComponent) — the same value the handlers mint from,
/// `state::TRIAL_COMPONENT_KIND`, spelled out here so the test pins the wire form rather
/// than echoing the constant under test.
const KIND: &str = "experiment-trial-component";

/// The name of the lineage group every lineage entity of a Region belongs to, read from
/// the crate's own derivation so the suite pins the name the SERVICE computes for it
/// rather than a second spelling of it.
fn default_lineage_group(region: &str) -> String {
    service_com_amazonaws_sagemaker_sagemaker::state::default_lineage_group_name(region)
}

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`.
fn name_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// RULE: `CreateTrialComponent` mints
/// `arn:aws:sagemaker:<signed region>:<account>:experiment-trial-component/<name>` —
/// the region of the caller's SigV4 credential scope (the emulator default when the
/// request is unsigned) and the stored name, whatever the name's shape.
#[tokio::test]
async fn create_trial_component_mints_the_modelled_arn_for_every_name_and_region() {
    let emulator = Emulator::new();

    // Rows differ along the axes the rule generalises over: the name's shape (single
    // character, digits, hyphens, non-ASCII-free upper bound) and the signed region.
    // `TrialComponentName` is `len 1..120` (smithy_query operation=CreateTrialComponent),
    // so one row sits on each published bound of that length range.
    let cases: Vec<(&str, String, &str)> = vec![
        // The lower bound of `ExperimentEntityName`'s `len 1..120`.
        ("length-lower-bound", name_of_length(1), "us-west-2"),
        ("digits", "component9000".to_string(), "us-east-1"),
        ("hyphens", "demo-trial-component-one".to_string(), "eu-west-3"),
        ("mixed-case", "DemoComponentTwo".to_string(), "ap-southeast-2"),
        // The upper bound of the same range.
        ("length-upper-bound", name_of_length(120), "ca-central-1"),
    ];

    for (case, name, region) in &cases {
        let (status, created) = emulator
            .call_in(region, "CreateTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from CreateTrialComponent, got {status} with {created:?}"
        );
        assert_eq!(
            str_field(&created, "TrialComponentArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{KIND}/{name}"),
            "case {case}: TrialComponentArn, got {created:?}"
        );

        // The describe replays the very ARN the create minted, so the two agree.
        let described = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            str_field(&created, "TrialComponentArn"),
            "case {case}: DescribeTrialComponent must replay the minted ARN, got {described:?}"
        );
    }

    // An unsigned request carries no credential scope, so the ARN falls back to the
    // emulator's default region.
    let (status, created) = emulator
        .call_unsigned(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "unsigned-component" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {created:?}"
    );
    assert_eq!(
        str_field(&created, "TrialComponentArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{KIND}/unsigned-component"),
        "case unsigned: TrialComponentArn, got {created:?}"
    );
}

/// Every member `DescribeTrialComponent` must replay unchanged from the create request.
/// `Tags` is stored but is not part of the describe's modelled output, `DisplayName` has
/// a documented default, the two timestamps are compared numerically (the JSON
/// encoder may render `1700000000.0` as an integer), and the three collections the
/// component OWNS are reported whatever the create carried (see [`HELD_COLLECTIONS`]), so
/// those are asserted separately.
const ROUND_TRIPPED: [&str; 2] = ["Status", "MetadataProperties"];

/// The collections a trial component HOLDS: reported as the map the component holds, which
/// is the EMPTY map for a component created without them rather than a missing member.
const HELD_COLLECTIONS: [&str; 3] = ["Parameters", "InputArtifacts", "OutputArtifacts"];

/// The create requests the round-trip rule is asserted over.
///
/// Rows differ along what the rule generalises over: which optional members are present,
/// the arm of the `TrialComponentParameterValue` union, the magnitude and precision of a
/// numeric parameter, map size (empty, one entry, several), the presence of an artifact's
/// optional `MediaType`, timestamp precision (whole seconds, milliseconds, the epoch
/// itself, a far-future instant) and the character set of keys and values.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "name-only",
            json!({ "TrialComponentName": "rt-name-only" }),
        ),
        (
            "status-in-progress",
            json!({
                "TrialComponentName": "rt-status-in-progress",
                "DisplayName": "Preprocessing",
                // `TrialComponentPrimaryStatus`'s documented states are InProgress,
                // Completed and Failed (CreateTrialComponent's `Status` doc).
                "Status": { "PrimaryStatus": "InProgress" },
            }),
        ),
        (
            "status-completed-with-message",
            json!({
                "TrialComponentName": "rt-status-completed",
                "Status": { "PrimaryStatus": "Completed", "Message": "all shards written" },
            }),
        ),
        (
            "status-failed-with-message",
            json!({
                "TrialComponentName": "rt-status-failed",
                "Status": { "PrimaryStatus": "Failed", "Message": "exit code 137" },
            }),
        ),
        (
            "timestamps-whole-seconds",
            json!({
                "TrialComponentName": "rt-timestamps-seconds",
                "StartTime": 1_700_000_000.0,
                "EndTime": 1_700_003_600.0,
            }),
        ),
        (
            "timestamps-millisecond-precision",
            json!({
                "TrialComponentName": "rt-timestamps-millis",
                "StartTime": 1_700_000_000.123_f64,
                "EndTime": 1_700_003_600.456_f64,
            }),
        ),
        (
            "timestamps-at-the-epoch-and-far-future",
            json!({
                "TrialComponentName": "rt-timestamps-edges",
                "StartTime": 0.0,
                "EndTime": 4_102_444_800.0,
            }),
        ),
        (
            "parameters-empty-map",
            json!({
                "TrialComponentName": "rt-parameters-empty",
                "Parameters": {},
            }),
        ),
        (
            "parameters-string-arm",
            json!({
                "TrialComponentName": "rt-parameters-string",
                "Parameters": { "optimizer": { "StringValue": "adam" } },
            }),
        ),
        (
            "parameters-number-arm",
            json!({
                "TrialComponentName": "rt-parameters-number",
                "Parameters": {
                    "epochs": { "NumberValue": 10.0 },
                    "learning-rate": { "NumberValue": 0.000_123_456_789_012_345_f64 },
                    "negative-bias": { "NumberValue": -273.15_f64 },
                    "large-batch": { "NumberValue": 1_000_000_000_000_000.0_f64 },
                    // 15 significant digits: the widest double the awsJson number
                    // encoding renders exactly, so the row pins precision rather than
                    // the encoder's rounding.
                    "precise-mantissa": { "NumberValue": 1_234.567_890_123_45_f64 },
                },
            }),
        ),
        (
            "parameters-both-arms-many-entries",
            json!({
                "TrialComponentName": "rt-parameters-mixed",
                "Parameters": {
                    "optimizer": { "StringValue": "sgd" },
                    "momentum": { "NumberValue": 0.9_f64 },
                    "schedule": { "StringValue": "cosine" },
                    "warmup-steps": { "NumberValue": 500.0 },
                },
            }),
        ),
        (
            "non-ascii-keys-and-values",
            json!({
                "TrialComponentName": "rt-unicode",
                "Parameters": {
                    "モデル": { "StringValue": "変換器 ✅" },
                    "tamaño-del-lote": { "NumberValue": 32.0 },
                },
                "InputArtifacts": {
                    "données": { "Value": "s3://bucket/données/ジョブ.csv" },
                },
                "MetadataProperties": { "GeneratedBy": "ユーザー-1 ✨" },
            }),
        ),
        (
            "artifacts-with-and-without-media-type",
            json!({
                "TrialComponentName": "rt-artifacts",
                "InputArtifacts": {
                    "dataset": { "Value": "s3://bucket/train.csv", "MediaType": "text/csv" },
                    "code": { "Value": "s3://bucket/train.py" },
                },
                "OutputArtifacts": {
                    "model": { "Value": "s3://bucket/model.tar.gz", "MediaType": "application/gzip" },
                },
            }),
        ),
        (
            "metadata-properties-every-member",
            json!({
                "TrialComponentName": "rt-metadata-full",
                "MetadataProperties": {
                    "CommitId": "0123456789abcdef0123456789abcdef01234567",
                    "Repository": "https://git.example.com/team/project",
                    "GeneratedBy": "pipeline-executor",
                    "ProjectId": "p-abcdef123456",
                },
            }),
        ),
        (
            "every-member-at-once",
            json!({
                "TrialComponentName": "rt-everything",
                "DisplayName": "Training-step-7",
                "Status": { "PrimaryStatus": "Completed", "Message": "converged" },
                "StartTime": 1_699_999_999.5_f64,
                "EndTime": 1_700_000_000.75_f64,
                "Parameters": {
                    "optimizer": { "StringValue": "adamw" },
                    "epochs": { "NumberValue": 3.0 },
                },
                "InputArtifacts": {
                    "dataset": { "Value": "s3://bucket/train.csv", "MediaType": "text/csv" },
                },
                "OutputArtifacts": {
                    "metrics": { "Value": "s3://bucket/metrics.json", "MediaType": "application/json" },
                },
                "MetadataProperties": { "ProjectId": "p-000000000001" },
                "Tags": [
                    { "Key": "team", "Value": "research" },
                    { "Key": "cost-centre", "Value": "4711" },
                ],
            }),
        ),
    ]
}

/// RULE: `DescribeTrialComponent` replays every member `CreateTrialComponent` stored —
/// present members and absent ones alike — reports `DisplayName` as the stored name when
/// the create did not give one, and carries the creation and last-modified instants the
/// create recorded.
#[tokio::test]
async fn describe_trial_component_replays_exactly_what_create_stored() {
    let emulator = Emulator::new();

    for (case, body) in round_trip_cases() {
        let name = str_field(&body, "TrialComponentName");
        let created = emulator.ok("CreateTrialComponent", &body).await;
        let described = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;

        assert_eq!(
            described.get("TrialComponentName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case}: TrialComponentName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            str_field(&created, "TrialComponentArn"),
            "case {case}: TrialComponentArn, got {described:?}"
        );

        for member in ROUND_TRIPPED {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }
        for member in HELD_COLLECTIONS {
            assert_eq!(
                described.get(member),
                Some(&body.get(member).cloned().unwrap_or_else(|| json!({}))),
                "case {case}: {member} must be the map the component holds, \
                 got {described:?}"
            );
        }

        // A wire timestamp is a JSON number of epoch seconds, so an instant on a whole
        // second may come back as `1700000000` where the request said `1700000000.0`:
        // the VALUE must round-trip, not its JSON spelling.
        for member in ["StartTime", "EndTime"] {
            assert_eq!(
                described.get(member).and_then(Value::as_f64),
                body.get(member).and_then(Value::as_f64),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }

        // `DisplayName`'s doc: "If DisplayName isn't specified, TrialComponentName is
        // displayed", so an absent one reads back as the component's name.
        let expected_display = body
            .get("DisplayName")
            .and_then(Value::as_str)
            .unwrap_or(name.as_str());
        assert_eq!(
            described.get("DisplayName").and_then(Value::as_str),
            Some(expected_display),
            "case {case}: DisplayName, got {described:?}"
        );

        let created_at = described.get("CreationTime").and_then(Value::as_f64);
        let modified_at = described.get("LastModifiedTime").and_then(Value::as_f64);
        assert!(
            created_at.is_some_and(|at| at > 0.0),
            "case {case}: CreationTime must be the stored instant, got {described:?}"
        );
        assert_eq!(
            modified_at, created_at,
            "case {case}: LastModifiedTime must equal CreationTime until something \
             modifies the component, got {described:?}"
        );

        // Nothing has produced a source job or a metric for a component created through
        // this API, so the emulator invents none: the single `Source` is absent, and the
        // LISTS of sources and metrics are empty — the component holds no entry in either.
        assert!(
            described.get("Source").is_none(),
            "case {case}: Source must be absent for an API-created component, \
             got {described:?}"
        );
        for member in ["Sources", "Metrics"] {
            assert_eq!(
                described.get(member),
                Some(&json!([])),
                "case {case}: {member} must be the empty list for an API-created \
                 component, got {described:?}"
            );
        }

        // A trial component IS a lineage entity, so it belongs to the default lineage
        // group of the Region it was created in — the group the create materialised there,
        // the same membership `DescribeArtifact` and `DescribeAction` report.
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/{}",
                    default_lineage_group("us-east-1")
                )
                .as_str()
            ),
            "case {case}: LineageGroupArn, got {described:?}"
        );

        // `CreatedBy` and `LastModifiedBy` name the IAM identity the request signed with;
        // nothing has modified the component, so the two agree.
        for member in ["CreatedBy", "LastModifiedBy"] {
            let identity = described
                .get(member)
                .unwrap_or_else(|| panic!("case {case}: {member}, got {described:?}"));
            assert_eq!(
                identity
                    .get("IamIdentity")
                    .and_then(|iam| iam.get("Arn"))
                    .and_then(Value::as_str),
                Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
                "case {case}: {member}.IamIdentity.Arn, got {identity:?}"
            );
        }
        assert_eq!(
            described.get("LastModifiedBy"),
            described.get("CreatedBy"),
            "case {case}: an unmodified component's last modifier is its creator, got \
             {described:?}"
        );
    }
}

/// RULE: a timestamp a caller sends is stored and replayed exactly, to the millisecond
/// the wire carries — the emulator never rounds it to a whole second.
#[tokio::test]
async fn trial_component_timestamps_keep_the_precision_the_request_carried() {
    let emulator = Emulator::new();

    // Rows differ along the sub-second part: none, a half second, one, two and three
    // decimal digits of milliseconds, and the epoch itself.
    let cases: [(&str, f64); 6] = [
        ("whole-second", 1_700_000_000.0),
        ("half-second", 1_700_000_000.5),
        ("one-decimal", 1_700_000_000.1),
        ("two-decimals", 1_700_000_000.12),
        ("three-decimals", 1_700_000_000.123),
        ("the-epoch", 0.0),
    ];

    for (case, instant) in cases {
        let name = format!("precision-{case}");
        emulator
            .ok(
                "CreateTrialComponent",
                &json!({
                    "TrialComponentName": name,
                    "StartTime": instant,
                    "EndTime": instant + 1.0,
                }),
            )
            .await;
        let described = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;

        for (member, expected) in [("StartTime", instant), ("EndTime", instant + 1.0)] {
            let actual = described
                .get(member)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case}: {member} missing from {described:?}"));
            // Whole milliseconds must match exactly: that is the precision the wire
            // timestamp carries for these instants.
            assert_eq!(
                (actual * 1_000.0).round() as i64,
                (expected * 1_000.0).round() as i64,
                "case {case}: {member} must keep millisecond precision, expected \
                 {expected}, got {actual}"
            );
            assert!(
                (actual - expected).abs() < 1e-6,
                "case {case}: {member} expected {expected}, got {actual}"
            );
        }
    }
}

/// RULE: a reader whose name member is modelled `ExperimentEntityNameOrArn` accepts the
/// entity's name or its ARN, and both forms answer with the same stored component.
#[tokio::test]
async fn describe_trial_component_accepts_the_name_or_the_arn() {
    let emulator = Emulator::new();

    // Rows differ along the name shape, so the resolution is not keyed to one spelling.
    for name in ["arn-lookup-one", "a", &name_of_length(120)] {
        let created = emulator
            .ok("CreateTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        let arn = str_field(&created, "TrialComponentArn");

        let by_name = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        let by_arn = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": arn }))
            .await;
        assert_eq!(
            by_arn, by_name,
            "case {name}: describing by ARN must answer exactly as describing by name, \
             got {by_arn:?}"
        );
    }
}

/// RULE: an `ExperimentEntityName` "is not case-sensitive" (`TrialComponentName`'s model
/// doc), so every spelling of one name denotes the one stored component: a reader
/// resolves it whatever the case, a create under another spelling is refused as a
/// duplicate, and the component replays the casing its create carried.
#[tokio::test]
async fn trial_component_names_are_not_case_sensitive() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "Mixed-Case-Component" }),
        )
        .await;
    let arn = str_field(&created, "TrialComponentArn");

    // Rows differ along the case of each character class the name pattern allows: as
    // created, fully lowered, fully raised, and alternating.
    let spellings = [
        "Mixed-Case-Component",
        "mixed-case-component",
        "MIXED-CASE-COMPONENT",
        "mIXED-cASE-cOMPONENT",
    ];

    for spelling in spellings {
        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": spelling }),
            )
            .await;
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            arn,
            "case {spelling}: every spelling must resolve to the one stored component, \
             got {described:?}"
        );
        // The stored record keeps the casing the create carried, so the read replays it
        // rather than the spelling the reader asked with.
        assert_eq!(
            str_field(&described, "TrialComponentName"),
            "Mixed-Case-Component",
            "case {spelling}: the stored name's casing must be replayed, got {described:?}"
        );

        // The ARN form resolves the same way, whatever the case of its name segment.
        let by_arn = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({
                    "TrialComponentName":
                        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{KIND}/{spelling}"),
                }),
            )
            .await;
        assert_eq!(
            str_field(&by_arn, "TrialComponentArn"),
            arn,
            "case {spelling}: the ARN form must resolve case-insensitively too, got \
             {by_arn:?}"
        );

        // And a create under that spelling is the same name, so it is refused.
        let (status, refused) = emulator
            .call(
                "CreateTrialComponent",
                &json!({ "TrialComponentName": spelling }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {spelling}: a create differing only in case is a duplicate, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {spelling}: error code, got {refused:?}"
        );
    }
}

/// RULE: a read never changes what a later caller sees — two describes of the same
/// component answer identically.
#[tokio::test]
async fn describing_a_trial_component_twice_answers_identically() {
    let emulator = Emulator::new();

    for (case, body) in round_trip_cases() {
        let name = str_field(&body, "TrialComponentName");
        emulator.ok("CreateTrialComponent", &body).await;
        let first = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        let second = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": name }))
            .await;
        assert_eq!(
            second, first,
            "case {case}: a second describe must answer identically, got {second:?}"
        );
    }
}

/// RULE: `TrialComponentName` is unique in the account, so a create naming a component
/// that already exists is refused with HTTP 400 `ValidationException` and leaves the
/// stored component exactly as it was.
#[tokio::test]
async fn create_trial_component_refuses_a_name_already_in_use() {
    let emulator = Emulator::new();

    // Rows differ along what the second request changes: nothing, the display name, and
    // a whole different body — none of them may overwrite the stored component.
    let cases: Vec<(&str, Value)> = vec![
        ("identical-request", json!({ "TrialComponentName": "taken-name" })),
        (
            "different-display-name",
            json!({ "TrialComponentName": "taken-name", "DisplayName": "Second" }),
        ),
        (
            "different-everything",
            json!({
                "TrialComponentName": "taken-name",
                "Status": { "PrimaryStatus": "Failed", "Message": "nope" },
                "Parameters": { "epochs": { "NumberValue": 99.0 } },
            }),
        ),
    ];

    let first = emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "taken-name", "DisplayName": "First" }),
        )
        .await;

    for (case, body) in cases {
        let (status, refused) = emulator.call("CreateTrialComponent", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with HTTP 400, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: error code, got {refused:?}"
        );

        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": "taken-name" }),
            )
            .await;
        assert_eq!(
            str_field(&described, "DisplayName"),
            "First",
            "case {case}: the stored component must survive the refused create, got \
             {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            str_field(&first, "TrialComponentArn"),
            "case {case}: the stored ARN must not change, got {described:?}"
        );
        assert!(
            described.get("Status").is_none(),
            "case {case}: the refused create must not write members, got {described:?}"
        );
    }
}

/// RULE: `DescribeTrialComponent` naming a component that is not in state is refused with
/// the modelled `ResourceNotFound` (HTTP 400) — never a 200 with empty members, never a
/// 501 — and the failed read creates nothing.
#[tokio::test]
async fn describe_trial_component_refuses_a_component_that_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "the-one-that-exists" }),
        )
        .await;

    // Rows differ along the form of the missing identifier: a bare name, the ARN form,
    // the longest name the model allows, and ARNs that are well formed but denote some
    // OTHER entity — another resource kind, account, region or service — none of which
    // may resolve to the component that happens to share the ARN's trailing segment.
    let existing = "the-one-that-exists";
    let cases: Vec<(&str, String)> = vec![
        ("bare-name", "never-created".to_string()),
        (
            "arn-form",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{KIND}/never-created"),
        ),
        ("length-upper-bound", name_of_length(120)),
        (
            // `ExperimentEntityNameOrArn`'s pattern admits the experiment, trial,
            // artifact, action and context kinds too; none of them is a trial component.
            "wrong-resource-kind-arn",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment-trial/{existing}"),
        ),
        (
            "wrong-account-arn",
            format!("arn:aws:sagemaker:us-east-1:999999999999:{KIND}/{existing}"),
        ),
        (
            "wrong-region-arn",
            format!("arn:aws:sagemaker:eu-west-3:{ACCOUNT}:{KIND}/{existing}"),
        ),
    ];

    for (case, identifier) in cases {
        let (status, refused) = emulator
            .call(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": identifier }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: error code, got {refused:?}"
        );
    }

    // An ARN of another SERVICE does not even satisfy `ExperimentEntityNameOrArn`'s
    // pattern (`^(arn:aws[a-z\-]*:sagemaker:...)?`), so the boundary validator refuses it
    // before the handler runs — 400, and never a 200 for the component whose name the ARN
    // ends with.
    let (status, refused) = emulator
        .call(
            "DescribeTrialComponent",
            &json!({
                "TrialComponentName": format!("arn:aws:s3:us-east-1:{ACCOUNT}:{KIND}/{existing}"),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case wrong-service-arn: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case wrong-service-arn: error code, got {refused:?}"
    );

    // The component that does exist is untouched by the refused reads, and no record
    // appeared for the names they named.
    let described = emulator
        .ok(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "the-one-that-exists" }),
        )
        .await;
    assert_eq!(
        str_field(&described, "TrialComponentName"),
        "the-one-that-exists",
        "the existing component must still read back, got {described:?}"
    );
    let (status, refused) = emulator
        .call(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "never-created" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a refused read must not have created the component it named, got {status} with \
         {refused:?}"
    );
}

/// RULE: a trial component is a lineage entity, so it belongs to the default lineage group
/// of the Region it was CREATED in — the group that create materialised there, which
/// `DescribeLineageGroup` reports — and that membership does not follow whichever Region a
/// later reader signs for.
#[tokio::test]
async fn a_trial_component_belongs_to_its_region_s_default_lineage_group() {
    // Rows differ along the Region the create signed for, including the emulator default
    // and Regions in three other partitions' worth of prefixes, so the reported group is
    // the component's own rather than a constant.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2", "ca-central-1"] {
        let emulator = Emulator::new();
        let name = format!("component-in-{region}");
        let group_name = default_lineage_group(region);
        let expected =
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{group_name}");

        let (created_status, created) = emulator
            .call_in(
                region,
                "CreateTrialComponent",
                &json!({ "TrialComponentName": &name }),
            )
            .await;
        assert_eq!(
            created_status, 200,
            "case {region}: CreateTrialComponent, got {created_status} with {created}"
        );

        // Read in the creating Region: the component reports its Region's group.
        let (described_status, described) = emulator
            .call_in(
                region,
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": &name }),
            )
            .await;
        assert_eq!(
            described_status, 200,
            "case {region}: DescribeTrialComponent, got {described_status} with {described}"
        );
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region}: LineageGroupArn, got {described:?}"
        );

        // And the group the create materialised is reportable in that Region, so the ARN
        // the component carries names a group that really exists.
        let (group_status, group) = emulator
            .call_in(
                region,
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": &group_name }),
            )
            .await;
        assert_eq!(
            group_status, 200,
            "case {region}: DescribeLineageGroup, got {group_status} with {group}"
        );
        assert_eq!(
            group.get("LineageGroupArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region}: the group's own ARN, got {group:?}"
        );

        // A reader signing for another Region resolves the component by its account-wide
        // name and still gets the CREATING Region's group, not its own.
        let elsewhere = if region == "us-east-1" {
            "eu-central-1"
        } else {
            "us-east-1"
        };
        let (elsewhere_status, from_elsewhere) = emulator
            .call_in(
                elsewhere,
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": &name }),
            )
            .await;
        assert_eq!(
            elsewhere_status, 200,
            "case {region}: a read from {elsewhere}, got {elsewhere_status} with \
             {from_elsewhere}"
        );
        assert_eq!(
            from_elsewhere.get("LineageGroupArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region}: a read from {elsewhere} must report the creating Region's \
             group, got {from_elsewhere:?}"
        );
    }
}
