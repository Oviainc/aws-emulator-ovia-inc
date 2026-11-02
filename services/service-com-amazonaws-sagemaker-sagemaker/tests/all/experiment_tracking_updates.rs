// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the experiment-tracking updates: `UpdateExperiment`,
//! `UpdateTrial` and `UpdateTrialComponent`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The RULE these tests assert, over a TABLE of inputs rather than one recorded call: an
//! update APPLIES the members its request carries to the stored resource and leaves every
//! other stored value alone, so the matching `Describe*` — and the `ListTrialComponents`
//! projection that reads the same record — reports the updated values, and the resource's
//! `LastModifiedTime`/`LastModifiedBy` move with the call. A name no resource is stored
//! under is refused with the modelled `ResourceNotFound` and writes nothing.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sagemaker/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create the experiment `name`, and a trial under it when `trial` is given.
    async fn seed_experiment(&self, name: &str, extra: &Value) -> Value {
        let mut body = json!({ "ExperimentName": name });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        self.ok("CreateExperiment", &body).await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

fn f64_field(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member} in {body}"))
}

/// A name of exactly `len` characters inside the modelled `ExperimentEntityName` pattern
/// (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`), so a case can sit on the published maximum of
/// 120 (`smithy_query(query="operation",operation="UpdateExperiment")`: `DisplayName`
/// len 1..120).
fn name_of_len(len: usize) -> String {
    "n".repeat(len)
}

/// A description of exactly `len` characters, for a case on the published maximum of 3072
/// (`smithy_query(query="operation",operation="UpdateExperiment")`: `Description`
/// len 0..3072).
fn text_of_len(len: usize) -> String {
    "d".repeat(len)
}

/// A `Parameters` map of `count` entries, for a case that differs along the ENTRY COUNT.
fn parameters_of(count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| (format!("p{index}"), json!({ "StringValue": format!("v{index}") })))
        .collect();
    Value::Object(map)
}

/// An artifact map of `count` entries, keyed `a0..`, for a count-varying case.
fn artifacts_of(count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| {
            (
                format!("a{index}"),
                json!({ "Value": format!("s3://bucket/{index}") }),
            )
        })
        .collect();
    Value::Object(map)
}

/// One row of an update table: the create body's extra members, the update to send, and
/// the members a following describe must report.
struct Row {
    case: &'static str,
    /// Extra members the create carries beside the resource's name.
    created: Value,
    /// The update body, without the name the test fills in.
    update: Value,
    /// Members a `Describe*` must report after the update, compared one by one.
    expected: Value,
}

/// RULE: `UpdateExperiment` applies the members the request carries and leaves the rest,
/// so `DescribeExperiment` reports the updated `DisplayName`/`Description`.
#[tokio::test]
async fn update_experiment_applies_the_members_the_request_carries() {
    let long_name = name_of_len(120);
    let long_description = text_of_len(3072);
    // Rows differ along what the rule generalises over: which members the update carries,
    // whether the create had set them, and the length of the values (one character, the
    // empty description the model's `len 0..3072` admits, the published maxima).
    let rows = vec![
        Row {
            case: "display-name-and-description-together",
            created: json!({ "DisplayName": "Initial", "Description": "initial text" }),
            update: json!({ "DisplayName": "Renamed", "Description": "updated text" }),
            expected: json!({ "DisplayName": "Renamed", "Description": "updated text" }),
        },
        Row {
            case: "display-name-only-keeps-the-description",
            created: json!({ "DisplayName": "Initial", "Description": "initial text" }),
            update: json!({ "DisplayName": "Renamed" }),
            expected: json!({ "DisplayName": "Renamed", "Description": "initial text" }),
        },
        Row {
            case: "description-only-keeps-the-display-name",
            created: json!({ "DisplayName": "Initial", "Description": "initial text" }),
            update: json!({ "Description": "updated text" }),
            expected: json!({ "DisplayName": "Initial", "Description": "updated text" }),
        },
        Row {
            case: "description-added-to-an-experiment-that-had-none",
            created: json!({}),
            update: json!({ "Description": "first description" }),
            // With no `DisplayName` at create the name is displayed, and the update did
            // not change that.
            expected: json!({
                "DisplayName": "description-added-to-an-experiment-that-had-none",
                "Description": "first description",
            }),
        },
        Row {
            case: "empty-description-is-a-value-the-model-admits",
            created: json!({ "Description": "initial text" }),
            update: json!({ "Description": "" }),
            expected: json!({ "Description": "" }),
        },
        Row {
            case: "members-on-their-published-maxima",
            created: json!({ "DisplayName": "Initial", "Description": "initial text" }),
            update: json!({ "DisplayName": long_name, "Description": long_description }),
            expected: json!({ "DisplayName": long_name, "Description": long_description }),
        },
        Row {
            case: "an-update-carrying-no-editable-member-changes-no-value",
            created: json!({ "DisplayName": "Initial", "Description": "initial text" }),
            update: json!({}),
            expected: json!({ "DisplayName": "Initial", "Description": "initial text" }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let created = emulator.seed_experiment(case, &row.created).await;
        let before = emulator
            .ok("DescribeExperiment", &json!({ "ExperimentName": case }))
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("ExperimentName".to_string(), json!(case));
        let updated = emulator
            .ok("UpdateExperiment", &Value::Object(update))
            .await;
        assert_eq!(
            updated.get("ExperimentArn"),
            created.get("ExperimentArn"),
            "case {case:?}: the output carries the experiment's own ARN, got {updated:?}"
        );
        assert_eq!(
            updated.as_object().map(Map::len),
            Some(1),
            "case {case:?}: the output models ExperimentArn alone, got {updated:?}"
        );

        // Round trip: the update is read back through the implemented reader.
        let described = emulator
            .ok("DescribeExperiment", &json!({ "ExperimentName": case }))
            .await;
        for (member, expected) in row.expected.as_object().expect("an object of members") {
            assert_eq!(
                described.get(member),
                Some(expected),
                "case {case:?}: {member} after the update, got {described:?}"
            );
        }
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
        // The identity behind the update is who last modified the experiment.
        assert_eq!(
            described
                .get("LastModifiedBy")
                .and_then(|identity| identity.get("IamIdentity"))
                .and_then(|iam| iam.get("Arn"))
                .and_then(Value::as_str),
            Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
            "case {case:?}: LastModifiedBy, got {described:?}"
        );
    }
}

/// RULE: `UpdateTrial` applies the `DisplayName` the request carries, and the trial's
/// other stored values — its experiment, its metadata properties, its creation instant —
/// stay exactly as the create left them.
#[tokio::test]
async fn update_trial_applies_the_display_name_and_leaves_the_rest() {
    let long_name = name_of_len(120);
    // Rows differ along whether the create set a display name, the length of the new one,
    // and whether the update carries one at all.
    let rows: Vec<(&str, Value, Value, Option<String>)> = vec![
        (
            "renaming-a-trial-that-had-a-display-name",
            json!({ "DisplayName": "Initial" }),
            json!({ "DisplayName": "Renamed" }),
            Some("Renamed".to_string()),
        ),
        (
            "naming-a-trial-that-had-none",
            json!({}),
            json!({ "DisplayName": "Named" }),
            Some("Named".to_string()),
        ),
        (
            "renaming-to-the-published-maximum-length",
            json!({ "DisplayName": "Initial" }),
            json!({ "DisplayName": long_name.clone() }),
            Some(long_name.clone()),
        ),
        (
            "renaming-to-a-single-character",
            json!({ "DisplayName": "Initial" }),
            json!({ "DisplayName": "x" }),
            Some("x".to_string()),
        ),
        (
            "an-update-carrying-no-display-name-keeps-the-stored-one",
            json!({ "DisplayName": "Initial" }),
            json!({}),
            Some("Initial".to_string()),
        ),
        (
            "an-update-carrying-nothing-on-a-trial-that-has-no-display-name",
            json!({}),
            json!({}),
            None,
        ),
    ];

    for (case, created, update, expected_display) in &rows {
        let emulator = Emulator::new();
        emulator.seed_experiment("holder", &json!({})).await;
        let mut body = json!({ "TrialName": case, "ExperimentName": "holder" });
        for (key, value) in created.as_object().expect("an object of extra members") {
            body[key] = value.clone();
        }
        body["MetadataProperties"] = json!({ "CommitId": "0123456789abcdef" });
        let seeded = emulator.ok("CreateTrial", &body).await;
        let before = emulator
            .ok("DescribeTrial", &json!({ "TrialName": case }))
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut sent = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        sent.insert("TrialName".to_string(), json!(case));
        let updated = emulator.ok("UpdateTrial", &Value::Object(sent)).await;
        assert_eq!(
            updated.get("TrialArn"),
            seeded.get("TrialArn"),
            "case {case:?}: the output carries the trial's own ARN, got {updated:?}"
        );

        let described = emulator
            .ok("DescribeTrial", &json!({ "TrialName": case }))
            .await;
        assert_eq!(
            described.get("DisplayName").and_then(Value::as_str),
            expected_display.as_deref(),
            "case {case:?}: DisplayName after the update, got {described:?}"
        );
        assert_eq!(
            described.get("ExperimentName").and_then(Value::as_str),
            Some("holder"),
            "case {case:?}: the owning experiment never moves, got {described:?}"
        );
        assert_eq!(
            described.get("MetadataProperties"),
            Some(&json!({ "CommitId": "0123456789abcdef" })),
            "case {case:?}: MetadataProperties are the create's, got {described:?}"
        );
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
        assert_eq!(
            described
                .get("LastModifiedBy")
                .and_then(|identity| identity.get("IamIdentity"))
                .and_then(|iam| iam.get("Arn"))
                .and_then(Value::as_str),
            Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
            "case {case:?}: LastModifiedBy names the identity behind the update, \
             got {described:?}"
        );
    }
}

/// RULE: `UpdateTrialComponent` replaces the status, the display name and the two instants
/// the request carries, UPSERTS the entries its keyed collections carry ("existing input
/// artifacts are replaced if the trial component is updated with an identical input
/// artifact key" — smithy_query doc=UpdateTrialComponentRequest$InputArtifacts) and then
/// drops the keys its removal lists name.
#[tokio::test]
async fn update_trial_component_applies_each_member_the_request_carries() {
    // Rows differ along which members the update carries, the direction of each collection
    // change (add, replace one entry, replace all, prune, prune a key that is not there),
    // the entry count, and the status members.
    let rows = vec![
        Row {
            case: "status-and-display-name",
            created: json!({
                "DisplayName": "Initial",
                "Status": { "PrimaryStatus": "InProgress", "Message": "running" },
            }),
            update: json!({
                "DisplayName": "Done",
                "Status": { "PrimaryStatus": "Completed", "Message": "all shards written" },
            }),
            expected: json!({
                "DisplayName": "Done",
                "Status": { "PrimaryStatus": "Completed", "Message": "all shards written" },
            }),
        },
        Row {
            case: "a-status-replaces-the-stored-one-whole",
            created: json!({
                "Status": { "PrimaryStatus": "InProgress", "Message": "running" },
            }),
            update: json!({ "Status": { "PrimaryStatus": "Failed" } }),
            // The member is "the new status of the component", so the message the stored
            // status carried is gone rather than merged into the new one.
            expected: json!({ "Status": { "PrimaryStatus": "Failed" } }),
        },
        Row {
            case: "parameters-upsert-one-key-and-keep-the-others",
            created: json!({
                "Parameters": {
                    "epochs": { "NumberValue": 10.0 },
                    "optimizer": { "StringValue": "sgd" },
                },
            }),
            update: json!({ "Parameters": { "optimizer": { "StringValue": "adam" } } }),
            expected: json!({
                "Parameters": {
                    "epochs": { "NumberValue": 10.0 },
                    "optimizer": { "StringValue": "adam" },
                },
            }),
        },
        Row {
            case: "parameters-to-remove-drops-a-subset",
            created: json!({
                "Parameters": {
                    "epochs": { "NumberValue": 10.0 },
                    "optimizer": { "StringValue": "sgd" },
                },
            }),
            update: json!({ "ParametersToRemove": ["epochs"] }),
            expected: json!({ "Parameters": { "optimizer": { "StringValue": "sgd" } } }),
        },
        Row {
            case: "a-key-named-by-both-the-map-and-the-removal-list-ends-up-removed",
            created: json!({ "Parameters": { "epochs": { "NumberValue": 1.0 } } }),
            update: json!({
                "Parameters": { "optimizer": { "StringValue": "adam" } },
                "ParametersToRemove": ["optimizer"],
            }),
            expected: json!({ "Parameters": { "epochs": { "NumberValue": 1.0 } } }),
        },
        Row {
            case: "input-artifacts-added-to-a-component-that-held-none",
            created: json!({}),
            update: json!({ "InputArtifacts": { "train": { "Value": "s3://bucket/train" } } }),
            expected: json!({
                "InputArtifacts": { "train": { "Value": "s3://bucket/train" } },
                "OutputArtifacts": {},
                "Parameters": {},
            }),
        },
        Row {
            case: "output-artifacts-upserted-beside-input-artifacts",
            created: json!({
                "InputArtifacts": { "train": { "Value": "s3://bucket/train" } },
                "OutputArtifacts": { "model": { "Value": "s3://bucket/model" } },
            }),
            update: json!({
                "InputArtifacts": {
                    "train": { "Value": "s3://bucket/train-v2", "MediaType": "text/csv" },
                },
                "OutputArtifacts": { "report": { "Value": "s3://bucket/report" } },
                "OutputArtifactsToRemove": ["model"],
            }),
            expected: json!({
                "InputArtifacts": {
                    "train": { "Value": "s3://bucket/train-v2", "MediaType": "text/csv" },
                },
                "OutputArtifacts": { "report": { "Value": "s3://bucket/report" } },
            }),
        },
        Row {
            case: "removing-from-a-collection-that-holds-nothing-leaves-it-empty",
            created: json!({}),
            update: json!({ "InputArtifactsToRemove": ["train"] }),
            expected: json!({ "InputArtifacts": {} }),
        },
        Row {
            case: "the-two-instants-are-replaced-when-sent",
            created: json!({ "StartTime": 1_700_000_000.0, "EndTime": 1_700_003_600.0 }),
            update: json!({ "StartTime": 1_700_000_500.0, "EndTime": 1_700_007_200.0 }),
            expected: json!({ "StartTime": 1_700_000_500.0, "EndTime": 1_700_007_200.0 }),
        },
        Row {
            case: "an-update-carrying-no-editable-member-changes-no-value",
            created: json!({
                "DisplayName": "Initial",
                "Status": { "PrimaryStatus": "InProgress" },
                "Parameters": { "epochs": { "NumberValue": 10.0 } },
            }),
            update: json!({}),
            expected: json!({
                "DisplayName": "Initial",
                "Status": { "PrimaryStatus": "InProgress" },
                "Parameters": { "epochs": { "NumberValue": 10.0 } },
            }),
        },
        Row {
            case: "collections-at-the-modelled-maximum-entry-counts",
            created: json!({}),
            update: json!({
                "Parameters": parameters_of(300),
                "InputArtifacts": artifacts_of(60),
                "OutputArtifacts": artifacts_of(60),
            }),
            expected: json!({
                "Parameters": parameters_of(300),
                "InputArtifacts": artifacts_of(60),
                "OutputArtifacts": artifacts_of(60),
            }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let mut body = json!({ "TrialComponentName": case });
        for (key, value) in row.created.as_object().expect("an object of members") {
            body[key] = value.clone();
        }
        let seeded = emulator.ok("CreateTrialComponent", &body).await;
        let before = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": case }),
            )
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("TrialComponentName".to_string(), json!(case));
        let updated = emulator
            .ok("UpdateTrialComponent", &Value::Object(update))
            .await;
        assert_eq!(
            updated.get("TrialComponentArn"),
            seeded.get("TrialComponentArn"),
            "case {case:?}: the output carries the component's own ARN, got {updated:?}"
        );

        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": case }),
            )
            .await;
        for (member, expected) in row.expected.as_object().expect("an object of members") {
            match expected.as_f64() {
                // A wire timestamp is a JSON number of epoch seconds, so the VALUE is
                // compared rather than its JSON spelling.
                Some(instant) => assert_eq!(
                    described.get(member).and_then(Value::as_f64),
                    Some(instant),
                    "case {case:?}: {member} after the update, got {described:?}"
                ),
                None => assert_eq!(
                    described.get(member),
                    Some(expected),
                    "case {case:?}: {member} after the update, got {described:?}"
                ),
            }
        }
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
    }
}

/// RULE: the `ListTrialComponents` summary projects the SAME stored record the reader
/// does, so an update is visible in the listing as well as in the describe.
#[tokio::test]
async fn list_trial_components_reports_the_updated_record() {
    let emulator = Emulator::new();

    // Two components, so the listing has a neighbour that must stay as it was.
    for name in ["updated-component", "bystander-component"] {
        emulator
            .ok(
                "CreateTrialComponent",
                &json!({
                    "TrialComponentName": name,
                    "DisplayName": "Initial",
                    "Status": { "PrimaryStatus": "InProgress", "Message": "running" },
                }),
            )
            .await;
    }

    emulator
        .ok(
            "UpdateTrialComponent",
            &json!({
                "TrialComponentName": "updated-component",
                "DisplayName": "Done",
                "Status": { "PrimaryStatus": "Completed", "Message": "finished" },
            }),
        )
        .await;

    let listed = emulator
        .ok("ListTrialComponents", &json!({ "SortBy": "Name" }))
        .await;
    let summaries = listed
        .get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {listed}"));
    let summary_of = |name: &str| -> Value {
        summaries
            .iter()
            .find(|summary| summary.get("TrialComponentName").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("case {name}: expected a summary in {listed}"))
            .clone()
    };

    let updated = summary_of("updated-component");
    assert_eq!(
        updated.get("DisplayName").and_then(Value::as_str),
        Some("Done"),
        "case updated: the summary carries the updated DisplayName, got {updated:?}"
    );
    assert_eq!(
        updated.get("Status"),
        Some(&json!({ "PrimaryStatus": "Completed", "Message": "finished" })),
        "case updated: the summary carries the updated Status, got {updated:?}"
    );
    let bystander = summary_of("bystander-component");
    assert_eq!(
        bystander.get("DisplayName").and_then(Value::as_str),
        Some("Initial"),
        "case bystander: an untouched component is unchanged, got {bystander:?}"
    );
    assert_eq!(
        bystander.get("Status"),
        Some(&json!({ "PrimaryStatus": "InProgress", "Message": "running" })),
        "case bystander: an untouched component is unchanged, got {bystander:?}"
    );
}

/// RULE: an update whose name no resource is stored under is refused with the modelled
/// `ResourceNotFound` (HTTP 400) and writes nothing — the refused call never mints the
/// resource it could not find.
#[tokio::test]
async fn updating_a_tracking_resource_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    // One of each resource exists, so a refusal below is about the name that misses and
    // not about an empty store.
    emulator.seed_experiment("present", &json!({})).await;
    emulator
        .ok(
            "CreateTrial",
            &json!({ "TrialName": "present", "ExperimentName": "present" }),
        )
        .await;
    emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "present" }),
        )
        .await;

    // Rows differ along the resource kind, how the name misses (never created, a prefix,
    // a suffix) and which editable members the refused update carried.
    let cases: Vec<(&str, &str, &str, Value, &str, &str)> = vec![
        (
            "experiment-never-created",
            "UpdateExperiment",
            "ExperimentName",
            json!({ "DisplayName": "Renamed" }),
            "Experiment",
            "experiment",
        ),
        (
            "experiment-name-prefix",
            "UpdateExperiment",
            "ExperimentName",
            json!({ "Description": "updated" }),
            "Experiment",
            "experiment",
        ),
        (
            "trial-never-created",
            "UpdateTrial",
            "TrialName",
            json!({ "DisplayName": "Renamed" }),
            "Trial",
            "experiment-trial",
        ),
        (
            "trial-no-members",
            "UpdateTrial",
            "TrialName",
            json!({}),
            "Trial",
            "experiment-trial",
        ),
        (
            "component-never-created",
            "UpdateTrialComponent",
            "TrialComponentName",
            json!({ "Status": { "PrimaryStatus": "Completed" } }),
            "Trial component",
            "experiment-trial-component",
        ),
        (
            "component-with-collections",
            "UpdateTrialComponent",
            "TrialComponentName",
            json!({ "InputArtifacts": { "train": { "Value": "s3://bucket/train" } } }),
            "Trial component",
            "experiment-trial-component",
        ),
    ];

    for (case, target, name_member, update, entity, kind) in &cases {
        let missing = format!("absent-{case}");
        let mut body = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert((*name_member).to_string(), json!(missing));
        let (status, parsed) = emulator.call(target, &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name nothing is stored under must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: the refusal must use the operation's modelled not-found error, \
             got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!(
                "{entity} 'arn:aws:sagemaker:us-east-1:{ACCOUNT}:{kind}/{missing}' \
                 does not exist."
            ),
            "case {case:?}: the refusal sentence, got {parsed:?}"
        );

        // The refused update created nothing.
        let reader = target.replace("Update", "Describe");
        let mut read_body = Map::new();
        read_body.insert((*name_member).to_string(), json!(missing));
        let (read_status, read) = emulator.call(&reader, &Value::Object(read_body)).await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused update must mint nothing, got {read_status} {read}"
        );
    }

    // Every resource that does exist is untouched, and still updatable.
    let described = emulator
        .ok("DescribeExperiment", &json!({ "ExperimentName": "present" }))
        .await;
    assert_eq!(
        described.get("DisplayName").and_then(Value::as_str),
        Some("present"),
        "case present: the live experiment must be unchanged, got {described:?}"
    );
    let updated = emulator
        .ok(
            "UpdateExperiment",
            &json!({ "ExperimentName": "present", "DisplayName": "Renamed" }),
        )
        .await;
    assert_eq!(
        str_field(&updated, "ExperimentArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:experiment/present"),
        "case present: the live experiment is still updatable, got {updated:?}"
    );
}

/// RULE: a component's collections are bounded by the maxima the model puts on the members
/// that write them (`InputArtifacts`/`OutputArtifacts` len 0..60, `Parameters` len 0..300),
/// so an upsert whose RESULT would pass the bound is refused and nothing is stored — while
/// one that rewrites entries the component already holds is not growth and is applied.
#[tokio::test]
async fn an_update_cannot_grow_a_component_collection_past_its_modelled_maximum() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateTrialComponent",
            &json!({
                "TrialComponentName": "full",
                "Parameters": parameters_of(300),
                "InputArtifacts": artifacts_of(60),
                "OutputArtifacts": artifacts_of(60),
            }),
        )
        .await;

    // Rewriting every entry a full collection holds leaves it exactly as full, so it is
    // accepted.
    emulator
        .ok(
            "UpdateTrialComponent",
            &json!({
                "TrialComponentName": "full",
                "Parameters": parameters_of(300),
                "InputArtifacts": artifacts_of(60),
            }),
        )
        .await;

    // Rows differ along which collection a new key would overflow, and its bound.
    let cases: Vec<(&str, Value, &str, usize)> = vec![
        (
            "one-parameter-past-the-bound",
            json!({ "Parameters": { "extra": { "StringValue": "x" } } }),
            "Parameters",
            300,
        ),
        (
            "one-input-artifact-past-the-bound",
            json!({ "InputArtifacts": { "extra": { "Value": "s3://bucket/extra" } } }),
            "InputArtifacts",
            60,
        ),
        (
            "one-output-artifact-past-the-bound",
            json!({ "OutputArtifacts": { "extra": { "Value": "s3://bucket/extra" } } }),
            "OutputArtifacts",
            60,
        ),
        (
            "a-removal-does-not-make-room-for-an-entry-past-the-bound",
            json!({
                "InputArtifacts": { "extra": { "Value": "s3://bucket/extra" } },
                "InputArtifactsToRemove": ["a0"],
            }),
            "InputArtifacts",
            60,
        ),
    ];

    for (case, update, member, maximum) in &cases {
        let mut body = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert("TrialComponentName".to_string(), json!("full"));
        let (status, parsed) = emulator
            .call("UpdateTrialComponent", &Value::Object(body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an update past the bound must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(member) && message(&parsed).contains(&maximum.to_string()),
            "case {case:?}: the refusal must name the member and its maximum, got {parsed:?}"
        );

        // Nothing the refused update named was written: the collection still holds
        // exactly what it did.
        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": "full" }),
            )
            .await;
        assert_eq!(
            described
                .get(member)
                .and_then(Value::as_object)
                .map(Map::len),
            Some(*maximum),
            "case {case:?}: {member} must still hold {maximum} entries, got {described:?}"
        );
    }
}
