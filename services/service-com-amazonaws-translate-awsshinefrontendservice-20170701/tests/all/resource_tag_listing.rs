// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`: that one call reports
//! exactly the tag set the resource the ARN names is holding at that moment,
//! whatever the tagging calls that left it that way, and that an ARN this
//! account holds no resource for is a not-found refusal rather than an empty
//! tag list.
//!
//! Every test drives the service the way the router does, through the harness
//! `terminology_lifecycle` builds (`router`, `call`, `error_code`, `message`,
//! `import`, `expected_arn`), so the wire protocol, the boundary constraint
//! validation and the handlers are all exercised, and what one request writes is
//! what the next request reads.
//!
//! The tests assert the RULE over a set of inputs rather than one recorded
//! request: the number of tags a resource holds (none, one, several, as many as
//! it may hold), the shape of a key and a value (empty value, both published
//! length bounds, non-ASCII, keys differing only in case), and the sequence of
//! calls that left the resource holding them (one call, several calls, a key
//! restated, keys removed again).
//!
//! The rows compare the reported list element by element ON PURPOSE: this
//! service reports a resource's tags in one stable order — the order the record
//! holds them in, which `TagResource` appends to and `UntagResource` only
//! removes from — and that is the order the removal suite pins too, so one
//! report of one stored tag set cannot drift from the other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::terminology_lifecycle::{
    call, error_code, expected_arn, import, message, router, EN_FR_CSV,
};

/// `Tag$Key` is `length 1..128` and `Tag$Value` is `length 0..256`
/// (`smithy_query shape=Tag`), so these are the longest key and value a request
/// may carry, and one character more is refused by the shape walk.
const MAX_TAG_KEY_CHARS: usize = 128;
const MAX_TAG_VALUE_CHARS: usize = 256;

/// "There can be a maximum of 50 tags (both existing and pending) associated
/// with a specific resource" (`smithy_query shape=TagResourceRequest
/// member=Tags`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One row of the table: the tagging calls that leave the resource holding a
/// tag set, and the tag set `ListTagsForResource` must then report.
struct Row {
    case: &'static str,
    /// Successive `TagResource` tag lists, applied in order.
    tag_calls: Vec<Vec<(String, String)>>,
    /// Keys removed with `UntagResource` after those calls.
    remove: Vec<String>,
    /// What the resource must report, in the order it holds its tags.
    expected: Vec<(String, String)>,
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The `TagList` wire shape of a set of key-value pairs.
fn tag_list(entries: &[(String, String)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The tags one `ListTagsForResource` call reports, asserted to be a 200 that
/// carries the `Tags` member.
async fn listed_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<Value> {
    let (status, body) = call(router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {status} {body}"
    );
    let tags = body.pointer("/Tags");
    assert!(
        tags.and_then(Value::as_array).is_some(),
        "case {case:?}: expected a Tags list, got {body}"
    );
    tags.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn rows() -> Vec<Row> {
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let at_the_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:02}"), format!("v{index}")))
        .collect();

    vec![
        Row {
            case: "a resource no tagging call ever touched",
            tag_calls: Vec::new(),
            remove: Vec::new(),
            expected: Vec::new(),
        },
        Row {
            case: "a call carrying no tag at all",
            tag_calls: vec![Vec::new()],
            remove: Vec::new(),
            expected: Vec::new(),
        },
        Row {
            case: "one tag",
            tag_calls: vec![pairs(&[("env", "prod")])],
            remove: Vec::new(),
            expected: pairs(&[("env", "prod")]),
        },
        Row {
            case: "several tags in one call",
            tag_calls: vec![pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")])],
            remove: Vec::new(),
            expected: pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")]),
        },
        Row {
            case: "two calls adding disjoint keys",
            tag_calls: vec![pairs(&[("env", "prod")]), pairs(&[("owner", "platform")])],
            remove: Vec::new(),
            expected: pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "a key a later call restates",
            tag_calls: vec![
                pairs(&[("env", "staging"), ("owner", "platform")]),
                pairs(&[("env", "prod")]),
            ],
            remove: Vec::new(),
            expected: pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        Row {
            case: "keys differing only in case are distinct tags",
            tag_calls: vec![pairs(&[("Env", "prod"), ("env", "staging")])],
            remove: Vec::new(),
            expected: pairs(&[("Env", "prod"), ("env", "staging")]),
        },
        Row {
            case: "a tag whose value is empty",
            tag_calls: vec![pairs(&[("env", ""), ("owner", "platform")])],
            remove: Vec::new(),
            expected: pairs(&[("env", ""), ("owner", "platform")]),
        },
        Row {
            case: "a key and a value at their published length bounds",
            tag_calls: vec![vec![(long_key.clone(), long_value.clone())]],
            remove: Vec::new(),
            expected: vec![(long_key, long_value)],
        },
        Row {
            case: "a non-ASCII key and value",
            tag_calls: vec![pairs(&[("équipe", "traduction"), ("チーム", "翻訳")])],
            remove: Vec::new(),
            expected: pairs(&[("équipe", "traduction"), ("チーム", "翻訳")]),
        },
        Row {
            case: "every tag removed again",
            tag_calls: vec![pairs(&[("env", "prod"), ("owner", "platform")])],
            remove: vec!["env".to_string(), "owner".to_string()],
            expected: Vec::new(),
        },
        Row {
            case: "some tags removed again",
            tag_calls: vec![pairs(&[("env", "prod"), ("owner", "platform"), ("cost", "42")])],
            remove: vec!["owner".to_string()],
            expected: pairs(&[("env", "prod"), ("cost", "42")]),
        },
        Row {
            case: "as many tags as one resource may hold",
            tag_calls: vec![at_the_limit.clone()],
            remove: Vec::new(),
            expected: at_the_limit,
        },
    ]
}

/// One call reports exactly the tags the resource is holding when it is made,
/// whatever the tagging and untagging calls that left it holding them — and a
/// resource holding none reports an empty list rather than no member at all.
/// Reading twice reports the same thing, because a report changes nothing.
#[tokio::test]
async fn list_tags_for_resource_reports_the_stored_tag_set() {
    let router = router();

    for (index, row) in rows().into_iter().enumerate() {
        let name = format!("listed-{index:02}-terms");
        let (status, imported) = import(&router, &name, EN_FR_CSV, "CSV", Some("UNI")).await;
        assert_eq!(
            status, 200,
            "case {:?}: ImportTerminology answered {status} {imported}",
            row.case
        );
        let arn = expected_arn(&name, EN_FR_CSV);

        for tags in &row.tag_calls {
            let (status, answered) = call(
                &router,
                "TagResource",
                &json!({"ResourceArn": arn, "Tags": tag_list(tags)}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: TagResource answered {status} {answered}",
                row.case
            );
        }
        if !row.remove.is_empty() {
            let (status, answered) = call(
                &router,
                "UntagResource",
                &json!({"ResourceArn": arn, "TagKeys": Value::Array(
                    row.remove.iter().map(|key| json!(key)).collect()
                )}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: UntagResource answered {status} {answered}",
                row.case
            );
        }

        let expected: Vec<Value> = row
            .expected
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let listed = listed_tags(&router, row.case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {:?}: expected the reported tags {expected:?}, got {listed:?}",
            row.case
        );

        // A report is a read: the second call answers exactly as the first did.
        let again = listed_tags(&router, row.case, &arn).await;
        assert_eq!(
            again, expected,
            "case {:?}: a repeated report must answer {expected:?}, got {again:?}",
            row.case
        );
    }
}

/// A report addresses one resource: the ARN it names, never another resource of
/// the same account, and it leaves the resource it reports on exactly as it was.
#[tokio::test]
async fn list_tags_for_resource_reports_only_the_resource_its_arn_names() {
    let router = router();

    let mut arns = Vec::new();
    for (name, tags) in [
        ("first-listed-terms", pairs(&[("env", "prod")])),
        (
            "second-listed-terms",
            pairs(&[("env", "dev"), ("owner", "platform")]),
        ),
        ("third-listed-terms", Vec::new()),
    ] {
        let (status, imported) = import(&router, name, EN_FR_CSV, "CSV", Some("UNI")).await;
        assert_eq!(status, 200, "{name}: ImportTerminology answered {imported}");
        let arn = expected_arn(name, EN_FR_CSV);
        if !tags.is_empty() {
            let (status, answered) = call(
                &router,
                "TagResource",
                &json!({"ResourceArn": arn, "Tags": tag_list(&tags)}),
            )
            .await;
            assert_eq!(status, 200, "{name}: TagResource answered {answered}");
        }
        arns.push((name, arn, tags));
    }

    for (name, arn, tags) in &arns {
        let expected: Vec<Value> = tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let listed = listed_tags(&router, name, arn).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // The resource itself is untouched by having been reported on: a read of
    // its own content answers as it did before.
    let (status, fetched) = call(
        &router,
        "GetTerminology",
        &json!({"Name": "first-listed-terms"}),
    )
    .await;
    assert_eq!(status, 200, "GetTerminology answered {fetched}");
    assert_eq!(
        fetched
            .pointer("/TerminologyProperties/TermCount")
            .and_then(Value::as_i64),
        Some(2),
        "reporting a resource's tags must not change the resource, got {fetched}"
    );
}

/// A tag a request could not carry is a tag no resource ever reports: a key one
/// character past its published bound is refused by the request shape, and the
/// resource keeps the tags it had.
#[tokio::test]
async fn list_tags_for_resource_never_reports_a_tag_the_shape_refused() {
    let router = router();
    let name = "bounded-listed-terms";
    let (status, imported) = import(&router, name, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let arn = expected_arn(name, EN_FR_CSV);

    let held = pairs(&[("env", "prod")]);
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tag_list(&held)}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {answered}");

    // Each row names the member the shape walk refuses and the bound it failed,
    // so the refusal's identity is pinned and not just its status class.
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    let rows: [(&str, &str, &str, &str, &str); 3] = [
        (
            "a key one character past its bound",
            long_key.as_str(),
            "v",
            "tags.1.member.key",
            "Member must have length less than or equal to 128",
        ),
        (
            "a value one character past its bound",
            "over",
            long_value.as_str(),
            "tags.1.member.value",
            "Member must have length less than or equal to 256",
        ),
        (
            "an empty key",
            "",
            "v",
            "tags.1.member.key",
            "Member must have length greater than or equal to 1",
        ),
    ];
    for (case, key, value, member, constraint) in rows {
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": [{"Key": key, "Value": value}]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a constraint refusal, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered}"
        );
        // The clauses are pinned, not the echo: a value longer than the sentence
        // may quote is elided by the shared validation rendering, so what the
        // message must carry is the sentence's fixed text, the member path and
        // the bound that failed, around an echo that starts with what was sent.
        let offending = if member.ends_with("key") { key } else { value };
        let reported = message(&answered);
        let suffix = format!("' at '{member}' failed to satisfy constraint: {constraint}");
        assert!(
            reported.starts_with("1 validation error detected: Value '")
                && reported.ends_with(&suffix),
            "case {case:?}: expected a validation sentence ending {suffix:?}, got {reported:?}"
        );
        let echoed: String = offending.chars().take(32).collect();
        assert!(
            reported.contains(&echoed),
            "case {case:?}: the sentence must quote the value that was sent, got {reported:?}"
        );
    }

    let listed = listed_tags(&router, "after the refusals", &arn).await;
    assert_eq!(
        listed,
        vec![json!({"Key": "env", "Value": "prod"})],
        "a refused tagging call changes nothing the report shows, got {listed:?}"
    );
}

/// An ARN this account holds no resource for is a not-found refusal, never an
/// empty tag list — including the ARN of a resource that was deleted and the
/// superseded ARN of one that was re-imported with different content.
#[tokio::test]
async fn list_tags_for_resource_refuses_an_arn_naming_no_resource() {
    let router = router();

    // A tagged resource exists, so each refusal is about the ARN rather than
    // about an empty store.
    let (status, imported) = import(&router, "present-listed-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let present = expected_arn("present-listed-terms", EN_FR_CSV);
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": present, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {answered}");

    // A resource imported, tagged and then deleted: its ARN names nothing.
    let deleted_name = "deleted-listed-terms";
    let (status, imported) = import(&router, deleted_name, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let deleted = expected_arn(deleted_name, EN_FR_CSV);
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": deleted, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {answered}");
    let (status, answered) = call(
        &router,
        "DeleteTerminology",
        &json!({"Name": deleted_name}),
    )
    .await;
    assert_eq!(status, 200, "DeleteTerminology answered {status} {answered}");

    // A resource re-imported with different content: the ARN the first import
    // minted is superseded.
    let superseded_name = "superseded-listed-terms";
    let (status, imported) = import(&router, superseded_name, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let superseded = expected_arn(superseded_name, EN_FR_CSV);
    let other_file: &[u8] = b"en,fr\nhello,salut\n";
    let (status, imported) =
        import(&router, superseded_name, other_file, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "the re-import answered {imported}");

    let cases = [
        ("an ARN of a name no import ever used", expected_arn("no-such-terms", b"no-such-file")),
        ("the ARN of a deleted resource", deleted),
        ("the superseded ARN of a re-imported resource", superseded),
        ("an ARN of another service", "arn:aws:s3:::a-bucket".to_string()),
        ("a string that is not an ARN at all", "not-an-arn".to_string()),
    ];
    for (case, arn) in cases {
        let (status, answered) =
            call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
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

    // Every refusal left the resource that is there reporting what it holds.
    let listed = listed_tags(&router, "the resource that is there", &present).await;
    assert_eq!(
        listed,
        vec![json!({"Key": "env", "Value": "prod"})],
        "a refused report changes no resource's tags, got {listed:?}"
    );
}
