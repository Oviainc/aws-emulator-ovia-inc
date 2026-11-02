// SPDX-License-Identifier: CC0-1.0

//! What this suite pins, over a SET of racer counts rather than one recorded
//! request: when several callers mutate ONE resource AT THE SAME TIME, the
//! emulator serialises them, so exactly one of them takes effect and every other
//! is refused with one of that operation's own modelled conflict errors — never
//! with a code outside the operation's error set, never with
//! `ResourceAlreadyExistsException` (which this service reserves for a
//! resource-IDENTITY collision), and never by merging two writers' effects.
//!
//! The sibling suites drive these rules SEQUENTIALLY — one call, then the next —
//! which cannot observe what a real client's thread pool does: two calls in
//! flight against one store. These tests run their calls as spawned tasks on a
//! multi-thread runtime against one shared router, so the calls genuinely
//! overlap, and assert the invariant the overlap must leave behind:
//!
//! 1. `AssociateAssets`: of N simultaneous identical `(parent, hierarchy, child)`
//!    calls exactly one succeeds and the child is recorded under the hierarchy
//!    exactly once; each loser is the transient `409
//!    ConflictingOperationException` while the association it collides with is
//!    still settling, or the terminal `400 InvalidRequestException` once that
//!    association is established. Once it IS established, every one of N
//!    simultaneous repeats is the terminal 400.
//! 2. `UpdateAssetModel`: of N simultaneous updates holding ONE `If-Match` ETag
//!    exactly one is accepted and the model carries that writer's definition
//!    whole; each loser is refused `412 PreconditionFailedException` (or the
//!    `409 ConflictingOperationException` the operation also models) and wrote
//!    nothing.
//! 3. `BatchPutAssetPropertyValue`: of N simultaneous writes to one brand-new
//!    `propertyAlias` every write is accepted and they create exactly ONE
//!    disassociated time series, whose single history holds every sample.
//!
//! The harness (`router_with_engine`, `call`, `error_code`, the hierarchy
//! helpers, `call_with_headers`, `describe_e_tag`) is the one the
//! `asset_lifecycle`, `asset_hierarchies` and `hierarchies_etags_and_refusals`
//! suites already define, re-used rather than copied.

use std::future::Future;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_hierarchies::{
    associate, create_asset, create_model, hierarchy_id, CHILD_ALREADY_HAS_A_PARENT,
};
use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::hierarchies_etags_and_refusals::{call_with_headers, describe_e_tag, PAST_ACTIVATION};
use crate::support::now_seconds;

/// How many callers race, per row of every table below: the smallest race there
/// is, an odd count, and two larger ones — the dimension the rule generalises
/// over is the NUMBER of simultaneous callers, so the rows differ only in it.
const RACER_COUNTS: [usize; 4] = [2, 3, 5, 8];

/// One racer's answer: its index, the HTTP status and the parsed body.
pub(crate) type Answer = (usize, u16, Value);

/// Fire `count` requests at `router` AT ONCE and answer one [`Answer`] per
/// racer, ordered by racer index.
///
/// Each request runs in its own spawned task against a CLONE of the router —
/// which shares the one engine and the one service instance — so on the
/// multi-thread runtime these tests declare the calls really do overlap rather
/// than interleaving at a single task's await points.
pub(crate) async fn race<F, Fut>(
    router: &MultiServiceRouter,
    count: usize,
    make: F,
) -> Vec<Answer>
where
    F: Fn(usize, MultiServiceRouter) -> Fut,
    Fut: Future<Output = (u16, Value)> + Send + 'static,
{
    let mut racers = tokio::task::JoinSet::new();
    for index in 0..count {
        let request = make(index, router.clone());
        racers.spawn(async move {
            let (status, body) = request.await;
            (index, status, body)
        });
    }
    let mut answers: Vec<Answer> = Vec::with_capacity(count);
    while let Some(joined) = racers.join_next().await {
        answers.push(joined.expect("a racer task must not panic"));
    }
    answers.sort_by_key(|(index, _, _)| *index);
    answers
}

/// The racer indexes whose call was ACCEPTED (`expected` is that operation's
/// success status).
fn winners(answers: &[Answer], expected: u16) -> Vec<usize> {
    answers
        .iter()
        .filter(|(_, status, _)| *status == expected)
        .map(|(index, _, _)| *index)
        .collect()
}

/// Assert that `answer` is one of the `(status, code)` pairs the operation
/// models for a collision, and in particular not a success and not
/// `ResourceAlreadyExistsException`.
fn assert_modelled_conflict(case: &str, answer: &Answer, allowed: &[(u16, &str)]) {
    let (index, status, body) = answer;
    let code = error_code(body);
    assert!(
        allowed
            .iter()
            .any(|(expected, expected_code)| expected == status && *expected_code == code),
        "case {case:?}: racer {index} must be refused with one of {allowed:?}, \
         got {status} {code:?} {body}"
    );
}

/// Two models — a leaf `Sensor` model and a `Boiler` model with one hierarchy
/// over it — plus the hierarchy's id. Answers
/// `(boilerModelId, sensorModelId, hierarchyId)`.
async fn race_models(router: &MultiServiceRouter) -> (String, String, String) {
    let sensor = create_model(router, json!({ "assetModelName": "Sensor" })).await;
    let boiler = create_model(
        router,
        json!({
            "assetModelName": "Boiler",
            "assetModelHierarchies": [{ "name": "sensors", "childAssetModelId": sensor }],
        }),
    )
    .await;
    let hierarchy = hierarchy_id(router, &boiler, "sensors").await;
    (boiler, sensor, hierarchy)
}

/// The child asset ids the store holds under `(parent, hierarchy)` — the
/// association set itself, read directly because `ListAssociatedAssets` is still
/// a 501 scaffold.
fn stored_children(engine: &engine::InMemoryEngine, parent: &str, hierarchy: &str) -> Vec<String> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store.hierarchy_children(parent, hierarchy).to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_identical_associate_assets_record_the_child_exactly_once() {
    let (router, engine) = router_with_engine();
    let (boiler_model, sensor_model, hierarchy) = race_models(&router).await;

    // The two refusals AssociateAssets models for a collision with an
    // association that is already there: 409 while that association is still
    // settling (retryable), 400 once it is established (terminal).
    let allowed = [
        (409u16, "ConflictingOperationException"),
        (400u16, "InvalidRequestException"),
    ];

    for count in RACER_COUNTS {
        let case = format!("{count} simultaneous identical associations");
        let parent = create_asset(
            &router,
            json!({ "assetName": format!("Boiler-{count}"), "assetModelId": boiler_model }),
        )
        .await;
        let child = create_asset(
            &router,
            json!({ "assetName": format!("Sensor-{count}"), "assetModelId": sensor_model }),
        )
        .await;

        let answers = race(&router, count, |_index, router| {
            let (parent, hierarchy, child) = (parent.clone(), hierarchy.clone(), child.clone());
            async move { associate(&router, &parent, &hierarchy, &child).await }
        })
        .await;

        let won = winners(&answers, 200);
        assert_eq!(
            won.len(),
            1,
            "case {case:?}: exactly one racer may win, got winners {won:?} in {answers:?}"
        );
        for answer in answers.iter().filter(|(index, _, _)| !won.contains(index)) {
            assert_modelled_conflict(&case, answer, &allowed);
        }

        // The single winner is what the store holds: the child is under the
        // hierarchy exactly once, whatever the losers did.
        let children = stored_children(&engine, &parent, &hierarchy);
        assert_eq!(
            children,
            vec![child.clone()],
            "case {case:?}: the association set must hold the child exactly once, got {children:?}"
        );
        // And the implemented reader agrees: an associated child is somebody's
        // child, so it drops out of the TOP_LEVEL listing exactly once.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/assets?filter=TOP_LEVEL&assetModelId={sensor_model}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAssets TOP_LEVEL got {status} {listed}"
        );
        let top_level: Vec<String> = listed["assetSummaries"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !top_level.contains(&child),
            "case {case:?}: the associated child must leave TOP_LEVEL, got {top_level:?}"
        );

        // Once the association the winner recorded has SETTLED, the same N
        // simultaneous calls are every one of them the terminal 400: an
        // established association is a statement about the request that no retry
        // can change.
        tokio::time::sleep(PAST_ACTIVATION).await;
        let settled = race(&router, count, |_index, router| {
            let (parent, hierarchy, child) = (parent.clone(), hierarchy.clone(), child.clone());
            async move { associate(&router, &parent, &hierarchy, &child).await }
        })
        .await;
        for (index, status, body) in &settled {
            assert_eq!(
                *status, 400,
                "case {case:?}: racer {index} against an established association got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(body),
                "InvalidRequestException",
                "case {case:?}: racer {index} refusal code, got {body}"
            );
            // Every loser of a race against an ESTABLISHED association is told
            // the one fact that refuses it: the child already has a parent.
            assert_eq!(
                body["message"].as_str(),
                Some(CHILD_ALREADY_HAS_A_PARENT),
                "case {case:?}: racer {index} expected the message \
                 {CHILD_ALREADY_HAS_A_PARENT:?}, got {body}"
            );
        }
        let children = stored_children(&engine, &parent, &hierarchy);
        assert_eq!(
            children,
            vec![child.clone()],
            "case {case:?}: the refused repeats must change nothing, got {children:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_update_asset_model_lets_exactly_one_etag_holder_win() {
    let (router, _engine) = router_with_engine();

    // The two refusals UpdateAssetModel models for a lost optimistic lock.
    let allowed = [
        (412u16, "PreconditionFailedException"),
        (409u16, "ConflictingOperationException"),
    ];

    for count in RACER_COUNTS {
        let case = format!("{count} simultaneous updates holding one ETag");
        let model = create_model(&router, json!({ "assetModelName": format!("Pump-{count}") })).await;
        // Settle the create's owed transition, so the tag every racer holds is
        // the one a read reports of a settled model.
        tokio::time::sleep(PAST_ACTIVATION).await;
        let shared = describe_e_tag(&router, &model).await;

        // Each racer asks for its OWN name and carries the tag they all read
        // before any of them wrote — the state a race leaves, since no racer
        // re-reads after another has landed.
        let name_of = |index: usize| format!("Pump-{count}-racer-{index}");
        let answers = race(&router, count, |index, router| {
            let (model, tag, name) = (model.clone(), shared.clone(), name_of(index));
            async move {
                let (status, _, body) = call_with_headers(
                    &router,
                    "PUT",
                    &format!("/asset-models/{model}"),
                    Some(json!({ "assetModelName": name })),
                    &[
                        ("if-match", tag.as_str()),
                        ("match-for-version-type", "LATEST"),
                    ],
                )
                .await;
                (status, body)
            }
        })
        .await;

        let won = winners(&answers, 202);
        assert_eq!(
            won.len(),
            1,
            "case {case:?}: exactly one racer may win the optimistic lock, \
             got winners {won:?} in {answers:?}"
        );
        for answer in answers.iter().filter(|(index, _, _)| !won.contains(index)) {
            assert_modelled_conflict(&case, answer, &allowed);
        }

        // The model carries the WINNER's definition whole — never a merge of two
        // racers' requests, and never a loser's name.
        let (status, described) = call(&router, "GET", &format!("/asset-models/{model}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModel got {status} {described}"
        );
        assert_eq!(
            described["assetModelName"].as_str().unwrap_or_default(),
            name_of(won[0]),
            "case {case:?}: the model must carry the winner's name, got {described}"
        );
        // One accepted write means the tag moved exactly once: it is no longer
        // the tag the racers shared, and a replay of any racer's request is
        // refused against it.
        let now_tag = describe_e_tag(&router, &model).await;
        assert_ne!(
            now_tag, shared,
            "case {case:?}: the accepted update must advance the ETag, got {now_tag}"
        );
        let (status, _, refused) = call_with_headers(
            &router,
            "PUT",
            &format!("/asset-models/{model}"),
            Some(json!({ "assetModelName": format!("Pump-{count}-replay") })),
            &[("if-match", shared.as_str())],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: a replay of the shared tag must be refused, got {status} {refused}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_alias_ingestion_creates_exactly_one_time_series() {
    let (router, _engine) = router_with_engine();

    for count in RACER_COUNTS {
        let case = format!("{count} simultaneous writes to one new alias");
        let alias = format!("/plant/line{count}/mixer/temperature");
        // One sample per racer, each at its own second, so the history can be
        // counted and no two writes are the same TQV.
        let base = now_seconds() - 600;

        let answers = race(&router, count, |index, router| {
            let alias = alias.clone();
            async move {
                let (status, body) = call(
                    &router,
                    "POST",
                    "/properties",
                    Some(json!({"entries": [{
                        "entryId": format!("racer-{index}"),
                        "propertyAlias": alias,
                        "propertyValues": [{
                            "value": {"doubleValue": 20.0 + index as f64},
                            "timestamp": {
                                "timeInSeconds": base + index as i64,
                                "offsetInNanos": 0,
                            },
                        }],
                    }]})),
                )
                .await;
                (status, body)
            }
        })
        .await;

        // Ingestion is not a conflict: every racer's write is accepted, because
        // they all address the one stream the alias names.
        for (index, status, body) in &answers {
            assert_eq!(
                *status, 200,
                "case {case:?}: racer {index} got {status} {body}"
            );
            assert_eq!(
                body["errorEntries"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: racer {index} must have no error entries, got {body}"
            );
        }

        // Exactly ONE disassociated time series exists for the alias, however
        // many writes created it.
        let (status, described) = call(
            &router,
            "GET",
            &format!("/timeseries/describe?alias={alias}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTimeSeries got {status} {described}"
        );
        let series_id = described["timeSeriesId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !series_id.is_empty(),
            "case {case:?}: the created series must report a timeSeriesId, got {described}"
        );
        assert_eq!(
            described["dataType"].as_str().unwrap_or_default(),
            "DOUBLE",
            "case {case:?}: the series takes the ingested value's type, got {described}"
        );

        let (status, listed) = call(
            &router,
            "GET",
            &format!("/timeseries?aliasPrefix={alias}&timeSeriesType=DISASSOCIATED"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTimeSeries got {status} {listed}"
        );
        let summaries: Vec<String> = listed["TimeSeriesSummaries"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row["timeSeriesId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            summaries,
            vec![series_id.clone()],
            "case {case:?}: the alias must name exactly one series, got {listed}"
        );

        // And the racers' samples are ONE history on that one series.
        let (status, history) = call(
            &router,
            "GET",
            &format!("/properties/history?propertyAlias={alias}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyValueHistory got {status} {history}"
        );
        let seconds: Vec<i64> = history["assetPropertyValueHistory"]
            .as_array()
            .map(|samples| {
                samples
                    .iter()
                    .map(|sample| sample["timestamp"]["timeInSeconds"].as_i64().unwrap_or(-1))
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<i64> = (0..count).map(|index| base + index as i64).collect();
        assert_eq!(
            seconds, expected,
            "case {case:?}: every racer's sample must land in the one history, got {history}"
        );
    }
}
