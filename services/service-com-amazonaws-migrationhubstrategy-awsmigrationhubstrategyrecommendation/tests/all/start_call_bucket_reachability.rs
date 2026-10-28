// SPDX-License-Identifier: CC0-1.0

//! Which refusal a start call answers for a bucket member it cannot use, end to end over
//! HTTP.
//!
//! The RULE under test, one level more general than any single request: a bucket member
//! naming a bucket a start call cannot use is refused by the condition that actually holds,
//! under that condition's own reason code and wording — a name the account holds no bucket
//! for is refused as the MISSING bucket it is, with the name the request sent interpolated
//! into the message, and a name the account does hold a bucket for but that lies outside
//! the `migrationhub-strategy-` prefix the service may be reached under is refused as a
//! PERMISSIONS failure naming that prefix. The two are opposite instructions to a client —
//! create or correct the bucket, versus rename it or fix its policy — so one cannot stand
//! in for the other. Only a bucket the account holds INSIDE the prefix is usable, and
//! whatever the refusal, nothing is minted.
//!
//! The rows span what the rule generalises over: which operation and which member carries
//! the name, how near the name comes to the prefix, and — the discrimination this suite
//! exists for — whether the account actually holds a bucket of that name. The emulator
//! holds the S3 service beside Strategy Recommendations over ONE engine, as the assembled
//! emulator does, so a row that needs the bucket to exist creates it through S3's own front
//! door and the start call looks it up by calling S3 in process.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, get_latest_assessment_id, register_collector_in, start_assessment, start_import,
    Answer, TEST_REGION,
};

/// The prefix every bucket member of this service documents ("The bucket name must begin
/// with `migrationhub-strategy-`", `smithy_query(query="doc",shape="StartAssessmentRequest",
/// member="s3bucketForAnalysisData")`), spelled out here rather than taken from the crate so
/// a wrong constant in `src/state.rs` fails this test.
///
/// `pub(crate)` so the suite beside this one holds its names to the same prefix.
pub(crate) const REQUIRED_PREFIX: &str = "migrationhub-strategy-";

/// The reason code each of the two refusals carries, spelled out for the same reason.
const PERMISSIONS_REASON: &str = "[S3PermissionsIsIncorrect]";
const NOT_FOUND_REASON: &str = "[S3BucketNotFound]";

/// A bucket name inside the prefix, for the members a row is not probing.
const CONFORMING_BUCKET: &str = "migrationhub-strategy-analysis";
/// A second one, so a row probing one member leaves the other conforming and distinct.
const CONFORMING_REPORT_BUCKET: &str = "migrationhub-strategy-reports";
/// The import bucket the `StartImportFileTask` rows that probe another member carry.
const CONFORMING_IMPORT_BUCKET: &str = "migrationhub-strategy-import";

/// A SigV4 credential scope naming S3 in the Region the Strategy Recommendations requests
/// of `crate::import_and_assess` are signed for, so a bucket this suite creates is the
/// bucket a start call looks up.
const SIGNED_FOR_S3: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator holding the S3 service beside Strategy Recommendations over one shared
/// engine, its account holding a registered data collector so a start call is decided on
/// its buckets rather than refused for having no data source.
fn emulator_with_object_storage() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    register_collector_in(&engine, &[TEST_REGION]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// Create `bucket` through S3's own front door, as the account's owner would before naming
/// it to a start call.
///
/// `pub(crate)` so the suite beside this one gives its account a bucket the same way.
pub(crate) async fn create_bucket(case: &str, router: &MultiServiceRouter, bucket: &str) {
    let payload = "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                   <LocationConstraint>eu-west-1</LocationConstraint>\
                   </CreateBucketConfiguration>";
    let request = ::http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", "s3.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_S3)
        .body(Bytes::from_static(payload.as_bytes()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: creating the bucket {bucket:?} must succeed, got {status} with {:?}",
        String::from_utf8_lossy(response.into_body().as_ref())
    );
}

/// The missing-bucket refusal as the service words it, written out here so a message built
/// any other way fails: its only variable part is the name the failing member sent.
///
/// `pub(crate)` so the suite beside this one holds the refusal to the same wording.
pub(crate) fn missing_bucket_message(bucket: &str) -> String {
    format!("S3 bucket \"{bucket}\" does not exist or is not accessible [S3BucketNotFound]")
}

/// What a start call does with the row's bucket name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The account holds a bucket of that name, inside the prefix: the start is accepted.
    Accepted,
    /// The account holds no bucket of that name: the missing-bucket refusal.
    MissingBucket,
    /// It holds one, outside the prefix: the permissions refusal.
    Permissions,
}

/// One row: which operation and member carry the name, the name, and whether the account
/// holds a bucket of that name when the call is made.
struct Row {
    operation: &'static str,
    member: &'static str,
    bucket: &'static str,
    held: bool,
}

impl Row {
    fn case(&self) -> String {
        format!(
            "{}${} = {:?}, bucket {}",
            self.operation,
            self.member,
            self.bucket,
            if self.held { "held" } else { "absent" }
        )
    }

    /// What the rule says this row answers.
    fn expected(&self) -> Expected {
        if !self.held {
            Expected::MissingBucket
        } else if self.bucket.starts_with(REQUIRED_PREFIX) {
            Expected::Accepted
        } else {
            Expected::Permissions
        }
    }
}

/// Every bucket member of both start calls, against names that come at the prefix from a
/// different side each time and against the shortest name inside it — each name both held
/// by the account and absent from it.
fn rows() -> Vec<Row> {
    // Names: nothing like the prefix; one character short of it; the prefix somewhere other
    // than the start; the prefix with a dot where it spells a hyphen; the prefix itself; the
    // shortest name inside it.
    let names: &[&str] = &[
        "my-bucket",
        "migrationhub-strateg",
        "x-migrationhub-strategy-data",
        "migrationhub.strategy-data",
        "migrationhub-strategy-data",
        "migrationhub-strategy-a",
    ];
    let members: &[(&str, &str)] = &[
        ("StartAssessment", "s3bucketForAnalysisData"),
        ("StartAssessment", "s3bucketForReportData"),
        ("StartImportFileTask", "S3Bucket"),
        ("StartImportFileTask", "s3bucketForReportData"),
    ];
    let mut rows = Vec::new();
    for (operation, member) in members {
        for bucket in names {
            for held in [false, true] {
                rows.push(Row {
                    operation,
                    member,
                    bucket,
                    held,
                });
            }
        }
    }
    rows
}

/// The request a row sends: its own member carries the row's name, every other member a
/// conforming one the account holds.
fn request_for(row: &Row) -> Value {
    let mut request = match row.operation {
        "StartAssessment" => json!({"s3bucketForAnalysisData": CONFORMING_BUCKET}),
        "StartImportFileTask" => json!({
            "name": "reachability-probe",
            "S3Bucket": CONFORMING_IMPORT_BUCKET,
            "s3key": "servers.csv",
        }),
        other => panic!("this table has no case for operation {other:?}"),
    };
    request[row.member] = json!(row.bucket);
    request
}

async fn dispatch(row: &Row, router: &MultiServiceRouter, request: Value) -> Answer {
    match row.operation {
        "StartAssessment" => start_assessment(router, request).await,
        "StartImportFileTask" => start_import(router, request).await,
        other => panic!("this table has no case for operation {other:?}"),
    }
}

/// How many import file tasks the account holds, read through the list operation.
///
/// `pub(crate)` so the suite beside this one counts them the same way.
pub(crate) async fn import_task_count(router: &MultiServiceRouter) -> usize {
    let listed: Answer = call(router, "GET", "/list-import-file-task", None).await;
    at(&listed.body, "/taskInfos")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// A bucket member is refused by the condition that holds: the missing-bucket message
/// naming the submitted bucket when the account holds no bucket of that name, the
/// permissions message naming the prefix when it holds one outside the prefix, and no
/// refusal at all for a bucket it holds inside the prefix. A refusal mints nothing.
#[tokio::test]
async fn a_bucket_member_is_refused_by_the_condition_that_holds() {
    for row in rows() {
        let case = row.case();
        // A fresh emulator per row, so "nothing was minted" is about THIS request.
        let router = emulator_with_object_storage();
        // The members this row is NOT probing name buckets the account holds, so the row is
        // decided on the name it IS probing.
        for bucket in [
            CONFORMING_BUCKET,
            CONFORMING_REPORT_BUCKET,
            CONFORMING_IMPORT_BUCKET,
        ] {
            create_bucket(&case, &router, bucket).await;
        }
        if row.held {
            create_bucket(&case, &router, row.bucket).await;
        }
        let answer = dispatch(&row, &router, request_for(&row)).await;
        let expected = row.expected();

        if expected == Expected::Accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a bucket the account holds inside the prefix is accepted, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the start to be refused with HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let message = answer.string("/message");
        match expected {
            Expected::MissingBucket => assert_eq!(
                message,
                missing_bucket_message(row.bucket),
                "case {case:?}: expected the missing-bucket refusal naming the submitted \
                 bucket, got {message:?}"
            ),
            Expected::Permissions => {
                assert!(
                    message.contains(PERMISSIONS_REASON),
                    "case {case:?}: a bucket the account holds outside the prefix is a \
                     permissions failure, so the refusal must carry \
                     {PERMISSIONS_REASON:?}, got {message:?}"
                );
                assert!(
                    !message.contains(NOT_FOUND_REASON),
                    "case {case:?}: a bucket that IS there must not be reported under \
                     {NOT_FOUND_REASON:?}, got {message:?}"
                );
                assert!(
                    message.contains(REQUIRED_PREFIX.trim_end_matches('-')),
                    "case {case:?}: the permissions refusal must name the prefix a usable \
                     bucket name begins with, got {message:?}"
                );
            }
            Expected::Accepted => unreachable!("the accepted case returned above"),
        }

        // And nothing was created against the name the service refused.
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

/// The name in the refusal is the one the FAILING member sent — so a caller that supplied
/// two bucket members can tell which of them the service could not use — and creating that
/// very bucket moves the same request from the missing-bucket refusal to the permissions
/// one, which is what makes the two conditions distinguishable at all.
#[tokio::test]
async fn the_refusal_names_the_bucket_the_failing_member_sent() {
    let router = emulator_with_object_storage();
    // The account holds the conforming buckets the rows below leave on the members they
    // are not probing, so each refusal is about the member that failed.
    for bucket in [CONFORMING_BUCKET, CONFORMING_REPORT_BUCKET] {
        create_bucket("the conforming buckets", &router, bucket).await;
    }

    // Both bucket members name a bucket the account does not hold: the refusal names the
    // first member the call checks, and never the other member's name.
    let both_absent = start_assessment(
        &router,
        json!({
            "s3bucketForAnalysisData": "alpha-bucket",
            "s3bucketForReportData": "beta-bucket",
        }),
    )
    .await;
    assert_eq!(
        both_absent.string("/message"),
        missing_bucket_message("alpha-bucket"),
        "two absent buckets: the refusal must name the member the call checks first, got {:?}",
        both_absent.body
    );

    // The analysis bucket conforms, so the report bucket is the member that fails: its own
    // name is the one in the message.
    let report_absent = start_assessment(
        &router,
        json!({
            "s3bucketForAnalysisData": CONFORMING_BUCKET,
            "s3bucketForReportData": "beta-bucket",
        }),
    )
    .await;
    assert_eq!(
        report_absent.string("/message"),
        missing_bucket_message("beta-bucket"),
        "an absent report bucket: the refusal must name the report bucket, got {:?}",
        report_absent.body
    );

    // Create that bucket and the SAME request earns the other refusal: the bucket is there
    // now, and what keeps the service out is its name.
    create_bucket("beta-bucket is created", &router, "beta-bucket").await;
    let report_held = start_assessment(
        &router,
        json!({
            "s3bucketForAnalysisData": CONFORMING_BUCKET,
            "s3bucketForReportData": "beta-bucket",
        }),
    )
    .await;
    let message = report_held.string("/message");
    assert!(
        message.contains(PERMISSIONS_REASON) && !message.contains(NOT_FOUND_REASON),
        "a report bucket that is now held must earn the permissions refusal, got {message:?}"
    );

    // The same flip on the other operation's required bucket member.
    let import_absent = start_import(
        &router,
        json!({"name": "flip", "S3Bucket": "gamma-bucket", "s3key": "servers.csv"}),
    )
    .await;
    assert_eq!(
        import_absent.string("/message"),
        missing_bucket_message("gamma-bucket"),
        "an absent import bucket: the refusal must name it, got {:?}",
        import_absent.body
    );
    create_bucket("gamma-bucket is created", &router, "gamma-bucket").await;
    let import_held = start_import(
        &router,
        json!({"name": "flip", "S3Bucket": "gamma-bucket", "s3key": "servers.csv"}),
    )
    .await;
    let message = import_held.string("/message");
    assert!(
        message.contains(PERMISSIONS_REASON) && !message.contains(NOT_FOUND_REASON),
        "an import bucket that is now held must earn the permissions refusal, got {message:?}"
    );

    // None of the refusals minted anything, and a request whose buckets are both inside the
    // prefix is accepted on the same path.
    assert_eq!(
        import_task_count(&router).await,
        0,
        "the refusals above must leave no import file task behind"
    );
    assert_eq!(
        get_latest_assessment_id(&router).await.status,
        404,
        "the refusals above must leave no assessment behind"
    );
    let accepted = start_assessment(
        &router,
        json!({
            "s3bucketForAnalysisData": CONFORMING_BUCKET,
            "s3bucketForReportData": CONFORMING_REPORT_BUCKET,
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a start naming only buckets inside the prefix is accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert!(
        !accepted.string("/assessmentId").is_empty(),
        "an accepted start hands back an assessment id, got {:?}",
        accepted.body
    );
}
