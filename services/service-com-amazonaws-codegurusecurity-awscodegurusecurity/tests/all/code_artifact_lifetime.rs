// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how long a minted `codeArtifactId` keeps naming its code
//! artifact.
//!
//! `ResourceId$codeArtifactId` is documented as "The identifier for the code file uploaded
//! to the resource object. Returned by `CreateUploadUrl` when you upload resources to be
//! scanned", and nothing in the model gives the artifact it names a lifetime. The ONE
//! time-limited thing `CreateUploadUrl` answers with is the pre-signed URL: a pre-signed
//! URL accepts a write only while its signature is valid. So the rule this suite asserts
//! is the separation of the two:
//!
//! * an identifier the service minted resolves for every later call that takes it,
//!   however much time passed since it was minted — `CreateScan` answers 200 and records
//!   a run, never a 404;
//! * the signature window of the published URL still closes, and closing it refuses only
//!   the UPLOAD to that URL, leaving the artifact, the resource already held against it
//!   and every answer about it untouched.
//!
//! Elapsed time is simulated by BACKDATING the stored record rather than by sleeping:
//! everything else (the mint, the upload, the scan, the read) goes through the real router
//! exactly as a client drives it.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    self as cgs_state, SCAN_RESOURCE_NOT_UPLOADED_MESSAGE, SCAN_STATE_FAILED,
    SCAN_STATE_IN_PROGRESS, SCAN_STATE_SUCCESSFUL, UPLOAD_URL_SIGNATURE_WINDOW_MILLIS,
};

use crate::scan_lifecycle::{call, create_scan, create_upload_url, harness, store_of};
use crate::upload_target::{
    archive_of, is_record_of, required_headers, settled_run, stored_upload, upload,
    wait_for_settle,
};

/// One millisecond, an hour, a day and a year in milliseconds, for the ages a case puts
/// between the mint and the call that uses the identifier.
const ONE_HOUR_MILLIS: i64 = 60 * 60 * 1_000;
const ONE_DAY_MILLIS: i64 = 24 * ONE_HOUR_MILLIS;
const ONE_YEAR_MILLIS: i64 = 365 * ONE_DAY_MILLIS;

/// Move the stored record of `artifact` back in time, so it reads as one minted
/// `age_millis` ago: the instant it was minted and the end of its pre-signed URL's
/// signature window both shift with it, which is exactly what the clock would have done.
fn age_artifact(engine: &Arc<engine::InMemoryEngine>, artifact: &str, age_millis: i64) {
    let store = store_of(engine);
    let mut guard = store.lock();
    let record = guard
        .upload_artifacts
        .get_mut(artifact)
        .unwrap_or_else(|| panic!("the store holds no artifact {artifact}"));
    let minted_at = cgs_state::now_millis() - age_millis;
    record.created_at_millis = minted_at;
    record.url_expires_at_millis = minted_at.saturating_add(UPLOAD_URL_SIGNATURE_WINDOW_MILLIS);
}

/// Whether the store still holds a record for `artifact` at all.
fn is_recorded(engine: &Arc<engine::InMemoryEngine>, artifact: &str) -> bool {
    store_of(engine).lock().upload_artifacts.contains_key(artifact)
}

#[tokio::test]
async fn create_scan_resolves_a_minted_artifact_however_long_ago_it_was_minted() {
    let (engine, router) = harness();

    // Rows: the age of the identifier when `CreateScan` is called with it — on both sides
    // of the pre-signed URL's signature window, and then at the spans a real client
    // reaches across a long build, a CI stage boundary or a manual pause. The window is
    // the only published time limit, and it governs the UPLOAD, so no row may change the
    // answer.
    let cases: [(&str, i64); 7] = [
        ("used straight after it was minted", 0),
        (
            "one millisecond inside the URL's signature window",
            UPLOAD_URL_SIGNATURE_WINDOW_MILLIS - 1,
        ),
        (
            "exactly at the end of the URL's signature window",
            UPLOAD_URL_SIGNATURE_WINDOW_MILLIS,
        ),
        (
            "one millisecond past the URL's signature window",
            UPLOAD_URL_SIGNATURE_WINDOW_MILLIS + 1,
        ),
        ("an hour after it was minted", ONE_HOUR_MILLIS),
        ("a day after it was minted", ONE_DAY_MILLIS),
        ("a year after it was minted", ONE_YEAR_MILLIS),
    ];

    let mut started: Vec<(&str, String)> = Vec::new();
    for (index, (case, age_millis)) in cases.iter().enumerate() {
        let scan_name = format!("aged-artifact-{index}");
        let archive = archive_of(192, u8::try_from(index + 1).unwrap_or(1));

        // The documented workflow, over HTTP: mint the target, upload the archive to it.
        let minted = create_upload_url(&router, &scan_name).await;
        assert_eq!(
            minted.status, 200,
            "case {case:?}: CreateUploadUrl must mint a target, got {} {}",
            minted.status, minted.json
        );
        let artifact = minted.str_of("codeArtifactId");
        let uploaded = upload(
            &router,
            &minted.str_of("s3Url"),
            &required_headers(&minted),
            archive.clone(),
        )
        .await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: the documented upload must be accepted, got {} {:?}",
            uploaded.status, uploaded.error_code
        );

        // ... and then the caller comes back with the identifier this much later.
        age_artifact(&engine, &artifact, *age_millis);

        let scan = create_scan(&router, json!({ "scanName": &scan_name }), &artifact).await;
        assert_eq!(
            scan.status, 200,
            "case {case:?}: CreateScan on a minted codeArtifactId must be served, got {} {}",
            scan.status, scan.json
        );
        assert_eq!(
            scan.get("scanState").as_str(),
            Some(SCAN_STATE_IN_PROGRESS),
            "case {case:?}: a started run must report {SCAN_STATE_IN_PROGRESS}, got {}",
            scan.json
        );
        let run_id = scan.str_of("runId");
        assert_eq!(
            scan.get("resourceId")["codeArtifactId"].as_str(),
            Some(artifact.as_str()),
            "case {case:?}: the run must be reported against the artifact it was started \
             on, got {}",
            scan.json
        );

        // Round trip: the run is readable back through GetScan, so a run really was
        // recorded rather than only answered.
        let read = call(&router, "GET", &format!("/scans/{scan_name}"), None).await;
        assert_eq!(
            (read.status, read.get("runId").as_str()),
            (200, Some(run_id.as_str())),
            "case {case:?}: GetScan must report run {run_id}, got {} {}",
            read.status,
            read.json
        );

        // Nothing released the artifact or the resource held against it.
        assert!(
            is_recorded(&engine, &artifact),
            "case {case:?}: the artifact must still be recorded after the call"
        );
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("case {case:?}: the target holds no resource"));
        assert!(
            is_record_of(&held, &archive),
            "case {case:?}: the uploaded resource must be untouched, got {} bytes",
            held.byte_count
        );
        started.push((case, scan_name));
    }

    // Each run had a code resource to analyse, so each was carried out: the age of the
    // identifier changes the outcome of the run no more than it changes the answer.
    wait_for_settle().await;
    for (case, scan_name) in &started {
        let settled = settled_run(&router, scan_name).await;
        assert_eq!(
            (settled.status, settled.scan_state.as_str()),
            (200, SCAN_STATE_SUCCESSFUL),
            "case {case:?}: expected a {SCAN_STATE_SUCCESSFUL} run, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn the_closed_signature_window_refuses_only_the_upload_to_the_published_url() {
    let (engine, router) = harness();
    let scan_name = "window-closed-after-upload";
    let archive = archive_of(320, 11);

    let minted = create_upload_url(&router, scan_name).await;
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");
    let headers = required_headers(&minted);
    assert_eq!(
        upload(&router, &url, &headers, archive.clone()).await.status,
        200,
        "the upload inside the signature window must be accepted"
    );

    // Rows: how far past the end of the signature window a second upload to the SAME
    // published URL arrives. A pre-signed URL whose signature is no longer valid refuses
    // the write it was signed for, and says so with the status and code S3 answers.
    for (case, age_millis) in [
        (
            "one millisecond past the window",
            UPLOAD_URL_SIGNATURE_WINDOW_MILLIS + 1,
        ),
        ("a day past the window", ONE_DAY_MILLIS),
        ("a year past the window", ONE_YEAR_MILLIS),
    ] {
        age_artifact(&engine, &artifact, age_millis);
        let refused = upload(&router, &url, &headers, archive_of(64, 13)).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected 403 AccessDenied, got {} {:?}",
            refused.status,
            refused.error_code
        );
        assert!(
            String::from_utf8_lossy(&refused.body).contains("Request has expired"),
            "case {case:?}: the refusal must say the request has expired, got {:?}",
            String::from_utf8_lossy(&refused.body)
        );
        // The artifact and the resource already held against it are untouched: the
        // closed window refused this write and nothing else.
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("case {case:?}: the target holds no resource"));
        assert!(
            is_record_of(&held, &archive),
            "case {case:?}: the resource uploaded inside the window must still be held, \
             got {} bytes",
            held.byte_count
        );
    }

    // And the artifact is still scannable, with the resource that was uploaded while the
    // window was open.
    let scan = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
    assert_eq!(
        scan.status, 200,
        "CreateScan after the signature window closed: expected 200, got {} {}",
        scan.status, scan.json
    );
    wait_for_settle().await;
    let settled = settled_run(&router, scan_name).await;
    assert_eq!(
        (settled.status, settled.scan_state.as_str()),
        (200, SCAN_STATE_SUCCESSFUL),
        "the run analysed the uploaded resource, got {settled:?}"
    );
}

#[tokio::test]
async fn an_artifact_nothing_was_uploaded_to_is_scanned_and_the_run_reports_why_it_failed() {
    let (engine, router) = harness();
    let scan_name = "never-uploaded";

    // The caller minted a target, never uploaded to it, and came back long after the
    // signature window closed. The identifier still resolves — the run is what reports
    // that there was no resource to analyse.
    let artifact = create_upload_url(&router, scan_name)
        .await
        .str_of("codeArtifactId");
    age_artifact(&engine, &artifact, ONE_YEAR_MILLIS);

    let scan = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
    assert_eq!(
        scan.status, 200,
        "CreateScan on an artifact nothing was uploaded to: expected 200, got {} {}",
        scan.status, scan.json
    );
    assert!(
        is_recorded(&engine, &artifact),
        "the artifact must still be recorded"
    );

    wait_for_settle().await;
    let settled = settled_run(&router, scan_name).await;
    assert_eq!(
        (settled.status, settled.scan_state.as_str()),
        (200, SCAN_STATE_FAILED),
        "a run with no resource to analyse must fail, got {settled:?}"
    );
    assert_eq!(
        settled.error_message, SCAN_RESOURCE_NOT_UPLOADED_MESSAGE,
        "the failed run must report why, got {settled:?}"
    );
}

#[tokio::test]
async fn an_identifier_this_service_never_minted_is_the_only_one_create_scan_cannot_resolve() {
    let (_engine, router) = harness();

    // Rows: identifiers no `CreateUploadUrl` of this account ever answered with. These —
    // and nothing about elapsed time — are what `CreateScan` reports as absent.
    for (case, candidate) in [
        ("a well-formed Uuid nobody minted", "00000000-0000-0000-0000-000000000000"),
        ("another well-formed Uuid nobody minted", "deadbeef-1234-5678-9abc-def012345678"),
    ] {
        let refused = create_scan(&router, json!({ "scanName": "no-artifact" }), candidate).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} {}",
            refused.status, refused.json
        );
        assert!(
            refused.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.get("resourceId").as_str(),
            Some(candidate),
            "case {case:?}: the refusal must name the identifier it could not resolve, got {}",
            refused.json
        );
    }
}
