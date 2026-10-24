// SPDX-License-Identifier: CC0-1.0

//! A feed's RESOURCE-BASED POLICY, over the wire: `PutFeedPolicy` attaches or replaces
//! it and `GetFeedPolicy` reports it.
//!
//! The rules under test, each asserted over a TABLE of documents rather than one case:
//!
//! * a feed holds AT MOST ONE policy document and reports back exactly the bytes the
//!   last put stored — whatever that document's size, nesting, statement count or
//!   alphabet, and a read never changes what a later read sees;
//! * a policy is a resource of its own under the feed (`/v1/feed/{id}/policy`) and
//!   `GetFeedPolicyResponse$policy` is required, so a feed with no policy attached and
//!   an id no live feed has are both `ResourceNotFoundException` (HTTP 404) with this
//!   service's one fixed not-found message;
//! * the policy lives on the feed, so it is gone once the feed is;
//! * a `policy` that is not a resource-based policy document (not one JSON object) is
//!   refused with the modelled `ValidationException` (HTTP 400) and stores nothing,
//!   while a document at the member's modelled `@length` maximum is accepted.

use crate::harness::{call, create_feed, past_the_transition_span, router};
use serde_json::json;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    FEED_BEING_DELETED_MESSAGE, INVALID_POLICY_DOCUMENT_MESSAGE, NOT_FOUND_MESSAGE,
};

/// `PolicyDocument`'s modelled maximum length
/// (`smithy_query(query="shape",shape="PolicyDocument")`: `length 1..2048`).
const POLICY_DOCUMENT_MAX_LEN: usize = 2048;

/// A policy document of exactly `len` characters: one JSON object whose `Sid` is padded
/// out to the requested total.
fn policy_of_length(len: usize) -> String {
    // `{"Sid":"<pad>"}` — ten characters of envelope around the padding.
    let envelope = r#"{"Sid":""}"#.len();
    assert!(
        len >= envelope,
        "a padded policy document is at least {envelope} characters, asked for {len}"
    );
    format!(r#"{{"Sid":"{}"}}"#, "s".repeat(len - envelope))
}

/// Policy documents that ARE resource-based policy documents, differing along what the
/// rule generalises over: size, statement count, nesting, alphabet, and the member's
/// published upper bound.
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
            "two statements, so order and count are carried through",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Sid": "second-is-a-deny", "Effect": "Allow",
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

/// Attach a policy and read it back; then attach the next one over it and read that.
/// One feed carries one policy, and a read reports the last document stored.
#[tokio::test]
async fn a_put_policy_is_read_back_verbatim_and_a_later_put_replaces_it() {
    let router = router();
    let feed_id = create_feed(&router, "policy-roundtrip").await;
    let uri = format!("/v1/feed/{feed_id}/policy");

    for (label, document) in policy_documents() {
        assert!(
            document.len() <= POLICY_DOCUMENT_MAX_LEN,
            "case {label:?}: the document must satisfy the modelled @length, got {} \
             characters",
            document.len()
        );

        let (status, body) = call(
            &router,
            "PUT",
            &uri,
            Some(json!({"policy": document.clone()})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutFeedPolicy answers the modelled 200, got {status} with {body}"
        );

        // Read twice: a report must not change what the next caller sees.
        for read in 1..=2 {
            let (status, got) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: read {read} reports the attached policy, got {status} with \
                 {got}"
            );
            assert_eq!(
                got["policy"],
                json!(document),
                "case {label:?}: read {read} must report the stored document verbatim, got {got}"
            );
        }
    }
}

/// A policy attached to one feed is not reported for another: the document is stored on
/// the feed it was attached to.
#[tokio::test]
async fn a_policy_belongs_to_the_feed_it_was_attached_to() {
    let router = router();
    let first = create_feed(&router, "policy-holder-one").await;
    let second = create_feed(&router, "policy-holder-two").await;

    let documents = [
        (first.clone(), json!({"Sid": "first"}).to_string()),
        (second.clone(), json!({"Sid": "second"}).to_string()),
    ];
    for (feed_id, document) in &documents {
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{feed_id}/policy"),
            Some(json!({"policy": document})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {feed_id:?}: the policy is attached, got {status} with {body}"
        );
    }
    for (feed_id, document) in &documents {
        let (status, got) = call(&router, "GET", &format!("/v1/feed/{feed_id}/policy"), None).await;
        assert_eq!(
            status, 200,
            "case {feed_id:?}: the feed reports its own policy, got {status} with {got}"
        );
        assert_eq!(
            got["policy"],
            json!(document),
            "case {feed_id:?}: expected this feed's own document, got {got}"
        );
    }
}

/// A feed that was never given a policy has none to report — and a live feed is not
/// distinguishable from an unknown id in that refusal.
#[tokio::test]
async fn a_feed_with_no_policy_and_an_unknown_feed_are_both_not_found() {
    let router = router();
    let live = create_feed(&router, "policy-less-feed").await;

    // Ids that satisfy `FeedId`'s `^[a-z0-9]{19}$`, so the boundary admits them and the
    // handler is what refuses them; none of them names a live feed.
    let unknown_ids = [
        "0000000000000000000",
        "zzzzzzzzzzzzzzzzzzz",
        "abcdefghij123456789",
    ];
    let mut cases: Vec<(String, String)> = vec![(
        "a live feed that holds no policy".to_string(),
        live.clone(),
    )];
    for id in unknown_ids {
        assert_ne!(id, live, "the unknown ids must not name the live feed");
        cases.push((format!("the unknown id {id}"), id.to_string()));
    }

    for (label, feed_id) in cases {
        let uri = format!("/v1/feed/{feed_id}/policy");
        for (method, body) in [
            ("GET", None),
            ("PUT", Some(json!({"policy": r#"{"Sid":"x"}"#}))),
        ] {
            // The live feed accepts a put (it is only the GET that has nothing to
            // report), so the put row applies to the unknown ids alone.
            if method == "PUT" && feed_id == live {
                continue;
            }
            let label = format!("{label}, on {method} {uri}");
            let (status, error) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected the modelled 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {label:?}: the not-found message is fixed and names no id, got {error}"
            );
        }
    }
}

/// The policy lives on the feed: while the feed's deletion is pending a put is refused,
/// and once the feed is gone its policy is gone with it.
#[tokio::test]
async fn a_deleted_feed_keeps_no_policy() {
    let router = router();
    let feed_id = create_feed(&router, "policy-then-deleted").await;
    let uri = format!("/v1/feed/{feed_id}/policy");
    let document = json!({"Sid": "attached-before-the-delete"}).to_string();

    let (status, body) = call(&router, "PUT", &uri, Some(json!({"policy": document}))).await;
    assert_eq!(
        status, 200,
        "the policy is attached before the delete, got {status} with {body}"
    );

    let (status, body) = call(&router, "DELETE", &format!("/v1/feed/{feed_id}"), None).await;
    assert_eq!(
        status, 202,
        "DeleteFeed is accepted, got {status} with {body}"
    );

    // The feed's removal is owed by the clock, so right now it is still DELETING: a put
    // onto a feed on its way out is the modelled conflict.
    let (status, error) = call(
        &router,
        "PUT",
        &uri,
        Some(json!({"policy": r#"{"Sid":"too-late"}"#})),
    )
    .await;
    assert_eq!(
        status, 409,
        "a put onto a feed being deleted is refused, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ConflictException"),
        "that 409 is a ConflictException, got {error}"
    );
    assert_eq!(
        error["message"],
        json!(FEED_BEING_DELETED_MESSAGE),
        "the conflict states the rule that refused it, got {error}"
    );

    past_the_transition_span();

    let (status, error) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 404,
        "a removed feed has no policy to report, got {status} with {error}"
    );
    assert_eq!(
        error["message"],
        json!(NOT_FOUND_MESSAGE),
        "the refusal carries the fixed not-found message, got {error}"
    );
}

/// Bodies that are NOT a resource-based policy document, differing along what the rule
/// generalises over: JSON of another type, text that is not JSON at all, and a truncated
/// object.
fn not_policy_documents() -> Vec<(&'static str, String)> {
    vec![
        ("plain text", "this is not a policy".to_string()),
        ("a JSON array of statements", "[{\"Effect\":\"Allow\"}]".to_string()),
        ("a JSON string", "\"Allow\"".to_string()),
        ("a JSON number", "42".to_string()),
        ("JSON null", "null".to_string()),
        ("a truncated object", "{\"Statement\": [".to_string()),
        ("two objects back to back", "{}{}".to_string()),
        // One character: inside `PolicyDocument`'s modelled `@length` minimum of 1, so
        // the boundary admits it and the handler is what refuses it.
        ("a single character", "x".to_string()),
    ]
}

/// A body that is not a policy document is refused, and the feed keeps the policy it
/// already had.
#[tokio::test]
async fn a_body_that_is_not_a_policy_document_is_refused_and_stores_nothing() {
    let router = router();
    let feed_id = create_feed(&router, "policy-validation").await;
    let uri = format!("/v1/feed/{feed_id}/policy");
    let held = json!({"Sid": "the-document-that-must-survive"}).to_string();

    let (status, body) = call(&router, "PUT", &uri, Some(json!({"policy": held}))).await;
    assert_eq!(
        status, 200,
        "the surviving policy is attached first, got {status} with {body}"
    );

    for (label, document) in not_policy_documents() {
        let (status, error) = call(
            &router,
            "PUT",
            &uri,
            Some(json!({"policy": document.clone()})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the modelled 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(INVALID_POLICY_DOCUMENT_MESSAGE),
            "case {label:?}: the refusal states the member and the form it must take, got \
             {error}"
        );

        let (status, got) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused put left the policy in place, got {status} with {got}"
        );
        assert_eq!(
            got["policy"],
            json!(held),
            "case {label:?}: the held document must be untouched, got {got}"
        );
    }
}

/// The member's published `@length` bounds: a document AT the maximum is accepted, one
/// past it is refused.
#[tokio::test]
async fn the_policy_document_length_bound_is_enforced() {
    let router = router();
    let feed_id = create_feed(&router, "policy-length-bound").await;
    let uri = format!("/v1/feed/{feed_id}/policy");

    let at_max = policy_of_length(POLICY_DOCUMENT_MAX_LEN);
    let (status, body) = call(&router, "PUT", &uri, Some(json!({"policy": at_max}))).await;
    assert_eq!(
        status, 200,
        "a document of exactly {POLICY_DOCUMENT_MAX_LEN} characters is accepted, got {status} \
         with {body}"
    );
    let (status, got) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "the maximum-length document is reported back, got {status} with {got}"
    );
    assert_eq!(
        got["policy"].as_str().map(str::len),
        Some(POLICY_DOCUMENT_MAX_LEN),
        "the whole document is stored, not a truncation, got {got}"
    );

    let past_max = policy_of_length(POLICY_DOCUMENT_MAX_LEN + 1);
    let (status, error) = call(&router, "PUT", &uri, Some(json!({"policy": past_max}))).await;
    assert_eq!(
        status, 400,
        "a document one character past the maximum is refused, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ValidationException"),
        "that 400 is a ValidationException, got {error}"
    );

    // And the over-long document stored nothing: the feed still reports the one it took.
    let (status, got) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "the refused put left the policy in place, got {status} with {got}"
    );
    assert_eq!(
        got["policy"].as_str().map(str::len),
        Some(POLICY_DOCUMENT_MAX_LEN),
        "the held document must be untouched, got {got}"
    );
}
