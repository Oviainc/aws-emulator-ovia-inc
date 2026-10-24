// SPDX-License-Identifier: CC0-1.0

//! What `PutFeedPolicy` WRITES, end to end over the wire, read back through `GetFeed`
//! and `GetFeedPolicy`.
//!
//! `feed_policy.rs` pins the document a put stores and the refusals a put earns. This
//! suite pins the other half of the same operation — the EXTENT of its write:
//!
//! * a put sets the feed's one policy document and touches NOTHING else the feed record
//!   holds: the `GetFeed` response (arn, name, id, dataEndpoints, outputs, accessRoleArn,
//!   status, association, tags) is identical before and after, whatever the feed carries;
//! * the operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="PutFeedPolicy")`), so the SAME put
//!   repeated any number of times answers the same success and leaves the same single
//!   document — a put neither accumulates documents nor counts calls;
//! * a feed takes a policy from every status this service reports except a pending
//!   deletion: `AVAILABLE`, `ACTIVE` and `ARCHIVED` accept it and keep the status they
//!   had, while a feed whose deletion has been accepted is the modelled
//!   `ConflictException` (HTTP 409) and stores nothing.

use crate::harness::{call, enabled_output, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    FEED_BEING_DELETED_MESSAGE, NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// `CreateFeed` bodies differing along what a feed record can carry: how many outputs
/// and of which `OutputConfig` kind, the optional `accessRoleArn`, and tags. A put must
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
            // names a role of the emulator's own account (a role of another account is
            // a cross-account pass role, which `CreateFeed` refuses).
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

/// Policy documents a put stores, in the order a test attaches them: the second
/// REPLACES the first, so a suite that puts both sees a replacement as well as a first
/// attach.
fn documents() -> Vec<(&'static str, String)> {
    vec![
        ("the first document", json!({"Sid": "first"}).to_string()),
        (
            "a second, larger document that replaces it",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "cross-account-read",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": ["elemental-inference:GetFeed"],
                    "Resource": "*"
                }]
            })
            .to_string(),
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

/// `PUT /v1/feed/{id}/policy` with `document`.
async fn put_policy(router: &MultiServiceRouter, id: &str, document: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v1/feed/{id}/policy"),
        Some(json!({"policy": document})),
    )
    .await
}

/// `GET /v1/feed/{id}/policy`.
async fn get_policy(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}/policy"), None).await
}

/// `GET /v1/feed/{id}`, asserting it is served, and returning the whole feed record as a
/// client sees it.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// `PutFeedPolicyResponse` models no members, so a successful put answers 200 and an
/// empty payload — nothing of the feed is reported back on the policy path.
fn assert_empty_success(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {label:?}: PutFeedPolicy answers the modelled 200, got {status} with {body}"
    );
    let members = match body {
        Value::Null => 0,
        Value::Object(members) => members.len(),
        other => panic!("case {label:?}: the response payload is an object or empty, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {label:?}: PutFeedPolicyResponse models no members, got {body}"
    );
}

/// A put sets the policy and nothing else: whatever the feed carries, its `GetFeed`
/// record is untouched by attaching a document and by replacing one.
#[tokio::test]
async fn a_put_writes_the_policy_and_nothing_else_on_the_feed() {
    let router = router();

    for (feed_label, body) in feed_bodies() {
        let id = create_feed_from(&router, feed_label, &body).await;
        let before = get_feed(&router, &id, feed_label).await;

        for (document_label, document) in documents() {
            let label = format!("{feed_label}, putting {document_label}");
            let (status, answered) = put_policy(&router, &id, &document).await;
            assert_empty_success(&label, status, &answered);

            let (status, got) = get_policy(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the attached policy is reported, got {status} with {got}"
            );
            assert_eq!(
                got["policy"],
                json!(document),
                "case {label:?}: the last document put is the one reported, got {got}"
            );

            let after = get_feed(&router, &id, &label).await;
            assert_eq!(
                after, before,
                "case {label:?}: a put must leave the feed record untouched, expected \
                 {before}, got {after}"
            );
        }
    }
}

/// `@idempotent`: the same put repeated is the same single document and the same
/// success, however many times it is sent.
#[tokio::test]
async fn the_same_put_repeated_leaves_one_policy_and_one_success() {
    let router = router();

    // Repeat counts spanning one call, two, and a run of them.
    for repeats in [1_usize, 2, 7] {
        let feed_label = format!("repeated {repeats} times");
        let id = create_feed_from(
            &router,
            &feed_label,
            &json!({"name": "idempotent-put", "outputs": [enabled_output("crop1", "cropping")]}),
        )
        .await;
        let before = get_feed(&router, &id, &feed_label).await;
        let document = json!({"Sid": "the-one-document", "Statement": []}).to_string();

        for round in 1..=repeats {
            let label = format!("{feed_label}, round {round}");
            let (status, answered) = put_policy(&router, &id, &document).await;
            assert_empty_success(&label, status, &answered);

            let (status, got) = get_policy(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the policy is reported, got {status} with {got}"
            );
            assert_eq!(
                got["policy"],
                json!(document),
                "case {label:?}: one document, not one per call, got {got}"
            );
            assert_eq!(
                got.as_object().map(serde_json::Map::len),
                Some(1),
                "case {label:?}: GetFeedPolicyResponse reports the one policy member, got {got}"
            );
        }

        let after = get_feed(&router, &id, &feed_label).await;
        assert_eq!(
            after, before,
            "case {feed_label:?}: repeating the put moved nothing on the feed, expected \
             {before}, got {after}"
        );
    }
}

/// One row per feed status this service reports (`FeedStatus`: `AVAILABLE`, `ACTIVE`,
/// `DELETING`, `ARCHIVED`), with whether a put is accepted from it. A policy may be
/// attached to a feed that exists; only a feed on its way out of the store refuses.
/// (The `DELETING` row is here so the table spans the whole status dimension; the
/// refusal's own wording is pinned in `feed_policy.rs`.)
const STATUS_ROWS: [(&str, bool); 4] = [
    ("AVAILABLE", true),
    ("ACTIVE", true),
    ("ARCHIVED", true),
    ("DELETING", false),
];

/// Drive a freshly created feed into `status_under_test` and return its id.
async fn feed_in_status(router: &MultiServiceRouter, status_under_test: &str) -> String {
    let id = create_feed_from(
        router,
        status_under_test,
        &json!({"name": "policy-status-feed", "outputs": [enabled_output("crop1", "cropping")]}),
    )
    .await;
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

/// A put is taken from every status that holds a feed, and refused only while the feed's
/// deletion is pending — and the feed keeps the status it had either way.
#[tokio::test]
async fn a_put_is_taken_from_every_status_but_a_pending_deletion() {
    for (status_under_test, accepted) in STATUS_ROWS {
        // One router per row: a row's own setup is the only thing that has moved the
        // clock for the feed it puts to.
        let router = router();
        let id = feed_in_status(&router, status_under_test).await;
        let document = json!({"Sid": "attached-in-this-status"}).to_string();

        let (status, answered) = put_policy(&router, &id, &document).await;
        if accepted {
            assert_empty_success(status_under_test, status, &answered);

            let (status, got) = get_policy(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {status_under_test:?}: the stored policy is reported, got {status} with \
                 {got}"
            );
            assert_eq!(
                got["policy"],
                json!(document),
                "case {status_under_test:?}: the document put in this status is the one \
                 reported, got {got}"
            );

            let feed = get_feed(&router, &id, status_under_test).await;
            assert_eq!(
                feed["status"],
                json!(status_under_test),
                "case {status_under_test:?}: a put moves no status, got {feed}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {status_under_test:?}: expected the modelled 409, got {status} with \
                 {answered}"
            );
            assert_eq!(
                answered["__type"],
                json!("ConflictException"),
                "case {status_under_test:?}: that 409 is a ConflictException, got {answered}"
            );
            assert_eq!(
                answered["message"],
                json!(FEED_BEING_DELETED_MESSAGE),
                "case {status_under_test:?}: the refusal states the rule that refused it, got \
                 {answered}"
            );

            let (status, error) = get_policy(&router, &id).await;
            assert_eq!(
                status, 404,
                "case {status_under_test:?}: the refused put stored nothing, got {status} with \
                 {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {status_under_test:?}: the refusal carries the fixed not-found message, \
                 got {error}"
            );

            let feed = get_feed(&router, &id, status_under_test).await;
            assert_eq!(
                feed["status"],
                json!(status_under_test),
                "case {status_under_test:?}: the refused put moved no status, got {feed}"
            );
        }
    }
}
