// SPDX-License-Identifier: CC0-1.0

//! What a tagging write does to the resource's own last-modified stamp, over awsJson1_0.
//!
//! The rule under test: `TagResource` is `@idempotent`
//! (`smithy_query(query="operation",operation="TagResource",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`
//! reports `idempotent: true`), so the call's end state is what it asks for and a call
//! whose merge leaves the stored tag map exactly as it already was modified the resource
//! not at all — its `modifiedAt` stays where it was. A call that really changes a tag
//! does stamp it, with the current clock.
//!
//! The table spans what the rule generalises over: how many tags the resource carries
//! (one, several, and the 200 the modelled `RequestTagMap` admits), which of them the
//! request repeats (all, a subset), the values involved (ordinary, empty — the modelled
//! `TagValue` is len 0..256, so the empty string is a value — the longest the model
//! allows, and one differing from the stored value only in case) and whether the merge
//! adds a key, overwrites one or changes nothing. The no-op rows and the changing rows
//! sit in one table so what separates them is the merge's result and not the shape of
//! the request.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads the resource back
//! through `GetEnvironment` and `ListTagsForResource`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, emulator, settle, Emulator};

/// The modelled bounds the rows below are built against, from
/// `smithy_query(query="shape",shape="RequestTagMap,TagValue")`: a tag map carries at
/// most 200 entries and a tag value is between 0 and 256 characters.
const MAX_TAGS_PER_RESOURCE: usize = 200;
const MAX_TAG_VALUE_LEN: usize = 256;

/// How far into the past a baseline stamp is moved before the call under test, so a
/// stamp written from the current clock is distinguishable from the baseline whatever
/// the clock's resolution — a minute, far longer than a test takes.
const BACKDATE_MS: i64 = 60_000;

/// A stored or requested tag map, from pairs.
fn map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A tag map as the wire carries it.
fn wire_tags(tags: &BTreeMap<String, String>) -> Value {
    let mut members = serde_json::Map::new();
    for (key, value) in tags {
        members.insert(key.clone(), json!(value));
    }
    Value::Object(members)
}

/// One row: what the resource already carries, what the request asks for, and whether
/// that merge is a modification of the resource.
struct Row {
    case: String,
    stored: BTreeMap<String, String>,
    request: BTreeMap<String, String>,
    expect_stamp: bool,
}

/// The longest value the modelled `TagValue` admits.
fn longest_value() -> String {
    "v".repeat(MAX_TAG_VALUE_LEN)
}

/// The most tags one resource can carry, as a map.
fn full_tag_map() -> BTreeMap<String, String> {
    (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

fn rows() -> Vec<Row> {
    let longest = longest_value();
    let full = full_tag_map();
    // One entry held back out of a full map, so the row below asks for the merge that
    // lands exactly on the 200-tag bound.
    let mut full_minus_one = full.clone();
    let held_back_key = format!("key-{}", MAX_TAGS_PER_RESOURCE - 1);
    let held_back_value = full_minus_one
        .remove(&held_back_key)
        .expect("the full map carries the held-back key");
    vec![
        // Nothing changes: the merge writes back the map that was already stored.
        Row {
            case: "the only tag replayed with its own value".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("team", "platform")]),
            expect_stamp: false,
        },
        Row {
            case: "every one of several tags replayed".to_string(),
            stored: map(&[("team", "platform"), ("cost", "42"), ("env", "prod")]),
            request: map(&[("team", "platform"), ("cost", "42"), ("env", "prod")]),
            expect_stamp: false,
        },
        Row {
            case: "a subset of the stored tags replayed".to_string(),
            stored: map(&[("team", "platform"), ("cost", "42"), ("env", "prod")]),
            request: map(&[("cost", "42")]),
            expect_stamp: false,
        },
        Row {
            case: "a tag whose value is the empty string replayed".to_string(),
            stored: map(&[("probe", ""), ("team", "platform")]),
            request: map(&[("probe", "")]),
            expect_stamp: false,
        },
        Row {
            case: "a tag carrying the longest value the model allows, replayed".to_string(),
            stored: map(&[("team", longest.as_str())]),
            request: map(&[("team", longest.as_str())]),
            expect_stamp: false,
        },
        Row {
            case: "the most tags a resource can carry, all replayed".to_string(),
            stored: full.clone(),
            request: full.clone(),
            expect_stamp: false,
        },
        // Something changes: the merge leaves a map the resource did not have.
        Row {
            case: "a key the resource does not carry".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("cost", "42")]),
            expect_stamp: true,
        },
        Row {
            case: "a stored key with a different value".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("team", "infra")]),
            expect_stamp: true,
        },
        Row {
            case: "a stored key with a value differing only in case".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("team", "Platform")]),
            expect_stamp: true,
        },
        Row {
            case: "a stored key replayed beside a new one".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("team", "platform"), ("cost", "42")]),
            expect_stamp: true,
        },
        Row {
            case: "a stored value replaced by the empty string".to_string(),
            stored: map(&[("team", "platform")]),
            request: map(&[("team", "")]),
            expect_stamp: true,
        },
        Row {
            case: "a stored empty value replaced by a real one".to_string(),
            stored: map(&[("probe", "")]),
            request: map(&[("probe", "ready")]),
            expect_stamp: true,
        },
        Row {
            case: "the one key missing from an almost-full map".to_string(),
            stored: full_minus_one,
            request: map(&[(held_back_key.as_str(), held_back_value.as_str())]),
            expect_stamp: true,
        },
    ]
}

/// Create an environment carrying `tags` and answer its `(environmentId, environmentArn)`.
async fn create_tagged(
    router: &Emulator,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> (String, String) {
    let mut request = router.create_request(Some(name));
    // The modelled `tags` map is len 1..200, so a request carries the member only when
    // the row gives the environment tags to start with.
    if !tags.is_empty() {
        request["tags"] = wire_tags(tags);
    }
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEnvironment expected 200, got {status} {body}"
    );
    let environment = &body["environment"];
    let id = environment["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();
    let arn = environment["environmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentArn in {body}"))
        .to_string();
    (id, arn)
}

/// The `modifiedAt` this service's own store holds for one environment, in epoch
/// milliseconds — the stamp `GetEnvironment` reports.
fn stored_modified_at(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .map(|environment| environment.modified_at_ms)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"))
}

/// Move one environment's stamp a known span into the past and answer the instant it now
/// carries, so a stamp written from the current clock is telling whatever the clock's
/// resolution.
fn backdate_modified_at(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let backdated = evs::state::now_ms() - BACKDATE_MS;
    let mut store = state.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    environment.modified_at_ms = backdated;
    backdated
}

/// The `modifiedAt` `GetEnvironment` reports for one environment, as the wire carries it.
async fn reported_modified_at(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment expected 200, got {status} {body}"
    );
    body["environment"]["modifiedAt"].clone()
}

/// Tag one resource, failing the test if the call is refused.
async fn tag(router: &Emulator, case: &str, resource_arn: &str, tags: Value) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({ "resourceArn": resource_arn, "tags": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource expected 200, got {status} {body}"
    );
}

/// The tags `ListTagsForResource` reports for one resource ARN.
async fn reported_tags(router: &Emulator, case: &str, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {body}"
    );
    body["tags"].clone()
}

#[tokio::test]
async fn tag_resource_stamps_modified_at_only_when_the_merge_changes_a_tag() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.case;
        let (id, arn) = create_tagged(
            &router,
            case,
            &format!("tag-stamp-{index}"),
            &row.stored,
        )
        .await;
        // Past the create transition, so the only thing that can move the stamp below is
        // the call under test.
        settle(&router);
        let baseline_ms = backdate_modified_at(&router, case, &id);
        let baseline_wire = reported_modified_at(&router, case, &id).await;

        let called_at_ms = evs::state::now_ms();
        tag(&router, case, &arn, wire_tags(&row.request)).await;

        // Whatever the stamp did, the merge itself is the documented one: "If existing
        // tags on a resource are not specified in the request parameters, they aren't
        // changed", and a key the request repeats takes the request's value.
        let mut merged = row.stored.clone();
        merged.extend(row.request.clone());
        let tags = reported_tags(&router, case, &arn).await;
        assert_eq!(
            tags,
            wire_tags(&merged),
            "case {case:?}: expected the merged tags {}, got {tags}",
            wire_tags(&merged)
        );

        let after_ms = stored_modified_at(&router, case, &id);
        let after_wire = reported_modified_at(&router, case, &id).await;
        if row.expect_stamp {
            assert!(
                after_ms >= called_at_ms,
                "case {case:?}: a call that changed a tag must stamp modifiedAt from the \
                 current clock, expected at least {called_at_ms}, got {after_ms}"
            );
            assert_ne!(
                after_wire, baseline_wire,
                "case {case:?}: a call that changed a tag must move the reported \
                 modifiedAt away from {baseline_wire}, got {after_wire}"
            );
        } else {
            assert_eq!(
                after_ms, baseline_ms,
                "case {case:?}: a call that changed no tag must leave modifiedAt at \
                 {baseline_ms}, got {after_ms}"
            );
            assert_eq!(
                after_wire, baseline_wire,
                "case {case:?}: a call that changed no tag must leave the reported \
                 modifiedAt at {baseline_wire}, got {after_wire}"
            );
        }
    }
}

#[tokio::test]
async fn the_idempotent_replay_of_a_tag_resource_call_leaves_modified_at_where_it_was() {
    // The same rule read off the wire alone, with no store surgery: three calls on one
    // environment, each far enough apart in wall-clock time that a stamp would show.
    let router = emulator();
    let (id, arn) = create_tagged(&router, "replay", "tag-stamp-replay", &map(&[])).await;
    settle(&router);

    tag(
        &router,
        "the first tagging",
        &arn,
        json!({ "team": "platform" }),
    )
    .await;
    let first = reported_modified_at(&router, "after the first tagging", &id).await;
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    // The idempotent replay: the stored map is already exactly what the request asks
    // for, so the resource is not modified and its stamp does not move.
    tag(&router, "the replay", &arn, json!({ "team": "platform" })).await;
    let replayed = reported_modified_at(&router, "after the replay", &id).await;
    assert_eq!(
        replayed, first,
        "case the idempotent replay: expected modifiedAt {first}, got {replayed}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    // And a call that really changes the value does stamp it, so the assertion above is
    // not simply reading a stamp nothing ever moves.
    tag(&router, "the real change", &arn, json!({ "team": "infra" })).await;
    let changed = reported_modified_at(&router, "after the real change", &id).await;
    assert!(
        changed.as_f64().unwrap_or_default() > first.as_f64().unwrap_or_default(),
        "case a tagging that changes a value: expected modifiedAt past {first}, got {changed}"
    );
}
