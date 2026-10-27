// SPDX-License-Identifier: CC0-1.0

//! `GetJobQueueSnapshot`: what a job queue's own snapshot reports, end to end over the
//! wire.
//!
//! The rule under test: **a job queue's snapshot lists the first
//! [`state::MAX_FRONT_OF_QUEUE_JOBS`] `RUNNABLE` jobs of the queue the request names, in
//! the order that queue dispatches them — submission order, with a job's own
//! `schedulingPriority` deciding first (the higher priority dispatching earlier) only on a
//! queue whose attached scheduling policy is a fair-share one — each carrying the instant
//! it reached its position; the first `RUNNABLE` job of each share is reported only for a
//! queue whose attached policy is a quota-share one, and a queue the service holds nothing
//! for is refused with the modelled `ClientException`.**
//!
//! Each suite asserts that over a SET of inputs: which policy kind the queue carries, what
//! priorities its jobs were submitted with (none, equal, mixed, each documented bound),
//! which shares they name, and how many jobs the queue holds (up to one past the published
//! hundred).

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, job_definition,
    until_status, Batch,
};

/// Which scheduling policy a row attaches to its job queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// None at all: a first-in-first-out queue.
    Unscheduled,
    /// A fair-share policy — the kind `JobDetail$schedulingPriority` documents as the only
    /// one a job's priority affects.
    FairShare,
    /// A quota-share policy, whose one documented `idleResourceAssignmentStrategy` is
    /// `FIFO`.
    QuotaShare,
}

/// One emulator with the compute environment and the container job definition every row
/// submits through, so a row differs only in its queue and its jobs.
struct Fixture {
    batch: Batch,
    environment: String,
    definition: String,
}

async fn fixture() -> Fixture {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-snapshot").await;
    let (definition, _) = job_definition(&batch, "jd-snapshot", container_properties()).await;
    Fixture {
        batch,
        environment,
        definition,
    }
}

/// A queue called `name` with `policy` attached; answers its ARN.
async fn job_queue(fixture: &Fixture, name: &str, policy: Policy) -> String {
    let policy_body = match policy {
        Policy::Unscheduled => None,
        Policy::FairShare => Some(json!({
            "name": format!("sp-fair-{name}"),
            "fairsharePolicy": {"shareDecaySeconds": 600, "computeReservation": 25},
        })),
        Policy::QuotaShare => Some(json!({
            "name": format!("sp-quota-{name}"),
            "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
        })),
    };
    let mut body = json!({
        "jobQueueName": name,
        "priority": 1,
        "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &fixture.environment}],
    });
    if let Some(policy_body) = policy_body {
        let created = fixture
            .batch
            .ok("/v1/createschedulingpolicy", policy_body)
            .await;
        body["schedulingPolicyArn"] = json!(created["arn"].as_str().unwrap_or_default());
    }
    let created = fixture.batch.ok("/v1/createjobqueue", body).await;
    created["jobQueueArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One submitted job: the id a poll addresses it by and the ARN the snapshot reports.
struct Submitted {
    id: String,
    arn: String,
}

/// Submit one job called `name` into `queue_arn`, optionally overriding its scheduling
/// priority and naming its share.
async fn submit(
    fixture: &Fixture,
    queue_arn: &str,
    name: &str,
    priority: Option<i32>,
    share: Option<&str>,
) -> Submitted {
    let mut body = json!({
        "jobName": name,
        "jobQueue": queue_arn,
        "jobDefinition": &fixture.definition,
    });
    if let Some(priority) = priority {
        body["schedulingPriorityOverride"] = json!(priority);
    }
    if let Some(share) = share {
        body["shareIdentifier"] = json!(share);
    }
    let answered = fixture.batch.ok("/v1/submitjob", body).await;
    let id = answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {answered}"))
        .to_string();
    let arn = answered["jobArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobArn, got {answered}"))
        .to_string();
    Submitted { id, arn }
}

/// The snapshot of `queue`, requiring the 200.
async fn snapshot(batch: &Batch, queue: &str) -> Value {
    batch
        .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": queue}))
        .await
}

/// The `jobArn`s of `frontOfQueue`, in the order the snapshot listed them.
fn front_of_queue(snapshot: &Value) -> Vec<String> {
    snapshot["frontOfQueue"]["jobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| job["jobArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Wait until every job in `submitted` rests in `RUNNABLE`, which is where a job this
/// emulator cannot place comes to rest.
async fn until_all_runnable(fixture: &Fixture, case: &str, submitted: &[Submitted]) {
    for job in submitted {
        until_status(&fixture.batch, case, &job.id, state::JOB_RUNNABLE).await;
    }
}

/// One row of the dispatch-order table.
struct Row {
    case: &'static str,
    policy: Policy,
    /// The jobs this row submits, in submission order: each one's `schedulingPriority`
    /// override and its `shareIdentifier`.
    jobs: &'static [(Option<i32>, Option<&'static str>)],
    /// The positions in `jobs`, in the order the queue must dispatch them.
    dispatched: &'static [usize],
}

/// The dispatch orders the rule fixes. The priorities span the range
/// `JobDetail$schedulingPriority` publishes — "The minimum supported value is 0 and the
/// maximum supported value is 9999", i.e. [`state::MIN_SCHEDULING_PRIORITY`] to
/// [`state::MAX_SCHEDULING_PRIORITY`] — and both of its bounds appear, beside an absent
/// priority and a priority every job shares.
const ROWS: &[Row] = &[
    Row {
        case: "no scheduling policy: submission order",
        policy: Policy::Unscheduled,
        jobs: &[(None, None), (None, None), (None, None)],
        dispatched: &[0, 1, 2],
    },
    Row {
        case: "no scheduling policy: a priority changes nothing",
        policy: Policy::Unscheduled,
        jobs: &[(Some(1), None), (Some(9999), None), (Some(500), None)],
        dispatched: &[0, 1, 2],
    },
    Row {
        case: "a fair-share policy, no priority anywhere: submission order",
        policy: Policy::FairShare,
        jobs: &[(None, None), (None, None), (None, None)],
        dispatched: &[0, 1, 2],
    },
    Row {
        case: "a fair-share policy: the higher priority dispatches first",
        policy: Policy::FairShare,
        jobs: &[(Some(1), None), (Some(9999), None), (Some(500), None)],
        dispatched: &[1, 2, 0],
    },
    Row {
        case: "a fair-share policy: one priority across two shares keeps submission order",
        policy: Policy::FairShare,
        jobs: &[
            (Some(7), Some("teamA")),
            (Some(7), Some("teamB")),
            (Some(7), Some("teamA")),
        ],
        dispatched: &[0, 1, 2],
    },
    Row {
        case: "a fair-share policy: an absent priority is the documented minimum",
        policy: Policy::FairShare,
        jobs: &[
            (None, None),
            (Some(state::MIN_SCHEDULING_PRIORITY), None),
            (Some(1), None),
        ],
        dispatched: &[2, 0, 1],
    },
    Row {
        case: "a fair-share policy: the documented bounds, highest first",
        policy: Policy::FairShare,
        jobs: &[
            (Some(state::MIN_SCHEDULING_PRIORITY), None),
            (Some(state::MAX_SCHEDULING_PRIORITY), None),
            (Some(state::MIN_SCHEDULING_PRIORITY), None),
        ],
        dispatched: &[1, 0, 2],
    },
    Row {
        case: "a quota-share policy: FIFO, so a priority changes nothing",
        policy: Policy::QuotaShare,
        jobs: &[(Some(1), Some("alpha")), (Some(9999), Some("beta"))],
        dispatched: &[0, 1],
    },
];

/// RULE: the snapshot lists the queue's `RUNNABLE` jobs in the order the queue dispatches
/// them — submission order, with a job's priority deciding first on a fair-share queue —
/// each carrying the instant it reached its position, and a second read reports exactly the
/// same queue.
#[tokio::test]
async fn the_snapshot_lists_the_queue_in_the_order_it_dispatches() {
    let fixture = fixture().await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let queue_arn = job_queue(&fixture, &format!("q-order-{index}"), row.policy).await;
        let mut submitted: Vec<Submitted> = Vec::new();
        for (position, (priority, share)) in row.jobs.iter().enumerate() {
            submitted.push(
                submit(
                    &fixture,
                    &queue_arn,
                    &format!("j{index}-{position}"),
                    *priority,
                    *share,
                )
                .await,
            );
        }
        until_all_runnable(&fixture, case, &submitted).await;

        let answered = snapshot(&fixture.batch, &queue_arn).await;
        let listed = front_of_queue(&answered);
        let expected: Vec<String> = row
            .dispatched
            .iter()
            .map(|position| submitted[*position].arn.clone())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the snapshot to list the queue in dispatch order, got \
             {answered}"
        );
        for (position, job) in answered["frontOfQueue"]["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            assert!(
                job["earliestTimeAtPosition"].as_i64().unwrap_or_default() > 0,
                "case {case:?}: job {position} must report when it reached its position, got \
                 {job}"
            );
        }
        assert!(
            answered["frontOfQueue"]["lastUpdatedAt"]
                .as_i64()
                .unwrap_or_default()
                > 0,
            "case {case:?}: the front of the queue must report when it was computed, got \
             {answered}"
        );
        // Nothing dispatches a job here, so the queue reports no capacity in use.
        assert_eq!(
            answered["queueUtilization"]["totalCapacityUsage"],
            json!([]),
            "case {case:?}: expected no capacity in use, got {answered}"
        );

        // A read leaves the queue exactly as it was: the same jobs, in the same order, at
        // the same positions.
        let again = snapshot(&fixture.batch, &queue_arn).await;
        assert_eq!(
            again["frontOfQueue"]["jobs"], answered["frontOfQueue"]["jobs"],
            "case {case:?}: a second snapshot must report the same queue, got {again}"
        );
    }
}

/// RULE: the first `RUNNABLE` job of each share is reported only for a queue whose attached
/// scheduling policy is a quota-share one, and then exactly one job per share the queue's
/// jobs name — a job naming no share is in no share at all.
#[tokio::test]
async fn the_front_of_each_quota_share_is_reported_only_for_a_quota_share_queue() {
    // (case, the policy on the queue, whether frontOfQuotaShares is reported)
    let rows: &[(&str, Policy, bool)] = &[
        ("no scheduling policy", Policy::Unscheduled, false),
        ("a fair-share policy", Policy::FairShare, false),
        ("a quota-share policy", Policy::QuotaShare, true),
    ];

    for (index, (case, policy, reported)) in rows.iter().enumerate() {
        let fixture = fixture().await;
        let queue_arn = job_queue(&fixture, &format!("q-shares-{index}"), *policy).await;
        // Two jobs in one share, one in another, one in none: the share a job names is
        // what puts it in a share.
        let shares: &[Option<&str>] = &[Some("teamA"), Some("teamB"), Some("teamA"), None];
        let mut submitted: Vec<Submitted> = Vec::new();
        for (position, share) in shares.iter().enumerate() {
            submitted
                .push(submit(&fixture, &queue_arn, &format!("s{position}"), None, *share).await);
        }
        until_all_runnable(&fixture, case, &submitted).await;

        let answered = snapshot(&fixture.batch, &queue_arn).await;
        assert_eq!(
            front_of_queue(&answered),
            submitted.iter().map(|job| job.arn.clone()).collect::<Vec<_>>(),
            "case {case:?}: every resting job is listed in submission order, got {answered}"
        );

        if !*reported {
            assert_eq!(
                answered["frontOfQuotaShares"],
                Value::Null,
                "case {case:?}: a queue with no quota share reports no front of quota \
                 shares, got {answered}"
            );
            continue;
        }

        let fronts = answered["frontOfQuotaShares"]["quotaShares"].clone();
        // Only the shares the queue's jobs NAME, so the job naming none adds nothing.
        let mut named: Vec<String> = fronts
            .as_object()
            .map(|shares| shares.keys().cloned().collect())
            .unwrap_or_default();
        named.sort();
        assert_eq!(
            named,
            vec!["teamA".to_string(), "teamB".to_string()],
            "case {case:?}: expected one entry per named share, got {answered}"
        );
        for (share, first) in [("teamA", &submitted[0]), ("teamB", &submitted[1])] {
            let listed = fronts[share].as_array().cloned().unwrap_or_default();
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: share {share} reports its FIRST job alone, got {answered}"
            );
            assert_eq!(
                listed[0]["jobArn"].as_str(),
                Some(first.arn.as_str()),
                "case {case:?}: share {share} must report the job it dispatches first, got \
                 {answered}"
            );
            assert!(
                listed[0]["earliestTimeAtPosition"]
                    .as_i64()
                    .unwrap_or_default()
                    > 0,
                "case {case:?}: share {share} must report when its job reached its \
                 position, got {answered}"
            );
        }
        assert!(
            answered["frontOfQuotaShares"]["lastUpdatedAt"]
                .as_i64()
                .unwrap_or_default()
                > 0,
            "case {case:?}: the front of the quota shares must report when it was computed, \
             got {answered}"
        );
    }
}

/// RULE: however many jobs rest in the queue, the snapshot lists at most the first
/// [`state::MAX_FRONT_OF_QUEUE_JOBS`] of them — the published hundred — and those are the
/// first hundred in dispatch order, not any hundred.
#[tokio::test]
async fn the_snapshot_lists_at_most_the_published_hundred_runnable_jobs() {
    const CASE: &str = "one job past the published hundred";

    let fixture = fixture().await;
    let queue_arn = job_queue(&fixture, "q-cap", Policy::Unscheduled).await;
    let mut submitted: Vec<Submitted> = Vec::new();
    for position in 0..=state::MAX_FRONT_OF_QUEUE_JOBS {
        submitted.push(submit(&fixture, &queue_arn, &format!("c{position}"), None, None).await);
    }
    until_all_runnable(&fixture, CASE, &submitted).await;

    let answered = snapshot(&fixture.batch, &queue_arn).await;
    let listed = front_of_queue(&answered);
    assert_eq!(
        listed.len(),
        state::MAX_FRONT_OF_QUEUE_JOBS,
        "case {CASE:?}: expected {} jobs listed out of {}, got {}",
        state::MAX_FRONT_OF_QUEUE_JOBS,
        submitted.len(),
        listed.len()
    );
    let expected: Vec<String> = submitted
        .iter()
        .take(state::MAX_FRONT_OF_QUEUE_JOBS)
        .map(|job| job.arn.clone())
        .collect();
    assert_eq!(
        listed, expected,
        "case {CASE:?}: expected the FIRST hundred in dispatch order, got {listed:?}"
    );
}

/// RULE: a queue the service holds nothing for is refused with the modelled
/// `ClientException` (HTTP 400) worded from the reference the request gave, while a queue
/// it does hold answers the 200 — with an empty front of queue when nothing rests in it.
#[tokio::test]
async fn a_queue_the_service_holds_nothing_for_is_refused() {
    let fixture = fixture().await;
    let present = job_queue(&fixture, "q-present", Policy::Unscheduled).await;

    // An empty queue is not an absent one: it answers the 200 with nothing in front.
    let answered = snapshot(&fixture.batch, &present).await;
    assert_eq!(
        answered["frontOfQueue"]["jobs"],
        json!([]),
        "case an empty queue: expected an empty front of queue, got {answered}"
    );

    // References differing in form: a plain name, a well-formed queue ARN, an ARN of
    // another resource kind, and a value that is no identifier at all.
    let absent = arn("job-queue/q-absent");
    let other_kind = arn("compute-environment/q-present");
    let references: &[&str] = &["q-absent", &absent, &other_kind, "not an identifier"];
    for reference in references {
        let (status, body) = fixture
            .batch
            .call("/v1/getjobqueuesnapshot", json!({"jobQueue": reference}))
            .await;
        // `ClientException` carries `httpError` 400 in the model.
        assert_eq!(
            status, 400,
            "case {reference:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {reference:?}: expected the modelled ClientException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("JobQueue {reference} not found."),
            "case {reference:?}: expected the refusal worded from the reference given, got \
             {body}"
        );
    }
}
