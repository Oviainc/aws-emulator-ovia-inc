// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelMessageMoveTask`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: cancelling a running message movement task stops the messages it
//! has not moved yet and leaves the ones it has already moved where they are — the stopped
//! messages are still the source queue's own (a task takes a message out of it only when
//! its schedule reaches that message), the moved ones stay in the destination queue, the
//! call answers with how many had been moved, and every later reader reports that same
//! progress under a cancelled status; a handle no recorded task answers to and a task whose
//! status is not running are refused, and a refusal moves nothing.
//!
//! The delivery table spans what the rule generalises over rather than one example: the
//! message count, the movement rate (fixed, or left to the service), both ways of
//! choosing a destination, how far into the task's schedule the cancel lands, and whether
//! the source queue took further messages while the task ran. Every expectation is
//! derived from the count the cancel call reports, so no row depends on how fast the
//! machine running it is.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    MAX_MESSAGES_PER_QUEUE, MESSAGE_MOVE_CANCELLATION_SPAN, MESSAGE_MOVE_TASK_START_DELAY,
};

use super::support::{
    attribute_count as count, call, create_ok, dead_letter_queue, emulator, error_code,
    expected_arn, message_counts, ok, receive_until, send_all,
};
use smithy_server_runtime::MultiServiceRouter;

/// The status a task reports while its cancellation is being applied, and the one it
/// reports once it has been: "Possible values are: RUNNING, COMPLETED, CANCELLING,
/// CANCELLED, and FAILED" (`ListMessageMoveTasksResultEntry$Status`).
const CANCELLING: &str = "CANCELLING";
const CANCELLED: &str = "CANCELLED";

/// The status a task reports once every message it took has been delivered.
const COMPLETED: &str = "COMPLETED";

/// What the service says when a `TaskHandle` names no message movement task.
const UNKNOWN_TASK_MESSAGE: &str = "Task does not exist.";

/// How long a loop waits for a status the clock owes before giving up. Far longer than
/// any span these rows wait on, so a slow machine cannot fail a row the emulator is still
/// working through.
const DEADLINE: Duration = Duration::from_secs(20);

/// Nanoseconds in one second, so a rate in messages per second becomes a span.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// One row of the delivery table: the case name, how many messages the dead-letter queue
/// holds, `MaxNumberOfMessagesPerSecond`, whether the request names the destination, how
/// long after the start call the cancel lands, and how many messages the dead-letter queue
/// is sent while the task runs.
type CancelRow = (&'static str, usize, Option<i64>, bool, Duration, usize);

/// How long a task moving `held` messages at `rate` takes to deliver all of them: the
/// start delay plus one slot per message after the first.
fn schedule_span(held: usize, rate: Option<i64>) -> Duration {
    MESSAGE_MOVE_TASK_START_DELAY
        + rate.map_or(Duration::ZERO, |rate| {
            Duration::from_nanos(
                u64::try_from(held.saturating_sub(1)).expect("a small count") * NANOS_PER_SECOND
                    / u64::try_from(rate).expect("a positive rate"),
            )
        })
}

/// The `ListMessageMoveTasks` entry for the most recent task under `source_arn`, which is
/// the one task each queue in this suite is started over.
///
/// The entry is located by the queue rather than by the handle its start answered with:
/// "When this field is returned in the response of the `ListMessageMoveTasks` action, it is
/// only populated for tasks that are in RUNNING status"
/// (`ListMessageMoveTasksResultEntry$TaskHandle`), and every status this suite waits for is
/// one the task has already left running for.
async fn task_entry(router: &MultiServiceRouter, source_arn: &str) -> Value {
    let reply = ok(
        router,
        "ListMessageMoveTasks",
        json!({"SourceArn": source_arn, "MaxResults": 10}),
    )
    .await;
    reply
        .get("Results")
        .and_then(Value::as_array)
        .and_then(|results| results.first().cloned())
        .unwrap_or_else(|| panic!("case the task listing: no entry under {source_arn} in {reply}"))
}

/// Wait until the most recent task of `source_arn` reports `wanted`, and answer whether it
/// did before the deadline.
async fn wait_for_status(
    router: &MultiServiceRouter,
    source_arn: &str,
    wanted: &str,
    deadline: Instant,
) -> String {
    loop {
        let entry = task_entry(router, source_arn).await;
        let status = entry["Status"].as_str().unwrap_or_default().to_string();
        if status == wanted || Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The bodies of the messages the queue at `url` hands over, in the order it does.
async fn receive_bodies(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    deadline: Instant,
) -> Vec<String> {
    receive_until(router, url, wanted, deadline)
        .await
        .into_iter()
        .map(|(_, body)| body)
        .collect()
}

/// Start a task over a fresh dead-letter queue holding `bodies`, and answer
/// `(dead-letter queue url, dead-letter queue ARN, destination url, task handle, the
/// instant the start call was made)`.
async fn start_task(
    router: &MultiServiceRouter,
    prefix: &str,
    bodies: &[String],
    rate: Option<i64>,
    name_destination: bool,
) -> (String, String, String, String, Instant) {
    let (dead_letter_url, dead_letter_arn, source_url) = dead_letter_queue(router, prefix).await;
    let (destination_url, destination_arn) = if name_destination {
        let name = format!("{prefix}-destination");
        let url = create_ok(router, &name, &json!({})).await;
        (url, Some(expected_arn(&name)))
    } else {
        (source_url, None)
    };
    send_all(router, &dead_letter_url, bodies).await;

    let mut request = json!({"SourceArn": dead_letter_arn});
    if let Some(arn) = &destination_arn {
        request["DestinationArn"] = json!(arn);
    }
    if let Some(rate) = rate {
        request["MaxNumberOfMessagesPerSecond"] = json!(rate);
    }
    let started = Instant::now();
    let reply = ok(router, "StartMessageMoveTask", request).await;
    let handle = reply["TaskHandle"].as_str().unwrap_or_default().to_string();
    assert!(
        !handle.is_empty(),
        "case the start of {prefix}: expected a TaskHandle, got {reply}"
    );
    (
        dead_letter_url,
        dead_letter_arn,
        destination_url,
        handle,
        started,
    )
}

/// A cancelled task leaves the messages it had already moved in the destination queue and
/// puts the ones it had not back in the source queue, so the pair of queues holds every
/// message it held before — and the count the call answers with is the progress every
/// later reader reports for the task.
#[tokio::test]
async fn cancelling_a_running_task_stops_what_it_had_not_moved_and_keeps_what_it_had() {
    // (case, how many messages the dead-letter queue holds, MaxNumberOfMessagesPerSecond,
    // whether the request names the destination, how long after the start call the cancel
    // lands, how many messages the dead-letter queue is sent while the task runs). The
    // rate column holds both forms the member documents: a fixed rate, and left blank so
    // "the system will optimize the rate".
    let rows: &[CancelRow] = &[
        (
            "six messages at one per second, cancelled at once",
            6,
            Some(1),
            false,
            Duration::ZERO,
            0,
        ),
        (
            "four messages at one per second, cancelled a second in, destination named",
            4,
            Some(1),
            true,
            Duration::from_secs(1),
            0,
        ),
        (
            "five messages at one per second, with one sent while the task ran",
            5,
            Some(1),
            false,
            Duration::from_secs(1),
            1,
        ),
        (
            "three messages with the rate left to the service, cancelled at once",
            3,
            None,
            true,
            Duration::ZERO,
            0,
        ),
    ];

    for (index, (case, held, rate, name_destination, wait, sent_meanwhile)) in
        rows.iter().enumerate()
    {
        let router = emulator();
        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        let (dead_letter_url, dead_letter_arn, destination_url, handle, started) = start_task(
            &router,
            &format!("cancel-{index}"),
            &bodies,
            *rate,
            *name_destination,
        )
        .await;

        // Messages the source queue takes while the task runs queue up behind the ones it
        // was sized over, so a cancel this early never reaches them.
        let meanwhile: Vec<String> = (0..*sent_meanwhile)
            .map(|number| format!("{case} sent meanwhile #{number}"))
            .collect();
        send_all(&router, &dead_letter_url, &meanwhile).await;

        tokio::time::sleep(*wait).await;
        let (status, parsed) = call(
            &router,
            "CancelMessageMoveTask",
            &json!({"TaskHandle": handle}),
        )
        .await;
        let cancelled_at = Instant::now();
        if status != 200 {
            // "A message movement can only be cancelled when the current status is
            // RUNNING": on a machine slow enough that the task's whole schedule elapsed
            // before this call, the refusal IS the right answer and the row is spent.
            assert!(
                started.elapsed() >= schedule_span(*held, *rate),
                "case {case}: expected HTTP 200 while the task was still running, got {status} \
                 with {parsed}"
            );
            continue;
        }
        let moved = parsed["ApproximateNumberOfMessagesMoved"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: no ApproximateNumberOfMessagesMoved in {parsed}"));
        let held_count = i64::try_from(*held).expect("a small count");
        assert!(
            (0..=held_count).contains(&moved),
            "case {case}: expected between 0 and {held_count} messages moved, got {moved}"
        );
        if started.elapsed() < MESSAGE_MOVE_TASK_START_DELAY {
            assert_eq!(
                moved, 0,
                "case {case}: a task cancelled before it could deliver anything has moved \
                 nothing, got {moved}"
            );
        }
        let stopped = held_count - moved;

        // (case, queue, attribute, what it must report). The stopped messages are still the
        // source queue's, the moved ones are the destination queue's, and the destination
        // owes no further delivery: the pair holds every message it held.
        let meanwhile_count = i64::try_from(*sent_meanwhile).expect("a small count");
        for (which, url, attribute, expected) in [
            (
                "the source queue holds the stopped messages",
                &dead_letter_url,
                "ApproximateNumberOfMessages",
                stopped + meanwhile_count,
            ),
            (
                "the destination queue keeps the moved messages",
                &destination_url,
                "ApproximateNumberOfMessages",
                moved,
            ),
            (
                "the destination queue is owed no further delivery",
                &destination_url,
                "ApproximateNumberOfMessagesDelayed",
                0,
            ),
            (
                "the source queue has nothing in flight",
                &dead_letter_url,
                "ApproximateNumberOfMessagesNotVisible",
                0,
            ),
        ] {
            assert_eq!(
                count(&router, url, attribute).await,
                expected,
                "case {case}: expected {attribute} of {url} to be {expected} with {moved} of \
                 {held_count} messages moved ({which})"
            );
        }

        // The task reports the same progress the cancel call answered with, under a
        // cancelling status that settles into a cancelled one.
        let before = Instant::now();
        let entry = task_entry(&router, &dead_letter_arn).await;
        let after = Instant::now();
        // A task that has left RUNNING reports no handle for a caller to cancel again.
        assert!(
            entry.get("TaskHandle").is_none(),
            "case {case}: expected a cancelled task to report no handle, got {entry}"
        );
        let reported = entry["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            [CANCELLING, CANCELLED].contains(&reported.as_str()),
            "case {case}: expected a cancelled task to report {CANCELLING} or {CANCELLED}, got \
             {entry}"
        );
        if after.duration_since(cancelled_at) < MESSAGE_MOVE_CANCELLATION_SPAN {
            assert_eq!(
                reported, CANCELLING,
                "case {case}: expected {CANCELLING} while the cancellation was being applied, \
                 got {entry}"
            );
        }
        if before.duration_since(cancelled_at) >= MESSAGE_MOVE_CANCELLATION_SPAN {
            assert_eq!(
                reported, CANCELLED,
                "case {case}: expected {CANCELLED} once the cancellation had been applied, got \
                 {entry}"
            );
        }

        // Past the whole schedule the cancelled task would have had, its progress is still
        // what it was when it was cancelled: it delivered nothing more.
        tokio::time::sleep(schedule_span(*held, *rate)).await;
        let settled = wait_for_status(
            &router,
            &dead_letter_arn,
            CANCELLED,
            Instant::now() + DEADLINE,
        )
        .await;
        assert_eq!(
            settled, CANCELLED,
            "case {case}: expected the task to have settled at {CANCELLED}"
        );
        let entry = task_entry(&router, &dead_letter_arn).await;
        assert_eq!(
            (
                entry["ApproximateNumberOfMessagesMoved"].as_i64(),
                entry["ApproximateNumberOfMessagesToMove"].as_i64()
            ),
            (Some(moved), Some(held_count)),
            "case {case}: expected the cancelled task's progress to stay at {moved} of the \
             {held_count} messages it was started over, got {entry}"
        );
        assert_eq!(
            count(&router, &destination_url, "ApproximateNumberOfMessages").await,
            moved,
            "case {case}: expected the destination queue to take nothing more after the \
             cancellation"
        );

        // The messages themselves: the destination kept the leading ones the task had
        // already delivered, and the source has the rest back, ahead of whatever it took
        // while the task ran.
        let deadline = Instant::now() + DEADLINE;
        let in_destination = receive_bodies(
            &router,
            &destination_url,
            usize::try_from(moved).expect("a small count"),
            deadline,
        )
        .await;
        assert_eq!(
            in_destination,
            bodies[..usize::try_from(moved).expect("a small count")].to_vec(),
            "case {case}: expected the destination queue to hold the messages the task had \
             moved, got {in_destination:?}"
        );
        let expected_source: Vec<String> = bodies
            [usize::try_from(moved).expect("a small count")..]
            .iter()
            .chain(meanwhile.iter())
            .cloned()
            .collect();
        let in_source = receive_bodies(
            &router,
            &dead_letter_url,
            expected_source.len(),
            deadline,
        )
        .await;
        assert_eq!(
            in_source, expected_source,
            "case {case}: expected the source queue to hold the messages the task had not \
             moved, got {in_source:?}"
        );
    }
}

/// Every way a cancel can name no task this store can stop is refused, and refusing moves
/// nothing.
#[tokio::test]
async fn a_cancel_naming_no_running_task_is_refused() {
    let router = emulator();

    // A task that is still running, so the handle of a task an earlier call cancelled is
    // a real handle rather than an invented one.
    let bodies: Vec<String> = (0..4).map(|number| format!("cancelled #{number}")).collect();
    let (cancelled_url, cancelled_arn, _, cancelled_handle, _) =
        start_task(&router, "refuse-cancelled", &bodies, Some(1), false).await;
    let first = ok(
        &router,
        "CancelMessageMoveTask",
        json!({"TaskHandle": cancelled_handle}),
    )
    .await;
    assert!(
        first.get("ApproximateNumberOfMessagesMoved").is_some(),
        "case the first cancel: expected a message count, got {first}"
    );

    // A task that has delivered everything it took, which is the other status a cancel
    // cannot act on.
    let (done_url, done_arn, _, done_handle, _) = start_task(
        &router,
        "refuse-done",
        &["completed".to_string()],
        None,
        false,
    )
    .await;
    let done_status = wait_for_status(&router, &done_arn, COMPLETED, Instant::now() + DEADLINE).await;
    assert_eq!(
        done_status, COMPLETED,
        "case the completed task: expected it to finish its one message"
    );

    // (case, TaskHandle, expected status, expected error code, the message it must carry
    // when the service words one for the refusal). A handle no record answers to names no
    // task, whatever shape it has; a task that is not running is a task a cancellation
    // cannot act on.
    let rows: &[(&str, String, u16, &str, Option<&str>)] = &[
        (
            "a handle no task answers to",
            "6f57b2cc-0000-4000-8000-000000000000".to_string(),
            404,
            "ResourceNotFoundException",
            Some(UNKNOWN_TASK_MESSAGE),
        ),
        (
            "a handle that is not of the shape this service mints",
            "not-a-task-handle".to_string(),
            404,
            "ResourceNotFoundException",
            Some(UNKNOWN_TASK_MESSAGE),
        ),
        (
            "an empty handle",
            String::new(),
            404,
            "ResourceNotFoundException",
            Some(UNKNOWN_TASK_MESSAGE),
        ),
        (
            "the handle of a task an earlier call cancelled",
            cancelled_handle.clone(),
            400,
            "UnsupportedOperation",
            None,
        ),
        (
            "the handle of a task that has delivered everything it took",
            done_handle.clone(),
            400,
            "UnsupportedOperation",
            None,
        ),
    ];

    let before: Vec<i64> = vec![
        count(&router, &cancelled_url, "ApproximateNumberOfMessages").await,
        count(&router, &done_url, "ApproximateNumberOfMessages").await,
    ];
    for (case, handle, expected_status, expected_code, expected_message) in rows {
        let (status, parsed) = call(
            &router,
            "CancelMessageMoveTask",
            &json!({"TaskHandle": handle}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (*expected_status, *expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} with \
             {parsed}"
        );
        if let Some(expected_message) = expected_message {
            // awsJson1_0 carries the modelled `message` member as the body's `message`.
            assert_eq!(
                parsed["message"].as_str(),
                Some(*expected_message),
                "case {case}: expected the message {expected_message:?}, got {parsed}"
            );
        }
        assert!(
            parsed.get("ApproximateNumberOfMessagesMoved").is_none(),
            "case {case}: a refusal cancels nothing, got {parsed}"
        );
    }

    let after: Vec<i64> = vec![
        count(&router, &cancelled_url, "ApproximateNumberOfMessages").await,
        count(&router, &done_url, "ApproximateNumberOfMessages").await,
    ];
    assert_eq!(
        after, before,
        "case the refused cancels: expected them to move nothing, with the cancelled task's \
         queue at {before:?}"
    );
    let still_cancelled = task_entry(&router, &cancelled_arn).await;
    assert!(
        [CANCELLING, CANCELLED]
            .contains(&still_cancelled["Status"].as_str().unwrap_or_default()),
        "case a refused second cancel: expected the task to stay cancelled, got \
         {still_cancelled}"
    );
}

/// "Only one active message movement task is supported per queue at any given time":
/// cancelling a task ends it, so the source queue can take another one straight away.
#[tokio::test]
async fn a_cancelled_task_frees_its_source_queue_for_another_task() {
    let router = emulator();
    let bodies: Vec<String> = (0..4).map(|number| format!("freed #{number}")).collect();
    let (_, dead_letter_arn, _, handle, _) =
        start_task(&router, "cancel-frees", &bodies, Some(1), false).await;

    let (status, parsed) = call(
        &router,
        "StartMessageMoveTask",
        &json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 1}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case a second start while the first task runs: expected HTTP 400 \
         InvalidParameterValue, got {status} with {parsed}"
    );

    ok(
        &router,
        "CancelMessageMoveTask",
        json!({"TaskHandle": handle}),
    )
    .await;
    let again = ok(
        &router,
        "StartMessageMoveTask",
        json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 1}),
    )
    .await;
    let second = again["TaskHandle"].as_str().unwrap_or_default().to_string();
    assert!(
        !second.is_empty() && second != handle,
        "case a start after the task was cancelled: expected a new TaskHandle, got {again}"
    );
}

/// A move puts messages INTO the destination queue, so it is bounded by what that queue
/// may hold like every other path that adds messages to one: a destination filled to its
/// allowance while the task runs takes no more of them, the messages it cannot take stay in
/// the source queue rather than being dropped, and cancelling the task leaves every message
/// in one of the two queues.
#[tokio::test]
async fn a_move_cannot_push_the_destination_queue_past_what_it_may_hold() {
    let router = emulator();
    // Enough messages, at the slowest rate the member admits (one per second), that the
    // task still owes several of them however long the refill below takes: a row where the
    // whole schedule had elapsed would assert the same thing without exercising the bound
    // at all.
    let moving: Vec<String> = (0..10).map(|number| format!("moving #{number}")).collect();
    let (dead_letter_url, dead_letter_arn, destination_url, handle, _) =
        start_task(&router, "move-bounded", &moving, Some(1), true).await;

    // Fill the destination queue to the whole allowance this store holds per queue while
    // the task runs, in batches of the ten entries a batch send accepts. What the task has
    // already moved into it counts toward that allowance, so the room left is re-read
    // before every send rather than assumed.
    let allowance = i64::try_from(MAX_MESSAGES_PER_QUEUE).expect("a small count");
    let mut filled = 0i64;
    let fill_deadline = Instant::now() + DEADLINE;
    loop {
        let (visible, delayed, in_flight) = message_counts(&router, &destination_url).await;
        let room = allowance - (visible + delayed + in_flight);
        if room <= 0 {
            break;
        }
        assert!(
            Instant::now() < fill_deadline,
            "case the refill: expected the destination queue filled to its allowance of \
             {allowance} within {DEADLINE:?}, still {room} short"
        );
        let batch = room.min(10);
        let entries: Vec<Value> = (0..batch)
            .map(|number| {
                json!({
                    "Id": format!("e{filled}x{number}"),
                    "MessageBody": format!("fill #{filled}x{number}"),
                })
            })
            .collect();
        let (status, _) = call(
            &router,
            "SendMessageBatch",
            &json!({"QueueUrl": destination_url, "Entries": entries}),
        )
        .await;
        if status == 200 {
            filled += batch;
        }
    }

    // Past the whole schedule the task was started over, so every slot it had has come
    // round: what it has not moved by now is what the destination queue would not take.
    tokio::time::sleep(schedule_span(moving.len(), Some(1))).await;
    let parsed = ok(
        &router,
        "CancelMessageMoveTask",
        json!({"TaskHandle": handle}),
    )
    .await;
    let moved = parsed["ApproximateNumberOfMessagesMoved"]
        .as_i64()
        .unwrap_or_else(|| panic!("case the cancel: no ApproximateNumberOfMessagesMoved in {parsed}"));
    let held = i64::try_from(moving.len()).expect("a small count");

    // Every message either queue holds is counted — visible, in flight or delayed — so
    // none can hide from the allowance or from the total.
    let mut totals = Vec::new();
    for (which, url) in [
        ("the source queue", &dead_letter_url),
        ("the destination queue", &destination_url),
    ] {
        let (visible, delayed, in_flight) = message_counts(&router, url).await;
        let total = visible + in_flight + delayed;
        assert!(
            total <= allowance,
            "case {which}: expected at most the allowance of {allowance} messages, got {total} \
             ({visible} available, {delayed} delayed, {in_flight} in flight)"
        );
        totals.push(total);
    }
    let pair: i64 = totals.iter().sum();
    assert_eq!(
        pair,
        filled + held,
        "case the pair of queues: expected every one of the {} messages still held, got \
         {totals:?} with {moved} of the {held} moved",
        filled + held
    );

    // The messages the destination queue could not take are the source queue's own, in the
    // order it held them: a move the bound withheld left them where they were.
    if filled + held > allowance {
        let owed = held - moved;
        assert!(
            owed > 0,
            "case the withheld move: expected the destination queue to have refused some of \
             the {held} messages, got {moved} moved"
        );
        let in_source = receive_bodies(
            &router,
            &dead_letter_url,
            usize::try_from(owed).expect("a small count"),
            Instant::now() + DEADLINE,
        )
        .await;
        assert_eq!(
            in_source,
            moving[usize::try_from(moved).expect("a small count")..].to_vec(),
            "case the source queue: expected it to still hold the messages the destination \
             could not take, got {in_source:?}"
        );
    }

    // A cancelled task is past RUNNING, whatever it still had to move.
    let settled = wait_for_status(
        &router,
        &dead_letter_arn,
        CANCELLED,
        Instant::now() + DEADLINE,
    )
    .await;
    assert_eq!(
        settled, CANCELLED,
        "case the cancelled task: expected it to settle at {CANCELLED}"
    );
}
