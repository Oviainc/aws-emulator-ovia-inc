// SPDX-License-Identifier: CC0-1.0

//! What the customer managed key a pipeline's artifact store declares does to the
//! objects an execution writes — end to end over the wire the service speaks.
//!
//! The rule under test: `ArtifactStore$encryptionKey` is "the encryption key used to
//! encrypt the data in the artifact store", so every object an execution writes is
//! written encrypted under that key — the artifacts its actions produce into the store,
//! and the object its S3 deploy action delivers (under the key that action's own
//! `KMSEncryptionKeyARN` names, when it names one). Encrypting an object under a
//! customer managed key is a USE of that key, so KMS decides it: while the key can be
//! used the execution runs and the objects report that key, and while it cannot — it is
//! disabled, or the service holds no such key — the artifact cannot be produced and the
//! action fails with that reason.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service, take their data keys from the emulated
//! KMS service, and every assertion goes through the router.

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, zip_of, Emulator,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "cmk-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The bucket the deploy action delivers into.
const DEPLOY_BUCKET: &str = "cmk-deploy";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The header S3 reports an object's server-side encryption algorithm in.
const SSE_HEADER: &str = "x-amz-server-side-encryption";

/// The header S3 reports the KMS key an object is encrypted under in.
const SSE_KEY_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The `ServerSideEncryption` value an object encrypted under a KMS key carries
/// (`smithy_query query="shape" shape="ServerSideEncryption"` of the S3 model ->
/// `AES256`, `aws:kms`, `aws:kms:dsse`, `AES256-HMAC-SHA256`).
const SSE_KMS: &str = "aws:kms";

/// Create a customer managed key and answer its ARN.
async fn customer_managed_key(emulator: &Emulator) -> String {
    let created = emulator
        .kms("CreateKey", &json!({"Description": "a pipeline artifact store"}))
        .await;
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created key reports its ARN, got {created}"))
        .to_string()
}

/// Create the buckets and the source object every pipeline here reads.
async fn fixtures(emulator: &Emulator, pipeline: &str, payload: &[u8]) {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, payload).await;
}

/// The `(bucket, key)` the source action's output artifact was written to, read back
/// from the action execution that produced it.
async fn produced_artifact(emulator: &Emulator, pipeline: &str, execution: &str) -> (String, String) {
    let page = emulator
        .ok(
            "ListActionExecutions",
            &json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution}}),
        )
        .await;
    let location = page["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|detail| detail["output"]["outputArtifacts"].as_array())
        .flatten()
        .find(|artifact| artifact["name"] == json!(ARTIFACT))
        .map(|artifact| artifact["s3location"].clone())
        .unwrap_or_else(|| panic!("the produced artifact reports its location, got {page}"));
    (
        location["bucket"].as_str().unwrap_or_default().to_string(),
        location["key"].as_str().unwrap_or_default().to_string(),
    )
}

/// The `(algorithm, key id)` S3 reports for one object, each absent as `None`.
async fn object_encryption(
    emulator: &Emulator,
    bucket: &str,
    key: &str,
    case: &str,
) -> (Option<String>, Option<String>) {
    let (status, headers) = emulator.head_object(bucket, key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: s3://{bucket}/{key} must exist, got {status}"
    );
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    (header(SSE_HEADER), header(SSE_KEY_HEADER))
}

/// RULE: every object an execution writes is encrypted under the key that applies to it
/// — the artifact store's for the artifacts, and the deploy action's own
/// `KMSEncryptionKeyARN` for the object it delivers when it names one — and a store that
/// declares no key writes objects with no KMS encryption at all.
#[tokio::test]
async fn every_object_an_execution_writes_carries_the_key_that_applies_to_it() {
    // (case, the artifact store declares a key, the deploy action names its own key,
    //  the deploy extracts the archive)
    let rows: Vec<(&str, bool, bool, bool)> = vec![
        ("no key declared at all", false, false, false),
        ("the store's key", true, false, false),
        ("the store's key, extracted delivery", true, false, true),
        ("the deploy action's own key", true, true, false),
        ("a deploy key with no store key", false, true, false),
    ];

    for (case, store_key, action_key, extract) in rows {
        let emulator = Emulator::new();
        let pipeline = "cmk-objects";
        let payload = zip_of(&[("out.txt", b"delivered bytes".as_slice())]);
        fixtures(&emulator, pipeline, &payload).await;
        // Two DISTINCT keys, so a row that declares both can tell which object took
        // which: the store's key is not the one the deploy action names.
        let store = customer_managed_key(&emulator).await;
        let delivered_key = customer_managed_key(&emulator).await;

        let mut body = declaration(
            pipeline,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "out", extract),
            ]),
            None,
        );
        if store_key {
            body["pipeline"]["artifactStore"]["encryptionKey"] =
                json!({"id": store, "type": "KMS"});
        }
        if action_key {
            body["pipeline"]["stages"][1]["actions"][0]["configuration"]
                ["KMSEncryptionKeyARN"] = json!(delivered_key);
        }
        emulator.ok("CreatePipeline", &body).await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: a key KMS can issue a data key for runs the execution, \
             got {answer}"
        );

        // The artifact the source action produced, in the pipeline's own store.
        let (bucket, key) = produced_artifact(&emulator, pipeline, &execution).await;
        let expected_store = store_key.then(|| store.clone());
        assert_encryption(
            &emulator,
            &bucket,
            &key,
            expected_store.as_deref(),
            &format!("{case}: the artifact in the store"),
        )
        .await;

        // The object(s) the deploy action delivered, under the key that action resolved:
        // its own when it names one, else the store's.
        let delivered = if extract { "out/out.txt" } else { "out" };
        let expected_delivered = match (action_key, store_key) {
            (true, _) => Some(delivered_key.clone()),
            (false, true) => Some(store.clone()),
            (false, false) => None,
        };
        assert_encryption(
            &emulator,
            DEPLOY_BUCKET,
            delivered,
            expected_delivered.as_deref(),
            &format!("{case}: the delivered object"),
        )
        .await;
    }
}

/// Assert that the object at `bucket`/`key` is encrypted under `expected`, or under no
/// KMS key at all when `expected` is `None`.
///
/// A store that declares no key leaves the write to whatever the BUCKET's own default
/// configuration is, which is S3's own fact about that bucket — so what is asserted
/// there is that no KMS key was named, not which other algorithm the bucket applied.
async fn assert_encryption(
    emulator: &Emulator,
    bucket: &str,
    key: &str,
    expected: Option<&str>,
    case: &str,
) {
    let (algorithm, key_id) = object_encryption(emulator, bucket, key, case).await;
    assert_eq!(
        key_id.as_deref(),
        expected,
        "case {case:?}: unexpected {SSE_KEY_HEADER}, got {key_id:?}"
    );
    match expected {
        Some(_) => assert_eq!(
            algorithm.as_deref(),
            Some(SSE_KMS),
            "case {case:?}: an object encrypted under a KMS key reports {SSE_KMS}, \
             got {algorithm:?}"
        ),
        None => assert_ne!(
            algorithm.as_deref(),
            Some(SSE_KMS),
            "case {case:?}: no key applies, so the object reports no KMS encryption"
        ),
    }
}

/// RULE: whether an artifact can be encrypted under the store's key is KMS's answer at
/// the instant the action runs, so the SAME pipeline runs while the key can be used,
/// fails while it cannot, and runs again once it can.
#[tokio::test]
async fn a_key_kms_refuses_fails_the_execution_and_a_usable_one_restores_it() {
    let emulator = Emulator::new();
    let pipeline = "cmk-key-state";
    fixtures(&emulator, pipeline, b"the source bytes").await;
    let store = customer_managed_key(&emulator).await;
    let mut body = declaration(
        pipeline,
        json!([
            source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
            deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "state.zip", false),
        ]),
        None,
    );
    body["pipeline"]["artifactStore"]["encryptionKey"] = json!({"id": store, "type": "KMS"});
    emulator.ok("CreatePipeline", &body).await;

    // (case, the key is usable for this run, the status the run reaches)
    let rows: Vec<(&str, bool, &str)> = vec![
        // The positive control first: without it nothing below could tell "the key
        // stopped it" from "this pipeline never worked".
        ("the key enabled", true, "Succeeded"),
        ("the key disabled", false, "Failed"),
        ("the key enabled again", true, "Succeeded"),
    ];

    for (case, usable, expected) in rows {
        let operation = if usable { "EnableKey" } else { "DisableKey" };
        emulator.kms(operation, &json!({"KeyId": store})).await;

        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some(expected),
            "case {case:?}: unexpected execution status, got {answer}"
        );
        if expected == "Failed" {
            let details = failure_details(&emulator, pipeline, &execution).await;
            assert_eq!(
                details["code"],
                json!("JobFailed"),
                "case {case:?}: the action's own run is what failed, got {details}"
            );
            let message = details["message"].as_str().unwrap_or_default();
            assert!(
                is_internal_error_detail(message),
                "case {case:?}: what KMS said is not relayed to the caller; the detail \
                 is CodePipeline's internal-error shape, got {message:?}"
            );
            // The execution's own summary carries the action's detail, so it says the
            // same thing rather than a second wording of it.
            let summary = answer["pipelineExecution"]["statusSummary"]
                .as_str()
                .unwrap_or_default();
            assert!(
                summary.contains(message),
                "case {case:?}: the execution's summary carries the action's detail, \
                 got {summary:?}"
            );
        }
    }

    // The two runs that could use the key each delivered their object; the one that
    // could not delivered nothing, so the key really gated the write.
    let (status, body) = emulator.get_object(DEPLOY_BUCKET, "state.zip").await;
    assert_eq!(
        status, 200,
        "the runs that could use the key delivered the object, got {status}"
    );
    assert_eq!(
        body, b"the source bytes",
        "the delivered object carries the source bytes"
    );
}

/// Whether `detail` is CodePipeline's generic internal-error detail: the fixed text and
/// a reference code, exactly as the service words it
/// (`InternalError. Error reference code: <code>.`).
///
/// `pub(crate)` with the suite: the `execution_engine` suite asserts the same shape for
/// the same class of failure rather than growing a second copy of this reading.
pub(crate) fn is_internal_error_detail(detail: &str) -> bool {
    let prefix = "InternalError. Error reference code: ";
    let Some(reference) = detail
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix('.'))
    else {
        return false;
    };
    !reference.is_empty()
        && reference
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
}

/// The `errorDetails` of the first failed action of `execution`.
async fn failure_details(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
    let page = emulator
        .ok(
            "ListActionExecutions",
            &json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution}}),
        )
        .await;
    page["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|detail| detail["status"] == json!("Failed"))
        .map(|detail| detail["output"]["executionResult"]["errorDetails"].clone())
        .unwrap_or_else(|| panic!("a failed execution reports the action that failed, got {page}"))
}
