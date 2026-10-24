// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMessageMoveTasks`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: `ListMessageMoveTasks` reports the message movement tasks
//! recorded under the source queue the request names and no other queue's, most recent
//! first, at most `MaxResults` of them (default 1, upper limit 10), each entry's handle,
//! queues, status and `ApproximateNumberOfMessages*` counts read off the task record and
//! the clock — and reporting them changes nothing a later caller sees. A `SourceArn` that
//! names no queue of this account, and a `MaxResults` outside the documented range, are
//! refused.
//!
//! Each table spans the class the rule generalises over rather than one example: the
//! listing rows walk the recorded task count (none, one, several) crossed with the
//! `MaxResults` forms (absent, below the recorded count, at the published upper limit),
//! and the refusal rows walk each published bound of `MaxResults` and one value past it
//! plus every way a `SourceArn` can fail to name a queue of this account.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    DEFAULT_MESSAGE_MOVE_TASK_RESULTS, MAX_MESSAGE_MOVE_TASKS_PER_QUEUE,
};

use super::support::{
    account, call, create_ok, dead_letter_queue, emulator, error_code, expected_arn, ok, send_all,
    OTHER_ACCOUNT,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the state the clock owes — the running task to finish, or
/// the next start to be accepted — before giving up. Far longer than any row's own
/// schedule, so a slow machine cannot fail a row the emulator is still working through.
const DEADLINE: Duration = Duration::from_secs(20);

/// How long a polling loop pauses between two attempts.
const PAUSE: Duration = Duration::from_millis(50);

/// The statuses a task this store records can be in: "Possible values are: RUNNING,
/// COMPLETED, CANCELLING, CANCELLED, and FAILED"
/// (`ListMessageMoveTasksResultEntry$Status`) — of those, a task that takes its messages
/// when it starts and is never cancelled reaches only these two.
const REACHABLE_STATUSES: &[&str] = &["RUNNING", "COMPLETED"];

/// The `Results` of a listing reply, as a list — empty when the member is absent, which
/// is how a reply that reports no task answers.
fn results(reply: &Value) -> Vec<Value> {
    reply
        .get("Results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// List the tasks of the queue `source_arn`, with `MaxResults` when one is given.
async fn list(
    router: &MultiServiceRouter,
    source_arn: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut request = json!({"SourceArn": source_arn});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    results(&ok(router, "ListMessageMoveTasks", request).await)
}

/// One entry's string member, or `"<absent>"` so a failing assertion names what it saw.
fn text(entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// One entry's `long` member. `ApproximateNumberOfMessagesMoved` and `StartedTimestamp`
/// both carry a modelled default of 0, which the wire may leave out, so an absent member
/// reads as that default.
fn number(entry: &Value, member: &str) -> i64 {
    entry.get(member).and_then(Value::as_i64).unwrap_or(0)
}

/// Start a message movement task from `source_arn`, waiting out the task already running
/// for that queue ("only one active message movement task is supported per queue at any
/// given time"), and return its handle.
async fn start_task(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    rate: Option<i64>,
) -> String {
    let mut request = json!({"SourceArn": source_arn});
    if let Some(rate) = rate {
        request["MaxNumberOfMessagesPerSecond"] = json!(rate);
    }
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, parsed) = call(router, "StartMessageMoveTask", &request).await;
        if status == 200 {
            let handle = parsed["TaskHandle"].as_str().unwrap_or_default().to_string();
            assert!(
                !handle.is_empty(),
                "case {case}: expected a TaskHandle, got {parsed}"
            );
            return handle;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected a start to be accepted within {DEADLINE:?}, got {status} \
             with {parsed}"
        );
        tokio::time::sleep(PAUSE).await;
    }
}

/// Now, as the epoch milliseconds a `StartedTimestamp` is reported in.
fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the Unix epoch")
            .as_millis(),
    )
    .expect("an instant that counts in an i64")
}

/// A listing reports the tasks recorded under the named source queue, most recent first,
/// bounded by `MaxResults` (default 1, upper limit 10), with each entry's queues and
/// handle read off the task record.
#[tokio::test]
async fn a_listing_reports_the_most_recent_tasks_of_its_source_queue_newest_first() {
    // (case, how many tasks are started, the MaxResults the listing asks for, how many
    // entries it must report). The rows walk the recorded task count against each form
    // `MaxResults` takes: absent ("the default is 1, which provides the most recent
    // message movement task"), below the recorded count, and at the published upper limit
    // of 10.
    let rows: &[(&str, usize, Option<i64>, usize)] = &[
        ("no task started, MaxResults absent", 0, None, 0),
        ("no task started, MaxResults at the upper limit", 0, Some(10), 0),
        ("one task, MaxResults absent", 1, None, 1),
        ("three tasks, MaxResults absent", 3, None, 1),
        ("three tasks, MaxResults 2", 3, Some(2), 2),
        ("three tasks, MaxResults at the upper limit", 3, Some(10), 3),
    ];
    assert_eq!(
        (DEFAULT_MESSAGE_MOVE_TASK_RESULTS, MAX_MESSAGE_MOVE_TASKS_PER_QUEUE),
        (1, 10),
        "case the documented bounds: the default is 1 and the upper limit is 10"
    );

    for (index, (case, started, max_results, expected)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("listing-{index}");
        let (_, dead_letter_arn, _) = dead_letter_queue(&router, &prefix).await;
        // The one queue that drives its messages to the dead-letter queue is where a task
        // that names no destination sends them back to.
        let destination_arn = expected_arn(&format!("{prefix}-source"));

        let before = now_millis();
        let mut handles: Vec<String> = Vec::with_capacity(*started);
        for task in 0..*started {
            handles.push(
                start_task(
                    &router,
                    &format!("{case}: task {task}"),
                    &dead_letter_arn,
                    None,
                )
                .await,
            );
        }
        let after = now_millis();

        let listed = list(&router, &dead_letter_arn, *max_results).await;
        assert_eq!(
            listed.len(),
            *expected,
            "case {case}: expected {expected} entries, got {listed:?}"
        );

        // "Gets the MOST RECENT message movement tasks (up to 10)": the newest task is the
        // first entry, so the reported handles are the last `expected` started, reversed.
        let expected_handles: Vec<String> =
            handles.iter().rev().take(*expected).cloned().collect();
        assert_eq!(
            listed
                .iter()
                .map(|entry| text(entry, "TaskHandle"))
                .collect::<Vec<String>>(),
            expected_handles,
            "case {case}: expected the most recent handles newest first, got {listed:?}"
        );

        let mut previous_timestamp = i64::MAX;
        for entry in &listed {
            assert_eq!(
                (text(entry, "SourceArn"), text(entry, "DestinationArn")),
                (dead_letter_arn.clone(), destination_arn.clone()),
                "case {case}: expected the queues the task resolved, got {entry}"
            );
            assert!(
                REACHABLE_STATUSES.contains(&text(entry, "Status").as_str()),
                "case {case}: expected one of {REACHABLE_STATUSES:?}, got {entry}"
            );
            // A task that took no message has nothing left to move and has moved nothing,
            // whatever the clock has done since.
            assert_eq!(
                (
                    number(entry, "ApproximateNumberOfMessagesMoved"),
                    number(entry, "ApproximateNumberOfMessagesToMove")
                ),
                (0, 0),
                "case {case}: a task over an empty queue moves nothing, got {entry}"
            );
            assert!(
                entry.get("MaxNumberOfMessagesPerSecond").is_none(),
                "case {case}: a start that fixed no rate reports none, got {entry}"
            );
            assert!(
                entry.get("FailureReason").is_none(),
                "case {case}: a task that has not failed reports no reason, got {entry}"
            );
            let started_timestamp = number(entry, "StartedTimestamp");
            assert!(
                (before..=after).contains(&started_timestamp),
                "case {case}: expected a StartedTimestamp in {before}..={after}, got {entry}"
            );
            assert!(
                started_timestamp <= previous_timestamp,
                "case {case}: expected the entries newest first, got {listed:?}"
            );
            previous_timestamp = started_timestamp;
        }
    }
}

/// Every way a listing can name no queue of this account, or ask for a number of results
/// outside the documented range, is refused — and each published bound of that range is
/// accepted.
#[tokio::test]
async fn a_listing_naming_no_queue_or_asking_outside_the_result_range_is_refused() {
    let router = emulator();
    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other-account row: its account id must not be this emulator's"
    );
    let (_, dead_letter_arn, _) = dead_letter_queue(&router, "refused-listing").await;

    // (case, request, expected status, expected error code — empty for the rows that must
    // be accepted). "The default is 1 ... The upper limit is 10"
    // (ListMessageMoveTasksRequest$MaxResults), so 1 and 10 are the published bounds and
    // 0, -1 and 11 are outside them. A value that names no queue in any account is not an
    // address this service can resolve, which is what `InvalidAddress` refuses; a
    // well-formed ARN of another account or service names a resource that does not exist
    // here.
    let rows: &[(&str, Value, u16, &str)] = &[
        (
            "MaxResults at the published lower bound of 1",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 1}),
            200,
            "",
        ),
        (
            "MaxResults at the published upper limit of 10",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 10}),
            200,
            "",
        ),
        (
            "MaxResults of zero, one below the lower bound",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 0}),
            400,
            "ValidationException",
        ),
        (
            "a negative MaxResults",
            json!({"SourceArn": dead_letter_arn, "MaxResults": -1}),
            400,
            "ValidationException",
        ),
        (
            "MaxResults of 11, one past the upper limit",
            json!({"SourceArn": dead_letter_arn, "MaxResults": 11}),
            400,
            "ValidationException",
        ),
        (
            "a SourceArn no queue of this account answers to",
            json!({"SourceArn": expected_arn("refused-listing-absent")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another account",
            json!({"SourceArn": format!("arn:aws:sqs:us-east-1:{OTHER_ACCOUNT}:refused-listing")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another service",
            json!({"SourceArn": format!("arn:aws:sns:us-east-1:{}:refused-listing", account())}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn that is not an ARN at all",
            json!({"SourceArn": "refused-listing"}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with a segment too few",
            json!({"SourceArn": format!("arn:aws:sqs:{}:refused-listing", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with an empty region segment",
            json!({"SourceArn": format!("arn:aws:sqs::{}:refused-listing", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn whose queue name is longer than a queue name may be",
            json!({
                "SourceArn": format!("arn:aws:sqs:us-east-1:{}:{}", account(), "q".repeat(81)),
            }),
            404,
            "InvalidAddress",
        ),
    ];

    // One task is recorded, so an accepted row has something to report and a refused one
    // cannot be mistaken for an empty store.
    let handle = start_task(&router, "the recorded task", &dead_letter_arn, None).await;

    for (case, request, expected_status, expected_code) in rows {
        let (status, parsed) = call(&router, "ListMessageMoveTasks", request).await;
        if *expected_status == 200 {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            assert_eq!(
                results(&parsed)
                    .iter()
                    .map(|entry| text(entry, "TaskHandle"))
                    .collect::<Vec<String>>(),
                vec![handle.clone()],
                "case {case}: expected the recorded task, got {parsed}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (*expected_status, *expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} with \
             {parsed}"
        );
        assert!(
            parsed.get("Results").is_none(),
            "case {case}: a refusal reports no task, got {parsed}"
        );
    }
}

/// A running task's rate and progress are reported from its record and the clock: the
/// counts always add up to the messages it took, never go backwards, and reach "every
/// message moved, none left" exactly when the task is `COMPLETED` — and reporting them
/// neither delivers a message nor frees the source queue for a second task.
#[tokio::test]
async fn a_tasks_reported_progress_follows_the_record_and_the_clock() {
    let router = emulator();
    let (dead_letter_url, dead_letter_arn, _) = dead_letter_queue(&router, "progress").await;
    // Three messages at one message per second, so the task is still running — with
    // messages left to move — for a whole two seconds after the start call.
    let bodies: Vec<String> = (0..3).map(|number| format!("message #{number}")).collect();
    send_all(&router, &dead_letter_url, &bodies).await;
    let taken = i64::try_from(bodies.len()).expect("a small count");
    let rate = 1;
    let handle = start_task(&router, "the running task", &dead_letter_arn, Some(rate)).await;

    let deadline = Instant::now() + DEADLINE;
    let mut moved_before = 0;
    let mut saw_running = false;
    loop {
        let listed = list(&router, &dead_letter_arn, None).await;
        let entry = listed
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case the running task: expected one entry, got {listed:?}"));
        assert_eq!(
            text(&entry, "TaskHandle"),
            handle,
            "case the running task: expected the handle the start returned, got {entry}"
        );
        assert_eq!(
            entry.get("MaxNumberOfMessagesPerSecond").and_then(Value::as_i64),
            Some(rate),
            "case the running task: expected the rate the start fixed, got {entry}"
        );
        let moved = number(&entry, "ApproximateNumberOfMessagesMoved");
        let to_move = number(&entry, "ApproximateNumberOfMessagesToMove");
        assert_eq!(
            moved + to_move,
            taken,
            "case the running task: expected the counts to add up to the {taken} messages \
             taken, got {entry}"
        );
        assert!(
            moved >= moved_before,
            "case the running task: expected the moved count never to go backwards, got \
             {moved} after {moved_before}"
        );
        moved_before = moved;

        let status = text(&entry, "Status");
        if status == "RUNNING" {
            saw_running = true;
            assert!(
                to_move > 0,
                "case a RUNNING task: expected messages still to move, got {entry}"
            );
            // A listing is a report, not a turn of the crank: while the task runs, the
            // source queue is still busy, whoever has read it.
            let (refused, parsed) = call(
                &router,
                "StartMessageMoveTask",
                &json!({"SourceArn": dead_letter_arn}),
            )
            .await;
            assert_eq!(
                (refused, error_code(&parsed).as_str()),
                (400, "UnsupportedOperation"),
                "case a start while the listed task runs: expected HTTP 400 \
                 UnsupportedOperation, got {refused} with {parsed}"
            );
        } else {
            assert_eq!(
                (status.as_str(), moved, to_move),
                ("COMPLETED", taken, 0),
                "case a finished task: expected every message moved and none left, got {entry}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case the running task: expected it to complete within {DEADLINE:?}"
        );
        tokio::time::sleep(PAUSE).await;
    }
    assert!(
        saw_running,
        "case the running task: expected to observe it RUNNING at a rate of {rate} message \
         per second"
    );
}

/// "Under a SPECIFIC source queue": a task is listed under the queue it moves messages
/// from and under no other, and reading one queue's tasks does not change what the next
/// read of either queue sees.
#[tokio::test]
async fn a_task_is_listed_only_under_the_source_queue_it_moves_from() {
    let router = emulator();
    let (_, first_arn, _) = dead_letter_queue(&router, "own-tasks-first").await;
    let (_, second_arn, _) = dead_letter_queue(&router, "own-tasks-second").await;
    // A queue that is nobody's dead-letter target can have no task started from it, so it
    // has none to report either.
    let plain_arn = expected_arn("own-tasks-plain");
    create_ok(&router, "own-tasks-plain", &json!({})).await;

    let first = start_task(&router, "the first queue's task", &first_arn, None).await;
    let second = start_task(&router, "the second queue's task", &second_arn, None).await;

    // (case, the queue listed, the handles it must report)
    let rows: &[(&str, &String, Vec<String>)] = &[
        ("the first dead-letter queue", &first_arn, vec![first.clone()]),
        ("the second dead-letter queue", &second_arn, vec![second.clone()]),
        ("a queue no task was started from", &plain_arn, Vec::new()),
    ];

    // Listed twice: a report is idempotent, so the second pass must say exactly what the
    // first did.
    for pass in 0..2 {
        for (case, source_arn, expected) in rows {
            let listed = list(&router, source_arn, Some(10)).await;
            assert_eq!(
                listed
                    .iter()
                    .map(|entry| text(entry, "TaskHandle"))
                    .collect::<Vec<String>>(),
                *expected,
                "case {case} on pass {pass}: expected {expected:?}, got {listed:?}"
            );
            for entry in &listed {
                assert_eq!(
                    text(entry, "SourceArn"),
                    **source_arn,
                    "case {case} on pass {pass}: expected the queue listed, got {entry}"
                );
            }
        }
    }
}
