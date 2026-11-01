// SPDX-License-Identifier: CC0-1.0

//! The association lifecycle driven by claims that SUPPLY NO OUTPUT, end to end over the
//! wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees.
//!
//! THE RULE under test: a list member whose target carries no `@length` admits a list of
//! any size, so where the operation that carries it only SUPPLEMENTS what the resource
//! already holds, a list that supplies nothing is served and leaves that stored list as it
//! was, the call taking its other effect alone. For this service that operation is
//! `AssociateFeed`: its `outputs` target (`CreateOutputList`,
//! `smithy_query(query="shape",shape="CreateOutputList")`) constrains no length, and the
//! operation is a PATCH whose outputs "supplement any outputs that you specified when you
//! created or updated the feed" and where "parameters that you don't include will not be
//! affected" — so a claim carrying `[]` is SERVED with HTTP 200 `{arn, id}`, takes the
//! association, leaves the feed holding exactly the outputs it already held, and opens the
//! rest of the lifecycle: a second resource is refused, a race of such claims has exactly
//! one winner, and the release path (its dry run, its wrong-name refusal, its
//! nothing-to-release refusal, the archive) is reachable through it.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: how
//! many outputs the stored feed holds — from one to as many as a feed may hold — which
//! features those outputs are for, the form of the resource name the claim carries (from
//! one character to the member's maximum length, with and without the punctuation its
//! pattern admits), and, for the race, how many callers claim at once. The cardinality rule
//! this one sits beside — which operations REFUSE a list that supplies nothing, because
//! theirs REPLACES the stored one — is `supplied_output_cardinality.rs`.

use crate::harness::{call, message_of, output, router, REGION};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    feed_arn, feed_not_associated_message, FeedStatus, FEED_ALREADY_ASSOCIATED_MESSAGE,
    MAX_OUTPUTS_PER_FEED, WRONG_ASSOCIATED_RESOURCE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The output list a claim under test carries: the empty one, which is what the rule is
/// about.
fn no_outputs() -> Value {
    json!([])
}

/// `POST /v1/feed/{id}/associate` carrying `outputs` — a dry run when `dry_run`.
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    outputs: Value,
    dry_run: bool,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": resource,
            "outputs": outputs,
            "dryRun": dry_run,
        })),
    )
    .await
}

/// `POST /v1/feed/{id}/disassociate` releasing `resource` — a dry run when `dry_run`.
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

/// Create a feed holding `outputs` and return its id. A created feed is settled and holds
/// no resource, so it is ready to be claimed when this returns.
async fn stored_feed(router: &MultiServiceRouter, case: &str, outputs: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": "claimed-feed", "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateFeed answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// `GET /v1/feed/{id}`, asserting the read itself is served.
async fn get_feed(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// The `outputs` a feed response reports.
fn outputs_of(feed: &Value) -> &Value {
    &feed["outputs"]
}

/// `count` outputs, each named apart and taking the next feature of `kinds` in turn, so a
/// row's feed holds the features it names.
fn held_outputs(count: usize, kinds: &[&str]) -> Value {
    Value::Array(
        (0..count)
            .map(|index| output(&format!("held{index}"), kinds[index % kinds.len()], "ENABLED"))
            .collect(),
    )
}

/// One row of the lifecycle table: the feed a claim is sent against, and the resource name
/// the claim carries.
struct ClaimCase {
    label: &'static str,
    /// How many outputs the stored feed holds.
    held: usize,
    /// The features those outputs are for, taken in turn.
    kinds: &'static [&'static str],
    /// The `associatedResourceName` the claim carries.
    resource: String,
}

/// The longest `associatedResourceName` the member admits (`@length` 1..2048, pattern
/// `^[\w \-\.',@:;/]*$`), built from a character the pattern admits.
fn longest_resource_name() -> String {
    "a".repeat(2048)
}

fn claim_cases() -> Vec<ClaimCase> {
    vec![
        ClaimCase {
            label: "one cropping output, claimed under a one-character name",
            held: 1,
            kinds: &["cropping"],
            // The shortest name the member admits, and a digit rather than a letter so a
            // sentence of words cannot contain it by accident.
            resource: "7".to_string(),
        },
        ClaimCase {
            label: "three outputs of three features, claimed under a name with spaces",
            held: 3,
            kinds: &["cropping", "subtitling", "contextualMetadata"],
            resource: "Match Day 3 - West.Stand".to_string(),
        },
        ClaimCase {
            label: "a clipping output bound to a fixture, claimed under a name with ':' and '/'",
            held: 2,
            kinds: &["clipping", "cropping"],
            resource: "media:live/west-stand".to_string(),
        },
        ClaimCase {
            label: "as many outputs as a feed may hold, claimed under the longest name",
            held: MAX_OUTPUTS_PER_FEED,
            kinds: &["cropping", "contextualMetadata"],
            resource: longest_resource_name(),
        },
    ]
}

/// A name no row claims under, well inside the member's `@length` and resembling no ARN:
/// the OTHER resource a row sends when the rule under test is about the name not matching.
const OTHER_RESOURCE: &str = "another-media";

/// A claim that supplies no output is served, takes the association, leaves the feed's
/// stored outputs untouched, and the whole lifecycle it opens — the second-resource
/// refusal, the release and its dry run, the wrong-name refusal and the archive — is
/// reachable through it.
#[tokio::test]
async fn a_claim_supplying_no_output_drives_the_whole_association_lifecycle() {
    for case in claim_cases() {
        let label = case.label;
        let resource = case.resource.as_str();
        assert_ne!(
            resource, OTHER_RESOURCE,
            "case {label:?}: the row's name must differ from the one it is refused under"
        );
        let router = router();
        let id = stored_feed(&router, label, held_outputs(case.held, case.kinds)).await;
        let before = get_feed(&router, label, &id).await;
        assert_eq!(
            (&before["status"], before.get("association")),
            (&json!("AVAILABLE"), None),
            "case {label:?}: the stored feed holds no resource yet, got {before}"
        );

        // Before the claim there is nothing to release, and the refusal reports the status
        // the feed was found in.
        let (status, refused) = disassociate(&router, &id, resource, false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: releasing a feed that holds no resource is a 400, got {status} \
             with {refused}"
        );
        let expected = feed_not_associated_message(FeedStatus::Available);
        assert_eq!(
            (&refused["__type"], message_of(&refused)),
            (&json!("ValidationException"), expected.as_str()),
            "case {label:?}: expected a ValidationException saying {expected:?}, got {refused}"
        );

        // A dry run of the claim that supplies no output is accepted and writes nothing.
        let (status, dry) = associate(&router, &id, resource, no_outputs(), true).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a dry-run claim supplying no output answers 200, got {status} \
             with {dry}"
        );
        let after_dry = get_feed(&router, label, &id).await;
        assert_eq!(
            after_dry, before,
            "case {label:?}: the dry run left the feed as it was ({before}), got {after_dry}"
        );

        // The claim itself: served with the feed's arn and id.
        let (status, claimed) = associate(&router, &id, resource, no_outputs(), false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a claim supplying no output answers 200, got {status} with \
             {claimed}"
        );
        assert_eq!(
            claimed,
            json!({"arn": feed_arn(REGION, &id), "id": id}),
            "case {label:?}: the claim answers the feed's arn and id, got {claimed}"
        );

        // The feed is ACTIVE under the claimed resource and holds exactly the outputs it
        // held before: a list that supplies none adds none and takes none away.
        let active = get_feed(&router, label, &id).await;
        assert_eq!(
            (&active["status"], &active["association"]),
            (
                &json!("ACTIVE"),
                &json!({"associatedResourceName": resource})
            ),
            "case {label:?}: the claimed feed reports ACTIVE and its association, got {active}"
        );
        assert_eq!(
            outputs_of(&active),
            outputs_of(&before),
            "case {label:?}: expected the feed to hold {:?}, got {:?}",
            outputs_of(&before),
            outputs_of(&active)
        );

        // "You can associate only one resource with each feed": another resource's claim
        // on the same feed is refused, and changes nothing.
        let (status, refused) = associate(&router, &id, OTHER_RESOURCE, no_outputs(), false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a second resource's claim is a 400, got {status} with {refused}"
        );
        assert_eq!(
            (&refused["__type"], message_of(&refused)),
            (
                &json!("ValidationException"),
                FEED_ALREADY_ASSOCIATED_MESSAGE
            ),
            "case {label:?}: expected a ValidationException saying \
             {FEED_ALREADY_ASSOCIATED_MESSAGE:?}, got {refused}"
        );
        let still = get_feed(&router, label, &id).await;
        assert_eq!(
            still, active,
            "case {label:?}: the refused claim left the feed as it was ({active}), got {still}"
        );

        // A release under a name that is not the associated resource's is refused, and
        // changes nothing.
        let (status, refused) = disassociate(&router, &id, OTHER_RESOURCE, false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a release under the wrong name is a 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            (&refused["__type"], message_of(&refused)),
            (
                &json!("ValidationException"),
                WRONG_ASSOCIATED_RESOURCE_MESSAGE
            ),
            "case {label:?}: expected a ValidationException saying \
             {WRONG_ASSOCIATED_RESOURCE_MESSAGE:?}, got {refused}"
        );

        // A dry run of the release validates and persists nothing.
        let (status, dry) = disassociate(&router, &id, resource, true).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a dry-run release answers 200, got {status} with {dry}"
        );
        let after_dry = get_feed(&router, label, &id).await;
        assert_eq!(
            after_dry, active,
            "case {label:?}: the dry-run release left the feed as it was ({active}), got \
             {after_dry}"
        );

        // The release: the resource is let go, every output is disabled and the feed is
        // archived.
        let (status, released) = disassociate(&router, &id, resource, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the release answers 200, got {status} with {released}"
        );
        assert_eq!(
            released,
            json!({"arn": feed_arn(REGION, &id), "id": id}),
            "case {label:?}: the release answers the feed's arn and id, got {released}"
        );
        let archived = get_feed(&router, label, &id).await;
        assert_eq!(
            (&archived["status"], archived.get("association")),
            (&json!("ARCHIVED"), None),
            "case {label:?}: a released feed reports ARCHIVED and holds no resource, got \
             {archived}"
        );
        let statuses: Vec<&str> = archived["outputs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: a feed reports an outputs array"))
            .iter()
            .map(|output| output["status"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            statuses,
            vec!["DISABLED"; case.held],
            "case {label:?}: every output of the released feed is DISABLED, got {statuses:?}"
        );

        // And the archive admits no further claim, supplying no output or otherwise.
        for (supplied, outputs) in [
            ("no output", no_outputs()),
            ("one output", json!([output("post1", "cropping", "ENABLED")])),
        ] {
            let (status, refused) = associate(&router, &id, resource, outputs, false).await;
            assert_eq!(
                status, 400,
                "case {label:?}: claiming an archived feed with {supplied} is a 400, got \
                 {status} with {refused}"
            );
            assert_eq!(
                refused["__type"],
                json!("ValidationException"),
                "case {label:?}: that 400 is a ValidationException, got {refused}"
            );
        }
    }
}

/// Concurrent claims that each supply no output resolve to exactly ONE winner: a claim
/// supplying nothing still takes the association, and the association is taken under the
/// state lock, so every other caller is refused — and the feed's stored outputs are
/// untouched by all of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_supplying_no_output_have_exactly_one_winner() {
    let router = Arc::new(router());

    // The rows differ in how many callers race for the one feed.
    for racers in [2_usize, 4, 8] {
        let case = format!("{racers} racers supplying no output");
        const HELD: usize = 2;
        let id = stored_feed(&router, &case, held_outputs(HELD, &["cropping", "subtitling"])).await;
        let before = get_feed(&router, &case, &id).await;

        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let id = id.clone();
            tasks.push(tokio::spawn(async move {
                let resource = format!("racing-media-{racer}");
                let (status, body) = associate(&router, &id, &resource, no_outputs(), false).await;
                (resource, status, body)
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        let mut losers: Vec<String> = Vec::new();
        for task in tasks {
            let (resource, status, body) = task.await.expect("the racing task completed");
            match status {
                200 => winners.push(resource),
                400 => {
                    assert_eq!(
                        (&body["__type"], message_of(&body)),
                        (
                            &json!("ValidationException"),
                            FEED_ALREADY_ASSOCIATED_MESSAGE
                        ),
                        "case {case:?}: a loser is refused with a ValidationException saying \
                         {FEED_ALREADY_ASSOCIATED_MESSAGE:?}, got {body}"
                    );
                    losers.push(resource);
                }
                other => {
                    panic!("case {case:?}: the claim {resource:?} answered HTTP {other} with {body}")
                }
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly 1 winner, got {winners:?} with losers {losers:?}"
        );
        assert_eq!(
            losers.len(),
            racers - 1,
            "case {case:?}: expected every other claim to lose, got {losers:?}"
        );

        // The feed holds the winner's claim, and the outputs it was created with.
        let after = get_feed(&router, &case, &id).await;
        assert_eq!(
            (&after["status"], &after["association"]),
            (
                &json!("ACTIVE"),
                &json!({"associatedResourceName": winners[0]})
            ),
            "case {case:?}: the raced feed is ACTIVE under the winner's resource, got {after}"
        );
        assert_eq!(
            outputs_of(&after),
            outputs_of(&before),
            "case {case:?}: expected the feed to hold {:?}, got {:?}",
            outputs_of(&before),
            outputs_of(&after)
        );
    }
}
