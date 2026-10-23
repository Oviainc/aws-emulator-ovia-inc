// SPDX-License-Identifier: CC0-1.0

//! Refusals the service DERIVES from the state it decided them in, end to end over the
//! wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: where the service's refusal REPORTS the state it was decided
//! from, that report is read out of the store on every call, so it moves with the store
//! and two records in two conditions answer two different messages. Two refusals of this
//! service are of that kind:
//!
//! * `DisassociateFeed` on a feed that holds no resource to release states the STATUS
//!   the feed was found in ("It has status AVAILABLE which indicate that it's not
//!   currently associated with a resource"), so a caller can tell which condition
//!   blocked it;
//! * `DeleteDictionary` on a dictionary a feed references states HOW MANY feeds
//!   reference it ("referenced by 1 feed(s)"), derived from the same feed store the
//!   refusal is decided from, so a caller tearing the dictionary down knows how much is
//!   left to remove.
//!
//! Each is asserted over a TABLE — every status that holds no resource, and reference
//! counts from one feed to several — and then over a WALK through the store, which is
//! what pins the message to the state rather than to the request: the same request,
//! answered differently as the records behind it change. The HTTP status and the error
//! code are what the model fixes (`ValidationException` 400, `ConflictException` 409)
//! and every row asserts them too.

use crate::harness::{call, output_config, past_the_transition_span, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    dictionary_referenced_message, feed_not_associated_message, FeedStatus,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `message` a refusal carries.
fn message_of(error: &Value) -> String {
    error["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal carries a message, got {error}"))
        .to_string()
}

/// One output, named `name`, in the smallest config the model admits — a feed the
/// documentation requires to carry "at least one output".
fn plain_output(name: &str) -> Value {
    json!({"name": name, "outputConfig": output_config("cropping"), "status": "ENABLED"})
}

/// One output, named `name`, whose subtitling config names `dictionary` when given
/// (`SubtitlingConfig$dictionary`: "the ID of a custom dictionary").
fn subtitling_output(name: &str, dictionary: Option<&str>) -> Value {
    let mut config = output_config("subtitling");
    if let Some(dictionary) = dictionary {
        config["subtitling"]
            .as_object_mut()
            .expect("the subtitling config is an object")
            .insert("dictionary".to_string(), json!(dictionary));
    }
    json!({"name": name, "outputConfig": config, "status": "ENABLED"})
}

/// Create one feed carrying `outputs` and return its id.
async fn create_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": name, "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreateFeed answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// `POST /v1/feed/{id}/associate`.
async fn associate(router: &MultiServiceRouter, id: &str, resource: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({"associatedResourceName": resource, "outputs": []})),
    )
    .await
}

/// `POST /v1/feed/{id}/disassociate`.
async fn disassociate(router: &MultiServiceRouter, id: &str, resource: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/disassociate"),
        Some(json!({"associatedResourceName": resource})),
    )
    .await
}

/// A feed in each status that holds NO resource, built the way a caller reaches it.
async fn feed_in_status(router: &MultiServiceRouter, status: FeedStatus) -> String {
    match status {
        // A created feed is settled and unassociated.
        FeedStatus::Available => {
            create_feed(router, "no-resource-feed", json!([plain_output("crop1")])).await
        }
        FeedStatus::Deleting => {
            let id = create_feed(router, "retiring-feed", json!([plain_output("crop1")])).await;
            let (code, body) = call(router, "DELETE", &format!("/v1/feed/{id}"), None).await;
            assert_eq!(code, 202, "DeleteFeed answers 202, got {code} with {body}");
            id
        }
        FeedStatus::Archived => {
            let id = create_feed(router, "released-feed", json!([plain_output("crop1")])).await;
            let (code, body) = associate(router, &id, "held-media").await;
            assert_eq!(code, 200, "the claim is accepted, got {code} with {body}");
            let (code, body) = disassociate(router, &id, "held-media").await;
            assert_eq!(code, 200, "the release is accepted, got {code} with {body}");
            id
        }
        other => panic!("that status holds a resource: {other:?}"),
    }
}

/// `DisassociateFeed` with nothing to release reports the status the feed was found in —
/// over every such status, and whatever name the request carried.
#[tokio::test]
async fn a_release_with_nothing_to_release_reports_the_feeds_own_status() {
    // One row per status that holds no resource. CREATING and UPDATING are never
    // reported by this service (a created feed is AVAILABLE at once, and UpdateFeed
    // answers 200), and ACTIVE is the one status that DOES hold a resource.
    let statuses = [
        ("AVAILABLE", FeedStatus::Available),
        ("DELETING", FeedStatus::Deleting),
        ("ARCHIVED", FeedStatus::Archived),
    ];
    // The names differ along everything `AssociatedResourceName` admits (pattern
    // `^[\w \-\.',@:;/]*$`, length 1..2048): none of them may reach the message, so the
    // shortest row is a digit rather than a letter, which a sentence of words cannot
    // contain by accident.
    let names = [
        "unrelated-media",
        "Match Day 3 - West.Stand",
        "media:live/west-stand",
        "7",
    ];

    let mut by_status: Vec<(&str, String)> = Vec::new();
    for (wire, status) in statuses {
        for name in names {
            let case = format!("{wire} refusing a release named {name:?}");
            let router = router();
            let id = feed_in_status(&router, status).await;

            let (code, error) = disassociate(&router, &id, name).await;
            assert_eq!(
                code, 400,
                "case {case:?}: a release with nothing to release is a 400, got {code} with \
                 {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ValidationException"),
                "case {case:?}: that 400 is a ValidationException, got {error}"
            );
            let message = message_of(&error);
            assert_eq!(
                message,
                feed_not_associated_message(status),
                "case {case:?}: the refusal is built from the stored feed, got {message:?}"
            );
            assert!(
                message.contains(wire),
                "case {case:?}: and reports the status it was decided from, got {message:?}"
            );
            assert!(
                !message.contains(name) && !message.contains(&id),
                "case {case:?}: and echoes nothing the request carried, got {message:?}"
            );
            by_status.push((wire, message));
        }
    }

    // One message per status, and a different one for each: a fixed sentence would
    // collapse these rows into one.
    for (wire, message) in &by_status {
        let same: Vec<&String> = by_status
            .iter()
            .filter(|(other, _)| other == wire)
            .map(|(_, message)| message)
            .collect();
        assert!(
            same.iter().all(|other| *other == message),
            "case {wire:?}: one status answers one message, got {same:?}"
        );
    }
    let mut distinct: Vec<&String> = by_status.iter().map(|(_, message)| message).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        statuses.len(),
        "each status answers its own message, got {distinct:?}"
    );
}

/// The same release request, answered differently as the feed moves through the store:
/// the message follows the state, not the request.
#[tokio::test]
async fn the_release_refusal_follows_the_feed_through_the_store() {
    let router = router();
    let id = create_feed(&router, "walked-feed", json!([plain_output("crop1")])).await;

    // AVAILABLE: nothing to release yet.
    let (code, error) = disassociate(&router, &id, "walked-media").await;
    assert_eq!(
        (code, message_of(&error)),
        (400, feed_not_associated_message(FeedStatus::Available)),
        "case \"before the claim\": the refusal reports AVAILABLE, got {code} with {error}"
    );

    // ACTIVE: the claim, then the release that succeeds.
    let (code, _) = associate(&router, &id, "walked-media").await;
    assert_eq!(code, 200, "case \"the claim\": AssociateFeed answers 200");
    let (code, _) = disassociate(&router, &id, "walked-media").await;
    assert_eq!(code, 200, "case \"the release\": DisassociateFeed answers 200");

    // ARCHIVED: the very same request, now reporting the status the release left.
    let (code, error) = disassociate(&router, &id, "walked-media").await;
    assert_eq!(
        (code, message_of(&error)),
        (400, feed_not_associated_message(FeedStatus::Archived)),
        "case \"after the release\": the same request now reports ARCHIVED, got {code} with \
         {error}"
    );
}

/// Create a dictionary and `count` feeds whose subtitling output names it; returns the
/// dictionary id and the feed ids.
async fn dictionary_referenced_by(
    router: &MultiServiceRouter,
    count: usize,
) -> (String, Vec<String>) {
    let (status, created) = call(
        router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": "refdict", "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDictionary answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDictionary returns an id, got {created}"))
        .to_string();

    let mut feeds = Vec::new();
    for index in 0..count {
        feeds.push(
            create_feed(
                router,
                &format!("holder-{index}"),
                json!([subtitling_output("subs1", Some(&id))]),
            )
            .await,
        );
    }
    (id, feeds)
}

/// `DELETE /v1/dictionary/{id}`.
async fn delete_dictionary(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/v1/dictionary/{id}"), None).await
}

/// The referenced-dictionary refusal states how many feeds reference the dictionary —
/// over a table of counts, and with a feed whose own deletion is already accepted left
/// out of the figure, because it no longer holds the dictionary back.
#[tokio::test]
async fn a_referenced_dictionary_refusal_states_the_number_of_referencing_feeds() {
    // Each row: how many feeds reference the dictionary, and how many of those have
    // already been deleted (a feed on its way out holds nothing back).
    for (referencing, retired) in [(1_usize, 0_usize), (2, 0), (3, 0), (5, 0), (3, 1), (2, 2)] {
        let expected = referencing - retired;
        let case = format!("{referencing} referencing feeds, {retired} of them retiring");
        let router = router();
        let (dictionary, feeds) = dictionary_referenced_by(&router, referencing).await;
        for feed in feeds.iter().take(retired) {
            let (code, body) = call(&router, "DELETE", &format!("/v1/feed/{feed}"), None).await;
            assert_eq!(
                code, 202,
                "case {case:?}: DeleteFeed answers 202, got {code} with {body}"
            );
        }
        // The recorded flow waits for the dictionary to settle before deleting it.
        past_the_transition_span();
        let (code, dictionary_body) =
            call(&router, "GET", &format!("/v1/dictionary/{dictionary}"), None).await;
        assert_eq!(
            (code, &dictionary_body["status"]),
            (
                200,
                &json!(if expected == 0 {
                    "AVAILABLE"
                } else {
                    "REFERENCED"
                })
            ),
            "case {case:?}: the dictionary reports REFERENCED exactly while a live feed \
             names it, got {code} with {dictionary_body}"
        );

        let (code, answer) = delete_dictionary(&router, &dictionary).await;
        if expected == 0 {
            // Nothing holds it back any more, so the delete is accepted.
            assert_eq!(
                code, 202,
                "case {case:?}: with no live reference the delete is accepted, got {code} \
                 with {answer}"
            );
            continue;
        }
        assert_eq!(
            code, 409,
            "case {case:?}: deleting a referenced dictionary is a 409, got {code} with \
             {answer}"
        );
        assert_eq!(
            answer["__type"],
            json!("ConflictException"),
            "case {case:?}: that 409 is a ConflictException, got {answer}"
        );
        let message = message_of(&answer);
        assert_eq!(
            message,
            dictionary_referenced_message(expected),
            "case {case:?}: the refusal states the number of referencing feeds, got \
             {message:?}"
        );
        assert!(
            message.contains(&expected.to_string()),
            "case {case:?}: so the count is in the message, got {message:?}"
        );
        for feed in &feeds {
            assert!(
                !message.contains(feed),
                "case {case:?}: and no feed is named, got {message:?}"
            );
        }
    }
}

/// The same delete request, answered with a smaller count as each referencing feed goes:
/// the figure is derived from the feed store on every call, not fixed in the sentence.
#[tokio::test]
async fn the_reference_count_falls_as_each_referencing_feed_is_removed() {
    let router = router();
    let (dictionary, feeds) = dictionary_referenced_by(&router, 3).await;
    past_the_transition_span();

    for (removed, feed) in feeds.iter().enumerate() {
        let remaining = feeds.len() - removed;
        let case = format!("{remaining} referencing feeds left");
        let (code, answer) = delete_dictionary(&router, &dictionary).await;
        assert_eq!(
            (code, message_of(&answer)),
            (409, dictionary_referenced_message(remaining)),
            "case {case:?}: the refusal states the feeds that are left, got {code} with \
             {answer}"
        );
        // Rewrite the feed so its output names no dictionary at all: the reference is
        // gone, and the next refusal must say so.
        let (code, body) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{feed}"),
            Some(json!({
                "name": "released-holder",
                "outputs": [subtitling_output("subs1", None)]
            })),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: UpdateFeed answers 200, got {code} with {body}"
        );
    }

    let (code, accepted) = delete_dictionary(&router, &dictionary).await;
    assert_eq!(
        code, 202,
        "case \"no references left\": the delete is accepted, got {code} with {accepted}"
    );
}
