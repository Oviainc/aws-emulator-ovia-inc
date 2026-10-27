// SPDX-License-Identifier: CC0-1.0

//! WHICH source object an execution's S3 source action reads, and what does — and does
//! not — decide it.
//!
//! The rule under test: a source action reads the source its own `configuration`
//! declares, and the only thing that changes that is a `SourceRevisionOverride` the
//! start stated for that action, applied according to its `revisionType` —
//! `S3_OBJECT_KEY` names the object key and `S3_OBJECT_VERSION_ID` the object version
//! ("either of the types can be used independently, or they can be used together to
//! override the source with a specific ObjectKey and VersionID"). Every other revision
//! an execution carries — an override of a type that describes another provider's
//! revision, or a revision merely REPORTED for an action through `PutActionRevision` —
//! says which revision the pipeline is running and selects no object and no version, so
//! the action reads the declared key at its current version.
//!
//! The rows span what that rule generalises over: which revision types the execution
//! carries (neither, each of the two S3 ones, both together, and the two that are not
//! about S3), and which call triggered the execution. The revision types are the
//! model's own (`smithy_query query="shape" shape="SourceRevisionType"` ->
//! `COMMIT_ID`, `IMAGE_DIGEST`, `S3_OBJECT_VERSION_ID`, `S3_OBJECT_KEY`), so the table
//! covers each of them.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so the bytes really move
//! through the emulated S3 service and every assertion goes through the router.

use serde_json::json;

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, Emulator,
};

/// The bucket the source objects live in.
const SOURCE_BUCKET: &str = "source-bucket";

/// The bucket the deploy action delivers the artifact to, where a row reads back the
/// bytes the source action actually fetched.
const DEPLOY_BUCKET: &str = "deploy-bucket";

/// The object key the pipeline's source action DECLARES.
const DECLARED_KEY: &str = "main.zip";

/// Another object key in the same bucket, which only an `S3_OBJECT_KEY` override
/// reaches.
const OTHER_KEY: &str = "other.zip";

/// The two writes of each object: the older bytes, then the current ones.
const DECLARED_OLD: &[u8] = b"declared key, first version";
/// See [`DECLARED_OLD`].
const DECLARED_NEW: &[u8] = b"declared key, current version";
/// See [`DECLARED_OLD`].
const OTHER_OLD: &[u8] = b"other key, first version";
/// See [`DECLARED_OLD`].
const OTHER_NEW: &[u8] = b"other key, current version";

/// The artifact the source action produces and the deploy action delivers.
const ARTIFACT: &str = "SourceArtifact";

/// The key the deploy action delivers the artifact under.
const DELIVERED: &str = "delivered";

/// What one row's override value is, resolved once the objects have been written
/// (a version id is minted by the write, so no row can name one literally).
#[derive(Debug, Clone, Copy)]
enum Pinned {
    /// The version id the FIRST write of the declared key reported.
    DeclaredFirstVersion,
    /// The version id the first write of the other key reported.
    OtherFirstVersion,
    /// The other object's key.
    OtherKey,
    /// Text that names no S3 object at all — what an override about another provider's
    /// revision carries.
    NotAboutS3,
}

/// What starts the execution a row observes.
#[derive(Debug)]
enum Trigger {
    /// `StartPipelineExecution`, carrying these `(revisionType, value)` overrides.
    Start(Vec<(&'static str, Pinned)>),
    /// A fresh revision reported for the source action through `PutActionRevision`,
    /// whose `revisionId` is no S3 object version.
    ReportedRevision,
}

/// The version ids the writes reported, so a row's `Pinned` can be resolved.
struct Versions {
    declared_first: String,
    other_first: String,
}

impl Versions {
    /// The literal one override carries for `pinned`.
    fn value(&self, pinned: Pinned) -> String {
        match pinned {
            Pinned::DeclaredFirstVersion => self.declared_first.clone(),
            Pinned::OtherFirstVersion => self.other_first.clone(),
            Pinned::OtherKey => OTHER_KEY.to_string(),
            Pinned::NotAboutS3 => "0123456789abcdef0123456789abcdef01234567".to_string(),
        }
    }
}

#[tokio::test]
async fn only_an_s3_source_revision_override_changes_the_object_a_source_action_reads() {
    // (case, what triggers the execution, the bytes the source action must have read)
    let rows: Vec<(&str, Trigger, &[u8])> = vec![
        (
            "no override at all",
            Trigger::Start(Vec::new()),
            DECLARED_NEW,
        ),
        (
            "an S3_OBJECT_VERSION_ID override",
            Trigger::Start(vec![(
                "S3_OBJECT_VERSION_ID",
                Pinned::DeclaredFirstVersion,
            )]),
            DECLARED_OLD,
        ),
        (
            "an S3_OBJECT_KEY override",
            Trigger::Start(vec![("S3_OBJECT_KEY", Pinned::OtherKey)]),
            OTHER_NEW,
        ),
        (
            "both S3 override types together",
            Trigger::Start(vec![
                ("S3_OBJECT_KEY", Pinned::OtherKey),
                ("S3_OBJECT_VERSION_ID", Pinned::OtherFirstVersion),
            ]),
            OTHER_OLD,
        ),
        (
            "a COMMIT_ID override, which is another provider's revision",
            Trigger::Start(vec![("COMMIT_ID", Pinned::NotAboutS3)]),
            DECLARED_NEW,
        ),
        (
            "an IMAGE_DIGEST override, which is another provider's revision",
            Trigger::Start(vec![("IMAGE_DIGEST", Pinned::NotAboutS3)]),
            DECLARED_NEW,
        ),
        (
            "a revision reported for the action rather than overridden",
            Trigger::ReportedRevision,
            DECLARED_NEW,
        ),
    ];

    for (index, (case, trigger, expected)) in rows.into_iter().enumerate() {
        // One pipeline and one emulator per row, so the object a row reads back is the
        // one its own execution delivered.
        let emulator = Emulator::new();
        let pipeline = format!("pinned-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;

        let declared_first = emulator
            .put_object(SOURCE_BUCKET, DECLARED_KEY, DECLARED_OLD)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a versioned write reports a version id"));
        emulator
            .put_object(SOURCE_BUCKET, DECLARED_KEY, DECLARED_NEW)
            .await;
        let other_first = emulator
            .put_object(SOURCE_BUCKET, OTHER_KEY, OTHER_OLD)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a versioned write reports a version id"));
        emulator.put_object(SOURCE_BUCKET, OTHER_KEY, OTHER_NEW).await;
        let versions = Versions {
            declared_first,
            other_first,
        };

        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, DECLARED_KEY, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, DELIVERED, false),
                    ]),
                    Some("PARALLEL"),
                ),
            )
            .await;

        let answered = match &trigger {
            Trigger::Start(overrides) => {
                let mut request = json!({"name": pipeline});
                if !overrides.is_empty() {
                    request["sourceRevisions"] = overrides
                        .iter()
                        .map(|(revision_type, pinned)| {
                            json!({
                                // `source_stage` names the source action `Src`.
                                "actionName": "Src",
                                "revisionType": revision_type,
                                "revisionValue": versions.value(*pinned),
                            })
                        })
                        .collect();
                }
                emulator.ok("StartPipelineExecution", &request).await
            }
            Trigger::ReportedRevision => {
                emulator
                    .ok(
                        "PutActionRevision",
                        &json!({
                            "pipelineName": pipeline,
                            "stageName": "Source",
                            "actionName": "Src",
                            "actionRevision": {
                                "revisionId": "reported-revision-1",
                                "revisionChangeId": "reported-change-1",
                                "created": 1_760_000_000_i64,
                            },
                        }),
                    )
                    .await
            }
        };
        let execution = answered["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the trigger reports an execution id, got {answered}"))
            .to_string();

        let final_answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            final_answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the source the rule resolves can be read, so the execution \
             must finish, got {final_answer}"
        );
        let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, DELIVERED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the artifact must be delivered, got {status}"
        );
        assert_eq!(
            bytes, expected,
            "case {case:?}: expected the source action to read {:?}, got {:?}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&bytes)
        );
    }
}

/// RULE: a revision reported for an action names no S3 object version, so the execution
/// it starts reads the declared source and carries on — it is the ACTION's own outcome,
/// not the reported revision, that decides where the execution gets to.
///
/// The rows span the two outcomes an S3 source action has: a declared object this
/// emulator can read (the execution carries on past the source stage) and one it cannot
/// (the action fails with the reason, and the execution with it).
#[tokio::test]
async fn a_reported_revision_is_not_read_as_an_object_version() {
    // (case, the key the declaration points at, whether that object was written,
    //  the status the execution must reach)
    let rows: Vec<(&str, &str, bool, &str)> = vec![
        ("a source object that is there", DECLARED_KEY, true, "Succeeded"),
        ("a source object that is not", "absent.zip", false, "Failed"),
    ];

    for (index, (case, key, written, expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("reported-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        if written {
            emulator.put_object(SOURCE_BUCKET, key, DECLARED_NEW).await;
        }
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, key, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, DELIVERED, false),
                    ]),
                    Some("PARALLEL"),
                ),
            )
            .await;

        let answered = emulator
            .ok(
                "PutActionRevision",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": "Source",
                    "actionName": "Src",
                    "actionRevision": {
                        // No S3 object carries this as a version id, and nothing has to:
                        // a reported revision is not one.
                        "revisionId": "a-revision-no-object-carries",
                        "revisionChangeId": "change-1",
                        "created": 1_760_000_000_i64,
                    },
                }),
            )
            .await;
        assert_eq!(
            answered["newRevision"],
            json!(true),
            "case {case:?}: the revision the action does not carry is new, got {answered}"
        );
        let execution = answered["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the report starts an execution, got {answered}"))
            .to_string();

        let final_answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        let status = final_answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            status, expected,
            "case {case:?}: expected the execution to reach {expected}, got {final_answer}"
        );
        let summary = final_answer["pipelineExecution"]["statusSummary"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if expected == "Succeeded" {
            let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, DELIVERED).await;
            assert_eq!(
                (status, bytes.as_slice()),
                (200, DECLARED_NEW),
                "case {case:?}: the declared object's current bytes are what ran, got {status}"
            );
            continue;
        }
        // The failure is the ABSENT object, named as such: nothing here failed over a
        // version id, because none was asked for.
        assert!(
            summary.contains(key) && !summary.contains("version"),
            "case {case:?}: the reason names the source that could not be read, got {summary:?}"
        );
    }
}
