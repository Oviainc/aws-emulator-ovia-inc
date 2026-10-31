// SPDX-License-Identifier: CC0-1.0

//! What StartFlow and StopFlow do to a flow that is NOT in the status their transition
//! begins at, end to end over HTTP.
//!
//! The rule, asserted over a SET of inputs rather than one call: a flow lifecycle
//! transition is accepted only from the status it begins at — a start from the stopped
//! status, a stop from the running one — answers a repeat of itself with the status the
//! flow already has, and is refused with the modelled semantic-error exception
//! (`BadRequestException`, HTTP 400) in every other status, leaving the flow, and any
//! status change the clock already owes it, exactly as they were.
//!
//! Where the statuses come from: `smithy_query(query="shape",shape="Status")` — STANDBY |
//! ACTIVE | UPDATING | DELETING | STARTING | STOPPING | ERROR. DeleteFlow's documentation
//! ("Deletes a flow. Before you can delete a flow, you must stop the flow",
//! `smithy_query(query="doc",shape="DeleteFlow")`) is what names `STANDBY` the stopped
//! status, and `StartFlowResponse$Status` / `StopFlowResponse$Status` ("the status of the
//! flow when the StartFlow process begins") is what makes `STARTING` and `STOPPING` the
//! transient statuses the clock settles.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the request path a MediaConnect client reaches.

use std::time::{Duration, SystemTime};

use serde_json::json;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::lifecycle_preconditions::{created_flow_arn, seed_flow};

/// Longer than [`mc::TRANSITION_SPAN`], so a status change the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Every value of the modelled `Status` enum, in the model's own order.
///
/// Source: `smithy_query(query="shape",shape="Status")`.
const EVERY_STATUS: [&str; 7] = [
    "STANDBY",
    "ACTIVE",
    "UPDATING",
    "DELETING",
    "STARTING",
    "STOPPING",
    "ERROR",
];

/// What the rule says one call in one status answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The transition begins: 202, and the flow reports the transient status.
    Accepted(&'static str),
    /// A repeat of a transition that is already running or already done: 202, and the flow
    /// keeps the status it has.
    Repeat,
    /// Refused with 400 `BadRequestException` naming the status the transition begins at.
    Refused(&'static str),
}

/// One flow lifecycle transition, as a client reaches it.
#[derive(Debug, Clone, Copy)]
struct Transition {
    /// The path segment the operation is bound to (`/v1/flows/{segment}/{FlowArn}`).
    segment: &'static str,
    /// The status the transition begins at, which its refusal names.
    begins_at: &'static str,
    /// The transient status an accepted call reports.
    transient: &'static str,
    /// The status the clock settles the transient one into.
    settles_into: &'static str,
}

const START: Transition = Transition {
    segment: "start",
    begins_at: "STANDBY",
    transient: "STARTING",
    settles_into: "ACTIVE",
};

const STOP: Transition = Transition {
    segment: "stop",
    begins_at: "ACTIVE",
    transient: "STOPPING",
    settles_into: "STANDBY",
};

/// What the rule says `transition` answers for a flow sitting in `status`.
fn expected(transition: &Transition, status: &str) -> Expected {
    if status == transition.begins_at {
        Expected::Accepted(transition.transient)
    } else if status == transition.transient || status == transition.settles_into {
        Expected::Repeat
    } else {
        Expected::Refused(transition.begins_at)
    }
}

async fn describe_status(emulator: &Emulator, flow_arn: &str) -> String {
    let (http_status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (http_status, code.as_deref()),
        (200, None),
        "DescribeFlow expected 200, got {http_status} {code:?}: {described}"
    );
    described["flow"]["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the described flow reports a status: {described}"))
        .to_string()
}

/// The status change the clock owes the stored flow, as `(status, due_at)`, read straight
/// off the record — so a case can assert that a refused call left the owed change alone
/// without waiting on any clock.
fn owed_change(emulator: &Emulator, flow_arn: &str) -> Option<(String, SystemTime)> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .flows
        .get(flow_arn)
        .and_then(|flow| flow.pending.clone())
        .map(|pending| (pending.status, pending.due_at))
}

/// Rule, the whole table: for each transition, every value of the modelled `Status` enum,
/// with the flow put in that status and nothing else varying.
#[tokio::test]
async fn a_flow_transition_is_accepted_only_from_the_status_it_begins_at() {
    let emulator = Emulator::new();

    for transition in [START, STOP] {
        for status in EVERY_STATUS {
            let case = format!("{} a flow in {status}", transition.segment);
            let flow_arn = seed_flow(
                &emulator,
                &format!("{}-from-{status}", transition.segment),
                status,
            );
            let (http_status, code, body) = emulator
                .send(
                    "POST",
                    &format!("/v1/flows/{}/{flow_arn}", transition.segment),
                    None,
                )
                .await;

            match expected(&transition, status) {
                Expected::Accepted(transient) => {
                    assert_eq!(
                        (http_status, code.as_deref(), body["status"].as_str()),
                        (202, None, Some(transient)),
                        "case {case:?}: expected 202 {transient}, got {http_status} {code:?}: \
                         {body}"
                    );
                    let described = describe_status(&emulator, &flow_arn).await;
                    assert_eq!(
                        described, transient,
                        "case {case:?}: the transient status is persisted, got {described:?}"
                    );
                }
                Expected::Repeat => {
                    assert_eq!(
                        (http_status, code.as_deref(), body["status"].as_str()),
                        (202, None, Some(status)),
                        "case {case:?}: a repeat answers the status the flow has, got \
                         {http_status} {code:?}: {body}"
                    );
                    let described = describe_status(&emulator, &flow_arn).await;
                    assert_eq!(
                        described, status,
                        "case {case:?}: a repeat leaves the status alone, got {described:?}"
                    );
                }
                Expected::Refused(begins_at) => {
                    assert_eq!(
                        (http_status, code.as_deref()),
                        (400, Some("BadRequestException")),
                        "case {case:?}: expected 400 BadRequestException, got {http_status} \
                         {code:?}: {body}"
                    );
                    let message = body["message"].as_str().unwrap_or_default();
                    let Some(detail) = message.strip_prefix(mc::BAD_REQUEST_PREFIX) else {
                        panic!(
                            "case {case:?}: the refusal opens with {:?}, got {message:?}",
                            mc::BAD_REQUEST_PREFIX
                        );
                    };
                    assert!(
                        detail.contains(begins_at),
                        "case {case:?}: the refusal names the status the transition begins \
                         at ({begins_at}), got {detail:?}"
                    );
                    let described = describe_status(&emulator, &flow_arn).await;
                    assert_eq!(
                        described, status,
                        "case {case:?}: a refused call leaves the flow intact, got \
                         {described:?}"
                    );
                }
            }
        }
    }
}

/// Rule, the case the clock makes: while a transition this service ACCEPTED is still in
/// flight, the opposite transition is refused and the owed change survives — same owed
/// status, same due instant — and the clock then settles it, so the accepted call really
/// happens. The rows are both directions (a stop a start is refused over, and a start a
/// stop is refused over), and each one repeats the refused call to show the refusal is not
/// a one-shot.
#[tokio::test]
async fn an_accepted_flow_transition_survives_the_opposite_call() {
    let emulator = Emulator::new();

    for (accepted, opposite) in [(STOP, START), (START, STOP)] {
        let case = format!("{} in flight, {} refused", accepted.segment, opposite.segment);
        let flow_arn = created_flow_arn(
            &emulator,
            &format!("in-flight-{}", accepted.segment),
            json!({}),
        )
        .await;

        // Put the flow in the status the accepted transition begins at. For a stop that is
        // the running status, which a start the clock has settled reaches.
        if accepted.begins_at != "STANDBY" {
            let (http_status, _, body) = emulator
                .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
                .await;
            assert_eq!(
                http_status, 202,
                "case {case:?}: the fixture start must be accepted, got {http_status}: {body}"
            );
            tokio::time::sleep(PAST_THE_TRANSITION).await;
            let settled = describe_status(&emulator, &flow_arn).await;
            assert_eq!(
                settled, "ACTIVE",
                "case {case:?}: the fixture flow must be running, got {settled:?}"
            );
        }

        // The call the service ACCEPTS: 202, the transient status, and a change the clock
        // owes.
        let (http_status, code, body) = emulator
            .send(
                "POST",
                &format!("/v1/flows/{}/{flow_arn}", accepted.segment),
                None,
            )
            .await;
        assert_eq!(
            (http_status, code.as_deref(), body["status"].as_str()),
            (202, None, Some(accepted.transient)),
            "case {case:?}: the accepted call expected 202 {}, got {http_status} {code:?}: \
             {body}",
            accepted.transient
        );
        let owed = owed_change(&emulator, &flow_arn);
        assert_eq!(
            owed.as_ref().map(|(status, _)| status.as_str()),
            Some(accepted.settles_into),
            "case {case:?}: the clock must owe the flow {}, got {owed:?}",
            accepted.settles_into
        );

        // The opposite call, twice, while that change is still owed: refused, and the owed
        // change is untouched both times.
        for attempt in 1..=2 {
            let (http_status, code, refused) = emulator
                .send(
                    "POST",
                    &format!("/v1/flows/{}/{flow_arn}", opposite.segment),
                    None,
                )
                .await;
            assert_eq!(
                (http_status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?} attempt {attempt}: expected 400 BadRequestException, got \
                 {http_status} {code:?}: {refused}"
            );
            let message = refused["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(opposite.begins_at),
                "case {case:?} attempt {attempt}: the refusal names the status the \
                 transition begins at ({}), got {message:?}",
                opposite.begins_at
            );
            assert_eq!(
                owed_change(&emulator, &flow_arn),
                owed,
                "case {case:?} attempt {attempt}: the refused call must leave the owed \
                 change alone"
            );
            let described = describe_status(&emulator, &flow_arn).await;
            assert_eq!(
                described, accepted.transient,
                "case {case:?} attempt {attempt}: the flow keeps its transient status, got \
                 {described:?}"
            );
        }

        // A repeat of the ACCEPTED call, still in flight, neither restarts the transition
        // nor pushes its deadline out.
        let (http_status, code, repeated) = emulator
            .send(
                "POST",
                &format!("/v1/flows/{}/{flow_arn}", accepted.segment),
                None,
            )
            .await;
        assert_eq!(
            (http_status, code.as_deref(), repeated["status"].as_str()),
            (202, None, Some(accepted.transient)),
            "case {case:?}: a repeat of the accepted call expected 202 {}, got {http_status} \
             {code:?}: {repeated}",
            accepted.transient
        );
        assert_eq!(
            owed_change(&emulator, &flow_arn),
            owed,
            "case {case:?}: a repeat must not move the instant the change is owed at"
        );

        // And the clock settles the change that was accepted: the call the caller was told
        // had been accepted really happens.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let settled = describe_status(&emulator, &flow_arn).await;
        assert_eq!(
            settled, accepted.settles_into,
            "case {case:?}: the accepted transition must settle into {}, got {settled:?}",
            accepted.settles_into
        );
        assert_eq!(
            owed_change(&emulator, &flow_arn),
            None,
            "case {case:?}: nothing is owed once the change has settled"
        );
    }
}

/// Rule, read through the operation the abandoned stop used to break: a flow whose stop was
/// accepted is deletable once the clock has settled it, because no start could have driven
/// it back to running in the meantime. The rows differ in what the flow holds, so the
/// assertion is about the status rather than the content.
#[tokio::test]
async fn a_stop_that_was_accepted_leaves_the_flow_deletable() {
    let emulator = Emulator::new();

    for (case, extra) in [
        ("a bare flow", json!({})),
        (
            "a flow with an output and tags",
            json!({
                "outputs": [
                    { "name": "out-1", "protocol": "rtp", "destination": "203.0.113.9", "port": 5004 }
                ],
                "flowTags": { "team": "video" }
            }),
        ),
    ] {
        let flow_arn = created_flow_arn(
            &emulator,
            &format!("stop-then-delete-{}", case.replace(' ', "-")),
            extra,
        )
        .await;

        for segment in ["start", "stop"] {
            let (http_status, _, body) = emulator
                .send("POST", &format!("/v1/flows/{segment}/{flow_arn}"), None)
                .await;
            assert_eq!(
                http_status, 202,
                "case {case:?}: {segment} expected 202, got {http_status}: {body}"
            );
            if segment == "start" {
                tokio::time::sleep(PAST_THE_TRANSITION).await;
            }
        }

        // The start issued while the stop is in flight is refused, so the stop stands.
        let (http_status, code, refused) = emulator
            .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
            .await;
        assert_eq!(
            (http_status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the start must be refused while the stop is in flight, got \
             {http_status} {code:?}: {refused}"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let settled = describe_status(&emulator, &flow_arn).await;
        assert_eq!(
            settled, "STANDBY",
            "case {case:?}: the stopped flow must be in STANDBY, got {settled:?}"
        );

        let (http_status, code, deleted) = emulator
            .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
            .await;
        assert_eq!(
            (http_status, code.as_deref(), deleted["status"].as_str()),
            (202, None, Some("DELETING")),
            "case {case:?}: the stopped flow must be deletable, got {http_status} {code:?}: \
             {deleted}"
        );
    }
}
