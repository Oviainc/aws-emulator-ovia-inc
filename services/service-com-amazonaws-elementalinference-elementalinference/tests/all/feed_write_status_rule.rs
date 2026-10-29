// SPDX-License-Identifier: CC0-1.0

//! What a write to a feed reports about the feed's lifecycle status, end to end over the
//! wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: the status a write reports is decided by whether the call
//! ACCEPTED work or COMPLETED it, which its modelled success status says.
//!
//! * `CreateFeed` answers HTTP 202 — accepted, not completed — so it reports the status
//!   the feed was accepted in, `CREATING`: "after creation of the feed has succeeded, the
//!   status will be AVAILABLE" (`CreateFeedResponse$status`), so `AVAILABLE` is what the
//!   feed becomes once the accepted creation is done and not what the accept itself
//!   reports. The creation is complete by the time the caller's next request arrives, so
//!   every read from then on reports the settled `AVAILABLE` and the feed takes an
//!   association straight away.
//! * `UpdateFeed` answers HTTP 200 — complete when it answers — so it reports the feed's
//!   SETTLED status: `AVAILABLE` with no resource associated, `ACTIVE` while one holds it,
//!   `ARCHIVED` once its resource has been released. A reconfiguration suspends that
//!   status for the span of the work the service then carries out, which is what a client
//!   polling the READ path observes; it is not what the write hands back.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! how many outputs the feed carries and which `OutputConfig` member each one is, whether
//! a role and tags are supplied, the length of the name, and (for the update) which
//! settled status the feed is in when the update arrives.

use crate::harness::{
    call, enabled_output, past_the_transition_span, router, subtitling_output, PAGE_SIZE,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `status` member of a response, as text.
fn status_of(body: &Value) -> &str {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the response carries a status, got {body}"))
}

/// `GET /v1/feed/{id}`.
async fn get_feed(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}"), None).await
}

/// The status `ListFeeds` reports for `id`, so the rule is asserted on the listing
/// projection too.
async fn listed_status(router: &MultiServiceRouter, id: &str, case: &str) -> String {
    let (status, listed) = call(
        router,
        "GET",
        &format!("/v1/feeds?maxResults={PAGE_SIZE}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListFeeds answers 200, got {status} with {listed}"
    );
    listed["feeds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListFeeds reports a page, got {listed}"))
        .iter()
        .find(|summary| summary["id"] == json!(id))
        .map(|summary| status_of(summary).to_string())
        .unwrap_or_else(|| panic!("case {case:?}: the feed is listed, got {listed}"))
}

/// The create bodies the rules are asserted over: rows differing in output count and
/// `OutputConfig` member, in whether a role and tags are supplied, and in the length of
/// the name.
fn create_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one cropping output, no role, no tags",
            json!({
                "name": "accepted-feed",
                "outputs": [enabled_output("crop1", "cropping")]
            }),
        ),
        (
            "a one-character name and a subtitling output",
            json!({
                "name": "a",
                "outputs": [subtitling_output("subs1", None)]
            }),
        ),
        (
            "three outputs with a role and tags",
            json!({
                "name": "Accepted_Feed-3",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("clip1", "clipping"),
                    enabled_output("meta1", "contextualMetadata")
                ],
                "tags": {"env": "test"}
            }),
        ),
    ]
}

/// A create is ACCEPTED in `CREATING`, and the creation it accepted is complete by the
/// next request: every read from then on reports the settled `AVAILABLE`, however long
/// the caller waits.
#[tokio::test]
async fn a_create_is_accepted_in_creating_and_has_settled_by_the_next_request() {
    for (case, body) in create_bodies() {
        let router = router();
        let (status, created) = call(&router, "POST", "/v1/feed", Some(body)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        assert_eq!(
            status_of(&created),
            "CREATING",
            "case {case:?}: the accept reports the status the feed was accepted in, got \
             {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports an id, got {created}"))
            .to_string();

        // The very next request, and the listing projection with it.
        let (status, fetched) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&fetched)),
            (200, "AVAILABLE"),
            "case {case:?}: the next request reports the settled status, got {status} with \
             {fetched}"
        );
        assert_eq!(
            listed_status(&router, &id, case).await,
            "AVAILABLE",
            "case {case:?}: ListFeeds reports the settled status too"
        );

        // And it stays settled: the accepted creation is the only move the clock owed.
        past_the_transition_span();
        let (status, later) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&later)),
            (200, "AVAILABLE"),
            "case {case:?}: the settled status does not move afterwards, got {status} with \
             {later}"
        );

        // A feed whose creation has completed is serviceable: the association a caller
        // issues next is accepted.
        let (status, claimed) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/associate"),
            Some(json!({
                "associatedResourceName": "source-media",
                "outputs": [enabled_output("assoc1", "contextualMetadata")]
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created feed takes an association, got {status} with {claimed}"
        );
    }
}

/// An update COMPLETES when it answers 200, so its own answer reports the feed's settled
/// status — never the transient one the reconfiguration it starts puts the feed in for
/// the reads.
#[tokio::test]
async fn an_update_answers_the_feeds_settled_status() {
    // One row per settled status a feed can be in when the update arrives, each with a
    // replacement payload of its own.
    for (settled, replacement) in [
        (
            "AVAILABLE",
            json!({
                "name": "updated-unheld-feed",
                "outputs": [enabled_output("clip1", "clipping")]
            }),
        ),
        (
            "ACTIVE",
            json!({
                "name": "updated-held-feed",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    subtitling_output("subs1", None)
                ]
            }),
        ),
        (
            "ARCHIVED",
            json!({
                "name": "updated-released-feed",
                "outputs": [enabled_output("meta1", "contextualMetadata")]
            }),
        ),
    ] {
        let case = format!("an update of a {settled} feed");
        let router = router();
        let (status, created) = call(
            &router,
            "POST",
            "/v1/feed",
            Some(json!({
                "name": "settled-feed",
                "outputs": [enabled_output("crop1", "cropping")]
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();

        if settled != "AVAILABLE" {
            let (status, claimed) = call(
                &router,
                "POST",
                &format!("/v1/feed/{id}/associate"),
                Some(json!({
                    "associatedResourceName": "held-media",
                    "outputs": [enabled_output("assoc1", "contextualMetadata")]
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the claim is accepted, got {status} with {claimed}"
            );
        }
        if settled == "ARCHIVED" {
            let (status, released) = call(
                &router,
                "POST",
                &format!("/v1/feed/{id}/disassociate"),
                Some(json!({"associatedResourceName": "held-media"})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the release is accepted, got {status} with {released}"
            );
        }
        let (status, before) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&before)),
            (200, settled),
            "case {case:?}: the feed starts in the status under test, got {status} with {before}"
        );

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(replacement.clone()),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, settled),
            "case {case:?}: the 200 reports the settled status, got {status} with {updated}"
        );
        // The write itself is in that same answer, so the settled status is not a stale
        // copy of the record from before the write.
        assert_eq!(
            updated["name"], replacement["name"],
            "case {case:?}: the answer carries the replacement name, got {:?}",
            updated["name"]
        );
        assert_eq!(
            updated["outputs"].as_array().map(Vec::len),
            replacement["outputs"].as_array().map(Vec::len),
            "case {case:?}: the answer carries the replacement outputs, got {:?}",
            updated["outputs"]
        );

        // And once the reconfiguration the update started has fallen due, the feed is in
        // exactly the status its answer reported.
        past_the_transition_span();
        let (status, after) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&after)),
            (200, settled),
            "case {case:?}: the feed settles into the status the update answered with, got \
             {status} with {after}"
        );
    }
}
