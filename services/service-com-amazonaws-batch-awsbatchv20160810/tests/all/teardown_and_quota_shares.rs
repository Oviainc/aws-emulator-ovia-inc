// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for tearing a Batch resource graph down, for quota shares, and for
//! the error code and message a refused Batch request carries.
//!
//! Every case drives real HTTP (restJson1) through the router the emulator mounts, so
//! what is asserted is the response a client gets.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a `Delete*` of one of this service's name-keyed resources is a MUTATION with the
//!   preconditions the model documents: the resource must exist and its state must be
//!   `DISABLED` (a compute environment must also be disassociated from every job queue);
//!   a request that breaks one is the modelled `ClientException` (HTTP 400) and changes
//!   nothing;
//! * a delete AWS accepts answers 200 and moves the resource to `DELETING`, and the
//!   removal is owed by the CLOCK, not by the call;
//! * `CreateQuotaShare` stores a share under
//!   `job-queue/<queue>/quota-share/<name>` and `DescribeQuotaShare` reads back exactly
//!   what was stored; a `quotaShareArn` that is not a quota share ARN at all and one that
//!   is well-formed but names nothing are DIFFERENT refusals;
//! * `ListJobs` refuses only a filter-SELECTION error (no filter, or two at once); a
//!   filter TARGET it does not hold selects nothing, which is a 200 with an empty page;
//! * `TerminateJob` accepts a `jobId` the service holds no job for, and fabricates
//!   nothing for it;
//! * a refusal this service answers carries a code the Batch API declares —
//!   `BadRequestException` for its tag map, `ClientException` otherwise — never the
//!   framework's synthetic `ValidationException`, and its message is the one the refusing
//!   code states.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, job_definition,
    job_queue, Batch,
};

/// How long to wait for a clock-owed transition to fall due: the span the clock owes it
/// ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// One of the resource classes a `Delete*` operation tears down, described by the wire
/// members that differ between them. Source: `smithy_query(operation=
/// "DeleteComputeEnvironment,DeleteJobQueue,DescribeComputeEnvironments,DescribeJobQueues")`.
struct Deletable {
    kind: &'static str,
    delete_uri: &'static str,
    /// The member the delete names the resource on, which is also the member the matching
    /// `Update*` names it on.
    delete_member: &'static str,
    update_uri: &'static str,
    describe_uri: &'static str,
    /// The member the describe filters on, and the list it answers with.
    describe_filter: &'static str,
    described: &'static str,
}

const DELETABLES: &[Deletable] = &[
    Deletable {
        kind: "compute environment",
        delete_uri: "/v1/deletecomputeenvironment",
        delete_member: "computeEnvironment",
        update_uri: "/v1/updatecomputeenvironment",
        describe_uri: "/v1/describecomputeenvironments",
        describe_filter: "computeEnvironments",
        described: "computeEnvironments",
    },
    Deletable {
        kind: "job queue",
        delete_uri: "/v1/deletejobqueue",
        delete_member: "jobQueue",
        update_uri: "/v1/updatejobqueue",
        describe_uri: "/v1/describejobqueues",
        describe_filter: "jobQueues",
        described: "jobQueues",
    },
];

/// Create a resource of `class` called `name` in `requested_state`, over a compute
/// environment of its own when it is a queue; answer its ARN.
async fn create_deletable(
    batch: &Batch,
    class: &Deletable,
    name: &str,
    requested_state: &str,
) -> String {
    match class.kind {
        "compute environment" => {
            let body = batch
                .ok(
                    "/v1/createcomputeenvironment",
                    json!({
                        "computeEnvironmentName": name,
                        "type": "UNMANAGED",
                        "state": requested_state,
                    }),
                )
                .await;
            body["computeEnvironmentArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
        _ => {
            let environment = compute_environment(batch, &format!("ce-for-{name}")).await;
            let body = batch
                .ok(
                    "/v1/createjobqueue",
                    json!({
                        "jobQueueName": name,
                        "priority": 1,
                        "state": requested_state,
                        "computeEnvironmentOrder": [
                            {"order": 1, "computeEnvironment": environment},
                        ],
                    }),
                )
                .await;
            body["jobQueueArn"].as_str().unwrap_or_default().to_string()
        }
    }
}

/// The one described record for `reference`, or `None` when the service holds none.
async fn describe_one(batch: &Batch, class: &Deletable, reference: &str) -> Option<Value> {
    let body = batch
        .ok(
            class.describe_uri,
            json!({class.describe_filter: [reference]}),
        )
        .await;
    body[class.described]
        .as_array()
        .and_then(|records| records.first().cloned())
}

/// Delete `reference` and answer the status and body.
async fn delete(batch: &Batch, class: &Deletable, reference: &str) -> (u16, Value) {
    batch
        .call(class.delete_uri, json!({class.delete_member: reference}))
        .await
}

/// A `Delete*` refuses what the model's preconditions refuse, and nothing it refuses
/// changes the store: asserted for EVERY deletable class, named by name and by ARN.
#[tokio::test]
async fn a_delete_refuses_an_unknown_or_enabled_resource_and_changes_nothing() {
    let batch = Batch::new();

    for class in DELETABLES {
        // An identifier the service holds nothing for, in each of the two forms the
        // member documents ("name or ARN").
        let absent_name = format!("absent-{}", class.kind.replace(' ', "-"));
        let absent_arn = arn(&format!(
            "{}/{absent_name}",
            if class.kind == "job queue" {
                "job-queue"
            } else {
                "compute-environment"
            }
        ));
        for reference in [absent_name.as_str(), absent_arn.as_str()] {
            let (status, refused) = delete(&batch, class, reference).await;
            assert_eq!(
                status, 400,
                "case delete an unknown {} by {reference:?}: expected 400, got {status} \
                 {refused}",
                class.kind
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case delete an unknown {} by {reference:?}: expected the modelled \
                 ClientException, got {refused}",
                class.kind
            );
        }

        // An ENABLED resource: refused, and still exactly as it was afterwards.
        let name = format!("enabled-{}", class.kind.replace(' ', "-"));
        let resource_arn = create_deletable(&batch, class, &name, "ENABLED").await;
        for reference in [name.as_str(), resource_arn.as_str()] {
            let (status, refused) = delete(&batch, class, reference).await;
            assert_eq!(
                status, 400,
                "case delete an ENABLED {} by {reference:?}: expected 400, got {status} \
                 {refused}",
                class.kind
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case delete an ENABLED {} by {reference:?}: expected the modelled \
                 ClientException, got {refused}",
                class.kind
            );
            let described = describe_one(&batch, class, reference).await;
            assert!(
                described.is_some(),
                "case delete an ENABLED {} by {reference:?}: a refused delete must leave \
                 the resource, got {described:?}",
                class.kind
            );
            assert_eq!(
                described.as_ref().and_then(|record| record["state"].as_str()),
                Some("ENABLED"),
                "case delete an ENABLED {} by {reference:?}: the refused delete must not \
                 touch the state, got {described:?}",
                class.kind
            );
        }
    }
}

/// A delete the preconditions allow answers 200, moves the resource to `DELETING`, and
/// the removal itself is owed by the CLOCK: asserted for every deletable class, with the
/// delete named by name in one case and by ARN in the other.
#[tokio::test]
async fn an_accepted_delete_reports_deleting_and_the_clock_removes_the_resource() {
    let batch = Batch::new();

    for (index, class) in DELETABLES.iter().enumerate() {
        let name = format!("doomed-{}", class.kind.replace(' ', "-"));
        let resource_arn = create_deletable(&batch, class, &name, "DISABLED").await;
        // The two forms the member documents, one per class, so both are exercised.
        let reference = if index % 2 == 0 {
            name.clone()
        } else {
            resource_arn.clone()
        };

        let (status, accepted) = delete(&batch, class, &reference).await;
        assert_eq!(
            status, 200,
            "case delete a DISABLED {} by {reference:?}: expected the empty 200, got \
             {status} {accepted}",
            class.kind
        );

        // Straight after the call the resource is still there, in DELETING: the removal
        // is a transition the clock owes, and the span is never zero.
        let described = describe_one(&batch, class, &reference).await;
        assert_eq!(
            described.as_ref().and_then(|record| record["status"].as_str()),
            Some(state::STATUS_DELETING),
            "case delete a DISABLED {}: expected DELETING straight after the call, got \
             {described:?}",
            class.kind
        );

        // Repeating the delete asks for a removal that is already owed: the same success.
        let (status, repeated) = delete(&batch, class, &reference).await;
        assert_eq!(
            status, 200,
            "case delete a DELETING {}: expected the same empty 200, got {status} \
             {repeated}",
            class.kind
        );

        // An UPDATE of it, though, is refused: the delete was accepted, so there is
        // nothing left to configure, and the resource stays exactly as it is.
        let (status, refused) = batch
            .call(
                class.update_uri,
                json!({class.delete_member: reference.clone(), "state": "ENABLED"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case update a DELETING {}: expected 400, got {status} {refused}",
            class.kind
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case update a DELETING {}: expected the modelled ClientException, got \
             {refused}",
            class.kind
        );
        let described = describe_one(&batch, class, &reference).await;
        assert_eq!(
            described.as_ref().and_then(|record| record["state"].as_str()),
            Some("DISABLED"),
            "case update a DELETING {}: the refused update must write nothing, got \
             {described:?}",
            class.kind
        );

        wait_for_the_clock().await;

        let described = describe_one(&batch, class, &reference).await;
        assert!(
            described.is_none(),
            "case delete a DISABLED {}: the clock must remove it, got {described:?}",
            class.kind
        );
        // Both identifier forms stop resolving, so the delete is the modelled refusal.
        for gone in [name.as_str(), resource_arn.as_str()] {
            let (status, refused) = delete(&batch, class, gone).await;
            assert_eq!(
                status, 400,
                "case delete a removed {} by {gone:?}: expected 400, got {status} \
                 {refused}",
                class.kind
            );
        }
    }
}

/// A compute environment a job queue still names cannot be deleted — the model's own
/// precondition ("disassociate it from any job queues") — and the same delete is accepted
/// once the queue is gone.
#[tokio::test]
async fn a_compute_environment_a_queue_still_names_cannot_be_deleted() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-attached").await;
    let _queue = job_queue(&batch, "jq-attached", &environment).await;
    // The CE under test is DISABLED through `UpdateComputeEnvironment`, the way the
    // delete's own doc says to, so the only precondition left is the queue relationship.
    let disabled = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": "ce-attached-disabled",
                "type": "UNMANAGED",
            }),
        )
        .await;
    let disabled_arn = disabled["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    batch
        .ok(
            "/v1/updatecomputeenvironment",
            json!({"computeEnvironment": "ce-attached-disabled", "state": "DISABLED"}),
        )
        .await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-holding",
                "priority": 1,
                "state": "DISABLED",
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": disabled_arn}],
            }),
        )
        .await;

    let (status, refused) = batch
        .call(
            "/v1/deletecomputeenvironment",
            json!({"computeEnvironment": "ce-attached-disabled"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case delete a compute environment a queue names: expected 400, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case delete a compute environment a queue names: expected the modelled \
         ClientException, got {refused}"
    );

    // Tear the queue down, let the clock remove it, and the same delete is accepted.
    batch
        .ok("/v1/deletejobqueue", json!({"jobQueue": "jq-holding"}))
        .await;
    wait_for_the_clock().await;
    let (status, accepted) = batch
        .call(
            "/v1/deletecomputeenvironment",
            json!({"computeEnvironment": "ce-attached-disabled"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case delete a disassociated compute environment: expected the empty 200, got \
         {status} {accepted}"
    );

    // An environment whose deletion the clock already owes can never be valid again, so
    // a queue cannot be attached to it either.
    let (status, refused) = batch
        .call(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-too-late",
                "priority": 1,
                "computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": "ce-attached-disabled"},
                ],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case create a queue over a DELETING compute environment: expected 400, got \
         {status} {refused}"
    );
    assert_eq!(
        error_message(&refused),
        state::COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE,
        "case create a queue over a DELETING compute environment: expected the \
         attachability refusal, got {refused}"
    );
}

/// Deleting a job queue ends the jobs that were submitted to it — "All jobs in the queue
/// are eventually terminated when you delete a job queue" — and the jobs stay
/// describable by id.
#[tokio::test]
async fn deleting_a_job_queue_terminates_the_jobs_submitted_to_it() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-queue-jobs").await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-with-jobs",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
            }),
        )
        .await;
    let (definition, _) = job_definition(&batch, "jd-queue-jobs", container_properties()).await;

    let mut submitted = Vec::new();
    for index in 0..3 {
        let body = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("queued-{index}"),
                    "jobQueue": "jq-with-jobs",
                    "jobDefinition": definition,
                }),
            )
            .await;
        submitted.push(body["jobId"].as_str().unwrap_or_default().to_string());
    }

    // "You must first disable submissions for a queue with the UpdateJobQueue
    // operation" (`DeleteJobQueue`).
    batch
        .ok(
            "/v1/updatejobqueue",
            json!({"jobQueue": "jq-with-jobs", "state": "DISABLED"}),
        )
        .await;
    batch
        .ok("/v1/deletejobqueue", json!({"jobQueue": "jq-with-jobs"}))
        .await;
    wait_for_the_clock().await;

    let described = batch
        .ok("/v1/describejobs", json!({"jobs": submitted.clone()}))
        .await;
    let jobs = described["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        jobs.len(),
        submitted.len(),
        "case describe the jobs of a deleted queue: every job stays describable, got \
         {described}"
    );
    for job in &jobs {
        assert_eq!(
            job["status"].as_str(),
            Some(state::JOB_FAILED),
            "case job of a deleted queue: expected the terminal FAILED, got {job}"
        );
    }
    // The queue itself selects nothing any more, which is an empty page and not a
    // refusal.
    let listed = batch
        .ok("/v1/listjobs", json!({"jobQueue": "jq-with-jobs"}))
        .await;
    assert_eq!(
        listed["jobSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case list the jobs of a deleted queue: expected an empty page, got {listed}"
    );
}

/// The body of a `CreateQuotaShare` for `name` on `queue`.
fn quota_share_request(name: &str, queue: &str, extra: Value) -> Value {
    let mut body = json!({
        "quotaShareName": name,
        "jobQueue": queue,
        "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
        "resourceSharingConfiguration": {"strategy": "RESERVE"},
        "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// A created quota share is stored under its ARN and `DescribeQuotaShare` answers from
/// the record: asserted over a table spanning the sharing strategies, the capacity-limit
/// list length, both `state` values (and its absence, which defaults to `ENABLED`), tags
/// and both forms of `jobQueue` the member documents.
#[tokio::test]
async fn a_created_quota_share_is_read_back_by_its_arn() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-shares").await;
    let queue_arn = job_queue(&batch, "jq-shares", &environment).await;

    // (case, name, the jobQueue reference, the members laid over the base request)
    // Strategies from the model's `QuotaShareResourceSharingStrategy` enum
    // (`state::QUOTA_SHARE_RESOURCE_SHARING_STRATEGY_VALUES`): RESERVE, LEND,
    // LEND_AND_BORROW; `inSharePreemption` from `QuotaShareInSharePreemptionState`.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        ("the defaults, by queue name", "qs-default", "jq-shares", json!({})),
        (
            "by queue ARN, DISABLED",
            "qs-disabled",
            queue_arn.as_str(),
            json!({"state": "DISABLED"}),
        ),
        (
            "LEND with two capacity limits",
            "qs-lend",
            "jq-shares",
            json!({
                "resourceSharingConfiguration": {"strategy": "LEND"},
                "capacityLimits": [
                    {"maxCapacity": 1, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 7, "capacityUnit": "ml.c5.xlarge"},
                ],
            }),
        ),
        (
            "LEND_AND_BORROW with a borrowLimit, preemption ENABLED and tags",
            "qs_lend_and_borrow-1",
            "jq-shares",
            json!({
                "resourceSharingConfiguration": {"strategy": "LEND_AND_BORROW", "borrowLimit": 200},
                "preemptionConfiguration": {"inSharePreemption": "ENABLED"},
                "state": "ENABLED",
                "tags": {"team": "a", "cost-centre": "42"},
                "capacityLimits": [
                    {"maxCapacity": 0, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 3, "capacityUnit": "ml.m5.2xlarge"},
                    {"maxCapacity": 9, "capacityUnit": "ml.p4d.24xlarge"},
                ],
            }),
        ),
    ];

    let mut created: BTreeMap<String, String> = BTreeMap::new();
    for (case, name, queue, extra) in &cases {
        let request = quota_share_request(name, queue, extra.clone());
        let body = batch.ok("/v1/createquotashare", request.clone()).await;
        assert_eq!(
            body["quotaShareName"].as_str(),
            Some(*name),
            "case {case:?}: the create answers the name it was given, got {body}"
        );
        let expected_arn = arn(&format!("job-queue/jq-shares/quota-share/{name}"));
        assert_eq!(
            body["quotaShareArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the share's ARN under its job queue, got {body}"
        );
        created.insert((*case).to_string(), expected_arn.clone());

        let described = batch
            .ok(
                "/v1/describequotashare",
                json!({"quotaShareArn": expected_arn}),
            )
            .await;
        assert_eq!(
            described["quotaShareName"].as_str(),
            Some(*name),
            "case {case:?}: expected the stored name, got {described}"
        );
        assert_eq!(
            described["jobQueueArn"].as_str(),
            Some(queue_arn.as_str()),
            "case {case:?}: expected the queue's ARN however the create named it, got \
             {described}"
        );
        assert_eq!(
            described["capacityLimits"], request["capacityLimits"],
            "case {case:?}: expected the stored capacity limits, got {described}"
        );
        assert_eq!(
            described["resourceSharingConfiguration"], request["resourceSharingConfiguration"],
            "case {case:?}: expected the stored sharing configuration, got {described}"
        );
        assert_eq!(
            described["preemptionConfiguration"], request["preemptionConfiguration"],
            "case {case:?}: expected the stored preemption configuration, got {described}"
        );
        // "The default state is ENABLED" (`CreateQuotaShare$state`).
        let expected_state = request["state"].as_str().unwrap_or(state::STATE_ENABLED);
        assert_eq!(
            described["state"].as_str(),
            Some(expected_state),
            "case {case:?}: expected the state the create asked for, got {described}"
        );
        if let Some(tags) = request.get("tags") {
            assert_eq!(
                &described["tags"], tags,
                "case {case:?}: expected the stored tags, got {described}"
            );
        }
        // A fresh share is handed back non-terminal, and the span is never zero.
        assert_eq!(
            described["status"].as_str(),
            Some(state::STATUS_CREATING),
            "case {case:?}: expected CREATING straight after the create, got {described}"
        );
    }

    // The CLOCK, not a read, settles every one of them.
    wait_for_the_clock().await;
    for (case, share_arn) in &created {
        let described = batch
            .ok("/v1/describequotashare", json!({"quotaShareArn": share_arn}))
            .await;
        assert_eq!(
            described["status"].as_str(),
            Some(state::STATUS_VALID),
            "case {case:?}: the clock must move CREATING -> VALID, got {described}"
        );
    }
}

/// The quota-share refusals: a `jobQueue` the service does not hold, a name the queue
/// already carries, a `quotaShareArn` that is no quota share ARN at all, and one that is
/// well-formed but names nothing. The last two are DIFFERENT refusals.
#[tokio::test]
async fn the_quota_share_refusals_are_distinct_client_exceptions() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-share-refusals").await;
    job_queue(&batch, "jq-share-refusals", &environment).await;

    // (case, request) — every one a ClientException, none of them stored.
    let create_refusals: Vec<(&str, Value)> = vec![
        (
            "a job queue name nothing holds",
            quota_share_request("qs-absent-queue", "no-such-queue", json!({})),
        ),
        (
            "a job queue ARN nothing holds",
            quota_share_request(
                "qs-absent-queue-arn",
                &arn("job-queue/no-such-queue"),
                json!({}),
            ),
        ),
        (
            "an empty capacityLimits",
            quota_share_request(
                "qs-no-capacity",
                "jq-share-refusals",
                json!({"capacityLimits": []}),
            ),
        ),
        (
            "a name outside the documented charset",
            quota_share_request("qs share!", "jq-share-refusals", json!({})),
        ),
        (
            "a name one past the documented length",
            quota_share_request(
                &"q".repeat(state::MAX_QUOTA_SHARE_NAME_LEN + 1),
                "jq-share-refusals",
                json!({}),
            ),
        ),
    ];
    for (case, request) in create_refusals {
        let (status, refused) = batch.call("/v1/createquotashare", request).await;
        assert_eq!(
            status, 400,
            "case create a quota share with {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case create a quota share with {case}: expected the modelled ClientException, \
             got {refused}"
        );
    }

    // A name at the documented length bound is accepted, and the same name twice is not.
    let at_bound = "q".repeat(state::MAX_QUOTA_SHARE_NAME_LEN);
    batch
        .ok(
            "/v1/createquotashare",
            quota_share_request(&at_bound, "jq-share-refusals", json!({})),
        )
        .await;
    batch
        .ok(
            "/v1/createquotashare",
            quota_share_request("qs-taken", "jq-share-refusals", json!({})),
        )
        .await;
    let (status, refused) = batch
        .call(
            "/v1/createquotashare",
            quota_share_request("qs-taken", "jq-share-refusals", json!({})),
        )
        .await;
    assert_eq!(
        status, 400,
        "case create a quota share twice: expected 400, got {status} {refused}"
    );
    assert!(
        error_message(&refused).contains(state::OBJECT_ALREADY_EXISTS),
        "case create a quota share twice: expected the duplicate detail, got {refused}"
    );

    // A `quotaShareArn` that carries no `job-queue/<queue>/quota-share/<share>` path is
    // not a quota share ARN at all: its refusal says so.
    let malformed = [
        "not-an-arn",
        "",
        "arn:aws:batch:us-east-1:000000000000",
        &arn("job-queue/jq-share-refusals"),
        &arn("quota-share/qs-taken"),
        &arn("job-queue/jq-share-refusals/quota-share/"),
        &arn("compute-environment/ce-share-refusals"),
    ];
    for reference in malformed {
        let (status, refused) = batch
            .call(
                "/v1/describequotashare",
                json!({"quotaShareArn": reference}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case describe a quota share by {reference:?}: expected 400, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case describe a quota share by {reference:?}: expected the modelled \
             ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains("valid Quota Share arn pattern"),
            "case describe a quota share by {reference:?}: expected the ARN-pattern \
             refusal, got {message:?}"
        );
    }

    // A well-formed ARN naming nothing stored is the OTHER refusal.
    for reference in [
        arn("job-queue/jq-share-refusals/quota-share/qs-absent"),
        arn("job-queue/no-such-queue/quota-share/qs-taken"),
    ] {
        let (status, refused) = batch
            .call(
                "/v1/describequotashare",
                json!({"quotaShareArn": reference}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case describe the unknown quota share {reference:?}: expected 400, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case describe the unknown quota share {reference:?}: expected the modelled \
             ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains("does not exist"),
            "case describe the unknown quota share {reference:?}: expected the \
             nonexistence refusal, got {message:?}"
        );
        assert!(
            !message.contains("valid Quota Share arn pattern"),
            "case describe the unknown quota share {reference:?}: a well-formed ARN must \
             not be reported as malformed, got {message:?}"
        );
    }
}

/// `ListJobs` refuses a filter-SELECTION error and only that: a filter target the
/// service does not hold selects nothing, which is a 200 with an empty page.
#[tokio::test]
async fn list_jobs_pages_empty_for_an_unknown_filter_target_and_refuses_only_a_selection_error() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-listing").await;
    job_queue(&batch, "jq-listing", &environment).await;

    // Targets nothing holds, spanning the three selector members and several value
    // shapes (a name, an ARN, a UUID, a value with no jobs behind it).
    let empty_pages: Vec<(&str, Value)> = vec![
        ("an unknown queue name", json!({"jobQueue": "no-such-queue"})),
        (
            "an unknown queue ARN",
            json!({"jobQueue": arn("job-queue/no-such-queue")}),
        ),
        (
            "an unknown array parent, UUID-shaped",
            json!({"arrayJobId": "d2f30357-a68b-4772-8a03-c2a8564f2244"}),
        ),
        (
            "an unknown array parent, name-shaped",
            json!({"arrayJobId": "not-a-job"}),
        ),
        (
            "an unknown multi-node parent",
            json!({"multiNodeJobId": "0389e230-0000-4000-8000-000000000000"}),
        ),
        (
            "a known but empty queue",
            json!({"jobQueue": "jq-listing"}),
        ),
        (
            "an unknown queue with a status and a page size",
            json!({"jobQueue": "no-such-queue", "jobStatus": "SUBMITTED", "maxResults": 10}),
        ),
    ];
    for (case, request) in empty_pages {
        let (status, listed) = batch.call("/v1/listjobs", request).await;
        assert_eq!(
            status, 200,
            "case list jobs for {case}: expected 200, got {status} {listed}"
        );
        assert_eq!(
            listed["jobSummaryList"].as_array().map(Vec::len),
            Some(0),
            "case list jobs for {case}: expected an empty jobSummaryList, got {listed}"
        );
        assert!(
            listed["nextToken"].is_null(),
            "case list jobs for {case}: an empty page continues nowhere, got {listed}"
        );
    }

    // The filter-SELECTION errors: no filter, or two of the three at once.
    let selection_errors: Vec<(&str, Value)> = vec![
        ("no filter at all", json!({})),
        ("only a jobStatus", json!({"jobStatus": "RUNNING"})),
        (
            "a queue and an array parent",
            json!({"jobQueue": "jq-listing", "arrayJobId": "any"}),
        ),
        (
            "an array parent and a multi-node parent",
            json!({"arrayJobId": "any", "multiNodeJobId": "other"}),
        ),
        (
            "all three at once",
            json!({"jobQueue": "jq-listing", "arrayJobId": "any", "multiNodeJobId": "other"}),
        ),
    ];
    for (case, request) in selection_errors {
        let (status, refused) = batch.call("/v1/listjobs", request).await;
        assert_eq!(
            status, 400,
            "case list jobs with {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case list jobs with {case}: expected the modelled ClientException, got \
             {refused}"
        );
    }
}

/// `TerminateJob` accepts a `jobId` the service holds no job for, over a set of id
/// shapes, and fabricates nothing for it.
#[tokio::test]
async fn terminate_job_accepts_an_unknown_job_id_and_fabricates_nothing() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-terminate").await;
    job_queue(&batch, "jq-terminate", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-terminate", container_properties()).await;
    let submitted = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "live-one",
                "jobQueue": "jq-terminate",
                "jobDefinition": definition,
            }),
        )
        .await;
    let live = submitted["jobId"].as_str().unwrap_or_default().to_string();

    let unknown = [
        "d2f30357-a68b-4772-8a03-c2a8564f2244",
        "0389e230-0000-4000-8000-000000000000",
        "not-a-job-id",
        &arn("job/d2f30357-a68b-4772-8a03-c2a8564f2244"),
    ];
    for job_id in unknown {
        for reason in ["cleanup", "a second, repeated call"] {
            let (status, body) = batch
                .call("/v1/terminatejob", json!({"jobId": job_id, "reason": reason}))
                .await;
            assert_eq!(
                status, 200,
                "case terminate the unknown {job_id:?} ({reason}): expected the empty 200, \
                 got {status} {body}"
            );
        }
        // Nothing was created for it: the id is still unknown to DescribeJobs.
        let described = batch.ok("/v1/describejobs", json!({"jobs": [job_id]})).await;
        assert_eq!(
            described["jobs"].as_array().map(Vec::len),
            Some(0),
            "case terminate the unknown {job_id:?}: no job may be fabricated, got \
             {described}"
        );
    }

    // The job that DOES exist is still terminated by the same call, twice over.
    for reason in ["stop it", "again"] {
        let (status, body) = batch
            .call("/v1/terminatejob", json!({"jobId": live, "reason": reason}))
            .await;
        assert_eq!(
            status, 200,
            "case terminate the live job ({reason}): expected the empty 200, got {status} \
             {body}"
        );
    }
    let described = batch.ok("/v1/describejobs", json!({"jobs": [&live]})).await;
    assert_eq!(
        described["jobs"][0]["status"].as_str(),
        Some(state::JOB_FAILED),
        "case terminate the live job: expected the terminal FAILED, got {described}"
    );
    assert_eq!(
        described["jobs"][0]["statusReason"].as_str(),
        Some("stop it"),
        "case terminate the live job: the FIRST terminate's reason is kept, got \
         {described}"
    );
}

/// A tag map `tags` with `count` entries.
fn tags(count: usize) -> Value {
    let map: BTreeMap<String, String> = (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    json!(map)
}

/// Every operation that takes this service's tag map, with the smallest request that
/// reaches the tag bound. Source: `smithy_query` — `TagrisTagsMap` carries
/// `@length(min: 1, max: 50)` wherever it is targeted.
fn tagged_requests(tag_map: Value) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "CreateComputeEnvironment",
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": "ce-tagged",
                "type": "UNMANAGED",
                "tags": tag_map,
            }),
        ),
        (
            "CreateJobQueue",
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-tagged",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-tags"}],
                "tags": tag_map,
            }),
        ),
        (
            "RegisterJobDefinition",
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-tagged",
                "type": "container",
                "containerProperties": container_properties(),
                "tags": tag_map,
            }),
        ),
        (
            "CreateConsumableResource",
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": "cr-tagged",
                "totalQuantity": 1,
                "tags": tag_map,
            }),
        ),
    ]
}

/// A tag map outside the bounds `TagrisTagsMap` declares is refused with the Tagris
/// code, `BadRequestException`, on EVERY operation that takes one — and a map at the
/// bound is accepted, so the refusal is the bound and not the member.
#[tokio::test]
async fn an_out_of_bounds_tag_map_is_a_bad_request_exception() {
    // The published bounds of `TagrisTagsMap`: @length(min: 1, max: 50).
    const MIN_TAGS: usize = 1;
    const MAX_TAGS: usize = 50;

    for count in [MIN_TAGS - 1, MAX_TAGS + 1, MAX_TAGS + 10] {
        for (operation, uri, request) in tagged_requests(tags(count)) {
            let batch = Batch::new();
            compute_environment(&batch, "ce-tags").await;
            let (status, refused) = batch.call(uri, request).await;
            assert_eq!(
                status, 400,
                "case {operation} with {count} tags: expected 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "BadRequestException",
                "case {operation} with {count} tags: expected the Tagris \
                 BadRequestException, got {refused}"
            );
        }
    }

    // Each published bound itself is accepted.
    for count in [MIN_TAGS, MAX_TAGS] {
        for (operation, uri, request) in tagged_requests(tags(count)) {
            let batch = Batch::new();
            compute_environment(&batch, "ce-tags").await;
            let (status, accepted) = batch.call(uri, request).await;
            assert_eq!(
                status, 200,
                "case {operation} with {count} tags: expected the create to be accepted, \
                 got {status} {accepted}"
            );
        }
    }
}

/// Every other refusal the framework raises for this service carries the code the model
/// declares — `ClientException` — and never the framework's synthetic
/// `ValidationException`, which no Batch operation lists.
#[tokio::test]
async fn a_non_tag_boundary_refusal_is_a_client_exception() {
    let batch = Batch::new();

    // (case, uri, request): a required member absent, and a value outside a declared
    // enum — the two classes the boundary refuses before any handler runs.
    let refusals: Vec<(&str, &str, Value)> = vec![
        (
            "CreateComputeEnvironment with no name",
            "/v1/createcomputeenvironment",
            json!({"type": "UNMANAGED"}),
        ),
        (
            "CreateComputeEnvironment with no type",
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-no-type"}),
        ),
        (
            "CreateComputeEnvironment with a type outside the enum",
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-bad-type", "type": "SEMI_MANAGED"}),
        ),
        (
            "DeleteJobQueue with no jobQueue",
            "/v1/deletejobqueue",
            json!({}),
        ),
        (
            "DeleteComputeEnvironment with no computeEnvironment",
            "/v1/deletecomputeenvironment",
            json!({}),
        ),
        (
            "CreateQuotaShare with no capacityLimits",
            "/v1/createquotashare",
            json!({
                "quotaShareName": "qs-no-limits",
                "jobQueue": "jq-none",
                "resourceSharingConfiguration": {"strategy": "RESERVE"},
                "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
            }),
        ),
    ];
    for (case, uri, request) in refusals {
        let (status, refused) = batch.call(uri, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {refused}"
        );
    }
}

/// The MESSAGE of a refusal is the one the refusing code states: the envelope Batch's
/// frontend adds belongs to the refusals AWS wraps, not to all of them.
#[tokio::test]
async fn a_refusal_carries_the_message_the_refusing_code_states() {
    let batch = Batch::new();

    compute_environment(&batch, "ce-messages").await;

    // Recorded BARE refusals: the detail alone, with no envelope around it.
    let bare: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a job queue over a compute environment nothing holds",
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-missing-ce",
                "priority": 1,
                "computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": "no-such-compute-environment"},
                ],
            }),
            state::COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE,
        ),
        (
            "a job queue over a compute environment ARN nothing holds",
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-missing-ce-arn",
                "priority": 1,
                "computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": arn("compute-environment/absent")},
                ],
            }),
            state::COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE,
        ),
        (
            "a compute environment name already stored",
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-messages", "type": "UNMANAGED"}),
            state::OBJECT_ALREADY_EXISTS,
        ),
        (
            "a nextToken this service did not issue",
            "/v1/describejobdefinitions",
            json!({"nextToken": "not-a-token-we-minted"}),
            state::NEXT_TOKEN_NOT_PROCESSABLE,
        ),
        (
            "a nextToken that is empty",
            "/v1/describejobdefinitions",
            json!({"nextToken": ""}),
            state::NEXT_TOKEN_NOT_PROCESSABLE,
        ),
    ];
    for (case, uri, request, expected) in bare {
        let (status, refused) = batch.call(uri, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case}: expected the bare detail, got {refused}"
        );
    }

    // A recorded ENVELOPED refusal: the detail inside Batch's frontend envelope.
    let (status, refused) = batch
        .call(
            "/v1/registerjobdefinition",
            json!({"jobDefinitionName": "jd-no-properties", "type": "container"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a container job definition with no properties object: expected 400, got \
         {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case a container job definition with no properties object: expected the \
         modelled ClientException, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        state::client_exception_message(
            "ECS ContainerProperties, ecsProperties, or eksProperties is required when \
             using type container."
        ),
        "case a container job definition with no properties object: expected the \
         enveloped detail, got {refused}"
    );
}
