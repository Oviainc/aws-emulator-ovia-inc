// SPDX-License-Identifier: CC0-1.0

//! What `DeleteFeedPolicy` does, end to end over the wire, read back through
//! `GetFeedPolicy` and `GetFeed`.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a delete DETACHES the feed's one policy document, so the grant it carried is gone
//!   and `GetFeedPolicy` has nothing left to report (HTTP 404) — whatever that document's
//!   size, nesting, statement count or alphabet;
//! * the operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="DeleteFeedPolicy")`), so a delete of a
//!   feed that holds no policy — a feed that never had one, and the same delete repeated
//!   — answers the modelled 204 again rather than refusing the state the first call left,
//!   and a later `PutFeedPolicy` attaches a document again;
//! * the delete touches the POLICY alone: the whole `GetFeed` record is identical before
//!   and after, and the policy of every other feed is untouched;
//! * the policy is a resource under the feed (`/v1/feed/{id}/policy`), so an id no live
//!   feed of the endpoint has — an unknown id, and a feed whose deletion has completed —
//!   is `ResourceNotFoundException` (HTTP 404) with this service's one fixed not-found
//!   message, and nothing is written.

use crate::harness::{call, enabled_output, past_the_transition_span, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::NOT_FOUND_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

/// `PolicyDocument`'s modelled maximum length
/// (`smithy_query(query="operation",operation="PutFeedPolicy")`: `policy` has
/// `length 1..2048`).
const POLICY_DOCUMENT_MAX_LEN: usize = 2048;

/// A policy document of exactly `len` characters: one JSON object whose `Sid` is padded
/// out to the requested total.
fn policy_of_length(len: usize) -> String {
    // `{"Sid":"<pad>"}` — ten characters of envelope around the padding.
    let envelope = r#"{"Sid":""}"#.len();
    format!(r#"{{"Sid":"{}"}}"#, "s".repeat(len - envelope))
}

/// Policy documents a delete has to detach, differing along what the rule generalises
/// over: size, statement count, nesting, alphabet, and the member's published upper
/// bound.
fn policy_documents() -> Vec<(&'static str, String)> {
    vec![
        ("the smallest JSON object", "{}".to_string()),
        (
            "one statement",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": "elemental-inference:GetFeed",
                    "Resource": "*"
                }]
            })
            .to_string(),
        ),
        (
            "two statements, so a multi-statement grant goes in one call",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Sid": "read", "Effect": "Allow",
                     "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                     "Action": ["elemental-inference:GetFeed", "elemental-inference:ListFeeds"],
                     "Resource": "*"},
                    {"Sid": "deny", "Effect": "Deny",
                     "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                     "Action": "elemental-inference:DeleteFeed",
                     "Resource": "*"}
                ]
            })
            .to_string(),
        ),
        (
            "a document whose text is not ASCII",
            json!({"Sid": "政策-ünïcode-☂", "Statement": []}).to_string(),
        ),
        (
            "a document at the member's modelled @length maximum",
            policy_of_length(POLICY_DOCUMENT_MAX_LEN),
        ),
    ]
}

/// `CreateFeed` with `body`, returning the created feed's id.
async fn create_feed_from(router: &MultiServiceRouter, label: &str, body: &Value) -> String {
    let (status, created) = call(router, "POST", "/v1/feed", Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreateFeed answers the modelled 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// One feed with one enabled cropping output, named for the case under test.
async fn feed(router: &MultiServiceRouter, label: &str) -> String {
    create_feed_from(
        router,
        label,
        &json!({"name": "delete-policy-feed", "outputs": [enabled_output("crop1", "cropping")]}),
    )
    .await
}

/// `PUT /v1/feed/{id}/policy` with `document`, asserting it was stored.
async fn attach_policy(router: &MultiServiceRouter, label: &str, id: &str, document: &str) {
    let (status, body) = call(
        router,
        "PUT",
        &format!("/v1/feed/{id}/policy"),
        Some(json!({"policy": document})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: PutFeedPolicy answers the modelled 200, got {status} with {body}"
    );
}

/// `DELETE /v1/feed/{id}/policy`.
async fn delete_policy(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/v1/feed/{id}/policy"), None).await
}

/// `GET /v1/feed/{id}/policy`.
async fn get_policy(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}/policy"), None).await
}

/// `GET /v1/feed/{id}`, asserting it is served, and returning the whole feed record as a
/// client sees it.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, record) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {record}"
    );
    record
}

/// `DeleteFeedPolicy` models no response members and a 204, so a served delete answers
/// that status and an empty payload.
fn assert_empty_success(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 204,
        "case {label:?}: DeleteFeedPolicy answers the modelled 204, got {status} with {body}"
    );
    let members = match body {
        Value::Null => 0,
        Value::Object(members) => members.len(),
        other => panic!("case {label:?}: the response payload is an object or empty, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {label:?}: DeleteFeedPolicyResponse models no members, got {body}"
    );
}

/// The modelled not-found refusal, as this service answers it for an id that names no
/// live resource of the endpoint it was sent to.
fn assert_not_found(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {label:?}: expected the modelled 404, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case {label:?}: that 404 is a ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(NOT_FOUND_MESSAGE),
        "case {label:?}: the not-found message is fixed and names no id, got {body}"
    );
}

/// A delete detaches whatever document the feed held, and a put after it attaches a
/// document again — the feed carries at most one policy either way.
#[tokio::test]
async fn a_delete_detaches_the_stored_policy_whatever_the_document() {
    let router = router();
    let id = feed(&router, "detach-every-document").await;

    for (label, document) in policy_documents() {
        attach_policy(&router, label, &id, &document).await;

        let (status, got) = get_policy(&router, &id).await;
        assert_eq!(
            (status, &got["policy"]),
            (200, &json!(document)),
            "case {label:?}: the document is attached before the delete, got {status} with {got}"
        );

        let (status, answered) = delete_policy(&router, &id).await;
        assert_empty_success(label, status, &answered);

        // Read twice: the detach is what a later caller sees, and a read changes nothing.
        for read in 1..=2 {
            let (status, error) = get_policy(&router, &id).await;
            assert_not_found(
                &format!("{label}, read {read} after the delete"),
                status,
                &error,
            );
        }
    }
}

/// `@idempotent`: a delete of a feed that holds no policy is the same served 204 —
/// whether the feed never had one or an earlier delete took it — and a put afterwards
/// still attaches one.
#[tokio::test]
async fn a_delete_of_a_feed_holding_no_policy_is_still_accepted() {
    // Repeat counts spanning one call, two, and a run of them.
    for repeats in [1_usize, 2, 7] {
        let router = router();
        let never_attached = format!("a feed that never held a policy, {repeats} deletes");
        let id = feed(&router, &never_attached).await;

        for round in 1..=repeats {
            let (status, answered) = delete_policy(&router, &id).await;
            assert_empty_success(
                &format!("{never_attached}, round {round}"),
                status,
                &answered,
            );
        }

        // The repeated delete left the feed able to take a policy, and one delete after
        // that is just as accepted.
        let document = json!({"Sid": "attached-after-the-deletes"}).to_string();
        attach_policy(&router, &never_attached, &id, &document).await;
        let (status, got) = get_policy(&router, &id).await;
        assert_eq!(
            (status, &got["policy"]),
            (200, &json!(document)),
            "case {never_attached:?}: a put after the deletes attaches a document again, got \
             {status} with {got}"
        );

        let attached = format!("a feed whose policy was deleted, {repeats} deletes");
        for round in 1..=repeats {
            let (status, answered) = delete_policy(&router, &id).await;
            assert_empty_success(&format!("{attached}, round {round}"), status, &answered);
            let (status, error) = get_policy(&router, &id).await;
            assert_not_found(&format!("{attached}, round {round}"), status, &error);
        }
    }
}

/// `CreateFeed` bodies differing along what a feed record can carry: how many outputs and
/// of which `OutputConfig` kind, the optional `accessRoleArn`, and tags. A delete must
/// leave every one of those as it found it.
fn feed_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one cropping output and no optional member",
            json!({"name": "scope-minimal", "outputs": [enabled_output("crop1", "cropping")]}),
        ),
        (
            // One of each member the `OutputConfig` union models
            // (`smithy_query(query="shape",shape="OutputConfig")`).
            "four outputs, one of each OutputConfig kind",
            json!({"name": "scope-every-output-kind", "outputs": [
                enabled_output("crop1", "cropping"),
                enabled_output("clip1", "clipping"),
                enabled_output("sub1", "subtitling"),
                enabled_output("meta1", "contextualMetadata"),
            ]}),
        ),
        (
            // `accessRoleArn` satisfies the modelled
            // `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$` and its `length 32..255`, and
            // names a role of the emulator's own account.
            "an accessRoleArn and tags",
            json!({
                "name": "scope-role-and-tags",
                "accessRoleArn": format!(
                    "arn:aws:iam::{}:role/elemental-inference-ingest",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "outputs": [enabled_output("clip1", "clipping")],
                "tags": {"team": "ingest", "env": "test"}
            }),
        ),
    ]
}

/// A delete clears the policy and nothing else: whatever the feed carries, its `GetFeed`
/// record is untouched by detaching a document.
#[tokio::test]
async fn a_delete_clears_the_policy_and_nothing_else_on_the_feed() {
    let router = router();

    for (label, body) in feed_bodies() {
        let id = create_feed_from(&router, label, &body).await;
        let before = get_feed(&router, &id, label).await;

        attach_policy(&router, label, &id, r#"{"Sid":"about-to-be-detached"}"#).await;
        let (status, answered) = delete_policy(&router, &id).await;
        assert_empty_success(label, status, &answered);

        let after = get_feed(&router, &id, label).await;
        assert_eq!(
            after, before,
            "case {label:?}: a delete must leave the feed record untouched, expected {before}, \
             got {after}"
        );
    }
}

/// The policy belongs to the feed it was attached to, so a delete detaches THAT feed's
/// document and leaves every other feed's in place.
#[tokio::test]
async fn a_delete_detaches_only_the_named_feeds_policy() {
    let router = router();

    // Three feeds, so the deleted one has a neighbour on either side of it.
    let mut held: Vec<(String, String)> = Vec::new();
    for index in 0..3 {
        let label = format!("feed {index}");
        let id = feed(&router, &label).await;
        let document = json!({"Sid": format!("policy-of-feed-{index}")}).to_string();
        attach_policy(&router, &label, &id, &document).await;
        held.push((id, document));
    }

    for (index, (id, _)) in held.iter().enumerate() {
        let label = format!("deleting the policy of feed {index}");
        let (status, answered) = delete_policy(&router, id).await;
        assert_empty_success(&label, status, &answered);

        for (other, (other_id, document)) in held.iter().enumerate() {
            let case = format!("{label}, then reading feed {other}");
            let (status, got) = get_policy(&router, other_id).await;
            if other <= index {
                // Its policy has already been deleted in an earlier round.
                assert_not_found(&case, status, &got);
            } else {
                assert_eq!(
                    (status, &got["policy"]),
                    (200, &json!(document)),
                    "case {case:?}: this feed keeps its own document, got {status} with {got}"
                );
            }
        }
    }
}

/// One row per SETTLED feed status this service reports (`FeedStatus`: `AVAILABLE`,
/// `ACTIVE`, `ARCHIVED`, `DELETING`). A policy may be detached from a feed that exists in
/// any of them: `DeleteFeedPolicy` models no conflict error, and revoking the grant on a
/// feed whose deletion is already accepted runs with that deletion rather than against it.
const STATUSES: [&str; 4] = ["AVAILABLE", "ACTIVE", "ARCHIVED", "DELETING"];

/// Create a feed, attach `document` to it, then drive it into `status_under_test` — the
/// policy is attached first so the `DELETING` row has one to detach (a put onto a feed
/// being deleted is the modelled conflict).
async fn feed_in_status(
    router: &MultiServiceRouter,
    status_under_test: &str,
    document: &str,
) -> String {
    let id = feed(router, status_under_test).await;
    attach_policy(router, status_under_test, &id, document).await;
    match status_under_test {
        // A created feed is settled and AVAILABLE at once.
        "AVAILABLE" => {}
        "ACTIVE" => {
            let (status, body) = call(
                router,
                "POST",
                &format!("/v1/feed/{id}/associate"),
                Some(json!({"associatedResourceName": "source-media", "outputs": []})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {status_under_test:?}: the claim is accepted, got {status} with {body}"
            );
        }
        "ARCHIVED" => {
            for verb in ["associate", "disassociate"] {
                let (status, body) = call(
                    router,
                    "POST",
                    &format!("/v1/feed/{id}/{verb}"),
                    Some(json!({"associatedResourceName": "released-media", "outputs": []})),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {status_under_test:?}: {verb} is accepted, got {status} with {body}"
                );
            }
        }
        "DELETING" => {
            let (status, body) = call(router, "DELETE", &format!("/v1/feed/{id}"), None).await;
            assert_eq!(
                status, 202,
                "case {status_under_test:?}: DeleteFeed is accepted, got {status} with {body}"
            );
        }
        other => panic!("no such status row: {other}"),
    }
    id
}

/// A delete is taken from every status that holds a feed, and moves no status.
#[tokio::test]
async fn a_delete_is_taken_from_every_status_that_holds_a_feed() {
    for status_under_test in STATUSES {
        // One router per row: a row's own setup is the only thing that has moved the
        // clock for the feed it deletes from.
        let router = router();
        let document = json!({"Sid": "attached-in-this-status"}).to_string();
        let id = feed_in_status(&router, status_under_test, &document).await;

        let (status, answered) = delete_policy(&router, &id).await;
        assert_empty_success(status_under_test, status, &answered);

        let (status, error) = get_policy(&router, &id).await;
        assert_not_found(status_under_test, status, &error);

        let record = get_feed(&router, &id, status_under_test).await;
        assert_eq!(
            record["status"],
            json!(status_under_test),
            "case {status_under_test:?}: a policy delete moves no status, got {record}"
        );
    }
}

/// An id no live feed of this endpoint has is the modelled not-found refusal, and the
/// repeated call answers the same.
#[tokio::test]
async fn an_id_no_live_feed_has_is_not_found() {
    let router = router();

    // The feed whose deletion has COMPLETED: the resource is gone, so its policy path
    // answers as any absent feed's does.
    let removed = feed(&router, "a feed that is then removed").await;
    attach_policy(
        &router,
        "a feed that is then removed",
        &removed,
        r#"{"Sid":"goes-with-the-feed"}"#,
    )
    .await;
    let (status, body) = call(&router, "DELETE", &format!("/v1/feed/{removed}"), None).await;
    assert_eq!(
        status, 202,
        "DeleteFeed is accepted, got {status} with {body}"
    );
    past_the_transition_span();

    // Ids that satisfy `FeedId`'s `^[a-z0-9]{19}$`, so the boundary admits them and the
    // handler is what refuses them; none of them names a live feed.
    let mut cases = vec![(
        "the feed whose deletion has completed".to_string(),
        removed.clone(),
    )];
    for id in [
        "0000000000000000000",
        "zzzzzzzzzzzzzzzzzzz",
        "abcdefghij123456789",
    ] {
        assert_ne!(id, removed, "the unknown ids must not name the removed feed");
        cases.push((format!("the unknown id {id}"), id.to_string()));
    }

    for (label, id) in cases {
        for round in 1..=2 {
            let (status, error) = delete_policy(&router, &id).await;
            assert_not_found(&format!("{label}, round {round}"), status, &error);
        }
    }
}
