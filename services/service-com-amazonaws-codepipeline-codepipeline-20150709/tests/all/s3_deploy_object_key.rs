// SPDX-License-Identifier: CC0-1.0

//! Which configuration keys an S3 DEPLOY action has to declare, end to end through the
//! execution plane and the emulated S3 service.
//!
//! The rule under test: an action's `configuration` is a free-form
//! `ActionConfigurationMap`, so what a key is required for is the PROVIDER's rule, and
//! the S3 deploy provider's rule is conditional — `BucketName` is always required, and
//! `ObjectKey` is required only when the artifact is NOT extracted, because an
//! extracting deploy writes one object per archive entry and has no single object key.
//! With `Extract` `true` a declared `ObjectKey` is therefore the PREFIX the entries
//! land beneath, and a declaration without one unzips the artifact into the bucket
//! itself.
//!
//! The rows span what the rule generalises over: whether `Extract` is declared at all,
//! the letter case of its value, whether `ObjectKey` is declared, and whether that key
//! ends in a separator — plus the two declarations the rule does refuse, each of which
//! must fail its action with a reason naming the key it is missing.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so every assertion goes
//! through the same router the production emulator builds and the bytes really move
//! through the emulated S3 service.

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, source_stage, wait_for_terminal, zip_of, Emulator,
};

/// The bucket the deploy action delivers to.
const DEPLOY_BUCKET: &str = "deploy-bucket";

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "source-bucket";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The artifact the source action produces and the deploy action consumes.
const ARTIFACT: &str = "SourceArtifact";

/// The archive the source object carries: one entry at the root and one under a
/// directory, so an extraction's own key layout is observable.
const ENTRIES: [(&str, &[u8]); 2] = [("one.txt", b"first entry"), ("nested/two.txt", b"second")];

/// A deploy stage whose action carries exactly `configuration`.
fn deploy_stage(configuration: Value) -> Value {
    json!({
        "name": "Deploy",
        "actions": [{
            "name": "DeployAction",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": configuration,
            "inputArtifacts": [{"name": ARTIFACT}],
        }],
    })
}

/// What one row expects of the deploy: the objects it delivers, keyed by the S3 key,
/// or the fragment the failure reason must name.
enum Expected {
    /// The execution succeeds and the bucket holds exactly these `(key, bytes)`.
    Delivered(Vec<(String, Vec<u8>)>),
    /// The execution fails and the reason names this configuration key.
    Refused(&'static str),
}

/// Every archive entry, delivered under `prefix`.
fn extracted(prefix: &str) -> Vec<(String, Vec<u8>)> {
    ENTRIES
        .iter()
        .map(|(name, body)| (format!("{prefix}{name}"), body.to_vec()))
        .collect()
}

#[tokio::test]
async fn an_s3_deploy_action_needs_an_object_key_only_when_it_does_not_extract() {
    let archive = zip_of(&ENTRIES);

    // (case, the action's whole configuration, what the deploy must do)
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "Extract true and no ObjectKey",
            json!({"BucketName": DEPLOY_BUCKET, "Extract": "true"}),
            Expected::Delivered(extracted("")),
        ),
        (
            "Extract true with an ObjectKey prefix",
            json!({"BucketName": DEPLOY_BUCKET, "ObjectKey": "unpacked", "Extract": "true"}),
            Expected::Delivered(extracted("unpacked/")),
        ),
        (
            "Extract true with a prefix that already ends in a separator",
            json!({"BucketName": DEPLOY_BUCKET, "ObjectKey": "dir/", "Extract": "true"}),
            Expected::Delivered(extracted("dir/")),
        ),
        (
            "Extract stated in another letter case",
            json!({"BucketName": DEPLOY_BUCKET, "Extract": "TRUE"}),
            Expected::Delivered(extracted("")),
        ),
        (
            "Extract false with an ObjectKey",
            json!({
                "BucketName": DEPLOY_BUCKET,
                "ObjectKey": "delivered.zip",
                "Extract": "false",
            }),
            Expected::Delivered(vec![("delivered.zip".to_string(), archive.clone())]),
        ),
        (
            "no Extract member at all, with an ObjectKey",
            json!({"BucketName": DEPLOY_BUCKET, "ObjectKey": "plain.zip"}),
            Expected::Delivered(vec![("plain.zip".to_string(), archive.clone())]),
        ),
        (
            "Extract false and no ObjectKey",
            json!({"BucketName": DEPLOY_BUCKET, "Extract": "false"}),
            Expected::Refused("ObjectKey"),
        ),
        (
            "no BucketName",
            json!({"ObjectKey": "delivered.zip", "Extract": "true"}),
            Expected::Refused("BucketName"),
        ),
    ];

    for (index, (case, configuration, expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("deploy-keys-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        emulator
            .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
            .await;

        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage(configuration.clone()),
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("the start reports an execution id")
            .to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        let status = answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let summary = answer["pipelineExecution"]["statusSummary"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        match expected {
            Expected::Delivered(objects) => {
                assert_eq!(
                    status, "Succeeded",
                    "case {case:?}: the declaration is complete for this provider, so the \
                     execution must succeed; got {status} with {summary:?}"
                );
                for (key, body) in objects {
                    let (read, bytes) = emulator.get_object(DEPLOY_BUCKET, &key).await;
                    assert_eq!(
                        read, 200,
                        "case {case:?}: s3://{DEPLOY_BUCKET}/{key} must be delivered, got {read}"
                    );
                    assert_eq!(
                        bytes.len(),
                        body.len(),
                        "case {case:?}: s3://{DEPLOY_BUCKET}/{key} must carry {} bytes, got {}",
                        body.len(),
                        bytes.len()
                    );
                    assert!(
                        bytes == body,
                        "case {case:?}: s3://{DEPLOY_BUCKET}/{key} must carry the bytes the \
                         artifact holds"
                    );
                }
            }
            Expected::Refused(key) => {
                assert_eq!(
                    status, "Failed",
                    "case {case:?}: a declaration missing {key} cannot be deployed, got \
                     {status} with {summary:?}"
                );
                assert!(
                    summary.contains(key),
                    "case {case:?}: the reason must name the missing {key}, got {summary:?}"
                );
            }
        }
    }
}

/// An extracting deploy that declares no `ObjectKey` writes each entry under its own
/// name, so nothing is delivered under the name of the artifact itself — the key an
/// unextracted delivery would have used.
#[tokio::test]
async fn an_extracting_deploy_without_an_object_key_delivers_only_the_entries() {
    let emulator = Emulator::new();
    let pipeline = "deploy-keys-entries-only";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    let archive = zip_of(&ENTRIES);
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
        .await;

    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage(json!({"BucketName": DEPLOY_BUCKET, "Extract": "true"})),
                ]),
                None,
            ),
        )
        .await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    let execution = started["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();
    let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        answer["pipelineExecution"]["status"],
        json!("Succeeded"),
        "an extracting deploy with no ObjectKey must run to completion, got {answer}"
    );

    for absent in [ARTIFACT, SOURCE_KEY] {
        let (status, _) = emulator.get_object(DEPLOY_BUCKET, absent).await;
        assert_ne!(
            status, 200,
            "nothing may be delivered under {absent:?}: the entries are the delivery"
        );
    }
}
