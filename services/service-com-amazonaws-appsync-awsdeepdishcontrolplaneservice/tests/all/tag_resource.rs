// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over HTTP.
//!
//! Five rules are asserted here, each over a TABLE or a class of inputs rather than
//! over the single request that exposed the defect:
//!
//! 1. a tag write MERGES the submitted pairs into the map the resource holds — a key
//!    the resource already carries takes the submitted value, a key it does not carry
//!    is added, a key the request does not name is untouched — and the merged map is
//!    what the tag listing and the resource's own `tags` member report afterwards. The
//!    table spans what the rule generalises over: additions, overwrites and both at
//!    once, the empty value in either direction, key length on both modelled bounds,
//!    value length at its modelled maximum, the character classes `TagKey` and
//!    `TagValue` admit, and pair counts from one to the modelled maximum of 50;
//! 2. a write reaches only the resource its ARN names, so a second resource in the
//!    same store keeps its own map;
//! 3. a well-formed resource ARN this account holds no resource for is a 404
//!    `NotFoundException` — over a never-created ARN, a deleted resource's ARN, and
//!    the ARN of a resource that exists under a different Region or account;
//! 4. a tag key outside `TagKey`'s rule (too short or too long, under the reserved
//!    `aws:` prefix, or drawn from outside its alphabet) is a 400 `BadRequestException`
//!    and leaves the stored map exactly as it was;
//! 5. a write that would leave the resource holding more than the per-resource tag
//!    quota is a 429 `LimitExceededException` and changes nothing, while a write that
//!    only overwrites keys already there is served at the quota.
//!
//! An API of either resource family (a `GraphqlApi` and an `Api`) is published under
//! the one ARN shape `ResourceArn` admits, so rule 1 is also asserted on an Event API.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, the `@httpLabel` ARN binding, restJson1 (de)serialization, the
//! handler and the shared engine together. The harness is the error-contract suite's
//! `call` (it reads the error-type header and the message) over the lifecycle suite's
//! router and create helper, with the tag suite's label encoder and modelled bounds.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, Answer};
use crate::graphql_api_lifecycle::{create_api, router, REGION};
use crate::resource_tags::{
    encode_label, ABSENT_API_ID, ACCOUNT, TAG_COUNT_MAX, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};

/// `TagKey`'s modelled minimum length. Source: `smithy_query(query="shape",
/// shape="TagKey")` — `@length` min 1, max 128.
pub(crate) const TAG_KEY_MIN_LEN: usize = 1;

/// The status a tag WRITE answers: 204, with no body at all. `TagResourceResponse` and
/// `UntagResourceResponse` model no members, and the tagging surface answers a served
/// write as no content rather than as an empty body.
pub(crate) const TAG_WRITE_STATUS: u16 = 204;

/// The fixed text the tagging surface answers for a resource that is absent: it
/// addresses a RESOURCE by ARN, so the refusal names no resource kind.
pub(crate) const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The fixed text AppSync answers for a tag key under the prefix it reserves — one
/// sentence of its own, naming the prefix and the comparison rather than reciting the
/// whole tag-key grammar.
pub(crate) const RESERVED_TAG_KEY_REFUSAL: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// `POST /v1/tags/{resourceArn}` carrying `tags`, as a client issues it.
pub(crate) async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/tags/{}", encode_label(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// `GET /v1/tags/{resourceArn}`, the read this suite proves a write through.
pub(crate) async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/v1/tags/{}", encode_label(arn)), None).await
}

/// `pairs` as the modelled `TagMap` JSON object.
pub(crate) fn tag_map(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// A borrowed pair table as an owned one.
pub(crate) fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct pairs, numbered from `first` so two runs can be disjoint.
pub(crate) fn numbered_pairs(first: usize, count: usize) -> Vec<(String, String)> {
    (first..first + count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// One row of the merge table: the pairs the resource is created with, and the pairs
/// one `TagResource` call submits for it.
#[derive(Debug)]
struct Row {
    label: String,
    initial: Vec<(String, String)>,
    submitted: Vec<(String, String)>,
}

impl Row {
    /// The map the merge rule says the resource ends up with: the initial pairs with
    /// every submitted pair written over them.
    fn expected(&self) -> Value {
        let mut merged: std::collections::BTreeMap<String, String> =
            self.initial.iter().cloned().collect();
        for (key, value) in &self.submitted {
            merged.insert(key.clone(), value.clone());
        }
        let mut map = Map::new();
        for (key, value) in merged {
            map.insert(key, Value::String(value));
        }
        Value::Object(map)
    }

    /// The `CreateGraphqlApi` body that puts this row's initial pairs on a resource.
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": "tagged", "authenticationType": "API_KEY" });
        if !self.initial.is_empty() {
            body["tags"] = tag_map(&self.initial);
        }
        body
    }
}

/// The merge table: every class the write rule generalises over.
fn rows() -> Vec<Row> {
    let long_key = "k".repeat(TAG_KEY_MAX_LEN);
    let short_key = "s".repeat(TAG_KEY_MIN_LEN);
    let long_value = "v".repeat(TAG_VALUE_MAX_LEN);
    let mut rows = vec![
        ("a pair added to an untagged resource", vec![], pairs(&[("env", "prod")])),
        (
            "a disjoint pair added to a tagged resource",
            pairs(&[("env", "prod")]),
            pairs(&[("team", "search")]),
        ),
        (
            "an existing key overwritten",
            pairs(&[("env", "prod")]),
            pairs(&[("env", "staging")]),
        ),
        // `TagValue`'s modelled minimum length is 0, so the empty string is a value a
        // key can hold — and holding it is different from not holding the key at all.
        (
            "an existing key overwritten with the empty value",
            pairs(&[("env", "prod")]),
            pairs(&[("env", "")]),
        ),
        (
            "an empty value overwritten with a non-empty one",
            pairs(&[("env", "")]),
            pairs(&[("env", "prod")]),
        ),
        (
            "a new key carrying the empty value",
            pairs(&[("env", "prod")]),
            pairs(&[("audit", "")]),
        ),
        (
            "an overwrite and an addition in one call",
            pairs(&[("env", "prod"), ("team", "search")]),
            pairs(&[("env", "dev"), ("owner", "platform")]),
        ),
        // `TagKey`'s modelled length bounds.
        (
            "a key at the minimum length of 1",
            vec![],
            pairs(&[(short_key.as_str(), "v")]),
        ),
        (
            "a key at the maximum length of 128",
            vec![],
            pairs(&[(long_key.as_str(), "v")]),
        ),
        // `TagValue`'s modelled maximum length.
        (
            "a value at the maximum length of 256",
            vec![],
            pairs(&[("long", long_value.as_str())]),
        ),
        // The character classes the modelled patterns admit: `TagKey`'s
        // `[ a-zA-Z+-=._:/]` (whose `+-=` range also admits the digits) and
        // `TagValue`'s `[\s\w+-=\.:/@]`.
        (
            "a key spanning the admitted character classes",
            vec![],
            pairs(&[("Cost.center/sub:team key-01+2=3", "v")]),
        ),
        (
            "a value spanning the admitted character classes",
            vec![],
            pairs(&[("punctuation", "a b+c-d=e.f:g/h@i_1")]),
        ),
        // The reserved prefix is `aws:`, so a key that merely begins with those
        // letters is an ordinary key.
        (
            "a key beginning with aws outside the reserved prefix",
            vec![],
            pairs(&[("awsome", "yes")]),
        ),
    ]
    .into_iter()
    .map(|(label, initial, submitted)| Row {
        label: label.to_string(),
        initial,
        submitted,
    })
    .collect::<Vec<Row>>();

    // Pair COUNT: one call carrying the modelled maximum, and a resource filled to
    // that maximum by two calls.
    rows.push(Row {
        label: format!("the modelled maximum of {TAG_COUNT_MAX} pairs in one call"),
        initial: vec![],
        submitted: numbered_pairs(0, TAG_COUNT_MAX),
    });
    rows.push(Row {
        label: format!("two halves of the {TAG_COUNT_MAX}-pair maximum"),
        initial: numbered_pairs(0, TAG_COUNT_MAX / 2),
        submitted: numbered_pairs(TAG_COUNT_MAX / 2, TAG_COUNT_MAX / 2),
    });
    rows
}

/// Rule 1: the write merges into the stored map, and the stored map is what every
/// later read reports.
#[tokio::test]
async fn a_tag_write_merges_into_the_map_the_resource_holds() {
    for row in rows() {
        let label = row.label.clone();
        let router = router();
        let created = create_api(&router, row.create_body()).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the created API carries an ARN: {created:?}"))
            .to_string();
        let api_id = created["apiId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the created API carries an id: {created:?}"))
            .to_string();

        let written = tag_resource(&router, &arn, tag_map(&row.submitted)).await;
        assert_eq!(
            written.status, TAG_WRITE_STATUS,
            "case {label}: expected TagResource {TAG_WRITE_STATUS}, got {} {} {:?}",
            written.status, written.error_type, written.body
        );
        // `TagResourceResponse` models no members, so the answer carries none.
        assert!(
            written.body.is_null()
                || written
                    .body
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
            "case {label}: expected an answer with no members, got {:?}",
            written.body
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {label}: expected ListTagsForResource 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body["tags"],
            row.expected(),
            "case {label} ({row:?}): expected the merged tag map, got {:?}",
            listed.body
        );

        // The resource's own `tags` member reads the same map, so the two answers
        // cannot disagree about what was written.
        let fetched = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            fetched.body["graphqlApi"]["tags"],
            row.expected(),
            "case {label}: expected the API to carry the merged tags, got {:?}",
            fetched.body
        );

        // Writing the same pairs again is a no-op: a merge of a map into itself.
        let again = tag_resource(&router, &arn, tag_map(&row.submitted)).await;
        assert_eq!(
            again.status, TAG_WRITE_STATUS,
            "case {label}: expected a repeated TagResource {TAG_WRITE_STATUS}, got {} {:?}",
            again.status, again.body
        );
        let relisted = list_tags(&router, &arn).await;
        assert_eq!(
            relisted.body["tags"],
            row.expected(),
            "case {label}: expected the repeated write to leave the same map, got {:?}",
            relisted.body
        );
    }
}

/// Rule 1, on the other resource family: an `Api` is published under the same ARN
/// shape, so it is tagged and listed through the same route.
#[tokio::test]
async fn an_event_api_is_tagged_through_the_same_route() {
    let router = router();
    let create = call(
        &router,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "events",
            "tags": { "env": "prod" },
            "eventConfig": {
                "authProviders": [{ "authType": "API_KEY" }],
                "connectionAuthModes": [{ "authType": "API_KEY" }],
                "defaultPublishAuthModes": [{ "authType": "API_KEY" }],
                "defaultSubscribeAuthModes": [{ "authType": "API_KEY" }],
            },
        })),
    )
    .await;
    assert_eq!(
        create.status, 200,
        "case create an Event API: expected 200, got {} {:?}",
        create.status, create.body
    );
    let arn = create.body["api"]["apiArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the created Event API carries an ARN: {:?}", create.body))
        .to_string();
    let api_id = create.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let written = tag_resource(&router, &arn, json!({ "env": "staging", "team": "events" })).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case tag an Event API: expected {TAG_WRITE_STATUS}, got {} {} {:?}",
        written.status, written.error_type, written.body
    );

    let expected = json!({ "env": "staging", "team": "events" });
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"], expected,
        "case tag an Event API: expected the merged tag map, got {:?}",
        listed.body
    );
    let fetched = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        fetched.body["api"]["tags"], expected,
        "case tag an Event API: expected the API to carry the merged tags, got {:?}",
        fetched.body
    );
}

/// Rule 2: a write reaches only the resource its ARN names.
#[tokio::test]
async fn a_write_reaches_only_the_resource_named() {
    let router = router();
    let rows = [
        ("first", json!({ "env": "prod", "team": "search" })),
        ("second", json!({ "env": "staging" })),
        ("third", json!({})),
    ];

    let mut arns = Vec::new();
    for (name, tags) in &rows {
        let mut body = json!({ "name": name, "authenticationType": "AWS_IAM" });
        if tags.as_object().is_some_and(|map| !map.is_empty()) {
            body["tags"] = tags.clone();
        }
        let created = create_api(&router, body).await;
        arns.push(
            created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name}: the created API carries an ARN"))
                .to_string(),
        );
    }

    // Tag the FIRST resource only.
    let written = tag_resource(&router, &arns[0], json!({ "env": "dev", "owner": "platform" })).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case tag the first resource: expected {TAG_WRITE_STATUS}, got {} {:?}",
        written.status, written.body
    );

    let expected = [
        json!({ "env": "dev", "team": "search", "owner": "platform" }),
        json!({ "env": "staging" }),
        json!({}),
    ];
    for (((name, _), arn), want) in rows.iter().zip(&arns).zip(&expected) {
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            &listed.body["tags"], want,
            "case {name}: expected only this resource's tags to have changed, got {:?}",
            listed.body
        );
    }
}

/// Rule 3: an ARN this account holds no resource for is a not-found, and nothing is
/// created for it.
#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "tagged", "authenticationType": "API_KEY", "tags": { "env": "prod" } }),
    )
    .await;
    let arn = created["arn"].as_str().expect("the created API carries an ARN").to_string();
    let api_id = created["apiId"].as_str().expect("the created API carries an id");

    let doomed = create_api(
        &router,
        json!({ "name": "gone", "authenticationType": "API_KEY", "tags": { "env": "dev" } }),
    )
    .await;
    let deleted_arn = doomed["arn"].as_str().expect("the created API carries an ARN").to_string();
    let deleted_id = doomed["apiId"].as_str().expect("the created API carries an id");
    let deleted = call(&router, "DELETE", &format!("/v1/apis/{deleted_id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "deleting the API answers 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let cases = [
        (
            "an id no create ever minted",
            format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
        ("a deleted API's ARN", deleted_arn),
        (
            "the API's id under another Region",
            format!("arn:aws:appsync:us-east-1:{ACCOUNT}:apis/{api_id}"),
        ),
        (
            "the API's id under another account",
            format!("arn:aws:appsync:{REGION}:123456789012:apis/{api_id}"),
        ),
    ];

    for (label, candidate) in &cases {
        let written = tag_resource(&router, candidate, json!({ "env": "prod" })).await;
        assert_eq!(
            (written.status, written.error_type.as_str(), written.message.as_str()),
            (404, "NotFoundException", RESOURCE_NOT_FOUND),
            "case {label} ({candidate}): expected 404 NotFoundException {RESOURCE_NOT_FOUND:?}, got {} {} {:?}",
            written.status,
            written.error_type,
            written.body
        );

        // The refused write created no resource to read back.
        let listed = list_tags(&router, candidate).await;
        assert_eq!(
            (listed.status, listed.error_type.as_str()),
            (404, "NotFoundException"),
            "case {label}: expected the ARN to still name no resource, got {} {} {:?}",
            listed.status,
            listed.error_type,
            listed.body
        );
    }

    // The resource that DOES exist is untouched, so the refusals are about the ARN
    // rather than about the route.
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"],
        json!({ "env": "prod" }),
        "case the stored API: expected its tags unchanged, got {:?}",
        listed.body
    );
}

/// Rule 4: a key outside `TagKey`'s rule is refused, and the stored map is untouched.
///
/// The rows span the classes the rule reads: length on each side of the modelled
/// bounds, the reserved prefix, and characters outside the modelled alphabet.
#[tokio::test]
async fn a_key_outside_the_modelled_rule_is_a_bad_request() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "tagged", "authenticationType": "API_KEY", "tags": { "env": "prod" } }),
    )
    .await;
    let arn = created["arn"].as_str().expect("the created API carries an ARN").to_string();

    let too_long = "k".repeat(TAG_KEY_MAX_LEN + 1);
    let cases: [(&str, String); 7] = [
        ("a key one character under the minimum of 1", String::new()),
        (
            "a key one character over the maximum of 128",
            too_long.clone(),
        ),
        ("the reserved prefix alone", "aws:".to_string()),
        ("a key under the reserved prefix", "aws:cost-center".to_string()),
        ("a character outside the alphabet", "bad*key".to_string()),
        ("another character outside the alphabet", "bad!key".to_string()),
        ("a non-ASCII letter", "café".to_string()),
    ];

    for (label, key) in &cases {
        let written =
            tag_resource(&router, &arn, tag_map(&[(key.clone(), "v".to_string())])).await;
        assert_eq!(
            (written.status, written.error_type.as_str()),
            (400, "BadRequestException"),
            "case {label} ({key:?}): expected 400 BadRequestException, got {} {} {:?}",
            written.status,
            written.error_type,
            written.body
        );
        assert!(
            written.message.contains("tag key"),
            "case {label}: expected the refusal to name the rejected key, got {:?}",
            written.message
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.body["tags"],
            json!({ "env": "prod" }),
            "case {label}: expected the refused write to change nothing, got {:?}",
            listed.body
        );
    }

    // A key ON the maximum bound is served, so the refusals above are about the bound
    // rather than about long keys in general.
    let accepted = "k".repeat(TAG_KEY_MAX_LEN);
    let written = tag_resource(&router, &arn, tag_map(&[(accepted, "v".to_string())])).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case a key at the maximum of {TAG_KEY_MAX_LEN}: expected {TAG_WRITE_STATUS}, got {} {:?}",
        written.status, written.body
    );
}

/// Rule 5: a write is refused when it would leave the resource past the tag quota,
/// and refusing it changes nothing.
#[tokio::test]
async fn a_write_past_the_tag_quota_is_refused_and_changes_nothing() {
    // (label, pairs the resource starts with, pairs submitted, served?)
    let cases: [(&str, usize, usize, bool); 4] = [
        ("a write that exactly fills the quota", TAG_COUNT_MAX - 5, 5, true),
        ("a write one pair past the quota", TAG_COUNT_MAX - 5, 6, false),
        ("a single new pair on a full resource", TAG_COUNT_MAX, 1, false),
        ("a write well past the quota", TAG_COUNT_MAX / 2, TAG_COUNT_MAX, false),
    ];

    for (label, held, submitted, served) in cases {
        let router = router();
        let initial = numbered_pairs(0, held);
        let created = create_api(
            &router,
            json!({
                "name": "tagged",
                "authenticationType": "API_KEY",
                "tags": tag_map(&initial),
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the created API carries an ARN"))
            .to_string();

        // Submitted pairs are all NEW keys, so each one grows the stored map.
        let fresh = numbered_pairs(TAG_COUNT_MAX, submitted);
        let written = tag_resource(&router, &arn, tag_map(&fresh)).await;
        let listed = list_tags(&router, &arn).await;
        if served {
            assert_eq!(
                written.status, TAG_WRITE_STATUS,
                "case {label}: expected {TAG_WRITE_STATUS}, got {} {} {:?}",
                written.status, written.error_type, written.body
            );
            assert_eq!(
                listed.body["tags"].as_object().map(serde_json::Map::len),
                Some(held + submitted),
                "case {label}: expected the merged map to hold {} pairs, got {:?}",
                held + submitted,
                listed.body["tags"]
            );
        } else {
            assert_eq!(
                (written.status, written.error_type.as_str()),
                (429, "LimitExceededException"),
                "case {label}: expected 429 LimitExceededException, got {} {} {:?}",
                written.status,
                written.error_type,
                written.body
            );
            assert_eq!(
                listed.body["tags"],
                tag_map(&initial),
                "case {label}: expected the refused write to change nothing, got {:?}",
                listed.body
            );
        }
    }

    // At the quota, a write that only OVERWRITES keys already there grows nothing, so
    // it is served.
    let router = router();
    let initial = numbered_pairs(0, TAG_COUNT_MAX);
    let created = create_api(
        &router,
        json!({
            "name": "tagged",
            "authenticationType": "API_KEY",
            "tags": tag_map(&initial),
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("the created API carries an ARN").to_string();
    let overwrite = numbered_pairs(0, 3)
        .into_iter()
        .map(|(key, _)| (key, "rewritten".to_string()))
        .collect::<Vec<(String, String)>>();
    let written = tag_resource(&router, &arn, tag_map(&overwrite)).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case an overwrite at the quota: expected {TAG_WRITE_STATUS}, got {} {} {:?}",
        written.status, written.error_type, written.body
    );
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"].as_object().map(serde_json::Map::len),
        Some(TAG_COUNT_MAX),
        "case an overwrite at the quota: expected the map to still hold {TAG_COUNT_MAX} pairs, got {:?}",
        listed.body["tags"]
    );
    for (key, value) in &overwrite {
        assert_eq!(
            listed.body["tags"][key],
            Value::String(value.clone()),
            "case an overwrite at the quota: expected {key} to carry the submitted value, got {:?}",
            listed.body["tags"][key]
        );
    }
}
