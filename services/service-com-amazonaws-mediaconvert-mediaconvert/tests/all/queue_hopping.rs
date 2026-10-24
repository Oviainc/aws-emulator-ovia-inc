// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what a job's `HopDestinations` DO.
//!
//! The rule every case below is one row of: once a job has waited a hop destination's
//! `WaitMinutes` in the queue it is in, that hop destination MOVES it — to the queue the
//! hop names, or to the account's default queue when it names none — the job takes the
//! hop's `Priority` if it states one, and the move is recorded as a `QueueTransition`
//! (source queue, destination queue, the instant of the move) on the job. Hop destinations
//! are used in request order, at most once each, and a job that is no longer waiting in a
//! queue hops nowhere.
//!
//! From the model
//! (`smithy_query(query="shape",shape="HopDestination",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! `HopDestination` is "Configuration for a destination queue to which the job can hop once
//! a customer-defined minimum wait time has passed"; `Queue` is "Optional unless the job is
//! submitted on the default queue … This queue cannot be the original queue to which the
//! job is submitted. If the original queue isn't the default queue and you don't specify
//! the destination queue, the job will move to the default queue"; `Priority` is "you can
//! specify a different relative priority for the job in the destination queue. If you don't
//! specify, the relative priority will remain the same as in the previous queue";
//! `WaitMinutes` is "Required for setting up a job to use queue hopping. Minimum wait time
//! in minutes until the job can hop to the destination queue. Valid range is 1 to 4320
//! minutes, inclusive." `QueueTransition` is "Description of the source and destination
//! queues between which the job has moved, along with the timestamp of the move".
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see. The one thing reached past HTTP is the
//! CLOCK: a hop's wait is the one the caller asked for, in minutes, so a suite cannot sit
//! one out — see [`advance`].

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    MAX_HOP_WAIT_MINUTES, MIN_HOP_WAIT_MINUTES,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{arn, settings as job_settings, Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// Longer than any row advances the clock by, so no row's job leaves `SUBMITTED` while its
/// hops are being materialised: the rows are about hopping, not about the settle span.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(100 * 24 * 60 * 60);

/// One minute, the unit `WaitMinutes` is stated in.
const MINUTE: Duration = Duration::from_secs(60);

/// Materialise every change the clock has made due by `ahead` from now.
///
/// A hop's wait is a duration the CALLER asked for, in minutes, so this suite cannot sit
/// one out in real time. No handler ever sleeps for a due instant — it is stored, and
/// `settle` applies exactly the changes owed at the instant it is handed — so giving it an
/// instant `ahead` from now materialises precisely what a reader arriving then would see.
fn advance(emulator: &Emulator, ahead: Duration) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .settle(SystemTime::now() + ahead);
}

/// `(sourceQueue, destinationQueue, timestamp)` of every entry of a job's
/// `queueTransitions`, in the order reported.
fn transitions(reply: &Reply) -> Vec<(String, String, i64)> {
    reply
        .at("/job/queueTransitions")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.pointer("/sourceQueue")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.pointer("/destinationQueue")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.pointer("/timestamp").and_then(Value::as_i64).unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The queue ARN of a queue name — the form `Job$Queue` and both `QueueTransition` members
/// are projected in.
fn queue_arn(name: &str) -> String {
    arn(&format!("queues/{name}"))
}

/// Create the queues a case needs, each `(name, paused)`.
async fn seed_queues(emulator: &Emulator, queues: &[(&str, bool)]) {
    for (name, paused) in queues {
        let mut request = json!({ "name": name });
        if *paused {
            request["status"] = json!("PAUSED");
        }
        let created = emulator.create_queue(request).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created, got {} with body {}",
            created.status, created.body
        );
    }
}

/// One row of the hopping table.
struct HopCase {
    case: &'static str,
    /// The queue the job is submitted to.
    source: &'static str,
    /// The `Priority` the request states, if any.
    priority: Option<i64>,
    /// The `hopDestinations` list the request carries.
    hops: Value,
    /// How far the clock is advanced before the job is read back.
    advance_by: Duration,
    /// The queue the job must be on afterwards.
    expected_queue: &'static str,
    /// The `Priority` the job must report afterwards.
    expected_priority: i64,
    /// `(source, destination, seconds after the job's createdAt)` of every transition the
    /// job must report, in order.
    expected_transitions: Vec<(&'static str, &'static str, i64)>,
}

/// The rows: the class the rule generalises over — how many hop destinations a job carries,
/// whether a hop names its destination queue (by name, by ARN, or not at all), whether it
/// states a priority, and how the wait compares with the time the job has actually waited.
fn hop_cases() -> Vec<HopCase> {
    vec![
        HopCase {
            case: "one hop that states a destination priority",
            source: "hop-a",
            priority: None,
            hops: json!([{ "queue": "hop-b", "waitMinutes": 1, "priority": 10 }]),
            advance_by: MINUTE + Duration::from_secs(1),
            expected_queue: "hop-b",
            expected_priority: 10,
            expected_transitions: vec![("hop-a", "hop-b", 60)],
        },
        HopCase {
            case: "one hop that states no priority, so the job keeps its own",
            source: "hop-a",
            priority: Some(7),
            hops: json!([{ "queue": "hop-b", "waitMinutes": 2 }]),
            advance_by: 2 * MINUTE + Duration::from_secs(1),
            expected_queue: "hop-b",
            expected_priority: 7,
            expected_transitions: vec![("hop-a", "hop-b", 120)],
        },
        HopCase {
            case: "a hop that names no queue, so the job moves to the default queue",
            source: "hop-a",
            priority: None,
            hops: json!([{ "waitMinutes": 1, "priority": -10 }]),
            advance_by: MINUTE + Duration::from_secs(1),
            expected_queue: "Default",
            expected_priority: -10,
            expected_transitions: vec![("hop-a", "Default", 60)],
        },
        HopCase {
            case: "a hop that names its destination by ARN",
            source: "hop-a",
            priority: None,
            hops: json!([{ "queue": "ARN:hop-b", "waitMinutes": 1 }]),
            advance_by: MINUTE + Duration::from_secs(1),
            expected_queue: "hop-b",
            expected_priority: 0,
            expected_transitions: vec![("hop-a", "hop-b", 60)],
        },
        HopCase {
            case: "two hops, used in request order, each waited for in the queue it moves \
                   the job out of",
            source: "hop-a",
            priority: None,
            hops: json!([
                { "queue": "hop-b", "waitMinutes": 1, "priority": -10 },
                { "queue": "hop-c", "waitMinutes": 2, "priority": 25 }
            ]),
            advance_by: 3 * MINUTE + Duration::from_secs(1),
            expected_queue: "hop-c",
            expected_priority: 25,
            expected_transitions: vec![("hop-a", "hop-b", 60), ("hop-b", "hop-c", 180)],
        },
        HopCase {
            case: "two hops where only the first one's wait has passed",
            source: "hop-a",
            priority: None,
            hops: json!([
                { "queue": "hop-b", "waitMinutes": 1 },
                { "queue": "hop-c", "waitMinutes": 10 }
            ]),
            advance_by: 2 * MINUTE,
            expected_queue: "hop-b",
            expected_priority: 0,
            expected_transitions: vec![("hop-a", "hop-b", 60)],
        },
        HopCase {
            case: "a hop whose wait has not passed yet",
            source: "hop-a",
            priority: Some(3),
            hops: json!([{ "queue": "hop-b", "waitMinutes": 10, "priority": 40 }]),
            advance_by: 9 * MINUTE,
            expected_queue: "hop-a",
            expected_priority: 3,
            expected_transitions: vec![],
        },
        HopCase {
            // The top of the range `WaitMinutes` publishes, 4320 minutes.
            case: "a hop at the longest wait the member allows",
            source: "hop-a",
            priority: None,
            hops: json!([{ "queue": "hop-b", "waitMinutes": MAX_HOP_WAIT_MINUTES }]),
            advance_by: Duration::from_secs(
                60 * MAX_HOP_WAIT_MINUTES.unsigned_abs() as u64 + 1,
            ),
            expected_queue: "hop-b",
            expected_priority: 0,
            expected_transitions: vec![("hop-a", "hop-b", 60 * i64::from(MAX_HOP_WAIT_MINUTES))],
        },
        HopCase {
            case: "no hop destination at all, so the job never moves",
            source: "hop-a",
            priority: None,
            hops: json!([]),
            advance_by: 10 * MINUTE,
            expected_queue: "hop-a",
            expected_priority: 0,
            expected_transitions: vec![],
        },
    ]
}

/// A job that has waited a hop destination's `WaitMinutes` is moved to that destination,
/// takes its `Priority` when it states one, and reports the move as a `QueueTransition`.
#[tokio::test]
async fn a_job_hops_to_its_destination_once_it_has_waited_the_hops_wait() {
    let emulator = Emulator::with_settle_span(HELD_IN_SUBMITTED);
    seed_queues(
        &emulator,
        &[("hop-a", false), ("hop-b", false), ("hop-c", false)],
    )
    .await;

    for (index, row) in hop_cases().iter().enumerate() {
        let case = row.case;
        // `json!` cannot build an ARN at compile time, so the row states it as a marker
        // and the ARN is substituted from the emulator's own ARN helper.
        let hops = match row.hops.as_array() {
            Some(entries) => Value::Array(
                entries
                    .iter()
                    .map(|entry| {
                        let mut entry = entry.clone();
                        if let Some(name) = entry
                            .pointer("/queue")
                            .and_then(Value::as_str)
                            .and_then(|queue| queue.strip_prefix("ARN:"))
                        {
                            entry["queue"] = json!(queue_arn(name));
                        }
                        entry
                    })
                    .collect(),
            ),
            None => row.hops.clone(),
        };
        let mut request = json!({
            "role": ROLE,
            "queue": row.source,
            "hopDestinations": hops,
            "settings": job_settings(&format!("s3://example-bucket/in/hop-{index}.mp4"), "_h"),
        });
        if let Some(priority) = row.priority {
            request["priority"] = json!(priority);
        }

        let created = emulator.create_job(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");
        let created_at = created.at("/job/createdAt").as_i64().unwrap_or_default();
        assert_eq!(
            transitions(&created),
            Vec::new(),
            "case {case:?}: a job has made no move when it is created, got {}",
            created.body
        );

        advance(&emulator, row.advance_by);

        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/job/queue"),
            &json!(queue_arn(row.expected_queue)),
            "case {case:?}: expected the job on queue {:?}, got {}",
            row.expected_queue,
            fetched.body
        );
        assert_eq!(
            fetched.at("/job/priority"),
            &json!(row.expected_priority),
            "case {case:?}: expected priority {}, got {}",
            row.expected_priority,
            fetched.body
        );
        let expected: Vec<(String, String, i64)> = row
            .expected_transitions
            .iter()
            .map(|(source, destination, offset)| {
                (
                    queue_arn(source),
                    queue_arn(destination),
                    created_at + offset,
                )
            })
            .collect();
        assert_eq!(
            transitions(&fetched),
            expected,
            "case {case:?}: expected the moves {expected:?}, got {:?} from body {}",
            transitions(&fetched),
            fetched.body
        );
        // The job still names the hop destinations it was submitted with, and reading it
        // again reports the same moves: a read materialises what the clock owed, never
        // another move.
        let again = emulator.get_job(&id).await;
        assert_eq!(
            again.at("/job"),
            fetched.at("/job"),
            "case {case:?}: a repeated read must answer the same job, got {}",
            again.body
        );
    }
}

/// A job that is no longer waiting in a queue hops nowhere: queue hopping exists to "avoid
/// overly long waits in the backlog of the queue that you submit your job to", and a
/// cancelled job is in no backlog.
#[tokio::test]
async fn a_job_that_is_no_longer_waiting_does_not_hop() {
    let emulator = Emulator::with_settle_span(HELD_IN_SUBMITTED);
    seed_queues(&emulator, &[("leave-a", false), ("leave-b", false)]).await;

    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": "leave-a",
            "hopDestinations": [{ "queue": "leave-b", "waitMinutes": 1, "priority": 20 }],
            "settings": job_settings("s3://example-bucket/in/cancelled.mp4", "_c"),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job must be created, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");
    let cancelled = emulator.cancel_job(&id).await;
    assert_eq!(
        cancelled.status, 202,
        "the fixture job must be cancellable, got {} with body {}",
        cancelled.status, cancelled.body
    );

    advance(&emulator, 10 * MINUTE);

    let fetched = emulator.get_job(&id).await;
    assert_eq!(
        fetched.string("/job/status"),
        "CANCELED",
        "case a cancelled job: it must stay CANCELED, got {}",
        fetched.body
    );
    assert_eq!(
        fetched.at("/job/queue"),
        &json!(queue_arn("leave-a")),
        "case a cancelled job: it must stay on the queue it was submitted to, got {}",
        fetched.body
    );
    assert_eq!(
        transitions(&fetched),
        Vec::new(),
        "case a cancelled job: it must report no move, got {}",
        fetched.body
    );
    assert_eq!(
        fetched.at("/job/priority"),
        &json!(0),
        "case a cancelled job: it must not take a hop's priority, got {}",
        fetched.body
    );
}

/// A job held back by a `PAUSED` queue is exactly the job queue hopping is for: it still
/// hops when its wait has passed, and once it is on a queue that lets it begin it leaves
/// `SUBMITTED`.
#[tokio::test]
async fn a_job_held_by_a_paused_queue_hops_and_then_begins() {
    let settle = Duration::from_millis(300);
    let emulator = Emulator::with_settle_span(settle);
    seed_queues(&emulator, &[("held-paused", true), ("held-active", false)]).await;

    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": "held-paused",
            "hopDestinations": [{ "queue": "held-active", "waitMinutes": 1 }],
            "settings": job_settings("s3://example-bucket/in/held.mp4", "_p"),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job must be created, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");

    // Past the settle span: a job in a PAUSED queue still has not begun, so it is still
    // waiting in that queue — and still owed its hop.
    tokio::time::sleep(settle + Duration::from_millis(400)).await;
    let held = emulator.get_job(&id).await;
    assert_eq!(
        held.string("/job/status"),
        "SUBMITTED",
        "case held by a paused queue: the job must still be SUBMITTED, got {}",
        held.body
    );

    advance(&emulator, MINUTE + Duration::from_secs(1));

    let hopped = emulator.get_job(&id).await;
    assert_eq!(
        hopped.at("/job/queue"),
        &json!(queue_arn("held-active")),
        "case held by a paused queue: the job must hop to the destination queue, got {}",
        hopped.body
    );
    assert_eq!(
        transitions(&hopped).len(),
        1,
        "case held by a paused queue: expected one recorded move, got {:?}",
        transitions(&hopped)
    );
    assert_ne!(
        hopped.string("/job/status"),
        "SUBMITTED",
        "case held by a paused queue: once on an ACTIVE queue the job must begin, got {}",
        hopped.body
    );
}

/// A hop destination cannot name the queue the job is submitted to: "This queue cannot be
/// the original queue to which the job is submitted", and because a hop that names no queue
/// moves the job to the default queue, the member is "Optional unless the job is submitted
/// on the default queue".
#[tokio::test]
async fn a_hop_destination_cannot_be_the_queue_the_job_was_submitted_to() {
    let emulator = Emulator::with_settle_span(HELD_IN_SUBMITTED);
    seed_queues(&emulator, &[("same-a", false)]).await;

    for (case, queue, hops) in [
        (
            "the source queue by name",
            Some("same-a"),
            json!([{ "queue": "same-a", "waitMinutes": 1 }]),
        ),
        (
            "the source queue by ARN",
            Some("same-a"),
            json!([{ "queue": queue_arn("same-a"), "waitMinutes": 1 }]),
        ),
        (
            "no queue at all, on a job submitted to the default queue",
            None,
            json!([{ "waitMinutes": 1 }]),
        ),
        (
            "the default queue by name, on a job submitted to the default queue",
            None,
            json!([{ "queue": "Default", "waitMinutes": 1 }]),
        ),
        (
            "a second hop naming the source queue, after a resolvable one",
            Some("same-a"),
            json!([
                { "queue": "Default", "waitMinutes": 1 },
                { "queue": "same-a", "waitMinutes": 2 }
            ]),
        ),
    ] {
        let mut request = json!({
            "role": ROLE,
            "hopDestinations": hops,
            "settings": job_settings("s3://example-bucket/in/same.mp4", "_s"),
        });
        if let Some(queue) = queue {
            request["queue"] = json!(queue);
        }
        let reply = emulator.create_job(request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// `WaitMinutes` is held to the range the member publishes — "Valid range is 1 to 4320
/// minutes, inclusive", and it is "required for setting up a job to use queue hopping" —
/// wherever a hop destination is accepted: `CreateJob`, `CreateJobTemplate` and
/// `UpdateJobTemplate` all answer the same.
///
/// The rows span each published bound, one step past either, a value well inside, a value
/// far outside, and a hop destination that states no wait at all.
#[tokio::test]
async fn a_hop_destinations_wait_is_held_to_the_published_range() {
    let emulator = Emulator::with_settle_span(HELD_IN_SUBMITTED);
    seed_queues(&emulator, &[("range-src", false), ("range-dst", false)]).await;
    let fixture = emulator
        .create_job_template(json!({
            "name": "range-template",
            "settings": template_settings(1, 1)
        }))
        .await;
    assert_eq!(
        fixture.status, 201,
        "the fixture job template must be created, got {} with body {}",
        fixture.status, fixture.body
    );

    // (case, the `waitMinutes` the hop states or `None` for a hop that states none,
    //  whether it is accepted)
    let rows: Vec<(String, Option<i32>, bool)> = vec![
        (
            format!("the published minimum {MIN_HOP_WAIT_MINUTES}"),
            Some(MIN_HOP_WAIT_MINUTES),
            true,
        ),
        (
            format!("the published maximum {MAX_HOP_WAIT_MINUTES}"),
            Some(MAX_HOP_WAIT_MINUTES),
            true,
        ),
        ("a wait well inside the range".to_string(), Some(2_160), true),
        (
            format!("one below the published minimum, {}", MIN_HOP_WAIT_MINUTES - 1),
            Some(MIN_HOP_WAIT_MINUTES - 1),
            false,
        ),
        (
            format!("one above the published maximum, {}", MAX_HOP_WAIT_MINUTES + 1),
            Some(MAX_HOP_WAIT_MINUTES + 1),
            false,
        ),
        ("a negative wait".to_string(), Some(-1), false),
        ("a wait far above the range".to_string(), Some(99_999), false),
        ("no wait at all".to_string(), None, false),
    ];

    for (index, (case, minutes, accepted)) in rows.iter().enumerate() {
        let mut hop = json!({ "queue": "range-dst" });
        if let Some(minutes) = minutes {
            hop["waitMinutes"] = json!(minutes);
        }
        let hops = json!([hop]);

        // What the addressed template carries BEFORE the update, so a refused update can
        // be shown to have changed nothing.
        let before = emulator.get_job_template("range-template").await;
        let before_hops = before.template().pointer("/hopDestinations").cloned();

        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": "range-src",
                "hopDestinations": hops.clone(),
                "settings": job_settings(
                    &format!("s3://example-bucket/in/range-{index}.mp4"),
                    "_r",
                ),
            }))
            .await;
        let template = emulator
            .create_job_template(json!({
                "name": format!("range-{index}"),
                "hopDestinations": hops.clone(),
                "settings": template_settings(1, 1)
            }))
            .await;
        let updated = emulator
            .update_job_template("range-template", json!({ "hopDestinations": hops.clone() }))
            .await;

        for (operation, reply, want) in [
            ("CreateJob", &job, 201),
            ("CreateJobTemplate", &template, 201),
            ("UpdateJobTemplate", &updated, 200),
        ] {
            let labelled = format!("{operation}: {case}");
            if *accepted {
                assert_eq!(
                    reply.status, want,
                    "case {labelled:?}: expected HTTP {want}, got {} with body {}",
                    reply.status, reply.body
                );
            } else {
                assert_eq!(
                    reply.status, 400,
                    "case {labelled:?}: expected HTTP 400, got {} with body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_type.as_deref(),
                    Some("BadRequestException"),
                    "case {labelled:?}: expected BadRequestException, got {:?}",
                    reply.error_type
                );
            }
        }

        let after = emulator.get_job_template("range-template").await;
        let after_hops = after.template().pointer("/hopDestinations").cloned();
        if *accepted {
            assert_eq!(
                after_hops,
                Some(hops.clone()),
                "case {case:?}: an accepted UpdateJobTemplate must store the hop list it was \
                 given, got {}",
                after.body
            );
            // Round trip: an accepted wait reads back off the job too.
            let fetched = emulator.get_job(&job.string("/job/id")).await;
            assert_eq!(
                fetched.at("/job/hopDestinations/0/waitMinutes"),
                &json!(minutes),
                "case {case:?}: GetJob must report the stored wait, got {}",
                fetched.body
            );
        } else {
            assert_eq!(
                after_hops, before_hops,
                "case {case:?}: a refused UpdateJobTemplate must change nothing, got {}",
                after.body
            );
            let absent = emulator.get_job_template(&format!("range-{index}")).await;
            assert_eq!(
                absent.status, 404,
                "case {case:?}: a refused CreateJobTemplate must store nothing, got {} with \
                 body {}",
                absent.status, absent.body
            );
        }
    }
}
