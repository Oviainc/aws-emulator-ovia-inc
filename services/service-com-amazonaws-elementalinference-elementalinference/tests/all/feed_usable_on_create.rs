// SPDX-License-Identifier: CC0-1.0

//! A created feed is USABLE at once, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: a resource the service hands back READY is ready, so nothing
//! about it is waiting on a clock. `CreateFeed` answers with a feed in the settled
//! `AVAILABLE` status ("After creation of the feed has succeeded, the status will be
//! AVAILABLE", `CreateFeedResponse$status`), and therefore
//!
//! * a `GetFeed` issued in the very next request reports `AVAILABLE`, not a transient
//!   status, and reports it however long the caller waits;
//! * the `AssociateFeed` a caller issues in the very next request is ACCEPTED (HTTP 200)
//!   and claims the feed, so the whole lifecycle — create, associate, read, release —
//!   runs with no wait anywhere in it;
//! * when several callers race to claim one feed, exactly ONE wins and every other is
//!   refused with the modelled `ValidationException`: the losers lose to the WINNER, not
//!   to a status the service put the feed in by itself.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over
//! (how many outputs the feed is created with and which `OutputConfig` member each
//! carries, whether a role and tags are supplied, how many outputs the association
//! appends, and how many callers race), never over one recorded request.

use crate::harness::{call, output_config, past_the_transition_span, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    FEED_ALREADY_ASSOCIATED_MESSAGE, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// One output, named `name`, carrying the `kind` config.
fn output(name: &str, kind: &str) -> Value {
    json!({"name": name, "outputConfig": output_config(kind), "status": "ENABLED"})
}

/// `POST /v1/feed/{id}/associate`, with `dryRun` when asked for.
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

/// `GET /v1/feed/{id}`.
async fn get_feed(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/v1/feed/{id}"), None).await
}

/// The `status` member of a response, as text.
fn status_of(body: &Value) -> &str {
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("the response carries a status, got {body}"))
}

/// One row of the lifecycle table: a feed to create and the outputs the association
/// appends to it.
struct LifecycleCase {
    label: &'static str,
    create: Value,
    appended: Value,
}

/// The rows differ in output count and `OutputConfig` member, in whether a role and tags
/// are supplied, in the length of the name, and in how many outputs the association
/// appends (none, one, two) — everything the rule generalises over.
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            label: "one cropping output, nothing appended",
            create: json!({
                "name": "usable-one",
                "outputs": [output("crop1", "cropping")]
            }),
            appended: json!([]),
        },
        LifecycleCase {
            label: "a single-character name, one output appended",
            create: json!({
                "name": "a",
                "outputs": [output("subs1", "subtitling")]
            }),
            appended: json!([output("clip1", "clipping")]),
        },
        LifecycleCase {
            label: "two outputs with a role and tags, two appended",
            create: json!({
                "name": "Usable_Feed-3",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [output("crop1", "cropping"), output("meta1", "contextualMetadata")],
                "tags": {"env": "test"}
            }),
            appended: json!([output("clip1", "clipping"), output("subs1", "subtitling")]),
        },
    ]
}

/// The recorded lifecycle, with no wait anywhere in it: create, then associate, read and
/// release in the requests that follow.
#[tokio::test]
async fn a_created_feed_runs_the_whole_lifecycle_without_any_wait() {
    for case in lifecycle_cases() {
        let label = case.label;
        let router = router();
        let (status, created) = call(&router, "POST", "/v1/feed", Some(case.create.clone())).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateFeed answers 202, got {status} with {created}"
        );
        assert_eq!(
            status_of(&created),
            "AVAILABLE",
            "case {label:?}: the created feed is AVAILABLE, got {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
            .to_string();

        // The NEXT request: the feed is settled, with no resource yet.
        let (status, fetched) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&fetched)),
            (200, "AVAILABLE"),
            "case {label:?}: a GetFeed in the next request reports AVAILABLE, got {status} \
             with {fetched}"
        );
        assert_eq!(
            fetched.get("association").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {label:?}: and the feed holds no resource yet, got {fetched}"
        );

        // And the request after that claims it.
        let held = format!("{label}-source-media");
        let created_outputs = case.create["outputs"].as_array().map(Vec::len).unwrap_or(0);
        let appended_outputs = case.appended.as_array().map(Vec::len).unwrap_or(0);
        let (status, claimed) = associate(&router, &id, &held, case.appended.clone(), false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: AssociateFeed right after the create is accepted, got {status} \
             with {claimed}"
        );
        assert_eq!(
            claimed["id"], created["id"],
            "case {label:?}: the claim answers for the same feed, got {claimed}"
        );

        let (status, active) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&active)),
            (200, "ACTIVE"),
            "case {label:?}: the claimed feed reports ACTIVE, got {status} with {active}"
        );
        assert_eq!(
            active["association"],
            json!({"associatedResourceName": held}),
            "case {label:?}: and reports the resource that holds it, got {active}"
        );
        let outputs = active["outputs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            outputs.len(),
            created_outputs + appended_outputs,
            "case {label:?}: the association appended its outputs to the stored ones, got \
             {:?}",
            active["outputs"]
        );
        assert!(
            outputs
                .iter()
                .skip(created_outputs)
                .all(|output| output["fromAssociation"] == json!(true)),
            "case {label:?}: an appended output reports fromAssociation, got {:?}",
            active["outputs"]
        );

        // The release, again in the next request.
        let (status, released) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/disassociate"),
            Some(json!({"associatedResourceName": held})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DisassociateFeed is accepted, got {status} with {released}"
        );
        let (status, archived) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&archived)),
            (200, "ARCHIVED"),
            "case {label:?}: the released feed is ARCHIVED, got {status} with {archived}"
        );
        let statuses: Vec<&Value> = archived["outputs"]
            .as_array()
            .map(|outputs| outputs.iter().map(|output| &output["status"]).collect())
            .unwrap_or_default();
        assert!(
            !statuses.is_empty() && statuses.iter().all(|status| *status == &json!("DISABLED")),
            "case {label:?}: every output of a released feed is DISABLED, got {:?}",
            archived["outputs"]
        );
    }
}

/// A created feed reports `AVAILABLE` however long the caller waits: the status is the
/// settled one, so no clock-owed move can change it afterwards.
#[tokio::test]
async fn a_created_feed_reports_the_same_status_before_and_after_any_transition_span() {
    let router = router();
    let mut ids: Vec<String> = Vec::new();
    for case in lifecycle_cases() {
        let (_, created) = call(&router, "POST", "/v1/feed", Some(case.create.clone())).await;
        assert_eq!(
            status_of(&created),
            "AVAILABLE",
            "case {:?}: the create answers AVAILABLE, got {created}",
            case.label
        );
        ids.push(created["id"].as_str().unwrap_or_default().to_string());
    }

    // Each row: how long the caller waits before reading. Nothing may move.
    for (label, wait) in [
        ("no wait at all", std::time::Duration::ZERO),
        ("half a transition span", TRANSITION_SPAN / 2),
    ] {
        std::thread::sleep(wait);
        for id in &ids {
            let (status, fetched) = get_feed(&router, id).await;
            assert_eq!(
                (status, status_of(&fetched)),
                (200, "AVAILABLE"),
                "case {label:?}: feed {id:?} is AVAILABLE, got {status} with {fetched}"
            );
        }
    }
    past_the_transition_span();
    for id in &ids {
        let (status, fetched) = get_feed(&router, id).await;
        assert_eq!(
            (status, status_of(&fetched)),
            (200, "AVAILABLE"),
            "case \"past the transition span\": feed {id:?} is still AVAILABLE, got {status} \
             with {fetched}"
        );
    }
}

/// Several callers racing to claim ONE freshly created feed: exactly one wins, and every
/// loser is refused because the WINNER holds the feed — not because the service had put
/// it in a status that takes no association.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_on_a_fresh_feed_leave_exactly_one_winner() {
    // Each row: how many callers race for the feed.
    for callers in [2_usize, 4] {
        let case = format!("{callers} concurrent callers");
        let router = router();
        let (_, created) = call(
            &router,
            "POST",
            "/v1/feed",
            Some(json!({"name": "raced-feed", "outputs": [output("crop1", "cropping")]})),
        )
        .await;
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateFeed returns an id, got {created}"))
            .to_string();

        let mut tasks = Vec::new();
        for caller in 0..callers {
            let router = router.clone();
            let id = id.clone();
            let resource = format!("race-media-{caller}");
            tasks.push(tokio::spawn(async move {
                let (status, body) = associate(&router, &id, &resource, json!([]), false).await;
                (resource, status, body)
            }));
        }
        let mut answered = Vec::new();
        for task in tasks {
            answered.push(task.await.expect("each racing caller answered"));
        }

        let winners: Vec<&String> = answered
            .iter()
            .filter(|(_, status, _)| *status == 200)
            .map(|(resource, _, _)| resource)
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: exactly one caller claims the feed, got {answered:?}"
        );
        for (resource, status, body) in &answered {
            if *status == 200 {
                continue;
            }
            assert_eq!(
                *status, 400,
                "case {case:?}: the loser {resource:?} is refused with a 400, got {status} \
                 with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case {case:?}: that 400 is a ValidationException for {resource:?}, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(FEED_ALREADY_ASSOCIATED_MESSAGE),
                "case {case:?}: the loser {resource:?} lost to the winner's claim, got {}",
                body["message"]
            );
        }

        let (status, feed) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&feed)),
            (200, "ACTIVE"),
            "case {case:?}: the raced feed is ACTIVE, got {status} with {feed}"
        );
        assert_eq!(
            feed["association"],
            json!({"associatedResourceName": winners[0]}),
            "case {case:?}: and it is held by the winner, got {feed}"
        );
    }
}

/// A `dryRun` claim on a feed created in the previous request validates and writes
/// NOTHING: it is accepted, and the feed is still unclaimed afterwards.
#[tokio::test]
async fn a_dry_run_claim_on_a_fresh_feed_is_accepted_and_writes_nothing() {
    for case in lifecycle_cases() {
        let label = case.label;
        let router = router();
        let (_, created) = call(&router, "POST", "/v1/feed", Some(case.create.clone())).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let created_outputs = case.create["outputs"].as_array().map(Vec::len).unwrap_or(0);

        let (status, dry) = associate(&router, &id, "dry-media", case.appended.clone(), true).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a dry-run claim on a fresh feed is accepted, got {status} with \
             {dry}"
        );

        let (status, feed) = get_feed(&router, &id).await;
        assert_eq!(
            (status, status_of(&feed)),
            (200, "AVAILABLE"),
            "case {label:?}: the dry run left the feed AVAILABLE, got {status} with {feed}"
        );
        assert_eq!(
            feed.get("association").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {label:?}: and unclaimed, got {feed}"
        );
        assert_eq!(
            feed["outputs"].as_array().map(Vec::len),
            Some(created_outputs),
            "case {label:?}: and appended nothing, got {:?}",
            feed["outputs"]
        );
    }
}
