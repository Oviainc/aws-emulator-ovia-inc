// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `TagResource`.
//!
//! The RULE under test, asserted over tables rather than one recorded request:
//! `TagResource` MERGES the request's `tags` into the tag set the addressed resource
//! already carries — a key the request names takes the request's value, a key it does
//! not name keeps the value it had, and nothing else about the resource changes — and
//! every reader of that resource's tags then reports the merged set. The resource is the
//! one the `resourceArn` label names; an ARN that names no resource this service holds is
//! refused with the service's `ResourceNotFoundException` (404) and writes nothing, and a
//! merge that would carry the resource past a tagging bound is refused with nothing
//! written either.
//!
//! The rows differ along what the rule generalises over: how many entries one call
//! carries (none, one, several), whether a key is new or already present, the FORM of the
//! key and the value (ASCII, non-ASCII, spaces and symbols, an empty value, each at its
//! published maximum length and one past it), how many calls have accumulated, which
//! resource type the ARN names (a `Domain` and a `Subscription`), and how an ARN fails to
//! resolve (wrong id, wrong region, wrong account, wrong service, not an ARN at all, or a
//! resource that has since been deleted).
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagKey` is `@length 1..128`, `TagValue` is `@length 0..256`, and `TagMap` itself
//!   declares no length, so the count limit is the service's own
//!   (`state::MAX_TAGS_PER_RESOURCE`, the AWS-standard 50 the crate already enforces on
//!   `CreateDomain`).
//! * `smithy_query(query="operation",operation="TagResource")` — `POST
//!   /tags/{resourceArn}` answering 204, `resourceArn` an `@httpLabel`, `tags` required,
//!   `@idempotent`, and `errors: []` (so the two exceptions asserted here are the shapes
//!   the SERVICE models, widened into this operation).
//!
//! The reader used for the round trip is `GetDomain`, whose `tags` member reports the
//! stored set. A `Subscription`'s own tag reader is `ListTagsForResource`, still a
//! scaffold stub under its own issue
//! (`issues/things-that-do-not-work/20261023T115949Z-connecthealth-listtagsforresource-stub.md`),
//! so a subscription ARN is asserted here through acceptance and through the isolation of
//! its tag set from its domain's.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, tag_map, Emulator};

/// The path `TagResource` is addressed at, with the ARN percent-encoded into the single
/// non-greedy `{resourceArn}` label — the `:` and `/` of an ARN are exactly what an SDK
/// encodes there.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"))
}

/// `arn` with its account field replaced, so it is a well-formed ARN of this service
/// that names a resource of a DIFFERENT account.
///
/// The account is the 5th colon-separated field of every ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
fn under_another_account(arn: &str) -> String {
    let mut fields: Vec<&str> = arn.split(':').collect();
    fields[4] = "999999999999";
    fields.join(":")
}

/// The tag set `GetDomain` reports for a domain.
///
/// A domain always reports a `tags` member, empty when it carries none, so an absent
/// member is a failure rather than "no tags".
async fn domain_tags(emulator: &Emulator, domain_id: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain {domain_id}: got {status} {body}");
    let reported = body
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("GetDomain {domain_id} must report a tags map, got {body}"));
    reported
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("tag {key:?} must be a string in {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// `TagResource`, asserting the modelled success: 204 with an empty body.
///
/// `tags` is the tag map itself; `resourceArn` is an `@httpLabel`, so the map is the one
/// body member the request carries (`{"tags": {..}}`).
async fn tag_resource(emulator: &Emulator, case: &str, arn: &str, tags: &Value) {
    let (status, body) = emulator
        .send("POST", &tags_path(arn), Some(&json!({ "tags": tags })))
        .await;
    assert_eq!(
        status, 204,
        "case {case:?}: TagResource {tags} must answer 204, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: a 204 carries no payload, got {body}"
    );
}

/// The entries of a JSON object as a map, for applying the merge rule independently of
/// the emulator.
fn entries(tags: &Value) -> Vec<(String, String)> {
    tags.as_object()
        .unwrap_or_else(|| panic!("a tags map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn tag_resource_merges_its_entries_into_the_resources_tag_set() {
    let emulator = Emulator::new("us-east-1");
    // The domain starts with tags of its own, so the first merge has something to merge
    // INTO rather than only to create.
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    // A key at each published maximum length, built here rather than taken from a
    // recorded request.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);

    // Each row is one `TagResource` call; the expectation after it is computed by
    // applying the merge rule to the previous expectation, never by restating a response.
    let rows: Vec<(&str, Value)> = vec![
        ("a single new key", json!({ "team": "cardiology" })),
        ("an existing key, overwritten", json!({ "env": "staging" })),
        (
            "several keys at once",
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
        ("an empty value", json!({ "retired": "" })),
        (
            "non-ASCII key and value",
            json!({ "clínica": "ü-ok", "日本語": "値" }),
        ),
        (
            "spaces and symbols",
            json!({ "cost center": "acct/42 +x" }),
        ),
        (
            "a key and a value at their maximum length",
            json!({ longest_key.clone(): longest_value.clone() }),
        ),
        ("no entries at all", json!({})),
        (
            "the same call repeated (@idempotent)",
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
        (
            "every earlier key overwritten at once",
            json!({ "env": "prod", "team": "oncology", "a": "9" }),
        ),
    ];

    let mut expected: BTreeMap<String, String> = [("env".to_string(), "prod".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        domain_tags(&emulator, &domain_id).await,
        expected,
        "the tags CreateDomain stored must be the set the merges start from"
    );
    for (case, tags) in &rows {
        tag_resource(&emulator, case, &arn, tags).await;
        expected.extend(entries(tags));
        let reported = domain_tags(&emulator, &domain_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: tagging with {tags} must leave the merged set, got {reported:?}"
        );
    }

    // The merge touched the tags and nothing else: every other member of the domain is
    // still the one the create reported.
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain: got {fetched}");
    for member in ["domainId", "arn", "name", "status", "createdAt", "accountId"] {
        assert_eq!(
            fetched.get(member),
            created.get(member),
            "member {member:?} must be untouched by tagging, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_subscriptions_tag_set_is_its_own() {
    let emulator = Emulator::new("eu-central-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let domain_arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_arn = string_at(&subscription, "arn");

    // Tagging the subscription is accepted on its own ARN...
    tag_resource(
        &emulator,
        "the subscription's own ARN",
        &subscription_arn,
        &json!({ "tier": "gold", "env": "subscription-only" }),
    )
    .await;
    // ...and leaves the DOMAIN's set exactly as it was: one store keyed by ARN, so two
    // resources never share a tag set.
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported,
        [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the domain's tags must not change when its subscription is tagged, got {reported:?}"
    );

    // And the reverse: tagging the domain does not reach the subscription's set, so the
    // domain's new keys are the only ones it reports.
    tag_resource(
        &emulator,
        "the domain's ARN after the subscription's",
        &domain_arn,
        &json!({ "tier": "silver" }),
    )
    .await;
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported,
        [
            ("env".to_string(), "prod".to_string()),
            ("tier".to_string(), "silver".to_string()),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "the domain reports its own merged set, got {reported:?}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_and_writes_nothing() {
    let emulator = Emulator::new("us-west-2");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_arn = string_at(&subscription, "arn");

    // A domain with no web application is deleted within the DeleteDomain call, so its
    // ARN stops resolving immediately.
    let doomed = emulator
        .create_domain(&json!({ "name": "clinic-doomed" }))
        .await;
    let doomed_arn = string_at(&doomed, "arn");
    let (status, deleted) = emulator
        .send(
            "DELETE",
            &format!("/domain/{}", string_at(&doomed, "domainId")),
            None,
        )
        .await;
    assert_eq!(status, 200, "DeleteDomain got {deleted}");

    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a well-formed domain ARN whose id names no domain",
            "arn:aws:health-agent:us-west-2:000000000000:domain/dom-zzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "the live domain's ARN in another region",
            arn.replace("us-west-2", "ap-south-1"),
        ),
        (
            "the live domain's ARN under another account",
            under_another_account(&arn),
        ),
        (
            "a subscription id no subscription of the live domain carries",
            format!("{arn}/subscription/sub-000000000000000000000"),
        ),
        (
            "the live subscription's ARN under a domain that does not exist",
            subscription_arn.replace(&domain_id, "dom-zzzzzzzzzzzzzzzz"),
        ),
        ("the ARN of a deleted domain", doomed_arn),
        ("another service's ARN", "arn:aws:s3:::a-bucket".to_string()),
        ("not an ARN at all", "not-an-arn".to_string()),
        ("a bare domain id", domain_id.clone()),
    ];

    for (case, unresolved) in &unresolvable {
        let (status, body) = emulator
            .send(
                "POST",
                &tags_path(unresolved),
                Some(&json!({ "tags": { "env": "should-not-persist" } })),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({unresolved}): expected 404 ResourceNotFoundException, \
             got {status} with {body}"
        );
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the 404 must carry a Message, got {body}"));
        assert!(
            message.contains(unresolved.as_str()),
            "case {case:?}: the message must name the ARN the request asked for, got {message:?}"
        );
    }

    // Nothing any of those refusals carried reached the store.
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported,
        [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "a refused TagResource must write nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn the_tagging_bounds_are_enforced_on_the_merged_set() {
    let emulator = Emulator::new("us-east-1");
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let one_past_the_longest_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let one_past_the_longest_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);

    // Each published bound, and one past each: the key and value lengths the model
    // declares, and the per-resource count the service enforces itself.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a key at the maximum length",
            json!({ longest_key.clone(): "x" }),
            204,
            "",
        ),
        (
            "a key one past the maximum length",
            json!({ one_past_the_longest_key.clone(): "x" }),
            400,
            "ValidationException",
        ),
        (
            "a value at the maximum length",
            json!({ "long-value": longest_value.clone() }),
            204,
            "",
        ),
        (
            "a value one past the maximum length",
            json!({ "long-value": one_past_the_longest_value.clone() }),
            400,
            "ValidationException",
        ),
        (
            "an empty value, which the modelled minimum of 0 allows",
            json!({ "empty": "" }),
            204,
            "",
        ),
        (
            "an empty key, below the modelled minimum of 1",
            json!({ "": "x" }),
            400,
            "ValidationException",
        ),
        (
            "a map with no entries",
            json!({}),
            204,
            "",
        ),
    ];
    for (case, tags, expected_status, expected_code) in &rows {
        // A fresh domain per row, so one row's accepted entries cannot change the next
        // row's merge.
        let created = emulator.create_domain(&json!({ "name": "clinic" })).await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let (status, body) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
            .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        let reported = domain_tags(&emulator, &domain_id).await;
        if *expected_status == 204 {
            assert_eq!(
                reported,
                entries(tags).into_iter().collect::<BTreeMap<_, _>>(),
                "case {case:?}: an accepted call stores exactly its entries, got {reported:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                *expected_code,
                "case {case:?}: got {body}"
            );
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused call writes nothing, got {reported:?}"
            );
        }
    }

    // The count limit is on the merged SET, not on one request: a resource already at
    // the cap accepts a call that only overwrites keys it carries, and refuses one that
    // would add a new key, with the service's quota error.
    let seeded = tag_map(MAX_TAGS_PER_RESOURCE, 8, 8);
    let created = emulator
        .create_domain(&json!({ "name": "clinic-full", "tags": seeded.clone() }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    let at_the_cap = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        at_the_cap.len(),
        MAX_TAGS_PER_RESOURCE,
        "the seeded domain must hold the cap, got {at_the_cap:?}"
    );

    let overwrite_key = at_the_cap
        .keys()
        .next()
        .expect("the seeded domain carries tags")
        .clone();
    tag_resource(
        &emulator,
        "an overwrite of an existing key at the cap",
        &arn,
        &json!({ overwrite_key.clone(): "overwritten" }),
    )
    .await;
    let mut expected = at_the_cap.clone();
    expected.insert(overwrite_key.clone(), "overwritten".to_string());
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported, expected,
        "an overwrite at the cap keeps the set the same size, got {reported:?}"
    );

    // One key past the cap, through tag maps of three different sizes: the refusal is on
    // the merged total, not on how many entries the request carried.
    for (case, extra) in [
        ("one new key", json!({ "one-too-many": "x" })),
        (
            "one new key beside an overwrite",
            json!({ "one-too-many": "x", overwrite_key.clone(): "overwritten-again" }),
        ),
        (
            "several new keys",
            json!({ "x1": "1", "x2": "2", "x3": "3" }),
        ),
    ] {
        let (status, body) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": extra })))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {case:?}: a merge past {MAX_TAGS_PER_RESOURCE} tags must be refused, \
             got {status} with {body}"
        );
        let reported = domain_tags(&emulator, &domain_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the refused merge must write nothing, got {reported:?}"
        );
    }
}
