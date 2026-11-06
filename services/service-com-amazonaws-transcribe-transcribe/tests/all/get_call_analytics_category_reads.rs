// SPDX-License-Identifier: CC0-1.0

//! What `GetCallAnalyticsCategory` ANSWERS over the wire, beyond the create -> read ->
//! delete walk `crate::call_analytics_categories` already drives.
//!
//! THE RULES THESE ROWS ASSERT:
//!
//! 1. The operation is SERVED by a handler that reports this service's persisted state:
//!    every answer is one of the operation's own modelled answers — the
//!    `CategoryProperties` of the category stored under the requested name, or one of
//!    this service's modelled client-error refusals — and never the framework's 501
//!    `NotImplemented`, which is the answer of an operation no handler implements.
//! 2. The name identifies the record exactly: each stored name reads back the rules,
//!    input type and timestamps persisted UNDER THAT NAME, so categories stored side by
//!    side are never confused for one another, and a name that is only a near match
//!    (another casing, a shorter spelling) reports no record at all.
//! 3. `CategoryName` outside its modelled constraints is refused by the boundary
//!    validator before the handler runs (`@length(min: 1, max: 200)`,
//!    `@pattern(^[0-9a-zA-Z._-]+$)` —
//!    `smithy_query(query="operation", operation="GetCallAnalyticsCategory")`), while a
//!    name the Region simply holds no category under is this family's not-found
//!    refusal, whose text names the KIND of resource and echoes no request value.
//! 4. The read REPORTS: neither a found read nor a refused one stores or removes
//!    anything, so the family's listing and every stored category read exactly the same
//!    after the whole table has run.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) and the lifecycle suite's rule builders
//! (`crate::call_analytics_categories`) rather than standing up a second emulator, so
//! these rows exercise the awsJson1_1 envelope, the boundary validator and the
//! handler's own refusals.

use serde_json::{json, Value};

use crate::call_analytics_categories::{non_talk_time_rule, sentiment_rule, transcript_rule};
use crate::call_analytics_rule_negate::as_recorded;
use crate::transcription_jobs::{Reply, Transcribe};

/// The status this service's awsJson1_1 wire protocol puts on every client error.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The modelled client error this family answers for a category the Region holds no
/// record of: the shape whose own documentation covers an entity that "doesn't exist".
const ABSENT_CODE: &str = "BadRequestException";

/// The error code the boundary validator answers for an input outside a modelled
/// constraint, before any handler runs.
const VALIDATION_CODE: &str = "ValidationException";

/// The framework's answer for an operation no handler implements — what this read must
/// never be.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";
/// See [`NOT_IMPLEMENTED_CODE`].
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The word this service's own client-error texts name this family by.
const CATEGORY_KIND: &str = "category";

/// RULE 1: the answer is the operation's own, never the 501 of an unimplemented
/// operation. Asserted on EVERY call this suite makes, successful or refused.
fn served(reply: &Reply, case: &str) {
    assert_ne!(
        reply.status, NOT_IMPLEMENTED_STATUS,
        "case {case:?}: the read must be served by a handler, not answered \
         {NOT_IMPLEMENTED_STATUS}, got body {:?}",
        reply.body
    );
    assert_ne!(
        reply.error_code(),
        NOT_IMPLEMENTED_CODE,
        "case {case:?}: the read must be served by a handler, not answered \
         {NOT_IMPLEMENTED_CODE:?}, got body {:?}",
        reply.body
    );
}

/// Read one category back, asserting RULE 1 on the way out.
async fn get(transcribe: &Transcribe, case: &str, name: &str) -> Reply {
    let reply = transcribe
        .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
        .await;
    served(&reply, case);
    reply
}

/// The `CategoryProperties` of a successful read.
fn properties(reply: &Reply, case: &str) -> Value {
    reply
        .ok(case)
        .get("CategoryProperties")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CategoryProperties in the answer, got {:?}",
                reply.body
            )
        })
}

/// One category this suite stores, and the rules it must read back.
struct Stored {
    case: &'static str,
    name: String,
    rules: Value,
    input_type: &'static str,
}

/// The categories stored before the table runs, one row per form `CategoryName`'s own
/// `@length(min: 1, max: 200)` / `@pattern(^[0-9a-zA-Z._-]+$)` admits: the shortest
/// name the length allows, a name using every punctuation character the pattern allows
/// alongside digits and both cases, and a name at the 200-character ceiling. Each row's
/// rules are distinct, so a read reporting another row's record cannot pass.
fn stored() -> Vec<Stored> {
    vec![
        Stored {
            case: "the shortest name the length allows",
            name: "a".to_string(),
            rules: json!([non_talk_time_rule(10_000)]),
            input_type: "POST_CALL",
        },
        Stored {
            case: "every punctuation character the pattern allows",
            name: "Reads.v2_final-1".to_string(),
            rules: json!([transcript_rule("please hold"), sentiment_rule("NEGATIVE")]),
            input_type: "REAL_TIME",
        },
        Stored {
            case: "a name at the 200-character ceiling",
            name: "r".repeat(200),
            rules: json!([sentiment_rule("POSITIVE")]),
            input_type: "POST_CALL",
        },
    ]
}

/// Store every row of [`stored`] and answer the properties each one reads back with.
async fn create_all(transcribe: &Transcribe, rows: &[Stored]) -> Vec<Value> {
    let mut created = Vec::new();
    for row in rows {
        let reply = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({
                    "CategoryName": row.name,
                    "Rules": row.rules,
                    "InputType": row.input_type
                }),
            )
            .await;
        created.push(properties(&reply, row.case));
    }
    created
}

/// Assert `found` is the record stored for `row`.
fn describes(found: &Value, row: &Stored) {
    let case = row.case;
    assert_eq!(
        found.get("CategoryName"),
        Some(&json!(row.name)),
        "case {case:?}: the read must report the name it was asked for, got {found:?}"
    );
    assert_eq!(
        found.get("Rules"),
        Some(&as_recorded(&row.rules)),
        "case {case:?}: the read must report the rules stored under this name, as the \
         service records them, got {found:?}"
    );
    assert_eq!(
        found.get("InputType"),
        Some(&json!(row.input_type)),
        "case {case:?}: expected InputType {:?}, got {found:?}",
        row.input_type
    );
    let create_time = found.get("CreateTime").and_then(Value::as_f64);
    assert!(
        create_time.is_some_and(|time| time > 0.0),
        "case {case:?}: expected a CreateTime, got {found:?}"
    );
}

/// The names the Region holds no category under, each a near match of a stored one: a
/// casing no create stored ("Category names are case sensitive", model doc for
/// `GetCallAnalyticsCategoryRequest$CategoryName`), a spelling one character short of a
/// stored name, the stored ceiling name one character shorter, and a plainly absent
/// name.
fn absent_names(rows: &[Stored]) -> Vec<(String, String)> {
    vec![
        (
            "a casing no create stored".to_string(),
            rows[0].name.to_uppercase(),
        ),
        (
            "a stored name with its last character dropped".to_string(),
            rows[1].name[..rows[1].name.len() - 1].to_string(),
        ),
        (
            "one character short of the ceiling name".to_string(),
            "r".repeat(199),
        ),
        (
            "a name nothing ever created".to_string(),
            "never-created-at-all".to_string(),
        ),
    ]
}

/// The names outside `CategoryName`'s modelled constraints, one row per bound and per
/// kind of character the pattern excludes.
fn invalid_names() -> Vec<(&'static str, String)> {
    vec![
        ("the empty name, below the length floor", String::new()),
        (
            "one character past the length ceiling",
            "n".repeat(201),
        ),
        ("a name carrying a space", "two words".to_string()),
        ("a name carrying a slash", "team/category".to_string()),
        ("a name carrying a star", "cat*".to_string()),
        ("a name carrying a colon", "ns:category".to_string()),
        ("a name outside ASCII", "catégorie".to_string()),
    ]
}

/// Every `CategoryProperties` the family's listing reports, by name.
async fn listed(transcribe: &Transcribe, case: &str) -> Vec<(String, Value)> {
    let reply = transcribe
        .call("ListCallAnalyticsCategories", json!({"MaxResults": 100}))
        .await;
    let body = reply.ok(case);
    let categories = body
        .get("Categories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut entries: Vec<(String, Value)> = categories
        .into_iter()
        .map(|properties| {
            let name = properties["CategoryName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a CategoryName, got {properties:?}")
                })
                .to_string();
            (name, properties)
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// RULES 1, 2 and 4: a stored category is reported from state under its exact name, and
/// reading it is reporting — the record, and the family's listing, are the same after
/// the whole table of found, absent and invalid reads has run.
#[tokio::test]
async fn the_read_reports_the_record_stored_under_the_requested_name() {
    let transcribe = Transcribe::new();
    let rows = stored();
    let created = create_all(&transcribe, &rows).await;

    let before = listed(&transcribe, "after the creates").await;
    assert_eq!(
        before.len(),
        rows.len(),
        "the store must hold exactly the categories created, got {:?}",
        before.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    // RULE 2: each name reports its own record, which is what the create answered.
    let mut first_reads = Vec::new();
    for (row, created) in rows.iter().zip(&created) {
        let found = properties(&get(&transcribe, row.case, &row.name).await, row.case);
        describes(&found, row);
        assert_eq!(
            &found, created,
            "case {:?}: the read must report what the create stored",
            row.case
        );
        first_reads.push(found);
    }

    // The refusals of RULE 3 run between the two rounds of reads, so RULE 4 covers them
    // too: a refused read must leave the store exactly as it was.
    for (name_case, name) in absent_names(&rows) {
        let case = format!("absent: {name_case}");
        get(&transcribe, &case, &name)
            .await
            .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }
    for (name_case, name) in invalid_names() {
        let case = format!("invalid: {name_case}");
        get(&transcribe, &case, &name)
            .await
            .refused(&case, CLIENT_ERROR_STATUS, VALIDATION_CODE);
    }

    // RULE 4: every stored category reads the same as it did, timestamps included, and
    // the listing agrees with the reader.
    for (row, first) in rows.iter().zip(&first_reads) {
        let case = format!("{} read a second time", row.case);
        let again = properties(&get(&transcribe, &case, &row.name).await, &case);
        assert_eq!(
            &again, first,
            "case {case:?}: reading a category must not change it"
        );
    }
    let after = listed(&transcribe, "after the whole table").await;
    assert_eq!(
        after, before,
        "the reads must have stored and removed nothing, got {:?}",
        after.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}

/// RULE 3, the WORDING half: the not-found refusal's text is this service's own — it
/// names the KIND of resource and echoes no value from the request. Which status and
/// which error code each kind of refusal carries is owned by the test above; this one
/// asserts only the message.
#[tokio::test]
async fn the_not_found_refusal_names_the_kind_and_echoes_no_request_value() {
    let transcribe = Transcribe::new();
    let rows = stored();
    create_all(&transcribe, &rows).await;

    for (name_case, name) in absent_names(&rows) {
        let case = format!("absent: {name_case}");
        let refused = get(&transcribe, &case, &name).await;

        let message = refused.error_message(&case);
        assert!(
            message.contains(CATEGORY_KIND),
            "case {case:?}: the refusal must name the {CATEGORY_KIND:?} kind, got \
             {message:?}"
        );
        // Asserted on the names long enough for the check to mean something: a
        // one-character name is a substring of most English sentences.
        if name.len() > 3 {
            assert!(
                !message.contains(&name),
                "case {case:?}: the refusal must not echo the requested name, got \
                 {message:?}"
            );
        }
    }
}
