// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the EventBridge Scheduler tagging surface:
//! `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! Every request goes through the real restJson1 front door —
//! [`MultiServiceRouter::dispatch`] — so URI-label and query binding, constraint
//! validation, the handler and response serialization are all exercised against
//! one shared engine, which is what makes a tag written by one request readable
//! by the next.
//!
//! The rule under test: a schedule group's tag set is a MAP keyed by tag key.
//! `TagResource` merges the request's pairs into it (a new key is added, a key
//! already held takes the new value), `UntagResource` drops the named keys and is
//! idempotent, and `ListTagsForResource` reports exactly the stored set, byte for
//! byte. A key under the reserved `aws:` prefix is refused with 400
//! `ValidationException` and nothing is stored, and an ARN that names no schedule
//! group in this account and Region answers 404 `ResourceNotFoundException`.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{
    account_id, call, encode_label, error_code, group_arn, list_tags, scheduler_router, tags_of,
    REGION,
};

/// One tag as a case writes it.
type Pair = (String, String);

/// Create a schedule group and answer its ARN.
async fn create_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", &format!("/schedule-groups/{name}"), None).await;
    assert_eq!(
        status, 200,
        "group {name:?}: CreateScheduleGroup got {status} {body}"
    );
    body.get("ScheduleGroupArn")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("group {name:?}: no ScheduleGroupArn in {body}"))
        .to_string()
}

/// The pairs a case writes, from the literals that spell them.
fn pairs(items: &[(&str, &str)]) -> Vec<Pair> {
    items
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

async fn tag(router: &MultiServiceRouter, arn: &str, tags: &[Pair]) -> (u16, serde_json::Value) {
    let tags: Vec<serde_json::Value> = tags
        .iter()
        .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
        .collect();
    call(
        router,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(serde_json::json!({ "Tags": tags })),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, serde_json::Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("TagKeys={}", encode_label(key)))
        .collect();
    let uri = format!("/tags/{}?{}", encode_label(arn), query.join("&"));
    call(router, "DELETE", &uri, None).await
}

/// The tag set `tags` stands for — the last value wins for a repeated key, the
/// way a map keyed by tag key behaves.
fn expected_map(tags: &[Pair]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// One row of the round-trip table: a group to tag and the tags to put on it.
struct TagCase {
    case: &'static str,
    group: &'static str,
    tags: Vec<Pair>,
}

/// The classes a tag set varies over: how many pairs one call carries, and what
/// a key and a value are made of. `Tag$Key` is modelled `length 1..128` and
/// `Tag$Value` `length 1..256` (`smithy_query(query="shape",shape="Tag")`), so
/// the bound rows sit exactly on those published maxima and minima; the character
/// rows cover the punctuation AWS allows in a tag (`+ - = . _ : / @`, plus
/// spaces) and non-ASCII text, which is what makes URI-label and query encoding
/// observable.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "one plain ASCII pair",
            group: "plain-ascii",
            tags: pairs(&[("Environment", "production")]),
        },
        TagCase {
            case: "several pairs in one call",
            group: "several-pairs",
            tags: pairs(&[
                ("Environment", "staging"),
                ("Owner", "platform"),
                ("CostCenter", "4711"),
                ("Project", "scheduler.emulator"),
            ]),
        },
        TagCase {
            case: "non-ASCII key and value",
            group: "unicode.tags",
            tags: pairs(&[
                ("プロジェクト", "本番環境 🚀"),
                ("Ümlaut-Schlüssel", "Grüße, Welt"),
                ("ключ", "значение"),
            ]),
        },
        TagCase {
            case: "punctuation AWS allows in a tag, which the URI must survive",
            group: "punctuated_tags",
            tags: pairs(&[
                ("path/key:1+2 3=x@y", "a/b:c+d e=f@g"),
                ("dot.key_under-score", "value.with_under-score"),
            ]),
        },
        TagCase {
            case: "key and value at their modelled upper bound",
            group: "bound-tags",
            tags: vec![("k".repeat(128), "v".repeat(256))],
        },
        TagCase {
            case: "single-character key and value, the modelled lower bound",
            group: "short-tags",
            tags: pairs(&[("k", "v")]),
        },
        TagCase {
            case: "a value that looks like an encoded tag list",
            group: "lookalike-tags",
            tags: pairs(&[("Tags", "a=b,c=d&e=f")]),
        },
    ]
}

#[tokio::test]
async fn tag_list_untag_round_trips_every_tag_shape() {
    let router = scheduler_router();

    for TagCase { case, group, tags } in tag_cases() {
        let arn = create_group(&router, group).await;

        // A group starts with no tags, and that is reported as an empty list,
        // never as a missing resource.
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, tags_of(&body)),
            (200, BTreeMap::new()),
            "case {case:?}: a fresh group must report no tags, got {status} {body}"
        );

        let (status, body) = tag(&router, &arn, &tags).await;
        assert_eq!(status, 200, "case {case:?}: TagResource got {status} {body}");

        let (status, body) = list_tags(&router, &arn).await;
        let listed = tags_of(&body);
        assert_eq!(
            (status, &listed),
            (200, &expected_map(&tags)),
            "case {case:?}: expected the tags just written, got {status} {listed:?}"
        );

        // Removing the first key leaves every other pair exactly as it was.
        let removed = tags[0].0.clone();
        let (status, body) = untag(&router, &arn, &[removed.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource({removed:?}) got {status} {body}"
        );
        let expected = expected_map(&tags[1..]);
        let (status, body) = list_tags(&router, &arn).await;
        let listed = tags_of(&body);
        assert_eq!(
            (status, &listed),
            (200, &expected),
            "case {case:?}: after removing {removed:?} expected {expected:?}, got {listed:?}"
        );

        // And it is idempotent: removing the same key again changes nothing.
        let (status, body) = untag(&router, &arn, &[removed.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated UntagResource must succeed, got {status} {body}"
        );
        let (status, body) = list_tags(&router, &arn).await;
        let listed = tags_of(&body);
        assert_eq!(
            (status, &listed),
            (200, &expected),
            "case {case:?}: a repeated UntagResource must change nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn tagging_merges_by_key_so_a_held_key_takes_the_new_value() {
    let router = scheduler_router();
    let arn = create_group(&router, "merge-group").await;

    /// One step of the merge table: what this call sends, and the whole tag set
    /// the group must hold afterwards.
    struct MergeStep {
        case: &'static str,
        sent: Vec<Pair>,
        expected: Vec<Pair>,
    }
    let step = |case, sent: &[(&str, &str)], expected: &[(&str, &str)]| MergeStep {
        case,
        sent: pairs(sent),
        expected: pairs(expected),
    };

    let steps = vec![
        step(
            "first write",
            &[("a", "1"), ("b", "2")],
            &[("a", "1"), ("b", "2")],
        ),
        step(
            "a new key is added, the held ones stay",
            &[("c", "3")],
            &[("a", "1"), ("b", "2"), ("c", "3")],
        ),
        step(
            "a held key takes the new value rather than appearing twice",
            &[("b", "two")],
            &[("a", "1"), ("b", "two"), ("c", "3")],
        ),
        step(
            "same key twice in one call: the last value wins",
            &[("a", "first"), ("a", "last")],
            &[("a", "last"), ("b", "two"), ("c", "3")],
        ),
        step(
            "an empty tag list is accepted and changes nothing",
            &[],
            &[("a", "last"), ("b", "two"), ("c", "3")],
        ),
        step(
            "a key differing only in case is a different key",
            &[("A", "upper")],
            &[("A", "upper"), ("a", "last"), ("b", "two"), ("c", "3")],
        ),
    ];

    for MergeStep {
        case,
        sent,
        expected,
    } in steps
    {
        let (status, body) = tag(&router, &arn, &sent).await;
        assert_eq!(status, 200, "case {case:?}: TagResource got {status} {body}");
        let (status, body) = list_tags(&router, &arn).await;
        let listed = tags_of(&body);
        let expected = expected_map(&expected);
        assert_eq!(
            (status, &listed),
            (200, &expected),
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Untag takes a SET of keys, and ignores the ones the group never carried.
    let (status, body) = untag(&router, &arn, &["a", "A", "never-set"]).await;
    assert_eq!(status, 200, "UntagResource got {status} {body}");
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    let expected = expected_map(&pairs(&[("b", "two"), ("c", "3")]));
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "case \"remove a set of keys\": expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let router = scheduler_router();
    let arn = create_group(&router, "reserved-keys").await;
    let held = pairs(&[("Environment", "production")]);
    let (status, body) = tag(&router, &arn, &held).await;
    assert_eq!(status, 200, "seeding a tag got {status} {body}");

    // The `aws:` prefix is reserved for AWS use, whatever its case and whatever
    // else the key says, and the refusal is a request refusal: nothing is stored.
    for case in [
        "aws:",
        "aws:cloudformation:stack-name",
        "AWS:Owner",
        "Aws:billing",
        "aWs:x",
    ] {
        let (status, body) = tag(&router, &arn, &pairs(&[(case, "v")])).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a reserved tag key must be refused, got {status} {body}"
        );

        let (status, body) = untag(&router, &arn, &[case]).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: removing a reserved tag key must be refused, got {status} {body}"
        );
    }

    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    let expected = expected_map(&held);
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "a refused request must store nothing: expected {expected:?}, got {listed:?}"
    );

    // A key that merely LOOKS reserved is an ordinary key.
    for case in ["awsome", "my-aws:key", "AWSOME", "a-aws:", "ws:aws"] {
        let (status, body) = tag(&router, &arn, &pairs(&[(case, "allowed")])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a key that only looks reserved must be accepted, got {status} {body}"
        );
        let (status, body) = list_tags(&router, &arn).await;
        let listed = tags_of(&body);
        assert_eq!(
            (status, listed.get(case).map(String::as_str)),
            (200, Some("allowed")),
            "case {case:?}: expected the key to be stored, got {listed:?}"
        );
        let (status, body) = untag(&router, &arn, &[case]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource got {status} {body}"
        );
    }
}

#[tokio::test]
async fn create_schedule_group_applies_the_same_tag_rules() {
    let router = scheduler_router();

    // Tags supplied at create time are the same tag set TagResource merges into,
    // so they are readable through ListTagsForResource and obey the same rules.
    let (status, body) = call(
        &router,
        "POST",
        "/schedule-groups/created-with-tags",
        Some(serde_json::json!({
            "Tags": [
                { "Key": "Environment", "Value": "production" },
                { "Key": "Owner", "Value": "platform" },
                { "Key": "Environment", "Value": "production-2" }
            ]
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateScheduleGroup with tags got {status} {body}"
    );
    let arn = group_arn(REGION, "created-with-tags");
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    let expected = expected_map(&pairs(&[("Environment", "production-2"), ("Owner", "platform")]));
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "case \"tags at create time\": expected {expected:?}, got {listed:?}"
    );

    let (status, body) = call(
        &router,
        "POST",
        "/schedule-groups/created-with-reserved-tag",
        Some(serde_json::json!({
            "Tags": [{ "Key": "aws:owner", "Value": "x" }]
        })),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"reserved key at create time\": got {status} {body}"
    );
}

#[tokio::test]
async fn the_built_in_default_group_holds_tags_too() {
    let router = scheduler_router();
    // The `default` group exists for every account without being created, so its
    // ARN is taggable without a CreateScheduleGroup call.
    let arn = group_arn(REGION, "default");

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        (status, tags_of(&body)),
        (200, BTreeMap::new()),
        "the default group must exist and report no tags, got {status} {body}"
    );

    let tags = pairs(&[("Environment", "shared"), ("Owner", "platform")]);
    let (status, body) = tag(&router, &arn, &tags).await;
    assert_eq!(status, 200, "tagging the default group got {status} {body}");
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    let expected = expected_map(&tags);
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "case \"default group\": expected {expected:?}, got {listed:?}"
    );

    let (status, body) = untag(&router, &arn, &["Environment"]).await;
    assert_eq!(
        status, 200,
        "untagging the default group got {status} {body}"
    );
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    let expected = expected_map(&tags[1..]);
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "case \"default group\": expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_no_group_in_this_account_and_region_is_not_found() {
    let router = scheduler_router();
    let present = create_group(&router, "present-group").await;
    let held = pairs(&[("Environment", "production")]);
    let (status, body) = tag(&router, &present, &held).await;
    assert_eq!(status, 200, "seeding a tag got {status} {body}");

    let account = account_id();
    // Each row is a well-formed TagResourceArn that addresses no schedule group
    // reachable from this endpoint: a name nobody created, another Region,
    // another account, another partition.
    let cases: Vec<(&str, String)> = vec![
        ("a group nobody created", group_arn(REGION, "never-created")),
        ("another Region", group_arn("us-east-1", "present-group")),
        (
            "another account",
            format!("arn:aws:scheduler:{REGION}:123456789012:schedule-group/present-group"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:scheduler:{REGION}:{account}:schedule-group/present-group"),
        ),
    ];

    let sent = pairs(&[("k", "v")]);
    for (case, arn) in cases {
        for (method, status_body) in [
            ("TagResource", tag(&router, &arn, &sent).await),
            ("UntagResource", untag(&router, &arn, &["k"]).await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            let (status, body) = status_body;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} ({method}): expected a not-found refusal, got {status} {body}"
            );
        }
    }

    // The present group is untouched by every refusal above.
    let (status, body) = list_tags(&router, &present).await;
    let listed = tags_of(&body);
    let expected = expected_map(&held);
    assert_eq!(
        (status, &listed),
        (200, &expected),
        "the addressed group must be unchanged: expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_that_is_not_a_schedule_group_arn_is_refused() {
    let router = scheduler_router();
    let account = account_id();
    // `TagResourceArn` is modelled
    // `^arn:aws[a-z-]*:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+$`
    // (`smithy_query(query="operation",operation="TagResource")`), so anything
    // else — a schedule's own ARN included — is a request-validation refusal, not
    // a missing resource.
    let cases: Vec<(&str, String)> = vec![
        (
            "a schedule ARN, not a schedule group ARN",
            format!("arn:aws:scheduler:{REGION}:{account}:schedule/default/some-schedule"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{account}:schedule-group/queue"),
        ),
        (
            "an account id that is not 12 digits",
            format!("arn:aws:scheduler:{REGION}:12345:schedule-group/present"),
        ),
        ("not an ARN at all", "present-group".to_string()),
    ];

    let sent = pairs(&[("k", "v")]);
    for (case, arn) in cases {
        for (method, status_body) in [
            ("TagResource", tag(&router, &arn, &sent).await),
            ("UntagResource", untag(&router, &arn, &["k"]).await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            let (status, body) = status_body;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({method}): expected a validation refusal, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_tag_value_is_refused_by_the_modelled_length_bound() {
    let router = scheduler_router();
    let arn = create_group(&router, "empty-value").await;
    // `Tag$Value` is modelled `length 1..256`
    // (`smithy_query(query="shape",shape="Tag")`), so an empty value cannot be
    // stored: the boundary validator refuses the request before the handler runs,
    // and the group keeps the tag set it had. What must never happen is a silent
    // success that drops the pair or stores it as absent.
    let (status, body) = tag(&router, &arn, &pairs(&[("Empty", ""), ("Full", "v")])).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"empty tag value\": the modelled length bound makes this a \
         validation refusal, got {status} {body}"
    );
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    assert_eq!(
        (status, &listed),
        (200, &BTreeMap::new()),
        "case \"empty tag value\": a refused request must store nothing, got {listed:?}"
    );

    // One character is inside the bound, and round-trips.
    let (status, body) = tag(&router, &arn, &pairs(&[("Empty", " ")])).await;
    assert_eq!(
        status, 200,
        "case \"single-space value\": got {status} {body}"
    );
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    assert_eq!(
        (status, listed.get("Empty").map(String::as_str)),
        (200, Some(" ")),
        "case \"single-space value\": expected it stored as sent, got {listed:?}"
    );
}

#[tokio::test]
async fn a_merge_past_the_modelled_tag_bound_is_refused() {
    let router = scheduler_router();
    let arn = create_group(&router, "bulk-tags").await;

    // `TagList` is modelled `length 0..200`, so 200 pairs is the most one group
    // can hold and still be reported by ListTagsForResource.
    let full: Vec<Pair> = (0..200)
        .map(|index| (format!("bulk-{index:03}"), "held".to_string()))
        .collect();
    let (status, body) = tag(&router, &arn, &full).await;
    assert_eq!(
        status, 200,
        "case \"200 tags, the modelled maximum\": got {status} {body}"
    );

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        (status, tags_of(&body).len()),
        (200, full.len()),
        "case \"200 tags, the modelled maximum\": got {status} {body}"
    );

    // One more distinct key would carry the group past the bound.
    let (status, body) = tag(&router, &arn, &pairs(&[("bulk-200", "one too many")])).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"one tag past the bound\": got {status} {body}"
    );

    // A key already held is not a new key, so re-tagging it stays inside it.
    let held = full[0].0.clone();
    let (status, body) = tag(&router, &arn, &[(held.clone(), "replaced".to_string())]).await;
    assert_eq!(
        status, 200,
        "case \"replacing a held key at the bound\": got {status} {body}"
    );
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    assert_eq!(
        (status, listed.len(), listed.get(&held).map(String::as_str)),
        (200, 200, Some("replaced")),
        "case \"replacing a held key at the bound\": got {listed:?}"
    );
}
