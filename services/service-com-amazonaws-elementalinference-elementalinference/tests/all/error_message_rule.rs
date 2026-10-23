// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a 4xx refusal, end to end over the wire.
//!
//! THE RULE under test: a refusal's message is determined by the RULE that refused the
//! request, never by the values the request happened to carry. So:
//!
//! * a refusal decided from STORED STATE (the feed already holds an association, the
//!   feed's status admits no association, the feed is being deleted, an append would
//!   pass the output quota) carries that rule's INVARIANT sentence — byte-identical
//!   across requests and across records, and interpolating no value the handler read out
//!   of state;
//! * EXCEPT where the refusal's point is to report the state it was decided from: the
//!   service's own refusals state the status it observed (`DisassociateFeed` on a feed
//!   that holds no resource) and how many records held the request back
//!   (`DeleteDictionary` on a referenced dictionary), so those messages are BUILT from
//!   the store and move with it — the status and the count are the information the
//!   refusal exists to give;
//! * a refusal decided from a RELATION between two members (`endDate` before
//!   `startDate`) likewise carries the rule's invariant sentence, naming the members
//!   rather than the dates they carried;
//! * a refusal decided from ONE value the caller supplied — an identifier that resolved
//!   to nothing — is BUILT FROM THE REQUEST and names that identifier (`GetFixture` on
//!   a `fixtureId` the data source does not have).
//!
//! Every case below is a TABLE: the rows differ along what the rule generalises over
//! (the resource names, the feed's status, how many outputs are held and added, how many
//! feeds reference a dictionary, the dates and the distance between them, the form and
//! length of the identifier), so an implementation that rebuilt a message from the
//! request, or that fixed a message the service derives from state, would answer rows
//! that differ where the rule says they must not — or answer rows alike where it says
//! they must differ.

use crate::harness::{call, output_config, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    feed_not_associated_message, FeedStatus, END_DATE_BEFORE_START_DATE_MESSAGE,
    FEED_ALREADY_ASSOCIATED_MESSAGE, FEED_BEING_DELETED_MESSAGE, FEED_NOT_ASSOCIABLE_MESSAGE,
    MAX_OUTPUTS_PER_FEED, WRONG_ASSOCIATED_RESOURCE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// One output named `name`, in the smallest config the model admits.
fn output(name: &str) -> Value {
    json!({"name": name, "outputConfig": output_config("cropping"), "status": "ENABLED"})
}

/// `count` distinct outputs, each named `<prefix><n>`.
fn outputs(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|n| output(&format!("{prefix}{n}")))
            .collect(),
    )
}

/// Create a feed carrying `count` outputs; a created feed is settled and `AVAILABLE`.
async fn create_feed(router: &MultiServiceRouter, name: &str, count: usize) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": name, "outputs": outputs("held", count)})),
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

/// Create a feed carrying `count` outputs and assert it is `AVAILABLE` — the status from
/// which a never-associated feed takes an association, and the one a create leaves it in.
async fn available_feed(router: &MultiServiceRouter, name: &str, count: usize) -> String {
    let id = create_feed(router, name, count).await;
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        (status, &feed["status"]),
        (200, &json!("AVAILABLE")),
        "case {name:?}: a created feed is AVAILABLE without any wait, got {feed}"
    );
    id
}

/// `POST /v1/feed/{id}/associate`.
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    outputs: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({"associatedResourceName": resource, "outputs": outputs})),
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

/// The first half of `name`, cut on a character boundary — a prefix of a name is one of
/// the ways a rejected `associatedResourceName` can differ from the associated one.
fn first_half(name: &str) -> String {
    name.chars().take(name.chars().count() / 2).collect()
}

/// The `message` of a refusal, after asserting its HTTP status and error code.
fn refusal(label: &str, answered: (u16, Value), expected_status: u16, expected_type: &str) -> String {
    let (status, body) = answered;
    assert_eq!(
        status, expected_status,
        "case {label:?}: expected HTTP {expected_status}, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!(expected_type),
        "case {label:?}: expected {expected_type}, got {body}"
    );
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the refusal carries a message, got {body}"))
        .to_string()
}

/// Resource names that differ along everything `AssociatedResourceName` admits (its
/// `@pattern` is `^[\w \-\.',@:;/]*$` and its `@length` is 1..2048): the ASCII form the
/// recording used, spaces and punctuation, a long name, and a non-ASCII word character.
/// None of them is a substring of any invariant sentence this suite asserts, so a
/// message that leaked one would be caught.
fn resource_names() -> Vec<(&'static str, String)> {
    vec![
        ("the recorded form", "err-source-01".to_string()),
        ("spaces and punctuation", "Match Day 3 - West.Stand".to_string()),
        ("path-like, with a colon", "media:live/west-stand".to_string()),
        ("a long name", format!("source-{}", "x".repeat(300))),
        ("non-ASCII word characters", "quelle-vidéo-source".to_string()),
    ]
}

/// "You can associate only one resource with each feed": the refusal is decided from the
/// association the feed already holds, so every row answers ONE invariant sentence that
/// reports neither the resource holding the feed nor the feed's status.
#[tokio::test]
async fn associate_on_an_already_claimed_feed_answers_one_invariant_sentence() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    for (label, holder) in resource_names() {
        let id = available_feed(&router, "claimed-feed", 1).await;
        let (status, claimed) = associate(&router, &id, &holder, json!([])).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first claim is accepted, got {status} with {claimed}"
        );

        // A second, different resource — differing from the holder in every row.
        let other = format!("{holder}-other");
        let message = refusal(
            label,
            associate(&router, &id, &other, json!([output("subs1")])).await,
            400,
            "ValidationException",
        );
        assert_eq!(
            message, FEED_ALREADY_ASSOCIATED_MESSAGE,
            "case {label:?}: expected the invariant sentence \
             {FEED_ALREADY_ASSOCIATED_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(holder.as_str()),
            "case {label:?}: the refusal must not report the resource that holds the feed \
             ({holder:?}), got {message:?}"
        );
        assert!(
            !message.contains("ACTIVE"),
            "case {label:?}: the refusal must not report the feed's internal status, got \
             {message:?}"
        );
        seen.push(message);
    }

    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the sentence is invariant across every row, got {seen:?}"
    );
}

/// From a status that admits no association at all, the refusal is likewise decided from
/// the stored feed — one invariant sentence, with no status in it.
#[tokio::test]
async fn associate_from_a_non_associable_status_answers_one_invariant_sentence() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    // One row per non-associable status this service produces (`FeedStatus`: DELETING,
    // ARCHIVED; AVAILABLE and ACTIVE are the associable ones, and neither CREATING nor
    // UPDATING is ever reported — a created feed is AVAILABLE at once, and UpdateFeed
    // answers 200 and so leaves the feed settled).
    for status_under_test in ["DELETING", "ARCHIVED"] {
        let id = match status_under_test {
            "DELETING" => {
                let id = available_feed(&router, "deleting-feed", 1).await;
                let (status, body) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
                assert_eq!(
                    status, 202,
                    "case {status_under_test:?}: DeleteFeed answers 202, got {body}"
                );
                id
            }
            "ARCHIVED" => {
                let id = available_feed(&router, "archived-feed", 1).await;
                let (status, body) = associate(&router, &id, "released-media", json!([])).await;
                assert_eq!(
                    status, 200,
                    "case {status_under_test:?}: the claim is accepted, got {body}"
                );
                let (status, body) = disassociate(&router, &id, "released-media").await;
                assert_eq!(
                    status, 200,
                    "case {status_under_test:?}: the release is accepted, got {body}"
                );
                id
            }
            other => panic!("no such status row: {other}"),
        };

        let message = refusal(
            status_under_test,
            associate(&router, &id, "would-be-media", json!([])).await,
            400,
            "ValidationException",
        );
        assert_eq!(
            message, FEED_NOT_ASSOCIABLE_MESSAGE,
            "case {status_under_test:?}: expected the invariant sentence \
             {FEED_NOT_ASSOCIABLE_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(status_under_test),
            "case {status_under_test:?}: the refusal must not report the feed's internal \
             status, got {message:?}"
        );
        seen.push(message);
    }

    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the sentence is invariant across every status, got {seen:?}"
    );
}

/// `DisassociateFeed` under a name that is not the associated resource's: decided from
/// the stored association, so one invariant sentence that reports neither the name the
/// request carried nor the one the feed really holds.
#[tokio::test]
async fn disassociate_under_the_wrong_name_answers_one_invariant_sentence() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    for (label, holder) in resource_names() {
        let id = available_feed(&router, "released-feed", 1).await;
        let (status, claimed) = associate(&router, &id, &holder, json!([])).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the claim is accepted, got {status} with {claimed}"
        );

        // The rows differ in how the rejected name differs from the holder's: a plain
        // other name, a case variation, and a prefix of the holder's.
        for (how, other) in [
            ("a plain other name", "does-not-match".to_string()),
            ("the holder in upper case", holder.to_uppercase()),
            ("a prefix of the holder", first_half(&holder)),
        ] {
            if other == holder {
                continue;
            }
            let label = format!("{label}, {how}");
            let message = refusal(
                &label,
                disassociate(&router, &id, &other).await,
                400,
                "ValidationException",
            );
            assert_eq!(
                message, WRONG_ASSOCIATED_RESOURCE_MESSAGE,
                "case {label:?}: expected the invariant sentence \
                 {WRONG_ASSOCIATED_RESOURCE_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(holder.as_str()),
                "case {label:?}: the refusal must not report the resource that holds the \
                 feed ({holder:?}), got {message:?}"
            );
            assert!(
                !message.contains(other.as_str()),
                "case {label:?}: the refusal must not echo the rejected name ({other:?}), \
                 got {message:?}"
            );
            seen.push(message);
        }
    }

    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the sentence is invariant across every row, got {seen:?}"
    );
}

/// `DisassociateFeed` on a feed that holds no resource at all: the refusal REPORTS the
/// status it was decided from, so each status answers its own sentence — one a client
/// can read that status out of.
#[tokio::test]
async fn disassociate_with_nothing_to_release_reports_the_status_it_was_decided_from() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    // One row per status that holds no resource: a created feed, one whose deletion has
    // been accepted, and one already released.
    for (status_under_test, status) in [
        ("AVAILABLE", FeedStatus::Available),
        ("DELETING", FeedStatus::Deleting),
        ("ARCHIVED", FeedStatus::Archived),
    ] {
        let id = match status_under_test {
            "AVAILABLE" => available_feed(&router, "never-associated", 1).await,
            "DELETING" => {
                let id = available_feed(&router, "never-associated", 1).await;
                let (code, body) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
                assert_eq!(
                    code, 202,
                    "case {status_under_test:?}: DeleteFeed answers 202, got {body}"
                );
                id
            }
            "ARCHIVED" => {
                let id = available_feed(&router, "released-feed", 1).await;
                let (code, body) = associate(&router, &id, "released-media", json!([])).await;
                assert_eq!(
                    code, 200,
                    "case {status_under_test:?}: the claim is accepted, got {body}"
                );
                let (code, body) = disassociate(&router, &id, "released-media").await;
                assert_eq!(
                    code, 200,
                    "case {status_under_test:?}: the release is accepted, got {body}"
                );
                id
            }
            other => panic!("no such status row: {other}"),
        };

        let message = refusal(
            status_under_test,
            disassociate(&router, &id, "never-associated-media").await,
            400,
            "ValidationException",
        );
        assert_eq!(
            message,
            feed_not_associated_message(status),
            "case {status_under_test:?}: expected the refusal built from the stored feed, \
             got {message:?}"
        );
        assert!(
            message.contains(status_under_test),
            "case {status_under_test:?}: the refusal reports the status it was decided \
             from, got {message:?}"
        );
        assert!(
            !message.contains("never-associated-media"),
            "case {status_under_test:?}: and it echoes no value the request carried, got \
             {message:?}"
        );
        seen.push(message);
    }

    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        3,
        "each status answers its own sentence, got {seen:?}"
    );
}

/// An append past the output quota states the BOUND, which is the rule — and not how
/// much of it the stored feed is already using.
#[tokio::test]
async fn an_append_past_the_output_quota_states_the_bound_and_no_stored_count() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    // (held, added) rows that each exceed the bound by a different amount, from
    // different directions.
    for (held, added) in [
        (1, MAX_OUTPUTS_PER_FEED),
        (5, MAX_OUTPUTS_PER_FEED),
        (3, MAX_OUTPUTS_PER_FEED - 1),
        (MAX_OUTPUTS_PER_FEED, 1),
    ] {
        let label = format!("{held} held + {added} added");
        let id = available_feed(&router, "quota-feed", held).await;
        let message = refusal(
            &label,
            associate(&router, &id, "quota-media", outputs("added", added)).await,
            402,
            "ServiceQuotaExceededException",
        );
        assert!(
            message.contains(&MAX_OUTPUTS_PER_FEED.to_string()),
            "case {label:?}: the refusal states the bound ({MAX_OUTPUTS_PER_FEED}), got \
             {message:?}"
        );
        seen.push(message);
    }

    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the quota sentence does not depend on what the feed already holds, got {seen:?}"
    );
}

/// `UpdateFeed` on a feed whose deletion was already accepted: decided from the stored
/// feed, so one invariant sentence that names neither the feed nor its status.
#[tokio::test]
async fn update_on_a_feed_being_deleted_answers_one_invariant_sentence() {
    let router = router();

    for held in [1_usize, 3] {
        let label = format!("{held} outputs");
        let id = available_feed(&router, "deleting-feed", held).await;
        let (status, body) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(status, 202, "case {label:?}: DeleteFeed answers 202, got {body}");

        let message = refusal(
            &label,
            call(
                &router,
                "PUT",
                &format!("/v1/feed/{id}"),
                Some(json!({"name": "renamed-feed", "outputs": outputs("held", held)})),
            )
            .await,
            409,
            "ConflictException",
        );
        assert_eq!(
            message, FEED_BEING_DELETED_MESSAGE,
            "case {label:?}: expected the invariant sentence {FEED_BEING_DELETED_MESSAGE:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()),
            "case {label:?}: the refusal must not echo the feed id {id:?}, got {message:?}"
        );
        assert!(
            !message.contains("DELETING"),
            "case {label:?}: the refusal must not report the feed's internal status, got \
             {message:?}"
        );
    }
}

/// `endDate` before `startDate` is a rule about the ORDER of the two members, so the
/// refusal names the members — the same sentence for every pair of dates, with neither
/// date in it.
#[tokio::test]
async fn an_inverted_window_answers_one_invariant_sentence_naming_the_members() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    // Inverted windows differing in the distance between the two dates, and in whether
    // they cross a month, a year or a leap day.
    let inverted = [
        ("the recorded pair", "2024-01-10", "2024-01-01"),
        ("one day inverted", "2026-03-14", "2026-03-13"),
        ("across a month boundary", "2026-03-01", "2026-02-27"),
        ("across a year boundary", "2026-01-02", "2025-12-28"),
        ("across a leap day", "2024-03-01", "2024-02-29"),
        ("years apart", "2026-06-15", "2019-06-15"),
    ];

    // Both sports the model declares (`smithy_query(query="shape",shape="DataSourceSport")`).
    for sport in ["basketball", "american-football"] {
        for (how, start, end) in inverted {
            let label = format!("{how} ({sport})");
            let message = refusal(
                &label,
                call(
                    &router,
                    "POST",
                    "/v1/fixtures",
                    Some(json!({"sport": sport, "startDate": start, "endDate": end})),
                )
                .await,
                400,
                "ValidationException",
            );
            assert_eq!(
                message, END_DATE_BEFORE_START_DATE_MESSAGE,
                "case {label:?}: expected the invariant sentence \
                 {END_DATE_BEFORE_START_DATE_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(start) && !message.contains(end),
                "case {label:?}: the refusal must not quote the dates ({start}, {end}), got \
                 {message:?}"
            );
            seen.push(message);
        }
    }

    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the sentence is invariant across every inverted window, got {seen:?}"
    );
}

/// `GetFixture` on a `fixtureId` the data source does not have: the rule is about the ONE
/// value the caller supplied, so the message is built from the request and NAMES that
/// identifier — a different one for a different id.
#[tokio::test]
async fn get_fixture_names_the_fixture_id_it_could_not_find() {
    let router = router();

    // Ids no fixture has, each within `FixtureId`'s modelled `@length` of 1..128 so the
    // handler is what refuses them, differing in form and length: the recorded probe, a
    // near miss on the catalog's own id form, a day that is not a date, a sport the
    // model does not declare, text with no structure, and the modelled maximum length.
    let unknown_ids: Vec<String> = vec![
        "nonexistent-fixture-probe-000".to_string(),
        "basketball-20260314-99".to_string(),
        "basketball-20260230-01".to_string(),
        "cricket-20260314-01".to_string(),
        "nonsense".to_string(),
        "0".to_string(),
        "a".repeat(128),
    ];

    let mut messages: Vec<String> = Vec::new();
    for id in &unknown_ids {
        let message = refusal(
            id,
            call(&router, "GET", &format!("/v1/fixtures/{id}"), None).await,
            400,
            "ValidationException",
        );
        assert!(
            message.contains(id.as_str()),
            "case {id:?}: the refusal names the fixtureId it could not find, got {message:?}"
        );
        assert!(
            message.contains("fixtureId"),
            "case {id:?}: the refusal names the member at fault, got {message:?}"
        );
        messages.push(message);
    }

    // Built from the request, so no two ids earn the same sentence.
    let mut distinct = messages.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        messages.len(),
        "each id earns its own message, got {messages:?}"
    );
}
