// SPDX-License-Identifier: CC0-1.0

//! Amazon Keyspaces resource tagging — `TagResource`, `UntagResource` and
//! `ListTagsForResource` — driven end to end over the service's own `awsJson1_0` wire
//! protocol through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! Every assertion goes through the HTTP surface a real client uses, and a tag is only
//! believed once the reader reports it back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A tag is identified by its KEY: tagging a resource adds a key it does not hold and
//!    REPLACES the value of one it does ("Each Amazon Keyspaces resource can only have up
//!    to one tag with the same key. If you try to add an existing tag (same key), the
//!    existing tag value will be updated to the new value", `Tag$key`). Keys are case
//!    sensitive.
//! 2. The tags a resource holds are the ones the reader reports, for every resource form
//!    this service mints an ARN for (a keyspace and a table), whether they were supplied
//!    at create time or added afterwards.
//! 3. A resource holds at most 50 tags ("You can add up to 50 tags to a single Amazon
//!    Keyspaces resource", `Tag`); a call that would carry it past that is refused as an
//!    invalid request (the modelled `ValidationException`), and stores nothing. The
//!    sentence that refusal carries, and the same rule at the other operations that take
//!    tags, are pinned by `tests/all/tag_count_limit_refusal.rs`.
//! 4. `UntagResource` removes the tags its keys name and is idempotent on a key the
//!    resource does not hold.
//! 5. A `resourceArn` that names no stored resource is the modelled
//!    `ResourceNotFoundException`, for every one of the three operations.
//! 6. `ListTagsForResource` pages a resource's tags in one total order, resuming exactly
//!    where the previous page stopped, for any page size.
//! 7. The `aws:` key prefix is reserved ("Amazon Web Services-assigned tag names and
//!    values are automatically assigned the `aws:` prefix, which the user cannot assign",
//!    `Tag`), so a caller may neither assign nor remove one.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, table_arn, Emulator, REGION,
};

/// The keyspace and table every case below tags, created once per emulator.
const KEYSPACE: &str = "tag_ks";
const TABLE: &str = "tag_tbl";

/// A `TagList` request member built from key/value pairs, in the order given — so a case
/// can repeat a key and pin which value wins.
///
/// Shared with `tests/all/tag_count_limit_refusal.rs`, so there is ONE builder of a tag
/// list for the suites that send one rather than a copy per file.
pub(crate) fn tags_json<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key.as_ref(), "value": value.as_ref()}))
            .collect(),
    )
}

/// The same pairs as the tag set they describe: last value wins per key, which is what
/// `Tag$key` documents.
fn expected_set<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
        .collect()
}

/// Create the keyspace and table the cases tag, and hand back their ARNs.
async fn with_resources(emulator: &Emulator) -> [String; 2] {
    assert_ok(
        "create keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": KEYSPACE}))
            .await,
    );
    assert_ok(
        "create table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": TABLE,
                    "schemaDefinition": minimal_schema("id"),
                }),
            )
            .await,
    );
    [keyspace_arn(KEYSPACE), table_arn(KEYSPACE, TABLE)]
}

/// Every tag the resource holds, read through `ListTagsForResource` one page at a time.
///
/// Returns the pages' entries in the order they were reported, so a caller can assert
/// both the SET and its ORDER.
async fn list_tags(
    case: &str,
    emulator: &Emulator,
    resource_arn: &str,
    max_results: Option<i64>,
) -> Vec<(String, String)> {
    let mut reported: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page carries at least one tag, so a resource within the published quota cannot
    // need more pages than it holds tags: anything beyond that is a pagination bug, not a
    // longer answer.
    for _ in 0..=keyspaces::MAX_TAGS_PER_RESOURCE {
        let mut request = json!({"resourceArn": resource_arn});
        if let Some(limit) = max_results {
            request["maxResults"] = json!(limit);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let answer = assert_ok(case, emulator.call("ListTagsForResource", request).await);
        let page = answer.get("tags").and_then(Value::as_array).cloned();
        if let Some(page) = &page {
            assert!(
                !page.is_empty(),
                "case {case:?}: the modelled TagList is len 1..60, so an empty page must \
                 be reported as an absent member, got {answer}"
            );
            if let Some(limit) = max_results {
                assert!(
                    page.len() as i64 <= limit,
                    "case {case:?}: expected at most {limit} tags on a page, got {}",
                    page.len()
                );
            }
            for tag in page {
                reported.push((
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
        match answer.get("nextToken").and_then(Value::as_str) {
            Some(token) => {
                assert!(
                    page.is_some(),
                    "case {case:?}: a nextToken promises a further page, so this one \
                     cannot be empty: {answer}"
                );
                next_token = Some(token.to_string());
            }
            None => return reported,
        }
    }
    panic!("case {case:?}: ListTagsForResource never reported a final page");
}

/// RULE 1 and RULE 2: tagging adds a key the resource does not hold, replaces the value
/// of one it does, and the reader reports exactly the resulting set — for a keyspace ARN
/// and for a table ARN alike.
///
/// The rows differ along what the rule generalises over: how many tags a call carries,
/// the key and value lengths (each published bound of `TagKey` `len 1..128` and
/// `TagValue` `len 1..256`), the character case of a key, and whether the call overlaps
/// the keys already held.
#[tokio::test]
async fn tagging_a_resource_upserts_by_key_and_reads_back_exactly() {
    /// One row: the calls to make, in order, and the tag set they must leave behind.
    struct Case {
        name: &'static str,
        calls: Vec<Vec<(String, String)>>,
        expected: BTreeMap<String, String>,
    }

    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let cases = vec![
        Case {
            name: "one tag",
            calls: vec![vec![("team".to_string(), "orders".to_string())]],
            expected: expected_set(&[("team", "orders")]),
        },
        Case {
            name: "several tags in one call",
            calls: vec![vec![
                ("team".to_string(), "orders".to_string()),
                ("env".to_string(), "prod".to_string()),
                ("costCenter".to_string(), "4711".to_string()),
            ]],
            expected: expected_set(&[("team", "orders"), ("env", "prod"), ("costCenter", "4711")]),
        },
        Case {
            name: "the published key and value bounds",
            calls: vec![vec![
                (longest_key.clone(), longest_value.clone()),
                ("a".to_string(), "1".to_string()),
            ]],
            expected: expected_set(&[
                (longest_key.as_str(), longest_value.as_str()),
                ("a", "1"),
            ]),
        },
        Case {
            name: "a repeated key in one call keeps the last value",
            calls: vec![vec![
                ("env".to_string(), "staging".to_string()),
                ("env".to_string(), "prod".to_string()),
            ]],
            expected: expected_set(&[("env", "prod")]),
        },
        Case {
            name: "a later call replaces the value of a key already held",
            calls: vec![
                vec![
                    ("env".to_string(), "staging".to_string()),
                    ("team".to_string(), "orders".to_string()),
                ],
                vec![("env".to_string(), "prod".to_string())],
            ],
            expected: expected_set(&[("env", "prod"), ("team", "orders")]),
        },
        Case {
            name: "keys are case sensitive, so Env and env are two tags",
            calls: vec![vec![
                ("env".to_string(), "prod".to_string()),
                ("Env".to_string(), "PROD".to_string()),
            ]],
            expected: expected_set(&[("env", "prod"), ("Env", "PROD")]),
        },
        Case {
            name: "a value differing only in case is still a replacement",
            calls: vec![
                vec![("env".to_string(), "prod".to_string())],
                vec![("env".to_string(), "PROD".to_string())],
            ],
            expected: expected_set(&[("env", "PROD")]),
        },
    ];

    for case in &cases {
        // A fresh emulator per row, so each row starts from a resource holding no tags.
        let emulator = Emulator::start();
        let arns = with_resources(&emulator).await;
        for resource_arn in &arns {
            let case_name = format!("{} on {resource_arn}", case.name);
            for call in &case.calls {
                assert_ok(
                    &case_name,
                    emulator
                        .call(
                            "TagResource",
                            json!({"resourceArn": resource_arn, "tags": tags_json(call)}),
                        )
                        .await,
                );
            }
            let reported = list_tags(&case_name, &emulator, resource_arn, None).await;
            let as_set: BTreeMap<String, String> = reported.iter().cloned().collect();
            assert_eq!(
                as_set, case.expected,
                "case {case_name:?}: expected the tag set {:?}, got {reported:?}",
                case.expected
            );
        }
    }
}

/// RULE 2, for the tags a create supplied: they are the same tag set the reader reports,
/// and a later `TagResource` merges into it rather than replacing it.
#[tokio::test]
async fn tags_supplied_at_create_are_the_ones_the_reader_reports() {
    let emulator = Emulator::start();
    assert_ok(
        "create keyspace with tags",
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tags": tags_json(&[("env", "prod"), ("team", "orders")]),
                }),
            )
            .await,
    );
    assert_ok(
        "create table with tags",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": TABLE,
                    "schemaDefinition": minimal_schema("id"),
                    "tags": tags_json(&[("env", "staging")]),
                }),
            )
            .await,
    );

    for (resource_arn, created, after_tagging) in [
        (
            keyspace_arn(KEYSPACE),
            expected_set(&[("env", "prod"), ("team", "orders")]),
            expected_set(&[("env", "dev"), ("team", "orders"), ("tier", "gold")]),
        ),
        (
            table_arn(KEYSPACE, TABLE),
            expected_set(&[("env", "staging")]),
            expected_set(&[("env", "dev"), ("tier", "gold")]),
        ),
    ] {
        let reported: BTreeMap<String, String> =
            list_tags("after create", &emulator, &resource_arn, None)
                .await
                .into_iter()
                .collect();
        assert_eq!(
            reported, created,
            "case {resource_arn:?}: expected the tags the create supplied {created:?}, got {reported:?}"
        );

        assert_ok(
            &resource_arn,
            emulator
                .call(
                    "TagResource",
                    json!({
                        "resourceArn": &resource_arn,
                        "tags": tags_json(&[("env", "dev"), ("tier", "gold")]),
                    }),
                )
                .await,
        );
        let merged: BTreeMap<String, String> =
            list_tags("after tagging", &emulator, &resource_arn, None)
                .await
                .into_iter()
                .collect();
        assert_eq!(
            merged, after_tagging,
            "case {resource_arn:?}: expected {after_tagging:?} after tagging, got {merged:?}"
        );
    }
}

/// RULE 3: a resource holds at most [`keyspaces::MAX_TAGS_PER_RESOURCE`] tags, and a call
/// that would carry it past that is refused as an invalid request while the tag set it
/// would have changed stays exactly as it was.
///
/// The rows span the boundary from both sides and in both shapes the quota can be
/// breached: in one call, and across calls. A row at the limit, and one that repeats keys
/// so a request longer than the quota still fits, are the accepted side of the same rule.
#[tokio::test]
async fn tagging_past_the_published_per_resource_quota_is_refused_and_stores_nothing() {
    let quota = keyspaces::MAX_TAGS_PER_RESOURCE;
    /// `n` distinct tags, named so their key order is their creation order.
    fn distinct(count: usize, value: &str) -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("key{index:03}"), value.to_string()))
            .collect()
    }

    struct Case {
        name: String,
        /// Calls that must be accepted, in order.
        accepted: Vec<Vec<(String, String)>>,
        /// The call that must be refused for the tag count it would leave, if the row has
        /// one.
        refused: Option<Vec<(String, String)>>,
        /// How many tags the resource holds afterwards.
        expected_count: usize,
    }

    let cases = vec![
        Case {
            name: format!("exactly {quota} tags in one call"),
            accepted: vec![distinct(quota, "v")],
            refused: None,
            expected_count: quota,
        },
        Case {
            name: format!("{} tags in one call", quota + 1),
            accepted: vec![],
            refused: Some(distinct(quota + 1, "v")),
            expected_count: 0,
        },
        Case {
            name: format!("{quota} tags, then one more key"),
            accepted: vec![distinct(quota, "v")],
            refused: Some(vec![("one_too_many".to_string(), "v".to_string())]),
            expected_count: quota,
        },
        Case {
            name: format!("{quota} tags, then a new value for a key already held"),
            accepted: vec![distinct(quota, "v"), vec![("key000".to_string(), "w".to_string())]],
            refused: None,
            expected_count: quota,
        },
        Case {
            name: format!("{quota} tags, then a batch whose keys are all already held"),
            accepted: vec![distinct(quota, "v"), distinct(quota, "w")],
            refused: None,
            expected_count: quota,
        },
        Case {
            // The modelled `TagList` maximum is 60, which is NOT the quota: a request
            // longer than the quota is inside the modelled bound and is accepted as long
            // as the resulting tag set is not.
            name: format!("{} tags of which only {} keys are distinct", quota + 10, quota - 1),
            accepted: vec![{
                let mut tags = distinct(quota - 1, "v");
                tags.extend(distinct(11, "w"));
                tags
            }],
            refused: None,
            expected_count: quota - 1,
        },
        Case {
            name: format!("{quota} tags, then a batch mixing held keys with one new one", ),
            accepted: vec![distinct(quota, "v")],
            refused: Some({
                let mut tags = distinct(quota, "w");
                tags.push(("extra".to_string(), "w".to_string()));
                tags
            }),
            expected_count: quota,
        },
    ];

    for case in &cases {
        let emulator = Emulator::start();
        let arns = with_resources(&emulator).await;
        for resource_arn in &arns {
            let case_name = format!("{} on {resource_arn}", case.name);
            for call in &case.accepted {
                assert_ok(
                    &case_name,
                    emulator
                        .call(
                            "TagResource",
                            json!({"resourceArn": resource_arn, "tags": tags_json(call)}),
                        )
                        .await,
                );
            }
            let before = list_tags(&case_name, &emulator, resource_arn, None).await;
            if let Some(refused) = &case.refused {
                assert_failure(
                    &case_name,
                    emulator
                        .call(
                            "TagResource",
                            json!({"resourceArn": resource_arn, "tags": tags_json(refused)}),
                        )
                        .await,
                    400,
                    "ValidationException",
                );
            }
            let after = list_tags(&case_name, &emulator, resource_arn, None).await;
            assert_eq!(
                after, before,
                "case {case_name:?}: a refused call must leave the tag set as it was, \
                 expected {before:?}, got {after:?}"
            );
            assert_eq!(
                after.len(),
                case.expected_count,
                "case {case_name:?}: expected {} tags, got {after:?}",
                case.expected_count
            );
        }
    }

    // One past the MODELLED request bound (`TagList` len 1..60) is refused by the
    // boundary validator the model drives, with the same 400 `ValidationException` the
    // handler answers for a resulting set past the per-resource limit — a different rule,
    // decided a stage earlier.
    let emulator = Emulator::start();
    let arns = with_resources(&emulator).await;
    for resource_arn in &arns {
        assert_failure(
            &format!("61 tags in one call on {resource_arn}"),
            emulator
                .call(
                    "TagResource",
                    json!({"resourceArn": resource_arn, "tags": tags_json(&distinct(61, "v"))}),
                )
                .await,
            400,
            "ValidationException",
        );
    }
}

/// RULE 4: `UntagResource` removes the tags its keys name, leaves every other tag, and is
/// idempotent on a key the resource does not hold.
///
/// The rows differ along how many keys the call names, whether they are held, and whether
/// the supplied value matches the stored one — the KEY identifies the tag, so it does not.
#[tokio::test]
async fn untagging_removes_the_keys_it_names_and_is_idempotent() {
    struct Case {
        name: &'static str,
        held: Vec<(&'static str, &'static str)>,
        remove: Vec<(&'static str, &'static str)>,
        expected: BTreeMap<String, String>,
    }

    let cases = vec![
        Case {
            name: "the only tag",
            held: vec![("env", "prod")],
            remove: vec![("env", "prod")],
            expected: BTreeMap::new(),
        },
        Case {
            name: "one of several, leaving the rest",
            held: vec![("env", "prod"), ("team", "orders"), ("tier", "gold")],
            remove: vec![("team", "orders")],
            expected: expected_set(&[("env", "prod"), ("tier", "gold")]),
        },
        Case {
            name: "several keys in one call",
            held: vec![("env", "prod"), ("team", "orders"), ("tier", "gold")],
            remove: vec![("env", "prod"), ("tier", "gold")],
            expected: expected_set(&[("team", "orders")]),
        },
        Case {
            name: "every key, leaving no tags",
            held: vec![("env", "prod"), ("team", "orders")],
            remove: vec![("env", "prod"), ("team", "orders")],
            expected: BTreeMap::new(),
        },
        Case {
            name: "a key the resource does not hold removes nothing and is not an error",
            held: vec![("env", "prod")],
            remove: vec![("absent", "whatever")],
            expected: expected_set(&[("env", "prod")]),
        },
        Case {
            name: "the key identifies the tag, so a different value still removes it",
            held: vec![("env", "prod"), ("team", "orders")],
            remove: vec![("env", "not-the-stored-value")],
            expected: expected_set(&[("team", "orders")]),
        },
        Case {
            name: "keys are case sensitive, so Env does not remove env",
            held: vec![("env", "prod"), ("Env", "PROD")],
            remove: vec![("Env", "PROD")],
            expected: expected_set(&[("env", "prod")]),
        },
        Case {
            name: "a mix of held and absent keys",
            held: vec![("env", "prod"), ("team", "orders")],
            remove: vec![("env", "prod"), ("absent", "x")],
            expected: expected_set(&[("team", "orders")]),
        },
    ];

    for case in &cases {
        let emulator = Emulator::start();
        let arns = with_resources(&emulator).await;
        for resource_arn in &arns {
            let case_name = format!("{} on {resource_arn}", case.name);
            assert_ok(
                &case_name,
                emulator
                    .call(
                        "TagResource",
                        json!({"resourceArn": resource_arn, "tags": tags_json(&case.held)}),
                    )
                    .await,
            );
            // Called TWICE: the second call must change nothing, which is what idempotence
            // means for a removal.
            for attempt in 1..=2 {
                assert_ok(
                    &format!("{case_name} (attempt {attempt})"),
                    emulator
                        .call(
                            "UntagResource",
                            json!({"resourceArn": resource_arn, "tags": tags_json(&case.remove)}),
                        )
                        .await,
                );
                let reported: BTreeMap<String, String> =
                    list_tags(&case_name, &emulator, resource_arn, None)
                        .await
                        .into_iter()
                        .collect();
                assert_eq!(
                    reported, case.expected,
                    "case {case_name:?} (attempt {attempt}): expected {:?}, got {reported:?}",
                    case.expected
                );
            }
        }
    }
}

/// RULE 7: the `aws:` key prefix is reserved, so a caller may neither assign one nor
/// remove one, on any operation that takes tags.
#[tokio::test]
async fn reserved_tag_keys_are_refused_wherever_tags_are_supplied() {
    let reserved: [&str; 4] = [
        "aws:cloudformation:stack-name",
        "aws:createdBy",
        // The prefix alone, and a key that merely starts with it, are the same rule.
        "aws:",
        "aws:x",
    ];
    // A key that only LOOKS reserved is accepted: the rule is the prefix, nothing else.
    let accepted: [&str; 3] = ["awsome", "AWS:upper", "notaws:prefixed"];

    let emulator = Emulator::start();
    let arns = with_resources(&emulator).await;

    for key in reserved {
        for resource_arn in &arns {
            for operation in ["TagResource", "UntagResource"] {
                assert_failure(
                    &format!("{operation} {key:?} on {resource_arn}"),
                    emulator
                        .call(
                            operation,
                            json!({
                                "resourceArn": resource_arn,
                                "tags": tags_json(&[(key, "v")]),
                            }),
                        )
                        .await,
                    400,
                    "ValidationException",
                );
            }
        }
        // The same rule holds where a create supplies the tags.
        assert_failure(
            &format!("CreateKeyspace {key:?}"),
            emulator
                .call(
                    "CreateKeyspace",
                    json!({
                        "keyspaceName": "reserved_key_ks",
                        "tags": tags_json(&[(key, "v")]),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        assert_failure(
            &format!("CreateTable {key:?}"),
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": "reserved_key_tbl",
                        "schemaDefinition": minimal_schema("id"),
                        "tags": tags_json(&[(key, "v")]),
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    for key in accepted {
        for resource_arn in &arns {
            assert_ok(
                &format!("TagResource {key:?} on {resource_arn}"),
                emulator
                    .call(
                        "TagResource",
                        json!({
                            "resourceArn": resource_arn,
                            "tags": tags_json(&[(key, "v")]),
                        }),
                    )
                    .await,
            );
        }
    }
    for resource_arn in &arns {
        let reported: BTreeMap<String, String> =
            list_tags("accepted keys", &emulator, resource_arn, None)
                .await
                .into_iter()
                .collect();
        assert_eq!(
            reported,
            expected_set(&accepted.map(|key| (key, "v"))),
            "case {resource_arn:?}: expected only the accepted keys, got {reported:?}"
        );
    }
}

/// RULE 5: an ARN that names no stored resource is the modelled `ResourceNotFoundException`
/// — never a silently created tag set and never the scaffold's 501.
///
/// The rows span the ways an ARN can fail to name a stored resource: an unknown name at
/// either level, a resource form this service does not mint, and an ARN for another
/// Region or another partition, which names a resource this store does not hold.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_the_modelled_not_found() {
    let emulator = Emulator::start();
    let account = engine::emulator_account_id();
    with_resources(&emulator).await;

    let unknown: Vec<(&str, String)> = vec![
        ("an unknown keyspace", keyspace_arn("no_such_ks")),
        (
            "an unknown table in a keyspace that exists",
            table_arn(KEYSPACE, "no_such_tbl"),
        ),
        (
            "a table in an unknown keyspace",
            table_arn("no_such_ks", TABLE),
        ),
        (
            "the keyspace that exists, in another Region",
            format!("arn:aws:cassandra:eu-central-1:{account}:/keyspace/{KEYSPACE}/"),
        ),
        (
            "the keyspace that exists, in another partition",
            format!("arn:aws-us-gov:cassandra:{REGION}:{account}:/keyspace/{KEYSPACE}/"),
        ),
        (
            "the keyspace that exists, under another account",
            format!("arn:aws:cassandra:{REGION}:111111111111:/keyspace/{KEYSPACE}/"),
        ),
        (
            "a resource form this service does not mint",
            format!("arn:aws:cassandra:{REGION}:{account}:/cluster/{KEYSPACE}/"),
        ),
        (
            "a keyspace ARN missing its trailing separator",
            format!("arn:aws:cassandra:{REGION}:{account}:/keyspace/{KEYSPACE}"),
        ),
    ];

    for (name, resource_arn) in &unknown {
        for operation in ["TagResource", "UntagResource"] {
            assert_failure(
                &format!("{operation} on {name}"),
                emulator
                    .call(
                        operation,
                        json!({
                            "resourceArn": resource_arn,
                            "tags": tags_json(&[("env", "prod")]),
                        }),
                    )
                    .await,
                400,
                "ResourceNotFoundException",
            );
        }
        let (status, body) = emulator
            .call(
                "ListTagsForResource",
                json!({"resourceArn": resource_arn}),
            )
            .await;
        assert_failure(
            &format!("ListTagsForResource on {name}"),
            (status, body.clone()),
            400,
            "ResourceNotFoundException",
        );
        assert_eq!(
            body["resourceArn"],
            json!(resource_arn),
            "case {name:?}: the not-found body must name the ARN that was looked for, got {body}"
        );
    }

    // A refused call stored nothing: the resources that DO exist still hold no tags.
    for resource_arn in [keyspace_arn(KEYSPACE), table_arn(KEYSPACE, TABLE)] {
        let reported = list_tags("untagged resource", &emulator, &resource_arn, None).await;
        assert!(
            reported.is_empty(),
            "case {resource_arn:?}: expected no tags, got {reported:?}"
        );
    }
}

/// RULE 6: `ListTagsForResource` pages a resource's tags in one total order, resuming
/// exactly where the previous page stopped, for every page size — and a token it did not
/// issue for this resource is refused rather than silently ignored.
#[tokio::test]
async fn listing_pages_the_tag_set_in_one_total_order_for_any_page_size() {
    let emulator = Emulator::start();
    let arns = with_resources(&emulator).await;
    // Keys deliberately not in sorted order, so a page boundary cannot come out right by
    // accident from insertion order alone.
    let held: Vec<(String, String)> = ["tier", "env", "owner", "team", "app", "costCenter", "zone"]
        .iter()
        .enumerate()
        .map(|(index, key)| (key.to_string(), format!("value{index}")))
        .collect();
    let expected: Vec<(String, String)> = {
        let mut sorted = held.clone();
        sorted.sort();
        sorted
    };

    for resource_arn in &arns {
        assert_ok(
            resource_arn,
            emulator
                .call(
                    "TagResource",
                    json!({"resourceArn": resource_arn, "tags": tags_json(&held)}),
                )
                .await,
        );

        // Every page size from one tag per page to more than the whole set, plus the
        // absent `maxResults` (the whole set), must report the same total order.
        for page_size in [None, Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), Some(1000)] {
            let case = format!("page size {page_size:?} on {resource_arn}");
            let reported = list_tags(&case, &emulator, resource_arn, page_size).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the tags in one total order {expected:?}, got {reported:?}"
            );
        }

        // One past each published bound of `maxResults` (range 1..1000) is refused by the
        // boundary validator the model drives.
        for page_size in [0, 1001] {
            assert_failure(
                &format!("maxResults {page_size} on {resource_arn}"),
                emulator
                    .call(
                        "ListTagsForResource",
                        json!({"resourceArn": resource_arn, "maxResults": page_size}),
                    )
                    .await,
                400,
                "ValidationException",
            );
        }
    }

    // A token is bound to the resource it was issued for: handing the keyspace's token to
    // the table (and the reverse), or a token this service never issued, is refused.
    let first_page = assert_ok(
        "token issue",
        emulator
            .call(
                "ListTagsForResource",
                json!({"resourceArn": &arns[0], "maxResults": 1}),
            )
            .await,
    );
    let issued = first_page["nextToken"]
        .as_str()
        .expect("a truncated page carries a nextToken")
        .to_string();
    for (name, resource_arn, token) in [
        ("another resource's token", arns[1].clone(), issued.clone()),
        ("a token this service never issued", arns[0].clone(), "6e6f70".to_string()),
        ("a token that is not even hex", arns[0].clone(), "not-a-token".to_string()),
    ] {
        assert_failure(
            name,
            emulator
                .call(
                    "ListTagsForResource",
                    json!({"resourceArn": resource_arn, "nextToken": token}),
                )
                .await,
            400,
            "ValidationException",
        );
    }
}
