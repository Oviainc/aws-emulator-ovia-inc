// SPDX-License-Identifier: CC0-1.0

//! The production router serves CloudTrail's `ListImportFailures`.
//!
//! This suite needs the WHOLE router: the symptom it pins is a request reaching the
//! emulator and being answered with the dispatcher's own `501 NotImplemented`
//! (`operation <full model id> is not implemented in this emulator`) rather than by
//! the operation's handler, which only the assembled, multi-service router can show —
//! a single-service test registers one service and cannot.
//!
//! The import registry is seeded through the shared engine: `StartImport`, the
//! operation that creates an import on AWS, is still a scaffold, so seeding stands in
//! for it and a 200 carrying the seeded failure proves the request reached the
//! `ListImportFailures` handler and read this engine's state.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use aws_emulator_router::{build_router, InMemoryEngine};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101 as cloudtrail;
use cloudtrail::ComAmazonawsCloudtrailCloudtrail20131101State as CloudTrailState;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `cloudtrail`, which is also the Region the
/// request is served in. The signature itself is never checked: the router here runs
/// with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudtrail/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=deadbeef";

/// The Region the credential scope above names.
const REGION: &str = "us-east-1";

/// The import the registry is seeded with — a 36-character `UUID`, as the model's
/// `ImportId` shape requires.
const IMPORT_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A well-formed import id the registry holds nothing for.
const ABSENT_IMPORT_ID: &str = "99999999-2222-4333-8444-555555555555";

/// The S3 location of the one seeded failure: what a 200 must report, so the answer
/// is a projection of state rather than anything the router invented.
const FAILURE_LOCATION: &str = "s3://import-bucket/AWSLogs/000000000000/0001.json.gz";

/// One awsJson1_1 `POST /` naming `target`, as a CloudTrail client sends it.
fn request(target: &str, body: String) -> ::http::Request<::bytes::Bytes> {
    ::http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .body(::bytes::Bytes::from(body))
        .expect("a well-formed request")
}

#[test]
fn the_router_serves_cloudtrail_list_import_failures() {
    std::thread::Builder::new()
        .name("cloudtrail-import-failures".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let created = UNIX_EPOCH + Duration::from_secs(1_759_000_000);
            // `ImportKey` is `(import id, Region)`: the import, and the failures it
            // recorded, are keyed alike.
            let key = (IMPORT_ID.to_string(), REGION.to_string());
            let state = engine.state::<CloudTrailState>();
            {
                let mut store = state.lock();
                store.imports.insert(
                    key.clone(),
                    cloudtrail::state::ImportRecord {
                        import_id: IMPORT_ID.to_string(),
                        destinations: vec![format!(
                            "arn:aws:cloudtrail:{REGION}:000000000000:eventdatastore/{IMPORT_ID}"
                        )],
                        import_source: None,
                        start_event_time: None,
                        end_event_time: None,
                        status: cloudtrail::state::IMPORT_STATUS_IN_PROGRESS.to_string(),
                        created_timestamp: created,
                        updated_timestamp: created,
                        // The `INITIALIZING` -> `IN_PROGRESS` move is due at this
                        // instant, which is immaterial here: the record is seeded
                        // already `IN_PROGRESS`.
                        ingests_at: created,
                        statistics: None,
                    },
                );
                store.import_failures.insert(
                    key,
                    vec![cloudtrail::state::ImportFailureRecord {
                        location: FAILURE_LOCATION.to_string(),
                        status: "FAILED".to_string(),
                        error_type: "InvalidLogFile".to_string(),
                        error_message: "The log file could not be read".to_string(),
                        last_updated_time: UNIX_EPOCH + Duration::from_secs(1_760_000_000),
                    }],
                );
            }

            let mut router = build_router(Arc::clone(&engine));
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The seeded import: the answer must carry its one recorded failure.
            let response = runtime.block_on(router.dispatch(request(
                "CloudTrail_20131101.ListImportFailures",
                format!(r#"{{"ImportId":"{IMPORT_ID}"}}"#),
            )));
            let status = response.status().as_u16();
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            assert_eq!(
                status, 200,
                "case the seeded import: expected HTTP 200 from the handler, got \
                 {status} with {rendered}"
            );
            let parsed: serde_json::Value =
                serde_json::from_str(&rendered).unwrap_or(serde_json::Value::Null);
            let failures = parsed
                .get("Failures")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("no Failures in {rendered}"))
                .clone();
            assert_eq!(
                (
                    failures.len(),
                    failures
                        .first()
                        .and_then(|failure| failure.get("Location"))
                        .and_then(serde_json::Value::as_str),
                ),
                (1, Some(FAILURE_LOCATION)),
                "case the seeded import: expected the one stored failure, got {rendered}"
            );

            // An import id the registry holds nothing for: the operation models no
            // not-found error, so the answer is a page of no failures.
            let response = runtime.block_on(router.dispatch(request(
                "CloudTrail_20131101.ListImportFailures",
                format!(r#"{{"ImportId":"{ABSENT_IMPORT_ID}"}}"#),
            )));
            let status = response.status().as_u16();
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            let parsed: serde_json::Value =
                serde_json::from_str(&rendered).unwrap_or(serde_json::Value::Null);
            assert_eq!(
                (status, parsed.get("Failures"), parsed.get("NextToken")),
                (200, Some(&serde_json::json!([])), None),
                "case an unknown import: expected HTTP 200 with an empty page and no \
                 token, got {status} with {rendered}"
            );
        })
        .expect("spawn the routing thread")
        .join()
        .expect("the router must serve CloudTrail's ListImportFailures");
}
