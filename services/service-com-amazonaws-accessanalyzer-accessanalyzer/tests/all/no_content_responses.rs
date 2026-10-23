// SPDX-License-Identifier: CC0-1.0

//! Which success STATUS a call of this service answers with, driven over the wire.
//!
//! The rule under test: a successful call whose modelled output is a DECLARED structure
//! with no members has no content to report, so it is answered with 204 No Content —
//! this service's model leaves every success code defaulted (`@http` carries no
//! `code:`), so the Smithy default of 200 is not a statement about it. The only such
//! operations in this model are the two tag mutations (`TagResource`,
//! `UntagResource`; `CancelPolicyGeneration`'s success path is still a 501 stub, so it
//! has no wire row here). The boundary is pinned too: a call whose output carries
//! members, and a call whose output is `smithy.api#Unit` (no response structure at all),
//! both keep 200.
//!
//! The tables span what the rule generalises over — which mutation, how many tags, the
//! key and value form (ASCII, Unicode, empty value, each published length bound), how
//! many calls the set was built up over, and whether the call changes anything at all.
//! The recorded `Owner`/`team-a` single-tag case is one row. A 204 is never an excuse
//! for doing nothing, so every row reads the tag set back through
//! `ListTagsForResource`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::harness::{expected_arn, Emulator, Reply};

/// The status a success with no content carries.
const NO_CONTENT: u16 = 204;

/// The status a success that reports content carries.
const OK: u16 = 200;

/// A `(key, value)` pair from borrowed literals, for the tables.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// A key or value of exactly `chars` characters, so a published bound can be tested AT
/// it.
fn text_of(chars: usize) -> String {
    "k".repeat(chars)
}

/// The tag set as the JSON object a request carries and a reader reports.
fn tag_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// Create an `ACCOUNT` analyzer carrying `tags`, and answer the ARN it must have.
async fn create_analyzer(emulator: &Emulator, name: &str, tags: &[(String, String)]) -> String {
    let mut body = json!({ "analyzerName": name, "type": "ACCOUNT" });
    if !tags.is_empty() {
        body["tags"] = tag_object(tags);
    }
    let reply = emulator.create_analyzer(body).await;
    assert_eq!(
        reply.status, OK,
        "creating analyzer {name}: expected {OK}, got {reply:?}"
    );
    expected_arn(name)
}

/// Assert that `reply` is a success that reports no content.
fn assert_no_content(case: &str, operation: &str, reply: &Reply) {
    assert_eq!(
        reply.status, NO_CONTENT,
        "case {case:?}: expected {operation} to answer {NO_CONTENT} No Content, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        "",
        "case {case:?}: expected {operation} to answer no error, got {reply:?}"
    );
    assert!(
        reply.carries_no_content(),
        "case {case:?}: expected {operation} to report no content, got {reply:?}"
    );
}

/// One row of the tag-mutation table.
struct TagCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    analyzer: &'static str,
    /// The tag set the resource starts from.
    initial: Vec<(String, String)>,
    /// The successive `TagResource` calls this row makes.
    calls: Vec<Vec<(String, String)>>,
    /// The tag set every reader must report once the calls are done.
    expected: Vec<(String, String)>,
}

/// Every successful `TagResource` answers 204 with no content, whatever the request
/// carried — and the tag set it left behind is the one a reader reports.
#[tokio::test]
async fn a_successful_tag_answers_204_with_no_content() {
    let cases = vec![
        TagCase {
            case: "one tag onto an untagged resource (the recorded case)",
            analyzer: "nc_recorded_tag",
            initial: vec![],
            calls: vec![vec![tag("Owner", "team-a")]],
            expected: vec![tag("Owner", "team-a")],
        },
        TagCase {
            case: "no tags at all: a call that changes nothing still has no content",
            analyzer: "nc_empty_map",
            initial: vec![tag("env", "dev")],
            calls: vec![vec![]],
            expected: vec![tag("env", "dev")],
        },
        TagCase {
            case: "a key the resource already holds takes the new value",
            analyzer: "nc_overwrite",
            initial: vec![tag("env", "dev")],
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![tag("env", "prod")],
        },
        TagCase {
            case: "eight tags in one call",
            analyzer: "nc_many_at_once",
            initial: vec![],
            calls: vec![(0..8)
                .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
                .collect()],
            expected: (0..8)
                .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
                .collect(),
        },
        TagCase {
            case: "successive calls, each answered the same way",
            analyzer: "nc_accumulated",
            initial: vec![],
            calls: vec![vec![tag("a", "1")], vec![tag("b", "2")]],
            expected: vec![tag("a", "1"), tag("b", "2")],
        },
        TagCase {
            case: "the same call repeated",
            analyzer: "nc_repeated",
            initial: vec![],
            calls: vec![vec![tag("a", "1")], vec![tag("a", "1")]],
            expected: vec![tag("a", "1")],
        },
        TagCase {
            case: "an empty value",
            analyzer: "nc_empty_value",
            initial: vec![],
            calls: vec![vec![tag("Owner", "")]],
            expected: vec![tag("Owner", "")],
        },
        TagCase {
            case: "a non-ASCII key and value",
            analyzer: "nc_unicode",
            initial: vec![],
            calls: vec![vec![tag("プロジェクト", "Ωmega")]],
            expected: vec![tag("プロジェクト", "Ωmega")],
        },
        TagCase {
            // The bounds come from this service's tag restrictions, held as
            // `state::MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS`.
            case: "a key and a value at their longest accepted length",
            analyzer: "nc_at_length_bounds",
            initial: vec![],
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
            case: "the resource taken to the per-resource maximum",
            analyzer: "nc_at_count_bound",
            initial: vec![],
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
            initial,
            calls,
            expected,
        } = case;
        let arn = create_analyzer(&emulator, analyzer, &initial).await;

        for tags in &calls {
            let reply = emulator.tag_resource(&arn, &tag_object(tags)).await;
            assert_no_content(case, "TagResource", &reply);
        }

        // A response with no content still reports what happened through the resource:
        // the content-bearing reader of the same tag set answers 200 WITH the set.
        let expected_tags = tag_object(&expected);
        let listed = emulator.list_tags_for_resource(&arn).await;
        assert_eq!(
            listed.status, OK,
            "case {case:?}: expected ListTagsForResource to answer {OK}, got {listed:?}"
        );
        assert_eq!(
            listed.member("tags"),
            Some(&expected_tags),
            "case {case:?}: expected the tag set {expected_tags:?} to be readable back, got {listed:?}"
        );
    }
}

/// One row of the untag-mutation table.
struct UntagCase {
    case: &'static str,
    analyzer: &'static str,
    initial: Vec<(String, String)>,
    /// The successive `UntagResource` calls this row makes.
    calls: Vec<Vec<String>>,
    expected: Vec<(String, String)>,
}

/// The keys of a call, from borrowed literals.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// Every successful `UntagResource` answers 204 with no content, whatever keys it named
/// — including keys the resource does not hold, where it removes nothing.
#[tokio::test]
async fn a_successful_untag_answers_204_with_no_content() {
    let cases = vec![
        UntagCase {
            case: "one key of several (the recorded case)",
            analyzer: "nc_untag_one",
            initial: vec![tag("Owner", "team-a"), tag("env", "dev")],
            calls: vec![keys(&["Owner"])],
            expected: vec![tag("env", "dev")],
        },
        UntagCase {
            case: "several keys in one call",
            analyzer: "nc_untag_several",
            initial: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            calls: vec![keys(&["a", "c"])],
            expected: vec![tag("b", "2")],
        },
        UntagCase {
            case: "a key the resource does not hold removes nothing",
            analyzer: "nc_untag_absent",
            initial: vec![tag("a", "1")],
            calls: vec![keys(&["absent"])],
            expected: vec![tag("a", "1")],
        },
        UntagCase {
            case: "the same removal repeated",
            analyzer: "nc_untag_repeated",
            initial: vec![tag("a", "1"), tag("b", "2")],
            calls: vec![keys(&["a"]), keys(&["a"])],
            expected: vec![tag("b", "2")],
        },
        UntagCase {
            case: "every key, leaving the set empty",
            analyzer: "nc_untag_everything",
            initial: vec![tag("a", "1"), tag("b", "2")],
            calls: vec![keys(&["a", "b"])],
            expected: vec![],
        },
        UntagCase {
            case: "a non-ASCII key",
            analyzer: "nc_untag_unicode",
            initial: vec![tag("プロジェクト", "Ωmega"), tag("a", "1")],
            calls: vec![keys(&["プロジェクト"])],
            expected: vec![tag("a", "1")],
        },
        UntagCase {
            // `MAX_TAGS_PER_RESOURCE` keys named in one call: the largest removal the
            // per-resource ceiling allows.
            case: "every key of a resource holding the per-resource maximum",
            analyzer: "nc_untag_at_count_bound",
            initial: (0..MAX_TAGS_PER_RESOURCE)
                .map(|n| tag(&format!("k{n}"), &format!("v{n}")))
                .collect(),
            calls: vec![(0..MAX_TAGS_PER_RESOURCE).map(|n| format!("k{n}")).collect()],
            expected: vec![],
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
        let arn = create_analyzer(&emulator, analyzer, &initial).await;

        for call in &calls {
            let named: Vec<&str> = call.iter().map(String::as_str).collect();
            let reply = emulator.untag_resource(&arn, &named).await;
            assert_no_content(case, "UntagResource", &reply);
        }

        let expected_tags = tag_object(&expected);
        let listed = emulator.list_tags_for_resource(&arn).await;
        assert_eq!(
            listed.status, OK,
            "case {case:?}: expected ListTagsForResource to answer {OK}, got {listed:?}"
        );
        assert_eq!(
            listed.member("tags"),
            Some(&expected_tags),
            "case {case:?}: expected the tag set {expected_tags:?} to be readable back, got {listed:?}"
        );
    }
}

/// 204 is a success status: a refused tag mutation keeps the refusal's own status, over
/// the whole class of refusals either operation raises.
#[tokio::test]
async fn a_refused_tag_mutation_keeps_its_modelled_status() {
    let emulator = Emulator::new();
    let arn = create_analyzer(&emulator, "nc_refusals", &[]).await;

    // (case, resourceArn, status, error code) — the refusals the two mutations answer.
    // The 404s carry the tagging path's own `NotFoundException` code; see
    // `tests/all/resource_tags.rs` for the rule.
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a bare resource name is not an ARN",
            "nc_refusals".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a well-formed analyzer ARN naming no analyzer",
            expected_arn("nc_no_such_analyzer"),
            404,
            "NotFoundException",
        ),
        (
            "an analyzer ARN in another account",
            "arn:aws:access-analyzer:us-east-1:111122223333:analyzer/nc_refusals".to_string(),
            404,
            "NotFoundException",
        ),
    ];

    for (case, resource_arn, status, code) in cases {
        let tagged = emulator.tag_resource(&resource_arn, &json!({ "k": "v" })).await;
        assert_eq!(
            (tagged.status, tagged.code()),
            (status, code),
            "case {case:?}: expected TagResource to answer {status} {code}, got {tagged:?}"
        );

        let untagged = emulator.untag_resource(&resource_arn, &["k"]).await;
        assert_eq!(
            (untagged.status, untagged.code()),
            (status, code),
            "case {case:?}: expected UntagResource to answer {status} {code}, got {untagged:?}"
        );
    }

    // A tag outside this service's restrictions is refused the same way, not answered
    // with the no-content success.
    let refused = emulator
        .tag_resource(&arn, &json!({ "": "no key" }))
        .await;
    assert_eq!(
        (refused.status, refused.code()),
        (400, "ValidationException"),
        "an empty tag key: expected 400 ValidationException, got {refused:?}"
    );
}

/// The boundary of the rule: a success that DOES have content keeps 200, and so does a
/// success whose operation models no response structure at all (`smithy.api#Unit`),
/// which the recordings show AWS answering with the defaulted 200.
#[tokio::test]
async fn a_call_outside_the_no_content_class_still_answers_200() {
    let emulator = Emulator::new();
    let arn = create_analyzer(&emulator, "nc_boundary", &[tag("env", "dev")]).await;
    let filter = json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}});

    // Operations whose output carries members: 200, WITH content.
    let content_bearing: Vec<(&str, Reply)> = vec![
        ("GetAnalyzer", emulator.get_analyzer("nc_boundary").await),
        ("ListAnalyzers", emulator.list_analyzers("").await),
        (
            "ListTagsForResource",
            emulator.list_tags_for_resource(&arn).await,
        ),
    ];
    for (operation, reply) in content_bearing {
        assert_eq!(
            reply.status, OK,
            "case {operation:?}: an output with members is answered {OK}, got {reply:?}"
        );
        assert!(
            !reply.carries_no_content(),
            "case {operation:?}: expected content in the response, got {reply:?}"
        );
    }

    // Operations modelled with a `smithy.api#Unit` output — no response structure at
    // all: 200 with an empty body, which is what the recordings show for them.
    let unit_output: Vec<(&str, Reply)> = vec![
        (
            "CreateArchiveRule",
            emulator
                .create_archive_rule(
                    "nc_boundary",
                    json!({"ruleName": "nc_rule", "filter": filter.clone()}),
                )
                .await,
        ),
        (
            "UpdateArchiveRule",
            emulator
                .update_archive_rule("nc_boundary", "nc_rule", json!({"filter": filter}))
                .await,
        ),
        (
            "DeleteArchiveRule",
            emulator.delete_archive_rule("nc_boundary", "nc_rule").await,
        ),
        (
            "DeleteAnalyzer",
            emulator.delete_analyzer("nc_boundary").await,
        ),
    ];
    for (operation, reply) in unit_output {
        assert_eq!(
            reply.status, OK,
            "case {operation:?}: a Unit output keeps the defaulted {OK}, got {reply:?}"
        );
        assert_eq!(
            reply.body_bytes, 0,
            "case {operation:?}: a Unit output carries no body, got {reply:?}"
        );
    }
}
