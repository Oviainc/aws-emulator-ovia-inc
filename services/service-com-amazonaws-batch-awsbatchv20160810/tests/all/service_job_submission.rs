// SPDX-License-Identifier: CC0-1.0

//! `SubmitServiceJob`, end to end over the wire, read back through `DescribeServiceJob`.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a submit PERSISTS what the request asked for and `DescribeServiceJob` reads it back
//!   member for member: a fresh `jobId`, the `jobArn`
//!   `arn:aws:batch:<region>:<account>:service-job/<jobId>` minted once, the ARN of the
//!   queue the request named (by name OR by ARN, the two forms `jobQueue` documents), and
//!   every optional member the request carried — absent when it carried none;
//! * the `jobQueue` must resolve to a queue this service holds and that queue must have
//!   the type `SAGEMAKER_TRAINING` ("The job queue must have the type
//!   `SAGEMAKER_TRAINING`"); anything else is the modelled `ClientException` (HTTP 400)
//!   and persists nothing. A reference nothing answers to is the service's own lookup
//!   missing, so its message is the not-found sentence built from the reference the
//!   request gave (`JobQueue <reference> not found.`) — the rule is the sentence, and the
//!   rows differ in which reference fills it;
//! * a member outside the range or charset its own doc publishes — `jobName`,
//!   `schedulingPriority` (0..9999), `retryStrategy.attempts` (1..10) — is refused with
//!   the same `ClientException`, and a value on either published bound is accepted;
//! * `clientToken` "is used to ensure idempotency of requests": the identical repeat
//!   resolves to the job the first submit minted and mints nothing, the same token with a
//!   different payload is refused, and two different tokens mint two jobs;
//! * a submitted service job is handed back in the non-terminal `SUBMITTED` status and
//!   only the CLOCK moves it — never a read — to `RUNNABLE` and then into the modelled
//!   failure state, because nothing here runs SageMaker AI: no `scheduledAt`,
//!   `startedAt`, attempt or capacity usage is invented for work nothing dispatched;
//! * a `SAGEMAKER_TRAINING` queue's `serviceEnvironmentOrder` resolves each entry by name
//!   or ARN and stores the service environment's ARN; an entry that resolves to nothing is
//!   a `ClientException` and the queue is not created.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::cancel_job::flip_last_hex_digit;
use crate::job_lifecycle::{arn, compute_environment, error_code, error_message, Batch, REGION};

/// The only `ServiceJobType` the model declares
/// (`smithy_query(query="shape", shape="ServiceJobType")`: SAGEMAKER_TRAINING).
const SERVICE_JOB_TYPE: &str = "SAGEMAKER_TRAINING";

/// The only `ServiceEnvironmentType` the model declares
/// (`smithy_query(query="shape", shape="ServiceEnvironmentType")`).
const ENVIRONMENT_TYPE: &str = "SAGEMAKER_TRAINING";

/// The statuses a service job this emulator accepted passes through, in order: it is
/// accepted in `SUBMITTED`, the scheduler may place it in `RUNNABLE`, and — nothing here
/// running SageMaker AI — it ends in the modelled failure state. Each value is one of the
/// model's own `ServiceJobStatus` values (`smithy_query(query="shape",
/// shape="ServiceJobStatus")`, which lists them alphabetically, so the ORDER here is the
/// lifecycle the service's own status docs describe, not the enum's listing order).
const STATUS_CHAIN: &[&str] = &[
    state::SERVICE_JOB_SUBMITTED,
    state::SERVICE_JOB_RUNNABLE,
    state::SERVICE_JOB_FAILED,
];

/// The rank of `status` in [`STATUS_CHAIN`]; panics for a status this emulator never puts
/// a service job in, which is itself the assertion.
fn status_rank(status: &str, detail: &Value) -> usize {
    STATUS_CHAIN
        .iter()
        .position(|known| *known == status)
        .unwrap_or_else(|| {
            panic!("case status chain: {status:?} is not a status this emulator mints, got {detail}")
        })
}

/// A service environment called `name`; answers its ARN.
async fn service_environment(batch: &Batch, name: &str) -> String {
    let body = batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": name,
                "serviceEnvironmentType": ENVIRONMENT_TYPE,
                "capacityLimits": [{"maxCapacity": 10, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    body["serviceEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A job queue of type `SAGEMAKER_TRAINING` over the service environment `environment`
/// (named by name or by ARN, whichever the caller passes); answers its ARN.
async fn sagemaker_queue(batch: &Batch, name: &str, environment: &str) -> String {
    let body = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": name,
                "priority": 1,
                "jobQueueType": state::JOB_QUEUE_TYPE_SAGEMAKER_TRAINING,
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": environment}],
            }),
        )
        .await;
    body["jobQueueArn"].as_str().unwrap_or_default().to_string()
}

/// One service environment and one `SAGEMAKER_TRAINING` queue over it; answers the
/// queue's name and ARN.
async fn environment_and_queue(batch: &Batch) -> (String, String) {
    let environment = service_environment(batch, "se-jobs").await;
    let queue_arn = sagemaker_queue(batch, "q-service-jobs", &environment).await;
    ("q-service-jobs".to_string(), queue_arn)
}

/// `SubmitServiceJob` over the wire: the status and the decoded body.
async fn submit(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/submitservicejob", body).await
}

/// `SubmitServiceJob`, requiring the 200.
async fn submitted(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = submit(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected SubmitServiceJob to answer 200, got {status} {decoded}"
    );
    decoded
}

/// `DescribeServiceJob`, requiring the 200.
async fn described(batch: &Batch, case: &str, job_id: &str) -> Value {
    let (status, body) = batch
        .call("/v1/describeservicejob", json!({"jobId": job_id}))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeServiceJob to answer 200, got {status} {body}"
    );
    body
}

/// How many service jobs the store holds, so a refusal can be shown to persist NOTHING.
fn stored_service_jobs(batch: &Batch) -> usize {
    batch.store().lock().service_jobs.len()
}

/// Assert `id` has the shape AWS Batch gives a job id: a lowercase version-4 UUID.
fn assert_uuid_shaped(case: &str, id: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case}: expected a 8-4-4-4-12 UUID-shaped jobId, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            && id.chars().all(|c| !c.is_ascii_uppercase()),
        "case {case}: expected lowercase hex groups, got {id:?}"
    );
    assert_eq!(
        groups[2].chars().next(),
        Some('4'),
        "case {case}: expected the UUID version nibble 4, got {id:?}"
    );
}

/// How the row names the queue it submits to: the two forms `jobQueue` documents ("You
/// can specify either the name or the ARN of the queue").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueRef {
    Name,
    Arn,
}

/// One accepted submit: the body it sends and what the read-back must carry.
struct Row {
    /// What this row varies; named in every assertion.
    label: &'static str,
    queue_ref: QueueRef,
    job_name: String,
    payload: String,
    /// Optional members, each left OUT of the request when `None`.
    scheduling_priority: Option<i64>,
    share_identifier: Option<&'static str>,
    quota_share_name: Option<&'static str>,
    retry_attempts: Option<i64>,
    attempt_duration_seconds: Option<i64>,
    preemption_retries: Option<i64>,
    tags: Option<Value>,
}

impl Row {
    /// The request body for this row, with every member it does not carry left out.
    fn body(&self, queue: &str) -> Value {
        let mut body = json!({
            "jobName": self.job_name,
            "jobQueue": queue,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": self.payload,
        });
        if let Some(priority) = self.scheduling_priority {
            body["schedulingPriority"] = json!(priority);
        }
        if let Some(share) = self.share_identifier {
            body["shareIdentifier"] = json!(share);
        }
        if let Some(quota) = self.quota_share_name {
            body["quotaShareName"] = json!(quota);
        }
        if let Some(attempts) = self.retry_attempts {
            body["retryStrategy"] = json!({
                "attempts": attempts,
                "evaluateOnExit": [{"action": "RETRY", "onStatusReason": "Internal*"}],
            });
        }
        if let Some(seconds) = self.attempt_duration_seconds {
            body["timeoutConfig"] = json!({"attemptDurationSeconds": seconds});
        }
        if let Some(retries) = self.preemption_retries {
            body["preemptionConfiguration"] = json!({"preemptionRetriesBeforeTermination": retries});
        }
        if let Some(tags) = self.tags.as_ref() {
            body["tags"] = tags.clone();
        }
        body
    }
}

/// The table: rows differ along what the rule generalises over — how the queue is named,
/// the `jobName`'s length and alphabet, the payload's size, which optional members are
/// carried, the values on the published bounds of `schedulingPriority` and
/// `retryStrategy.attempts`, and how many tags there are.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "queue by name, only the required members",
            queue_ref: QueueRef::Name,
            job_name: "sj-minimal".to_string(),
            payload: r#"{"TrainingJobName":"t-1"}"#.to_string(),
            scheduling_priority: None,
            share_identifier: None,
            quota_share_name: None,
            retry_attempts: None,
            attempt_duration_seconds: None,
            preemption_retries: None,
            tags: None,
        },
        Row {
            label: "queue by ARN, every optional member, priority at the minimum",
            queue_ref: QueueRef::Arn,
            job_name: "SJ_Everything-1".to_string(),
            payload: r#"{"TrainingJobName":"t-2","InstanceCount":2}"#.to_string(),
            // 0 is the lowest `schedulingPriority` the member documents.
            scheduling_priority: Some(i64::from(state::MIN_SCHEDULING_PRIORITY)),
            share_identifier: Some("share-a"),
            quota_share_name: Some("quota-a"),
            // 1 is the fewest attempts `ServiceJobRetryStrategy$attempts` documents.
            retry_attempts: Some(i64::from(state::MIN_SERVICE_JOB_RETRY_ATTEMPTS)),
            attempt_duration_seconds: Some(600),
            preemption_retries: Some(2),
            tags: Some(json!({"env": "test", "team": "batch"})),
        },
        Row {
            label: "both published maxima, a long payload, one tag",
            queue_ref: QueueRef::Name,
            job_name: "a".repeat(state::MAX_JOB_NAME_LEN),
            payload: format!(
                r#"{{"TrainingJobName":"t-3","Note":"{}"}}"#,
                "x".repeat(4096)
            ),
            scheduling_priority: Some(i64::from(state::MAX_SCHEDULING_PRIORITY)),
            share_identifier: None,
            quota_share_name: None,
            retry_attempts: Some(i64::from(state::MAX_SERVICE_JOB_RETRY_ATTEMPTS)),
            attempt_duration_seconds: Some(60),
            preemption_retries: None,
            tags: Some(json!({"only": "one"})),
        },
        Row {
            label: "a one-character jobName and a payload that is not an object",
            queue_ref: QueueRef::Arn,
            job_name: "j".to_string(),
            payload: "\"a bare JSON string\"".to_string(),
            scheduling_priority: Some(123),
            share_identifier: Some("s"),
            quota_share_name: None,
            retry_attempts: Some(5),
            attempt_duration_seconds: None,
            preemption_retries: Some(0),
            tags: None,
        },
    ]
}

/// RULE: a submit persists what the request asked for, under a fresh id and ARN, and
/// `DescribeServiceJob` reads it back member for member — the optional members it carried
/// present and the ones it did not ABSENT.
#[tokio::test]
async fn submitting_a_service_job_persists_it_and_describe_reads_it_back() {
    let batch = Batch::new();
    let (queue_name, queue_arn) = environment_and_queue(&batch).await;
    let mut seen_ids: Vec<String> = Vec::new();

    for row in rows() {
        let case = row.label;
        let queue = match row.queue_ref {
            QueueRef::Name => queue_name.clone(),
            QueueRef::Arn => queue_arn.clone(),
        };
        let accepted = submitted(&batch, case, row.body(&queue)).await;

        let job_id = accepted["jobId"].as_str().unwrap_or_default().to_string();
        assert_uuid_shaped(case, &job_id);
        assert!(
            !seen_ids.contains(&job_id),
            "case {case}: every submit mints its own jobId, got the repeat {job_id:?}"
        );
        seen_ids.push(job_id.clone());
        assert_eq!(
            accepted["jobName"].as_str(),
            Some(row.job_name.as_str()),
            "case {case}: the answer echoes the jobName submitted, got {accepted}"
        );
        assert_eq!(
            accepted["jobArn"].as_str(),
            Some(arn(&format!("service-job/{job_id}")).as_str()),
            "case {case}: expected the service-job ARN of the minted id, got {accepted}"
        );

        let detail = described(&batch, case, &job_id).await;
        assert_eq!(
            detail["jobArn"].as_str(),
            accepted["jobArn"].as_str(),
            "case {case}: the ARN is minted once, got {detail}"
        );
        assert_eq!(
            detail["jobName"].as_str(),
            Some(row.job_name.as_str()),
            "case {case}: expected the submitted jobName, got {detail}"
        );
        assert_eq!(
            detail["jobQueue"].as_str(),
            Some(queue_arn.as_str()),
            "case {case}: jobQueue is the ARN the request's queue reference resolved to, \
             got {detail}"
        );
        assert_eq!(
            detail["serviceJobType"].as_str(),
            Some(SERVICE_JOB_TYPE),
            "case {case}: expected the submitted serviceJobType, got {detail}"
        );
        assert_eq!(
            detail["serviceRequestPayload"].as_str(),
            Some(row.payload.as_str()),
            "case {case}: the payload is persisted verbatim, got {detail}"
        );
        assert_eq!(
            detail["status"].as_str(),
            Some(state::SERVICE_JOB_SUBMITTED),
            "case {case}: a service job is accepted in SUBMITTED, got {detail}"
        );
        assert_eq!(
            detail["isTerminated"].as_bool(),
            Some(false),
            "case {case}: nothing terminated this job, got {detail}"
        );
        assert!(
            detail["createdAt"].as_i64().is_some_and(|created| created > 0),
            "case {case}: expected a createdAt in epoch milliseconds, got {detail}"
        );
        // Nothing dispatched the job's work, so none of what a dispatch would produce is
        // invented for it.
        for member in [
            "scheduledAt",
            "stoppedAt",
            "statusReason",
            "attempts",
            "latestAttempt",
            "capacityUsage",
            "preemptionSummary",
        ] {
            assert_eq!(
                detail[member], Value::Null,
                "case {case}: nothing dispatched this job, so {member} must be absent, \
                 got {detail}"
            );
        }
        assert_eq!(
            detail["startedAt"].as_i64(),
            Some(0),
            "case {case}: startedAt is modelled required, so an unstarted job reports 0, \
             got {detail}"
        );

        // The optional members, each read back exactly as the row carried it — or absent.
        assert_eq!(
            detail["schedulingPriority"].as_i64(),
            row.scheduling_priority,
            "case {case}: expected the submitted schedulingPriority, got {detail}"
        );
        assert_eq!(
            detail["shareIdentifier"].as_str(),
            row.share_identifier,
            "case {case}: expected the submitted shareIdentifier, got {detail}"
        );
        assert_eq!(
            detail["quotaShareName"].as_str(),
            row.quota_share_name,
            "case {case}: expected the submitted quotaShareName, got {detail}"
        );
        assert_eq!(
            detail["retryStrategy"]["attempts"].as_i64(),
            row.retry_attempts,
            "case {case}: expected the submitted retryStrategy, got {detail}"
        );
        if row.retry_attempts.is_some() {
            assert_eq!(
                detail["retryStrategy"]["evaluateOnExit"],
                json!([{"action": "RETRY", "onStatusReason": "Internal*"}]),
                "case {case}: expected the submitted evaluateOnExit, got {detail}"
            );
        }
        assert_eq!(
            detail["timeoutConfig"]["attemptDurationSeconds"].as_i64(),
            row.attempt_duration_seconds,
            "case {case}: expected the submitted timeoutConfig, got {detail}"
        );
        assert_eq!(
            detail["preemptionConfiguration"]["preemptionRetriesBeforeTermination"].as_i64(),
            row.preemption_retries,
            "case {case}: expected the submitted preemptionConfiguration, got {detail}"
        );
        assert_eq!(
            detail["tags"],
            row.tags.clone().unwrap_or(Value::Null),
            "case {case}: expected the submitted tags, got {detail}"
        );
    }

    assert_eq!(
        stored_service_jobs(&batch),
        rows().len(),
        "case the store: every accepted submit persisted exactly one service job"
    );
}

/// RULE: the `jobQueue` must resolve to a queue this service holds, and that queue must
/// have the type `SAGEMAKER_TRAINING`; otherwise the submit answers the modelled
/// `ClientException` (HTTP 400) and persists nothing. Rows: the forms an unresolvable
/// reference takes, and queues of every other `JobQueueType` the model declares — plus a
/// queue created without one at all.
#[tokio::test]
async fn submitting_a_service_job_needs_a_sagemaker_training_queue_that_exists() {
    let batch = Batch::new();
    let (queue_name, queue_arn) = environment_and_queue(&batch).await;

    // Queues that exist but are not of the type a service job needs. The other
    // `JobQueueType` values the model declares
    // (`smithy_query(query="shape", shape="JobQueueType")`: EKS, ECS, ECS_FARGATE,
    // SAGEMAKER_TRAINING, ECS_MANAGED_INSTANCES), plus a queue whose create named none.
    let environment_arn = compute_environment(&batch, "ce-wrong-type").await;
    let mut wrong_type: Vec<(String, String)> = Vec::new();
    for queue_type in ["EKS", "ECS", "ECS_FARGATE", "ECS_MANAGED_INSTANCES"] {
        let name = format!("q-{}", queue_type.to_lowercase());
        batch
            .ok(
                "/v1/createjobqueue",
                json!({
                    "jobQueueName": &name,
                    "priority": 1,
                    "jobQueueType": queue_type,
                    "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment_arn}],
                }),
            )
            .await;
        wrong_type.push((format!("a {queue_type} queue"), name));
    }
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "q-no-type",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment_arn}],
            }),
        )
        .await;
    wrong_type.push(("a queue created with no jobQueueType".to_string(), "q-no-type".to_string()));

    let unresolvable: Vec<(String, String)> = vec![
        ("a word".to_string(), "does-not-exist".to_string()),
        (
            "the queue's name with one character changed".to_string(),
            format!("{queue_name}x"),
        ),
        (
            "a job queue ARN for a name nothing holds".to_string(),
            state::job_queue_arn(REGION, "q-absent"),
        ),
        ("one character".to_string(), "q".to_string()),
        (
            "a service environment's ARN rather than a queue's".to_string(),
            state::service_environment_arn(REGION, "se-jobs"),
        ),
    ];

    let before = stored_service_jobs(&batch);
    let rows = unresolvable
        .iter()
        .map(|(case, queue)| (case, queue, true))
        .chain(wrong_type.iter().map(|(case, queue)| (case, queue, false)));
    for (case, queue, not_found) in rows {
        let (status, body) = submit(
            &batch,
            json!({
                "jobName": "sj-refused",
                "jobQueue": queue,
                "serviceJobType": SERVICE_JOB_TYPE,
                "serviceRequestPayload": "{}",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException status, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {body}"
        );
        if not_found {
            // The lookup missed, so the message is the service's own not-found sentence,
            // built from the reference the request gave.
            assert_eq!(
                error_message(&body),
                format!("JobQueue {queue} not found."),
                "case {case}: expected the not-found wording naming the reference given, \
                 got {body}"
            );
        } else {
            assert!(
                error_message(&body).contains(queue.as_str()),
                "case {case}: the refusal must name the queue the request gave, got {body}"
            );
            assert!(
                error_message(&body)
                    .contains(state::JOB_QUEUE_TYPE_SAGEMAKER_TRAINING),
                "case {case}: the refusal must name the type a service job queue needs, \
                 got {body}"
            );
        }
        assert_eq!(
            stored_service_jobs(&batch),
            before,
            "case {case}: a refused submit persists nothing"
        );
    }

    // The same request against the SAGEMAKER_TRAINING queue is accepted, so the rows above
    // were refused for their queue and nothing else.
    let accepted = submitted(
        &batch,
        "the SAGEMAKER_TRAINING queue",
        json!({
            "jobName": "sj-accepted",
            "jobQueue": &queue_arn,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": "{}",
        }),
    )
    .await;
    assert!(
        accepted["jobId"].as_str().is_some_and(|id| !id.is_empty()),
        "case the SAGEMAKER_TRAINING queue: expected a minted jobId, got {accepted}"
    );
    assert_eq!(
        stored_service_jobs(&batch),
        before + 1,
        "case the SAGEMAKER_TRAINING queue: exactly one service job was persisted"
    );
}

/// RULE: a member outside the range or charset its own doc publishes is refused with the
/// modelled `ClientException` and persists nothing, and a value ON either published bound
/// is accepted. The rows walk `jobName` (1..128 letters, numbers, hyphens, underscores),
/// `schedulingPriority` (0..9999) and `retryStrategy.attempts` (1..10), each at both
/// bounds and one past each of them.
#[tokio::test]
async fn submitting_a_service_job_applies_the_bounds_its_members_document() {
    let batch = Batch::new();
    let (_, queue_arn) = environment_and_queue(&batch).await;

    // (case, the member overrides, accepted?)
    let longest_name = "n".repeat(state::MAX_JOB_NAME_LEN);
    let too_long_name = "n".repeat(state::MAX_JOB_NAME_LEN + 1);
    let rows: Vec<(&str, Value, bool)> = vec![
        ("jobName of one character", json!({"jobName": "n"}), true),
        (
            "jobName at the documented maximum",
            json!({"jobName": longest_name}),
            true,
        ),
        (
            "jobName with hyphens and underscores",
            json!({"jobName": "a-b_C-9"}),
            true,
        ),
        (
            "jobName one character past the maximum",
            json!({"jobName": too_long_name}),
            false,
        ),
        ("an empty jobName", json!({"jobName": ""}), false),
        (
            "a jobName with a space",
            json!({"jobName": "not a name"}),
            false,
        ),
        (
            "a jobName with a slash",
            json!({"jobName": "not/a/name"}),
            false,
        ),
        (
            "schedulingPriority at the documented minimum",
            json!({"schedulingPriority": state::MIN_SCHEDULING_PRIORITY}),
            true,
        ),
        (
            "schedulingPriority at the documented maximum",
            json!({"schedulingPriority": state::MAX_SCHEDULING_PRIORITY}),
            true,
        ),
        (
            "schedulingPriority one below the minimum",
            json!({"schedulingPriority": state::MIN_SCHEDULING_PRIORITY - 1}),
            false,
        ),
        (
            "schedulingPriority one above the maximum",
            json!({"schedulingPriority": state::MAX_SCHEDULING_PRIORITY + 1}),
            false,
        ),
        (
            "retryStrategy.attempts at the documented minimum",
            json!({"retryStrategy": {"attempts": state::MIN_SERVICE_JOB_RETRY_ATTEMPTS}}),
            true,
        ),
        (
            "retryStrategy.attempts at the documented maximum",
            json!({"retryStrategy": {"attempts": state::MAX_SERVICE_JOB_RETRY_ATTEMPTS}}),
            true,
        ),
        (
            "retryStrategy.attempts one below the minimum",
            json!({"retryStrategy": {"attempts": state::MIN_SERVICE_JOB_RETRY_ATTEMPTS - 1}}),
            false,
        ),
        (
            "retryStrategy.attempts one above the maximum",
            json!({"retryStrategy": {"attempts": state::MAX_SERVICE_JOB_RETRY_ATTEMPTS + 1}}),
            false,
        ),
    ];

    for (case, overrides, accepted) in rows {
        let mut body = json!({
            "jobName": "sj-bounds",
            "jobQueue": &queue_arn,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": "{}",
        });
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            body[member] = value.clone();
        }
        let before = stored_service_jobs(&batch);
        let (status, answer) = submit(&batch, body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: a value on a published bound is accepted, got {status} {answer}"
            );
            assert_eq!(
                stored_service_jobs(&batch),
                before + 1,
                "case {case}: an accepted submit persists exactly one service job"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled ClientException status, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ClientException",
                "case {case}: expected the modelled ClientException, got {answer}"
            );
            assert_eq!(
                stored_service_jobs(&batch),
                before,
                "case {case}: a refused submit persists nothing"
            );
        }
    }
}

/// RULE: `clientToken` makes a submit idempotent — the identical repeat is the same
/// request, so it resolves to the job the first one minted and mints no second one; the
/// same token carrying a DIFFERENT request is refused with the modelled
/// `ClientException`; and two different tokens mint two jobs. The rows differ in WHICH
/// member the second request changes.
#[tokio::test]
async fn a_client_token_makes_submitting_a_service_job_idempotent() {
    let base = |queue: &str| {
        json!({
            "jobName": "sj-token",
            "jobQueue": queue,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": r#"{"TrainingJobName":"t"}"#,
            "schedulingPriority": 7,
            "tags": {"env": "test", "team": "batch"},
            "clientToken": "token-1",
        })
    };

    // Each row: what the repeat changes, and whether that still counts as the same request.
    /// One row's change to the repeated request body.
    type Change = fn(&mut Value);
    let changes: Vec<(&str, Change)> = vec![
        ("nothing at all", |_body| {}),
        ("the tag order only", |body| {
            // The same tags, written the other way round: a map, so the request is
            // identical and the repeat must still resolve to the first job.
            body["tags"] = json!({"team": "batch", "env": "test"});
        }),
        ("the jobName", |body| body["jobName"] = json!("sj-token-2")),
        ("the payload", |body| {
            body["serviceRequestPayload"] = json!(r#"{"TrainingJobName":"u"}"#);
        }),
        ("the schedulingPriority", |body| {
            body["schedulingPriority"] = json!(8);
        }),
        ("a tag's value", |body| {
            body["tags"] = json!({"env": "prod", "team": "batch"});
        }),
        ("by adding a member the first did not carry", |body| {
            body["shareIdentifier"] = json!("share-a");
        }),
    ];
    let identical: &[&str] = &["nothing at all", "the tag order only"];

    for (case, change) in changes {
        let batch = Batch::new();
        let (_, queue_arn) = environment_and_queue(&batch).await;

        let first = submitted(&batch, case, base(&queue_arn)).await;
        let first_id = first["jobId"].as_str().unwrap_or_default().to_string();

        let mut repeat = base(&queue_arn);
        change(&mut repeat);
        let (status, answer) = submit(&batch, repeat).await;

        if identical.contains(&case) {
            assert_eq!(
                status, 200,
                "case the repeat changes {case}: the identical repeat is the same request, \
                 got {status} {answer}"
            );
            assert_eq!(
                answer["jobId"].as_str(),
                Some(first_id.as_str()),
                "case the repeat changes {case}: expected the first job's id, got {answer}"
            );
            assert_eq!(
                answer["jobArn"].as_str(),
                first["jobArn"].as_str(),
                "case the repeat changes {case}: expected the first job's ARN, got {answer}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case the repeat changes {case}: one token cannot name two different \
                 requests, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ClientException",
                "case the repeat changes {case}: expected the modelled ClientException, \
                 got {answer}"
            );
        }
        assert_eq!(
            stored_service_jobs(&batch),
            1,
            "case the repeat changes {case}: one clientToken mints at most one service job"
        );

        // A DIFFERENT token for the same payload is a different request, and mints its own
        // job.
        let mut other = base(&queue_arn);
        other["clientToken"] = json!("token-2");
        let second = submitted(&batch, case, other).await;
        assert_ne!(
            second["jobId"].as_str(),
            Some(first_id.as_str()),
            "case the repeat changes {case}: a different clientToken mints its own job, \
             got {second}"
        );
        assert_eq!(
            stored_service_jobs(&batch),
            2,
            "case the repeat changes {case}: the second token's job is persisted beside the \
             first"
        );
    }
}

/// RULE: a submitted service job is handed back in the non-terminal `SUBMITTED` status and
/// only the CLOCK moves it — a read never does — forward through the statuses this
/// emulator mints, ending in the modelled failure state with the reason that nothing here
/// dispatched its work. Nothing a dispatch would have produced is invented on the way.
#[tokio::test]
async fn a_submitted_service_job_moves_only_with_the_clock_and_never_because_of_a_read() {
    let batch = Batch::new();
    let (queue_name, _) = environment_and_queue(&batch).await;
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);

    let accepted = submitted(
        &batch,
        "the clock",
        json!({
            "jobName": "sj-clock",
            "jobQueue": &queue_name,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": "{}",
        }),
    )
    .await;
    let job_id = accepted["jobId"].as_str().unwrap_or_default().to_string();

    // Two reads back to back: a read must not advance anything, and the transient status
    // is observable because the span the clock owes is never zero.
    let before = described(&batch, "read twice", &job_id).await;
    let after = described(&batch, "read twice", &job_id).await;
    assert_eq!(
        before, after,
        "case read twice: two reads at the same instant must answer identically, got {after}"
    );
    assert_eq!(
        before["status"].as_str(),
        Some(state::SERVICE_JOB_SUBMITTED),
        "case straight after submit: expected SUBMITTED, got {before}"
    );

    // Poll to the end, checking the status never goes backwards and only ever takes a
    // value this emulator mints.
    let mut rank = 0_usize;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut detail = before;
    for _ in 0..12 {
        detail = described(&batch, "poll", &job_id).await;
        let status = detail["status"].as_str().unwrap_or_default().to_string();
        let observed = status_rank(&status, &detail);
        assert!(
            observed >= rank,
            "case poll: the status went backwards to {status:?}, got {detail}"
        );
        rank = observed;
        *seen.entry(status).or_default() += 1;
        if rank == STATUS_CHAIN.len() - 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(span / 2)).await;
    }

    assert_eq!(
        detail["status"].as_str(),
        Some(state::SERVICE_JOB_FAILED),
        "case poll to the end: nothing here runs SageMaker AI, so the job ends in the \
         modelled failure state, got {detail}"
    );
    assert_eq!(
        detail["statusReason"].as_str(),
        Some(state::NO_SERVICE_JOB_RUNTIME_REASON),
        "case poll to the end: the failure must say why nothing ran, got {detail}"
    );
    assert!(
        seen.contains_key(state::SERVICE_JOB_RUNNABLE),
        "case poll to the end: the job passes through RUNNABLE, saw {seen:?}"
    );
    assert!(
        detail["stoppedAt"]
            .as_i64()
            .is_some_and(|stopped| stopped >= detail["createdAt"].as_i64().unwrap_or_default()),
        "case poll to the end: expected stoppedAt at or after createdAt, got {detail}"
    );
    assert_eq!(
        detail["isTerminated"].as_bool(),
        Some(false),
        "case poll to the end: no TerminateServiceJob ended this job, got {detail}"
    );
    // Nothing dispatched the work, so nothing a dispatch would produce is reported.
    for member in [
        "scheduledAt",
        "attempts",
        "latestAttempt",
        "capacityUsage",
        "preemptionSummary",
    ] {
        assert_eq!(
            detail[member], Value::Null,
            "case poll to the end: {member} must not be invented, got {detail}"
        );
    }
    assert_eq!(
        detail["startedAt"].as_i64(),
        Some(0),
        "case poll to the end: the job never started, so startedAt stays 0, got {detail}"
    );

    // A terminal service job stays exactly where it is.
    tokio::time::sleep(Duration::from_millis(span * 2)).await;
    let later = described(&batch, "after the end", &job_id).await;
    assert_eq!(
        later, detail,
        "case after the end: a terminal service job never changes again, got {later}"
    );
}

/// RULE: a `SAGEMAKER_TRAINING` queue's `serviceEnvironmentOrder` resolves each entry by
/// name OR by ARN — the two forms `ServiceEnvironmentOrder$serviceEnvironment` documents
/// ("The name or ARN of the service environment") — and stores the environment's ARN,
/// which is what `DescribeJobQueues` reports; an entry that resolves to nothing is the
/// modelled `ClientException` (HTTP 400) and no queue is created, so a service job cannot
/// be submitted to it either.
#[tokio::test]
async fn a_service_job_queue_resolves_its_service_environments_by_name_or_arn() {
    let batch = Batch::new();
    let environment_arn = service_environment(&batch, "se-resolve").await;

    for (case, reference) in [
        ("by name", "se-resolve".to_string()),
        ("by ARN", environment_arn.clone()),
    ] {
        let name = format!("q-{}", case.replace(' ', "-"));
        let queue_arn = sagemaker_queue(&batch, &name, &reference).await;
        let described = batch
            .ok("/v1/describejobqueues", json!({"jobQueues": [&queue_arn]}))
            .await;
        assert_eq!(
            described["jobQueues"][0]["serviceEnvironmentOrder"],
            json!([{"order": 1, "serviceEnvironment": &environment_arn}]),
            "case a queue naming its service environment {case}: the order is stored as the \
             environment's ARN, got {described}"
        );

        // And the queue accepts a service job, which is what the type is for.
        let accepted = submitted(
            &batch,
            case,
            json!({
                "jobName": "sj-resolved",
                "jobQueue": &queue_arn,
                "serviceJobType": SERVICE_JOB_TYPE,
                "serviceRequestPayload": "{}",
            }),
        )
        .await;
        assert!(
            accepted["jobId"].as_str().is_some_and(|id| !id.is_empty()),
            "case a queue naming its service environment {case}: expected a minted jobId, \
             got {accepted}"
        );
    }

    // An entry that resolves to nothing: the forms an unknown reference takes.
    let unknown: Vec<(&str, String)> = vec![
        ("a word", "does-not-exist".to_string()),
        (
            "the environment's name with one character changed",
            "se-resolv".to_string(),
        ),
        (
            "a service environment ARN nothing holds",
            state::service_environment_arn(REGION, "se-absent"),
        ),
        (
            "the environment's ARN with one hex digit flipped",
            flip_last_hex_digit(&environment_arn),
        ),
    ];
    for (case, reference) in unknown {
        let name = "q-unresolvable";
        let (status, body) = batch
            .call(
                "/v1/createjobqueue",
                json!({
                    "jobQueueName": name,
                    "priority": 1,
                    "jobQueueType": state::JOB_QUEUE_TYPE_SAGEMAKER_TRAINING,
                    "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": &reference}],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case a queue naming {case}: expected the modelled ClientException status, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case a queue naming {case}: expected the modelled ClientException, got {body}"
        );
        assert!(
            !batch.store().lock().job_queues.contains_key(name),
            "case a queue naming {case}: a refused create persists no queue"
        );

        let (status, body) = submit(
            &batch,
            json!({
                "jobName": "sj-no-queue",
                "jobQueue": name,
                "serviceJobType": SERVICE_JOB_TYPE,
                "serviceRequestPayload": "{}",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case submitting to the queue that was not created ({case}): expected the \
             modelled ClientException status, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case submitting to the queue that was not created ({case}): expected the \
             modelled ClientException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("JobQueue {name} not found."),
            "case submitting to the queue that was not created ({case}): expected the \
             not-found wording naming the reference given, got {body}"
        );
    }
}
