// SPDX-License-Identifier: CC0-1.0

//! A REPEATED update of a feed's payload, end to end over the wire: `UpdateFeed`.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the body, the handler runs, and the
//! response is serialized as `restJson1` — so what these tests assert is what a client
//! sees.
//!
//! THE RULE under test: `UpdateFeed` is `@idempotent`
//! (`smithy_query(query="operation",operation="UpdateFeed")`, also `OPERATIONS.md`) and
//! carries no `@idempotencyToken`, so the request's own members are its key. A request
//! that states the payload the feed already holds, while the transient reconfiguration
//! which stored that payload is still in flight, is that one request once more — what an
//! SDK repeats when it never saw the first response — and several identical requests
//! have the effect of ONE: nothing further is written and the move the clock owes stays
//! due at the instant the first call set, so no number of retries re-enters the
//! transient status or postpones the feed's return to the status that call suspended.
//! The rule reaches no further than the request it recognises: a payload differing in
//! any member is a reconfiguration of its own and starts its own span, whatever status
//! the feed is in.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the settled status the reconfiguration suspends, how many outputs the payload carries
//! and which `OutputConfig` member each one is, each output's own status, the length of
//! the name, whether the payload supplies or drops the access role, and how many times
//! the request arrives.
//!
//! WHEN the feed's transition falls due is the fact the rule is about, and a request
//! arriving over the wire cannot report it: the span is short
//! (`crate::harness::past_the_transition_span`), so a case that read it by sleeping up to
//! it would be asserting on the scheduler rather than on the service. It is read instead
//! off the stored record through the same state seam the fixture suites use
//! (`Feed::transition_due_at`), which is the instant itself and needs no wait at all.

use crate::harness::{
    call, enabled_output, output, past_the_transition_span, router_with_engine, subtitling_output,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::ComAmazonawsElementalinferenceElementalinferenceState;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Instant;

/// The resource the `ACTIVE` and `ARCHIVED` rows claim their feed with.
const HELD_MEDIA: &str = "retried-update-media";

/// A role ARN in the account this emulator serves — the only account whose roles a feed
/// may carry (`IamRoleArn`: `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$`).
fn own_account_role(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::EMULATOR_ACCOUNT_ID)
}

/// `GET /v1/feed/{id}`.
async fn get_feed(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}"), None).await
}

/// `PUT /v1/feed/{id}` with `payload`.
async fn update_feed(router: &MultiServiceRouter, id: &str, payload: &Value) -> (u16, Value) {
    call(router, "PUT", &format!("/v1/feed/{id}"), Some(payload.clone())).await
}

/// The `status` member of a response, as text.
fn status_of(body: &Value) -> &str {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the response carries a status, got {body}"))
}

/// How many outputs a payload carries.
fn output_count(payload: &Value) -> Option<usize> {
    payload["outputs"].as_array().map(Vec::len)
}

/// The instant the clock owes this feed its next move at, read off the stored record.
fn transition_due_at(
    engine: &Arc<engine::InMemoryEngine>,
    id: &str,
    case: &str,
) -> Option<Instant> {
    engine
        .state::<ComAmazonawsElementalinferenceElementalinferenceState>()
        .lock()
        .feeds
        .get(id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the feed under test"))
        .transition_due_at
}

/// As [`transition_due_at`], for a feed that is expected to owe the clock a move.
fn owed_move(engine: &Arc<engine::InMemoryEngine>, id: &str, case: &str) -> Instant {
    transition_due_at(engine, id, case).unwrap_or_else(|| {
        panic!("case {case:?}: a feed being reconfigured owes the clock the move back")
    })
}

/// Create one feed and drive it into `held`, the settled status the reconfiguration under
/// test is to suspend; return its id.
///
/// The feed starts out with a name, an access role and one output, so a payload below can
/// differ from it — or match it — in any member a `PUT` states.
async fn feed_in_status(router: &MultiServiceRouter, held: &str, case: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "retried-update-feed",
            "accessRoleArn": own_account_role("retried-update-original"),
            "outputs": [enabled_output("crop1", "cropping")],
            "tags": {"env": "test"}
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateFeed answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateFeed returns an id, got {created}"))
        .to_string();

    if held != "AVAILABLE" {
        let (status, claimed) = call(
            router,
            "POST",
            &format!("/v1/feed/{id}/associate"),
            Some(json!({
                "associatedResourceName": HELD_MEDIA,
                "outputs": [enabled_output("assoc1", "contextualMetadata")],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the claim is accepted, got {status} with {claimed}"
        );
    }
    if held == "ARCHIVED" {
        let (status, released) = call(
            router,
            "POST",
            &format!("/v1/feed/{id}/disassociate"),
            Some(json!({"associatedResourceName": HELD_MEDIA})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the release is accepted, got {status} with {released}"
        );
    }

    let (status, feed) = get_feed(router, &id).await;
    assert_eq!(
        (status, status_of(&feed)),
        (200, held),
        "case {case:?}: the feed starts in the status under test, got {status} with {feed}"
    );
    id
}

/// One row of the repeat table: the settled status the reconfiguration suspends, the
/// payload the request states, and how many times that one request arrives.
struct ReplayCase {
    label: &'static str,
    held: &'static str,
    payload: Value,
    arrivals: usize,
}

/// A row per shape the repeated request can take: each suspendable status, one output and
/// several, every `OutputConfig` member, an output that is `DISABLED` rather than
/// `ENABLED`, a one-character name, a payload that keeps the access role and ones that
/// drop it, and two to five arrivals of the one request.
fn replay_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            label: "an unassociated feed, one clipping output and a one-character name",
            held: "AVAILABLE",
            payload: json!({
                "name": "a",
                "accessRoleArn": own_account_role("retried-update-replacement"),
                "outputs": [enabled_output("clip1", "clipping")]
            }),
            arrivals: 2,
        },
        ReplayCase {
            label: "an unassociated feed grown to three outputs, with the role dropped",
            held: "AVAILABLE",
            payload: json!({
                "name": "retried-update-grown",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("meta1", "contextualMetadata"),
                    subtitling_output("subs1", None)
                ]
            }),
            arrivals: 4,
        },
        ReplayCase {
            label: "an unassociated feed whose single output is DISABLED",
            held: "AVAILABLE",
            payload: json!({
                "name": "retried-update-disabled",
                "outputs": [output("crop1", "cropping", "DISABLED")]
            }),
            arrivals: 3,
        },
        ReplayCase {
            label: "a feed a resource holds, shrunk to one subtitling output",
            held: "ACTIVE",
            payload: json!({
                "name": "retried-update-while-held",
                "outputs": [subtitling_output("subs1", None)]
            }),
            arrivals: 3,
        },
        ReplayCase {
            label: "a feed whose resource has been released, two outputs and the role kept",
            held: "ARCHIVED",
            payload: json!({
                "name": "retried-update-after-release",
                "accessRoleArn": own_account_role("retried-update-original"),
                "outputs": [
                    enabled_output("clip1", "clipping"),
                    enabled_output("meta1", "contextualMetadata")
                ]
            }),
            arrivals: 5,
        },
    ]
}

/// The one request arriving again while the reconfiguration it asked for is still in
/// flight has the effect of that ONE request: every arrival answers the same body, the
/// feed stays in the transient status it was already in, and the move the clock owes
/// stays due at the instant the FIRST arrival set — so retrying cannot postpone the
/// feed's return to the status the reconfiguration suspended.
#[tokio::test]
async fn a_repeated_update_in_flight_has_the_effect_of_one_request() {
    for case in replay_cases() {
        let label = case.label;
        let (router, engine) = router_with_engine();
        let id = feed_in_status(&router, case.held, label).await;

        let (status, first) = update_feed(&router, &id, &case.payload).await;
        assert_eq!(
            (status, status_of(&first)),
            (200, case.held),
            "case {label:?}: the first update answers 200 with the settled status, got {status} \
             with {first}"
        );
        let due = owed_move(&engine, &id, label);
        let (status, in_flight) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&in_flight)),
            (200, "UPDATING"),
            "case {label:?}: the reconfiguration is in flight, got {status} with {in_flight}"
        );

        for arrival in 2..=case.arrivals {
            let (status, repeated) = update_feed(&router, &id, &case.payload).await;
            assert_eq!(
                status, 200,
                "case {label:?} arrival {arrival}: the repeat answers 200, got {status} with \
                 {repeated}"
            );
            assert_eq!(
                repeated, first,
                "case {label:?} arrival {arrival}: expected the repeat to answer the first \
                 call's body {first}, got {repeated}"
            );
            let repeated_due = owed_move(&engine, &id, label);
            assert_eq!(
                repeated_due, due,
                "case {label:?} arrival {arrival}: expected the move to stay due at the instant \
                 the first call set, got a due instant {:?} later",
                repeated_due.saturating_duration_since(due)
            );
            let (status, still_in_flight) = get_feed(&router, &id).await;
            assert_eq!(
                (status, status_of(&still_in_flight)),
                (200, "UPDATING"),
                "case {label:?} arrival {arrival}: expected the one reconfiguration to still be \
                 in flight, got {status} with {still_in_flight}"
            );
        }

        // One span after the FIRST arrival the feed is back in the status that
        // reconfiguration suspended, carrying the payload it stored exactly once.
        past_the_transition_span();
        let (status, settled) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&settled)),
            (200, case.held),
            "case {label:?}: expected the feed to settle back into {:?}, got {status} with \
             {settled}",
            case.held
        );
        assert_eq!(
            transition_due_at(&engine, &id, label),
            None,
            "case {label:?}: expected the settled feed to owe the clock nothing"
        );
        assert_eq!(
            settled["name"], case.payload["name"],
            "case {label:?}: expected the stored name {:?}, got {:?}",
            case.payload["name"], settled["name"]
        );
        assert_eq!(
            output_count(&settled),
            output_count(&case.payload),
            "case {label:?}: expected the stated output list to be stored, got {:?}",
            settled["outputs"]
        );
        assert_eq!(
            settled.get("accessRoleArn").cloned().unwrap_or(Value::Null),
            case.payload
                .get("accessRoleArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {label:?}: expected the stated access role to be the stored one, got {:?}",
            settled.get("accessRoleArn")
        );
    }
}

/// One row of the difference table: the payload the first update states and a second
/// payload differing from it in exactly one member.
struct DifferenceCase {
    label: &'static str,
    held: &'static str,
    first: Value,
    second: Value,
}

/// A row per member a `PUT` of a feed's payload states — the name, an output's name, its
/// config member, its own status, how many outputs there are, and the access role, both
/// changed and dropped — each over a feed in a settled status an update suspends.
fn difference_cases() -> Vec<DifferenceCase> {
    let base = json!({
        "name": "retried-update-first",
        "accessRoleArn": own_account_role("retried-update-first"),
        "outputs": [enabled_output("crop1", "cropping")]
    });
    let with = |member: &str, value: Value| -> Value {
        let mut payload = base.clone();
        payload
            .as_object_mut()
            .expect("an update payload is an object")
            .insert(member.to_string(), value);
        payload
    };
    let without = |member: &str| -> Value {
        let mut payload = base.clone();
        payload
            .as_object_mut()
            .expect("an update payload is an object")
            .remove(member);
        payload
    };
    vec![
        DifferenceCase {
            label: "another name",
            held: "AVAILABLE",
            first: base.clone(),
            second: with("name", json!("retried-update-second")),
        },
        DifferenceCase {
            label: "the output under another name",
            held: "AVAILABLE",
            first: base.clone(),
            second: with("outputs", json!([enabled_output("crop2", "cropping")])),
        },
        DifferenceCase {
            label: "the output carrying another config member",
            held: "ACTIVE",
            first: base.clone(),
            second: with("outputs", json!([enabled_output("crop1", "clipping")])),
        },
        DifferenceCase {
            label: "the output DISABLED",
            held: "AVAILABLE",
            first: base.clone(),
            second: with("outputs", json!([output("crop1", "cropping", "DISABLED")])),
        },
        DifferenceCase {
            label: "a second output",
            held: "ARCHIVED",
            first: base.clone(),
            second: with(
                "outputs",
                json!([
                    enabled_output("crop1", "cropping"),
                    subtitling_output("subs1", None)
                ]),
            ),
        },
        DifferenceCase {
            label: "another access role",
            held: "AVAILABLE",
            first: base.clone(),
            second: with(
                "accessRoleArn",
                json!(own_account_role("retried-update-second")),
            ),
        },
        DifferenceCase {
            label: "no access role at all",
            held: "ACTIVE",
            first: base.clone(),
            second: without("accessRoleArn"),
        },
    ]
}

/// An update stating a payload that differs in any ONE member is a reconfiguration of its
/// own: it is applied, and the feed's return to its settled status is due a fresh span
/// later — the recognition above pins a REPEAT, it does not stop the next request from
/// starting work.
#[tokio::test]
async fn an_update_differing_in_any_one_member_starts_its_own_span() {
    for case in difference_cases() {
        let label = case.label;
        let (router, engine) = router_with_engine();
        let id = feed_in_status(&router, case.held, label).await;

        let (status, first) = update_feed(&router, &id, &case.first).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first update answers 200, got {status} with {first}"
        );
        let first_due = owed_move(&engine, &id, label);

        let (status, second) = update_feed(&router, &id, &case.second).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the differing update answers 200, got {status} with {second}"
        );
        let second_due = owed_move(&engine, &id, label);
        assert!(
            second_due > first_due,
            "case {label:?}: expected the differing payload to start its own span, got the \
             first call's due instant again"
        );
        let (status, in_flight) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&in_flight)),
            (200, "UPDATING"),
            "case {label:?}: that reconfiguration is in flight, got {status} with {in_flight}"
        );

        past_the_transition_span();
        let (status, settled) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&settled)),
            (200, case.held),
            "case {label:?}: expected the feed to settle back into {:?}, got {status} with \
             {settled}",
            case.held
        );
        assert_eq!(
            settled["name"], case.second["name"],
            "case {label:?}: expected the second payload's name {:?} to be the stored one, got \
             {:?}",
            case.second["name"], settled["name"]
        );
        assert_eq!(
            output_count(&settled),
            output_count(&case.second),
            "case {label:?}: expected the second payload's output list to be stored, got {:?}",
            settled["outputs"]
        );
        assert_eq!(
            settled.get("accessRoleArn").cloned().unwrap_or(Value::Null),
            case.second
                .get("accessRoleArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {label:?}: expected the second payload's access role to be the stored one, \
             got {:?}",
            settled.get("accessRoleArn")
        );
    }
}
