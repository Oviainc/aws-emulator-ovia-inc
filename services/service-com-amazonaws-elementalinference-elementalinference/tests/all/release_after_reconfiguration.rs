// SPDX-License-Identifier: CC0-1.0

//! Releasing the resource a feed holds while a reconfiguration of its payload is still in
//! flight, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: which resource a feed holds is a STORED fact — its association —
//! so `DisassociateFeed` is decided from that association and not from the feed's status.
//! Reconfiguring a feed's payload (`UpdateFeed`: "the payload that you specify completely
//! overwrites the existing payload") suspends the feed's settled status without touching
//! the resource that holds it, so it neither grants nor revokes an association: a release
//! issued straight after an update, with no wait, releases what the feed still holds and
//! archives the feed, while a feed that stores no resource is refused whatever status it
//! reports. A removal the clock already owes is the one thing a release may not discard,
//! so a feed on its way out of the store has nothing left to release either.
//!
//! The rules are asserted over TABLES whose rows differ along what they generalise over —
//! how many reconfigurations precede the release, how many outputs the replacement
//! payload carries and which `OutputConfig` member each one is, the form and length of
//! the `associatedResourceName`, the status the reconfiguration suspended, and whether the
//! replacement keeps or drops a reference to another resource — never over one recorded
//! request.

use crate::harness::{
    call, enabled_output, message_of, past_the_transition_span, router, subtitling_output,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    feed_not_associated_message, FeedStatus, WRONG_ASSOCIATED_RESOURCE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// `GET /v1/feed/{id}`.
async fn get_feed(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}"), None).await
}

/// `PUT /v1/feed/{id}` with `payload`.
async fn update_feed(router: &MultiServiceRouter, id: &str, payload: &Value) -> (u16, Value) {
    call(router, "PUT", &format!("/v1/feed/{id}"), Some(payload.clone())).await
}

/// `POST /v1/feed/{id}/associate`, claiming `resource` and appending one output.
///
/// `outputs` carries "an array of one or more outputs", so a claim supplies at least one;
/// the one here is named apart from every output the rows below store, so the append is
/// what it says and never collides with the payload under test.
async fn associate(router: &MultiServiceRouter, id: &str, resource: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": resource,
            "outputs": [enabled_output("assoc1", "contextualMetadata")],
        })),
    )
    .await
}

/// `POST /v1/feed/{id}/disassociate`, releasing `resource` — as a dry run when `dry_run`.
async fn disassociate(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    dry_run: bool,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/disassociate"),
        Some(json!({"associatedResourceName": resource, "dryRun": dry_run})),
    )
    .await
}

/// The `status` member of a response, as text.
fn status_of(body: &Value) -> &str {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the response carries a status, got {body}"))
}

/// The statuses of a feed's outputs, in the order the feed reports them.
fn output_statuses(feed: &Value) -> Vec<String> {
    feed["outputs"]
        .as_array()
        .unwrap_or_else(|| panic!("a feed response carries an outputs list, got {feed}"))
        .iter()
        .map(|output| status_of(output).to_string())
        .collect()
}

/// Create one feed carrying `outputs` and return its id and ARN.
async fn create_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> (String, String) {
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
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateFeed returns an id, got {created}"))
        .to_string();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateFeed returns an arn, got {created}"))
        .to_string();
    (id, arn)
}

/// One row of the release-after-reconfiguration table: the resource that holds the feed,
/// and the replacement payloads the update(s) store before the release.
struct ReleaseCase {
    label: &'static str,
    resource: String,
    replacements: Vec<Value>,
}

/// The rows differ in how many reconfigurations precede the release (one, two), in how
/// many outputs the replacement carries (one, two, three) and which `OutputConfig` member
/// each one is, and in the form and length of the `associatedResourceName` — which the
/// model admits as `len 1..2048, pattern ^[\w \-\.',@:;/]*$`
/// (`smithy_query(query="operation",operation="DisassociateFeed")`), so the rows span a
/// one-character name, names carrying each punctuation class the pattern allows, and one
/// far longer than any recorded name.
fn release_cases() -> Vec<ReleaseCase> {
    vec![
        ReleaseCase {
            label: "one reconfiguration shrinking the feed to a subtitling output",
            resource: "src-1".to_string(),
            replacements: vec![json!({
                "name": "reconfigured-then-released",
                "outputs": [subtitling_output("subs1", None)]
            })],
        },
        ReleaseCase {
            label: "one reconfiguration growing the feed to three outputs of three kinds",
            resource: "Match Day 3 - West.Stand".to_string(),
            replacements: vec![json!({
                "name": "grown-then-released",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("meta1", "contextualMetadata"),
                    enabled_output("clip1", "clipping")
                ]
            })],
        },
        ReleaseCase {
            label: "a reconfiguration writing back the payload the feed already holds",
            resource: "7".to_string(),
            replacements: vec![json!({
                "name": "held-feed",
                "outputs": [enabled_output("crop1", "cropping")]
            })],
        },
        ReleaseCase {
            label: "two reconfigurations back to back",
            resource: "media:live/west-stand".to_string(),
            replacements: vec![
                json!({
                    "name": "first-replacement",
                    "outputs": [enabled_output("clip1", "clipping")]
                }),
                json!({
                    "name": "second-replacement",
                    "outputs": [
                        enabled_output("crop1", "cropping"),
                        subtitling_output("subs1", None)
                    ]
                }),
            ],
        },
        ReleaseCase {
            label: "a resource name far longer than any recorded one",
            resource: "a".repeat(2048),
            replacements: vec![json!({
                "name": "long-name-release",
                "outputs": [enabled_output("meta1", "contextualMetadata")]
            })],
        },
    ]
}

/// How many outputs a payload carries.
fn output_count(payload: &Value) -> Option<usize> {
    payload["outputs"].as_array().map(Vec::len)
}

/// A release issued while the feed's payload is still being reconfigured releases the
/// resource the feed stores: it answers 200, archives the feed, disables its outputs, and
/// the archived feed stays archived once the reconfiguration's span has passed.
#[tokio::test]
async fn a_release_during_a_reconfiguration_releases_the_stored_resource() {
    for case in release_cases() {
        let label = case.label;
        let router = router();
        let (id, arn) = create_feed(
            &router,
            "held-feed",
            json!([enabled_output("crop1", "cropping")]),
        )
        .await;

        let (status, claimed) = associate(&router, &id, &case.resource).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the claim is accepted, got {status} with {claimed}"
        );

        for (pass, replacement) in case.replacements.iter().enumerate() {
            let (status, updated) = update_feed(&router, &id, replacement).await;
            assert_eq!(
                status, 200,
                "case {label:?}: update {pass} answers 200, got {status} with {updated}"
            );
        }
        let last = case
            .replacements
            .last()
            .expect("every row stores at least one replacement payload");

        // The release is issued with NO wait, so the reconfiguration is still in flight:
        // that is the condition under test, and the feed's association is what it has to
        // be decided from.
        let (status, in_flight) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&in_flight)),
            (200, "UPDATING"),
            "case {label:?}: expected the reconfiguration to be in flight, got {status} with \
             {in_flight}"
        );
        assert_eq!(
            in_flight["association"],
            json!({"associatedResourceName": case.resource}),
            "case {label:?}: expected the reconfiguration to leave the association stored, \
             got {:?}",
            in_flight["association"]
        );

        // A dry run runs every check and persists nothing, so it answers the release the
        // real call would and leaves the feed holding its resource.
        let (status, dry) = disassociate(&router, &id, &case.resource, true).await;
        assert_eq!(
            (status, &dry["arn"], &dry["id"]),
            (200, &json!(arn), &json!(id)),
            "case {label:?}: expected the dry run to answer the release, got {status} with {dry}"
        );
        let (status, untouched) = get_feed(&router, &id).await;
        assert_eq!(
            (status, &untouched["association"]),
            (200, &json!({"associatedResourceName": case.resource})),
            "case {label:?}: expected the dry run to persist nothing, got {status} with \
             {untouched}"
        );

        let (status, released) = disassociate(&router, &id, &case.resource, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the release to be served, got {status} with {released}"
        );
        assert_eq!(
            (&released["arn"], &released["id"]),
            (&json!(arn), &json!(id)),
            "case {label:?}: expected the released feed's arn and id, got {released}"
        );

        // The feed reports the release at once: archived, holding no resource, every
        // output disabled — and carrying the payload the last update stored.
        let (status, archived) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&archived)),
            (200, "ARCHIVED"),
            "case {label:?}: expected the released feed to report ARCHIVED, got {status} with \
             {archived}"
        );
        assert!(
            archived["association"].is_null(),
            "case {label:?}: expected no resource to be reported, got {:?}",
            archived["association"]
        );
        let statuses = output_statuses(&archived);
        assert!(
            statuses.iter().all(|status| status == "DISABLED"),
            "case {label:?}: expected every output to be disabled, got {statuses:?}"
        );
        assert_eq!(
            (archived["name"].clone(), output_count(&archived)),
            (last["name"].clone(), output_count(last)),
            "case {label:?}: expected the last replacement payload to be the stored one, got \
             {archived}"
        );

        // The reconfiguration's own span had not fallen due when the release ran, and the
        // release is what decided where the feed went: once that span passes the feed is
        // still archived, never back in the status the update suspended.
        past_the_transition_span();
        let (status, settled) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&settled)),
            (200, "ARCHIVED"),
            "case {label:?}: expected the released feed to stay ARCHIVED, got {status} with \
             {settled}"
        );
        assert!(
            settled["association"].is_null(),
            "case {label:?}: and to hold no resource, got {:?}",
            settled["association"]
        );
    }
}

/// A reconfiguration does not give a feed a resource it never held: a release on a feed
/// that stores none is still refused, and the refusal reports the status the feed is in.
#[tokio::test]
async fn a_reconfigured_feed_that_stores_no_resource_is_still_refused() {
    // One row per way a feed comes to store no resource: never claimed, and already
    // released. The released row asks for a name the feed never served a release of,
    // because repeating the released name is that same release once more under the
    // operation's idempotency token (`feed_release_retry.rs`).
    for (label, already_released, asked_for) in [
        ("a feed that was never claimed", false, "unclaimed-media"),
        ("a feed whose resource was released", true, "other-media"),
    ] {
        let router = router();
        let (id, _) = create_feed(
            &router,
            "unheld-feed",
            json!([enabled_output("crop1", "cropping")]),
        )
        .await;
        if already_released {
            let (status, claimed) = associate(&router, &id, "released-media").await;
            assert_eq!(
                status, 200,
                "case {label:?}: the claim is accepted, got {status} with {claimed}"
            );
            let (status, released) = disassociate(&router, &id, "released-media", false).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the release is served, got {status} with {released}"
            );
        }

        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({
                "name": "reconfigured-unheld-feed",
                "outputs": [enabled_output("clip1", "clipping")]
            }),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, "UPDATING"),
            "case {label:?}: the update answers 200, got {status} with {updated}"
        );

        let (status, refused) = disassociate(&router, &id, asked_for, false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a release with nothing to release is a 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {refused}"
        );
        let message = message_of(&refused);
        assert_eq!(
            message,
            feed_not_associated_message(FeedStatus::Updating),
            "case {label:?}: the refusal reports the status the feed was found in, got \
             {message:?}"
        );
    }
}

/// The name still has to be the resource that holds the feed, reconfiguration or not: a
/// release under any other name is refused and persists nothing, and the right name is
/// served afterwards.
#[tokio::test]
async fn a_release_under_the_wrong_name_during_a_reconfiguration_is_refused() {
    const HELD: &str = "held-media";
    // Names that are not the held one, differing along everything
    // `AssociatedResourceName` admits: a one-character name, the held name with one
    // character added, one carrying every punctuation class, and one of another length.
    for other in [
        "9",
        "held-media-2",
        "held media:west/stand.1",
        "completely-different-source-media",
    ] {
        let case = format!("a release of {other:?} on a feed held by {HELD:?}");
        let router = router();
        let (id, _) = create_feed(
            &router,
            "wrong-name-feed",
            json!([enabled_output("crop1", "cropping")]),
        )
        .await;
        let (status, claimed) = associate(&router, &id, HELD).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the claim is accepted, got {status} with {claimed}"
        );
        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({
                "name": "reconfigured-wrong-name-feed",
                "outputs": [enabled_output("crop1", "cropping"), subtitling_output("subs1", None)]
            }),
        )
        .await;
        assert_eq!(
            (status, status_of(&updated)),
            (200, "UPDATING"),
            "case {case:?}: the update answers 200, got {status} with {updated}"
        );

        let (status, refused) = disassociate(&router, &id, other, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {case:?}: that 400 is a ValidationException, got {refused}"
        );
        let message = message_of(&refused);
        assert_eq!(
            message, WRONG_ASSOCIATED_RESOURCE_MESSAGE,
            "case {case:?}: expected the rule's invariant sentence, got {message:?}"
        );

        // Nothing was released: the feed still holds its resource with its outputs
        // enabled, and the release under the right name is served.
        let (status, held) = get_feed(&router, &id).await;
        assert_eq!(
            (status, &held["association"]),
            (200, &json!({"associatedResourceName": HELD})),
            "case {case:?}: the refused release released nothing, got {status} with {held}"
        );
        let statuses = output_statuses(&held);
        assert!(
            statuses.iter().all(|status| status == "ENABLED"),
            "case {case:?}: and disabled no output, got {statuses:?}"
        );
        let (status, served) = disassociate(&router, &id, HELD, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the right name is served, got {status} with {served}"
        );
    }
}

/// A removal the clock already owes is the one thing a release may not discard: a feed on
/// its way out of the store has nothing left to release, and the removal still happens.
#[tokio::test]
async fn a_release_does_not_discard_a_removal_the_clock_already_owes() {
    // Both rows hold a resource when the removal is accepted; they differ in whether a
    // reconfiguration of the payload is in flight on top of it.
    for (label, reconfigured) in [
        ("a held feed being removed", false),
        ("a held feed being removed while reconfigured", true),
    ] {
        let router = router();
        let (id, _) = create_feed(
            &router,
            "retiring-held-feed",
            json!([enabled_output("crop1", "cropping")]),
        )
        .await;
        let (status, claimed) = associate(&router, &id, "retiring-media").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the claim is accepted, got {status} with {claimed}"
        );
        if reconfigured {
            let (status, updated) = update_feed(
                &router,
                &id,
                &json!({
                    "name": "reconfigured-retiring-feed",
                    "outputs": [enabled_output("clip1", "clipping")]
                }),
            )
            .await;
            assert_eq!(
                (status, status_of(&updated)),
                (200, "UPDATING"),
                "case {label:?}: the update answers 200, got {status} with {updated}"
            );
        }

        let (status, deleted) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            (status, status_of(&deleted)),
            (202, "DELETING"),
            "case {label:?}: the removal is accepted, got {status} with {deleted}"
        );

        let (status, refused) = disassociate(&router, &id, "retiring-media", false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a feed on its way out has nothing to release, got {status} with \
             {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {refused}"
        );
        let message = message_of(&refused);
        assert_eq!(
            message,
            feed_not_associated_message(FeedStatus::Deleting),
            "case {label:?}: the refusal reports the status the feed was found in, got \
             {message:?}"
        );

        past_the_transition_span();
        let (status, gone) = get_feed(&router, &id).await;
        assert_eq!(
            (status, &gone["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {label:?}: expected the removal to have happened, got {status} with {gone}"
        );
    }
}

/// `POST /v1/dictionary` — a dictionary a subtitling output can reference.
async fn create_dictionary(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": "eng", "entries": "[{\"content\": \"birdie\"}]"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreateDictionary answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateDictionary returns an id, got {created}"))
        .to_string()
}

/// The whole multi-resource teardown runs: a feed whose output references a dictionary is
/// claimed, reconfigured, released and archived, and the dictionary is free exactly when
/// the replacement payload stopped referencing it.
#[tokio::test]
async fn a_referenced_dictionary_is_freed_by_the_reconfiguration_the_release_follows() {
    // The two ways a replacement payload can treat the reference the feed held: dropped,
    // and kept. The release is served either way; what differs is what the dictionary
    // reports afterwards and whether it can then be deleted.
    for (label, keep_reference) in [
        ("the replacement drops the dictionary reference", false),
        ("the replacement keeps the dictionary reference", true),
    ] {
        let router = router();
        let dictionary = create_dictionary(&router, "teardown-dictionary").await;
        let (id, _) = create_feed(
            &router,
            "referencing-feed",
            json!([subtitling_output("subs1", Some(&dictionary))]),
        )
        .await;
        let (status, claimed) = associate(&router, &id, "src-1").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the claim is accepted, got {status} with {claimed}"
        );

        let replacement = subtitling_output("subs1", keep_reference.then_some(&dictionary[..]));
        let (status, updated) = update_feed(
            &router,
            &id,
            &json!({"name": "reconfigured-referencing-feed", "outputs": [replacement]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );

        // The release follows the update with no wait, as a client tearing the resources
        // down issues them.
        let (status, released) = disassociate(&router, &id, "src-1", false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the release to be served, got {status} with {released}"
        );
        let (status, archived) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&archived)),
            (200, "ARCHIVED"),
            "case {label:?}: expected the released feed to report ARCHIVED, got {status} with \
             {archived}"
        );

        // The dictionary's own creation span has passed by now, so what it reports is
        // decided by whether a feed still references it.
        past_the_transition_span();
        let dictionary_path = format!("/v1/dictionary/{dictionary}");
        let (status, read) = call(&router, "GET", &dictionary_path, None).await;
        let expected = if keep_reference {
            "REFERENCED"
        } else {
            "AVAILABLE"
        };
        assert_eq!(
            (status, status_of(&read)),
            (200, expected),
            "case {label:?}: expected the dictionary to report {expected}, got {status} with \
             {read}"
        );

        let (status, answered) = call(&router, "DELETE", &dictionary_path, None).await;
        let expected_status = if keep_reference { 409 } else { 202 };
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected the dictionary delete to answer {expected_status}, got \
             {status} with {answered}"
        );
    }
}
