// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over HTTP.
//!
//! Six rules are asserted here, each over a TABLE or a class of inputs rather than
//! over the single request that exposed the defect:
//!
//! 1. an untag REMOVES from the resource's stored tag map exactly the keys the request
//!    lists — a key the resource carries goes together with its value, a key it does
//!    not carry simply is not there afterwards, and a key the request does not list is
//!    untouched — and what is left is what the tag listing and the resource's own
//!    `tags` member report afterwards; repeating the call answers the same way. The
//!    table spans what the rule generalises over: removing the only key, one of
//!    several, several at once and every key; a key the resource never carried, alone
//!    and mixed with one it does carry; a key whose value is the empty string; key
//!    length on both modelled bounds; the character classes `TagKey` admits; and key
//!    counts from one to the modelled maximum of 50;
//! 2. removing a key is not writing an empty value over it, and it is not permanent:
//!    a removed key is absent from the listing, and tagging it again brings its value
//!    back;
//! 3. an untag reaches only the resource its ARN names, so a second resource in the
//!    same store keeps its own map;
//! 4. a well-formed resource ARN this account holds no resource for is a 404
//!    `NotFoundException` — over a never-created ARN, a deleted resource's ARN, and the
//!    ARN of a resource that exists under a different Region or account — and the
//!    refused call creates nothing;
//! 5. a key outside `TagKey`'s rule (under the reserved `aws:` prefix, drawn from
//!    outside its alphabet, or outside its modelled length bounds) is refused with
//!    status 400 and leaves the stored map exactly as it was, and a request that names
//!    no `tagKeys` at all removes nothing;
//! 6. an API of either resource family (a `GraphqlApi` and an `Api`) is published under
//!    the one ARN shape `ResourceArn` admits, so rule 1 is also asserted on an Event
//!    API.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, the `@httpLabel` ARN binding, the repeated `@httpQuery` list
//! binding, restJson1 (de)serialization, the handler and the shared engine together.
//! The harness is the error-contract suite's `call` over the lifecycle suite's router
//! and create helper, with the tag suites' label encoder, pair tables and modelled
//! bounds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, Answer};
use crate::graphql_api_lifecycle::{create_api, router, REGION};
use crate::resource_tags::{
    encode_label, ABSENT_API_ID, ACCOUNT, TAG_COUNT_MAX, TAG_KEY_MAX_LEN,
};
use crate::tag_resource::{
    list_tags, numbered_pairs, pairs, tag_map, tag_resource, API_NOT_FOUND, TAG_KEY_MIN_LEN,
};

/// `DELETE /v1/tags/{resourceArn}?tagKeys=…`, as a client issues it: `tagKeys` is a
/// LIST bound to a repeated query parameter (`smithy_query(query="operation",
/// operation="UntagResource")` — `tagKeys` targets `TagKeyList` with
/// `httpQuery: "tagKeys"`), so one parameter is sent per key.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<String>>()
        .join("&");
    let label = encode_label(arn);
    let path = if query.is_empty() {
        format!("/v1/tags/{label}")
    } else {
        format!("/v1/tags/{label}?{query}")
    };
    call(router, "DELETE", &path, None).await
}

/// A borrowed key table as an owned one.
fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// One row of the removal table: the pairs the resource is created with, and the keys
/// one `UntagResource` call names for removal.
#[derive(Debug)]
struct Row {
    label: String,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
}

impl Row {
    /// The map the removal rule says the resource ends up with: the initial pairs
    /// without any of the named keys.
    fn expected(&self) -> Value {
        let mut left: std::collections::BTreeMap<String, String> =
            self.initial.iter().cloned().collect();
        for key in &self.removed {
            left.remove(key);
        }
        let pairs: Vec<(String, String)> = left.into_iter().collect();
        tag_map(&pairs)
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

/// The removal table: every class the removal rule generalises over.
fn rows() -> Vec<Row> {
    let long_key = "k".repeat(TAG_KEY_MAX_LEN);
    let short_key = "s".repeat(TAG_KEY_MIN_LEN);
    // The character classes `TagKey`'s modelled pattern admits: `[ a-zA-Z+-=._:/]`,
    // whose `+-=` range also admits the digits.
    let spanning_key = "Cost.center/sub:team key-01+2=3";
    let three = pairs(&[("env", "prod"), ("team", "search"), ("owner", "platform")]);
    let mut rows = vec![
        (
            "the only key of a one-pair resource",
            pairs(&[("env", "prod")]),
            keys(&["env"]),
        ),
        ("one key of several", three.clone(), keys(&["team"])),
        (
            "two keys of several in one call",
            three.clone(),
            keys(&["env", "owner"]),
        ),
        (
            "every key the resource carries",
            three.clone(),
            keys(&["env", "team", "owner"]),
        ),
        (
            "a key the resource never carried",
            pairs(&[("env", "prod")]),
            keys(&["absent"]),
        ),
        (
            "a key it carries and a key it does not, in one call",
            pairs(&[("env", "prod"), ("team", "search")]),
            keys(&["team", "absent"]),
        ),
        // `TagValue`'s modelled minimum length is 0, so a key can hold the empty
        // string — and removing such a key is different from writing that value.
        (
            "a key whose value is the empty string",
            pairs(&[("env", ""), ("team", "search")]),
            keys(&["env"]),
        ),
        (
            "every key of a resource whose values are empty",
            pairs(&[("env", ""), ("team", "")]),
            keys(&["env", "team"]),
        ),
        // `TagKey`'s modelled length bounds.
        (
            "a key at the minimum length of 1",
            pairs(&[(short_key.as_str(), "v"), ("env", "prod")]),
            keys(&[short_key.as_str()]),
        ),
        (
            "a key at the maximum length of 128",
            pairs(&[(long_key.as_str(), "v"), ("env", "prod")]),
            keys(&[long_key.as_str()]),
        ),
        (
            "a key spanning the admitted character classes",
            pairs(&[(spanning_key, "v"), ("env", "prod")]),
            keys(&[spanning_key]),
        ),
        // The reserved prefix is `aws:`, so a key that merely begins with those
        // letters is an ordinary key and can be removed.
        (
            "a key beginning with aws outside the reserved prefix",
            pairs(&[("awsome", "yes"), ("env", "prod")]),
            keys(&["awsome"]),
        ),
    ]
    .into_iter()
    .map(|(label, initial, removed)| Row {
        label: label.to_string(),
        initial,
        removed,
    })
    .collect::<Vec<Row>>();

    // Key COUNT: one call naming the modelled maximum, and one naming half of a
    // resource filled to that maximum.
    let full = numbered_pairs(0, TAG_COUNT_MAX);
    rows.push(Row {
        label: format!("the modelled maximum of {TAG_COUNT_MAX} keys in one call"),
        initial: full.clone(),
        removed: full.iter().map(|(key, _)| key.clone()).collect(),
    });
    rows.push(Row {
        label: format!("half of a resource holding {TAG_COUNT_MAX} pairs"),
        initial: full.clone(),
        removed: full
            .iter()
            .take(TAG_COUNT_MAX / 2)
            .map(|(key, _)| key.clone())
            .collect(),
    });
    rows
}

/// Rule 1: the call removes the named keys from the stored map, and the stored map is
/// what every later read reports; repeating it leaves the same map.
#[tokio::test]
async fn an_untag_removes_exactly_the_keys_it_names() {
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

        let removed = untag_resource(&router, &arn, &row.removed).await;
        assert_eq!(
            removed.status, 200,
            "case {label}: expected UntagResource 200, got {} {} {:?}",
            removed.status, removed.error_type, removed.body
        );
        // `UntagResourceResponse` models no members, so the answer carries none.
        assert!(
            removed.body.is_null()
                || removed
                    .body
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
            "case {label}: expected an answer with no members, got {:?}",
            removed.body
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
            "case {label} ({row:?}): expected the remaining tag map, got {:?}",
            listed.body
        );

        // The resource's own `tags` member reads the same map, so the two answers
        // cannot disagree about what was removed.
        let fetched = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            fetched.body["graphqlApi"]["tags"],
            row.expected(),
            "case {label}: expected the API to carry the remaining tags, got {:?}",
            fetched.body
        );

        // Naming the same keys again is a no-op: they are already gone.
        let again = untag_resource(&router, &arn, &row.removed).await;
        assert_eq!(
            again.status, 200,
            "case {label}: expected a repeated UntagResource 200, got {} {} {:?}",
            again.status, again.error_type, again.body
        );
        let relisted = list_tags(&router, &arn).await;
        assert_eq!(
            relisted.body["tags"],
            row.expected(),
            "case {label}: expected the repeated call to leave the same map, got {:?}",
            relisted.body
        );
    }
}

/// Rule 2: a removed key is GONE rather than emptied, and tagging it again brings its
/// value back.
///
/// The rows differ along the value the key held before it was removed and the value it
/// is given afterwards, the empty string included on either side.
#[tokio::test]
async fn a_removed_key_is_absent_and_can_be_set_again() {
    // (label, the value the key starts with, the value it is re-tagged with)
    let cases: [(&str, &str, &str); 4] = [
        ("a non-empty value restored", "prod", "staging"),
        ("the same value written again", "prod", "prod"),
        ("a key that held the empty string", "", "prod"),
        ("a key re-tagged with the empty string", "prod", ""),
    ];

    for (label, first, second) in cases {
        let router = router();
        let created = create_api(
            &router,
            json!({
                "name": "tagged",
                "authenticationType": "API_KEY",
                "tags": tag_map(&pairs(&[("env", first), ("team", "search")])),
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the created API carries an ARN"))
            .to_string();

        let removed = untag_resource(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            removed.status, 200,
            "case {label}: expected UntagResource 200, got {} {} {:?}",
            removed.status, removed.error_type, removed.body
        );
        let listed = list_tags(&router, &arn).await;
        // The key is absent, not present with an empty value: `tags` holds no `env`
        // member at all.
        assert!(
            listed.body["tags"].get("env").is_none(),
            "case {label}: expected the removed key to be absent, got {:?}",
            listed.body["tags"]
        );
        assert_eq!(
            listed.body["tags"],
            json!({ "team": "search" }),
            "case {label}: expected only the key that was not named, got {:?}",
            listed.body
        );

        let written = tag_resource(&router, &arn, json!({ "env": second })).await;
        assert_eq!(
            written.status, 200,
            "case {label}: expected TagResource 200 after the removal, got {} {} {:?}",
            written.status, written.error_type, written.body
        );
        let relisted = list_tags(&router, &arn).await;
        assert_eq!(
            relisted.body["tags"],
            json!({ "env": second, "team": "search" }),
            "case {label}: expected the key to carry its new value, got {:?}",
            relisted.body
        );
    }
}

/// Rule 3: an untag reaches only the resource its ARN names.
#[tokio::test]
async fn an_untag_reaches_only_the_resource_named() {
    let router = router();
    let rows = [
        ("first", json!({ "env": "prod", "team": "search" })),
        ("second", json!({ "env": "staging", "team": "search" })),
        ("third", json!({ "env": "dev" })),
    ];

    let mut arns = Vec::new();
    for (name, tags) in &rows {
        let created = create_api(
            &router,
            json!({ "name": name, "authenticationType": "AWS_IAM", "tags": tags }),
        )
        .await;
        arns.push(
            created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name}: the created API carries an ARN"))
                .to_string(),
        );
    }

    // Untag the FIRST resource only.
    let removed = untag_resource(&router, &arns[0], &keys(&["env", "team"])).await;
    assert_eq!(
        removed.status, 200,
        "case untag the first resource: expected 200, got {} {:?}",
        removed.status, removed.body
    );

    let expected = [
        json!({}),
        json!({ "env": "staging", "team": "search" }),
        json!({ "env": "dev" }),
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

/// Rule 4: an ARN this account holds no resource for is a not-found, and nothing is
/// created for it.
#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "tagged", "authenticationType": "API_KEY", "tags": { "env": "prod" } }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .expect("the created API carries an ARN")
        .to_string();
    let api_id = created["apiId"].as_str().expect("the created API carries an id");

    let doomed = create_api(
        &router,
        json!({ "name": "gone", "authenticationType": "API_KEY", "tags": { "env": "dev" } }),
    )
    .await;
    let deleted_arn = doomed["arn"]
        .as_str()
        .expect("the created API carries an ARN")
        .to_string();
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
        let removed = untag_resource(&router, candidate, &keys(&["env"])).await;
        assert_eq!(
            (
                removed.status,
                removed.error_type.as_str(),
                removed.message.as_str()
            ),
            (404, "NotFoundException", API_NOT_FOUND),
            "case {label} ({candidate}): expected 404 NotFoundException {API_NOT_FOUND:?}, got {} {} {:?}",
            removed.status,
            removed.error_type,
            removed.body
        );

        // The refused call created no resource to read back.
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

/// Rule 5: a key outside `TagKey`'s rule is refused, and the stored map is untouched.
///
/// The rows span the classes the rule reads: the reserved prefix, characters outside
/// the modelled alphabet, and length on each side of the modelled bounds. The first
/// two classes are stated by `TagKey`'s pattern and refused as AppSync's own
/// `BadRequestException`; the length bounds are refused at the request boundary, so
/// the row asserts the status and that nothing changed.
#[tokio::test]
async fn a_key_outside_the_modelled_rule_is_refused_and_changes_nothing() {
    let router = router();
    let created = create_api(
        &router,
        json!({
            "name": "tagged",
            "authenticationType": "API_KEY",
            "tags": { "env": "prod", "team": "search" },
        }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .expect("the created API carries an ARN")
        .to_string();
    let held = json!({ "env": "prod", "team": "search" });

    // (label, key, whether AppSync's own tag-key refusal states it)
    let too_long = "k".repeat(TAG_KEY_MAX_LEN + 1);
    let cases: [(&str, String, bool); 7] = [
        ("the reserved prefix alone", "aws:".to_string(), true),
        (
            "a key under the reserved prefix",
            "aws:cost-center".to_string(),
            true,
        ),
        ("a character outside the alphabet", "bad*key".to_string(), true),
        (
            "another character outside the alphabet",
            "bad!key".to_string(),
            true,
        ),
        ("a non-ASCII letter", "café".to_string(), true),
        (
            "a key one character under the minimum of 1",
            String::new(),
            false,
        ),
        (
            "a key one character over the maximum of 128",
            too_long,
            false,
        ),
    ];

    for (label, key, own_refusal) in &cases {
        let removed = untag_resource(&router, &arn, std::slice::from_ref(key)).await;
        assert_eq!(
            removed.status, 400,
            "case {label} ({key:?}): expected status 400, got {} {} {:?}",
            removed.status, removed.error_type, removed.body
        );
        if *own_refusal {
            assert_eq!(
                removed.error_type, "BadRequestException",
                "case {label} ({key:?}): expected error type BadRequestException, got {} {:?}",
                removed.error_type, removed.body
            );
            assert!(
                removed.message.contains("tag key"),
                "case {label}: expected the refusal to name the rejected key, got {:?}",
                removed.message
            );
        }

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.body["tags"], held,
            "case {label}: expected the refused call to change nothing, got {:?}",
            listed.body
        );
    }

    // A well-formed key mixed in with an offending one is not removed either: the
    // refusal is decided before anything is.
    let mixed = untag_resource(&router, &arn, &keys(&["env", "aws:cost-center"])).await;
    assert_eq!(
        (mixed.status, mixed.error_type.as_str()),
        (400, "BadRequestException"),
        "case a well-formed key beside an offending one: expected 400 BadRequestException, \
         got {} {} {:?}",
        mixed.status,
        mixed.error_type,
        mixed.body
    );
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"], held,
        "case a well-formed key beside an offending one: expected nothing removed, got {:?}",
        listed.body
    );

    // `tagKeys` is `@required` and bound to the query string, so a call that names
    // none is not served and removes nothing.
    let none = untag_resource(&router, &arn, &[]).await;
    assert_ne!(
        none.status, 200,
        "case no tagKeys at all: expected the call not to be served, got {} {} {:?}",
        none.status, none.error_type, none.body
    );
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"], held,
        "case no tagKeys at all: expected nothing removed, got {:?}",
        listed.body
    );
}

/// Rule 6, on the other resource family: an `Api` is published under the same ARN
/// shape, so it is untagged and listed through the same route.
#[tokio::test]
async fn an_event_api_is_untagged_through_the_same_route() {
    let router = router();
    let create = call(
        &router,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "events",
            "tags": { "env": "prod", "team": "events" },
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

    let removed = untag_resource(&router, &arn, &keys(&["env"])).await;
    assert_eq!(
        removed.status, 200,
        "case untag an Event API: expected 200, got {} {} {:?}",
        removed.status, removed.error_type, removed.body
    );

    let expected = json!({ "team": "events" });
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"], expected,
        "case untag an Event API: expected the remaining tag map, got {:?}",
        listed.body
    );
    let fetched = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        fetched.body["api"]["tags"], expected,
        "case untag an Event API: expected the API to carry the remaining tags, got {:?}",
        fetched.body
    );
}
