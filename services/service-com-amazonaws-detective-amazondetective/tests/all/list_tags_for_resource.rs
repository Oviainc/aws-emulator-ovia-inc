// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource` — the service's tag READER.
//!
//! The rules under test, each asserted over a SET of inputs rather than over the one
//! graph and the one `Env`/`Team` pair a recording happened to carry:
//!
//! 1. The call REPORTS the tag map the named behavior graph holds, exactly as the
//!    writers left it: every key and value stored, no key that is not stored, and no
//!    value altered — whichever writer stored it (`CreateGraph`'s seed, `TagResource`'s
//!    merge, what `UntagResource` left behind), whatever the map's size from none up to
//!    the per-resource tag quota, and whatever character form its keys and values take.
//!    An untagged graph reports an empty map, not a missing one.
//! 2. It is a pure report (`GET /tags/{ResourceArn}`, modelled success 200): reading
//!    the tags, however many times, changes nothing a later caller sees — not the map
//!    itself, and not the graph record `ListGraphs` reports.
//! 3. Each graph reports its OWN map: a read is keyed by the ARN it names and no other
//!    graph's tags leak into it.
//! 4. It resolves the graph first, so a graph the service does not hold is the modelled
//!    `ResourceNotFoundException` (404) carrying this service's fixed message, with no
//!    tag data in the answer.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    GRAPH_NOT_FOUND_MESSAGE, MAX_TAGS_PER_GRAPH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, call_with, create_graph_ok, emulator, error_code, error_message, graph_arn_of, list_tags,
    pairs, tag_map, tags_of_case, ACCOUNT, REGION,
};

/// The modelled success status of this operation
/// (`smithy_query(query="operation",operation="ListTagsForResource")`: `GET
/// /tags/{ResourceArn}`, `code` 200).
const LIST_TAGS_STATUS: u16 = 200;

/// The longest value the modelled `TagValue` allows (length 0..256,
/// `smithy_query(query="shape",shape="TagValue")`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// The longest key the modelled `TagKey` allows (length 1..128,
/// `smithy_query(query="shape",shape="TagKey")`).
const MAX_TAG_KEY_LEN: usize = 128;

/// Key forms drawn from the modelled `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// (`smithy_query(query="shape",shape="TagKey")`), whose `+-=` range spans
/// `+ , - . / 0-9 : ; < =` — so a key may carry punctuation and digits as well as
/// letters. None of these is the `Env`/`Team` pair the recordings used, and each goes in
/// the request BODY, so every form reaches the store verbatim.
const TAG_KEY_FORMS: &[&str] = &[
    "Letters",
    "cost.center",
    "team:security",
    "path/segment",
    "with-dash",
    "under_score",
    "digits123",
    "semi;colon",
    "comma,separated",
    "eq=sign",
    "plus+sign",
    "lt<sign",
];

/// The map a call that must succeed reported. The shared harness's `tags_of_case` is
/// the reader (it asserts the modelled [`LIST_TAGS_STATUS`] and that the `Tags` member
/// is present, so an untagged graph has to report an EMPTY map rather than none); the
/// raw `list_tags` beside it is what the refusal rows read, since those have no map.
async fn reported_tags(
    router: &MultiServiceRouter,
    region: &str,
    graph_arn: &str,
    case: &str,
) -> Map<String, Value> {
    tags_of_case(router, region, graph_arn, case).await
}

/// Everything `ListGraphs` reports about the stored graphs — the record a read must not
/// disturb (it carries each graph's ARN and its `CreatedTime`).
async fn graph_records(router: &MultiServiceRouter) -> Value {
    let (status, page) = call(router, "/graphs/list", &json!({})).await;
    assert_eq!(status, 200, "ListGraphs: expected 200, got {status} {page:?}");
    page
}

/// A `Tags` request member from owned pairs.
fn tags_member(entries: &[(String, String)]) -> Value {
    Value::Object(tag_map(entries))
}

/// `TagResource` with `tags` on `graph_arn`; must answer the modelled 204.
async fn tag_ok(
    router: &MultiServiceRouter,
    graph_arn: &str,
    tags: &[(String, String)],
    case: &str,
) {
    let (status, body) = call(
        router,
        &format!("/tags/{graph_arn}"),
        &json!({ "Tags": tags_member(tags) }),
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: TagResource expected 204, got {status} {body:?}"
    );
}

/// `UntagResource` naming `tag_keys` on `graph_arn`; must answer the modelled 204.
/// `TagKeys` is bound to the repeated `tagKeys` query parameter, so the keys this
/// helper is given must be query-safe.
async fn untag_ok(
    router: &MultiServiceRouter,
    graph_arn: &str,
    tag_keys: &[String],
    case: &str,
) {
    let query = tag_keys
        .iter()
        .map(|key| {
            // The same guard the writer suite states (`tests/all/tag_and_untag_resource.rs`,
            // `TAG_KEY_FORMS`): a raw query carries every `TagKey` character except `+`
            // (the query reader turns it into a space) and `<` (not a legal URI
            // character), both of which a caller has to percent-encode. `=` does
            // survive, because that reader splits a pair on the FIRST `=` only.
            assert!(
                !key.contains(['&', '+', '%', '<']),
                "case {case:?}: tag key {key:?} needs percent-encoding before a raw query"
            );
            format!("tagKeys={key}")
        })
        .collect::<Vec<_>>()
        .join("&");
    let (status, body) = call_with(
        router,
        "DELETE",
        REGION,
        &format!("/tags/{graph_arn}?{query}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: UntagResource expected 204, got {status} {body:?}"
    );
}

/// One row of the reporting table: how the graph's map came to be, and what the read
/// must then report.
struct ReportCase {
    name: String,
    /// The `CreateGraph` body that seeds the graph.
    create: Value,
    /// A `TagResource` call applied after the create, if the row makes one.
    then_tag: Vec<(String, String)>,
    /// An `UntagResource` call applied after that, if the row makes one.
    then_untag: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The table. Rows differ along what the rule generalises over: WHICH writer stored the
/// map (create seed, later merge, what a removal left), the map's SIZE (none, one, a
/// few, the per-resource quota), and the FORM of its keys and values (punctuation and
/// digits, the modelled length bounds, an empty value).
fn report_cases() -> Vec<ReportCase> {
    let quota: Vec<(String, String)> = (0..MAX_TAGS_PER_GRAPH)
        .map(|n| (format!("Key{n:02}"), format!("value-{n}")))
        .collect();

    // One tag per modelled key form, so a single read reports every form at once.
    let forms: Vec<(String, String)> = TAG_KEY_FORMS
        .iter()
        .enumerate()
        .map(|(n, key)| ((*key).to_string(), format!("value-{n}")))
        .collect();

    // The modelled length bounds: the shortest and longest key `TagKey` allows, and the
    // shortest (empty) and longest value `TagValue` allows.
    let bounds = vec![
        ("k".to_string(), "v".to_string()),
        ("K".repeat(MAX_TAG_KEY_LEN), String::new()),
        (
            "boundary/value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN),
        ),
    ];

    vec![
        ReportCase {
            name: "a-graph-created-with-no-tags-reports-an-empty-map".to_string(),
            create: json!({}),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: Vec::new(),
        },
        ReportCase {
            name: "one-tag-stored-by-the-create".to_string(),
            create: json!({ "Tags": { "cost.center": "4711" } }),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: pairs(&[("cost.center", "4711")]),
        },
        ReportCase {
            name: "three-tags-stored-by-the-create".to_string(),
            create: json!({
                "Tags": { "with-dash": "a", "under_score": "b", "path/segment": "c" }
            }),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: pairs(&[("path/segment", "c"), ("under_score", "b"), ("with-dash", "a")]),
        },
        ReportCase {
            name: "tags-stored-only-by-a-later-TagResource".to_string(),
            create: json!({}),
            then_tag: pairs(&[("team:security", "oncall"), ("digits123", "7")]),
            then_untag: Vec::new(),
            expected: pairs(&[("digits123", "7"), ("team:security", "oncall")]),
        },
        ReportCase {
            name: "the-merge-of-a-create-seed-and-a-later-TagResource".to_string(),
            create: json!({ "Tags": { "Letters": "seed", "semi;colon": "kept" } }),
            then_tag: pairs(&[("Letters", "merged"), ("comma,separated", "added")]),
            then_untag: Vec::new(),
            expected: pairs(&[
                ("Letters", "merged"),
                ("comma,separated", "added"),
                ("semi;colon", "kept"),
            ]),
        },
        ReportCase {
            name: "what-an-UntagResource-left-behind".to_string(),
            create: json!({ "Tags": { "Letters": "a", "with-dash": "b", "digits123": "c" } }),
            then_tag: Vec::new(),
            then_untag: vec!["with-dash".to_string()],
            expected: pairs(&[("Letters", "a"), ("digits123", "c")]),
        },
        ReportCase {
            name: "an-emptied-map-reports-as-empty-again".to_string(),
            create: json!({ "Tags": { "Letters": "a", "digits123": "c" } }),
            then_tag: Vec::new(),
            then_untag: vec!["Letters".to_string(), "digits123".to_string()],
            expected: Vec::new(),
        },
        ReportCase {
            name: "every-modelled-key-form".to_string(),
            create: json!({ "Tags": tags_member(&forms) }),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: forms,
        },
        ReportCase {
            name: "the-modelled-key-and-value-length-bounds".to_string(),
            create: json!({ "Tags": tags_member(&bounds) }),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: bounds,
        },
        ReportCase {
            name: "a-graph-holding-the-whole-per-resource-tag-quota".to_string(),
            create: json!({ "Tags": tags_member(&quota) }),
            then_tag: Vec::new(),
            then_untag: Vec::new(),
            expected: quota,
        },
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_exactly_the_map_the_graph_holds() {
    for case in report_cases() {
        // A behavior graph is a per-account, per-Region singleton, so each row gets its
        // own emulator rather than sharing one graph.
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &case.create).await;
        if !case.then_tag.is_empty() {
            tag_ok(&router, &graph_arn, &case.then_tag, &case.name).await;
        }
        if !case.then_untag.is_empty() {
            untag_ok(&router, &graph_arn, &case.then_untag, &case.name).await;
        }

        let expected = tag_map(&case.expected);
        let reported = reported_tags(&router, REGION, &graph_arn, &case.name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the stored map {expected:?}, got {reported:?}",
            case.name
        );

        // The whole envelope, read raw: the modelled success status, and a body whose
        // only member is that map.
        let (status, body) = list_tags(&router, REGION, &graph_arn).await;
        assert_eq!(
            status, LIST_TAGS_STATUS,
            "case {:?}: expected {LIST_TAGS_STATUS}, got {status} {body:?}",
            case.name
        );
        let envelope = json!({ "Tags": Value::Object(expected.clone()) });
        assert_eq!(
            body, envelope,
            "case {:?}: expected the answer {envelope:?}, got {body:?}",
            case.name
        );
    }
}

/// Rule 1 over EVERY map size the store can hold, so each bound is covered rather than
/// sampled: none, one, the per-resource quota, and every size between. Each size's keys
/// and values are distinct, so a report that dropped, duplicated or crossed an entry
/// cannot pass by accident.
#[tokio::test]
async fn every_map_size_up_to_the_per_resource_quota_is_reported_whole() {
    for size in 0..=MAX_TAGS_PER_GRAPH {
        let case = format!("{size} stored tag(s)");
        let stored: Vec<(String, String)> = (0..size)
            .map(|n| {
                (
                    format!("{}/{n:02}", TAG_KEY_FORMS[n % TAG_KEY_FORMS.len()]),
                    format!("value-{size}-{n}"),
                )
            })
            .collect();

        let router = emulator();
        // Half the sizes are seeded by the create and half written afterwards, so the
        // sweep covers both writers at every size. The modelled `TagMap` length is
        // 1..50, so size 0 is the one row that can carry no `Tags` member at all.
        let (create, later) = match (size, size % 2) {
            (0, _) => (json!({}), Vec::new()),
            (_, 0) => (json!({ "Tags": tags_member(&stored) }), Vec::new()),
            _ => (json!({}), stored.clone()),
        };
        let graph_arn = create_graph_ok(&router, REGION, &create).await;
        if !later.is_empty() {
            tag_ok(&router, &graph_arn, &later, &case).await;
        }

        let expected = tag_map(&stored);
        let reported = reported_tags(&router, REGION, &graph_arn, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stored map {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_tags_changes_nothing_a_later_caller_sees() {
    const READS: usize = 5;

    let router = emulator();
    let stored = pairs(&[("Letters", "a"), ("cost.center", "4711"), ("eq=sign", "")]);
    let graph_arn = create_graph_ok(&router, REGION, &json!({ "Tags": tags_member(&stored) })).await;

    let expected = tag_map(&stored);
    let before = graph_records(&router).await;
    for read in 0..READS {
        let case = format!("read {read}");
        let reported = reported_tags(&router, REGION, &graph_arn, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: every read reports the same map {expected:?}, got {reported:?}"
        );
    }
    let after = graph_records(&router).await;
    assert_eq!(
        after, before,
        "case {READS} reads: the graph record must be untouched, expected {before:?}, got {after:?}"
    );

    // The same holds for a graph with no tags at all: reading an empty map neither
    // creates one nor disturbs the graph.
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let before = graph_records(&router).await;
    for read in 0..READS {
        let case = format!("untagged read {read}");
        let reported = reported_tags(&router, REGION, &graph_arn, &case).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: an untagged graph reports an empty map, got {reported:?}"
        );
    }
    let after = graph_records(&router).await;
    assert_eq!(
        after, before,
        "case untagged {READS} reads: the graph record must be untouched, expected {before:?}, got {after:?}"
    );

    // And a read does not disturb the WRITERS either: what a later TagResource merges
    // into is still the map the reads reported.
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({ "Tags": { "Letters": "a" } })).await;
    for read in 0..READS {
        reported_tags(&router, REGION, &graph_arn, &format!("pre-write read {read}")).await;
    }
    tag_ok(
        &router,
        &graph_arn,
        &pairs(&[("digits123", "7")]),
        "after the reads",
    )
    .await;
    let expected = tag_map(&pairs(&[("Letters", "a"), ("digits123", "7")]));
    let reported = reported_tags(&router, REGION, &graph_arn, "after the reads").await;
    assert_eq!(
        reported, expected,
        "case \"after the reads\": a write still merges into the reported map, expected {expected:?}, got {reported:?}"
    );
}

/// One created graph of the isolation table: the Region it was created in, the ARN it
/// was minted with, and the map it holds.
struct GraphRow {
    region: &'static str,
    arn: String,
    stored: Vec<(String, String)>,
}

#[tokio::test]
async fn each_graph_reports_its_own_map() {
    // A graph is one per account and Region, so the rows differ along the Region — and
    // along the map each graph holds, including one graph with no tags.
    let router = emulator();
    let rows = [
        ("us-west-2", pairs(&[("Letters", "west"), ("digits123", "2")])),
        ("us-east-1", pairs(&[("Letters", "east")])),
        ("eu-central-1", Vec::new()),
        (
            "ap-southeast-2",
            pairs(&[("Letters", "south"), ("path/segment", "x"), ("with-dash", "y")]),
        ),
    ];

    let mut created: Vec<GraphRow> = Vec::new();
    for (region, stored) in rows {
        let body = if stored.is_empty() {
            json!({})
        } else {
            json!({ "Tags": tags_member(&stored) })
        };
        let arn = create_graph_ok(&router, region, &body).await;
        created.push(GraphRow {
            region,
            arn,
            stored,
        });
    }

    for row in &created {
        let case = format!("the graph in {}", row.region);
        let expected = tag_map(&row.stored);
        let reported = reported_tags(&router, row.region, &row.arn, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected only its own map {expected:?}, got {reported:?}"
        );
    }

    // Removing one graph's tags leaves every other graph's map exactly as it was.
    let first = &created[0];
    let removed: Vec<String> = first.stored.iter().map(|(key, _)| key.clone()).collect();
    untag_ok(&router, &first.arn, &removed, "clearing the first graph").await;
    assert!(
        reported_tags(&router, first.region, &first.arn, "the cleared graph")
            .await
            .is_empty(),
        "the cleared graph must report an empty map"
    );
    for row in created.iter().skip(1) {
        let case = format!(
            "the graph in {} after another graph was cleared",
            row.region
        );
        let expected = tag_map(&row.stored);
        let reported = reported_tags(&router, row.region, &row.arn, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected its map untouched {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn list_tags_for_resource_on_a_graph_the_service_does_not_hold_is_the_modelled_404() {
    let router = emulator();

    // A graph that existed and was deleted: its tags go with it.
    let deleted = create_graph_ok(&router, REGION, &json!({ "Tags": { "Letters": "a" } })).await;
    let (status, body) = call(&router, "/graph/removal", &json!({ "GraphArn": deleted })).await;
    assert_eq!(
        status, 200,
        "DeleteGraph on the graph about to be absent: got {status} {body:?}"
    );

    // The rows differ along HOW the graph is absent and along the ARN's own shape — the
    // id's characters, the Region, the partition and the account — each row satisfying
    // the modelled `GraphArn` pattern, so every refusal comes from the handler's lookup
    // rather than from the constraint walk.
    let absent = [
        ("deleted", deleted.clone()),
        (
            "never-created-all-zero-id",
            graph_arn_of("aws", REGION, ACCOUNT, &"0".repeat(32)),
        ),
        (
            "never-created-all-f-id",
            graph_arn_of("aws", REGION, ACCOUNT, &"f".repeat(32)),
        ),
        (
            "never-created-mixed-id",
            graph_arn_of("aws", REGION, ACCOUNT, "0123456789abcdef0123456789abcdef"),
        ),
        (
            "never-created-in-another-region",
            graph_arn_of("aws", "eu-west-1", ACCOUNT, &"a".repeat(32)),
        ),
        (
            "never-created-in-another-partition",
            graph_arn_of("aws-us-gov", "us-gov-west-1", ACCOUNT, &"b".repeat(32)),
        ),
        (
            "never-created-in-another-account",
            graph_arn_of("aws", REGION, "123456789012", &"c".repeat(32)),
        ),
    ];

    for (how, arn) in absent {
        let case = format!("ListTagsForResource on a {how} graph");
        let (status, answered) = list_tags(&router, REGION, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered:?}"
        );
        let message = error_message(&answered);
        // The expectation is this service's own fixed constant, which every operation
        // that resolves a graph shares: what this row pins is that the reader answers
        // the SAME input-independent sentence the writers do, not the wording itself —
        // the status and the error code are the wire requirements.
        assert_eq!(
            message, GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the fixed graph-not-found message, got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()),
            "case {case:?}: the message must not echo the requested ARN, got {message:?}"
        );
        assert!(
            answered.get("Tags").is_none(),
            "case {case:?}: a refusal carries no tag data, got {answered:?}"
        );
    }

    // The refusal is a refusal, not a create: the store is still empty, so the next
    // CreateGraph in this Region mints a graph and that graph starts out untagged.
    let page = graph_records(&router).await;
    assert_eq!(
        page.get("GraphList"),
        Some(&json!([])),
        "the refused reads must have stored nothing, got {page:?}"
    );
    let fresh = create_graph_ok(&router, REGION, &json!({})).await;
    assert_ne!(
        fresh, deleted,
        "expected a freshly minted graph, got the deleted ARN {fresh:?}"
    );
    assert!(
        reported_tags(&router, REGION, &fresh, "the freshly minted graph")
            .await
            .is_empty(),
        "a freshly minted graph reports an empty map"
    );
}
