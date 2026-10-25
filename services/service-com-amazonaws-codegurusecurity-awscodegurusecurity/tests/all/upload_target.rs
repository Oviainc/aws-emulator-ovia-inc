// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the upload target `CreateUploadUrl` publishes as `s3Url`.
//!
//! `CreateUploadUrlResponse$s3Url` is documented as a pre-signed URL the caller uploads
//! the code file it wants to scan to, "with the required `requestHeaders` using any HTTP
//! client", and `CreateUploadUrlResponse$codeArtifactId` as the identifier of "the
//! uploaded code resource" that `CreateScan` is then started on. The rule this suite
//! asserts is the one that makes those two members hold together:
//!
//! * the published URL is a target THIS emulator serves, so the documented upload can be
//!   performed against it — unsigned, as a pre-signed URL is followed;
//! * the upload is PERSISTED against the code artifact the URL names, so the resource a
//!   scan run is started on is the archive that was uploaded, whatever its size or media
//!   type and whatever scan name the target was minted for;
//! * a target holds one resource, so a repeated upload replaces it;
//! * the archive's SIZE is never a refusal — see `crate::upload_size`, which asserts that
//!   rule over a table of sizes;
//! * everything else on that path space is refused: a method the URL does not describe
//!   and a path naming no target of this account.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), the same
//! path a client takes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    self as cgs_state, UploadArtifact, UploadedResource, DEFAULT_REGION,
    SCAN_RESOURCE_NOT_UPLOADED_MESSAGE, SCAN_STATE_FAILED, SCAN_STATE_SUCCESSFUL,
    STANDARD_SCAN_SETTLE_MILLIS, UPLOAD_ARTIFACT_TTL_MILLIS, UPLOAD_RETAINED_PREFIX_MAX_BYTES,
};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{
    call, create_scan, create_upload_url, harness, store_of, Answer, REGION,
};

/// The media type the response's `requestHeaders` names for the archive, which is what a
/// client following the documented workflow sends.
const ARCHIVE_CONTENT_TYPE: &str = "application/zip";

/// What a scan run that has settled reports, read off `GetScan`.
#[derive(Debug)]
pub(crate) struct SettledRun {
    pub(crate) status: u16,
    pub(crate) scan_state: String,
    pub(crate) error_message: String,
}

/// What a request to a published upload target answered.
#[derive(Debug)]
pub(crate) struct TargetAnswer {
    pub(crate) status: u16,
    /// The wire error code of a refusal, from `x-amzn-errortype`; empty on a success.
    pub(crate) error_code: String,
    pub(crate) body: Vec<u8>,
}

/// The `requestHeaders` a `CreateUploadUrl` answered with — "the required headers when
/// uploading your resource". Shared with `crate::scan_completion`, which drives the same
/// documented upload.
pub(crate) fn required_headers(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .get("requestHeaders")
        .as_object()
        .unwrap_or_else(|| panic!("the response carries no requestHeaders: {}", answer.json))
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The host and the path-with-query of an absolute URL.
fn split_url(url: &str) -> (&str, &str) {
    let rest = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"))
        .1;
    match rest.find('/') {
        Some(mark) => (&rest[..mark], &rest[mark..]),
        None => (rest, "/"),
    }
}

/// Address a published upload target exactly as a client does: `method` against the host
/// and path the URL names, with the headers the response said are required, and with NO
/// credential of any kind — a pre-signed URL is followed unsigned.
pub(crate) async fn send_to_target(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    body: Vec<u8>,
) -> TargetAnswer {
    let (host, path) = split_url(url);
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let request = request
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    TargetAnswer {
        status,
        error_code,
        body: response.into_body().to_vec(),
    }
}

/// Perform the documented upload: `PUT` the archive to the published target.
pub(crate) async fn upload(
    router: &MultiServiceRouter,
    url: &str,
    headers: &BTreeMap<String, String>,
    body: Vec<u8>,
) -> TargetAnswer {
    send_to_target(router, "PUT", url, headers, body).await
}

/// `CreateUploadUrl` signed for `region`, so a case can drive a credential scope other
/// than the suite's.
async fn create_upload_url_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    scan_name: &str,
) -> Answer {
    let host = format!("codeguru-security.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri("https://{host}/uploadUrl".replace("{host}", &host))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/codeguru-security/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::from(json!({ "scanName": scan_name }).to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let json: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Answer {
        status,
        error_type: String::new(),
        json,
    }
}

/// The resource held against `artifact` in the shared store.
pub(crate) fn stored_upload(
    engine: &Arc<engine::InMemoryEngine>,
    artifact: &str,
) -> Option<UploadedResource> {
    store_of(engine)
        .lock()
        .upload_artifacts
        .get(artifact)
        .and_then(|held| held.upload.clone())
}

/// Lowercase hex SHA-256 of `bytes`, computed here so a case checks the digest the store
/// recorded against an independently taken one.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Whether `held` is this store's record of the archive `bytes`: the length of the whole
/// archive, a digest over every one of its bytes, and the leading content retained of it
/// (all of it for an archive no longer than what one target retains).
pub(crate) fn is_record_of(held: &UploadedResource, bytes: &[u8]) -> bool {
    let retained = bytes.len().min(UPLOAD_RETAINED_PREFIX_MAX_BYTES);
    held.byte_count == u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        && held.content_sha256 == sha256_hex(bytes)
        && held.retained_prefix == bytes[..retained]
}

/// How `held` reads in an assertion message.
pub(crate) fn describe(held: &UploadedResource) -> String {
    format!(
        "{} bytes, sha256 {}, {} retained",
        held.byte_count,
        held.content_sha256,
        held.retained_prefix.len()
    )
}

/// `GetScan` for the latest run of `scan_name`.
pub(crate) async fn settled_run(router: &MultiServiceRouter, scan_name: &str) -> SettledRun {
    let answer = call(router, "GET", &format!("/scans/{scan_name}"), None).await;
    SettledRun {
        status: answer.status,
        scan_state: answer
            .get("scanState")
            .as_str()
            .unwrap_or_default()
            .to_string(),
        error_message: answer
            .get("errorMessage")
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Wait past a Standard run's clock-owed transition out of `InProgress`, so the next read
/// observes the settled run. The handlers store the due instant rather than sleeping, so
/// only the test waits.
pub(crate) async fn wait_for_settle() {
    let span = u64::try_from(STANDARD_SCAN_SETTLE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span + 150)).await;
}

/// `n` bytes of a zip-shaped archive: the local-file-header signature every zip starts
/// with (`PK\x03\x04`), then filler, so a case's bytes are distinguishable and are not
/// text.
pub(crate) fn archive_of(len: usize, seed: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len);
    for index in 0..len {
        bytes.push(match index {
            0 => b'P',
            1 => b'K',
            2 => 3,
            3 => 4,
            _ => seed.wrapping_add((index % 251) as u8),
        });
    }
    bytes
}

// ---------------------------------------------------------------------------------
// The published target receives the documented upload
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_published_target_receives_every_shape_of_resource_and_holds_it_for_the_scan() {
    let (engine, router) = harness();
    // Rows span what the rule generalises over: the SIZE of the uploaded archive (empty,
    // one byte, a few KiB, and either side of what one target retains) and the scan name
    // the target was minted for (each character class `ScanName`'s pattern
    // `^[a-zA-Z0-9-_$:.]*$` allows, and its modelled maximum length of 140).
    let long_name = "n".repeat(140);
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("an empty archive", "a", Vec::new()),
        ("one byte", "9", vec![b'P']),
        ("a small archive", "demo-scan", archive_of(64, 7)),
        ("a few KiB of source", "with_underscore", archive_of(4096, 19)),
        ("a dollar sign in the name", "dollar$sign", archive_of(1024, 31)),
        ("a colon in the name", "colon:separated", archive_of(999, 47)),
        ("a dot in the name", "dot.separated", archive_of(1, 53)),
        ("mixed case", "MixedCase0123", archive_of(2048, 61)),
        (
            "the longest allowed name",
            long_name.as_str(),
            archive_of(128, 71),
        ),
        (
            "exactly what one target retains",
            "at-the-retained-prefix",
            archive_of(UPLOAD_RETAINED_PREFIX_MAX_BYTES, 83),
        ),
        (
            "past what one target retains",
            "over-the-retained-prefix",
            archive_of(UPLOAD_RETAINED_PREFIX_MAX_BYTES + 1, 89),
        ),
    ];

    let mut artifacts: Vec<(&str, &str, String)> = Vec::new();
    for (case, scan_name, bytes) in &cases {
        let minted = create_upload_url(&router, scan_name).await;
        assert_eq!(
            minted.status, 200,
            "case {case:?}: CreateUploadUrl expected 200, got {} {}",
            minted.status, minted.json
        );
        let artifact = minted.str_of("codeArtifactId");
        let url = minted.str_of("s3Url");

        // The published URL is the target this emulator serves for THIS artifact.
        assert_eq!(
            url,
            cgs_state::upload_url(REGION, scan_name, &artifact),
            "case {case:?}: s3Url must be the upload target of this artifact, got {url:?}"
        );
        let (host, _path) = split_url(&url);
        assert!(
            cgs_state::is_upload_host(host),
            "case {case:?}: s3Url host {host:?} is not a host this emulator serves"
        );

        // The documented upload, with the headers the response said are required.
        let headers = required_headers(&minted);
        let answer = upload(&router, &url, &headers, bytes.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: the documented upload must be accepted, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            String::from_utf8_lossy(&answer.body)
        );

        // It is held, byte for byte accounted for, against the artifact the URL names.
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("case {case:?}: the target holds no resource"));
        assert_eq!(
            held.byte_count,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            "case {case:?}: expected {} bytes received, got {}",
            bytes.len(),
            describe(&held)
        );
        assert!(
            is_record_of(&held, bytes),
            "case {case:?}: the held resource is not the uploaded archive, got {}",
            describe(&held)
        );
        assert_eq!(
            held.content_type.as_deref(),
            Some(ARCHIVE_CONTENT_TYPE),
            "case {case:?}: the media type the upload declared must be recorded, got {:?}",
            held.content_type
        );

        artifacts.push((case, scan_name, artifact));
    }

    // A scan started on an uploaded resource HAD a resource to analyse, so its run is
    // carried out and ends in the modelled successful state, reporting no error.
    for (case, scan_name, artifact) in &artifacts {
        let created = create_scan(&router, json!({ "scanName": scan_name }), artifact).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateScan on the uploaded artifact expected 200, got {} {}",
            created.status, created.json
        );
    }
    wait_for_settle().await;
    for (case, scan_name, _artifact) in &artifacts {
        let run = settled_run(&router, scan_name).await;
        assert_eq!(
            run.status, 200,
            "case {case:?}: GetScan expected 200, got {run:?}"
        );
        assert_eq!(
            run.scan_state, SCAN_STATE_SUCCESSFUL,
            "case {case:?}: a run started on an uploaded resource completes, got {run:?}"
        );
        assert_eq!(
            run.error_message, "",
            "case {case:?}: a run that did not fail carries no errorMessage, got {run:?}"
        );
    }
}

#[tokio::test]
async fn a_run_started_on_a_target_nothing_was_uploaded_to_reports_the_missing_resource() {
    let (engine, router) = harness();

    // Row 1: nothing was ever uploaded to the minted target.
    let never = create_upload_url(&router, "never-uploaded").await;
    let never_artifact = never.str_of("codeArtifactId");
    assert!(
        stored_upload(&engine, &never_artifact).is_none(),
        "a freshly minted target holds no resource yet"
    );
    let created = create_scan(
        &router,
        json!({ "scanName": "never-uploaded" }),
        &never_artifact,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateScan on a target nothing was uploaded to: expected 200, got {} {}",
        created.status, created.json
    );

    // Row 2: the upload happens only AFTER the run started, so that run still had no
    // resource — what a run reports is decided when it starts.
    let late = create_upload_url(&router, "uploaded-late").await;
    let late_artifact = late.str_of("codeArtifactId");
    let late_url = late.str_of("s3Url");
    let started = create_scan(
        &router,
        json!({ "scanName": "uploaded-late" }),
        &late_artifact,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "CreateScan before the upload: expected 200, got {} {}",
        started.status, started.json
    );
    let answer = upload(
        &router,
        &late_url,
        &required_headers(&late),
        archive_of(32, 11),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the late upload must still be accepted, got {} {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    wait_for_settle().await;
    for case in ["never-uploaded", "uploaded-late"] {
        let run = settled_run(&router, case).await;
        assert_eq!(
            run.error_message, SCAN_RESOURCE_NOT_UPLOADED_MESSAGE,
            "case {case:?}: a run with no uploaded resource must report that, got {run:?}"
        );
        assert_eq!(
            run.scan_state, SCAN_STATE_FAILED,
            "case {case:?}: the run ends in the modelled failure state, got {run:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_upload_replaces_the_one_resource_a_target_holds() {
    let (engine, router) = harness();
    let minted = create_upload_url(&router, "retried-upload").await;
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");
    let headers = required_headers(&minted);

    // Rows: successive uploads to ONE target, differing in size and content, including a
    // shorter one after a longer one (a replacement, never an append).
    let revisions = [
        archive_of(128, 3),
        archive_of(4096, 5),
        archive_of(7, 9),
        Vec::new(),
    ];
    for (index, bytes) in revisions.iter().enumerate() {
        let answer = upload(&router, &url, &headers, bytes.clone()).await;
        assert_eq!(
            answer.status, 200,
            "upload {index}: expected 200, got {} {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("upload {index}: the target holds no resource"));
        assert!(
            is_record_of(&held, bytes),
            "upload {index}: the target must hold exactly the last archive uploaded, got {} \
             for {} bytes uploaded",
            describe(&held),
            bytes.len()
        );
    }

    // Two targets hold their own resource: writing one leaves the other alone.
    let other = create_upload_url(&router, "other-target").await;
    let other_artifact = other.str_of("codeArtifactId");
    let other_bytes = archive_of(321, 13);
    let answer = upload(
        &router,
        &other.str_of("s3Url"),
        &required_headers(&other),
        other_bytes.clone(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the second target's upload: expected 200, got {}",
        answer.status
    );
    let first = stored_upload(&engine, &artifact).expect("the first target holds a resource");
    let second =
        stored_upload(&engine, &other_artifact).expect("the second target holds a resource");
    assert!(
        is_record_of(&first, &[]) && is_record_of(&second, &other_bytes),
        "each target holds its own resource, got {} and {}",
        describe(&first),
        describe(&second)
    );
}

// ---------------------------------------------------------------------------------
// What the published target refuses
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_published_target_is_written_only_by_the_documented_method() {
    let (engine, router) = harness();
    let minted = create_upload_url(&router, "put-only").await;
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");
    let headers = required_headers(&minted);
    let held = archive_of(256, 17);
    assert_eq!(
        upload(&router, &url, &headers, held.clone()).await.status,
        200,
        "the documented upload must be accepted"
    );

    // Rows: every other method a client could send to the published URL.
    for method in ["GET", "HEAD", "POST", "DELETE", "PATCH"] {
        let answer = send_to_target(&router, method, &url, &headers, archive_of(16, 23)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (405, "MethodNotAllowed"),
            "case {method:?}: the published URL describes an upload, got {} {:?}",
            answer.status,
            answer.error_code
        );
        let still = stored_upload(&engine, &artifact).expect("the target still holds its resource");
        assert!(
            is_record_of(&still, &held),
            "case {method:?}: a refused request must change nothing, got {}",
            describe(&still)
        );
    }
}

#[tokio::test]
async fn a_path_that_names_no_target_of_this_account_is_refused() {
    let (engine, router) = harness();
    let minted = create_upload_url(&router, "real-target").await;
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");
    let headers = required_headers(&minted);
    let account = engine::emulator_account_id();
    let host = cgs_state::upload_host(REGION);

    // An artifact whose minted lifetime the clock has already ended: its target is
    // released, so nothing can be uploaded to it.
    let expired_scan = "expired-target";
    let expired_artifact = "11111111-2222-3333-4444-555555555555";
    let now = cgs_state::now_millis();
    store_of(&engine).lock().record_artifact(UploadArtifact {
        code_artifact_id: expired_artifact.to_string(),
        scan_name: expired_scan.to_string(),
        s3_url: cgs_state::upload_url(REGION, expired_scan, expired_artifact),
        request_headers: BTreeMap::new(),
        created_at_millis: now - UPLOAD_ARTIFACT_TTL_MILLIS - 1,
        expires_at_millis: now - 1,
        upload: None,
    });

    // Rows: each way a path on the upload host can fail to name a target of this
    // account, and the one target that does (the control row).
    let unknown_artifact = "00000000-0000-0000-0000-000000000000";
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a code artifact this emulator never minted",
            format!("https://{host}/{account}/real-target/{unknown_artifact}"),
            404,
            "NoSuchKey",
        ),
        (
            "a real artifact under another scan's name",
            format!("https://{host}/{account}/another-scan/{artifact}"),
            404,
            "NoSuchKey",
        ),
        (
            "a real artifact under another account",
            format!("https://{host}/111122223333/real-target/{artifact}"),
            404,
            "NoSuchKey",
        ),
        (
            "a target whose lifetime has ended",
            format!("https://{host}/{account}/{expired_scan}/{expired_artifact}"),
            404,
            "NoSuchKey",
        ),
        ("the target itself", url.clone(), 200, ""),
    ];
    for (case, candidate, status, code) in &cases {
        let answer = upload(&router, candidate, &headers, archive_of(48, 29)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code:?}, got {} {:?}",
            answer.status,
            answer.error_code
        );
    }
    // Only the control row's target holds anything.
    assert!(
        stored_upload(&engine, &artifact)
            .is_some_and(|held| is_record_of(&held, &archive_of(48, 29))),
        "the one named target holds the archive"
    );
    assert!(
        stored_upload(&engine, unknown_artifact).is_none()
            && stored_upload(&engine, expired_artifact).is_none(),
        "a refused upload stores nothing"
    );

    // A path on the upload host that is not a target path at all is not served as one:
    // the router answers its routing refusal, never a success.
    for (case, path) in [
        ("too few segments", format!("/{account}/real-target")),
        (
            "too many segments",
            format!("/{account}/real-target/{artifact}/extra"),
        ),
        ("an empty segment", format!("/{account}//{artifact}")),
        ("the host root", "/".to_string()),
    ] {
        let request = http::Request::builder()
            .method("PUT")
            .uri(path.clone())
            .header("host", host.clone())
            .body(Bytes::from(archive_of(8, 37)))
            .expect("a well-formed test request");
        let status = router.dispatch(request).await.status().as_u16();
        assert!(
            status != 200,
            "case {case:?}: {path} is not an upload target, got {status}"
        );
    }
}

// The size of the archive is NOT one of this plane's refusals; `crate::upload_size`
// asserts what the target does with an archive of any size.

// ---------------------------------------------------------------------------------
// The published URL is followable as a pre-signed URL is
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_published_target_is_served_without_a_credential_on_an_authenticating_router() {
    let (engine, router) = harness();
    let minted = create_upload_url(&router, "unsigned-upload").await;
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");
    let headers = required_headers(&minted);

    // A second router over the SAME engine, with request authentication ON, as the
    // emulator serves it.
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(EmulatedService::new(Arc::clone(&engine)));

    // A modelled operation with no credential is refused there — so authentication is
    // genuinely on for this router.
    let unsigned_operation = http::Request::builder()
        .method("POST")
        .uri("/uploadUrl")
        .header("host", format!("codeguru-security.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .body(Bytes::from(json!({ "scanName": "unsigned" }).to_string()))
        .expect("a well-formed test request");
    let refused = authenticating.dispatch(unsigned_operation).await;
    assert!(
        refused.status().as_u16() >= 400,
        "an unsigned modelled operation must be refused, got {}",
        refused.status()
    );

    // The published upload target, followed with no credential of any kind, is served:
    // that is what makes it a pre-signed URL.
    let bytes = archive_of(512, 59);
    let answer = upload(&authenticating, &url, &headers, bytes.clone()).await;
    assert_eq!(
        answer.status, 200,
        "an unsigned upload to the published target must be served, got {} {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        stored_upload(&engine, &artifact).is_some_and(|held| is_record_of(&held, &bytes)),
        "the unsigned upload is held against the artifact"
    );
}

#[tokio::test]
async fn a_credential_scope_that_names_no_region_still_publishes_a_served_target() {
    let (engine, router) = harness();
    // Rows: the region of the request's credential scope. A Region-shaped scope names the
    // host it was signed for; anything else cannot shape a host, so the emulator's
    // documented default Region does.
    let cases: [(&str, &str); 5] = [
        ("us-west-2", "us-west-2"),
        ("eu-central-1", "eu-central-1"),
        ("ap-southeast-3", "ap-southeast-3"),
        ("NOT_A_REGION", DEFAULT_REGION),
        ("evil.example.com", DEFAULT_REGION),
    ];
    for (index, (scope, expected_region)) in cases.iter().enumerate() {
        let scan_name = format!("scope-{index}");
        let minted = create_upload_url_signed_for(&router, scope, &scan_name).await;
        assert_eq!(
            minted.status, 200,
            "case {scope:?}: CreateUploadUrl expected 200, got {} {}",
            minted.status, minted.json
        );
        let artifact = minted.str_of("codeArtifactId");
        let url = minted.str_of("s3Url");
        let (host, _path) = split_url(&url);
        assert_eq!(
            host,
            cgs_state::upload_host(expected_region),
            "case {scope:?}: expected the upload host of {expected_region}, got {host:?}"
        );
        let bytes = archive_of(64 + index, 67);
        let answer = upload(&router, &url, &required_headers(&minted), bytes.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {scope:?}: the published target must be served, got {} {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            stored_upload(&engine, &artifact).is_some_and(|held| is_record_of(&held, &bytes)),
            "case {scope:?}: the upload is held against the artifact"
        );
    }

    // A host that is not the published upload host is not this service's: nothing is
    // served for it.
    let request = http::Request::builder()
        .method("PUT")
        .uri("/000000000000/scope-0/not-a-target")
        .header("host", "codeguru-security-upload.NOT_A_REGION.amazonaws.com")
        .body(Bytes::from(archive_of(8, 71)))
        .expect("a well-formed test request");
    let status = router.dispatch(request).await.status().as_u16();
    assert!(
        status != 200,
        "a host this service does not publish must not be served, got {status}"
    );
}
