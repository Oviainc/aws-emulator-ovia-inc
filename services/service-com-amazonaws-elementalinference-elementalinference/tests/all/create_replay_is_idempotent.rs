// SPDX-License-Identifier: CC0-1.0

//! A REPLAYED create, end to end over the wire: `CreateFeed` and `CreateDictionary`.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the body, the handler runs, and the
//! response is serialized as `restJson1` — so what these tests assert is what a client
//! sees.
//!
//! THE RULE under test: both creates are `@idempotent`
//! (`smithy_query(query="operation",operation="CreateFeed")` and the same for
//! `CreateDictionary`) and neither carries an `@idempotencyToken`, so the request's own
//! members are its key: a request a Region has already served is that one create once
//! more — what an SDK sends when it never saw the first response — and the effect of
//! several identical requests is the effect of ONE. It answers with the resource that
//! create minted (the same id and ARN, reporting the record as it stands), the Region
//! holds one record, and the account has spent one unit of quota. The rule reaches no
//! further than the request it recognises:
//!
//! * a request differing in ANY member is a create of its own and mints its own
//!   resource;
//! * a resource a later call has moved on — updated, associated, released or deleted —
//!   is not one a replay can answer with, so a request arriving then mints afresh.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the resource kind, which optional members the create carries, how many outputs and
//! tags it carries, and which single member the second request differs in.

use crate::harness::{call, call_in_region, enabled_output, output, past_the_transition_span, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::MAX_DICTIONARIES;
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN in the account this emulator serves — the only account whose roles a feed
/// may be created with.
fn own_account_role(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::EMULATOR_ACCOUNT_ID)
}

/// `POST /v1/{kind}` with `body`, returning the status and the parsed response.
async fn create(
    router: &MultiServiceRouter,
    kind: &str,
    body: &Value,
    label: &str,
) -> (u16, Value) {
    let (status, created) = call(router, "POST", &format!("/v1/{kind}"), Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "case {label:?}: creating a {kind} answers 202, got {status} with {created}"
    );
    (status, created)
}

/// The `id` of a created resource.
fn minted_id(created: &Value, label: &str) -> String {
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: a created resource carries an id: {created}"))
        .to_string()
}

/// The ids of every resource of `kind` the Region holds, read through the list
/// operation (`GET /v1/feeds` / `GET /v1/dictionaries`), which is where a second record
/// would show up.
async fn held_ids(router: &MultiServiceRouter, kind: &str, label: &str) -> Vec<String> {
    let plural = match kind {
        "feed" => "feeds",
        "dictionary" => "dictionaries",
        other => panic!("no such resource kind: {other}"),
    };
    let (status, listed) = call(router, "GET", &format!("/v1/{plural}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: listing {plural} answers 200, got {status} with {listed}"
    );
    listed[plural]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: the list carries its items: {listed}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: a summary carries an id: {summary}"))
                .to_string()
        })
        .collect()
}

/// One row of the replay table: a resource kind and the create body submitted for it.
struct CreateCase {
    label: String,
    kind: &'static str,
    body: Value,
}

/// A row per shape a create can take, for both resource kinds: the smallest admissible
/// request, one carrying every optional member, and ones differing in how many outputs,
/// entries and tags they carry.
fn replay_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "dictionary, name and language only".to_string(),
            kind: "dictionary",
            body: json!({"name": "replaydict", "language": "eng"}),
        },
        CreateCase {
            label: "dictionary with an entries payload".to_string(),
            kind: "dictionary",
            body: json!({
                "name": "replaydict-entries",
                "language": "fra",
                "entries": "Hawks\nFalcons\n"
            }),
        },
        CreateCase {
            label: "dictionary with an empty entries payload".to_string(),
            kind: "dictionary",
            body: json!({"name": "replaydict-empty", "language": "por", "entries": ""}),
        },
        CreateCase {
            label: "dictionary with entries and three tags".to_string(),
            kind: "dictionary",
            body: json!({
                "name": "replaydict-tagged",
                "language": "deu",
                "entries": "Bundesliga\n",
                "tags": {"team": "subtitles", "env": "test", "owner": "qa"}
            }),
        },
        CreateCase {
            label: "feed with one output".to_string(),
            kind: "feed",
            body: json!({"name": "replayfeed", "outputs": [enabled_output("crop1", "cropping")]}),
        },
        CreateCase {
            label: "feed with an output of every modelled kind".to_string(),
            kind: "feed",
            body: json!({
                "name": "replayfeed-every-kind",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("clip1", "clipping"),
                    output("subs1", "subtitling", "DISABLED"),
                    enabled_output("meta1", "contextualMetadata")
                ]
            }),
        },
        CreateCase {
            label: "feed with an access role and tags".to_string(),
            kind: "feed",
            body: json!({
                "name": "replayfeed-role",
                "accessRoleArn": own_account_role("replay-feed-role"),
                "outputs": [enabled_output("crop1", "cropping")],
                "tags": {"team": "broadcast"}
            }),
        },
    ]
}

/// A request a Region has already served answers with the resource that create minted,
/// and the Region holds exactly one record for it however many times it arrives.
#[tokio::test]
async fn a_replayed_create_answers_with_the_resource_the_first_create_minted() {
    for case in replay_cases() {
        let (label, kind) = (&case.label, case.kind);
        let router = router();
        let (_, first) = create(&router, kind, &case.body, label).await;
        let id = minted_id(&first, label);

        // Three arrivals of the one request, which is what an SDK that retried twice
        // sends: each answers with the resource the first minted.
        for attempt in 2..=3 {
            let (_, replayed) = create(&router, kind, &case.body, label).await;
            assert_eq!(
                replayed["id"],
                json!(id),
                "case {label:?} attempt {attempt}: the replay answers with the resource \
                 already minted, got {replayed}"
            );
            assert_eq!(
                replayed["arn"], first["arn"],
                "case {label:?} attempt {attempt}: the replay reports the same ARN, got {:?}",
                replayed["arn"]
            );
            assert_eq!(
                replayed["name"], first["name"],
                "case {label:?} attempt {attempt}: the replay reports the same name, got {:?}",
                replayed["name"]
            );
            assert_eq!(
                replayed["tags"], first["tags"],
                "case {label:?} attempt {attempt}: the replay reports the same tags, got {:?}",
                replayed["tags"]
            );
        }

        // ONE stored record: the retries neither added a second resource nor orphaned
        // the first.
        let held = held_ids(&router, kind, label).await;
        assert_eq!(
            held,
            vec![id.clone()],
            "case {label:?}: the Region holds exactly the one record the create minted, \
             got {held:?}"
        );

        // And that record is the one the caller was told about: it reads back by id.
        let (status, fetched) = call(&router, "GET", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the id the replay reported reads back, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["id"],
            json!(id),
            "case {label:?}: the read reports the id it was asked for, got {fetched}"
        );
        assert_eq!(
            fetched["name"], first["name"],
            "case {label:?}: the stored record is the one the create reported, got {fetched}"
        );
    }
}

/// One row of the difference table: a baseline create and a second create differing in
/// exactly one member.
struct DifferenceCase {
    label: &'static str,
    kind: &'static str,
    first: Value,
    second: Value,
}

/// A row per member a create carries, for both resource kinds: a second request that
/// differs in any one of them is a create of its own, so recognition is not keyed on
/// the name alone.
fn difference_cases() -> Vec<DifferenceCase> {
    let dictionary = json!({"name": "diffdict", "language": "eng", "entries": "Hawks\n"});
    let feed = json!({
        "name": "difffeed",
        "outputs": [enabled_output("crop1", "cropping")],
        "tags": {"team": "broadcast"}
    });
    let with = |body: &Value, member: &str, value: Value| -> Value {
        let mut body = body.clone();
        body.as_object_mut()
            .expect("a create body is an object")
            .insert(member.to_string(), value);
        body
    };
    let without = |body: &Value, member: &str| -> Value {
        let mut body = body.clone();
        body.as_object_mut()
            .expect("a create body is an object")
            .remove(member);
        body
    };
    vec![
        DifferenceCase {
            label: "dictionary under another name",
            kind: "dictionary",
            first: dictionary.clone(),
            second: with(&dictionary, "name", json!("diffdict2")),
        },
        DifferenceCase {
            label: "dictionary in another language",
            kind: "dictionary",
            first: dictionary.clone(),
            second: with(&dictionary, "language", json!("ita")),
        },
        DifferenceCase {
            label: "dictionary with other entries",
            kind: "dictionary",
            first: dictionary.clone(),
            second: with(&dictionary, "entries", json!("Falcons\n")),
        },
        DifferenceCase {
            label: "dictionary with no entries at all",
            kind: "dictionary",
            first: dictionary.clone(),
            second: without(&dictionary, "entries"),
        },
        DifferenceCase {
            label: "dictionary with a tag added",
            kind: "dictionary",
            first: dictionary.clone(),
            second: with(&dictionary, "tags", json!({"team": "subtitles"})),
        },
        DifferenceCase {
            label: "feed under another name",
            kind: "feed",
            first: feed.clone(),
            second: with(&feed, "name", json!("difffeed2")),
        },
        DifferenceCase {
            label: "feed whose output is named differently",
            kind: "feed",
            first: feed.clone(),
            second: with(&feed, "outputs", json!([enabled_output("crop2", "cropping")])),
        },
        DifferenceCase {
            label: "feed whose output carries another config kind",
            kind: "feed",
            first: feed.clone(),
            second: with(&feed, "outputs", json!([enabled_output("crop1", "clipping")])),
        },
        DifferenceCase {
            label: "feed whose output is DISABLED",
            kind: "feed",
            first: feed.clone(),
            second: with(
                &feed,
                "outputs",
                json!([output("crop1", "cropping", "DISABLED")]),
            ),
        },
        DifferenceCase {
            label: "feed carrying a second output",
            kind: "feed",
            first: feed.clone(),
            second: with(
                &feed,
                "outputs",
                json!([
                    enabled_output("crop1", "cropping"),
                    enabled_output("clip1", "clipping")
                ]),
            ),
        },
        DifferenceCase {
            label: "feed with an access role",
            kind: "feed",
            first: feed.clone(),
            second: with(
                &feed,
                "accessRoleArn",
                json!(own_account_role("difference-role")),
            ),
        },
        DifferenceCase {
            label: "feed with another tag value",
            kind: "feed",
            first: feed.clone(),
            second: with(&feed, "tags", json!({"team": "subtitles"})),
        },
        DifferenceCase {
            label: "feed with no tags at all",
            kind: "feed",
            first: feed.clone(),
            second: without(&feed, "tags"),
        },
    ]
}

/// A create differing in any ONE member is a request of its own, and mints its own
/// resource.
#[tokio::test]
async fn a_create_differing_in_any_one_member_mints_its_own_resource() {
    for case in difference_cases() {
        let (label, kind) = (case.label, case.kind);
        let router = router();
        let (_, first) = create(&router, kind, &case.first, label).await;
        let (_, second) = create(&router, kind, &case.second, label).await;
        let (first_id, second_id) = (minted_id(&first, label), minted_id(&second, label));
        assert_ne!(
            first_id, second_id,
            "case {label:?}: a create differing in one member mints its own resource, got \
             {first_id:?} twice"
        );

        let mut held = held_ids(&router, kind, label).await;
        held.sort();
        let mut expected = vec![first_id, second_id];
        expected.sort();
        assert_eq!(
            held, expected,
            "case {label:?}: the Region holds both resources, got {held:?}"
        );
    }
}

/// A dictionary a later call has moved on is not a record a repeated create answers
/// with: the request mints afresh, whether the record was updated or has gone.
#[tokio::test]
async fn a_dictionary_that_has_moved_on_is_not_answered_to_a_repeated_create() {
    let body = json!({"name": "movedondict", "language": "eng", "entries": "Hawks\n"});

    // Updated: the stored record is no longer what the request asks for.
    let updated_router = router();
    let (_, created) = create(&updated_router, "dictionary", &body, "updated").await;
    let id = minted_id(&created, "updated");
    let (status, updated) = call(
        &updated_router,
        "PATCH",
        &format!("/v1/dictionary/{id}"),
        Some(json!({"entries": "Falcons\n"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"updated\": UpdateDictionary answers 200, got {status} with {updated}"
    );
    let (_, after_update) = create(&updated_router, "dictionary", &body, "updated").await;
    assert_ne!(
        minted_id(&after_update, "updated"),
        id,
        "case \"updated\": a create arriving after the dictionary was updated mints its own, \
         got {after_update}"
    );

    // Gone: the Region holds no record for the request to answer with.
    let deleted_router = router();
    let (_, created) = create(&deleted_router, "dictionary", &body, "deleted").await;
    let id = minted_id(&created, "deleted");
    let (status, deleted) =
        call(&deleted_router, "DELETE", &format!("/v1/dictionary/{id}"), None).await;
    assert_eq!(
        status, 202,
        "case \"deleted\": DeleteDictionary answers 202, got {status} with {deleted}"
    );
    // Inside the DELETING window the record is already on its way out, and once the
    // removal falls due it is gone: neither is a record a create can be answered with.
    let (_, during_deletion) = create(&deleted_router, "dictionary", &body, "deleting").await;
    let during_id = minted_id(&during_deletion, "deleting");
    assert_ne!(
        during_id, id,
        "case \"deleting\": a create arriving while the dictionary is being deleted mints its \
         own, got {during_deletion}"
    );
    past_the_transition_span();
    let (_, after_removal) = create(&deleted_router, "dictionary", &body, "deleted").await;
    let after_id = minted_id(&after_removal, "deleted");
    assert!(
        after_id != id && after_id == during_id,
        "case \"deleted\": once the removal fell due the only record left is the one minted \
         during the deletion, got {after_removal}"
    );
}

/// A feed a later call has moved on is not a record a repeated create answers with
/// either: a feed that has been claimed is no longer the feed that create left behind.
#[tokio::test]
async fn a_feed_that_has_been_associated_is_not_answered_to_a_repeated_create() {
    let body = json!({
        "name": "movedonfeed",
        "outputs": [enabled_output("crop1", "cropping")]
    });
    let router = router();
    let (_, created) = create(&router, "feed", &body, "associated").await;
    let id = minted_id(&created, "associated");

    let (status, associated) = call(
        &router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        // No further outputs: the claim alone is what moves the feed on.
        Some(json!({"associatedResourceName": "source-media", "outputs": []})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associated\": AssociateFeed answers 200, got {status} with {associated}"
    );

    let (_, after) = create(&router, "feed", &body, "associated").await;
    let after_id = minted_id(&after, "associated");
    assert_ne!(
        after_id, id,
        "case \"associated\": a create arriving after the feed was claimed mints its own, got \
         {after}"
    );
    assert_eq!(
        after["association"],
        Value::Null,
        "case \"associated\": the feed it mints is unclaimed, got {after}"
    );
}

/// A replay costs the account no further quota: it is recognised before the quota is
/// consulted, so a retry at the ceiling still answers with the dictionary it minted
/// while a create of its own is refused.
#[tokio::test]
async fn a_replayed_create_at_the_quota_still_answers_the_resource_it_minted() {
    let router = router();
    let first_body = json!({"name": "quotareplay0", "language": "eng"});
    let (_, first) = create(&router, "dictionary", &first_body, "at the quota").await;
    let first_id = minted_id(&first, "at the quota");
    for index in 1..MAX_DICTIONARIES {
        let body = json!({"name": format!("quotareplay{index}"), "language": "eng"});
        create(&router, "dictionary", &body, "at the quota").await;
    }

    let (_, replayed) = create(&router, "dictionary", &first_body, "at the quota").await;
    assert_eq!(
        replayed["id"],
        json!(first_id),
        "case \"at the quota\": the replay still answers with the dictionary it minted, got \
         {replayed}"
    );

    let (status, refused) = call(
        &router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": "onepastthequota", "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"at the quota\": a create of its own is still refused past the quota, got \
         {status} with {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case \"at the quota\": that 402 is a ServiceQuotaExceededException, got {refused}"
    );
}

/// The Region owns its resources, so a request served by one Region's endpoint is not a
/// replay for another's: the same create in a second Region mints that Region's own
/// resource.
#[tokio::test]
async fn the_same_create_in_another_region_mints_that_regions_own_resource() {
    let router = router();
    let body = json!({"name": "regiondict", "language": "eng"});
    let mut ids: Vec<String> = Vec::new();
    for region in ["us-west-2", "eu-west-1", "ap-south-1"] {
        let (status, created) =
            call_in_region(&router, region, "POST", "/v1/dictionary", Some(body.clone())).await;
        assert_eq!(
            status, 202,
            "case {region:?}: the create is accepted, got {status} with {created}"
        );
        let id = minted_id(&created, region);
        assert!(
            !ids.contains(&id),
            "case {region:?}: each Region mints its own dictionary, got the already-minted \
             {id:?}"
        );
        // And within that Region the request is still one create: the replay answers
        // with the record just minted.
        let (status, replayed) =
            call_in_region(&router, region, "POST", "/v1/dictionary", Some(body.clone())).await;
        assert_eq!(
            status, 202,
            "case {region:?}: the replay is accepted, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["id"],
            json!(id),
            "case {region:?}: the replay answers with that Region's own record, got {replayed}"
        );
        ids.push(id);
    }
    assert_eq!(
        ids.len(),
        3,
        "three Regions each mint one dictionary, got {ids:?}"
    );
}
