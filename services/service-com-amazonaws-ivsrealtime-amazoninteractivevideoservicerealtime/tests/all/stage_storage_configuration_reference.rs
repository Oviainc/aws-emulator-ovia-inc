// SPDX-License-Identifier: CC0-1.0

//! The storage configuration a stage's individual-participant recording points at, over
//! the wire.
//!
//! One rule, exercised on both operations that attach such a configuration
//! (`CreateStage`, `UpdateStage`) and over a SET of inputs rather than one request: a
//! non-empty `autoParticipantRecordingConfiguration.storageConfigurationArn` is a
//! REFERENCE to a `StorageConfiguration` of this same service
//! (`smithy_query(query="shape",shape="AutoParticipantRecordingConfiguration")`: "ARN of
//! the StorageConfiguration resource to use for individual participant recording"), so an
//! ARN this account holds no configuration for is refused with the operation's modelled
//! `ValidationException` (HTTP 400) and nothing is created or changed, while the empty
//! string — the documented "no storage configuration is specified" value — names no
//! resource and is accepted.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code, the message and the JSON
//! body — never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries (`engine::emulator_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// The modelled `@length` maximum of
/// `AutoParticipantRecordingStorageConfigurationArn`, from
/// `smithy_query(query="shape",shape="AutoParticipantRecordingConfiguration")` — member
/// `storageConfigurationArn`, `length 0..128`.
const STORAGE_CONFIGURATION_ARN_MAX_LENGTH: usize = 128;

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a storage configuration and return the ARN the service minted for it.
    async fn create_storage_configuration(&self, bucket: &str) -> String {
        let request = json!({ "s3": { "bucketName": bucket }, "name": bucket });
        let (status, response) = self.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "CreateStorageConfiguration {request}: expected 200, got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, body: &Value) -> String {
        let (status, response) = self.call("CreateStage", body).await;
        assert_eq!(
            status, 200,
            "CreateStage {body}: expected 200, got {status} {response}"
        );
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The stage `arn`, as `GetStage` reports it.
    async fn get_stage(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {arn} expected 200, got {status} {response}"
        );
        response["stage"].clone()
    }

    /// How many stages this account holds, as `ListStages` reports it.
    async fn stage_count(&self, case: &str) -> usize {
        let (status, response) = self.call("ListStages", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStages expected 200, got {status} {response}"
        );
        response["stages"].as_array().map_or(0, Vec::len)
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The failure's text, under the `message` key an SDK error parser reads.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// Well-formed `storageConfigurationArn` values that name no storage configuration this
/// account holds, each differing from the others along what the rule generalises over —
/// the id, the region, the account and the length of the reference.
///
/// Every row satisfies the member's modelled `@pattern`
/// (`^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:storage-configuration/[a-zA-Z0-9-]+$`) and
/// `@length(0..128)`, so each is refused for the reference it carries and not for its
/// form. `live` is the ARN of a configuration this account DOES hold, which the
/// one-character row is derived from.
fn dangling_arns(live: &str) -> Vec<(String, String)> {
    let longest_prefix = format!("arn:aws:ivs:{REGION}:{ACCOUNT}:storage-configuration/");
    let longest = format!(
        "{longest_prefix}{}",
        "a".repeat(STORAGE_CONFIGURATION_ARN_MAX_LENGTH - longest_prefix.len())
    );
    assert_eq!(
        longest.len(),
        STORAGE_CONFIGURATION_ARN_MAX_LENGTH,
        "the longest-allowed row must sit exactly on the modelled length bound, got {}",
        longest.len()
    );

    // One character of a live configuration's id, changed: the closest a mistyped ARN
    // can get to a real one.
    let mut mistyped: Vec<char> = live.chars().collect();
    if let Some(last) = mistyped.last_mut() {
        *last = if *last == 'z' { 'y' } else { 'z' };
    }
    let mistyped: String = mistyped.into_iter().collect();
    assert_ne!(
        mistyped, live,
        "the mistyped row must differ from the live configuration's arn"
    );

    vec![
        (
            "an id this account never minted".to_string(),
            format!("arn:aws:ivs:{REGION}:{ACCOUNT}:storage-configuration/abcd1234"),
        ),
        (
            "a single character changed on a live configuration's id".to_string(),
            mistyped,
        ),
        (
            "another region".to_string(),
            format!("arn:aws:ivs:eu-west-1:{ACCOUNT}:storage-configuration/abcd1234"),
        ),
        (
            "another account".to_string(),
            format!("arn:aws:ivs:{REGION}:111122223333:storage-configuration/abcd1234"),
        ),
        (
            "the longest id the member's length allows".to_string(),
            longest,
        ),
        (
            "an id of one character".to_string(),
            format!("arn:aws:ivs:{REGION}:{ACCOUNT}:storage-configuration/a"),
        ),
    ]
}

/// A refusal of a dangling reference states the member that carried it and the value the
/// request sent, so a client can tell it from a refusal of some other member.
fn assert_names_the_reference(case: &str, operation: &str, arn: &str, response: &Value) {
    let message = error_message(response);
    assert!(
        message.contains("autoParticipantRecordingConfiguration.storageConfigurationArn"),
        "case {case:?}: expected the {operation} refusal to name the member that carried \
         the reference, got {message:?}"
    );
    assert!(
        message.contains(arn),
        "case {case:?}: expected the {operation} refusal to name the arn {arn:?} the \
         request sent, got {message:?}"
    );
}

/// CreateStage refuses every well-formed reference to a storage configuration this
/// account does not hold, and creates no stage for it.
#[tokio::test]
async fn create_stage_refuses_a_storage_configuration_this_account_does_not_hold() {
    let emulator = Emulator::new();
    let live = emulator.create_storage_configuration("live").await;

    for (case, arn) in dangling_arns(&live) {
        let before = emulator.stage_count(&case).await;
        let (status, response) = emulator
            .call(
                "CreateStage",
                &json!({
                    "name": "recorded",
                    "autoParticipantRecordingConfiguration": {
                        "storageConfigurationArn": arn.as_str(),
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {arn}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException for {arn}, got {response}"
        );
        assert_names_the_reference(&case, "CreateStage", &arn, &response);

        let after = emulator.stage_count(&case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused create to leave {before} stages, got {after}"
        );
    }
}

/// UpdateStage applies the same rule, and a refused update leaves the stage exactly as it
/// was — including a stage that already records into a configuration this account holds.
#[tokio::test]
async fn update_stage_refuses_a_storage_configuration_this_account_does_not_hold() {
    let emulator = Emulator::new();
    let live = emulator.create_storage_configuration("live").await;

    // Rows differ in what the stage being updated already records into: nothing, and a
    // configuration this account holds.
    let stages: Vec<(&str, Value)> = vec![
        ("a stage with no storage configured", json!({ "name": "plain" })),
        (
            "a stage already recording into a live configuration",
            json!({
                "name": "recorded",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": live.as_str(),
                },
            }),
        ),
    ];

    for (stage_case, request) in stages {
        let stage_arn = emulator.create_stage(&request).await;
        let before = emulator.get_stage(stage_case, &stage_arn).await;

        for (arn_case, arn) in dangling_arns(&live) {
            let case = format!("{stage_case}, {arn_case}");
            let (status, response) = emulator
                .call(
                    "UpdateStage",
                    &json!({
                        "arn": stage_arn.as_str(),
                        "autoParticipantRecordingConfiguration": {
                            "storageConfigurationArn": arn.as_str(),
                        },
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for {arn}, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException for {arn}, got {response}"
            );
            assert_names_the_reference(&case, "UpdateStage", &arn, &response);

            let after = emulator.get_stage(&case, &stage_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to leave the stage as it was \
                 ({before}), got {after}"
            );
        }
    }
}

/// A refused update applies NONE of the request: the members sent beside the dangling
/// reference — the stage's `name`, and the other members of the recording configuration —
/// are not written either, because the reference is resolved before the record is touched.
///
/// Rows differ in what the refused request carries beside the reference, which is what the
/// rule generalises over here: the reference alone, a new `name`, every other member of
/// `AutoParticipantRecordingConfiguration`
/// (`smithy_query(query="shape",shape="AutoParticipantRecordingConfiguration")`), and both.
#[tokio::test]
async fn a_refused_update_applies_none_of_the_members_sent_beside_the_reference() {
    let emulator = Emulator::new();
    let live = emulator.create_storage_configuration("live").await;

    // The stage under test already carries a name and a configuration this account holds,
    // so applying any part of a refused request would be visible to a later read.
    let stage_arn = emulator
        .create_stage(&json!({
            "name": "original",
            "autoParticipantRecordingConfiguration": {
                "storageConfigurationArn": live.as_str(),
                "recordingReconnectWindowSeconds": 30,
            },
        }))
        .await;
    let before = emulator.get_stage("baseline", &stage_arn).await;
    assert_eq!(
        before["name"],
        json!("original"),
        "baseline: expected the stage to be created as \"original\", got {before}"
    );

    // Every other member of the configuration, each inside its modelled range or length.
    let every_other_member = json!({
        "mediaTypes": ["AUDIO_VIDEO"],
        "thumbnailConfiguration": {
            "targetIntervalSeconds": 120,
            "storage": ["LATEST"],
            "recordingMode": "INTERVAL",
        },
        "recordingReconnectWindowSeconds": 300,
        "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
        "recordParticipantReplicas": false,
    });
    let cases: Vec<(&str, Option<&str>, Value)> = vec![
        ("the dangling reference alone", None, json!({})),
        ("a new name beside the reference", Some("renamed"), json!({})),
        (
            "every other configuration member beside the reference",
            None,
            every_other_member.clone(),
        ),
        (
            "a new name and every other configuration member",
            Some("renamed"),
            every_other_member,
        ),
    ];

    for (case, name, other_members) in cases {
        for (arn_case, arn) in dangling_arns(&live) {
            let case = format!("{case}, {arn_case}");
            let mut configuration = json!({ "storageConfigurationArn": arn.as_str() });
            for (member, value) in other_members
                .as_object()
                .expect("the other-member rows are objects")
            {
                configuration[member] = value.clone();
            }
            let mut request = json!({
                "arn": stage_arn.as_str(),
                "autoParticipantRecordingConfiguration": configuration,
            });
            if let Some(name) = name {
                request["name"] = json!(name);
            }

            let (status, response) = emulator.call("UpdateStage", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for {request}, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException for {request}, got {response}"
            );
            assert_names_the_reference(&case, "UpdateStage", &arn, &response);

            let after = emulator.get_stage(&case, &stage_arn).await;
            assert_eq!(
                after["name"],
                json!("original"),
                "case {case:?}: expected the refused update to leave the name \"original\", \
                 got {after}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to leave the stage as it was \
                 ({before}), got {after}"
            );
        }
    }
}

/// A reference this account DOES hold is attached and read back, and so is the documented
/// empty string, which names no resource at all.
#[tokio::test]
async fn a_storage_configuration_this_account_holds_is_attached_and_read_back() {
    let emulator = Emulator::new();
    let first = emulator.create_storage_configuration("first").await;
    let second = emulator.create_storage_configuration("second").await;

    // Rows differ in WHICH held configuration is named, and in whether the request names
    // one at all: the empty string is the documented "no storage configuration is
    // specified" value and the absent member is the same thing left out.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "the first configuration this account holds",
            json!({ "storageConfigurationArn": first.as_str() }),
            first.as_str(),
        ),
        (
            "the second configuration this account holds",
            json!({ "storageConfigurationArn": second.as_str() }),
            second.as_str(),
        ),
        (
            "the documented empty string",
            json!({ "storageConfigurationArn": "" }),
            "",
        ),
        ("no configuration member at all", Value::Null, ""),
    ];

    for (case, configuration, expected) in cases {
        let mut request = json!({ "name": "accepted" });
        if !configuration.is_null() {
            request["autoParticipantRecordingConfiguration"] = configuration.clone();
        }
        let (status, created) = emulator.call("CreateStage", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {request}, got {status} {created}"
        );
        let stage_arn = created["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let actual =
            &created["stage"]["autoParticipantRecordingConfiguration"]["storageConfigurationArn"];
        assert_eq!(
            actual,
            &json!(expected),
            "case {case:?}: expected CreateStage to report {expected:?}, got {created}"
        );

        // The create response is what a client caches, so a read must report exactly it.
        let read = emulator.get_stage(case, &stage_arn).await;
        assert_eq!(
            read, created["stage"],
            "case {case:?}: expected GetStage to report the created stage, got {read}"
        );

        // And an update attaching the same reference is accepted and read back too.
        if configuration.is_null() {
            continue;
        }
        let other = emulator.create_stage(&json!({ "name": "updated" })).await;
        let (status, updated) = emulator
            .call(
                "UpdateStage",
                &json!({
                    "arn": other.as_str(),
                    "autoParticipantRecordingConfiguration": configuration,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStage expected 200, got {status} {updated}"
        );
        let updated_arn =
            &updated["stage"]["autoParticipantRecordingConfiguration"]["storageConfigurationArn"];
        assert_eq!(
            updated_arn,
            &json!(expected),
            "case {case:?}: expected UpdateStage to report {expected:?}, got {updated}"
        );
        let read = emulator.get_stage(case, &other).await;
        assert_eq!(
            read, updated["stage"],
            "case {case:?}: expected GetStage to report the update, got {read}"
        );
    }
}
