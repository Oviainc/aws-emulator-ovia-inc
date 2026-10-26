// SPDX-License-Identifier: CC0-1.0

//! The admission-and-creation rule every `Export*Recommendations` operation shares.
//!
//! Each of this service's eight export operations queues ONE asynchronous export job against
//! the destination bucket the request names, and every one of them admits the request by the
//! same rules: the mutually exclusive `accountIds`/`includeMemberAccounts` pair, the account
//! ids themselves, a destination bucket that is named and that the object store already
//! holds, the enrollment gate, and the one-job-in-progress-per-Region limit the operation
//! documents. That rule lives in `crate::state::export` once; this suite drives it END TO END
//! through EVERY operation that reaches it, so a surface that stopped applying one of those
//! checks — or worded one refusal differently from its siblings — fails here.
//!
//! One table row per export operation; the per-operation suites
//! (`auto_scaling_group_recommendations_export.rs` and its siblings) keep the rules specific
//! to one surface. Every call goes through the same `MultiServiceRouter` dispatch a client
//! uses, so what is asserted is what a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    RESOURCE_TYPE_AUTO_SCALING_GROUP, RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_ECS_SERVICE, RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION,
    RESOURCE_TYPE_LICENSE, RESOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_bucket, emulator, error_code, opt_in, ACCOUNT};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it; the
/// request just names it.
const BUCKET: &str = "shared-recommendation-exports";

/// A bucket name the object store does not hold, which every export refuses: "You must create
/// the destination Amazon S3 bucket for your recommendations export before you create the
/// export job."
const MISSING_BUCKET: &str = "no-such-recommendation-export-bucket";

/// One export surface: the operation, the `ResourceType` its job carries and the
/// one-job-in-progress sentence the model documents for it.
struct Surface {
    /// The operation this row drives.
    operation: &'static str,
    /// The `ResourceType` of the job it records, from the crate's own constants.
    resource_type: &'static str,
    /// The sentence this operation's documentation states the limit with, which its refusal
    /// of a second in-progress export opens with.
    one_job_in_progress: &'static str,
    /// The `keyPrefix` this row's request carries, so the rows also span the forms a
    /// destination prefix takes.
    key_prefix: Option<&'static str>,
}

/// Every operation that reaches the shared export rule, with the limit sentence each one
/// documents.
///
/// The sentences are the model's own, one per operation
/// (`smithy_query(query="doc",shape="Export<Resource>Recommendations")`) — note that the
/// Amazon ECS surface words it "You can only have one" where the others say "You can have
/// only one".
fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            operation: "ExportAutoScalingGroupRecommendations",
            resource_type: RESOURCE_TYPE_AUTO_SCALING_GROUP,
            one_job_in_progress: "You can have only one Auto Scaling group export job in \
                                  progress per Amazon Web Services Region.",
            key_prefix: None,
        },
        Surface {
            operation: "ExportEBSVolumeRecommendations",
            resource_type: RESOURCE_TYPE_EBS_VOLUME,
            one_job_in_progress: "You can have only one Amazon EBS volume export job in \
                                  progress per Amazon Web Services Region.",
            key_prefix: Some("volumes"),
        },
        Surface {
            operation: "ExportEC2InstanceRecommendations",
            resource_type: RESOURCE_TYPE_EC2_INSTANCE,
            one_job_in_progress: "You can have only one Amazon EC2 instance export job in \
                                  progress per Amazon Web Services Region.",
            key_prefix: Some("/instances/"),
        },
        Surface {
            operation: "ExportECSServiceRecommendations",
            resource_type: RESOURCE_TYPE_ECS_SERVICE,
            one_job_in_progress: "You can only have one Amazon ECS service export job in \
                                  progress per Amazon Web Services Region.",
            key_prefix: Some("fargate/services"),
        },
        Surface {
            operation: "ExportIdleRecommendations",
            resource_type: RESOURCE_TYPE_IDLE,
            one_job_in_progress: "You can have only one idle resource export job in progress \
                                  per Amazon Web Services Region.",
            key_prefix: Some(""),
        },
        Surface {
            operation: "ExportLambdaFunctionRecommendations",
            resource_type: RESOURCE_TYPE_LAMBDA_FUNCTION,
            one_job_in_progress: "You can have only one Lambda function export job in \
                                  progress per Amazon Web Services Region.",
            key_prefix: Some("a/b/c"),
        },
        Surface {
            operation: "ExportLicenseRecommendations",
            resource_type: RESOURCE_TYPE_LICENSE,
            one_job_in_progress: "You can have only one license export job in progress per \
                                  Amazon Web Services Region.",
            key_prefix: Some("licences/"),
        },
        Surface {
            operation: "ExportRDSDatabaseRecommendations",
            resource_type: RESOURCE_TYPE_RDS_DB_INSTANCE,
            one_job_in_progress: "You can have only one Amazon Aurora or RDS export job in \
                                  progress per Amazon Web Services Region.",
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
                format!(
                    "The value supplied for parameter s3DestinationConfig is not valid: the \
                     Amazon S3 bucket {MISSING_BUCKET} does not exist. You must create the \
                     destination Amazon S3 bucket for your recommendations export before you \
                     create the export job."
                ),
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
/// resource type, and refuses the second call with that operation's own sentence naming the
/// job still in progress.
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
            Some(
                format!(
                    "{} Export job {first} is still in progress.",
                    surface.one_job_in_progress
                )
                .as_str()
            ),
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
