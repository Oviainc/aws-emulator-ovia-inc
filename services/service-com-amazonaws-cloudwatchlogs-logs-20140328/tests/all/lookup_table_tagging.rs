// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lookup table as a TAGGABLE resource of this service, and for the
//! refusal a create under a name the account already holds answers.
//!
//! The rules under test:
//!
//! * a lookup table carries the tag set `CreateLookupTable$tags` assigns ("An optional list of
//!   key-value pairs to associate with the resource"), and the tagging operations reach it
//!   under the table's own `lookupTableArn` like the tag set of every other resource of this
//!   service whose model shape carries `tags`: `ListTagsForResource` reports the stored set,
//!   `TagResource` merges into it and `UntagResource` takes keys off it, while a
//!   lookup-table ARN the account holds no table under is refused as a missing resource;
//! * a create under a name the account already holds a table under is refused
//!   `ResourceAlreadyExistsException`, and the sentence states WHICH table collided and in
//!   WHICH account — the mirror of the refusal a table the account does NOT hold answers.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and the tags a create
//! assigned are read back through another operation.
//!
//! The tables span what the rules generalise over rather than one request: table names of
//! every form the model's `LookupTableName` admits (`@length` 1..256, `@pattern
//! ^[a-zA-Z0-9_]+$` — so a single character, digits, underscores, mixed case and a name at
//! the published maximum), tag sets of zero, one and several entries, and creates that
//! collide by the whole name as well as near-miss spellings that do not collide at all.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::lookup_table_arn;

use crate::harness::{Logs, REGION};

/// The operations under test.
const CREATE: &str = "CreateLookupTable";
const LIST_TAGS: &str = "ListTagsForResource";
const TAG: &str = "TagResource";
const UNTAG: &str = "UntagResource";

/// The refusals these operations model for the cases below.
const ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A lookup table body: a header row naming the columns and one data row, which is the
/// smallest content `CreateLookupTable$tableBody` is read from.
const TABLE_BODY: &str = "user,team\nalice,platform\n";

/// The published maximum length of `LookupTableName`. Source: the model's `LookupTableName`
/// shape (`@length` 1..256, `@pattern ^[a-zA-Z0-9_]+$`), which
/// `CreateLookupTableRequest$lookupTableName` targets.
const NAME_MAX_LENGTH: usize = 256;

/// One table to create: its name and the tag set the create assigns to it.
struct Row {
    /// What this row varies, named in every assertion.
    label: &'static str,
    name: String,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
}

/// The tables the tagging cases run over: names of every form the model admits, and tag sets
/// of every size.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a lower-case name, two tags",
            name: "users".to_string(),
            tags: vec![("env", "probe"), ("purpose", "lookup")],
        },
        Row {
            label: "a name at the minimum length, one tag",
            name: "u".to_string(),
            tags: vec![("env", "probe")],
        },
        Row {
            label: "digits and underscores, no tags at all",
            name: "accounts_2026_eu".to_string(),
            tags: Vec::new(),
        },
        Row {
            label: "mixed case, several tags",
            name: "Users_EU_2".to_string(),
            tags: vec![("env", "prod"), ("owner", "obs"), ("tier", "gold")],
        },
        Row {
            label: "a name at the published maximum length, one tag",
            name: "n".repeat(NAME_MAX_LENGTH),
            tags: vec![("env", "probe")],
        },
    ]
}

impl Row {
    /// The `CreateLookupTable` request that creates this table.
    fn create_body(&self) -> Value {
        let mut body = json!({"lookupTableName": self.name, "tableBody": TABLE_BODY});
        if !self.tags.is_empty() {
            let map: Map<String, Value> = self
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            body["tags"] = Value::Object(map);
        }
        body
    }

    /// The tag set the create assigned, as a key-ordered map.
    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The tag set the service reports for one resource, as a key-ordered map.
async fn reported_tags(logs: &Logs, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let response = logs.ok(LIST_TAGS, &json!({"resourceArn": resource_arn})).await;
    let tags = response
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: {LIST_TAGS} reports a tag set, got {response}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: tags is a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: tag {key} holds a string, got {value}"));
            (key.clone(), value.to_string())
        })
        .collect()
}

/// The tags a create assigned to a lookup table are read back under the table's own ARN, and
/// the tagging writes reach the same one set.
#[tokio::test]
async fn a_lookup_tables_tags_are_read_and_written_under_its_arn() {
    let logs = Logs::new();
    let rows = rows();

    for row in &rows {
        logs.ok(CREATE, &row.create_body()).await;
    }

    for row in &rows {
        let arn = lookup_table_arn(REGION, &row.name);
        let reported = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            reported,
            row.expected_tags(),
            "case {:?}: expected the tags the create assigned {:?}, got {reported:?}",
            row.label,
            row.expected_tags()
        );

        // A read changes nothing a later caller sees.
        let again = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            again, reported,
            "case {:?}: a second read reports the same tag set, got {again:?}",
            row.label
        );
    }

    // The tagging writes reach the SAME set: a merge adds a key and replaces one, and an
    // untag takes a key off — read back through the reporting operation.
    for row in &rows {
        let arn = lookup_table_arn(REGION, &row.name);
        logs.ok(
            TAG,
            &json!({"resourceArn": arn, "tags": {"env": "staging", "added": "yes"}}),
        )
        .await;
        let mut expected = row.expected_tags();
        expected.insert("env".to_string(), "staging".to_string());
        expected.insert("added".to_string(), "yes".to_string());
        let reported = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the merged tag set {expected:?}, got {reported:?}",
            row.label
        );

        logs.ok(UNTAG, &json!({"resourceArn": arn, "tagKeys": ["added"]}))
            .await;
        expected.remove("added");
        let reported = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the untagged tag set {expected:?}, got {reported:?}",
            row.label
        );
    }

    // One table's tags are not another's: every row still reports its own set.
    for row in &rows {
        let arn = lookup_table_arn(REGION, &row.name);
        let mut expected = row.expected_tags();
        expected.insert("env".to_string(), "staging".to_string());
        let reported = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            reported, expected,
            "case {:?}: after every other write, expected {expected:?}, got {reported:?}",
            row.label
        );
    }
}

/// A lookup-table ARN the account holds no table under names a resource that is not there, so
/// the tag report is refused rather than answered with an empty set.
#[tokio::test]
async fn a_lookup_table_arn_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    logs.ok(
        CREATE,
        &json!({"lookupTableName": "held", "tableBody": TABLE_BODY}),
    )
    .await;

    // Names that satisfy `LookupTableName` and that the account holds no table under: one
    // never used, one differing from the held name only in case, and one at each published
    // length bound.
    let absent = [
        ("a name never used", "absent".to_string()),
        ("the held name in another case", "HELD".to_string()),
        ("a one-character name", "x".to_string()),
        ("a name at the maximum length", "z".repeat(NAME_MAX_LENGTH)),
    ];

    for (case, name) in &absent {
        let arn = lookup_table_arn(REGION, name);
        let (code, message) = logs
            .refusal(LIST_TAGS, &json!({"resourceArn": arn}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message, "The specified lookup table does not exist.",
            "case {case:?}: the refusal names the resource kind that is missing, got {message:?}"
        );
    }

    // The held table is still reported, so the refusals above read the same store.
    let reported = reported_tags(&logs, "the held table", &lookup_table_arn(REGION, "held")).await;
    assert!(
        reported.is_empty(),
        "case \"the held table\": a table created with no tags carries none, got {reported:?}"
    );
}

/// A create under a name the account already holds a table under is refused, and the sentence
/// states that table's name and the account it is already held in — while a near-miss
/// spelling collides with nothing and is created.
#[tokio::test]
async fn a_duplicate_create_names_the_table_and_the_account() {
    let logs = Logs::new();
    let rows = rows();
    let account = engine::emulator_account_id();

    for row in &rows {
        logs.ok(CREATE, &row.create_body()).await;
    }

    for row in &rows {
        // The same name again, with and without the members the first create named: the
        // collision is the NAME, so neither changes the refusal.
        let bodies = [
            ("the same request again", row.create_body()),
            (
                "the same name with other members",
                json!({
                    "lookupTableName": row.name,
                    "tableBody": "metric\n",
                    "description": "a second table under one name",
                }),
            ),
        ];
        for (variant, body) in bodies {
            let case = (row.label, variant);
            let (code, message) = logs.refusal(CREATE, &body).await;
            assert_eq!(
                code, ALREADY_EXISTS,
                "case {case:?}: expected {ALREADY_EXISTS}, got {code} {message:?}"
            );
            let expected = format!(
                "Lookup table '{}' already exists in account '{account}'",
                row.name
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // A refused create left the stored table exactly as it was: its tags, and so its record,
    // are the ones the accepted create stored.
    for row in &rows {
        let arn = lookup_table_arn(REGION, &row.name);
        let reported = reported_tags(&logs, row.label, &arn).await;
        assert_eq!(
            reported,
            row.expected_tags(),
            "case {:?}: a refused create changes nothing, expected {:?}, got {reported:?}",
            row.label,
            row.expected_tags()
        );
    }

    // Near-miss spellings of a held name collide with nothing: each is created under its own
    // name.
    let held = &rows[0].name;
    for (case, name) in [
        ("the held name upper-cased", held.to_uppercase()),
        ("the held name with a suffix", format!("{held}_2")),
        ("one character short", held[..held.len() - 1].to_string()),
    ] {
        let created = logs
            .ok(
                CREATE,
                &json!({"lookupTableName": name, "tableBody": TABLE_BODY}),
            )
            .await;
        let expected = lookup_table_arn(REGION, &name);
        assert_eq!(
            created.get("lookupTableArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: a name no table is held under is created, got {created}"
        );
    }
}
