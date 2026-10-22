// SPDX-License-Identifier: CC0-1.0

//! The assessment error paths over HTTP: which status and error code each failure
//! condition answers, and the bucket-name prefix rule every start call enforces.
//!
//! Two rules are under test, both one level more general than any single route.
//!
//! 1. A failure condition is answered with the status and error code the service answers
//!    for THAT CONDITION — not with whichever client error the operation's own modelled
//!    `errors` list happens to contain. A narrow list is widened with `extra_errors`, so
//!    `StopAssessment` and `GetLatestAssessmentId` can report an absent resource with
//!    `ResourceNotFoundException` (404) and `GetAssessment` can refuse an id it was never
//!    issued as an invalid parameter (`ValidationException`, 400).
//! 2. A constraint a request member's documentation states in prose that its `@pattern`
//!    and `@length` cannot express — "the bucket name must begin with
//!    `migrationhub-strategy-`" — is enforced by the handler, which refuses the request
//!    with `ValidationException` (400) before any state is created: naming the member and
//!    the prefix when the operation cannot run without a bucket it was not given, and
//!    naming the bucket when the name it was given is not one the service can use.
//!
//! Every row goes through the router, so each case exercises routing, the boundary
//! validator, the handler and error serialization — the same path a client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    emulator, get_assessment, get_latest_assessment_id, start_assessment, start_import,
    stop_assessment, Answer,
};

/// The prefix every bucket member of this service documents. Spelled out here rather
/// than taken from the crate so a wrong constant in `src/state.rs` fails this test.
const REQUIRED_PREFIX: &str = "migrationhub-strategy-";

/// What each operation that takes an assessment id answers when the id resolves to no
/// assessment: the status and the `x-amzn-errortype` of the recorded wire behaviour of
/// its route. `GetAssessment` refuses the read as an invalid parameter; `StopAssessment`
/// reports the resource as absent.
const UNKNOWN_ID_REFUSALS: &[(&str, u16, &str)] = &[
    ("GetAssessment", 400, "ValidationException"),
    ("StopAssessment", 404, "ResourceNotFoundException"),
];

/// Route the named operation at `id`, so one table can span both operations.
async fn refuse(router: &MultiServiceRouter, operation: &str, id: &str) -> Answer {
    match operation {
        "GetAssessment" => get_assessment(router, id).await,
        "StopAssessment" => stop_assessment(router, id).await,
        other => panic!("this table has no case for operation {other:?}"),
    }
}

/// Assessment ids that name no assessment. Every row satisfies `AsyncTaskId`'s
/// `@pattern([0-9a-z-:]+)` and its `@length(0,52)`, so the refusal comes from the handler
/// and not from the boundary validator, and they differ along what the rule generalises
/// over: the form of the id, its length, and how close it is to a real one.
fn unknown_ids(known: &str) -> Vec<(String, String)> {
    vec![
        // The id the recorded request used: a legal, never-issued one.
        (
            "a uuid nothing minted".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a single digit".to_string(), "0".to_string()),
        ("an unknown word".to_string(), "nope".to_string()),
        (
            "an id of the minted shape that was never minted".to_string(),
            "assessment-ffffffffffff".to_string(),
        ),
        (
            "a colon-separated id".to_string(),
            "task:0:does-not-exist".to_string(),
        ),
        ("a known id with a suffix".to_string(), format!("{known}-1")),
        (
            "the longest id the model admits".to_string(),
            "a".repeat(52),
        ),
    ]
}

/// An assessment id that resolves to nothing is refused with the status and error code
/// its own operation answers — and the two operations do NOT answer the same pair, which
/// is the whole point: a client branching on the error type has to see the service's.
#[tokio::test]
async fn an_unresolvable_assessment_id_is_refused_per_operation() {
    let router = emulator();
    let known = start_assessment(
        &router,
        json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"}),
    )
    .await
    .string("/assessmentId");

    for (case, id) in unknown_ids(&known) {
        for (operation, status, code) in UNKNOWN_ID_REFUSALS {
            let answer = refuse(&router, operation, &id).await;
            assert_eq!(
                answer.status, *status,
                "case {case:?}: {operation} on {id:?} must answer HTTP {status}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                *code,
                "case {case:?}: {operation} on {id:?} must answer {code}, got {:?}",
                answer.body
            );
            assert!(
                !answer.string("/message").is_empty(),
                "case {case:?}: {operation} on {id:?} must say why it refused, got {:?}",
                answer.body
            );
        }
    }

    // None of the refusals touched the assessment that does exist.
    assert_eq!(
        get_assessment(&router, &known).await.status,
        200,
        "the assessment that was started is still reported after the refusals"
    );
}

/// `GetLatestAssessmentId` takes no input, so an invalid-request refusal is unreachable:
/// in an account where no assessment has ever been started the absent resource is
/// reported with `ResourceNotFoundException` (404), and the answer is the same however
/// many times it is asked.
#[tokio::test]
async fn an_account_with_no_assessment_reports_the_latest_id_as_absent() {
    let router = emulator();

    for attempt in 1..=3 {
        let answer = get_latest_assessment_id(&router).await;
        assert_eq!(
            answer.status, 404,
            "attempt {attempt}: an account with no assessment has no latest id, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "attempt {attempt}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert!(
            answer.string("/id").is_empty(),
            "attempt {attempt}: a refusal must not carry an id nothing minted, got {:?}",
            answer.body
        );
    }

    // An import brings servers in without starting an assessment, so the empty answer is
    // about assessments and not about an empty store.
    let imported = start_import(
        &router,
        json!({
            "name": "import-1",
            "S3Bucket": "migrationhub-strategy-import",
            "s3key": "servers.csv",
        }),
    )
    .await;
    assert_eq!(
        imported.status, 200,
        "the import that sets this case up must succeed, got {} with {:?}",
        imported.status, imported.body
    );
    assert_eq!(
        get_latest_assessment_id(&router).await.status,
        404,
        "an import is not an assessment, so there is still no latest assessment id"
    );
}

/// One row of the bucket-name table: the request, and what the service does with it.
struct BucketCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// `None` when the request is accepted.
    refusal: Option<Refusal>,
}

/// The two ways a bucket member is refused, and what each refusal has to say.
enum Refusal {
    /// The operation cannot run without the bucket and was not given one: the message
    /// names the MEMBER and the prefix a bucket for it has to carry.
    Missing(&'static str),
    /// The member names a bucket the service cannot use: the message names the BUCKET.
    Unusable(&'static str),
}

/// A `StartImportFileTask` request with the two members that are not buckets filled in.
fn import_request(bucket: &str, report_bucket: Option<&str>) -> Value {
    let mut request = json!({"name": "import-1", "S3Bucket": bucket, "s3key": "servers.csv"});
    if let Some(report) = report_bucket {
        request["s3bucketForReportData"] = json!(report);
    }
    request
}

/// Every bucket member this service documents the `migrationhub-strategy-` prefix on,
/// across both operations that take one:
/// `StartAssessmentRequest$s3bucketForAnalysisData`, `StartAssessmentRequest$s3bucketForReportData`,
/// `StartImportFileTaskRequest$S3Bucket` and `StartImportFileTaskRequest$s3bucketForReportData`
/// (from `smithy_query(query="shape",shape="StartAssessmentRequest")` and
/// `…shape="StartImportFileTaskRequest"`).
///
/// The rows differ along what the rule generalises over: which member carries the bucket,
/// whether it is present at all, whether it is required, and how near the name comes to
/// the prefix — including the shortest conforming name and names one character short of
/// the prefix on either side.
fn bucket_cases() -> Vec<BucketCase> {
    vec![
        BucketCase {
            // The recorded request: no analysis bucket at all.
            case: "StartAssessment with no analysis bucket",
            operation: "StartAssessment",
            request: json!({}),
            refusal: Some(Refusal::Missing("s3bucketForAnalysisData")),
        },
        BucketCase {
            case: "StartAssessment with targets but no analysis bucket",
            operation: "StartAssessment",
            request: json!({"assessmentTargets": [
                {"condition": "EQUALS", "name": "OS_NAME", "values": ["Linux"]},
            ]}),
            refusal: Some(Refusal::Missing("s3bucketForAnalysisData")),
        },
        BucketCase {
            case: "StartAssessment with a pattern-legal bucket that has no prefix",
            operation: "StartAssessment",
            request: json!({"s3bucketForAnalysisData": "my-bucket"}),
            refusal: Some(Refusal::Unusable("my-bucket")),
        },
        BucketCase {
            case: "StartAssessment with the prefix one character short",
            operation: "StartAssessment",
            request: json!({"s3bucketForAnalysisData": "migrationhub-strategy"}),
            refusal: Some(Refusal::Unusable("migrationhub-strategy")),
        },
        BucketCase {
            case: "StartAssessment with the prefix somewhere other than the start",
            operation: "StartAssessment",
            request: json!({"s3bucketForAnalysisData": "x-migrationhub-strategy-analysis"}),
            refusal: Some(Refusal::Unusable("x-migrationhub-strategy-analysis")),
        },
        BucketCase {
            case: "StartAssessment with the shortest conforming analysis bucket",
            operation: "StartAssessment",
            request: json!({"s3bucketForAnalysisData": "migrationhub-strategy-a"}),
            refusal: None,
        },
        BucketCase {
            case: "StartAssessment with a conforming analysis bucket and a bare report bucket",
            operation: "StartAssessment",
            request: json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
                "s3bucketForReportData": "my-reports",
            }),
            refusal: Some(Refusal::Unusable("my-reports")),
        },
        BucketCase {
            case: "StartAssessment with both buckets conforming",
            operation: "StartAssessment",
            request: json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis.data",
                "s3bucketForReportData": "migrationhub-strategy-reports",
                "assessmentDataSourceType": "ManualImport",
            }),
            refusal: None,
        },
        BucketCase {
            case: "StartImportFileTask with a bare import bucket",
            operation: "StartImportFileTask",
            request: import_request("my-bucket", None),
            refusal: Some(Refusal::Unusable("my-bucket")),
        },
        BucketCase {
            case: "StartImportFileTask with a conforming import bucket and a bare report bucket",
            operation: "StartImportFileTask",
            request: import_request("migrationhub-strategy-import", Some("reports")),
            refusal: Some(Refusal::Unusable("reports")),
        },
        BucketCase {
            case: "StartImportFileTask with both buckets conforming",
            operation: "StartImportFileTask",
            request: import_request(
                "migrationhub-strategy-import",
                Some("migrationhub-strategy-reports"),
            ),
            refusal: None,
        },
    ]
}

/// A bucket the operation cannot run without and was not given, or a bucket name that
/// does not begin with the documented prefix, is refused with `ValidationException`
/// (HTTP 400) — the first naming the member and the prefix, the second naming the bucket
/// — and nothing is minted. A conforming request is accepted on the same path.
#[tokio::test]
async fn a_bucket_name_is_held_to_the_documented_prefix() {
    for BucketCase {
        case,
        operation,
        request,
        refusal,
    } in bucket_cases()
    {
        // A fresh emulator per row, so "nothing was minted" is about THIS request.
        let router = emulator();
        let answer = match operation {
            "StartAssessment" => start_assessment(&router, request).await,
            "StartImportFileTask" => start_import(&router, request).await,
            other => panic!("case {case:?}: this table has no case for operation {other:?}"),
        };

        let Some(refusal) = refusal else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a conforming request is accepted, got {} with {:?}",
                answer.status, answer.body
            );
            continue;
        };

        assert_eq!(
            answer.status, 400,
            "case {case:?}: {operation} must answer HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: {operation} must answer ValidationException, got {:?}",
            answer.body
        );

        let message = answer.string("/message");
        match refusal {
            Refusal::Missing(member) => {
                assert!(
                    message.contains(member),
                    "case {case:?}: the refusal must name {member:?}, got {message:?}"
                );
                assert!(
                    message.contains(REQUIRED_PREFIX),
                    "case {case:?}: the refusal must name the {REQUIRED_PREFIX:?} prefix, \
                     got {message:?}"
                );
            }
            Refusal::Unusable(bucket) => {
                assert!(
                    message.contains(bucket),
                    "case {case:?}: the refusal must name the bucket {bucket:?}, got {message:?}"
                );
            }
        }
    }
}

/// A refused start creates nothing: the account that had no assessment still has none,
/// so `GetLatestAssessmentId` keeps reporting the absent resource until a start the
/// service accepts mints one.
#[tokio::test]
async fn a_refused_start_assessment_leaves_no_assessment_behind() {
    let router = emulator();

    for BucketCase {
        case,
        operation,
        request,
        refusal,
    } in bucket_cases()
    {
        if operation != "StartAssessment" || refusal.is_none() {
            continue;
        }
        let refused = start_assessment(&router, request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the request must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            refused.string("/assessmentId").is_empty(),
            "case {case:?}: a refused start must hand back no assessment id, got {:?}",
            refused.body
        );
        assert_eq!(
            get_latest_assessment_id(&router).await.status,
            404,
            "case {case:?}: a refused start must not mint an assessment"
        );
    }

    let accepted = start_assessment(
        &router,
        json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an accepted start follows the refusals, got {} with {:?}",
        accepted.status, accepted.body
    );
    let latest = get_latest_assessment_id(&router).await;
    assert_eq!(
        latest.status, 200,
        "once an assessment exists its id is reported, got {} with {:?}",
        latest.status, latest.body
    );
    assert_eq!(
        latest.string("/id"),
        accepted.string("/assessmentId"),
        "the latest id must be the one the accepted start minted, got {:?}",
        latest.body
    );
}
