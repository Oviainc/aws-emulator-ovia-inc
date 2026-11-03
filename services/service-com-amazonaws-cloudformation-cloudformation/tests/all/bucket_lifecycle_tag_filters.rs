// SPDX-License-Identifier: CC0-1.0

//! The `Filter` a tag-narrowed lifecycle rule of an `AWS::S3::Bucket` is reported with.
//!
//! RULE: in the CloudFormation translation of a bucket's `LifecycleConfiguration`, a rule
//! whose declared scope narrows by OBJECT TAGS travels to Amazon S3 as the `And` conjunction
//! whatever the tag count, and that conjunction states a `Prefix` — the declared one, or the
//! EMPTY prefix for a rule that declares none — so `GetBucketLifecycleConfiguration` reports
//! `Filter.And{Prefix, Tags}` and never the single `Filter.Tag` member for such a rule. A
//! scope that narrows by no tag keeps the mapping the API's own container describes: "a
//! Filter must have exactly one of Prefix, Tag, ObjectSizeGreaterThan, ObjectSizeLessThan, or
//! And specified" (`smithy_query(query="shape",shape="LifecycleRuleFilter")`), so one
//! declared predicate is that predicate's own member, several are the conjunction of them
//! (which states no prefix unless one was declared), and none is the empty filter that
//! "applies to all objects in the bucket".
//!
//! `Filter.Tag` remains the shape the same single tag takes when a configuration is written
//! straight through `PutBucketLifecycleConfiguration` — `service-com-amazonaws-s3-amazons3`'s
//! own `tests/all/bucket_lifecycle_configuration.rs` pins that path — so the two are asserted
//! apart: this suite is about what the stack's own translation produces.
//!
//! The rows differ along what the rule generalises over: the TAG COUNT (one, two, three), the
//! companion predicates a tag-narrowed scope declares beside the tags (none, a prefix, an
//! object-size bound), both `Status` values, and — as the contrast rows that keep the rule
//! from being read too wide — the scopes that narrow by no tag at all (one prefix, two
//! object-size bounds, nothing). Each row's expected filter is spelled out beside the
//! properties it declares; nothing is read back out of the answer under test.
//!
//! The reverse transition the same surface publishes closes the suite: after
//! `DeleteBucketLifecycle` the read is refused with the code and the fixed sentence
//! `GetBucketLifecycleConfiguration`'s own documentation publishes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_text,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The bucket the stack of this suite declares, and the stack's own name.
const BUCKET: &str = "stack-lifecycle-tag-filter-source";
const STACK: &str = "stack-lifecycle-tag-filter";

/// Every `<tag>…</tag>` element's inner text inside `xml`, in document order, with a
/// SELF-CLOSING `<tag/>` reported as the empty text it carries.
///
/// The shared `xml_text` cannot tell `<Prefix></Prefix>` from no `Prefix` element at all once
/// a serializer writes the empty value self-closing, and the EMPTY prefix is exactly what
/// this suite asserts — so presence and content are read here in one pass. A longer element
/// name that merely starts with `tag` is not that element (`<Tagging>` is not a `<Tag>`).
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let head = &after[..open_end];
        let named = head.is_empty() || head.starts_with(char::is_whitespace) || head == "/";
        if !named {
            rest = &rest[start + 1..];
            continue;
        }
        if head.ends_with('/') {
            found.push(String::new());
            rest = &after[open_end + 1..];
            continue;
        }
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else {
            break;
        };
        found.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    found
}

/// The first `<tag>` element's text, or `None` when `xml` carries no such element.
fn element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// `xml` with the first `<tag>…</tag>` element, its own tags included, cut out — so a member
/// of the enclosing container can be told from a member of that element with the same name
/// (a rule's legacy top-level `Prefix` from the `Prefix` inside its `Filter`).
fn without_element(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return xml.to_string();
    };
    match xml[start..].find(&close) {
        Some(end) => format!("{}{}", &xml[..start], &xml[start + end + close.len()..]),
        None => xml[..start].to_string(),
    }
}

/// The object scope one reported rule carries, as the `Filter` block spells it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Scope {
    /// Whether the predicates arrived inside the `And` conjunction. A scope naming one
    /// predicate is the predicate's own member, so this tells the two forms apart.
    conjunction: bool,
    /// The `Prefix` element, `Some("")` for the empty prefix and `None` for no element.
    prefix: Option<String>,
    /// Each `Tag`, as its `(Key, Value)`, in reported order.
    tags: Vec<(String, String)>,
    object_size_greater_than: Option<String>,
    object_size_less_than: Option<String>,
    /// Whether the `Filter` container is there at all: an empty one is the rule that covers
    /// every object, which is not the same as no `Filter`.
    present: bool,
}

/// The scope one `<Rule>` block reports.
fn reported_scope(rule: &str) -> Scope {
    let Some(block) = element(rule, "Filter") else {
        return Scope::default();
    };
    // `And` is the conjunction's own element, so its PRESENCE is what tells a conjunction
    // from the single-predicate members.
    let (inside, conjunction) = match element(&block, "And") {
        Some(conjunction) => (conjunction, true),
        None => (block, false),
    };
    Scope {
        conjunction,
        prefix: element(&inside, "Prefix"),
        tags: elements(&inside, "Tag")
            .iter()
            .map(|tag| {
                (
                    element(tag, "Key").unwrap_or_default(),
                    element(tag, "Value").unwrap_or_default(),
                )
            })
            .collect(),
        object_size_greater_than: element(&inside, "ObjectSizeGreaterThan"),
        object_size_less_than: element(&inside, "ObjectSizeLessThan"),
        present: true,
    }
}

/// A scope a row EXPECTS, in the shape [`reported_scope`] reads one into.
fn scope(
    conjunction: bool,
    prefix: Option<&str>,
    tags: &[(&str, &str)],
    sizes: (Option<i64>, Option<i64>),
) -> Scope {
    Scope {
        conjunction,
        prefix: prefix.map(str::to_string),
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        object_size_greater_than: sizes.0.map(|bytes| bytes.to_string()),
        object_size_less_than: sizes.1.map(|bytes| bytes.to_string()),
        present: true,
    }
}

/// One row: the lifecycle rule a template declares, and the filter Amazon S3 must report it
/// with. `days` is the rule's `ExpirationInDays`, which is what gives every row an action.
struct Row<'a> {
    case: &'a str,
    id: &'a str,
    status: &'a str,
    days: i64,
    /// The scope properties the rule declares, FLAT, as the template spells them.
    declared: Value,
    expected: Scope,
}

/// The table: every row deploys as one rule of one bucket's `LifecycleConfiguration`.
fn rows() -> Vec<Row<'static>> {
    vec![
        Row {
            case: "one tag and nothing else: the conjunction with the EMPTY prefix",
            id: "OneTag",
            status: "Enabled",
            days: 1,
            declared: json!({"TagFilters": [{"Key": "archive", "Value": "yes"}]}),
            expected: scope(true, Some(""), &[("archive", "yes")], (None, None)),
        },
        Row {
            case: "two tags: the same conjunction, both tags in declared order",
            id: "TwoTags",
            status: "Enabled",
            days: 2,
            declared: json!({"TagFilters": [
                {"Key": "tier", "Value": "cold"},
                {"Key": "owner", "Value": "data"}
            ]}),
            expected: scope(
                true,
                Some(""),
                &[("tier", "cold"), ("owner", "data")],
                (None, None),
            ),
        },
        Row {
            case: "three tags, and the other Status value",
            id: "ThreeTagsDisabled",
            status: "Disabled",
            days: 3,
            declared: json!({"TagFilters": [
                {"Key": "tier", "Value": "warm"},
                {"Key": "owner", "Value": "search"},
                {"Key": "retain", "Value": "no"}
            ]}),
            expected: scope(
                true,
                Some(""),
                &[("tier", "warm"), ("owner", "search"), ("retain", "no")],
                (None, None),
            ),
        },
        Row {
            case: "a tag beside a declared prefix: the declared prefix, not the empty one",
            id: "TagAndPrefix",
            status: "Enabled",
            days: 4,
            declared: json!({
                "Prefix": "reports/",
                "TagFilters": [{"Key": "stage", "Value": "done"}]
            }),
            expected: scope(true, Some("reports/"), &[("stage", "done")], (None, None)),
        },
        Row {
            case: "a tag beside an object-size bound: the bound travels in the conjunction",
            id: "TagAndSize",
            status: "Enabled",
            days: 5,
            declared: json!({
                "ObjectSizeGreaterThan": 4096,
                "TagFilters": [{"Key": "large", "Value": "yes"}]
            }),
            expected: scope(true, Some(""), &[("large", "yes")], (Some(4096), None)),
        },
        Row {
            case: "no tag, one prefix: the Prefix member of the filter itself",
            id: "PrefixOnly",
            status: "Enabled",
            days: 6,
            declared: json!({"Prefix": "logs/"}),
            expected: scope(false, Some("logs/"), &[], (None, None)),
        },
        Row {
            case: "no tag, two object-size bounds: a conjunction stating no prefix",
            id: "SizesOnly",
            status: "Enabled",
            days: 7,
            declared: json!({"ObjectSizeGreaterThan": 1024, "ObjectSizeLessThan": 8192}),
            expected: scope(true, None, &[], (Some(1024), Some(8192))),
        },
        Row {
            case: "no scope at all: the empty filter, every object of the bucket",
            id: "NoScope",
            status: "Enabled",
            days: 8,
            declared: json!({}),
            expected: Scope {
                present: true,
                ..Scope::default()
            },
        },
    ]
}

/// The template the rows amount to: one bucket whose `LifecycleConfiguration` declares every
/// row as a rule, publishing the bucket name as a stack output.
fn template(rows: &[Row<'_>]) -> String {
    let rules: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut rule = json!({
                "Id": row.id,
                "Status": row.status,
                "ExpirationInDays": row.days,
            });
            let declared = row
                .declared
                .as_object()
                .expect("a row declares its scope as an object");
            for (property, value) in declared {
                rule[property] = value.clone();
            }
            rule
        })
        .collect();
    json!({
        "Resources": {
            "TaggedBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": BUCKET,
                    "LifecycleConfiguration": {"Rules": rules},
                },
            },
        },
        "Outputs": {"TaggedBucketName": {"Value": {"Ref": "TaggedBucket"}}},
    })
    .to_string()
}

/// `GET /<bucket>?lifecycle` — `GetBucketLifecycleConfiguration`.
async fn get_lifecycle(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?lifecycle"), b"").await;
    (answer.status, answer.body)
}

/// `DELETE /<bucket>?lifecycle` — `DeleteBucketLifecycle`.
async fn delete_lifecycle(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "DELETE", &format!("/{bucket}?lifecycle"), b"").await;
    (answer.status, answer.body)
}

#[tokio::test]
async fn a_tag_narrowed_rule_a_stack_declares_reads_back_as_the_and_conjunction() {
    let rows = rows();
    let router = router();

    let (http_status, created) = create_stack(&router, STACK, &template(&rows), &[]).await;
    assert_eq!(
        http_status, 200,
        "CreateStack must answer 200, got {http_status} {created}"
    );
    wait_for_the_transition().await;
    let (http_status, described) = describe_stack(&router, STACK).await;
    assert_eq!(
        http_status, 200,
        "DescribeStacks must answer 200, got {http_status} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the stack must reach CREATE_COMPLETE, got {described}"
    );
    let name = output_value(&described, "TaggedBucketName");
    assert_eq!(
        name, BUCKET,
        "the stack must publish the declared bucket name, got {name:?} ({described})"
    );

    let (http_status, body) = get_lifecycle(&router, &name).await;
    assert_eq!(
        http_status, 200,
        "s3:GetBucketLifecycleConfiguration({name:?}) must answer 200, got {http_status} {body}"
    );
    // The rules are reported in the order the declaration states them, which is the order the
    // rows are written in, so each row is read at its own position.
    let reported = elements(&body, "Rule");
    assert_eq!(
        reported.len(),
        rows.len(),
        "the bucket must report one rule per declared rule, got {reported:?} ({body})"
    );
    for (row, rule) in rows.iter().zip(reported.iter()) {
        assert_eq!(
            element(rule, "ID").as_deref(),
            Some(row.id),
            "case {:?}: the rule must keep its declared ID, got {rule}",
            row.case
        );
        assert_eq!(
            element(rule, "Status").as_deref(),
            Some(row.status),
            "case {:?}: the rule must keep its declared Status, got {rule}",
            row.case
        );
        let expiration = element(rule, "Expiration").unwrap_or_default();
        assert_eq!(
            element(&expiration, "Days"),
            Some(row.days.to_string()),
            "case {:?}: Expiration.Days must be the declared ExpirationInDays {}, got {rule}",
            row.case,
            row.days
        );
        // The legacy top-level `Prefix` is the OTHER version of the API: a rule carrying a
        // `Filter` carries none of it, and the `Prefix` inside the filter is not it.
        let outside_the_filter = without_element(rule, "Filter");
        assert_eq!(
            element(&outside_the_filter, "Prefix"),
            None,
            "case {:?}: the rule must carry no top-level Prefix member, got {rule}",
            row.case
        );
        let scope = reported_scope(rule);
        assert_eq!(
            scope, row.expected,
            "case {:?}: the rule {:?} must report {:?}, got {scope:?} ({rule})",
            row.case, row.id, row.expected
        );
    }

    // The reverse transition: the configuration is gone, and the read says so with the code
    // and the fixed sentence the operation publishes for a bucket carrying none.
    let (http_status, deleted) = delete_lifecycle(&router, &name).await;
    assert_eq!(
        http_status, 204,
        "s3:DeleteBucketLifecycle({name:?}) must answer 204, got {http_status} {deleted}"
    );
    let (http_status, body) = get_lifecycle(&router, &name).await;
    assert_eq!(
        http_status, 404,
        "s3:GetBucketLifecycleConfiguration({name:?}) after the delete must answer 404, got \
         {http_status} {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("NoSuchLifecycleConfiguration"),
        "the refused read must carry the published code, got {body}"
    );
    assert_eq!(
        element(&body, "Message").as_deref(),
        Some("The lifecycle configuration does not exist"),
        "the refused read must carry the published sentence, got {body}"
    );
}
