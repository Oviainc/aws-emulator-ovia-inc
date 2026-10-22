// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CodeGuru Security scan lifecycle:
//! `CreateUploadUrl` -> `CreateScan` -> `GetScan` / `ListScans` -> `GetFindings` ->
//! `BatchGetFindings`.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), so the
//! restJson1 wire shape, the HTTP status of each modelled error and the shared engine
//! store are all exercised exactly as a client would exercise them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::Finding;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    self as cgs_state, Scan, ScanRun, Store, UploadArtifact, ANALYSIS_TYPE_SECURITY,
    EXPRESS_SCAN_SETTLE_MILLIS, QUOTA_CODE_RUNS, QUOTA_CODE_SCANS, QUOTA_CODE_UPLOAD_ARTIFACTS,
    RUNS_PER_SCAN_QUOTA, SCAN_QUOTA, SCAN_STATE_FAILED, SCAN_STATE_IN_PROGRESS, SCAN_TYPE_EXPRESS,
    SCAN_TYPE_STANDARD, STANDARD_SCAN_SETTLE_MILLIS, UPLOAD_ARTIFACT_QUOTA,
    UPLOAD_ARTIFACT_TTL_MILLIS,
};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region of the test requests' SigV4 credential scope. Deliberately not the
/// emulator's `us-east-1` default, so an ARN built from the request's own region is
/// distinguishable from one built from the default.
pub(crate) const REGION: &str = "us-west-2";

/// The `maxResults` ceiling `ListScans` models (`ListScansRequest$maxResults` range
/// 1..100), used as the page size when a request names none.
const LIST_SCANS_MAX_PAGE: usize = 100;

/// One dispatched request's answer. Shared with the sibling suites of this test target
/// (`crate::account_configuration`, `crate::resource_tagging`), so every suite reads a
/// response the same way.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The wire error code, from `x-amzn-errortype` or the body's `__type`; empty on a
    /// success.
    pub(crate) error_type: String,
    pub(crate) json: Value,
}

impl Answer {
    pub(crate) fn get(&self, member: &str) -> &Value {
        &self.json[member]
    }

    pub(crate) fn str_of(&self, member: &str) -> String {
        self.json[member]
            .as_str()
            .unwrap_or_else(|| panic!("member {member} is not a string in {}", self.json))
            .to_string()
    }
}

/// The router every test drives: this one service, on a fresh shared engine. The engine
/// is handed back too, so a test can seed the store with state no request path produces
/// (findings a real run's analysis would have written, or a store already at its quota).
pub(crate) fn harness() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// This service's store in `engine`.
pub(crate) fn store_of(engine: &Arc<engine::InMemoryEngine>) -> engine::ServiceState<Store> {
    cgs_state::service_state(&**engine).expect("the CodeGuru Security store")
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("codeguru-security.{REGION}.amazonaws.com");
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path_and_query}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/codeguru-security/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let json: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let error_type = if error_type.is_empty() {
        json["__type"].as_str().unwrap_or_default().to_string()
    } else {
        error_type
    };
    Answer {
        status,
        error_type,
        json,
    }
}

pub(crate) async fn create_upload_url(router: &MultiServiceRouter, scan_name: &str) -> Answer {
    call(
        router,
        "POST",
        "/uploadUrl",
        Some(json!({ "scanName": scan_name })),
    )
    .await
}

/// Mint an artifact for `scan_name` and return its `codeArtifactId`.
pub(crate) async fn artifact_for(router: &MultiServiceRouter, scan_name: &str) -> String {
    let answer = create_upload_url(router, scan_name).await;
    assert_eq!(
        answer.status, 200,
        "CreateUploadUrl({scan_name}): expected 200, got {} {}",
        answer.status, answer.json
    );
    answer.str_of("codeArtifactId")
}

pub(crate) async fn create_scan(
    router: &MultiServiceRouter,
    mut body: Value,
    artifact: &str,
) -> Answer {
    body["resourceId"] = json!({ "codeArtifactId": artifact });
    call(router, "POST", "/scans", Some(body)).await
}

/// `CreateUploadUrl` + `CreateScan` for `scan_name`, asserting both succeeded.
async fn start_scan(router: &MultiServiceRouter, scan_name: &str) -> Answer {
    let artifact = artifact_for(router, scan_name).await;
    let answer = create_scan(router, json!({ "scanName": scan_name }), &artifact).await;
    assert_eq!(
        answer.status, 200,
        "CreateScan({scan_name}): expected 200, got {} {}",
        answer.status, answer.json
    );
    answer
}

async fn get_scan(router: &MultiServiceRouter, scan_name: &str, run_id: Option<&str>) -> Answer {
    let query = run_id.map(|id| format!("?runId={id}")).unwrap_or_default();
    call(router, "GET", &format!("/scans/{scan_name}{query}"), None).await
}

async fn get_findings(router: &MultiServiceRouter, scan_name: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/findings/{scan_name}{query}"),
        None,
    )
    .await
}

async fn list_scans(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/scans{query}"), None).await
}

async fn batch_get_findings(router: &MultiServiceRouter, identifiers: Value) -> Answer {
    call(
        router,
        "POST",
        "/batchGetFindings",
        Some(json!({ "findingIdentifiers": identifiers })),
    )
    .await
}

/// Wait past the clock-owed transition of a run of `scan_type`, so the next read
/// observes the settled run.
async fn wait_for_settle(scan_type: &str) {
    let span = if scan_type == SCAN_TYPE_EXPRESS {
        EXPRESS_SCAN_SETTLE_MILLIS
    } else {
        STANDARD_SCAN_SETTLE_MILLIS
    };
    tokio::time::sleep(Duration::from_millis(u64::try_from(span).unwrap_or(500) + 150)).await;
}

/// The `Uuid` shape's pattern:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_modelled_uuid(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// The `ScanNameArn` shape's pattern:
/// `^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$`.
pub(crate) fn expected_scan_arn(scan_name: &str) -> String {
    format!("arn:aws:codeguru-security:{REGION}:000000000000:scans/{scan_name}")
}

// ---------------------------------------------------------------------------------
// The happy path, end to end
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn upload_url_then_scan_round_trips_through_get_scan_list_scans_and_findings() {
    let (_engine, router) = harness();
    let scan_name = "demo-scan";

    // 1. CreateUploadUrl mints and records an artifact.
    let upload = create_upload_url(&router, scan_name).await;
    assert_eq!(
        upload.status, 200,
        "CreateUploadUrl: expected 200, got {} {}",
        upload.status, upload.json
    );
    let artifact = upload.str_of("codeArtifactId");
    assert!(
        is_modelled_uuid(&artifact),
        "CreateUploadUrl: codeArtifactId {artifact:?} does not match the Uuid pattern"
    );
    assert!(
        upload.str_of("s3Url").contains(&artifact),
        "CreateUploadUrl: s3Url {:?} does not identify artifact {artifact}",
        upload.str_of("s3Url")
    );
    assert!(
        upload
            .get("requestHeaders")
            .as_object()
            .is_some_and(|headers| !headers.is_empty()),
        "CreateUploadUrl: requestHeaders is required and must carry the upload headers, got {}",
        upload.get("requestHeaders")
    );

    // 2. CreateScan records a scan against that artifact.
    let created = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
    assert_eq!(
        created.status, 200,
        "CreateScan: expected 200, got {} {}",
        created.status, created.json
    );
    let run_id = created.str_of("runId");
    assert!(
        is_modelled_uuid(&run_id),
        "CreateScan: runId {run_id:?} does not match the Uuid pattern"
    );
    assert_eq!(
        created.str_of("scanState"),
        SCAN_STATE_IN_PROGRESS,
        "CreateScan: a run that has just started is InProgress, got {}",
        created.json
    );
    assert_eq!(
        created.str_of("scanNameArn"),
        expected_scan_arn(scan_name),
        "CreateScan: scanNameArn must be built from the request's region and the canonical \
         account, got {}",
        created.json
    );
    assert_eq!(
        created.get("resourceId")["codeArtifactId"].as_str(),
        Some(artifact.as_str()),
        "CreateScan: resourceId must name the artifact that was scanned, got {}",
        created.get("resourceId")
    );

    // 3. GetScan reads the SAME record back.
    let got = get_scan(&router, scan_name, None).await;
    assert_eq!(
        got.status, 200,
        "GetScan: expected 200, got {} {}",
        got.status, got.json
    );
    assert_eq!(
        got.str_of("runId"),
        run_id,
        "GetScan: must report the runId CreateScan created, got {}",
        got.json
    );
    assert_eq!(
        got.str_of("scanNameArn"),
        expected_scan_arn(scan_name),
        "GetScan: scanNameArn must match CreateScan's, got {}",
        got.json
    );
    assert_eq!(
        got.str_of("analysisType"),
        ANALYSIS_TYPE_SECURITY,
        "GetScan: analysisType defaults to Security when CreateScan named none, got {}",
        got.json
    );
    assert_eq!(
        got.get("numberOfRevisions").as_i64(),
        Some(0),
        "GetScan: a scan that has never been re-run reports 0 revisions, got {}",
        got.json
    );
    assert!(
        got.get("createdAt").is_number(),
        "GetScan: createdAt is required, got {}",
        got.json
    );
    assert_eq!(
        got.str_of("scanState"),
        SCAN_STATE_IN_PROGRESS,
        "GetScan: the run is still InProgress straight after CreateScan, got {}",
        got.json
    );

    // Reading twice must not change what the next caller sees.
    let again = get_scan(&router, scan_name, None).await;
    assert_eq!(
        (again.get("createdAt"), again.get("updatedAt")),
        (got.get("createdAt"), got.get("updatedAt")),
        "GetScan: two reads of one scan must report the same timestamps, got {} then {}",
        got.json,
        again.json
    );

    // 4. ListScans builds its summary from the same record.
    let listed = list_scans(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "ListScans: expected 200, got {} {}",
        listed.status, listed.json
    );
    let summaries = listed.get("summaries").as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "ListScans: one scan exists, got {}",
        listed.json
    );
    assert_eq!(
        (
            summaries[0]["scanName"].as_str(),
            summaries[0]["runId"].as_str(),
            summaries[0]["scanNameArn"].as_str(),
        ),
        (
            Some(scan_name),
            Some(run_id.as_str()),
            Some(expected_scan_arn(scan_name).as_str()),
        ),
        "ListScans: the summary must come from the created scan, got {}",
        summaries[0]
    );
    assert!(
        listed.get("nextToken").is_null(),
        "ListScans: nextToken is absent on the last page, got {}",
        listed.json
    );

    // 5. GetFindings conflicts while the run has not finished.
    let in_progress = get_findings(&router, scan_name, "?status=All").await;
    assert_eq!(
        in_progress.status, 409,
        "GetFindings on an InProgress run: expected 409 ConflictException, got {} {}",
        in_progress.status, in_progress.json
    );
    assert!(
        in_progress.error_type.contains("ConflictException"),
        "GetFindings on an InProgress run: expected ConflictException, got {:?}",
        in_progress.error_type
    );

    // 6. Once the run's transition falls due, it is reported as settled.
    wait_for_settle(SCAN_TYPE_STANDARD).await;
    let settled = get_scan(&router, scan_name, Some(&run_id)).await;
    assert_eq!(
        settled.status, 200,
        "GetScan(runId): expected 200, got {} {}",
        settled.status, settled.json
    );
    assert_eq!(
        settled.str_of("scanState"),
        SCAN_STATE_FAILED,
        "GetScan: a run whose analysis this emulator cannot perform ends in the modelled \
         failure state, got {}",
        settled.json
    );
    assert!(
        !settled.str_of("errorMessage").is_empty(),
        "GetScan: a Failed run must say why, got {}",
        settled.json
    );
    assert!(
        settled.get("updatedAt").is_number(),
        "GetScan: a Standard scan reports updatedAt once it has changed, got {}",
        settled.json
    );

    // 7. GetFindings now reports the stored findings of that scan.
    let findings = get_findings(&router, scan_name, "?status=All").await;
    assert_eq!(
        findings.status, 200,
        "GetFindings on a settled run: expected 200, got {} {}",
        findings.status, findings.json
    );
    let listed_findings = findings.get("findings").as_array().cloned().unwrap_or_default();
    assert!(
        listed_findings.is_empty(),
        "GetFindings: a run nothing analyzed produced no findings, got {}",
        findings.json
    );
    assert!(
        findings.get("nextToken").is_null(),
        "GetFindings: nextToken is absent on the last page, got {}",
        findings.json
    );

    // 8. BatchGetFindings resolves against the same store: every id GetFindings did not
    //    list is a failedFindings entry, not a 404 for the whole call.
    let batch = batch_get_findings(
        &router,
        json!([{ "scanName": scan_name, "findingId": "does-not-exist" }]),
    )
    .await;
    assert_eq!(
        batch.status, 200,
        "BatchGetFindings: expected 200, got {} {}",
        batch.status, batch.json
    );
    assert_eq!(
        batch.get("findings").as_array().map(Vec::len),
        Some(0),
        "BatchGetFindings: no finding is stored, got {}",
        batch.json
    );
    assert_eq!(
        batch.get("failedFindings")[0]["errorCode"].as_str(),
        Some("ITEM_DOES_NOT_EXIST"),
        "BatchGetFindings: an unknown findingId of a known scan is ITEM_DOES_NOT_EXIST, got {}",
        batch.json
    );
}

// ---------------------------------------------------------------------------------
// CreateUploadUrl over the span of names the model allows
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn create_upload_url_records_a_distinct_artifact_for_every_allowed_scan_name() {
    let (engine, router) = harness();
    // Rows span the `ScanName` shape: length 1 to its modelled maximum of 140, and each
    // character class its pattern `^[a-zA-Z0-9-_$:.]*$` allows.
    let long_name = "n".repeat(140);
    let cases: &[&str] = &[
        "a",
        "9",
        "demo-scan",
        "with_underscore",
        "dollar$sign",
        "colon:separated",
        "dot.separated",
        "MixedCase0123",
        &long_name,
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for case in cases {
        let answer = create_upload_url(&router, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {}",
            answer.status, answer.json
        );
        let artifact = answer.str_of("codeArtifactId");
        assert!(
            is_modelled_uuid(&artifact),
            "case {case:?}: codeArtifactId {artifact:?} does not match the Uuid pattern"
        );
        let s3_url = answer.str_of("s3Url");
        assert!(
            !s3_url.is_empty() && s3_url.contains(case) && s3_url.contains(&artifact),
            "case {case:?}: s3Url must identify this scan's artifact, got {s3_url:?}"
        );
        assert!(
            minted.insert(artifact.clone()),
            "case {case:?}: codeArtifactId {artifact} was minted twice"
        );

        // The minted id is recorded, so CreateScan resolves it.
        let created = create_scan(&router, json!({ "scanName": case }), &artifact).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateScan on the minted artifact: expected 200, got {} {}",
            created.status, created.json
        );
    }

    // Two calls for ONE name mint two distinct artifacts: they are two revisions.
    let first = artifact_for(&router, "repeat-me").await;
    let second = artifact_for(&router, "repeat-me").await;
    assert_ne!(
        first, second,
        "two CreateUploadUrl calls for one scan name must mint distinct artifacts, got \
         {first} twice"
    );

    // `requestHeaders` comes out of the store: with no customer-managed key configured
    // it carries only the archive's content type, and once the account configuration
    // names one it carries the server-side-encryption headers that key requires.
    let plain = create_upload_url(&router, "unencrypted").await;
    assert_eq!(
        plain.get("requestHeaders"),
        &json!({ "Content-Type": "application/zip" }),
        "with no configured KMS key the upload needs no encryption header, got {}",
        plain.json
    );
    let key_arn = "arn:aws:kms:us-west-2:000000000000:key/22222222-3333-4444-5555-666666666666";
    store_of(&engine).lock().encryption_kms_key_arn = Some(key_arn.to_string());
    let encrypted = create_upload_url(&router, "encrypted").await;
    assert_eq!(
        encrypted.get("requestHeaders"),
        &json!({
            "Content-Type": "application/zip",
            "x-amz-server-side-encryption": "aws:kms",
            "x-amz-server-side-encryption-aws-kms-key-id": key_arn,
        }),
        "with a configured KMS key the upload must carry its encryption headers, got {}",
        encrypted.json
    );
}

// ---------------------------------------------------------------------------------
// The minted identifiers
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn minted_identifiers_are_unique_and_match_the_modelled_uuid_pattern() {
    // The `Uuid` shape's pattern is the rule; uniqueness is the invariant. Ten thousand
    // mints is far more than any one test makes through HTTP, so a minter that repeated
    // itself — or that emitted an out-of-pattern character — cannot pass.
    const MINTS: usize = 10_000;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for index in 0..MINTS {
        let minted = cgs_state::mint_uuid();
        assert!(
            is_modelled_uuid(&minted),
            "mint {index}: {minted:?} does not match the Uuid pattern"
        );
        assert!(
            seen.insert(minted.clone()),
            "mint {index}: {minted} was minted twice"
        );
    }
    assert_eq!(
        seen.len(),
        MINTS,
        "every mint must be distinct, got {} of {MINTS}",
        seen.len()
    );
}

// ---------------------------------------------------------------------------------
// The documented defaults, and the members each scan type reports
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn get_scan_reports_the_documented_defaults_for_every_scan_and_analysis_type() {
    let (_engine, router) = harness();
    // Rows: every (scanType, analysisType) the model allows, plus the absent case for
    // each. `scanType` "Defaults to Standard type if missing" and `analysisType`
    // "Defaults to Security type if missing" (CreateScanRequest docs).
    let cases: &[(Option<&str>, Option<&str>, &str, &str)] = &[
        (None, None, SCAN_TYPE_STANDARD, "Security"),
        (None, Some("All"), SCAN_TYPE_STANDARD, "All"),
        (Some("Standard"), None, SCAN_TYPE_STANDARD, "Security"),
        (Some("Standard"), Some("All"), SCAN_TYPE_STANDARD, "All"),
        (Some("Standard"), Some("Security"), SCAN_TYPE_STANDARD, "Security"),
        (Some("Express"), None, SCAN_TYPE_EXPRESS, "Security"),
        (Some("Express"), Some("All"), SCAN_TYPE_EXPRESS, "All"),
        (Some("Express"), Some("Security"), SCAN_TYPE_EXPRESS, "Security"),
    ];

    for (index, (scan_type, analysis_type, expected_type, expected_analysis)) in
        cases.iter().enumerate()
    {
        let case = format!("scanType={scan_type:?} analysisType={analysis_type:?}");
        let scan_name = format!("defaults-{index}");
        let artifact = artifact_for(&router, &scan_name).await;
        let mut body = json!({ "scanName": scan_name });
        if let Some(scan_type) = scan_type {
            body["scanType"] = json!(scan_type);
        }
        if let Some(analysis_type) = analysis_type {
            body["analysisType"] = json!(analysis_type);
        }
        let created = create_scan(&router, body, &artifact).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} {}",
            created.status, created.json
        );

        wait_for_settle(expected_type).await;
        let got = get_scan(&router, &scan_name, None).await;
        assert_eq!(
            got.str_of("analysisType"),
            *expected_analysis,
            "case {case}: analysisType must be the stored one, got {}",
            got.json
        );
        assert_eq!(
            got.str_of("scanState"),
            SCAN_STATE_FAILED,
            "case {case}: the run's clock-owed transition must have been materialised, got {}",
            got.json
        );
        // "`updatedAt` ... Only available for STANDARD scan types"
        // (GetScanResponse$updatedAt).
        if *expected_type == SCAN_TYPE_STANDARD {
            assert!(
                got.get("updatedAt").is_number(),
                "case {case}: a Standard scan reports updatedAt, got {}",
                got.json
            );
        } else {
            assert!(
                got.get("updatedAt").is_null(),
                "case {case}: updatedAt is only available for Standard scan types, got {}",
                got.json
            );
        }
    }
}

// ---------------------------------------------------------------------------------
// ListScans pagination
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn list_scans_pages_every_scan_exactly_once_for_every_page_size() {
    let (_engine, router) = harness();
    let names: Vec<String> = (0..7).map(|i| format!("paged-scan-{i}")).collect();
    for name in &names {
        start_scan(&router, name).await;
    }
    let mut expected: Vec<String> = names.clone();
    expected.sort();

    // Rows: page sizes below, at, and above the number of scans, plus the absent case
    // (which pages at the modelled maximum).
    let cases: &[Option<usize>] = &[Some(1), Some(2), Some(3), Some(6), Some(7), Some(100), None];
    for case in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case maxResults={case:?}: paging did not terminate after {pages} pages"
            );
            let mut query = String::new();
            if let Some(max) = case {
                query.push_str(&format!("?maxResults={max}"));
            }
            if let Some(token) = &token {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_scans(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={case:?}: expected 200, got {} {}",
                answer.status, answer.json
            );
            let page = answer.get("summaries").as_array().cloned().unwrap_or_default();
            let page_limit = case.unwrap_or(LIST_SCANS_MAX_PAGE);
            assert!(
                page.len() <= page_limit,
                "case maxResults={case:?}: a page carried {} summaries, more than {page_limit}",
                page.len()
            );
            for summary in page {
                seen.push(
                    summary["scanName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match answer.get("nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={case:?}: paging must yield every scan exactly once, in one \
             stable order"
        );
    }
}

// ---------------------------------------------------------------------------------
// BatchGetFindings classification
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_findings_classifies_every_identifier_it_cannot_resolve() {
    let (_engine, router) = harness();
    start_scan(&router, "known-scan").await;

    // Rows: each `ErrorCode` the store can justify for an unresolvable identifier.
    let identifiers = json!([
        { "scanName": "known-scan", "findingId": "absent-finding" },
        { "scanName": "no-such-scan", "findingId": "absent-finding" },
        { "scanName": "known-scan", "findingId": "absent-finding" },
    ]);
    let expected = ["ITEM_DOES_NOT_EXIST", "INVALID_SCAN_NAME", "DUPLICATE_IDENTIFIER"];

    let answer = batch_get_findings(&router, identifiers).await;
    assert_eq!(
        answer.status, 200,
        "BatchGetFindings: an unresolvable identifier is reported per entry, not as a \
         failure of the call; got {} {}",
        answer.status, answer.json
    );
    assert_eq!(
        answer.get("findings").as_array().map(Vec::len),
        Some(0),
        "BatchGetFindings: nothing resolves out of an empty finding store, got {}",
        answer.json
    );
    let failed = answer
        .get("failedFindings")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        failed.len(),
        expected.len(),
        "BatchGetFindings: one failedFindings entry per unresolvable identifier, got {}",
        answer.json
    );
    for (index, expected_code) in expected.iter().enumerate() {
        assert_eq!(
            failed[index]["errorCode"].as_str(),
            Some(*expected_code),
            "case identifier {index}: expected {expected_code}, got {}",
            failed[index]
        );
        assert!(
            !failed[index]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case identifier {index}: message is required, got {}",
            failed[index]
        );
    }
}

// ---------------------------------------------------------------------------------
// Findings of a scan: the status filter and the page cursor, over a POPULATED store
// ---------------------------------------------------------------------------------

/// A finding as a run's analysis would have stored it. Seeded directly into the store:
/// this emulator performs no analysis (see `state::SCAN_NOT_ANALYSED_MESSAGE`), so this
/// is the only way to exercise the readers over a non-empty list.
fn seeded_finding(finding_id: &str, status: &str) -> Finding {
    Finding {
        id: Some(finding_id.to_string()),
        status: Some(status.to_string()),
        title: Some(format!("seeded finding {finding_id}")),
        ..Finding::default()
    }
}

/// The ids of the findings in an answer, in the order the answer listed them.
fn finding_ids(answer: &Answer) -> Vec<String> {
    answer
        .get("findings")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| finding["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn get_findings_partitions_and_pages_the_stored_list() {
    let (engine, router) = harness();
    let scan_name = "seeded-scan";
    start_scan(&router, scan_name).await;
    wait_for_settle(SCAN_TYPE_STANDARD).await;

    // Rows of the store: both `Open` and `Closed` findings, interleaved, plus one
    // finding of ANOTHER scan that must never show up in this scan's answers.
    let seeded: &[(&str, &str)] = &[
        ("f-1", "Open"),
        ("f-2", "Closed"),
        ("f-3", "Open"),
        ("f-4", "Closed"),
        ("f-5", "Open"),
    ];
    {
        let store = store_of(&engine);
        let mut guard = store.lock();
        for (finding_id, status) in seeded {
            guard.findings.insert(
                (scan_name.to_string(), (*finding_id).to_string()),
                seeded_finding(finding_id, status),
            );
        }
        guard.findings.insert(
            ("other-scan".to_string(), "f-9".to_string()),
            seeded_finding("f-9", "Open"),
        );
    }

    // Rows: every value `Status` accepts ("Pass either Open, Closed, or All") and the
    // absent case, each with the ids it must select out of the stored list.
    let cases: &[(&str, &[&str])] = &[
        ("", &["f-1", "f-2", "f-3", "f-4", "f-5"]),
        ("?status=All", &["f-1", "f-2", "f-3", "f-4", "f-5"]),
        ("?status=Open", &["f-1", "f-3", "f-5"]),
        ("?status=Closed", &["f-2", "f-4"]),
    ];
    for (case, expected) in cases {
        let answer = get_findings(&router, scan_name, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {}",
            answer.status, answer.json
        );
        assert_eq!(
            finding_ids(&answer),
            *expected,
            "case {case:?}: the status filter must partition the stored list, got {}",
            answer.json
        );
        assert!(
            answer.get("nextToken").is_null(),
            "case {case:?}: one page held every match, so nextToken is absent, got {}",
            answer.json
        );
    }

    // Rows: page sizes below, at and above the match count, plus the modelled maximum.
    let all: Vec<String> = seeded
        .iter()
        .map(|(finding_id, _)| (*finding_id).to_string())
        .collect();
    for page_size in [1usize, 2, 3, 5, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            let mut query = format!("?status=All&maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let answer = get_findings(&router, scan_name, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {} {}",
                answer.status, answer.json
            );
            let page = finding_ids(&answer);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: a page carried {} findings",
                page.len()
            );
            seen.extend(page);
            match answer.get("nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case maxResults={page_size}: paging must yield every finding exactly once, in \
             findingId order"
        );
    }

    // And an id GetFindings listed resolves through BatchGetFindings, out of the same
    // store, while one it did not list fails per entry.
    let batch = batch_get_findings(
        &router,
        json!([
            { "scanName": scan_name, "findingId": "f-2" },
            { "scanName": scan_name, "findingId": "f-404" },
            { "scanName": scan_name, "findingId": "f-5" },
        ]),
    )
    .await;
    assert_eq!(
        batch.status, 200,
        "BatchGetFindings: expected 200, got {} {}",
        batch.status, batch.json
    );
    assert_eq!(
        finding_ids(&batch),
        vec!["f-2".to_string(), "f-5".to_string()],
        "BatchGetFindings: every resolvable identifier must resolve out of the store that \
         GetFindings pages, got {}",
        batch.json
    );
    let failed = batch
        .get("failedFindings")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        failed.len(),
        1,
        "BatchGetFindings: exactly one identifier was unresolvable, got {}",
        batch.json
    );
    assert_eq!(
        (
            failed[0]["findingId"].as_str(),
            failed[0]["errorCode"].as_str()
        ),
        (Some("f-404"), Some("ITEM_DOES_NOT_EXIST")),
        "BatchGetFindings: the unresolvable identifier must be reported as itself, got {}",
        failed[0]
    );
}

// ---------------------------------------------------------------------------------
// Every caller-driven store is bounded
// ---------------------------------------------------------------------------------

/// A settled run, as `CreateScan` plus its clock-owed transition would have left it.
fn settled_run(run_id: &str, now: i64) -> ScanRun {
    ScanRun {
        run_id: run_id.to_string(),
        code_artifact_id: format!("artifact-of-{run_id}"),
        scan_type: SCAN_TYPE_STANDARD.to_string(),
        analysis_type: ANALYSIS_TYPE_SECURITY.to_string(),
        scan_state: SCAN_STATE_FAILED.to_string(),
        created_at_millis: now,
        updated_at_millis: Some(now),
        error_message: Some("seeded".to_string()),
        settle_due_millis: now,
    }
}

#[tokio::test]
async fn each_caller_driven_store_refuses_growth_past_its_quota() {
    let now = cgs_state::now_millis();

    // 1. upload_artifacts: at its quota, CreateUploadUrl is refused.
    {
        let (engine, router) = harness();
        {
            let store = store_of(&engine);
            let mut guard = store.lock();
            for index in 0..UPLOAD_ARTIFACT_QUOTA {
                guard.record_artifact(UploadArtifact {
                    code_artifact_id: format!("seed-{index}"),
                    scan_name: "seeded".to_string(),
                    s3_url: "https://seeded.emulator.invalid/seed".to_string(),
                    request_headers: BTreeMap::new(),
                    created_at_millis: now,
                    expires_at_millis: now + UPLOAD_ARTIFACT_TTL_MILLIS,
                });
            }
        }
        let refused = create_upload_url(&router, "over-quota").await;
        assert_eq!(
            refused.status, 429,
            "CreateUploadUrl at the artifact quota: expected 429 ThrottlingException, got {} {}",
            refused.status, refused.json
        );
        assert_eq!(
            refused.get("quotaCode").as_str(),
            Some(QUOTA_CODE_UPLOAD_ARTIFACTS),
            "CreateUploadUrl at the artifact quota: the refusal must name the quota, got {}",
            refused.json
        );
    }

    // 2. scans: at its quota, CreateScan starts no new scan.
    {
        let (engine, router) = harness();
        let artifact = artifact_for(&router, "over-quota").await;
        {
            let store = store_of(&engine);
            let mut guard = store.lock();
            for index in 0..SCAN_QUOTA {
                let scan_name = format!("seed-scan-{index}");
                guard.scans.insert(
                    scan_name.clone(),
                    Scan {
                        scan_name: scan_name.clone(),
                        scan_name_arn: expected_scan_arn(&scan_name),
                        created_at_millis: now,
                        updated_at_millis: None,
                        tags: BTreeMap::new(),
                        runs: vec![settled_run("seed-run", now)],
                    },
                );
            }
        }
        let refused = create_scan(&router, json!({ "scanName": "over-quota" }), &artifact).await;
        assert_eq!(
            refused.status, 429,
            "CreateScan at the scan quota: expected 429 ThrottlingException, got {} {}",
            refused.status, refused.json
        );
        assert_eq!(
            refused.get("quotaCode").as_str(),
            Some(QUOTA_CODE_SCANS),
            "CreateScan at the scan quota: the refusal must name the quota, got {}",
            refused.json
        );
    }

    // 3. runs of one scan: at its quota, CreateScan starts no new revision.
    {
        let (engine, router) = harness();
        let scan_name = "many-revisions";
        let artifact = artifact_for(&router, scan_name).await;
        {
            let store = store_of(&engine);
            let mut guard = store.lock();
            guard.scans.insert(
                scan_name.to_string(),
                Scan {
                    scan_name: scan_name.to_string(),
                    scan_name_arn: expected_scan_arn(scan_name),
                    created_at_millis: now,
                    updated_at_millis: None,
                    tags: BTreeMap::new(),
                    runs: (0..RUNS_PER_SCAN_QUOTA)
                        .map(|index| settled_run(&format!("seed-run-{index}"), now))
                        .collect(),
                },
            );
        }
        let refused = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
        assert_eq!(
            refused.status, 429,
            "CreateScan at the revision quota: expected 429 ThrottlingException, got {} {}",
            refused.status, refused.json
        );
        assert_eq!(
            refused.get("quotaCode").as_str(),
            Some(QUOTA_CODE_RUNS),
            "CreateScan at the revision quota: the refusal must name the quota, got {}",
            refused.json
        );
    }

    // 4. a minted upload target is released once its lifetime has passed, so an
    //    artifact nobody scanned does not hold a slot for ever.
    {
        let (engine, router) = harness();
        let scan_name = "expired-artifact";
        let expired = "11111111-2222-3333-4444-555555555555";
        {
            let store = store_of(&engine);
            let mut guard = store.lock();
            guard.record_artifact(UploadArtifact {
                code_artifact_id: expired.to_string(),
                scan_name: scan_name.to_string(),
                s3_url: "https://seeded.emulator.invalid/expired".to_string(),
                request_headers: BTreeMap::new(),
                created_at_millis: now - UPLOAD_ARTIFACT_TTL_MILLIS - 1,
                expires_at_millis: now - 1,
            });
        }
        let refused = create_scan(&router, json!({ "scanName": scan_name }), expired).await;
        assert_eq!(
            refused.status, 404,
            "CreateScan on an expired upload target: expected 404 ResourceNotFoundException, \
             got {} {}",
            refused.status, refused.json
        );
        let store = store_of(&engine);
        assert!(
            store.lock().upload_artifacts.is_empty(),
            "the expired artifact must have been released from the store"
        );
    }
}

// ---------------------------------------------------------------------------------
// The modelled refusals
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn unknown_names_and_ids_are_the_modelled_refusals_not_a_501() {
    let (_engine, router) = harness();
    let started = start_scan(&router, "real-scan").await;
    let real_run = started.str_of("runId");

    // Rows: (what was asked for, the request, expected status, expected error code).
    let unknown_scan = get_scan(&router, "no-such-scan", None).await;
    let unknown_findings = get_findings(&router, "no-such-scan", "?status=All").await;
    let unknown_run = get_scan(
        &router,
        "real-scan",
        Some("00000000-0000-0000-0000-000000000000"),
    )
    .await;
    let unknown_artifact = create_scan(
        &router,
        json!({ "scanName": "real-scan" }),
        "00000000-0000-0000-0000-000000000000",
    )
    .await;
    // A token this service never issued: the readers hand back an opaque, hex-encoded
    // cursor, so anything else cannot be resumed from.
    let bad_scan_token = list_scans(&router, "?nextToken=not-a-token").await;
    let bad_finding_token = get_findings(&router, "real-scan", "?nextToken=not-a-token").await;
    let cases: &[(&str, &Answer, u16, &str)] = &[
        ("GetScan(unknown scanName)", &unknown_scan, 404, "ResourceNotFoundException"),
        (
            "GetFindings(unknown scanName)",
            &unknown_findings,
            404,
            "ResourceNotFoundException",
        ),
        ("GetScan(unknown runId)", &unknown_run, 404, "ResourceNotFoundException"),
        (
            "CreateScan(unknown codeArtifactId)",
            &unknown_artifact,
            404,
            "ResourceNotFoundException",
        ),
        ("ListScans(foreign nextToken)", &bad_scan_token, 400, "ValidationException"),
        (
            "GetFindings(foreign nextToken)",
            &bad_finding_token,
            400,
            "ValidationException",
        ),
    ];
    for (case, answer, status, code) in cases {
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} {}",
            answer.status, answer.json
        );
        assert!(
            answer.error_type.contains(code),
            "case {case}: expected {code}, got {:?} {}",
            answer.error_type,
            answer.json
        );
    }

    // The run that DOES exist still resolves, so the refusals above are not blanket.
    let found = get_scan(&router, "real-scan", Some(&real_run)).await;
    assert_eq!(
        found.status, 200,
        "GetScan(real runId): expected 200, got {} {}",
        found.status, found.json
    );

    // An artifact minted for one scan name cannot be scanned under another.
    let crossed = artifact_for(&router, "name-a").await;
    let refused = create_scan(&router, json!({ "scanName": "name-b" }), &crossed).await;
    assert_eq!(
        refused.status, 400,
        "CreateScan(artifact of another scanName): expected 400 ValidationException, got {} {}",
        refused.status, refused.json
    );
    assert!(
        refused.error_type.contains("ValidationException"),
        "CreateScan(artifact of another scanName): expected ValidationException, got {:?}",
        refused.error_type
    );

    // And a scan whose latest run has not finished cannot start another revision.
    let again = artifact_for(&router, "real-scan").await;
    let conflicted = create_scan(&router, json!({ "scanName": "real-scan" }), &again).await;
    assert_eq!(
        conflicted.status, 409,
        "CreateScan while a revision is InProgress: expected 409 ConflictException, got {} {}",
        conflicted.status, conflicted.json
    );
    assert!(
        conflicted.error_type.contains("ConflictException"),
        "CreateScan while a revision is InProgress: expected ConflictException, got {:?}",
        conflicted.error_type
    );
}

// ---------------------------------------------------------------------------------
// Revisions and idempotency
// ---------------------------------------------------------------------------------

#[tokio::test]
async fn client_token_is_idempotent_and_a_fresh_run_adds_one_revision() {
    let (_engine, router) = harness();
    let scan_name = "revised-scan";
    let artifact = artifact_for(&router, scan_name).await;

    // Rows: the same token repeated must answer with the same run, whatever else the
    // retry carries.
    let first = create_scan(
        &router,
        json!({ "scanName": scan_name, "clientToken": "token-one" }),
        &artifact,
    )
    .await;
    assert_eq!(
        first.status, 200,
        "CreateScan: expected 200, got {} {}",
        first.status, first.json
    );
    let run_id = first.str_of("runId");

    for (case, body) in [
        ("identical retry", json!({ "scanName": scan_name, "clientToken": "token-one" })),
        (
            "retry naming the defaults",
            json!({
                "scanName": scan_name,
                "clientToken": "token-one",
                "scanType": "Standard",
                "analysisType": "Security",
            }),
        ),
    ] {
        let retry = create_scan(&router, body, &artifact).await;
        assert_eq!(
            retry.status, 200,
            "case {case}: expected 200, got {} {}",
            retry.status, retry.json
        );
        assert_eq!(
            retry.str_of("runId"),
            run_id,
            "case {case}: an idempotent retry must answer with the original run, got {}",
            retry.json
        );
    }

    let after_retries = get_scan(&router, scan_name, None).await;
    assert_eq!(
        after_retries.get("numberOfRevisions").as_i64(),
        Some(0),
        "an idempotent retry must not add a revision, got {}",
        after_retries.json
    );

    // A genuinely new run on a new revision of the resource adds one revision.
    wait_for_settle(SCAN_TYPE_STANDARD).await;
    let revised_artifact = artifact_for(&router, scan_name).await;
    let revision = create_scan(
        &router,
        json!({ "scanName": scan_name, "clientToken": "token-two" }),
        &revised_artifact,
    )
    .await;
    assert_eq!(
        revision.status, 200,
        "CreateScan(second revision): expected 200, got {} {}",
        revision.status, revision.json
    );
    let revision_run = revision.str_of("runId");
    assert_ne!(
        revision_run, run_id,
        "a new revision must carry a fresh runId, got {revision_run} twice"
    );

    let got = get_scan(&router, scan_name, None).await;
    assert_eq!(
        got.get("numberOfRevisions").as_i64(),
        Some(1),
        "a scan re-run once reports 1 revision, got {}",
        got.json
    );
    assert_eq!(
        got.str_of("runId"),
        revision_run,
        "GetScan with no runId answers with the latest run, got {}",
        got.json
    );

    // The earlier run is still readable by its own id, with its own state.
    let earlier = get_scan(&router, scan_name, Some(&run_id)).await;
    assert_eq!(
        earlier.status, 200,
        "GetScan(first runId): expected 200, got {} {}",
        earlier.status, earlier.json
    );
    assert_eq!(
        earlier.str_of("runId"),
        run_id,
        "GetScan(runId) must answer with the run that was asked for, got {}",
        earlier.json
    );
    assert_eq!(
        earlier.str_of("scanState"),
        SCAN_STATE_FAILED,
        "the first run had already settled, got {}",
        earlier.json
    );
}
