// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TagResource`: which tags one call leaves the
//! Amazon Translate resource an ARN names holding, read back through
//! `ListTagsForResource`.
//!
//! Every test drives the service the way the router does, through the harness
//! `terminology_lifecycle` already builds (`router`, `call`, `error_code`,
//! `expected_arn`, `import`, `message`), so the wire protocol, the boundary
//! constraint validation and the handlers are all exercised, and what one
//! request writes is what the next request reads.
//!
//! The tests assert the RULE over a set of inputs rather than one recorded
//! request: the number of tags one call carries (none, one, several, enough to
//! reach the per-resource limit), keys the resource already holds against keys
//! it does not, a key restated inside one call, keys differing only in case,
//! values at the model's length bounds, and the same call repeated.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::terminology_lifecycle::{
    call, error_code, expected_arn, import, listed_tags, message, router, string_at, tag_list,
    tag_pairs, EN_FR_CSV,
};

/// The documented tag limit for one resource: "There can be a maximum of 50
/// tags (both existing and pending) associated with a specific resource"
/// (`smithy_query shape=TagResourceRequest member=Tags`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest key one tag may carry: `TagKey` is `length 1..128`
/// (`smithy_query shape=Tag member=Key`).
const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest value one tag may carry: `TagValue` is `length 0..256`
/// (`smithy_query shape=Tag member=Value`).
const MAX_TAG_VALUE_CHARS: usize = 256;

/// One row of the table: the tags the resource is left holding after one
/// `TagResource` call carrying `requested` against a resource imported holding
/// `held`.
struct Row {
    case: &'static str,
    held: Vec<(String, String)>,
    requested: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The tags the resource reports, keyed by tag key so a row states the SET one
/// call leaves rather than an order the service does not document. A duplicate
/// key would collapse here, so the reported count is asserted beside it.
async fn held_tags(router: &MultiServiceRouter, arn: &str) -> (BTreeMap<String, String>, usize) {
    let reported = listed_tags(router, "the tagged resource", arn).await;
    let set = reported
        .iter()
        .map(|tag| {
            (
                string_at(tag, "/Key"),
                tag.get("Value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    (set, reported.len())
}

/// Import a terminology under `name`, carrying `held` as its tags.
async fn import_holding(
    router: &MultiServiceRouter,
    name: &str,
    held: &[(String, String)],
) -> String {
    let (status, imported) = if held.is_empty() {
        import(router, name, EN_FR_CSV, "CSV", Some("UNI")).await
    } else {
        call(
            router,
            "ImportTerminology",
            &json!({
                "Name": name,
                "MergeStrategy": "OVERWRITE",
                "TerminologyData": {
                    "File": aws_smithy_types::base64::encode(EN_FR_CSV),
                    "Format": "CSV",
                    "Directionality": "UNI",
                },
                "Tags": tag_list(held),
            }),
        )
        .await
    };
    assert_eq!(
        status, 200,
        "{name}: ImportTerminology answered {status} {imported}"
    );
    expected_arn(name, EN_FR_CSV)
}

fn rows() -> Vec<Row> {
    // A resource holding a handful of tags and a call adding enough new keys to
    // leave it exactly at the per-resource limit.
    let few: Vec<(String, String)> = (0..10)
        .map(|index| (format!("held{index:02}"), format!("v{index}")))
        .collect();
    let fills_the_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - few.len())
        .map(|index| (format!("added{index:02}"), format!("v{index}")))
        .collect();
    let at_the_limit: Vec<(String, String)> = few
        .iter()
        .chain(fills_the_limit.iter())
        .cloned()
        .collect();
    // The same resource, every held key restated with a new value: the limit
    // counts tags, so restating cannot push it over.
    let restated: Vec<(String, String)> = few
        .iter()
        .map(|(key, _)| (key.clone(), "restated".to_string()))
        .collect();

    vec![
        Row {
            case: "a request carrying no tag at all",
            held: tag_pairs(&[("env", "prod")]),
            requested: Vec::new(),
            expected: tag_pairs(&[("env", "prod")]),
        },
        Row {
            case: "one tag onto a resource holding none",
            held: Vec::new(),
            requested: tag_pairs(&[("env", "prod")]),
            expected: tag_pairs(&[("env", "prod")]),
        },
        Row {
            case: "several tags onto a resource holding none",
            held: Vec::new(),
            requested: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
            expected: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
        },
        Row {
            case: "every requested key is one the resource does not hold",
            held: tag_pairs(&[("env", "prod")]),
            requested: tag_pairs(&[("owner", "platform"), ("cost", "42")]),
            expected: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
        },
        Row {
            case: "a requested key the resource already holds takes the new value",
            held: tag_pairs(&[("env", "staging"), ("owner", "platform")]),
            requested: tag_pairs(&[("env", "prod")]),
            expected: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "a held key beside a new one",
            held: tag_pairs(&[("env", "staging"), ("owner", "platform")]),
            requested: tag_pairs(&[("env", "prod"), ("cost", "42")]),
            expected: tag_pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
        },
        Row {
            case: "every held key restated",
            held: tag_pairs(&[("env", "staging"), ("owner", "platform")]),
            requested: tag_pairs(&[("owner", "nlp"), ("env", "prod")]),
            expected: tag_pairs(&[("env", "prod"), ("owner", "nlp")]),
        },
        Row {
            case: "one key stated twice in the same call",
            held: tag_pairs(&[("owner", "platform")]),
            requested: tag_pairs(&[("env", "first"), ("env", "last")]),
            expected: tag_pairs(&[("owner", "platform"), ("env", "last")]),
        },
        Row {
            case: "a key differing from a held one only in case",
            held: tag_pairs(&[("Env", "prod")]),
            requested: tag_pairs(&[("env", "dev")]),
            expected: tag_pairs(&[("Env", "prod"), ("env", "dev")]),
        },
        Row {
            case: "a held value replaced by the empty string",
            held: tag_pairs(&[("env", "prod"), ("owner", "platform")]),
            requested: tag_pairs(&[("env", "")]),
            expected: tag_pairs(&[("env", ""), ("owner", "platform")]),
        },
        Row {
            case: "a value at the model's maximum length",
            held: tag_pairs(&[("env", "prod")]),
            requested: vec![("long-value".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS))],
            expected: vec![
                ("env".to_string(), "prod".to_string()),
                ("long-value".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS)),
            ],
        },
        Row {
            case: "a key at the model's maximum length",
            held: tag_pairs(&[("env", "prod")]),
            requested: vec![("k".repeat(MAX_TAG_KEY_CHARS), "long".to_string())],
            expected: vec![
                ("env".to_string(), "prod".to_string()),
                ("k".repeat(MAX_TAG_KEY_CHARS), "long".to_string()),
            ],
        },
        Row {
            case: "a request that fills the resource to the tag limit",
            held: few.clone(),
            requested: fills_the_limit,
            expected: at_the_limit,
        },
        Row {
            case: "every key of a resource restated at once",
            held: few,
            requested: restated.clone(),
            expected: restated,
        },
    ]
}

/// One call merges the tags it carries into the tags the resource already
/// holds: a key the resource holds takes the request's value, every other key
/// is added, and no other tag changes — whatever the number of keys the call
/// names and whether or not the resource holds them. Merging by key is what
/// makes the call idempotent, so the same call repeated leaves the same tag set.
#[tokio::test]
async fn tag_resource_merges_the_requested_tags_by_key() {
    let router = router();

    for (index, row) in rows().into_iter().enumerate() {
        let name = format!("tag-{index:02}-terms");
        let arn = import_holding(&router, &name, &row.held).await;
        let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();

        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tag_list(&row.requested)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: TagResource answered {status} {answered}",
            row.case
        );
        assert_eq!(
            error_code(&answered),
            "<no __type>",
            "case {:?}: a merge names no error, got {answered}",
            row.case
        );

        let (held, reported) = held_tags(&router, &arn).await;
        assert_eq!(
            held, expected,
            "case {:?}: expected the resource to be left holding {expected:?}, got {held:?}",
            row.case
        );
        assert_eq!(
            reported,
            expected.len(),
            "case {:?}: a key is carried once, got {reported} reported tags for {held:?}",
            row.case
        );

        // The same call again: the merge is addressed by key, so a key already
        // carrying the requested value adds nothing more.
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tag_list(&row.requested)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the repeated call answered {status} {answered}",
            row.case
        );
        let (held_again, reported_again) = held_tags(&router, &arn).await;
        assert_eq!(
            held_again, expected,
            "case {:?}: the repeated call must leave {expected:?}, got {held_again:?}",
            row.case
        );
        assert_eq!(
            reported_again,
            expected.len(),
            "case {:?}: the repeated call duplicates no key, got {reported_again} reported tags",
            row.case
        );
    }
}

/// A merge addresses one resource: the ARN it names, not another resource
/// holding the same keys, and not the resource's own stored content.
#[tokio::test]
async fn tag_resource_touches_only_the_resource_its_arn_names() {
    let router = router();
    let shared = tag_pairs(&[("env", "staging")]);
    let first = import_holding(&router, "first-tag-terms", &shared).await;
    let second = import_holding(&router, "second-tag-terms", &shared).await;

    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": first, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {status} {answered}");

    let (held_first, _) = held_tags(&router, &first).await;
    assert_eq!(
        held_first,
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "the named resource takes the requested value, got {held_first:?}"
    );
    let (held_second, _) = held_tags(&router, &second).await;
    assert_eq!(
        held_second,
        BTreeMap::from([("env".to_string(), "staging".to_string())]),
        "another resource holding the same key keeps its own value, got {held_second:?}"
    );

    // The resource itself is untouched: only its tags changed.
    let (status, fetched) = call(&router, "GetTerminology", &json!({"Name": "first-tag-terms"})).await;
    assert_eq!(status, 200, "GetTerminology answered {status} {fetched}");
    assert_eq!(
        fetched
            .pointer("/TerminologyProperties/TermCount")
            .and_then(Value::as_i64),
        Some(2),
        "adding a tag must not change the stored terminology, got {fetched}"
    );
}

/// A refused merge writes nothing: whether the request is refused because this
/// account holds no resource with that ARN, because the resulting tag set would
/// pass the per-resource limit, or because a tag is outside the length the model
/// admits, the resource keeps exactly the tags it held.
#[tokio::test]
async fn a_refused_merge_leaves_the_stored_tags_as_they_were() {
    let router = router();
    let held = tag_pairs(&[("env", "prod"), ("owner", "platform")]);
    let arn = import_holding(&router, "refused-tag-terms", &held).await;
    let expected: BTreeMap<String, String> = held.iter().cloned().collect();

    // Past the per-resource limit: the resource holds two tags, so a call
    // carrying every remaining slot plus one is refused.
    let over_the_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - held.len() + 1)
        .map(|index| (format!("added{index:02}"), "v".to_string()))
        .collect();
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tag_list(&over_the_limit)}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a merge past the tag limit is refused, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "TooManyTagsException",
        "got {answered}"
    );
    assert_eq!(
        message(&answered),
        "Allowable tag limit for resource exceeded",
        "got {answered}"
    );
    assert_eq!(
        string_at(&answered, "/ResourceArn"),
        arn,
        "the refusal names the resource it refused, got {answered}"
    );
    let (after_limit, _) = held_tags(&router, &arn).await;
    assert_eq!(
        after_limit, expected,
        "the refused merge applies none of its tags, got {after_limit:?}"
    );

    // One character past the key length the model admits (`TagKey` is
    // `length 1..128`), so the boundary walk refuses the request before the
    // handler sees it.
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [
            {"Key": "k".repeat(MAX_TAG_KEY_CHARS + 1), "Value": "v"}
        ]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a key past the modelled length is refused, got {status} {answered}"
    );
    let (after_long_key, _) = held_tags(&router, &arn).await;
    assert_eq!(
        after_long_key, expected,
        "the refused merge applies none of its tags, got {after_long_key:?}"
    );

    // An ARN this account holds no resource for, with the tag counts around it:
    // none, one and several. A stored resource exists, so the refusal is about
    // the ARN rather than about an empty store.
    let unknown = expected_arn("no-such-terms", b"no-such-file");
    let cases: [(&str, Vec<(String, String)>); 3] = [
        ("no tag", Vec::new()),
        ("one tag", tag_pairs(&[("env", "prod")])),
        (
            "several tags",
            tag_pairs(&[("env", "prod"), ("owner", "platform")]),
        ),
    ];
    for (case, requested) in cases {
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": unknown, "Tags": tag_list(&requested)}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a not-found refusal, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: got {answered}"
        );
        assert_eq!(
            message(&answered),
            "The resource you are looking for has not been found. Review the resource \
             you're looking for and see if a different resource will accomplish your \
             needs before retrying the revised request.",
            "case {case:?}: got {answered}"
        );
    }

    let (finally, _) = held_tags(&router, &arn).await;
    assert_eq!(
        finally, expected,
        "every refused merge leaves the stored resource's tags as they were, got {finally:?}"
    );
}
