// SPDX-License-Identifier: CC0-1.0

//! The admission-and-creation rule every `Export*Recommendations` operation shares.
//!
//! Each of this service's eight export operations queues ONE asynchronous export job against
//! the destination bucket the request names, and every one of them admits the request by the
//! same rules: the mutually exclusive `accountIds`/`includeMemberAccounts` pair, the account
//! ids themselves — their shape AND whose analysis they ask for — a destination bucket that is
//! named and that the object store already holds, the enrollment gate, and the
//! one-job-in-progress-per-Region limit the operation documents. That rule lives in `crate::state::export` once; this suite drives it END TO END
//! through EVERY operation that reaches it, so a surface that stopped applying one of those
//! checks — or worded one refusal differently from its siblings — fails here.
//!
//! One table row per export operation; the per-operation suites
//! (`auto_scaling_group_recommendations_export.rs` and its siblings) keep the rules specific
//! to one surface. Every call goes through the same `MultiServiceRouter` dispatch a client
//! uses, so what is asserted is what a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::export::{
    EXPORT_JOB_LIMIT_MESSAGE, MISSING_DESTINATION_MESSAGE,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    EXPORT_JOB_STEP, RESOURCE_TYPE_AUTO_SCALING_GROUP, RESOURCE_TYPE_EBS_VOLUME,
    RESOURCE_TYPE_EC2_INSTANCE, RESOURCE_TYPE_ECS_SERVICE, RESOURCE_TYPE_IDLE,
    RESOURCE_TYPE_LAMBDA_FUNCTION, RESOURCE_TYPE_LICENSE, RESOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_bucket, emulator, error_code, export_destination, get_object, opt_in, ACCOUNT,
};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it; the
/// request just names it.
const BUCKET: &str = "shared-recommendation-exports";

/// A bucket name the object store does not hold, which every export refuses: "You must create
/// the destination Amazon S3 bucket for your recommendations export before you create the
/// export job."
const MISSING_BUCKET: &str = "no-such-recommendation-export-bucket";

/// One export surface: the operation and the `ResourceType` its job carries.
struct Surface {
    /// The operation this row drives.
    operation: &'static str,
    /// The `ResourceType` of the job it records, from the crate's own constants.
    resource_type: &'static str,
    /// The `keyPrefix` this row's request carries, so the rows also span the forms a
    /// destination prefix takes.
    key_prefix: Option<&'static str>,
}

/// Every operation that reaches the shared export rule.
fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            operation: "ExportAutoScalingGroupRecommendations",
            resource_type: RESOURCE_TYPE_AUTO_SCALING_GROUP,
            key_prefix: None,
        },
        Surface {
            operation: "ExportEBSVolumeRecommendations",
            resource_type: RESOURCE_TYPE_EBS_VOLUME,
            key_prefix: Some("volumes"),
        },
        Surface {
            operation: "ExportEC2InstanceRecommendations",
            resource_type: RESOURCE_TYPE_EC2_INSTANCE,
            key_prefix: Some("/instances/"),
        },
        Surface {
            operation: "ExportECSServiceRecommendations",
            resource_type: RESOURCE_TYPE_ECS_SERVICE,
            key_prefix: Some("fargate/services"),
        },
        Surface {
            operation: "ExportIdleRecommendations",
            resource_type: RESOURCE_TYPE_IDLE,
            key_prefix: Some(""),
        },
        Surface {
            operation: "ExportLambdaFunctionRecommendations",
            resource_type: RESOURCE_TYPE_LAMBDA_FUNCTION,
            key_prefix: Some("a/b/c"),
        },
        Surface {
            operation: "ExportLicenseRecommendations",
            resource_type: RESOURCE_TYPE_LICENSE,
            key_prefix: Some("licences/"),
        },
        Surface {
            operation: "ExportRDSDatabaseRecommendations",
            resource_type: RESOURCE_TYPE_RDS_DB_INSTANCE,
            key_prefix: Some("databases"),
        },
    ]
}

/// An emulator with the destination bucket created and the account opted in, which every
/// export call requires.
async fn enrolled_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    router
}

/// The request body one surface exports with: its own key prefix, plus any extra members.
fn export_request(surface: &Surface) -> Value {
    let mut destination = json!({"bucket": BUCKET});
    if let Some(key_prefix) = surface.key_prefix {
        destination["keyPrefix"] = json!(key_prefix);
    }
    json!({"s3DestinationConfig": destination})
}

/// The object-key prefix a `keyPrefix` member derives: the prefix without its surrounding
/// slashes, then one separator, or nothing at all when it names none.
fn expected_prefix(key_prefix: Option<&str>) -> String {
    match key_prefix.map(|prefix| prefix.trim_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/"),
        _ => String::new(),
    }
}

/// The job ids a `DescribeRecommendationExportJobs` body reports, in the order it reported
/// them.
async fn reported_jobs(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, body) = call(router, "DescribeRecommendationExportJobs", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the export jobs back: got {status}: {body}"
    );
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"))
        .clone()
}

/// Every export operation records its job through the one shared rule: the call answers a
/// minted job id and the destination derived from the bucket and key prefix the request
/// named, the account and the operation's own resource type — and the job the call recorded
/// names exactly those objects.
#[tokio::test]
async fn every_export_operation_records_its_job_through_the_shared_rule() {
    let account = engine::emulator_account_id();
    let mut minted: Vec<String> = Vec::new();

    for surface in surfaces() {
        let case = surface.operation;
        // A fresh emulator per row: only one export job of a resource type may be in
        // progress per Region, and this row asserts the store holds exactly its own job.
        let router = enrolled_emulator().await;
        let (status, body) = call(&router, case, export_request(&surface)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the export to be accepted, got {status}: {body}"
        );

        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a jobId, got {body}"))
            .to_string();
        assert_eq!(
            job_id.len(),
            36,
            "case {case:?}: expected a 36-character job id, got {job_id:?}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every export job gets its own id, got {job_id:?} twice"
        );
        minted.push(job_id.clone());

        let stem = format!(
            "{}{account}/ComputeOptimizer/{}/{job_id}",
            expected_prefix(surface.key_prefix),
            surface.resource_type
        );
        let destination = json!({
            "bucket": BUCKET,
            "key": format!("{stem}.csv"),
            "metadataKey": format!("{stem}-metadata.json"),
        });
        assert_eq!(
            body["s3Destination"], destination,
            "case {case:?}: unexpected destination, got {body}"
        );

        let jobs = reported_jobs(&router, case).await;
        assert_eq!(
            jobs.len(),
            1,
            "case {case:?}: the export records exactly one job, got {jobs:?}"
        );
        assert_eq!(
            jobs[0]["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: the recorded job is the one the call answered, got {:?}",
            jobs[0]
        );
        assert_eq!(
            jobs[0]["destination"],
            json!({"s3": destination}),
            "case {case:?}: the reported destination must be the one the export answered, \
             got {:?}",
            jobs[0]
        );
        assert_eq!(
            jobs[0]["resourceType"].as_str(),
            Some(surface.resource_type),
            "case {case:?}: the job carries the resource type the operation exports, got {:?}",
            jobs[0]
        );
    }
}

/// Every export operation refuses the same malformed requests, with the same wording and
/// without recording a job: one row per rule the shared admission applies before the store is
/// written.
#[tokio::test]
async fn every_export_operation_refuses_the_same_malformed_requests() {
    let router = enrolled_emulator().await;

    for surface in surfaces() {
        // (what the request carries, the message the service answers)
        let rows: Vec<(Value, String)> = vec![
            (
                json!({"s3DestinationConfig": {}}),
                "The value supplied for parameter s3DestinationConfig is not valid: a \
                 destination bucket must be supplied."
                    .to_string(),
            ),
            (
                json!({"s3DestinationConfig": {"bucket": "   "}}),
                "The value supplied for parameter s3DestinationConfig is not valid: a \
                 destination bucket must be supplied."
                    .to_string(),
            ),
            (
                json!({"s3DestinationConfig": {"bucket": MISSING_BUCKET}}),
                MISSING_DESTINATION_MESSAGE.to_string(),
            ),
            (
                json!({
                    "s3DestinationConfig": {"bucket": BUCKET},
                    "accountIds": [ACCOUNT],
                    "includeMemberAccounts": true,
                }),
                "The value supplied for parameter accountIds is not valid: accountIds and \
                 includeMemberAccounts are mutually exclusive."
                    .to_string(),
            ),
            (
                json!({"s3DestinationConfig": {"bucket": BUCKET}, "accountIds": ["12345"]}),
                "The value supplied for parameter accountIds is not valid: 12345 is not a \
                 12-digit Amazon Web Services account ID."
                    .to_string(),
            ),
            (
                json!({
                    "s3DestinationConfig": {"bucket": BUCKET},
                    "accountIds": [ACCOUNT, "not-an-account"],
                }),
                "The value supplied for parameter accountIds is not valid: not-an-account is \
                 not a 12-digit Amazon Web Services account ID."
                    .to_string(),
            ),
        ];

        for (request, expected) in rows {
            let case = format!("{} refusing {request}", surface.operation);
            let (status, body) = call(&router, surface.operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the request to be refused, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: unexpected message, got {body}"
            );
            assert!(
                body["jobId"].is_null(),
                "case {case:?}: a refused export answers no job id, got {body}"
            );
        }
    }

    // Not one of those calls wrote to the store.
    let jobs = reported_jobs(&router, "after every refusal").await;
    assert!(
        jobs.is_empty(),
        "a refused export records no job, got {jobs:?}"
    );
}

/// Every export operation holds the one-job-in-progress limit it documents, counts it per
/// resource type, and refuses the second call with the service's one limit sentence
/// ([`EXPORT_JOB_LIMIT_MESSAGE`]) — the same text on every surface, naming neither the
/// resource type nor the job that still holds the slot.
#[tokio::test]
async fn every_export_operation_holds_the_limit_it_documents() {
    let router = enrolled_emulator().await;
    let mut first_jobs: Vec<String> = Vec::new();

    for surface in surfaces() {
        let case = surface.operation;
        // The limit counts the jobs of ONE resource type, so this first export is admitted
        // even while every earlier row's job is still in progress in the same Region.
        let (status, body) = call(&router, case, export_request(&surface)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an export of this resource type is not blocked by another's, got \
             {status}: {body}"
        );
        let first = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a jobId, got {body}"))
            .to_string();

        let (status, body) = call(&router, case, export_request(&surface)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second export while one is in progress is refused, got \
             {status}: {body}"
        );
        assert!(
            error_code(&body).contains("LimitExceededException"),
            "case {case:?}: expected LimitExceededException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(EXPORT_JOB_LIMIT_MESSAGE),
            "case {case:?}: unexpected message, got {body}"
        );
        first_jobs.push(first);
    }

    // The refused calls recorded nothing: the store holds exactly the admitted jobs, in
    // creation order.
    let reported: Vec<String> = reported_jobs(&router, "every admitted job")
        .await
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, first_jobs,
        "the store holds exactly the admitted jobs, got {reported:?}"
    );
}

/// The refusal every operation of this service answers a request about another account with:
/// the service's constant, which names neither the member nor the account — it states the
/// permission that is missing and how an organization account gets it. Spelled out here rather
/// than imported so the WORDING is what this suite pins.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// Well-formed 12-digit account ids that are NOT the caller's own — the class "an account this
/// service answers nothing about", spanned rather than named once: the extremes of the 12-digit
/// range, the AWS documentation's own example account, and an id one digit away from this
/// emulator's own account (derived from it, so the row stays a different account whatever that
/// account is).
fn other_accounts() -> Vec<String> {
    let mut neighbour = ACCOUNT.to_string();
    let last = neighbour.pop().unwrap_or('0');
    neighbour.push(if last == '9' { '8' } else { '9' });
    vec![
        "000000000001".to_string(),
        "999999999999".to_string(),
        "111122223333".to_string(),
        "210987654321".to_string(),
        neighbour,
    ]
}

/// The `accountIds` lists that name at least one account other than the caller's: one foreign
/// id alone, and lists where the caller's own account is named BESIDE a foreign one, in either
/// position — an export may carry several ids ("You can specify multiple account IDs per
/// request"), and one id it cannot answer about is enough to refuse the whole export.
fn cross_account_lists() -> Vec<Value> {
    let others = other_accounts();
    let mut rows: Vec<Value> = others.iter().map(|id| json!([id])).collect();
    if let Some(id) = others.first() {
        rows.push(json!([ACCOUNT, id]));
        rows.push(json!([id, ACCOUNT]));
    }
    rows
}

/// The `accountIds` forms that name the caller's own account and nothing else: naming none at
/// all, an empty list, the account once, and the account more than once — "You can specify
/// multiple account IDs per request", so an export's list is not limited to one id the way a
/// recommendation reader's is.
fn own_account_lists() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("absent", None),
        ("empty list", Some(json!([]))),
        ("own account", Some(json!([ACCOUNT]))),
        ("own account twice", Some(json!([ACCOUNT, ACCOUNT]))),
    ]
}

/// Every export operation refuses an `accountIds` naming any account other than the one this
/// emulator presents, with the same refusal its recommendation reader answers the same id with
/// — rather than exporting THIS account's rows into a file whose metadata names that other
/// account. One row per foreign-id form, crossed with every export surface.
#[tokio::test]
async fn every_export_operation_refuses_an_account_id_naming_another_account() {
    let router = enrolled_emulator().await;

    for surface in surfaces() {
        for account_ids in cross_account_lists() {
            let case = format!("{} with accountIds {account_ids}", surface.operation);
            let mut request = export_request(&surface);
            request["accountIds"] = account_ids.clone();

            let (status, body) = call(&router, surface.operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the export to be refused, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(CROSS_ACCOUNT_MESSAGE),
                "case {case:?}: expected the cross-account refusal, got {body}"
            );
            assert!(
                body["jobId"].is_null(),
                "case {case:?}: a refused export answers no job id, got {body}"
            );
        }
    }

    // Not one of those calls recorded a job, so nothing was ever queued to write a file.
    let jobs = reported_jobs(&router, "after every cross-account refusal").await;
    assert!(
        jobs.is_empty(),
        "an export refused for naming another account records no job, got {jobs:?}"
    );
}

/// The request about another account is refused BEFORE the enrollment gate, for every export
/// surface: the refusal is about what the request asks for, not about what the store holds, so
/// an account that never opted in gets the same 400 rather than the 403 its own export gets.
#[tokio::test]
async fn the_export_cross_account_refusal_precedes_the_opt_in_gate() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    let other = other_accounts().remove(0);

    for surface in surfaces() {
        let case = format!("{} not opted in, another account", surface.operation);
        let mut request = export_request(&surface);
        request["accountIds"] = json!([other]);

        let (status, body) = call(&router, surface.operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the cross-account refusal, got {status}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CROSS_ACCOUNT_MESSAGE),
            "case {case:?}: expected the cross-account refusal, got {body}"
        );
    }
}

/// Every export operation admits every form that names the caller's own account and nothing
/// else — and the metadata of the file it writes names that one account, whichever form asked
/// for it, so the file's stated owner is the account whose rows it holds.
#[tokio::test]
async fn every_export_operation_admits_the_caller_s_own_account_and_names_only_it() {
    for surface in surfaces() {
        // One emulator per admitted call: only one export job of a resource type may be in
        // progress per Region, so the rows of one surface cannot share a store.
        let mut written: Vec<(MultiServiceRouter, String, String, String)> = Vec::new();

        for (form, account_ids) in own_account_lists() {
            let case = format!("{} with accountIds {form}", surface.operation);
            let router = enrolled_emulator().await;
            let mut request = export_request(&surface);
            if let Some(account_ids) = &account_ids {
                request["accountIds"] = account_ids.clone();
            }

            let (status, body) = call(&router, surface.operation, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the export to be accepted, got {status}: {body}"
            );
            assert!(
                body["jobId"].is_string(),
                "case {case:?}: an accepted export answers a job id, got {body}"
            );
            let (bucket, _, metadata_key) = export_destination(&case, &body);
            written.push((router, case, bucket, metadata_key));
        }

        // Every job above was created within milliseconds of the others, so one wait covers
        // the whole surface's rows: the clock makes a completion due a fixed span after the
        // call that created it.
        tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;

        for (router, case, bucket, metadata_key) in &written {
            let (status, metadata) = get_object(router, bucket, metadata_key).await;
            assert_eq!(
                status, 200,
                "case {case:?}: downloading {metadata_key}: got {status}: {}",
                String::from_utf8_lossy(&metadata)
            );
            let metadata: Value = serde_json::from_slice(&metadata).unwrap_or_else(|error| {
                panic!("case {case:?}: the metadata file is JSON: {error}")
            });
            let listed: Vec<String> = metadata["accountIds"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the metadata lists the accounts exported, got {metadata}")
                })
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !listed.is_empty() && listed.iter().all(|id| id == ACCOUNT),
                "case {case:?}: the metadata names only the account whose rows the file holds \
                 ({ACCOUNT}), got {listed:?}"
            );
        }
    }
}

/// The enrollment gate is the same for every export operation: "The account is not opted in to
/// Compute Optimizer" while the account is not enrolled, with HTTP 403 and no job recorded.
#[tokio::test]
async fn every_export_operation_refuses_while_the_account_is_not_opted_in() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;

    for surface in surfaces() {
        let case = surface.operation;
        let (status, body) = call(&router, case, export_request(&surface)).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 while not opted in, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "case {case:?}: expected OptInRequiredException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("The account is not opted in to Compute Optimizer."),
            "case {case:?}: unexpected message, got {body}"
        );
    }

    // Enrolling is what admits an export; the store the refused calls left alone is empty.
    opt_in(&router).await;
    let jobs = reported_jobs(&router, "after enrolling").await;
    assert!(
        jobs.is_empty(),
        "an export refused by the enrollment gate records no job, got {jobs:?}"
    );
}
