// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `TagResource` over the awsJson1_1 wire, read back through
//! `ListTagsForResource`.
//!
//! The rule under test: `TagResource` MERGES the submitted `ResourceTagList` into the tag
//! set persisted for the export its `ResourceArn` names — a submitted key the export does
//! not carry is added, a key it already carries takes the submitted value, and a key the
//! request does not name keeps the value it had — so the call is additive, idempotent for
//! a list already in effect, and observable by every later reader; it reports no payload
//! ("Adds tags for an existing data export definition", `TagResourceResponse` has no
//! members), refuses an ARN that names no export with the modelled
//! `ResourceNotFoundException`, and refuses a
//! request it cannot read as a tag set ("each key must be unique for the resource" —
//! smithy_query shape `TagResourceRequest`, member `ResourceTags`) with the modelled 400.
//!
//! It is asserted over a TABLE whose rows differ along everything the merge rule
//! generalises over: the number of tags already on the export and the number submitted
//! (zero, one, many), how the two key sets relate (disjoint, overlapping, identical), the
//! submission order versus the reported order, and the published bounds of the tag
//! member constraints — `ResourceTagKey` `@length 1..128` and `ResourceTagValue`
//! `@length 0..256` (smithy_query shapes `ResourceTagKey`, `ResourceTagValue`).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    MAX_RESOURCE_TAGS, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// `ResourceTagKey`: `@length 1..128` (smithy_query shape `ResourceTagKey`).
const KEY_MIN_LEN: usize = 1;
const KEY_MAX_LEN: usize = 128;

/// `ResourceTagValue`: `@length 0..256` (smithy_query shape `ResourceTagValue`).
const VALUE_MAX_LEN: usize = 256;

/// A tag set as key -> value, which is how the service holds it: one value per key.
type Tags = BTreeMap<String, String>;

/// A tag set as a `ResourceTagList`, in the order given.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Owned `(key, value)` pairs from string literals, so a row can be written inline.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create an export named `name` carrying `tags`, and return the ARN the service minted.
///
/// The export DEFINITION is the first row of the lifecycle suite's table — this suite is
/// about the tag set, which is the only part of the request it varies.
async fn create_export(client: &Client, name: &str, tags: &[(String, String)]) -> String {
    let mut request = cases().remove(0).create_request();
    request["Export"]["Name"] = json!(name);
    request["ResourceTags"] = tag_list(tags);
    let created = client.ok("CreateExport", &request).await;
    created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExport must return an ExportArn, got {created}"))
        .to_string()
}

/// Every tag `ListTagsForResource` reports for `arn`, as a map — asserting on the way that
/// the reported list is ordered by key and names each key once.
async fn read_tags(case: &str, client: &Client, arn: &str) -> Tags {
    let listed = client
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    let reported = listed["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource must report ResourceTags, got {listed}"))
        .clone();

    let mut tags = Tags::new();
    let mut keys: Vec<String> = Vec::new();
    for tag in &reported {
        let key = tag["Key"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: every tag carries a Key, got {tag}"))
            .to_string();
        let value = tag["Value"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: every tag carries a Value, got {tag}"))
            .to_string();
        assert!(
            tags.insert(key.clone(), value).is_none(),
            "case {case:?}: each key must be unique for the resource, but {key:?} appears twice in {reported:?}"
        );
        keys.push(key);
    }
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "case {case:?}: expected the reported tags in key order, got {keys:?}"
    );
    tags
}

/// Call `TagResource` for `arn` with `tags`, requiring the modelled empty 200.
async fn tag_resource(case: &str, client: &Client, arn: &str, tags: &[(String, String)]) {
    let body = client
        .ok(
            "TagResource",
            &json!({ "ResourceArn": arn, "ResourceTags": tag_list(tags) }),
        )
        .await;
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: TagResourceResponse models no members, got {body}"
    );
}

/// One row: what the export already carries, and what the call submits.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    /// The export's name, which is also the key of its ARN — distinct per row so each row
    /// gets its own export.
    name: String,
    existing: Vec<(String, String)>,
    submitted: Vec<(String, String)>,
}

impl Row {
    /// The tag set the merge rule says the export holds after the call: the existing set
    /// with every submitted key set to its submitted value.
    fn expected(&self) -> Tags {
        let mut expected: Tags = self.existing.iter().cloned().collect();
        expected.extend(self.submitted.iter().cloned());
        expected
    }
}

/// The table: one row per shape of merge the rule has to hold for.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            label: "untagged export, one tag submitted",
            name: "untagged_one".to_string(),
            existing: Vec::new(),
            submitted: pairs(&[("env", "dev")]),
        },
        Row {
            label: "empty submission leaves the set alone",
            name: "empty_submission".to_string(),
            existing: pairs(&[("env", "dev"), ("owner", "finops")]),
            submitted: Vec::new(),
        },
        Row {
            label: "empty submission on an untagged export",
            name: "empty_on_empty".to_string(),
            existing: Vec::new(),
            submitted: Vec::new(),
        },
        Row {
            label: "disjoint keys are added",
            name: "disjoint_keys".to_string(),
            existing: pairs(&[("env", "dev")]),
            submitted: pairs(&[("owner", "finops"), ("tier", "gold")]),
        },
        Row {
            label: "an existing key takes the submitted value",
            name: "overwrite_one".to_string(),
            existing: pairs(&[("env", "dev"), ("owner", "finops")]),
            submitted: pairs(&[("env", "prod")]),
        },
        Row {
            label: "identical key sets, every value replaced",
            name: "replace_all".to_string(),
            existing: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            submitted: pairs(&[("a", "9"), ("b", "8"), ("c", "7")]),
        },
        Row {
            label: "overlapping sets: one replaced, one added",
            name: "overlap_mixed".to_string(),
            existing: pairs(&[("env", "dev"), ("owner", "finops")]),
            submitted: pairs(&[("env", "staging"), ("cost-center", "4711")]),
        },
        Row {
            label: "re-submitting the value already stored",
            name: "same_value".to_string(),
            existing: pairs(&[("env", "dev")]),
            submitted: pairs(&[("env", "dev")]),
        },
        Row {
            label: "a value may be emptied (ResourceTagValue length 0)",
            name: "empty_value".to_string(),
            existing: pairs(&[("note", "something")]),
            submitted: pairs(&[("note", "")]),
        },
        Row {
            label: "submitted out of key order, reported in key order",
            name: "unordered_submission".to_string(),
            existing: pairs(&[("m", "middle")]),
            submitted: pairs(&[("z", "last"), ("a", "first"), ("n", "next")]),
        },
        // The published bounds of the tag member constraints.
        Row {
            label: "key at the minimum length",
            name: "key_min_len".to_string(),
            existing: Vec::new(),
            submitted: vec![("k".repeat(KEY_MIN_LEN), "short key".to_string())],
        },
        Row {
            label: "key at the maximum length",
            name: "key_max_len".to_string(),
            existing: vec![("K".repeat(KEY_MAX_LEN), "before".to_string())],
            submitted: vec![("K".repeat(KEY_MAX_LEN), "after".to_string())],
        },
        Row {
            label: "value at the maximum length",
            name: "value_max_len".to_string(),
            existing: Vec::new(),
            submitted: vec![("long-value".to_string(), "v".repeat(VALUE_MAX_LEN))],
        },
    ];

    // A mechanical sweep over the counts the rule generalises over: how many tags the
    // export already carries crossed with how many the call submits, with half the
    // submitted keys overlapping the existing ones.
    for existing_count in [0usize, 1, 7] {
        for submitted_count in [0usize, 1, 9, 40] {
            let existing: Vec<(String, String)> = (0..existing_count)
                .map(|i| (format!("key-{i:03}"), format!("old-{i}")))
                .collect();
            let submitted: Vec<(String, String)> = (0..submitted_count)
                .map(|i| {
                    if i % 2 == 0 && i < existing_count {
                        (format!("key-{i:03}"), format!("new-{i}"))
                    } else {
                        (format!("added-{i:03}"), format!("value-{i}"))
                    }
                })
                .collect();
            rows.push(Row {
                label: "count sweep",
                name: format!("sweep_{existing_count}_{submitted_count}"),
                existing,
                submitted,
            });
        }
    }
    rows
}

/// `TagResource` merges the submitted list into the export's persisted tag set, and every
/// later reader sees exactly the merged set.
#[tokio::test]
async fn tag_resource_merges_the_submitted_tags_into_the_persisted_set() {
    let client = Client::new();

    for row in rows() {
        let label = format!("{} [{}]", row.label, row.name);
        let arn = create_export(&client, &row.name, &row.existing).await;

        // What the export looked like before the call, so the tagging can be shown to
        // leave the definition and its status timestamps alone.
        let before = client.ok("GetExport", &json!({ "ExportArn": &arn })).await;

        tag_resource(&label, &client, &arn, &row.submitted).await;

        let expected = row.expected();
        let actual = read_tags(&label, &client, &arn).await;
        assert_eq!(
            actual, expected,
            "case {label:?}: expected the merged tag set {expected:?}, got {actual:?}"
        );

        // Idempotent: the same list applied again is already in effect.
        tag_resource(&label, &client, &arn, &row.submitted).await;
        let repeated = read_tags(&label, &client, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {label:?}: re-applying the same list must change nothing, got {repeated:?}"
        );

        // Tagging is not an export update: the definition and the status it carries are
        // untouched.
        let after = client.ok("GetExport", &json!({ "ExportArn": &arn })).await;
        assert_eq!(
            after, before,
            "case {label:?}: TagResource must not change the export definition or its status, got {after}"
        );
    }
}

/// A key the request names twice cannot be satisfied — the list would set two values for
/// one tag — so it is refused with the modelled 400 and NOTHING is written.
///
/// Asserted over the shapes a duplicate takes: adjacent and separated, with equal and
/// with different values, on a key the export already carries and on one it does not, and
/// with a key repeated more than twice.
#[tokio::test]
async fn a_duplicated_tag_key_is_refused_and_persists_nothing() {
    let client = Client::new();
    let existing = pairs(&[("env", "dev"), ("owner", "finops")]);

    let duplicates: Vec<(&str, Vec<(String, String)>)> = vec![
        ("adjacent, differing values", pairs(&[("a", "1"), ("a", "2")])),
        ("adjacent, equal values", pairs(&[("a", "1"), ("a", "1")])),
        (
            "separated by other keys",
            pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("a", "4")]),
        ),
        (
            "a key the export already carries",
            pairs(&[("env", "prod"), ("env", "staging")]),
        ),
        (
            "repeated three times",
            pairs(&[("a", "1"), ("a", "2"), ("a", "3")]),
        ),
        (
            "duplicate at the maximum key length",
            vec![
                ("K".repeat(KEY_MAX_LEN), "1".to_string()),
                ("K".repeat(KEY_MAX_LEN), "2".to_string()),
            ],
        ),
    ];

    for (index, (label, submitted)) in duplicates.into_iter().enumerate() {
        let arn = create_export(&client, &format!("duplicate_{index}"), &existing).await;
        let (status, body) = client
            .call(
                "TagResource",
                &json!({ "ResourceArn": &arn, "ResourceTags": tag_list(&submitted) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: a duplicated tag key must be refused, got {status} with {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!("ResourceTags"),
            "case {label:?}: the refusal must name the offending member, got {body}"
        );

        let actual = read_tags(label, &client, &arn).await;
        let expected: Tags = existing.iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: a refused call must persist nothing, expected {expected:?}, got {actual:?}"
        );
    }
}

/// An ARN of THIS ACCOUNT that names no export is refused with the modelled
/// `ResourceNotFoundException`, whether
/// it was never created or has since been deleted — and a deleted export's tags do not come
/// back. An ARN of ANOTHER account is refused before any lookup, with the modelled 400:
/// see `tests/all/tagging_arn_validation.rs` for that rule, which this test's foreign rows
/// cross-check on `TagResource`.
#[tokio::test]
async fn tagging_an_arn_that_names_no_export_is_not_found() {
    let client = Client::new();
    let own = ::engine::emulator_account_id();
    let live = create_export(&client, "live_export", &pairs(&[("env", "dev")])).await;
    client.ok("DeleteExport", &json!({ "ExportArn": &live })).await;

    let absent = vec![
        ("never created, own account", format!("arn:aws:bcm-data-exports:us-east-1:{own}:export/never_created")),
        // An own-account ARN whose resource is not even an export form: the tagging check
        // judges the account only, so this is still looked up and not found.
        (
            "own account, another resource class",
            format!("arn:aws:bcm-data-exports:us-east-1:{own}:table/COST_AND_USAGE_REPORT"),
        ),
        ("deleted export", live.clone()),
    ];

    for (label, arn) in absent {
        let (status, body) = client
            .call(
                "TagResource",
                &json!({
                    "ResourceArn": &arn,
                    "ResourceTags": tag_list(&pairs(&[("env", "dev")])),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {label:?}: tagging an absent export must be the not-found refusal, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(RESOURCE_NOT_FOUND_MESSAGE),
            "case {label:?}: expected this service's fixed not-found message, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(arn),
            "case {label:?}: the refusal must name the ARN it could not find, got {body}"
        );
    }

    // And the refusals created nothing: the deleted ARN is still absent to its reader.
    let (status, body) = client
        .call("ListTagsForResource", &json!({ "ResourceArn": &live }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "a refused TagResource must not resurrect the export, got {status} with {body}"
    );
}

/// A resource holds at most `ResourceTagList`'s modelled 200 tags — the bound on the very
/// list `ListTagsForResource` reports it through — so a call that would take it past that
/// is refused and the set is left as it was, while a call that only REPLACES values at the
/// ceiling still succeeds.
#[tokio::test]
async fn the_tag_set_is_bounded_by_the_modelled_list_length() {
    let client = Client::new();

    // Each published bound of the merge: one short of the ceiling, exactly at it, and one
    // past it.
    let filler = |count: usize, value: &str| -> Vec<(String, String)> {
        (0..count)
            .map(|i| (format!("key-{i:03}"), format!("{value}-{i}")))
            .collect()
    };

    // Exactly at the ceiling, reached in one call.
    let at_ceiling = create_export(&client, "at_ceiling", &[]).await;
    let full = filler(MAX_RESOURCE_TAGS, "v");
    tag_resource("at the ceiling", &client, &at_ceiling, &full).await;
    let tags = read_tags("at the ceiling", &client, &at_ceiling).await;
    assert_eq!(
        tags.len(),
        MAX_RESOURCE_TAGS,
        "case \"at the ceiling\": expected {MAX_RESOURCE_TAGS} tags, got {}",
        tags.len()
    );

    // Replacing values at the ceiling adds no key, so it is still accepted.
    let replaced = filler(MAX_RESOURCE_TAGS, "w");
    tag_resource("replace at the ceiling", &client, &at_ceiling, &replaced).await;
    let tags = read_tags("replace at the ceiling", &client, &at_ceiling).await;
    let expected: Tags = replaced.iter().cloned().collect();
    assert_eq!(
        tags, expected,
        "case \"replace at the ceiling\": a replacement at the ceiling must be honoured"
    );

    // One key past the ceiling is refused, and nothing is written.
    let (status, body) = client
        .call(
            "TagResource",
            &json!({
                "ResourceArn": &at_ceiling,
                "ResourceTags": tag_list(&pairs(&[("one-too-many", "x")])),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"one past the ceiling\": expected a refusal, got {status} with {body}"
    );
    let tags = read_tags("one past the ceiling", &client, &at_ceiling).await;
    assert_eq!(
        tags, expected,
        "case \"one past the ceiling\": a refused call must persist nothing, got {tags:?}"
    );

    // One short of the ceiling: a single added key fits exactly, a second does not.
    let short = create_export(&client, "one_short", &[]).await;
    tag_resource(
        "one short of the ceiling",
        &client,
        &short,
        &filler(MAX_RESOURCE_TAGS - 1, "v"),
    )
    .await;
    tag_resource(
        "the last free slot",
        &client,
        &short,
        &pairs(&[("last-slot", "x")]),
    )
    .await;
    let tags = read_tags("the last free slot", &client, &short).await;
    assert_eq!(
        tags.len(),
        MAX_RESOURCE_TAGS,
        "case \"the last free slot\": expected the set to fill to {MAX_RESOURCE_TAGS}, got {}",
        tags.len()
    );
    let (status, body) = client
        .call(
            "TagResource",
            &json!({
                "ResourceArn": &short,
                "ResourceTags": tag_list(&pairs(&[("over", "x")])),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"past a filled set\": expected a refusal, got {status} with {body}"
    );
}
