// SPDX-License-Identifier: CC0-1.0

//! A job's `Batch Job State Change` stream begins at its FIRST MOVE, never at its submit.
//!
//! The RULE under test: a job state change event reports a job the service already holds
//! MOVING into a status, so the status the creating call stored is not published at all —
//! a job's event stream begins with the first status it moves INTO and no envelope ever
//! carries the status it was created in. Source: the AWS Batch User Guide "Job state
//! change events", which creates an event "anytime that an existing (previously
//! submitted) job changes states" and states that "events aren't created for the initial
//! job submission"; the statuses themselves are the model's
//! `smithy_query(query="shape", shape="JobStatus")` enum.
//!
//! The rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over — WHICH status the job first moves into, WHICH actor moves it, and WHETHER it has
//! been driven anywhere at all: a job read back inside its own span (nothing has driven
//! it, so its first move may not have been published yet), a job the clock moves into
//! `RUNNABLE`, a job a caller terminates and a job a caller cancels before anything placed
//! it (`FAILED`), and a job that waits on a job nothing finishes (`PENDING`). Each row
//! states the statuses its job may MOVE INTO, and asserts the stream carries those moves
//! in order and nothing else — so the status the submit stored fails the row wherever it
//! appears. One further test varies the COUNT of jobs in flight, because the exclusion is
//! per job. Every row reads the job back through `DescribeJobs` straight after the submit,
//! so the absence of an event is never the absence of the status.
//!
//! The delivery half is asserted the documented way: an ENABLED rule on the default bus
//! whose pattern filters `detail.jobQueue` on the submitted queue's ARN, targeting an SQS
//! queue, and a second rule filtering on ANOTHER queue's ARN, which must receive nothing.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue,
    until_status, Batch,
};
use crate::job_state_change_events::{drain, given_queue, published_for, submit};

/// The `source` and `detail-type` AWS Batch publishes its job events under, spelled out
/// here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass. Source: "AWS Batch events" (the EventBridge reference).
const BATCH_EVENT_SOURCE: &str = "aws.batch";
const JOB_STATE_CHANGE: &str = "Batch Job State Change";

/// An ENABLED rule called `rule` on the DEFAULT bus matching a Batch job state change for
/// the job queue `job_queue_arn`, delivering to an SQS queue of its own; answers that
/// queue's url.
///
/// The pattern is the one a consumer writes to watch one queue's jobs: the source, the
/// detail type and `detail.jobQueue`.
async fn given_watcher(batch: &Batch, rule: &str, job_queue_arn: &str) -> String {
    let (url, target) = given_queue(batch, rule).await;
    let pattern = json!({
        "source": [BATCH_EVENT_SOURCE],
        "detail-type": [JOB_STATE_CHANGE],
        "detail": { "jobQueue": [job_queue_arn] },
    })
    .to_string();
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
            json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": target }] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {rule}: PutTargets answered {status} {body}"
    );
    url
}

/// Receive from the SQS queue at `url` until `enough` is satisfied by what has arrived for
/// the jobs in `job_ids`, or the poll budget runs out; answers `(jobId, detail.status)` per
/// message, oldest first.
///
/// Bounded, and driven off what has ARRIVED rather than one fixed sleep, so a row waiting
/// for a transition does not race the clock that owes it and a row waiting for nothing
/// costs one receive.
async fn delivered(
    batch: &Batch,
    url: &str,
    job_ids: &[&str],
    enough: impl Fn(&[(String, String)]) -> bool,
) -> Vec<(String, String)> {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut seen: Vec<(String, String)> = Vec::new();
    for _ in 0..40 {
        for message in drain(batch, url).await {
            let job_id = message["detail"]["jobId"].as_str().unwrap_or_default();
            if job_ids.contains(&job_id) {
                seen.push((
                    job_id.to_string(),
                    message["detail"]["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ));
            }
        }
        if enough(&seen) {
            break;
        }
        tokio::time::sleep(Duration::from_millis((span / 5).max(5))).await;
    }
    seen
}

/// Whether `observed` appears in `allowed` in order, each entry at most once.
///
/// The statuses a job's event stream may carry are the moves it makes, in the order it
/// makes them, and nothing else — so a status outside `allowed` (the one the submit
/// stored, above all), a repeat of one, or a pair out of order fails this.
fn moves_in_order(observed: &[String], allowed: &[&str]) -> bool {
    let mut next = 0usize;
    for status in observed {
        match allowed[next..].iter().position(|moved_to| moved_to == status) {
            Some(offset) => next += offset + 1,
            None => return false,
        }
    }
    true
}

/// What one row of the table does to the job after it is submitted.
#[derive(Debug, Clone, Copy)]
enum Action {
    /// Nothing at all: the job is read back inside its own span, where it is still in the
    /// status the submit stored, and then left alone.
    ReadItBackOnly,
    /// Left to the clock, which moves it into `RUNNABLE`, where it rests because nothing
    /// here places it on compute capacity.
    LeaveToTheClock,
    /// Terminated by a caller before anything placed it.
    Terminate,
    /// Cancelled by a caller before anything placed it.
    Cancel,
    /// Submitted waiting on a job nothing finishes, so the clock moves it into `PENDING`
    /// and it waits there.
    DependOnALiveJob,
}

/// One row of the table: what is done to the job after it is submitted, the statuses it
/// may therefore MOVE INTO (in order — the status the submit stored is in no row), and the
/// status its stream must reach when the row drives it to one.
struct Row {
    case: &'static str,
    action: Action,
    moves: &'static [&'static str],
    /// `None` for the row that drives the job nowhere: whether the clock has published its
    /// first move yet is that row's whole point, so it asserts what may appear rather than
    /// that something has.
    reaches: Option<&'static str>,
}

/// No event reports the status a submit stored, whichever status the job first moves into
/// and whoever moves it.
#[tokio::test]
async fn no_event_reports_the_status_a_submit_stored() {
    let rows = [
        Row {
            case: "read back inside its own span",
            action: Action::ReadItBackOnly,
            moves: &[state::JOB_RUNNABLE],
            reaches: None,
        },
        Row {
            case: "left to the clock",
            action: Action::LeaveToTheClock,
            moves: &[state::JOB_RUNNABLE],
            reaches: Some(state::JOB_RUNNABLE),
        },
        Row {
            case: "terminated before it was placed",
            action: Action::Terminate,
            // The caller ends the job; whether the clock had already released it into
            // RUNNABLE first changes nothing about the rule, so both moves are allowed and
            // the terminal status is what the stream must reach.
            moves: &[state::JOB_RUNNABLE, state::JOB_FAILED],
            reaches: Some(state::JOB_FAILED),
        },
        Row {
            case: "cancelled before it was placed",
            action: Action::Cancel,
            moves: &[state::JOB_RUNNABLE, state::JOB_FAILED],
            reaches: Some(state::JOB_FAILED),
        },
        Row {
            case: "waiting on a job nothing finishes",
            action: Action::DependOnALiveJob,
            moves: &[state::JOB_PENDING],
            reaches: Some(state::JOB_PENDING),
        },
    ];

    for Row {
        case,
        action,
        moves,
        reaches,
    } in rows
    {
        let batch = Batch::new();
        let environment = compute_environment(&batch, "ce-submit-events").await;
        let watched = job_queue(&batch, "jq-watched", &environment).await;
        let sibling = job_queue(&batch, "jq-sibling", &environment).await;
        let (definition, _) =
            job_definition(&batch, "jd-submit-events", container_properties()).await;
        let watched_url = given_watcher(&batch, "watched-queue-rule", &watched).await;
        let sibling_url = given_watcher(&batch, "sibling-queue-rule", &sibling).await;

        // The dependency row needs a job that is still live, so the dependent is placed in
        // PENDING and waits there: nothing here finishes a job by itself.
        let dependency = if matches!(action, Action::DependOnALiveJob) {
            let (id, _) = submit(&batch, "dependency", &watched, &definition, &[]).await;
            vec![id]
        } else {
            Vec::new()
        };
        let waits: Vec<&str> = dependency.iter().map(String::as_str).collect();
        let (job_id, _) = submit(&batch, "submitted-job", &watched, &definition, &waits).await;

        // The status the submit stored IS observable, through the reader: what follows is
        // the absence of an event for it, not the absence of the status.
        let detail = describe_job(&batch, &job_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(state::JOB_SUBMITTED),
            "case {case}: expected the submitted job to be reported {}, got {detail}",
            state::JOB_SUBMITTED
        );

        match action {
            Action::ReadItBackOnly => {}
            Action::LeaveToTheClock => {
                until_status(&batch, case, &job_id, state::JOB_RUNNABLE).await;
            }
            Action::Terminate => {
                let _ = batch
                    .ok(
                        "/v1/terminatejob",
                        json!({ "jobId": &job_id, "reason": "terminated by the test" }),
                    )
                    .await;
            }
            Action::Cancel => {
                let _ = batch
                    .ok(
                        "/v1/canceljob",
                        json!({ "jobId": &job_id, "reason": "cancelled by the test" }),
                    )
                    .await;
            }
            Action::DependOnALiveJob => {
                until_status(&batch, case, &job_id, state::JOB_PENDING).await;
            }
        }

        // What the rule's SQS target actually received, over the wire: polled until the
        // status this row drives the job to has arrived.
        let received: Vec<String> = delivered(&batch, &watched_url, &[job_id.as_str()], |seen| {
            reaches.is_none_or(|wanted| seen.iter().any(|(_, status)| status == wanted))
        })
        .await
        .into_iter()
        .map(|(_, status)| status)
        .collect();
        assert!(
            !received.iter().any(|status| status == state::JOB_SUBMITTED),
            "case {case}: no event may report the status the submit stored, got {received:?}"
        );
        assert!(
            moves_in_order(&received, moves),
            "case {case}: expected the queue to receive the job's moves {moves:?} in order, \
             got {received:?}"
        );
        if let Some(wanted) = reaches {
            assert_eq!(
                received.last().map(String::as_str),
                Some(wanted),
                "case {case}: expected the delivered stream to end at {wanted}, got {received:?}"
            );
        }

        // And the same read off the routing log, which also shows WHICH rule matched: the
        // one watching this job's queue, never the one watching the sibling queue.
        let events = published_for(&batch, &job_id);
        let logged: Vec<String> = events
            .iter()
            .map(|event| {
                event.detail["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !logged.iter().any(|status| status == state::JOB_SUBMITTED),
            "case {case}: no published event may report the status the submit stored, got \
             {logged:?}"
        );
        assert!(
            moves_in_order(&logged, moves),
            "case {case}: expected the published statuses to be the job's moves {moves:?} in \
             order, got {logged:?}"
        );
        if let Some(wanted) = reaches {
            assert_eq!(
                logged.last().map(String::as_str),
                Some(wanted),
                "case {case}: expected the published stream to end at {wanted}, got {logged:?}"
            );
        }
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
                event.matched_rules,
                vec!["watched-queue-rule".to_string()],
                "case {case}: expected only the rule watching this queue to match, got {:?}",
                event.matched_rules
            );
        }

        let withheld = drain(&batch, &sibling_url).await;
        assert!(
            withheld.is_empty(),
            "case {case}: a rule watching another queue must receive nothing, got {withheld:?}"
        );
    }
}

/// The exclusion is PER JOB: a burst of submits publishes one first transition per job
/// and no event for any of the submits.
#[tokio::test]
async fn a_burst_of_submits_publishes_only_the_first_transitions() {
    let case = "a burst of submits";
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-burst").await;
    let queue = job_queue(&batch, "jq-burst", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-burst", container_properties()).await;
    let url = given_watcher(&batch, "burst-rule", &queue).await;

    let jobs = 3usize;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..jobs {
        let (id, _) = submit(
            &batch,
            &format!("burst-job-{index}"),
            &queue,
            &definition,
            &[],
        )
        .await;
        ids.push(id);
    }
    for id in &ids {
        until_status(&batch, case, id, state::JOB_RUNNABLE).await;
    }

    let watched: Vec<&str> = ids.iter().map(String::as_str).collect();
    let received = delivered(&batch, &url, &watched, |seen| seen.len() >= jobs).await;
    assert!(
        !received
            .iter()
            .any(|(_, status)| status == state::JOB_SUBMITTED),
        "case {case}: no event may report the status a submit stored, got {received:?}"
    );
    for id in &ids {
        let per_job: Vec<String> = received
            .iter()
            .filter(|(job_id, _)| job_id == id)
            .map(|(_, status)| status.clone())
            .collect();
        assert_eq!(
            per_job,
            vec![state::JOB_RUNNABLE.to_string()],
            "case {case}: expected exactly the first transition of {id}, got {per_job:?}"
        );
    }
    assert_eq!(
        received.len(),
        jobs,
        "case {case}: expected one event per job, got {received:?}"
    );
}
