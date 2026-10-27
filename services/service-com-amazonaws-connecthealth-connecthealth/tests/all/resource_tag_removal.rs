// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UntagResource`.
//!
//! The RULE under test, asserted over tables rather than one recorded request:
//! `UntagResource` REMOVES from the tag set the addressed resource carries exactly the
//! keys the request names — a key it names is dropped whatever its value, a key it does
//! not name keeps the value it had, a key the resource does not carry drops nothing, and
//! nothing else about the resource changes — so every reader of that resource's tags
//! reports the reduced set afterwards. The resource is the one the `resourceArn` label
//! names; an ARN that names no resource this service holds is refused with the service's
//! `ResourceNotFoundException` (404) and removes nothing, and a key this service does not
//! accept is refused with HTTP 400 `ValidationException`, likewise removing nothing.
//!
//! The rows differ along what the rule generalises over: how many keys one call names
//! (one, several, every key left, the published maximum and one past it), whether a named
//! key is carried or absent, the FORM of the key (ASCII, non-ASCII, spaces and symbols, a
//! key whose value is empty, a key at its published maximum length and one past it), how
//! many calls have accumulated, which resource kind the ARN names (a `Domain` and a
//! `Subscription`), and how an ARN fails to resolve (wrong id, wrong region, wrong
//! account, wrong service, not an ARN at all, or a resource that has since been deleted).
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="operation",operation="UntagResource",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `DELETE /tags/{resourceArn}` answering 204, `resourceArn` an `@httpLabel` and
//!   required, `tagKeys` an `@httpQuery("tagKeys")` list of 1..50 and required,
//!   `@idempotent`, and `errors: []` (so the exceptions asserted here are shapes the
//!   SERVICE models, widened into this operation exactly as `TagResource` widens them).
//! * `smithy_query(query="shape",shape="TagKeyList,TagKey",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagKeyList` is `@length 1..50` of `TagKey`, and `TagKey` is `@length 1..128`; the
//!   key length is stated once in the crate (`state::MAX_TAG_KEY_LEN` /
//!   `MIN_TAG_KEY_LEN`), which is what this suite imports rather than restating.
//!
//! The readers each case reads the set back through are `ListTagsForResource` (which
//! addresses either resource kind by ARN) and `GetDomain`'s `tags` member, so a removal
//! that reached only one of the two views would fail here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, RESERVED_TAG_KEY_PREFIX,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::list_tags_for_resource::list_tags;
use crate::resource_tagging::{domain_tags, entries, tag_resource, tags_path, under_another_account};

/// How many keys ONE `UntagResource` call may name: `TagKeyList`'s `@length` maximum
/// (`smithy_query(query="shape",shape="TagKeyList")` reports `{min: 1, max: 50}`). The
/// model owns this bound, so it is refused at the request boundary rather than by the
/// handler; asserted below at the maximum and one past it.
const MAX_TAG_KEYS_PER_CALL: usize = 50;

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, which is how an SDK sends each element of the `@httpQuery`-bound
/// `tagKeys` list.
///
/// `+` in particular has to be escaped: a query value is decoded with `+` meaning a
/// space, so an unescaped `+` would name a different key than the stored one.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The `DELETE /tags/{resourceArn}?tagKeys=…` URI: `tagKeys` is an `@httpQuery` list, so
/// each key travels as its own repeated `tagKeys=` parameter.
///
/// `pub(crate)` so a sibling suite addresses this operation the same way rather than
/// growing a second copy of the URI builder.
pub(crate) fn untag_path(arn: &str, keys: &[String]) -> String {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    format!("{}?{}", tags_path(arn), query.join("&"))
}

/// `UntagResource`, asserting the modelled success: 204 with an empty body.
async fn untag(emulator: &Emulator, case: &str, arn: &str, keys: &[String]) {
    let (status, body) = emulator
        .send("DELETE", &untag_path(arn, keys), None)
        .await;
    assert_eq!(
        status, 204,
        "case {case:?}: UntagResource {keys:?} must answer 204, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: a 204 carries no payload, got {body}"
    );
}

/// The tag set a domain reports through BOTH of its readers, which must agree: one store
/// per resource, so a removal that reached one view and not the other is a failure.
async fn agreed_tags(emulator: &Emulator, case: &str, domain_id: &str, arn: &str) -> BTreeMap<String, String> {
    let by_arn = list_tags(emulator, case, arn).await;
    let by_domain = domain_tags(emulator, domain_id).await;
    assert_eq!(
        by_arn, by_domain,
        "case {case:?}: ListTagsForResource and GetDomain must report the same set, got \
         {by_arn:?} and {by_domain:?}"
    );
    by_arn
}

/// The seed set every removal row starts from: keys of each form the rule generalises
/// over, written here rather than taken from a recorded request.
fn seed_tags(longest_key: &str, longest_value: &str) -> Value {
    json!({
        "env": "prod",
        "team": "cardiology",
        "tier": "gold",
        "retired": "",
        "clínica": "ü-ok",
        "日本語": "値",
        "cost center": "acct/42 +x",
        longest_key: longest_value,
    })
}

#[tokio::test]
async fn untag_removes_exactly_the_keys_it_names() {
    let emulator = Emulator::new("us-east-1");
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let seed = seed_tags(&longest_key, &longest_value);
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": seed.clone() }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    let mut expected: BTreeMap<String, String> = entries(&seed).into_iter().collect();
    let reported = agreed_tags(&emulator, "the seeded set", &domain_id, &arn).await;
    assert_eq!(
        reported, expected,
        "the tags CreateDomain stored must be the set the removals start from, got \
         {reported:?}"
    );

    // Each row is one `UntagResource` call, applied cumulatively; the expectation after
    // it is computed by applying the removal rule to the previous expectation, never by
    // restating a response.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one carried key", vec!["env".to_string()]),
        ("a key the resource does not carry", vec!["never-set".to_string()]),
        ("the same call repeated (@idempotent)", vec!["env".to_string()]),
        (
            "several carried keys at once",
            vec!["team".to_string(), "tier".to_string()],
        ),
        ("a key whose value is empty", vec!["retired".to_string()]),
        ("a non-ASCII key", vec!["clínica".to_string()]),
        (
            "a carried key beside an absent one",
            vec!["cost center".to_string(), "never-set".to_string()],
        ),
        (
            "the same key named twice in one call",
            vec!["日本語".to_string(), "日本語".to_string()],
        ),
        ("a key at the maximum length", vec![longest_key.clone()]),
        (
            "a removal from an already empty set",
            vec!["env".to_string(), "team".to_string()],
        ),
    ];
    for (case, keys) in &rows {
        untag(&emulator, case, &arn, keys).await;
        for key in keys {
            expected.remove(key);
        }
        let reported = agreed_tags(&emulator, case, &domain_id, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: removing {keys:?} must leave the reduced set, got {reported:?}"
        );
    }
    assert!(
        expected.is_empty(),
        "the rows above must have removed every seeded key, {expected:?} are left"
    );

    // The removals touched the tags and nothing else: every other member of the domain is
    // still the one the create reported.
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain: got {fetched}");
    for member in ["domainId", "arn", "name", "status", "createdAt", "accountId"] {
        assert_eq!(
            fetched.get(member),
            created.get(member),
            "member {member:?} must be untouched by untagging, got {fetched}"
        );
    }

    // And a write after the removals merges into the reduced set, so no removal left a
    // key behind in the store for a later merge to resurrect.
    tag_resource(
        &emulator,
        "a write after the removals",
        &arn,
        &json!({ "env": "staging" }),
    )
    .await;
    let reported = agreed_tags(&emulator, "after the write", &domain_id, &arn).await;
    assert_eq!(
        reported,
        [("env".to_string(), "staging".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the emptied set must hold only what the later write stored, got {reported:?}"
    );
}

#[tokio::test]
async fn a_removal_reaches_only_the_addressed_resources_set() {
    let emulator = Emulator::new("eu-central-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod", "tier": "silver" } }))
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
    tag_resource(
        &emulator,
        "seeding the subscription",
        &subscription_arn,
        &json!({ "env": "subscription-only", "tier": "gold" }),
    )
    .await;

    // Removing `env` from the SUBSCRIPTION leaves the domain's `env` exactly as it was:
    // one store keyed by ARN, so two resources never share a tag set.
    untag(
        &emulator,
        "the subscription's own ARN",
        &subscription_arn,
        &["env".to_string()],
    )
    .await;
    let subscription_set = list_tags(&emulator, "the subscription after its removal", &subscription_arn).await;
    assert_eq!(
        subscription_set,
        [("tier".to_string(), "gold".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the subscription must report only its remaining key, got {subscription_set:?}"
    );
    let domain_set = agreed_tags(&emulator, "the domain after the subscription's removal", &domain_id, &domain_arn).await;
    assert_eq!(
        domain_set,
        [
            ("env".to_string(), "prod".to_string()),
            ("tier".to_string(), "silver".to_string()),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "the domain's tags must not change when its subscription is untagged, got \
         {domain_set:?}"
    );

    // And the reverse: removing `tier` from the DOMAIN leaves the subscription's `tier`.
    untag(
        &emulator,
        "the domain's ARN after the subscription's",
        &domain_arn,
        &["tier".to_string()],
    )
    .await;
    let domain_set = agreed_tags(&emulator, "the domain after its own removal", &domain_id, &domain_arn).await;
    assert_eq!(
        domain_set,
        [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the domain must report only its remaining key, got {domain_set:?}"
    );
    let subscription_set = list_tags(&emulator, "the subscription after the domain's removal", &subscription_arn).await;
    assert_eq!(
        subscription_set,
        [("tier".to_string(), "gold".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the subscription's tags must not change when its domain is untagged, got \
         {subscription_set:?}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_and_removes_nothing() {
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
        .create_domain(&json!({ "name": "clinic-doomed", "tags": { "env": "doomed" } }))
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
            .send("DELETE", &untag_path(unresolved, &["env".to_string()]), None)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({unresolved}): expected 404 ResourceNotFoundException, \
             got {status} with {body}"
        );
        // The detail text is built from the REQUEST: it names the ARN the caller asked
        // for, which is what tells one 404 from another.
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the 404 must carry a Message, got {body}"));
        assert!(
            message.contains(unresolved.as_str()),
            "case {case:?}: the message must name the ARN the request asked for, got {message:?}"
        );
    }

    // None of those refusals reached the store: the live resource still carries its key.
    let reported = agreed_tags(&emulator, "after the refusals", &domain_id, &arn).await;
    assert_eq!(
        reported,
        [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "a refused UntagResource must remove nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_key_this_service_does_not_accept_is_refused_and_removes_nothing() {
    let emulator = Emulator::new("us-east-1");
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let one_past_the_longest_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let created = emulator
        .create_domain(&json!({
            "name": "clinic",
            "tags": { "env": "prod", longest_key.clone(): "long" },
        }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    // Each published bound of a tag KEY, and one past it, plus the namespace the service
    // reserves for the tags it sets itself — which names a tag no caller may set OR
    // remove, so a request naming one is refused rather than served.
    let rows: Vec<(&str, Vec<String>, u16)> = vec![
        ("a key at the maximum length", vec![longest_key.clone()], 204),
        (
            "a key one past the maximum length",
            vec![one_past_the_longest_key.clone()],
            400,
        ),
        ("a one-character key, the modelled minimum", vec!["k".to_string()], 204),
        ("an empty key, below the modelled minimum", vec![String::new()], 400),
        (
            "an acceptable key beside one past the maximum length",
            vec!["env".to_string(), one_past_the_longest_key.clone()],
            400,
        ),
        (
            "a key in the reserved namespace",
            vec![format!("{RESERVED_TAG_KEY_PREFIX}cloudformation")],
            400,
        ),
        (
            "a reserved key spelled in another case",
            vec![format!("{}owner", RESERVED_TAG_KEY_PREFIX.to_uppercase())],
            400,
        ),
        (
            "an acceptable key beside a reserved one",
            vec![
                "env".to_string(),
                format!("{RESERVED_TAG_KEY_PREFIX}owner"),
            ],
            400,
        ),
        (
            "a key that carries the reserved prefix somewhere other than its start",
            vec![format!("team{RESERVED_TAG_KEY_PREFIX}owner")],
            204,
        ),
    ];

    // Every row runs against a resource carrying the SAME set, so a refusal that removed
    // something shows up as a changed set; the two accepted rows name keys the resource
    // does not carry, which leaves the set unchanged too.
    let seeded: BTreeMap<String, String> = [
        ("env".to_string(), "prod".to_string()),
        (longest_key.clone(), "long".to_string()),
    ]
    .into_iter()
    .collect();
    for (case, keys, expected_status) in &rows {
        // `longest_key` IS carried, so the accepted row that names it would reduce the
        // set; it is re-tagged after each row instead of being special-cased.
        let (status, body) = emulator
            .send("DELETE", &untag_path(&arn, keys), None)
            .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if *expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body}"
            );
            let reported = agreed_tags(&emulator, case, &domain_id, &arn).await;
            assert_eq!(
                reported, seeded,
                "case {case:?}: a refused call must remove nothing, got {reported:?}"
            );
        } else {
            // Restore whatever the accepted row removed, so the next row starts from the
            // same set.
            tag_resource(
                &emulator,
                case,
                &arn,
                &json!({ "env": "prod", longest_key.clone(): "long" }),
            )
            .await;
            let reported = agreed_tags(&emulator, case, &domain_id, &arn).await;
            assert_eq!(
                reported, seeded,
                "case {case:?}: the set must be restored for the next row, got {reported:?}"
            );
        }
    }

    // The refusal says which key it refused and why, in the form a refusal of a bound is
    // worded: a length violation names the element's own position in the `tagKeys` list,
    // and the namespace refusal names the key and the prefix no caller may use.
    let (status, body) = emulator
        .send(
            "DELETE",
            &untag_path(
                &arn,
                &["env".to_string(), one_past_the_longest_key.clone()],
            ),
            None,
        )
        .await;
    assert_eq!(status, 400, "an over-long key: got {status} with {body}");
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the 400 must carry a Message, got {body}"));
    assert_eq!(
        message,
        format!(
            "1 validation error detected: Value '{one_past_the_longest_key}' at \
             'tagKeys.2.member' failed to satisfy constraint: Member must have length \
             less than or equal to {MAX_TAG_KEY_LEN}"
        ),
        "the refusal must name the element that carried the over-long key, got {message:?}"
    );

    let reserved_key = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation");
    let (status, body) = emulator
        .send(
            "DELETE",
            &untag_path(&arn, std::slice::from_ref(&reserved_key)),
            None,
        )
        .await;
    assert_eq!(status, 400, "a reserved key: got {status} with {body}");
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the 400 must carry a Message, got {body}"));
    assert!(
        message.contains(&reserved_key)
            && message.contains(RESERVED_TAG_KEY_PREFIX)
            && message.contains("tagKeys"),
        "the refusal must name the key, the reserved prefix and the member that carried \
         it, got {message:?}"
    );
}

#[tokio::test]
async fn the_number_of_keys_one_call_may_name_is_bounded() {
    let emulator = Emulator::new("ap-south-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    // Keys the resource does not carry, so every row below tests the COUNT alone: an
    // accepted call removes nothing and a refused one must remove nothing either.
    let absent: Vec<String> = (0..=MAX_TAG_KEYS_PER_CALL)
        .map(|index| format!("absent-{index}"))
        .collect();
    let accepted: Vec<(&str, &[String])> = vec![
        ("one key, the modelled minimum", &absent[..1]),
        ("half the maximum", &absent[..MAX_TAG_KEYS_PER_CALL / 2]),
        ("the maximum number of keys", &absent[..MAX_TAG_KEYS_PER_CALL]),
    ];
    for (case, keys) in &accepted {
        untag(&emulator, case, &arn, keys).await;
    }

    let refused: Vec<(&str, String)> = vec![(
        "one key past the maximum",
        untag_path(&arn, &absent[..MAX_TAG_KEYS_PER_CALL + 1]),
    )];
    for (case, path) in &refused {
        let (status, body) = emulator.send("DELETE", path, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: got {body}"
        );
    }
    // A request that names NO key at all is not this operation's to refuse: `tagKeys` is
    // required, and the REST router rejects a candidate route whose required bindings the
    // request does not carry, so such a request reaches no operation of this service and
    // is answered by the front door rather than by a handler.

    // Nothing any of those calls named was carried, so the seeded key is still there.
    let reported = agreed_tags(&emulator, "after the bound rows", &domain_id, &arn).await;
    assert_eq!(
        reported,
        [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "a call naming only absent keys must leave the set alone, got {reported:?}"
    );
}
