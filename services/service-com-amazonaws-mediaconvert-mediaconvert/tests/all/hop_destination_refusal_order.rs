// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that what a request member says about ITSELF is
//! decided before any reference that member carries is resolved against stored state, so
//! one request body is answered with one status and one error code at every operation that
//! accepts the member.
//!
//! `HopDestinations` is the member this suite drives it through, because it is the one
//! member this service accepts at more than one door that carries BOTH kinds of test: what
//! the entry says about itself — `WaitMinutes` is "Required for setting up a job to use
//! queue hopping … Valid range is 1 to 4320 minutes, inclusive" and `Queue`, when stated,
//! has to carry a reference at all — and a reference the store resolves, since `Queue` "is
//! a destination queue" a job may hop to
//! (`smithy_query(query="shape",shape="HopDestination",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`).
//! A list that cannot describe a hop at all is therefore the modelled `BadRequestException`
//! (400) wherever it is sent, whether or not the queue it names is one this account holds;
//! the lookup refusal `NotFoundException` (404) is reached only by a list that is otherwise
//! a hop.
//!
//! Every row is driven through all three operations that accept a hop list — `CreateJob`,
//! `CreateJobTemplate` and `UpdateJobTemplate` — and each row asserts the three answers are
//! IDENTICAL as well as what they are, which is the half of the rule a door taking the two
//! tests in its own order breaks. `tests/all/queue_hopping.rs` pins the published range
//! itself and `tests/all/blank_resource_name.rs` the blank-reference half; this suite is
//! about which answer a list carrying more than one problem gets.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    not_found_message, MAX_HOP_WAIT_MINUTES, MIN_HOP_WAIT_MINUTES,
};

use super::blank_resource_name::refusal_message;
use super::job_lifecycle::{queue_arn, settings as job_settings, Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// The queue the jobs below are submitted to. A hop destination "cannot be the original
/// queue to which the job is submitted", so the source is a queue of this suite's own
/// rather than the account's default.
const SOURCE_QUEUE: &str = "order-source";

/// A stored queue a hop destination can resolve to.
const DESTINATION_QUEUE: &str = "order-destination";

/// A queue reference this account holds nothing for, so resolving it is the lookup refusal.
const UNKNOWN_QUEUE: &str = "order-no-such-queue";

/// The template `UpdateJobTemplate` addresses in every row.
const FIXTURE_TEMPLATE: &str = "order-template";

/// How this service words a `WaitMinutes` outside the range the member publishes: the
/// sentence names the member and the published bounds, which this suite reads from the
/// crate's own constants rather than restating them.
fn wait_out_of_range_message() -> String {
    format!(
        "You specified an invalid value for WaitMinutes. Valid range is \
         {MIN_HOP_WAIT_MINUTES} to {MAX_HOP_WAIT_MINUTES} minutes, inclusive."
    )
}

/// How this service words a hop destination that states no wait at all, which is what
/// "required for setting up a job to use queue hopping" leaves it as.
fn wait_missing_message() -> String {
    "WaitMinutes is required for every entry in hopDestinations: it is the minimum wait \
     time in minutes until the job can hop to the destination queue."
        .to_string()
}

/// What every door that accepts a hop list answers for one row of it.
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` the refusal carries, or `None` for a row that is accepted.
    code: Option<&'static str>,
    /// The sentence the refusal states, or `None` for a row that is accepted.
    message: Option<String>,
}

impl Answer {
    /// The modelled bad request this service answers a list that describes no hop with.
    fn bad_request(message: String) -> Self {
        Self {
            status: 400,
            code: Some("BadRequestException"),
            message: Some(message),
        }
    }

    /// The modelled lookup refusal a list that IS a hop, but names a queue this account
    /// does not hold, gets.
    fn not_found() -> Self {
        Self {
            status: 404,
            code: Some("NotFoundException"),
            message: Some(not_found_message("hopDestinations")),
        }
    }

    /// A list that sets up a hop: accepted, with the status the door reports a created or
    /// an updated resource with.
    fn accepted() -> Self {
        Self {
            status: 0,
            code: None,
            message: None,
        }
    }

    /// Whether this row is accepted, which is the only case where the expected status
    /// differs between the doors (201 for a create, 200 for an update).
    fn is_accepted(&self) -> bool {
        self.code.is_none()
    }
}

/// Assert `reply` is the answer `expected` states, for the door and row `case` names.
fn assert_answer(case: &str, accepted_status: u16, expected: &Answer, reply: &Reply) {
    let want = if expected.is_accepted() {
        accepted_status
    } else {
        expected.status
    };
    assert_eq!(
        reply.status, want,
        "case {case:?}: expected HTTP {want}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        expected.code,
        "case {case:?}: expected error code {:?}, got {:?} with body {}",
        expected.code,
        reply.error_type,
        reply.body
    );
    if let Some(message) = expected.message.as_deref() {
        assert_eq!(
            reply.string("/message"),
            message,
            "case {case:?}: expected the refusal to state {message:?}, got body {}",
            reply.body
        );
    }
}

/// One hop list, answered at all three doors.
struct Replies {
    job: Reply,
    template: Reply,
    updated: Reply,
}

/// Send `hops` to all three operations that accept a hop list, each otherwise valid.
async fn through_every_door(emulator: &Emulator, index: usize, hops: &Value) -> Replies {
    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": SOURCE_QUEUE,
            "hopDestinations": hops.clone(),
            "settings": job_settings(
                &format!("s3://example-bucket/in/order-{index}.mp4"),
                "_o",
            ),
        }))
        .await;
    let template = emulator
        .create_job_template(json!({
            "name": created_name(index),
            "hopDestinations": hops.clone(),
            "settings": template_settings(1, 1),
        }))
        .await;
    let updated = emulator
        .update_job_template(FIXTURE_TEMPLATE, json!({ "hopDestinations": hops.clone() }))
        .await;
    Replies {
        job,
        template,
        updated,
    }
}

/// The name row `index` creates a template under, so a refused create can be shown to have
/// stored nothing.
fn created_name(index: usize) -> String {
    format!("order-{index}")
}

/// Create the queues and the template every row needs.
async fn seed(emulator: &Emulator) {
    for name in [SOURCE_QUEUE, DESTINATION_QUEUE] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created, got {} with body {}",
            created.status, created.body
        );
    }
    let created = emulator
        .create_job_template(json!({
            "name": FIXTURE_TEMPLATE,
            "settings": template_settings(1, 1),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created, got {} with body {}",
        created.status, created.body
    );
}

/// One hop list gets ONE status and ONE error code at every door that accepts hop
/// destinations: what the list says about itself is decided first, so a list that is wrong
/// in its own value AND names a queue this account does not hold is the bad request, never
/// the lookup refusal at one door and the bad request at another.
///
/// The rows span what the rule generalises over: which of the member's own tests the list
/// fails (a wait below the published minimum, one above the published maximum, a negative
/// one, none at all, a `Queue` carrying no characters), the reference form the unresolvable
/// queue is stated in (a name and an ARN), the position of the offending entry in the list
/// (the only one, the first of two, the second of two), and — as the boundary — a list whose
/// only problem is the lookup (the 404) and one with no problem at all (accepted).
#[tokio::test]
async fn one_hop_list_is_answered_the_same_way_at_every_door_that_accepts_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(String, Value, Answer)> = vec![
        (
            format!(
                "an unknown destination queue and a wait one below the published minimum, {}",
                MIN_HOP_WAIT_MINUTES - 1
            ),
            json!([{ "queue": UNKNOWN_QUEUE, "waitMinutes": MIN_HOP_WAIT_MINUTES - 1 }]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            format!(
                "an unknown destination queue and a wait one above the published maximum, {}",
                MAX_HOP_WAIT_MINUTES + 1
            ),
            json!([{ "queue": UNKNOWN_QUEUE, "waitMinutes": MAX_HOP_WAIT_MINUTES + 1 }]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            "an unknown destination queue by ARN and a negative wait".to_string(),
            json!([{ "queue": queue_arn(UNKNOWN_QUEUE), "waitMinutes": -5 }]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            "an unknown destination queue and no wait at all".to_string(),
            json!([{ "queue": UNKNOWN_QUEUE }]),
            Answer::bad_request(wait_missing_message()),
        ),
        (
            "a destination queue carrying no characters and a wait below the minimum"
                .to_string(),
            json!([{ "queue": "   ", "waitMinutes": 0 }]),
            Answer::bad_request(refusal_message("HopDestinations.Queue")),
        ),
        (
            "a stored destination queue and a wait above the maximum".to_string(),
            json!([{ "queue": DESTINATION_QUEUE, "waitMinutes": 10_000 }]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            "a first entry wrong only in its wait, a second naming an unknown queue"
                .to_string(),
            json!([
                { "queue": DESTINATION_QUEUE, "waitMinutes": 0 },
                { "queue": UNKNOWN_QUEUE, "waitMinutes": 20 },
            ]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            "a first entry naming an unknown queue, a second wrong only in its wait"
                .to_string(),
            json!([
                { "queue": UNKNOWN_QUEUE, "waitMinutes": 20 },
                { "queue": DESTINATION_QUEUE, "waitMinutes": 0 },
            ]),
            Answer::bad_request(wait_out_of_range_message()),
        ),
        (
            "an unknown destination queue and a valid wait".to_string(),
            json!([{ "queue": UNKNOWN_QUEUE, "waitMinutes": 10 }]),
            Answer::not_found(),
        ),
        (
            "a stored destination queue and a valid wait".to_string(),
            json!([{ "queue": DESTINATION_QUEUE, "waitMinutes": MIN_HOP_WAIT_MINUTES }]),
            Answer::accepted(),
        ),
    ];

    for (index, (case, hops, expected)) in rows.iter().enumerate() {
        // What the addressed template carries BEFORE the update, so a refused update can be
        // shown to have changed nothing whatever an earlier row left there.
        let before = emulator.get_job_template(FIXTURE_TEMPLATE).await;
        let before_hops = before.at("/jobTemplate/hopDestinations").clone();

        let replies = through_every_door(&emulator, index, hops).await;
        for (operation, reply, accepted_status) in [
            ("CreateJob", &replies.job, 201),
            ("CreateJobTemplate", &replies.template, 201),
            ("UpdateJobTemplate", &replies.updated, 200),
        ] {
            assert_answer(
                &format!("{operation}: {case}"),
                accepted_status,
                expected,
                reply,
            );
        }
        // The same hop list, so the same refusal: the two template doors answer whatever
        // `CreateJob` answered, code and sentence alike.
        for (operation, reply) in [
            ("CreateJobTemplate", &replies.template),
            ("UpdateJobTemplate", &replies.updated),
        ] {
            assert_eq!(
                (reply.error_type.as_deref(), reply.string("/message")),
                (
                    replies.job.error_type.as_deref(),
                    replies.job.string("/message")
                ),
                "case {operation}: {case:?}: the door must answer the hop list exactly as \
                 CreateJob did, got {} with body {}",
                reply.status,
                reply.body
            );
        }

        let fetched = emulator.get_job_template(&created_name(index)).await;
        let after = emulator.get_job_template(FIXTURE_TEMPLATE).await;
        if expected.is_accepted() {
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: an accepted create must store the template, got {} with \
                 body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                after.at("/jobTemplate/hopDestinations").as_array().map(Vec::len),
                hops.as_array().map(Vec::len),
                "case {case:?}: an accepted update must store the hop list it was given, \
                 got body {}",
                after.body
            );
        } else {
            // Every refusal above left the store exactly as it was.
            assert_eq!(
                fetched.status, 404,
                "case {case:?}: a refused create must store nothing, got {} with body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                after.at("/jobTemplate/hopDestinations"),
                &before_hops,
                "case {case:?}: a refused update must change nothing, got body {}",
                after.body
            );
        }
    }
}

/// The same rule on the OTHER references a request carries: a hop list that describes no
/// hop is the bad request even when the request names a resource this account does not hold
/// — the top-level `Queue` of a job or a new template, and the `{Name}` an update addresses
/// — because what the member says about itself is decided from the request alone, before
/// any lookup.
///
/// The rows span the reference each door resolves: a job's queue, a new template's queue,
/// and the template an update addresses, driven with each of the member's own problems.
#[tokio::test]
async fn a_hop_list_that_describes_no_hop_is_refused_before_any_reference_is_resolved() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for (problem, hops, message) in [
        (
            "a wait below the published minimum",
            json!([{ "queue": DESTINATION_QUEUE, "waitMinutes": 0 }]),
            wait_out_of_range_message(),
        ),
        (
            "no wait at all",
            json!([{ "queue": DESTINATION_QUEUE }]),
            wait_missing_message(),
        ),
        (
            "a destination queue carrying no characters",
            json!([{ "queue": "", "waitMinutes": 10 }]),
            refusal_message("HopDestinations.Queue"),
        ),
    ] {
        let expected = Answer::bad_request(message);

        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": UNKNOWN_QUEUE,
                "hopDestinations": hops.clone(),
                "settings": job_settings("s3://example-bucket/in/order-ref.mp4", "_x"),
            }))
            .await;
        assert_answer(
            &format!("CreateJob onto an unknown queue, with {problem}"),
            201,
            &expected,
            &job,
        );

        let template = emulator
            .create_job_template(json!({
                "name": "order-unresolvable",
                "queue": UNKNOWN_QUEUE,
                "hopDestinations": hops.clone(),
                "settings": template_settings(1, 1),
            }))
            .await;
        assert_answer(
            &format!("CreateJobTemplate naming an unknown queue, with {problem}"),
            201,
            &expected,
            &template,
        );

        let updated = emulator
            .update_job_template(
                "order-no-such-template",
                json!({ "hopDestinations": hops.clone() }),
            )
            .await;
        assert_answer(
            &format!("UpdateJobTemplate addressing an unknown template, with {problem}"),
            200,
            &expected,
            &updated,
        );

        let fetched = emulator.get_job_template("order-unresolvable").await;
        assert_eq!(
            fetched.status, 404,
            "case {problem:?}: a refused create must store nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }
}
