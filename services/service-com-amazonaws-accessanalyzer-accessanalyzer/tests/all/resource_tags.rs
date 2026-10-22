// SPDX-License-Identifier: CC0-1.0

//! The tagging surface, driven over the wire: `TagResource`, `ListTagsForResource` and
//! `UntagResource` against the resource their `resourceArn` names.
//!
//! The rule under test is that a resource carries ONE tag set held in its own record:
//! tagging merges the request's keys into it, untagging drops the keys it names, and
//! every reader of that resource — `ListTagsForResource` and `GetAnalyzer` alike —
//! reports the same set. Every case goes through the real router (restJson1 routing,
//! boundary validation, handler, serialization), and nothing reaches into
//! `crate::state`: a write is only believed when a later read reports it back.
//!
//! The tables span what the rule generalises over — how many tags, whether a key is new
//! or already held, the key and value form (ASCII, Unicode, empty value, each published
//! length bound and one past it), and how many calls the set was built up over. The
//! recorded `Owner`/`team-a`-style single-tag round trip is one row.

use serde_json::{json, Map, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

// The wire harness is shared by every suite in this target; see `tests/all/harness.rs`.
// The three tagging calls (and the `@httpLabel` encoding they need) live there, so this
// suite and `no_content_responses.rs` drive the same ones.
use crate::harness::{expected_arn, Emulator, Reply};

/// `POST /tags/{resourceArn}` with the given tag map.
async fn tag_resource(emulator: &Emulator, resource_arn: &str, tags: &Value) -> Reply {
    emulator.tag_resource(resource_arn, tags).await
}

/// `GET /tags/{resourceArn}`.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator.list_tags_for_resource(resource_arn).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`, one query entry per key (`tagKeys` is an
/// `@httpQuery` list).
async fn untag_resource(emulator: &Emulator, resource_arn: &str, keys: &[&str]) -> Reply {
    emulator.untag_resource(resource_arn, keys).await
}

/// Create an `ACCOUNT` analyzer, optionally carrying create-time tags, and answer the
/// ARN it must have.
async fn create_analyzer(emulator: &Emulator, name: &str, tags: Option<Value>) -> String {
    let mut body = json!({ "analyzerName": name, "type": "ACCOUNT" });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let reply = emulator.create_analyzer(body).await;
    assert_eq!(
        reply.status, 200,
        "creating analyzer {name}: expected 200, got {reply:?}"
    );
    expected_arn(name)
}

/// The tag set as a JSON object, for comparison against a response member.
fn tag_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// A `(key, value)` pair from borrowed literals, for the tables.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// A key or value of exactly `chars` characters, so a bound can be tested AT it and one
/// past it.
fn text_of(chars: usize) -> String {
    "k".repeat(chars)
}

/// One row of the tag/read-back table.
struct TagCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    analyzer: &'static str,
    /// The `tags` the create carried, `None` when it carried none.
    created_with: Option<Value>,
    /// The successive `TagResource` calls this row makes.
    calls: Vec<Vec<(String, String)>>,
    /// The tag set every reader must report afterwards.
    expected: Vec<(String, String)>,
}

/// Tagging merges into the resource's tag set, and every reader of the resource reports
/// the set the last call left behind.
#[tokio::test]
async fn tagging_merges_into_the_tag_set_and_reads_back() {
    let cases = vec![
        TagCase {
            case: "one tag onto an untagged resource (the recorded round trip)",
            analyzer: "recorded_roundtrip",
            created_with: None,
            calls: vec![vec![tag("Owner", "team-a")]],
            expected: vec![tag("Owner", "team-a")],
        },
        TagCase {
            case: "a tag added beside the create-time tags",
            analyzer: "merged_with_create_tags",
            created_with: Some(json!({ "env": "dev" })),
            calls: vec![vec![tag("Owner", "team-a")]],
            expected: vec![tag("env", "dev"), tag("Owner", "team-a")],
        },
        TagCase {
            case: "a key the resource already holds takes the new value",
            analyzer: "overwritten_key",
            created_with: Some(json!({ "env": "dev", "Owner": "team-a" })),
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![tag("env", "prod"), tag("Owner", "team-a")],
        },
        TagCase {
            case: "many tags in one call",
            analyzer: "many_at_once",
            created_with: None,
            calls: vec![(0..8).map(|n| tag(&format!("k{n}"), &format!("v{n}"))).collect()],
            expected: (0..8).map(|n| tag(&format!("k{n}"), &format!("v{n}"))).collect(),
        },
        TagCase {
            case: "successive calls accumulate rather than replace",
            analyzer: "accumulated",
            created_with: None,
            calls: vec![
                vec![tag("a", "1")],
                vec![tag("b", "2")],
                vec![tag("c", "3"), tag("d", "4")],
            ],
            expected: vec![tag("a", "1"), tag("b", "2"), tag("c", "3"), tag("d", "4")],
        },
        TagCase {
            case: "the same call repeated is idempotent",
            analyzer: "repeated_call",
            created_with: None,
            calls: vec![vec![tag("a", "1")], vec![tag("a", "1")]],
            expected: vec![tag("a", "1")],
        },
        TagCase {
            case: "an empty tag map changes nothing",
            analyzer: "empty_request_map",
            created_with: Some(json!({ "env": "dev" })),
            calls: vec![vec![]],
            expected: vec![tag("env", "dev")],
        },
        TagCase {
            case: "an empty tag map on a resource that has never carried tags",
            analyzer: "empty_request_map_untagged",
            created_with: None,
            calls: vec![vec![]],
            expected: vec![],
        },
        TagCase {
            case: "an empty value is a value",
            analyzer: "empty_value",
            created_with: None,
            calls: vec![vec![tag("Owner", "")]],
            expected: vec![tag("Owner", "")],
        },
        TagCase {
            case: "a non-ASCII key and value survive the round trip",
            analyzer: "unicode_tag",
            created_with: None,
            calls: vec![vec![tag("プロジェクト", "Ωmega")]],
            expected: vec![tag("プロジェクト", "Ωmega")],
        },
        TagCase {
            case: "a key and a value at their longest accepted length",
            analyzer: "at_the_length_bounds",
            created_with: None,
            calls: vec![vec![tag(
                &text_of(MAX_TAG_KEY_CHARS),
                &text_of(MAX_TAG_VALUE_CHARS),
            )]],
            expected: vec![tag(
                &text_of(MAX_TAG_KEY_CHARS),
                &text_of(MAX_TAG_VALUE_CHARS),
            )],
        },
        TagCase {
            case: "a key of a single character",
            analyzer: "shortest_key",
            created_with: None,
            calls: vec![vec![tag("k", "v")]],
            expected: vec![tag("k", "v")],
        },
        TagCase {
            case: "the resource holds the per-resource maximum",
            analyzer: "at_the_count_bound",
            created_with: None,
            calls: vec![(0..MAX_TAGS_PER_RESOURCE)
                .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
                .collect()],
            expected: (0..MAX_TAGS_PER_RESOURCE)
                .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
                .collect(),
        },
    ];

    let emulator = Emulator::new();
    for case in cases {
        let TagCase {
            case,
            analyzer,
            created_with,
            calls,
            expected,
        } = case;
        let arn = create_analyzer(&emulator, analyzer, created_with).await;

        for tags in &calls {
            let reply = tag_resource(&emulator, &arn, &tag_object(tags)).await;
            assert_eq!(
                reply.status, 204,
                "case {case:?}: expected TagResource to answer 204 No Content, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "",
                "case {case:?}: expected TagResource to answer no error, got {reply:?}"
            );
        }

        let expected_tags = tag_object(&expected);
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {listed:?}"
        );
        assert_eq!(
            listed.member("tags"),
            Some(&expected_tags),
            "case {case:?}: expected ListTagsForResource to report {expected_tags:?}, got {:?}",
            listed.member("tags")
        );

        // The same single copy of the tag set, through the resource's own reader: a
        // resource with no tags has an EMPTY tag collection there, not a missing one.
        let read_back = emulator.get_analyzer(analyzer).await;
        let summary_tags = read_back
            .member("analyzer")
            .and_then(|analyzer| analyzer.get("tags"));
        assert_eq!(
            summary_tags,
            Some(&expected_tags),
            "case {case:?}: expected GetAnalyzer to report the same tag set {expected_tags:?}, got {read_back:?}"
        );
    }
}

/// One row of the untag table.
struct UntagCase {
    case: &'static str,
    analyzer: &'static str,
    /// The tag set the resource starts from.
    initial: Vec<(String, String)>,
    /// The successive `UntagResource` calls this row makes.
    calls: Vec<Vec<&'static str>>,
    expected: Vec<(String, String)>,
}

/// Untagging removes exactly the keys it names and leaves every other tag in place.
#[tokio::test]
async fn untagging_removes_only_the_named_keys() {
    let cases = vec![
        UntagCase {
            case: "one key of several",
            analyzer: "untag_one_of_several",
            initial: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            calls: vec![vec!["b"]],
            expected: vec![tag("a", "1"), tag("c", "3")],
        },
        UntagCase {
            case: "several keys in one call",
            analyzer: "untag_several",
            initial: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            calls: vec![vec!["a", "c"]],
            expected: vec![tag("b", "2")],
        },
        UntagCase {
            case: "a key the resource does not hold removes nothing",
            analyzer: "untag_absent_key",
            initial: vec![tag("a", "1")],
            calls: vec![vec!["absent"]],
            expected: vec![tag("a", "1")],
        },
        UntagCase {
            case: "the same removal repeated is idempotent",
            analyzer: "untag_repeated",
            initial: vec![tag("a", "1"), tag("b", "2")],
            calls: vec![vec!["a"], vec!["a"]],
            expected: vec![tag("b", "2")],
        },
        UntagCase {
            case: "every key, leaving the set empty",
            analyzer: "untag_everything",
            initial: vec![tag("a", "1"), tag("b", "2")],
            calls: vec![vec!["a", "b"]],
            expected: vec![],
        },
        UntagCase {
            case: "a non-ASCII key",
            analyzer: "untag_unicode_key",
            initial: vec![tag("プロジェクト", "Ωmega"), tag("a", "1")],
            calls: vec![vec!["プロジェクト"]],
            expected: vec![tag("a", "1")],
        },
    ];

    let emulator = Emulator::new();
    for case in cases {
        let UntagCase {
            case,
            analyzer,
            initial,
            calls,
            expected,
        } = case;
        let arn = create_analyzer(&emulator, analyzer, Some(tag_object(&initial))).await;

        for keys in &calls {
            let reply = untag_resource(&emulator, &arn, keys).await;
            assert_eq!(
                reply.status, 204,
                "case {case:?}: expected UntagResource to answer 204 No Content, got {reply:?}"
            );
        }

        let expected_tags = tag_object(&expected);
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.member("tags"),
            Some(&expected_tags),
            "case {case:?}: expected ListTagsForResource to report {expected_tags:?}, got {listed:?}"
        );

        // The tag set a tagging call has touched still EXISTS when it is empty: the
        // resource's own summary reports an empty map, not an absent member.
        let read_back = emulator.get_analyzer(analyzer).await;
        let summary_tags = read_back
            .member("analyzer")
            .and_then(|analyzer| analyzer.get("tags"))
            .filter(|tags| !tags.is_null());
        assert_eq!(
            summary_tags,
            Some(&expected_tags),
            "case {case:?}: expected GetAnalyzer to report {expected_tags:?}, got {summary_tags:?}"
        );
    }
}

/// A resource with no tags at all answers with an empty tag set rather than a refusal,
/// through its own reader as well as through the tag listing.
#[tokio::test]
async fn an_untagged_resource_reports_an_empty_tag_set() {
    let emulator = Emulator::new();
    let arn = create_analyzer(&emulator, "never_tagged", None).await;

    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.status, 200,
        "an untagged resource: expected 200, got {listed:?}"
    );
    assert_eq!(
        listed.member("tags"),
        Some(&json!({})),
        "an untagged resource: expected an empty tag set, got {listed:?}"
    );

    let read_back = emulator.get_analyzer("never_tagged").await;
    assert_eq!(
        read_back
            .member("analyzer")
            .and_then(|analyzer| analyzer.get("tags")),
        Some(&json!({})),
        "a resource that has never carried tags: expected an empty tag set, got {read_back:?}"
    );

    // Once a tag has been added and removed again, the set is empty once more.
    let tagged = tag_resource(&emulator, &arn, &json!({ "a": "1" })).await;
    assert_eq!(tagged.status, 204, "tagging it: got {tagged:?}");
    let untagged = untag_resource(&emulator, &arn, &["a"]).await;
    assert_eq!(untagged.status, 204, "untagging it: got {untagged:?}");
    let read_back = emulator.get_analyzer("never_tagged").await;
    assert_eq!(
        read_back
            .member("analyzer")
            .and_then(|analyzer| analyzer.get("tags")),
        Some(&json!({})),
        "a resource whose last tag was removed: expected an empty tag set, got {read_back:?}"
    );
}

/// One row of the refusal table: an ARN, and what every tagging operation must answer
/// for it.
struct ArnCase {
    case: &'static str,
    resource_arn: String,
    status: u16,
    code: &'static str,
}

/// An ARN that is not an ARN is refused as invalid, and a well-formed ARN no resource
/// has is refused as not found — on each of the three tagging operations, with the
/// tagged resource left untouched.
#[tokio::test]
async fn a_tagging_call_on_an_unknown_or_malformed_arn_is_refused() {
    let emulator = Emulator::new();
    let arn = create_analyzer(&emulator, "bystander", Some(json!({ "env": "dev" }))).await;

    let cases = vec![
        ArnCase {
            case: "a bare resource name is not an ARN",
            resource_arn: "bystander".to_string(),
            status: 400,
            code: "ValidationException",
        },
        ArnCase {
            case: "an ARN missing its resource segment",
            resource_arn: "arn:aws:access-analyzer:us-east-1:000000000000".to_string(),
            status: 400,
            code: "ValidationException",
        },
        ArnCase {
            case: "a well-formed analyzer ARN naming no analyzer",
            resource_arn: expected_arn("no_such_analyzer"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        ArnCase {
            case: "an analyzer ARN in another account",
            resource_arn: "arn:aws:access-analyzer:us-east-1:111122223333:analyzer/bystander"
                .to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        ArnCase {
            case: "another service's resource ARN",
            resource_arn: "arn:aws:s3:::some-bucket".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    for case in cases {
        let ArnCase {
            case,
            resource_arn,
            status,
            code,
        } = case;

        let tagged = tag_resource(&emulator, &resource_arn, &json!({ "k": "v" })).await;
        assert_eq!(
            (tagged.status, tagged.code()),
            (status, code),
            "case {case:?}: expected TagResource to answer {status} {code}, got {tagged:?}"
        );

        let listed = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            (listed.status, listed.code()),
            (status, code),
            "case {case:?}: expected ListTagsForResource to answer {status} {code}, got {listed:?}"
        );

        let untagged = untag_resource(&emulator, &resource_arn, &["k"]).await;
        assert_eq!(
            (untagged.status, untagged.code()),
            (status, code),
            "case {case:?}: expected UntagResource to answer {status} {code}, got {untagged:?}"
        );
    }

    // A refused call writes nothing: the resource that does exist is unchanged.
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.member("tags"),
        Some(&json!({ "env": "dev" })),
        "a refused tagging call must leave other resources untouched, got {listed:?}"
    );
}

/// One row of the tag-restriction table.
struct RestrictionCase {
    case: &'static str,
    tags: Vec<(String, String)>,
    /// `None` when the call must be accepted.
    refusal: Option<&'static str>,
}

/// A tag outside this service's tag restrictions is refused with the modelled
/// `ValidationException`, at each published bound and one past it, and the resource's
/// tag set is left as it was.
#[tokio::test]
async fn a_tag_outside_the_restrictions_is_refused() {
    let cases = vec![
        RestrictionCase {
            case: "a key at its maximum length is accepted",
            tags: vec![tag(&text_of(MAX_TAG_KEY_CHARS), "v")],
            refusal: None,
        },
        RestrictionCase {
            case: "a key one character past its maximum length",
            tags: vec![tag(&text_of(MAX_TAG_KEY_CHARS + 1), "v")],
            refusal: Some("ValidationException"),
        },
        RestrictionCase {
            case: "an empty key",
            tags: vec![tag("", "v")],
            refusal: Some("ValidationException"),
        },
        RestrictionCase {
            case: "a value at its maximum length is accepted",
            tags: vec![tag("k", &text_of(MAX_TAG_VALUE_CHARS))],
            refusal: None,
        },
        RestrictionCase {
            case: "a value one character past its maximum length",
            tags: vec![tag("k", &text_of(MAX_TAG_VALUE_CHARS + 1))],
            refusal: Some("ValidationException"),
        },
        RestrictionCase {
            case: "an empty value is accepted",
            tags: vec![tag("k", "")],
            refusal: None,
        },
        RestrictionCase {
            case: "one bad key refuses the whole call",
            tags: vec![tag("good", "v"), tag("", "v")],
            refusal: Some("ValidationException"),
        },
    ];

    let emulator = Emulator::new();
    for (row, case) in cases.into_iter().enumerate() {
        let RestrictionCase {
            case,
            tags,
            refusal,
        } = case;
        // One fresh resource per row, so a row only ever sees its own call.
        let arn = create_analyzer(&emulator, &format!("restrictions_row{row}"), None).await;

        let reply = tag_resource(&emulator, &arn, &tag_object(&tags)).await;
        match refusal {
            None => assert_eq!(
                reply.status, 204,
                "case {case:?}: expected the call to be accepted with 204, got {reply:?}"
            ),
            Some(code) => {
                assert_eq!(
                    (reply.status, reply.code()),
                    (400, code),
                    "case {case:?}: expected 400 {code}, got {reply:?}"
                );
                let after = list_tags(&emulator, &arn).await;
                assert_eq!(
                    after.member("tags"),
                    Some(&json!({})),
                    "case {case:?}: a refused call must leave the tag set empty, got {after:?}"
                );
            }
        }
    }
}

/// The per-resource tag ceiling holds: the resource takes tags up to it, and the call
/// that would take it past is refused with the tag set unchanged.
#[tokio::test]
async fn the_tag_set_of_a_resource_is_bounded() {
    let emulator = Emulator::new();
    let arn = create_analyzer(&emulator, "bounded_tag_set", None).await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
        .collect();
    let reply = tag_resource(&emulator, &arn, &tag_object(&full)).await;
    assert_eq!(
        reply.status, 204,
        "a resource takes {MAX_TAGS_PER_RESOURCE} tags, got {reply:?}"
    );

    // One more NEW key is past the ceiling.
    let reply = tag_resource(&emulator, &arn, &json!({ "one-too-many": "v" })).await;
    assert_eq!(
        (reply.status, reply.code()),
        (400, "ValidationException"),
        "tag {} of a resource: expected 400 ValidationException, got {reply:?}",
        MAX_TAGS_PER_RESOURCE + 1
    );

    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.member("tags"),
        Some(&tag_object(&full)),
        "a refused call must leave the full tag set as it was, got {listed:?}"
    );

    // A key the resource already holds is an overwrite, not an addition, so it is
    // accepted even with the set full.
    let reply = tag_resource(&emulator, &arn, &json!({ "k0": "rewritten" })).await;
    assert_eq!(
        reply.status, 204,
        "overwriting a held key with the set full: expected 204, got {reply:?}"
    );
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed
            .member("tags")
            .and_then(|tags| tags.get("k0"))
            .and_then(Value::as_str),
        Some("rewritten"),
        "overwriting a held key with the set full must take the new value, got {listed:?}"
    );
    assert_eq!(
        listed
            .member("tags")
            .and_then(Value::as_object)
            .map(serde_json::Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "overwriting a held key must not grow the tag set, got {listed:?}"
    );
}
