// SPDX-License-Identifier: CC0-1.0

//! A RETRY of a `DisassociateFeed` that was already served, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: `DisassociateFeed` is `@idempotent` and its
//! `associatedResourceName` is the operation's `@idempotencyToken`
//! (`smithy_query(query="operation",operation="DisassociateFeed")`), so a request that
//! repeats a release the feed has already served is that one release once more rather
//! than a new request: it answers the success the first call answered and leaves the feed
//! exactly as that call left it. What the token identifies is ONE release of ONE feed, so
//! the rule reaches no further than that:
//!
//! * a name the feed never served a release of is a request of its own, judged against
//!   the feed's state as any other is;
//! * the served release belongs to the feed it was served for, so the same name on
//!   another feed is that other feed's own request;
//! * a `dryRun` serves no release at all ("without actually making any changes"), so it
//!   leaves nothing for a later request to repeat.
//!
//! The first rule is asserted over a TABLE whose rows differ along what it generalises
//! over — the form and length of the resource name (every character class its pattern
//! `^[\w \-\.',@:;/]*$` admits, at the minimum and the maximum of its `@length` 1..2048),
//! how many outputs the released feed holds, and how many times the release is retried,
//! with and without `dryRun` on the retry.

use crate::harness::{call, output, output_config, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    feed_not_associated_message, FeedStatus,
};
use smithy_server_runtime::MultiServiceRouter;

/// Create one feed carrying `outputs` and return its id. A created feed is settled and
/// `AVAILABLE` at once, so the caller can associate it in the very next request.
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

/// `POST /v1/feed/{id}/associate`, claiming the feed for `resource` and appending
/// `outputs`.
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

/// `GET /v1/feed/{id}`, asserting it is served, and returning the record it reports.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// Create a feed carrying `feed_outputs`, claim it for `resource` while appending
/// `appended`, and return its id.
async fn claimed_feed(
    router: &MultiServiceRouter,
    label: &str,
    feed_outputs: Value,
    resource: &str,
    appended: Value,
) -> String {
    let id = create_feed(router, "retry-feed", feed_outputs).await;
    let (status, claimed) = associate(router, &id, resource, appended).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the setup claim is accepted, got {status} with {claimed}"
    );
    id
}

/// One row of the retry table: the feed's own outputs, the resource that claims and then
/// releases it, the outputs the claim appends, and how many times the release is retried.
struct RetryCase {
    label: &'static str,
    feed_outputs: Value,
    resource: String,
    appended: Value,
    retries: usize,
}

fn retry_cases() -> Vec<RetryCase> {
    vec![
        RetryCase {
            label: "a plain resource name, retried once",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "source-media".to_string(),
            appended: json!([]),
            retries: 1,
        },
        RetryCase {
            label: "a name carrying the spaces and punctuation its pattern admits",
            feed_outputs: json!([output("crop1", "cropping", "DISABLED")]),
            resource: "Stadium cam 2 - east.side's feed".to_string(),
            appended: json!([output("subs1", "subtitling", "ENABLED")]),
            retries: 2,
        },
        RetryCase {
            label: "a name carrying the @ : ; / its pattern admits",
            feed_outputs: json!([output("clip1", "clipping", "ENABLED")]),
            resource: "media@venue:cam/1;take-2".to_string(),
            appended: json!([
                output("crop9", "cropping", "ENABLED"),
                {"name": "meta1", "outputConfig": output_config("contextualMetadata"),
                 "status": "DISABLED", "description": "contextual metadata"}
            ]),
            retries: 3,
        },
        RetryCase {
            label: "a one-character name, the minimum its @length admits",
            feed_outputs: json!([output("meta1", "contextualMetadata", "ENABLED")]),
            resource: "a".to_string(),
            appended: json!([]),
            retries: 2,
        },
        RetryCase {
            label: "a 2048-character name, the maximum its @length admits",
            feed_outputs: json!([output("subs1", "subtitling", "ENABLED")]),
            resource: "m".repeat(2048),
            appended: json!([output("crop1", "cropping", "DISABLED")]),
            retries: 1,
        },
        RetryCase {
            label: "a name of non-ASCII word characters, which its \\w class admits",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "Estádio-Ñ-日本語".to_string(),
            appended: json!([output("clip2", "clipping", "ENABLED")]),
            retries: 2,
        },
    ]
}

/// A retried release answers the first call's success again — the same `arn` and `id` —
/// and leaves the feed exactly as the first call left it.
#[tokio::test]
async fn a_retried_release_answers_the_first_calls_success_and_changes_nothing() {
    let router = router();

    for case in retry_cases() {
        let label = case.label;
        let id = claimed_feed(
            &router,
            label,
            case.feed_outputs.clone(),
            &case.resource,
            case.appended.clone(),
        )
        .await;

        let (status, served) = disassociate(&router, &id, &case.resource, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the release is served, got {status} with {served}"
        );
        // What the release left behind: the record every retry below must still find.
        let released = get_feed(&router, &id, label).await;
        assert_eq!(
            released["status"],
            json!("ARCHIVED"),
            "case {label:?}: the served release archives the feed, got {released}"
        );

        for attempt in 1..=case.retries {
            // An SDK retry repeats the request it sent, so the plain retry carries the
            // same `dryRun: false`; the odd attempts send it as a dry run, which asks
            // whether the real request would succeed — and it would, identically.
            let dry_run = attempt % 2 == 1;
            let (status, replayed) = disassociate(&router, &id, &case.resource, dry_run).await;
            assert_eq!(
                status, 200,
                "case {label:?}: retry {attempt} (dryRun={dry_run}) answers 200, got {status} \
                 with {replayed}"
            );
            assert_eq!(
                replayed, served,
                "case {label:?}: retry {attempt} (dryRun={dry_run}) answers the first call's \
                 body {served}, got {replayed}"
            );
            let after = get_feed(&router, &id, label).await;
            assert_eq!(
                after, released,
                "case {label:?}: retry {attempt} (dryRun={dry_run}) leaves the feed as the \
                 first call left it ({released}), got {after}"
            );
        }
    }
}

/// The token identifies ONE release: a release of a resource this feed never released is
/// a request of its own, judged against the feed's state — so it is refused with the
/// modelled `ValidationException` reporting the status the feed was found in, and nothing
/// is written.
#[tokio::test]
async fn a_release_of_a_name_the_feed_never_released_is_still_refused() {
    let router = router();
    let holder = "source-media-holder";
    // The rows differ in HOW the name differs from the released one: an unrelated name,
    // one differing only in case, a prefix, a suffix, and the shortest name the member
    // admits.
    for other in [
        "source-media-other",
        "Source-Media-Holder",
        "source-media",
        "source-media-holder-2",
        "a",
    ] {
        let id = claimed_feed(
            &router,
            other,
            json!([output("crop1", "cropping", "ENABLED")]),
            holder,
            json!([]),
        )
        .await;
        let (status, _) = disassociate(&router, &id, holder, false).await;
        assert_eq!(status, 200, "case {other:?}: the release is served");
        let released = get_feed(&router, &id, other).await;

        let (status, error) = disassociate(&router, &id, other, false).await;
        assert_eq!(
            status, 400,
            "case {other:?}: a name no release was served for is refused, got {status} with \
             {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {other:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(feed_not_associated_message(FeedStatus::Archived)),
            "case {other:?}: it reports the status the feed was found in, got {error}"
        );
        let after = get_feed(&router, &id, other).await;
        assert_eq!(
            after, released,
            "case {other:?}: the refused release wrote nothing ({released}), got {after}"
        );
    }
}

/// A served release belongs to the FEED it was served for: the same resource name on
/// another feed is that feed's own request, answered from that feed's state.
#[tokio::test]
async fn a_served_release_is_the_release_of_one_feed_only() {
    let router = router();
    let resource = "shared-media-name";
    let outputs = json!([output("crop1", "cropping", "ENABLED")]);

    let released_id = claimed_feed(&router, "released", outputs.clone(), resource, json!([])).await;
    let (status, served) = disassociate(&router, &released_id, resource, false).await;
    assert_eq!(status, 200, "the first feed's release is served, got {served}");

    // A feed that never held the resource: its own state decides, so the release is
    // refused even though another feed served a release of that very name.
    let available_id = create_feed(&router, "never-claimed", outputs.clone()).await;
    let (status, error) = disassociate(&router, &available_id, resource, false).await;
    assert_eq!(
        status, 400,
        "case \"a never-claimed feed\": the other feed's served release is not this feed's, \
         got {status} with {error}"
    );
    assert_eq!(
        error["message"],
        json!(feed_not_associated_message(FeedStatus::Available)),
        "case \"a never-claimed feed\": the refusal reports its own status, got {error}"
    );

    // A feed that DOES hold the resource under the same name: a real release of its own,
    // which archives it and disables its outputs.
    let claimed_id = claimed_feed(&router, "second holder", outputs, resource, json!([])).await;
    let (status, body) = disassociate(&router, &claimed_id, resource, false).await;
    assert_eq!(
        status, 200,
        "case \"a second holder\": its own release is served, got {status} with {body}"
    );
    assert_ne!(
        body, served,
        "case \"a second holder\": the release answers ITS feed's arn and id, got {body}"
    );
    let feed = get_feed(&router, &claimed_id, "second holder").await;
    assert_eq!(
        (&feed["status"], &feed["outputs"][0]["status"]),
        (&json!("ARCHIVED"), &json!("DISABLED")),
        "case \"a second holder\": its release really ran, got {feed}"
    );
}

/// A dry-run release serves no release ("without actually making any changes"), so it
/// leaves nothing for a later request to repeat: the real release that follows still runs.
#[tokio::test]
async fn a_dry_run_release_leaves_no_release_to_repeat() {
    let router = router();
    let resource = "dry-run-media";
    let id = claimed_feed(
        &router,
        "dry run first",
        json!([output("crop1", "cropping", "ENABLED")]),
        resource,
        json!([]),
    )
    .await;

    let (status, dry) = disassociate(&router, &id, resource, true).await;
    assert_eq!(
        status, 200,
        "the dry run is accepted, got {status} with {dry}"
    );
    let feed = get_feed(&router, &id, "dry run first").await;
    assert_eq!(
        (&feed["status"], &feed["outputs"][0]["status"]),
        (&json!("ACTIVE"), &json!("ENABLED")),
        "the dry run changed nothing, got {feed}"
    );

    let (status, served) = disassociate(&router, &id, resource, false).await;
    assert_eq!(
        (status, &served),
        (200, &dry),
        "the real release still runs and answers the same arn and id, got {status} with {served}"
    );
    let feed = get_feed(&router, &id, "dry run first").await;
    assert_eq!(
        (&feed["status"], &feed["outputs"][0]["status"]),
        (&json!("ARCHIVED"), &json!("DISABLED")),
        "the real release released the resource, got {feed}"
    );
    assert_eq!(
        feed.get("association").cloned().unwrap_or(Value::Null),
        Value::Null,
        "the real release cleared the association, got {feed}"
    );
}
