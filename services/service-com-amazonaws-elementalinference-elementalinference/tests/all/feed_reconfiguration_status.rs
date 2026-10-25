// SPDX-License-Identifier: CC0-1.0

//! A feed being reconfigured in place reports the transient status its own status enum
//! models for that work, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: an operation that reconfigures a stored resource's payload in
//! place leaves the resource in the transient status its status enum models for that
//! work, for a non-zero span, and the clock then settles it back into the status the
//! work SUSPENDED — the status is suspended, not replaced. For this service the
//! operation is `UpdateFeed` ("the payload that you specify completely overwrites the
//! existing payload") and the modelled value is `UPDATING`
//! (`smithy_query(query="shape",shape="FeedStatus")`: `CREATING | AVAILABLE | ACTIVE |
//! UPDATING | DELETING | DELETED | ARCHIVED`), so
//!
//! * `UpdateFeed` answers HTTP 200 reporting `UPDATING`, and every reader of the feed
//!   (`GetFeed`, `ListFeeds`) reports `UPDATING` while the reconfiguration is in flight,
//!   which is what a client polling after an update waits on;
//! * once the span falls due the feed is back in the status it was reconfigured from —
//!   `AVAILABLE` with no resource associated, `ACTIVE` while one holds it, `ARCHIVED`
//!   once its resource has been released — carrying the payload the update stored;
//! * a reconfiguration arriving while another is in flight settles back to the same
//!   suspended status, never to the transient one;
//! * the in-flight status is a real one: a feed carrying it takes no association, and a
//!   removal accepted during the reconfiguration is still what the clock carries out.
//!
//! The rules are asserted over TABLES whose rows differ along what they generalise over
//! (which status the reconfiguration suspends, how many outputs the replacement list
//! carries and which `OutputConfig` member each one is, the length of the new name, and
//! whether the payload supplies or drops the access role), never over one recorded
//! request.

use crate::harness::{call, enabled_output, past_the_transition_span, router, subtitling_output};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::FEED_NOT_ASSOCIABLE_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

/// The settled statuses a feed can be reconfigured from — every status this service
/// reports for a feed that is not already on its way out of the store.
const SUSPENDABLE: [&str; 3] = ["AVAILABLE", "ACTIVE", "ARCHIVED"];

/// The resource name the `ACTIVE` and `ARCHIVED` rows claim the feed with.
const HELD_MEDIA: &str = "held-media";

/// `GET /v1/feed/{id}`.
async fn get_feed(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}"), None).await
}

/// `PUT /v1/feed/{id}` with `payload`.
async fn update_feed(router: &MultiServiceRouter, id: &str, payload: &Value) -> (u16, Value) {
    call(router, "PUT", &format!("/v1/feed/{id}"), Some(payload.clone())).await
}

/// `POST /v1/feed/{id}/associate` claiming `resource` and appending nothing.
async fn associate(router: &MultiServiceRouter, id: &str, resource: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({"associatedResourceName": resource, "outputs": []})),
    )
    .await
}

/// The `status` member of a response, as text.
fn status_of(body: &Value) -> &str {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the response carries a status, got {body}"))
}

/// The status `ListFeeds` reports for `id`, so a rule about a feed's status is asserted
/// on the listing projection too.
async fn listed_status(router: &MultiServiceRouter, id: &str, case: &str) -> String {
    let (status, listed) = call(router, "GET", "/v1/feeds", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListFeeds answers 200, got {status} with {listed}"
    );
    listed["feeds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListFeeds reports a feeds list, got {listed}"))
        .iter()
        .find(|summary| summary["id"] == json!(id))
        .map(|summary| status_of(summary).to_string())
        .unwrap_or_else(|| panic!("case {case:?}: the feed is listed, got {listed}"))
}

/// Create one feed and drive it into `held`, the settled status the reconfiguration
/// under test is to suspend; return its id.
async fn feed_in_status(router: &MultiServiceRouter, held: &str, case: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "reconfigured-feed",
            "accessRoleArn": "arn:aws:iam::000000000000:role/original",
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

    match held {
        // A created feed is settled and unassociated at once.
        "AVAILABLE" => {}
        "ACTIVE" => {
            let (status, claimed) = associate(router, &id, HELD_MEDIA).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the claim is accepted, got {status} with {claimed}"
            );
        }
        "ARCHIVED" => {
            let (status, claimed) = associate(router, &id, HELD_MEDIA).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the claim is accepted, got {status} with {claimed}"
            );
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
        other => panic!("case {case:?}: no such suspendable status: {other}"),
    }

    let (status, feed) = get_feed(router, &id).await;
    assert_eq!(
        (status, status_of(&feed)),
        (200, held),
        "case {case:?}: the feed starts in the status under test, got {status} with {feed}"
    );
    id
}

/// One row of the reconfiguration table: the status the update suspends, and the payload
/// that replaces the feed's.
struct ReconfigurationCase {
    label: &'static str,
    held: &'static str,
    replacement: Value,
}

/// The rows differ in the suspended status, in how many outputs the replacement carries
/// (one, two, three) and which `OutputConfig` member each one is, in the length of the
/// new name, and in whether the payload supplies the access role or drops it — and one
/// row replaces the payload with exactly what the feed already holds, which is a
/// reconfiguration like any other.
fn reconfiguration_cases() -> Vec<ReconfigurationCase> {
    vec![
        ReconfigurationCase {
            label: "an unassociated feed, one output of another kind and a one-character name",
            held: "AVAILABLE",
            replacement: json!({
                "name": "a",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [enabled_output("clip1", "clipping")]
            }),
        },
        ReconfigurationCase {
            label: "an unassociated feed grown to three outputs, with the role dropped",
            held: "AVAILABLE",
            replacement: json!({
                "name": "grown-feed",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("meta1", "contextualMetadata"),
                    subtitling_output("subs1", None)
                ]
            }),
        },
        ReconfigurationCase {
            label: "a feed a resource holds, shrunk to one subtitling output",
            held: "ACTIVE",
            replacement: json!({
                "name": "reconfigured-while-held",
                "outputs": [subtitling_output("subs1", None)]
            }),
        },
        ReconfigurationCase {
            label: "a feed a resource holds, written back with the payload it already has",
            held: "ACTIVE",
            replacement: json!({
                "name": "reconfigured-feed",
                "accessRoleArn": "arn:aws:iam::000000000000:role/original",
                "outputs": [enabled_output("crop1", "cropping")]
            }),
        },
        ReconfigurationCase {
            label: "a feed whose resource has been released, two outputs",
            held: "ARCHIVED",
            replacement: json!({
                "name": "reconfigured-after-release",
                "outputs": [
                    enabled_output("clip1", "clipping"),
                    enabled_output("meta1", "contextualMetadata")
                ]
            }),
        },
    ]
}

/// How many outputs a payload carries.
fn output_count(payload: &Value) -> Option<usize> {
    payload["outputs"].as_array().map(Vec::len)
}

/// An update reports `UPDATING` and so does every reader of the feed, until the clock
/// settles the feed back into the status the reconfiguration suspended — with the
/// payload the update stored.
#[tokio::test]
async fn an_update_reports_updating_and_settles_back_into_the_status_it_suspended() {
    for case in reconfiguration_cases() {
        let label = case.label;
        let router = router();
        let id = feed_in_status(&router, case.held, label).await;
        let before = get_feed(&router, &id).await.1;

        let (status, updated) = update_feed(&router, &id, &case.replacement).await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            status_of(&updated),
            "UPDATING",
            "case {label:?}: expected the reconfiguration to report UPDATING, got {updated}"
        );
        // The overwrite itself is in the response: the transient status is about the work
        // the service then carries out, not about the payload being unstored.
        assert_eq!(
            updated["name"], case.replacement["name"],
            "case {label:?}: expected the new name {:?}, got {:?}",
            case.replacement["name"], updated["name"]
        );
        assert_eq!(
            output_count(&updated),
            output_count(&case.replacement),
            "case {label:?}: expected the replacement list to be reported, got {:?}",
            updated["outputs"]
        );
        // The status is SUSPENDED, not replaced: what decides the status the feed goes
        // back into — the resource that holds it — is reported unchanged throughout.
        assert_eq!(
            updated.get("association").cloned().unwrap_or(Value::Null),
            before.get("association").cloned().unwrap_or(Value::Null),
            "case {label:?}: expected the association to be untouched, got {:?}",
            updated.get("association")
        );

        // A client polling straight after the update observes the transient status, on
        // the read path and on the listing path alike.
        let (status, in_flight) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&in_flight)),
            (200, "UPDATING"),
            "case {label:?}: expected GetFeed to report UPDATING, got {status} with {in_flight}"
        );
        assert_eq!(
            listed_status(&router, &id, label).await,
            "UPDATING",
            "case {label:?}: expected ListFeeds to report UPDATING for the feed"
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
            listed_status(&router, &id, label).await,
            case.held,
            "case {label:?}: expected ListFeeds to report {:?} once the feed settled",
            case.held
        );
        assert_eq!(
            settled["name"], case.replacement["name"],
            "case {label:?}: expected the stored name {:?}, got {:?}",
            case.replacement["name"], settled["name"]
        );
        assert_eq!(
            output_count(&settled),
            output_count(&case.replacement),
            "case {label:?}: expected the replacement list to be stored, got {:?}",
            settled["outputs"]
        );
        assert_eq!(
            settled.get("accessRoleArn").cloned().unwrap_or(Value::Null),
            case.replacement
                .get("accessRoleArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {label:?}: expected the PUT's access role to be the stored one, got {:?}",
            settled.get("accessRoleArn")
        );
    }
}

/// A reconfiguration that arrives while another is still in flight settles back into the
/// status the FIRST one suspended: the transient status is never the one the feed is left
/// in, however many updates pile up.
#[tokio::test]
async fn a_second_update_in_flight_still_settles_back_into_the_suspended_status() {
    for held in SUSPENDABLE {
        let case = format!("{held} reconfigured twice");
        let router = router();
        let id = feed_in_status(&router, held, &case).await;

        let first = json!({
            "name": "first-replacement",
            "outputs": [enabled_output("clip1", "clipping")]
        });
        let second = json!({
            "name": "second-replacement",
            "outputs": [
                enabled_output("crop1", "cropping"),
                subtitling_output("subs1", None)
            ]
        });
        for (pass, payload) in [("first", &first), ("second", &second)] {
            let (status, updated) = update_feed(&router, &id, payload).await;
            assert_eq!(
                (status, status_of(&updated)),
                (200, "UPDATING"),
                "case {case:?}: the {pass} update reports UPDATING, got {status} with {updated}"
            );
        }

        past_the_transition_span();
        let (status, settled) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&settled)),
            (200, held),
            "case {case:?}: expected the feed to settle back into {held:?}, got {status} with \
             {settled}"
        );
        assert_eq!(
            settled["name"],
            json!("second-replacement"),
            "case {case:?}: expected the last payload to be the stored one, got {:?}",
            settled["name"]
        );
        assert_eq!(
            output_count(&settled),
            output_count(&second),
            "case {case:?}: expected the last replacement list to be stored, got {:?}",
            settled["outputs"]
        );
    }
}

/// The in-flight status is a real one: a feed carrying it admits no association, and
/// admits one again as soon as it has settled.
#[tokio::test]
async fn a_feed_being_reconfigured_takes_no_association_until_it_has_settled() {
    // One row per status that admits an association at all: a feed with no resource yet,
    // and the resource that already holds one re-claiming it (the appending claim).
    for (held, resource) in [("AVAILABLE", "fresh-media"), ("ACTIVE", HELD_MEDIA)] {
        let case = format!("a claim on a {held} feed being reconfigured");
        let router = router();
        let id = feed_in_status(&router, held, &case).await;

        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({
                "name": "claimed-while-reconfigured",
                "outputs": [enabled_output("crop1", "cropping")]
            }),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, "UPDATING"),
            "case {case:?}: the update reports UPDATING, got {status} with {updated}"
        );

        let (status, refused) = associate(&router, &id, resource).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!(FEED_NOT_ASSOCIABLE_MESSAGE),
            "case {case:?}: expected the rule's invariant sentence, got {:?}",
            refused["message"]
        );

        past_the_transition_span();
        let (status, claimed) = associate(&router, &id, resource).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settled feed takes the claim, got {status} with {claimed}"
        );
        let (status, active) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&active)),
            (200, "ACTIVE"),
            "case {case:?}: and holds the resource afterwards, got {status} with {active}"
        );
        assert_eq!(
            active["association"],
            json!({"associatedResourceName": resource}),
            "case {case:?}: expected the claiming resource to be reported, got {active}"
        );
    }
}

/// A removal accepted while a reconfiguration is in flight is what the clock carries
/// out: the feed goes, rather than settling back into the status the update suspended.
#[tokio::test]
async fn a_removal_accepted_during_a_reconfiguration_is_what_the_clock_carries_out() {
    for held in SUSPENDABLE {
        let case = format!("{held} deleted while being reconfigured");
        let router = router();
        let id = feed_in_status(&router, held, &case).await;

        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({
                "name": "deleted-while-reconfigured",
                "outputs": [enabled_output("clip1", "clipping")]
            }),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, "UPDATING"),
            "case {case:?}: the update reports UPDATING, got {status} with {updated}"
        );

        let (status, deleted) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            (status, status_of(&deleted)),
            (202, "DELETING"),
            "case {case:?}: the delete is accepted, got {status} with {deleted}"
        );
        let (status, deleting) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&deleting)),
            (200, "DELETING"),
            "case {case:?}: the feed is on its way out, got {status} with {deleting}"
        );

        past_the_transition_span();
        let (status, gone) = get_feed(&router, &id).await;
        assert_eq!(
            (status, &gone["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {case:?}: expected the removal to have happened, got {status} with {gone}"
        );
    }
}

/// Attaching a policy is not a reconfiguration of the feed's payload, so it moves no
/// status: it is accepted on a feed that is being reconfigured, and that feed still
/// settles back into the status the update suspended.
#[tokio::test]
async fn attaching_a_policy_to_a_feed_being_reconfigured_moves_no_status() {
    for held in SUSPENDABLE {
        let case = format!("a policy put on a {held} feed being reconfigured");
        let router = router();
        let id = feed_in_status(&router, held, &case).await;

        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({
                "name": "policy-while-reconfigured",
                "outputs": [enabled_output("crop1", "cropping")]
            }),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, "UPDATING"),
            "case {case:?}: the update reports UPDATING, got {status} with {updated}"
        );

        let document = json!({"Sid": "attached-while-reconfigured"}).to_string();
        let (status, answered) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}/policy"),
            Some(json!({"policy": document})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutFeedPolicy answers 200, got {status} with {answered}"
        );
        let (status, in_flight) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&in_flight)),
            (200, "UPDATING"),
            "case {case:?}: the put moved no status, got {status} with {in_flight}"
        );

        past_the_transition_span();
        let (status, settled) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&settled)),
            (200, held),
            "case {case:?}: expected the feed to settle back into {held:?}, got {status} with \
             {settled}"
        );
        let (status, policy) = call(&router, "GET", &format!("/v1/feed/{id}/policy"), None).await;
        assert_eq!(
            (status, &policy["policy"]),
            (200, &json!(document)),
            "case {case:?}: expected the attached document to be reported, got {status} with \
             {policy}"
        );
    }
}
