// SPDX-License-Identifier: CC0-1.0

//! That a start call resolves the BUCKET a bucket member names, end to end over HTTP.
//!
//! The RULE under test, one level more general than any single request: a start call reads
//! and writes the objects of every bucket member it is given, so each member is resolved
//! against object storage before anything is minted, and a member naming a bucket the
//! account does not hold is refused with 400 `ValidationException` under the missing-bucket
//! reason naming the submitted bucket, leaving no task and no assessment behind — whatever
//! the name looks like, a name being spelled the way the service's own buckets are spelled
//! included. The same request is accepted once the account holds a bucket of that name, and
//! the resource it mints reports the bucket it resolved.
//!
//! The rows span what the rule generalises over: which operation and which member carries
//! the name, and the shape of the name (the shortest the documented prefix admits, one
//! carrying dots, one carrying hyphens, one carrying digits, and one at `importS3Bucket`'s
//! `@length` max of 63) — every one of them inside the `migrationhub-strategy-` prefix, so
//! no row can be refused by the prefix rule instead (that rule, and which of the two
//! refusals a name earns, is tabled in `start_call_bucket_reachability.rs`).
//!
//! The emulator holds the S3 service beside Strategy Recommendations over ONE engine, as
//! the assembled emulator does: a bucket a row needs is created through S3's own front door
//! and the start call resolves it by calling S3 in process.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, get_import, get_latest_assessment_id, mount_object_storage, register_collector_in,
    start_assessment, start_import, Answer, TEST_REGION,
};
// The documented prefix, the refusal's wording and the task count come from the sibling
// table rather than from a second copy here, so both suites hold the service to the one
// prefix and the one sentence.
use crate::start_call_bucket_reachability::{
    create_bucket, import_task_count, missing_bucket_message, REQUIRED_PREFIX,
};

/// `importS3Bucket`'s `@length(0, 63)`, from
/// `smithy_query(query="shape",shape="importS3Bucket")`: the longest bucket name a start
/// call admits at all, so the longest one this table can probe.
const MAX_BUCKET_NAME: usize = 63;

/// The buckets the members a row is NOT probing name, held by the account throughout so
/// every row is decided on the member it is probing.
const HELD_IMPORT_BUCKET: &str = "migrationhub-strategy-held-import";
const HELD_ANALYSIS_BUCKET: &str = "migrationhub-strategy-held-analysis";

/// One emulator holding the S3 service beside Strategy Recommendations over one shared
/// engine, its account holding a registered data collector — so an assessment row is
/// decided on its bucket member rather than refused for having no data source — and the
/// buckets of [`HELD_IMPORT_BUCKET`] and [`HELD_ANALYSIS_BUCKET`] and nothing else.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    register_collector_in(&engine, &[TEST_REGION]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    mount_object_storage(
        &mut router,
        &engine,
        &[HELD_IMPORT_BUCKET, HELD_ANALYSIS_BUCKET],
    );
    router
}

/// Bucket names inside the documented prefix, differing in the shape of what follows it.
fn probe_names() -> Vec<String> {
    let longest = format!(
        "{REQUIRED_PREFIX}{}",
        "a".repeat(MAX_BUCKET_NAME - REQUIRED_PREFIX.len())
    );
    vec![
        // The shortest name the prefix admits at all.
        format!("{REQUIRED_PREFIX}a"),
        // Hyphens, dots and digits: every separator and class `importS3Bucket`'s
        // `@pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+` admits.
        format!("{REQUIRED_PREFIX}does-not-exist"),
        format!("{REQUIRED_PREFIX}import.data"),
        format!("{REQUIRED_PREFIX}2026"),
        longest,
    ]
}

/// Every member of every start call that names a bucket: which operation carries it, and
/// which member.
const MEMBERS: &[(&str, &str)] = &[
    ("StartImportFileTask", "S3Bucket"),
    ("StartImportFileTask", "s3bucketForReportData"),
    ("StartAssessment", "s3bucketForAnalysisData"),
    ("StartAssessment", "s3bucketForReportData"),
];

/// The request a row sends: its own member carries the probed name, every other member a
/// bucket the account holds.
fn request_for(operation: &str, member: &str, bucket: &str) -> Value {
    let mut request = match operation {
        "StartImportFileTask" => json!({
            "name": "bucket-existence-probe",
            "S3Bucket": HELD_IMPORT_BUCKET,
            "s3key": "inventory/servers.csv",
        }),
        "StartAssessment" => json!({"s3bucketForAnalysisData": HELD_ANALYSIS_BUCKET}),
        other => panic!("this table has no case for operation {other:?}"),
    };
    request[member] = json!(bucket);
    request
}

async fn dispatch(operation: &str, router: &MultiServiceRouter, request: Value) -> Answer {
    match operation {
        "StartImportFileTask" => start_import(router, request).await,
        "StartAssessment" => start_assessment(router, request).await,
        other => panic!("this table has no case for operation {other:?}"),
    }
}

/// A bucket member naming a bucket the account does not hold is refused as the missing
/// bucket it is, and the refused start mints nothing — on every bucket member of every
/// start call, for every shape of name, the prefix the service's own buckets carry
/// included.
#[tokio::test]
async fn a_bucket_member_naming_no_bucket_is_refused_and_mints_nothing() {
    for (operation, member) in MEMBERS {
        for bucket in probe_names() {
            let case = format!("{operation}${member} = {bucket:?}");
            // A fresh emulator per row, so "nothing was minted" is about THIS request.
            let router = emulator();
            let answer = dispatch(operation, &router, request_for(operation, member, &bucket)).await;

            assert_eq!(
                answer.status, 400,
                "case {case:?}: a bucket the account does not hold must be refused with \
                 HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                missing_bucket_message(&bucket),
                "case {case:?}: expected the missing-bucket refusal naming the submitted \
                 bucket, got {:?}",
                answer.body
            );
            assert!(
                answer.string("/id").is_empty() && answer.string("/assessmentId").is_empty(),
                "case {case:?}: a refused start hands back no id, got {:?}",
                answer.body
            );
            assert_eq!(
                import_task_count(&router).await,
                0,
                "case {case:?}: a refused start must leave no import file task behind"
            );
            assert_eq!(
                get_latest_assessment_id(&router).await.status,
                404,
                "case {case:?}: a refused start must leave no assessment behind"
            );
        }
    }
}

/// Creating the very bucket the refusal named makes the SAME request succeed, and the
/// resource it mints reports the bucket it resolved: the refusal is about the bucket being
/// absent and about nothing else.
#[tokio::test]
async fn creating_the_bucket_makes_the_same_start_call_succeed() {
    for (operation, member) in MEMBERS {
        for bucket in probe_names() {
            let case = format!("{operation}${member} = {bucket:?}");
            let router = emulator();
            let request = request_for(operation, member, &bucket);

            let refused = dispatch(operation, &router, request.clone()).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: the bucket is absent to begin with, so the start is \
                 refused, got {} with {:?}",
                refused.status, refused.body
            );

            create_bucket(&case, &router, &bucket).await;

            let accepted = dispatch(operation, &router, request.clone()).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the account now holds that bucket, so the same start must \
                 be accepted, got {} with {:?}",
                accepted.status, accepted.body
            );

            if *operation == "StartImportFileTask" {
                let id = accepted.string("/id");
                assert!(
                    !id.is_empty(),
                    "case {case:?}: an accepted start hands back the task's id, got {:?}",
                    accepted.body
                );
                // Read back through the implemented reader: the task reports the buckets
                // the start call resolved.
                let reported = get_import(&router, &id).await;
                assert_eq!(
                    reported.status, 200,
                    "case {case:?}: the minted task must be readable, got {} with {:?}",
                    reported.status, reported.body
                );
                assert_eq!(
                    at(&reported.body, "/inputS3Bucket"),
                    request["S3Bucket"].clone(),
                    "case {case:?}: inputS3Bucket is the bucket the start call named, got {:?}",
                    at(&reported.body, "/inputS3Bucket")
                );
                assert_eq!(
                    at(&reported.body, "/statusReportS3Bucket"),
                    request
                        .get("s3bucketForReportData")
                        .cloned()
                        .unwrap_or(Value::Null),
                    "case {case:?}: statusReportS3Bucket is the bucket the start call \
                     named, got {:?}",
                    at(&reported.body, "/statusReportS3Bucket")
                );
                assert_eq!(
                    import_task_count(&router).await,
                    1,
                    "case {case:?}: exactly the one accepted start minted a task"
                );
                continue;
            }

            let assessment_id = accepted.string("/assessmentId");
            assert!(
                !assessment_id.is_empty(),
                "case {case:?}: an accepted start hands back the assessment's id, got {:?}",
                accepted.body
            );
            let latest = get_latest_assessment_id(&router).await;
            assert_eq!(
                latest.string("/id"),
                assessment_id,
                "case {case:?}: the accepted start is the account's latest assessment, got {:?}",
                latest.body
            );
        }
    }
}
