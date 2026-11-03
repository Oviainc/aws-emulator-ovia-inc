// SPDX-License-Identifier: CC0-1.0

//! AWS Batch publishes a `Batch Job State Change` event for every job transition.
//!
//! The RULE under test: every status a Batch job MOVES INTO — each move the clock
//! materialises, and the terminal status a cancel or a terminate writes — is published
//! once onto the account's DEFAULT EventBridge bus as `source` `aws.batch` /
//! `detail-type` `Batch Job State Change`, carrying that job's own description as the
//! event `detail` and the job's ARN in `resources`; the status the submit itself stored is
//! not a move, so no event carries it (the exclusion has its own suite,
//! `job_submission_publishes_no_event`). EventBridge then
//! routes it: a rule whose `EventPattern` matches delivers it to its targets, and a rule
//! that does not match receives nothing. Source: the EventBridge reference page "AWS Batch
//! events" (`events-ref-batch.html`), which names the source, the detail type and the
//! default bus, and `smithy_query(query="shape", shape="JobStatus")` for the statuses a
//! job can take.
//!
//! The tests assert that rule over a TABLE of job lifecycles, which differ along what the
//! rule generalises over — WHICH statuses a job takes and WHICH operation materialises
//! them: a job left to the clock (`RUNNABLE`, published by the
//! reader that materialises it), a job terminated before it was placed, a job cancelled
//! before it was placed, and a job that waits on another (`PENDING`). Every row's
//! published `detail` is checked against what `DescribeJobs` reports for the same job, so
//! the event is the job's own state and not a second, invented copy of it.
//!
//! The negative half of the contract is asserted in the same rows (the non-matching
//! rule's queue is never a delivery target), and the boundary that makes the `aws.batch`
//! source trustworthy has its own case: a CLIENT publishing under a reserved `aws.*`
//! source is still refused, because only the in-process service path may claim it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue, Batch,
    REGION,
};

/// The `source` and `detail-type` AWS Batch publishes its job events under, spelled out
/// here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass. Source: "AWS Batch events" (the EventBridge reference), whose job state
/// change event carries `"source": "aws.batch"` and
/// `"detail-type": "Batch Job State Change"`.
const BATCH_EVENT_SOURCE: &str = "aws.batch";
const JOB_STATE_CHANGE: &str = "Batch Job State Change";

/// The rule that matches a Batch job event, and the rule that must not.
const MATCHING_RULE: &str = "batch-job-state-change";
const OTHER_RULE: &str = "unrelated-events";

/// One published event, projected onto what this suite asserts.
#[derive(Debug, Clone)]
pub(crate) struct Published {
    pub(crate) source: String,
    pub(crate) detail_type: String,
    pub(crate) resources: Vec<String>,
    pub(crate) detail: Value,
    pub(crate) matched_rules: Vec<String>,
}

/// Every event EventBridge has routed, oldest first, as this suite reads them.
pub(crate) fn published(batch: &Batch) -> Vec<Published> {
    batch
        .event_bus_store()
        .lock()
        .delivered
        .iter()
        .map(|event| Published {
            source: event.source.clone().unwrap_or_default(),
            detail_type: event.detail_type.clone().unwrap_or_default(),
            resources: event.resources.clone(),
            detail: event
                .detail
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or(Value::Null),
            matched_rules: event.matched_rules.clone(),
        })
        .collect()
}

/// The events published for ONE job, oldest first.
pub(crate) fn published_for(batch: &Batch, job_id: &str) -> Vec<Published> {
    published(batch)
        .into_iter()
        .filter(|event| event.detail["jobId"] == json!(job_id))
        .collect()
}

/// An SQS queue called `name`, created through SQS itself and carrying the grant a
/// rule's target needs — EventBridge makes the target's `sqs:SendMessage` as its own
/// service principal and "relies on resource-based policies" for a queue
/// (`PutTargets`), so a queue that has granted it nothing receives nothing; answers
/// its url and the ARN a rule names it by.
pub(crate) async fn given_queue(batch: &Batch, name: &str) -> (String, String) {
    let (status, body) = batch.sqs("CreateQueue", json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = batch
        .sqs(
            "GetQueueAttributes",
            json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    let (status, body) = batch
        .sqs(
            "SetQueueAttributes",
            json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name}: SetQueueAttributes answered {status} {body}"
    );
    (url, arn)
}

/// Every message waiting on the queue at `url`, as parsed JSON bodies.
pub(crate) async fn drain(batch: &Batch, url: &str) -> Vec<Value> {
    let (status, body) = batch
        .sqs(
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Install the two rules every row runs against: one whose pattern matches a Batch job
/// state change, one whose pattern cannot. Each gets its own SQS queue as its target, so
/// a delivery is attributable to the rule that matched; answers the two queue urls,
/// matching one first.
pub(crate) async fn given_rules(batch: &Batch) -> (String, String) {
    let (matching_url, matching_arn) = given_queue(batch, "batch-events-queue").await;
    let (other_url, other_arn) = given_queue(batch, "unrelated-queue").await;
    let rows = [
        (
            MATCHING_RULE,
            json!({"source": [BATCH_EVENT_SOURCE], "detail-type": [JOB_STATE_CHANGE]}).to_string(),
            matching_arn,
        ),
        (
            OTHER_RULE,
            // A different service's events: nothing Batch publishes can match it.
            json!({"source": ["aws.ecs"], "detail-type": ["ECS Task State Change"]}).to_string(),
            other_arn,
        ),
    ];
    for (rule, pattern, queue) in rows {
        let (status, body) = batch
            .events(
                "PutRule",
                json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
            )
            .await;
        assert_eq!(status, 200, "case {rule}: PutRule answered {status} {body}");
        let (status, body) = batch
            .events(
                "PutTargets",
                json!({
                    "Rule": rule,
                    "Targets": [{ "Id": "t1", "Arn": queue }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {rule}: PutTargets answered {status} {body}"
        );
    }
    (matching_url, other_url)
}

/// Submit a job onto `queue` against `definition`, optionally waiting on `depends_on`;
/// answers its id and ARN.
pub(crate) async fn submit(
    batch: &Batch,
    name: &str,
    queue: &str,
    definition: &str,
    depends_on: &[&str],
) -> (String, String) {
    let mut request = json!({
        "jobName": name,
        "jobQueue": queue,
        "jobDefinition": definition,
    });
    if !depends_on.is_empty() {
        request["dependsOn"] = Value::Array(
            depends_on
                .iter()
                .map(|id| json!({ "jobId": id, "type": "SEQUENTIAL" }))
                .collect(),
        );
    }
    let body = batch.ok("/v1/submitjob", request).await;
    (
        body["jobId"].as_str().unwrap_or_default().to_string(),
        body["jobArn"].as_str().unwrap_or_default().to_string(),
    )
}

/// Long enough for every clock-owed transition of a job with no dependencies to fall
/// due: each takes [`state::JOB_PHASE_MILLIS`], a plain job makes ONE (`SUBMITTED` ->
/// `RUNNABLE`, where it rests), and the margin covers a slow machine.
pub(crate) async fn wait_past_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// What one row of the table does to a job after it is submitted, and the statuses the
/// job therefore takes.
#[derive(Debug, Clone, Copy)]
enum Row {
    /// Left to the clock, then read back: it moves into RUNNABLE, where the job rests
    /// because nothing here places it on compute capacity.
    LeftToTheClock,
    /// Terminated while still `SUBMITTED`.
    Terminated,
    /// Cancelled while still `SUBMITTED`.
    Cancelled,
    /// Submitted waiting on a job that has already failed, so it is placed in `PENDING`
    /// and then fails on its dependency.
    WaitingOnADependency,
}

impl Row {
    /// The statuses the job MOVES INTO, in order — the status it was submitted in is
    /// not one of them, so no event reports it.
    fn statuses(self) -> &'static [&'static str] {
        match self {
            Row::LeftToTheClock => &["RUNNABLE"],
            Row::Terminated | Row::Cancelled => &["FAILED"],
            Row::WaitingOnADependency => &["PENDING", "FAILED"],
        }
    }
}

/// Every job transition is published to the default bus, reaches the rule whose pattern
/// matches it, and reaches no other rule.
#[tokio::test]
async fn every_job_state_change_is_published_to_the_matching_rule_only() {
    let rows = [
        ("a job left to the clock", Row::LeftToTheClock),
        ("a job terminated before it was placed", Row::Terminated),
        ("a job cancelled before it was placed", Row::Cancelled),
        ("a job waiting on a failed dependency", Row::WaitingOnADependency),
    ];

    for (case, row) in rows {
        let batch = Batch::new();
        let (matching_queue, other_queue) = given_rules(&batch).await;
        let environment = compute_environment(&batch, "ce-events").await;
        let queue = job_queue(&batch, "jq-events", &environment).await;
        let (definition, _) = job_definition(&batch, "jd-events", container_properties()).await;

        // The dependency row needs a job that has already finished, so its own statuses
        // are driven by the dependency's outcome rather than by this row's clock. Nothing
        // places a job here, so a caller is what takes the dependency terminal.
        let dependency = if matches!(row, Row::WaitingOnADependency) {
            let (id, _) = submit(&batch, "dependency", &queue, &definition, &[]).await;
            let _ = batch
                .ok(
                    "/v1/terminatejob",
                    json!({ "jobId": &id, "reason": "end the dependency" }),
                )
                .await;
            let detail = describe_job(&batch, &id).await;
            assert_eq!(
                detail["status"],
                json!("FAILED"),
                "case {case}: the dependency must have finished before the dependent is \
                 submitted, got {detail}"
            );
            vec![id]
        } else {
            Vec::new()
        };
        let waits: Vec<&str> = dependency.iter().map(String::as_str).collect();
        let (job_id, job_arn) = submit(&batch, "events-job", &queue, &definition, &waits).await;

        match row {
            Row::LeftToTheClock | Row::WaitingOnADependency => {
                wait_past_the_clock().await;
                // A reader materialises what the clock owed and publishes those moves.
                let _ = describe_job(&batch, &job_id).await;
            }
            Row::Terminated => {
                let _ = batch
                    .ok(
                        "/v1/terminatejob",
                        json!({ "jobId": job_id, "reason": "terminated by the test" }),
                    )
                    .await;
            }
            Row::Cancelled => {
                let _ = batch
                    .ok(
                        "/v1/canceljob",
                        json!({ "jobId": job_id, "reason": "cancelled by the test" }),
                    )
                    .await;
            }
        }

        let events = published_for(&batch, &job_id);
        let statuses: Vec<String> = events
            .iter()
            .map(|event| {
                event.detail["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            statuses,
            row.statuses(),
            "case {case}: expected one event per status in {:?}, got {statuses:?}",
            row.statuses()
        );

        for event in &events {
            assert_eq!(
                event.source, BATCH_EVENT_SOURCE,
                "case {case}: expected source {BATCH_EVENT_SOURCE}, got {:?}",
                event.source
            );
            assert_eq!(
                event.detail_type, JOB_STATE_CHANGE,
                "case {case}: expected detail-type {JOB_STATE_CHANGE}, got {:?}",
                event.detail_type
            );
            assert_eq!(
                event.resources,
                vec![job_arn.clone()],
                "case {case}: expected the job's ARN in resources, got {:?}",
                event.resources
            );
            // The matching rule, and ONLY it: the other rule's pattern names another
            // service's events, so it must receive nothing.
            assert_eq!(
                event.matched_rules,
                vec![MATCHING_RULE.to_string()],
                "case {case}: expected only {MATCHING_RULE} to match, got {:?}",
                event.matched_rules
            );
        }

        // The delivery half of the contract, over the wire: the matching rule's QUEUE
        // holds one message per published event, each carrying the whole event envelope,
        // and the non-matching rule's queue holds nothing.
        let delivered: Vec<Value> = drain(&batch, &matching_queue)
            .await
            .into_iter()
            // The dependency row submits a second job, whose own events reach the same
            // queue: this row is about the job under test.
            .filter(|message| message["detail"]["jobId"] == json!(job_id))
            .collect();
        let delivered_statuses: Vec<String> = delivered
            .iter()
            .map(|event| {
                event["detail"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            delivered_statuses,
            row.statuses(),
            "case {case}: expected the matching rule's queue to receive one message per \
             status in {:?}, got {delivered_statuses:?}",
            row.statuses()
        );
        for message in &delivered {
            // The body is the whole event envelope AWS delivers, not just the detail.
            assert_eq!(
                message["source"],
                json!(BATCH_EVENT_SOURCE),
                "case {case}: expected the envelope's source, got {message}"
            );
            assert_eq!(
                message["detail-type"],
                json!(JOB_STATE_CHANGE),
                "case {case}: expected the envelope's detail-type, got {message}"
            );
            assert_eq!(
                message["account"],
                json!(engine::EMULATOR_ACCOUNT_ID),
                "case {case}: expected the envelope's account, got {message}"
            );
            assert_eq!(
                message["region"],
                json!(REGION),
                "case {case}: expected the envelope's region, got {message}"
            );
            assert_eq!(
                message["resources"],
                json!([job_arn.clone()]),
                "case {case}: expected the job's ARN in the envelope, got {message}"
            );
            assert_eq!(
                message["detail"]["jobId"],
                json!(job_id),
                "case {case}: expected the job's id in the detail, got {message}"
            );
            assert!(
                message["id"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case}: expected the envelope to carry an event id, got {message}"
            );
        }
        let withheld = drain(&batch, &other_queue).await;
        assert!(
            withheld.is_empty(),
            "case {case}: the non-matching rule's queue must receive nothing, got {withheld:?}"
        );

        // The detail is the job's OWN description, not a second copy of it: EVERY
        // member the event and the `DescribeJobs` reader both carry must agree, so a
        // later change to one projection and not the other is caught here rather than
        // in whichever handful of members a test happened to name.
        let detail = describe_job(&batch, &job_id).await;
        let last = events.last().expect("at least one published event");
        let published = last
            .detail
            .as_object()
            .expect("the published detail is a JSON object");
        let reported = detail
            .as_object()
            .expect("the DescribeJobs detail is a JSON object");
        let mut shared = 0usize;
        for (member, value) in published {
            let Some(theirs) = reported.get(member) else {
                continue;
            };
            shared += 1;
            assert_eq!(
                value, theirs,
                "case {case}: the published {member} must be the job's own, got {value:?} \
                 against {theirs:?}"
            );
        }
        // The members that must be in both, so the comparison above cannot pass by
        // comparing nothing.
        for member in [
            "jobId",
            "jobName",
            "jobQueue",
            "jobDefinition",
            "status",
            "createdAt",
            "isCancelled",
            "isTerminated",
            "container",
        ] {
            assert!(
                published.contains_key(member) && reported.contains_key(member),
                "case {case}: expected {member} in both the event detail {:?} and the \
                 DescribeJobs detail {detail:?}",
                last.detail
            );
        }
        assert!(
            shared >= 9,
            "case {case}: expected the event detail to share its members with the reader, \
             got {shared} shared of {:?}",
            last.detail
        );
        assert_eq!(
            last.detail["attempts"],
            json!([]),
            "case {case}: no container ran, so attempts must be empty, got {:?}",
            last.detail["attempts"]
        );
        // A DELIBERATE difference between the two projections, pinned here so it
        // cannot drift unnoticed: the event detail carries `attempts` present and
        // empty (the "AWS Batch events" sample event shows `"attempts": []`), while
        // `JobDetail` leaves the member out for a job with no attempt — which that
        // operation's own suite pins.
        assert!(
            reported.get("attempts").is_none(),
            "case {case}: DescribeJobs reports no attempts for a job nothing ran, got {detail:?}"
        );
        assert_eq!(
            last.detail["container"]["image"],
            json!("busybox"),
            "case {case}: the published container must be the job's effective container, got {:?}",
            last.detail["container"]
        );
    }
}

/// The published event is routed by its CONTENT, so a rule that filters on the job's
/// detail (its queue, its status) sees only the events that satisfy the filter.
#[tokio::test]
async fn a_rule_filtering_on_the_job_detail_sees_only_the_matching_transitions() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-filtered").await;
    let queue = job_queue(&batch, "jq-filtered", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-filtered", container_properties()).await;

    // One rule per status of the chain a plain job walks, plus rules naming statuses no
    // event reports: the status the submit stored (never published) and two this job
    // never takes (nothing places it, so it rests in RUNNABLE). Each must see exactly
    // the transitions its pattern names, and each delivers to its own queue, so what a
    // rule saw is observable from the queue too.
    let rows = [
        ("submitted-only", "SUBMITTED", 0),
        ("runnable-only", "RUNNABLE", 1),
        ("failed-only", "FAILED", 0),
        ("succeeded-only", "SUCCEEDED", 0),
    ];
    let mut urls = Vec::new();
    for (rule, status, _) in rows {
        let (url, target) = given_queue(&batch, rule).await;
        urls.push(url);
        let pattern = json!({
            "source": [BATCH_EVENT_SOURCE],
            "detail-type": [JOB_STATE_CHANGE],
            "detail": { "status": [status] },
        })
        .to_string();
        let (code, body) = batch
            .events(
                "PutRule",
                json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
            )
            .await;
        assert_eq!(code, 200, "case {rule}: PutRule answered {code} {body}");
        let (code, body) = batch
            .events(
                "PutTargets",
                json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": target }] }),
            )
            .await;
        assert_eq!(code, 200, "case {rule}: PutTargets answered {code} {body}");
    }

    let (job_id, _) = submit(&batch, "filtered-job", &queue, &definition, &[]).await;
    wait_past_the_clock().await;
    let _ = describe_job(&batch, &job_id).await;

    let events = published_for(&batch, &job_id);
    for ((rule, status, expected), url) in rows.iter().zip(&urls) {
        let matched = events
            .iter()
            .filter(|event| event.matched_rules.contains(&(*rule).to_string()))
            .count();
        assert_eq!(
            matched, *expected,
            "case {rule}: expected {expected} event(s) with detail.status {status}, got \
             {matched} of {:?}",
            events
                .iter()
                .map(|event| event.detail["status"].clone())
                .collect::<Vec<_>>()
        );
        let received = drain(&batch, url).await;
        assert_eq!(
            received.len(),
            *expected,
            "case {rule}: expected {expected} message(s) on the rule's queue, got {received:?}"
        );
        for message in &received {
            assert_eq!(
                message["detail"]["status"],
                json!(status),
                "case {rule}: the rule's queue must hold only its own status, got {message}"
            );
        }
    }
}

/// The transitions the CLOCK owes are published as they fall due, with NO further Batch
/// call: a client that watches a job through EventBridge — the documented, polling-free
/// way to observe one — sees the whole sequence on its queue.
#[tokio::test]
async fn the_clock_owed_transitions_arrive_without_any_further_batch_call() {
    let batch = Batch::new();
    let (matching_queue, other_queue) = given_rules(&batch).await;
    let environment = compute_environment(&batch, "ce-watched").await;
    let queue = job_queue(&batch, "jq-watched", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-watched", container_properties()).await;

    let (job_id, _) = submit(&batch, "watched-job", &queue, &definition, &[]).await;

    // From here on NOTHING calls Batch again: every message below is one the clock
    // produced. The poll is bounded; the statuses a plain job takes are its whole chain.
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut statuses: Vec<String> = Vec::new();
    for _ in 0..40 {
        for message in drain(&batch, &matching_queue).await {
            if message["detail"]["jobId"] == json!(job_id) {
                statuses.push(
                    message["detail"]["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        if statuses.iter().any(|status| status == "RUNNABLE") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(span / 5)).await;
    }

    assert_eq!(
        statuses,
        vec!["RUNNABLE".to_string()],
        "expected the whole clock-owed chain delivered without a further Batch call, got \
         {statuses:?}"
    );
    let withheld = drain(&batch, &other_queue).await;
    assert!(
        withheld.is_empty(),
        "the non-matching rule's queue must receive nothing, got {withheld:?}"
    );
}

/// The `aws.` namespace stays closed to CLIENTS: only an emulated service publishing its
/// own events in-process may claim it, so a caller that forges one is refused per entry
/// and nothing is routed.
///
/// The per-entry code is the one the model lists for a source the caller may not publish
/// under (`PutEventsResultEntry$ErrorCode`).
#[tokio::test]
async fn a_client_cannot_publish_under_a_reserved_aws_source() {
    let batch = Batch::new();
    let (status, body) = batch
        .events(
            "PutRule",
            json!({
                "Name": MATCHING_RULE,
                "EventPattern": json!({"source": [BATCH_EVENT_SOURCE]}).to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    // Rows differ along the source a client sends: the service's own namespace, another
    // service's, the partner namespace, and — as the control — a namespace a client owns.
    let rows = [
        ("the Batch namespace", BATCH_EVENT_SOURCE, true),
        ("another service's namespace", "aws.ecs", true),
        ("the partner namespace", "aws.partner/example/1", true),
        ("a namespace the client owns", "com.example.aws.batch", false),
    ];
    for (case, source, refused) in rows {
        let (status, body) = batch
            .events(
                "PutEvents",
                json!({
                    "Entries": [{
                        "Source": source,
                        "DetailType": JOB_STATE_CHANGE,
                        "Detail": "{\"status\":\"SUCCEEDED\"}",
                    }],
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case}: PutEvents answered {status} {body}");
        let expected_failures = i64::from(refused);
        assert_eq!(
            body["FailedEntryCount"].as_i64(),
            Some(expected_failures),
            "case {case}: expected FailedEntryCount {expected_failures}, got {body}"
        );
        if refused {
            assert_eq!(
                body["Entries"][0]["ErrorCode"],
                json!("NotAuthorizedForSourceException"),
                "case {case}: expected the per-entry refusal code, got {body}"
            );
        }
    }

    // Only the client's own event was routed: nothing a client sent under `aws.*` is in
    // the log, so the rule matching `aws.batch` saw nothing.
    let sources: Vec<String> = published(&batch)
        .into_iter()
        .map(|event| event.source)
        .collect();
    assert_eq!(
        sources,
        vec!["com.example.aws.batch".to_string()],
        "expected only the client's own source to be routed, got {sources:?}"
    );
}

/// A job that is never submitted publishes nothing: the queue of changes is written by a
/// transition, not by a call, so a Batch call that moves no job routes no event.
#[tokio::test]
async fn a_call_that_moves_no_job_publishes_nothing() {
    let batch = Batch::new();
    let (matching_queue, _) = given_rules(&batch).await;
    let environment = compute_environment(&batch, "ce-quiet").await;
    let queue = job_queue(&batch, "jq-quiet", &environment).await;
    let _ = job_definition(&batch, "jd-quiet", container_properties()).await;

    // Readers and a refused submit, with no job in the store to move.
    let _ = batch.ok("/v1/describejobs", json!({ "jobs": ["none"] })).await;
    let _ = batch
        .ok("/v1/listjobs", json!({ "jobQueue": queue, "jobStatus": "RUNNABLE" }))
        .await;
    let (status, _) = batch
        .call(
            "/v1/submitjob",
            json!({ "jobName": "no-such", "jobQueue": queue, "jobDefinition": "missing" }),
        )
        .await;
    assert_eq!(status, 400, "a submit naming no definition is refused");
    let (status, _) = batch
        .call("/v1/terminatejob", json!({ "jobId": "none", "reason": "nothing" }))
        .await;
    assert_eq!(status, 200, "terminating an unknown job still succeeds");

    let events = published(&batch);
    assert!(
        events.is_empty(),
        "expected no event for calls that moved no job, got {events:?}"
    );
    let delivered = drain(&batch, &matching_queue).await;
    assert!(
        delivered.is_empty(),
        "expected the rule's queue to receive nothing, got {delivered:?}"
    );
}
