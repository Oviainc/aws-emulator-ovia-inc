// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the CHARACTER SET a tag of this service may use, over every
//! operation that writes one: `TagResource`, `StartSearchJob` and
//! `StartSearchResultExportJob`.
//!
//! The rule under test: a tag key or value this service stores may hold only UTF-8
//! letters, numbers, spaces and the punctuation its `Tags` member documents
//! (`state::TAG_PUNCTUATION`); a write carrying any other character is refused 400
//! `ValidationException` and persists nothing, so no later read reports the entry and no
//! resource is minted carrying it. The model puts no `@pattern` on `TagMap`, so the
//! boundary validator cannot enforce that set and the service does.
//!
//! So every assertion is made over a TABLE spanning the class rather than the one request
//! the issue cited: the rows differ in which character is used, whether it sits in the key
//! or in the value, where in the string it sits, which script the allowed letters and
//! digits come from, and whether the character is one byte or several. Each documented
//! punctuation character gets its own accepted row, read off the catalog in `state.rs`
//! rather than retyped here.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `percent_encode`, `Answer`) is the
//! search-job suite's and the exportable search job is seeded with the export-listing
//! suite's `seed_job`, so this suite drives the same emulator the same way the others do.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{passable_role, seed_job};
use crate::search_jobs::{call, emulator_with_engine, percent_encode, Answer};

/// The region every call in this suite is signed for. `seed_job` mints its ARN for this
/// same region, so a seeded job and a called-for job are addressed alike.
const REGION: &str = "us-west-2";

/// The `SearchJobIdentifier` of the seeded, exportable search job the export rows name.
const EXPORTABLE_JOB: &str = "exportable-job";

/// Characters OUTSIDE the set `Tags` documents, one row of the table each.
///
/// Source: the complement of the documented set — anything that is not a UTF-8 letter, a
/// number, a space or one of `state::TAG_PUNCTUATION`. The list walks the punctuation a
/// caller plausibly reaches for (`*`, `#`, `@`, `,`, the quotes and brackets), the
/// whitespace that is not a space (tab, newline), and characters that are several bytes
/// wide (an em dash, a copyright sign, an emoji) so the check cannot be a byte scan that
/// mistakes a multi-byte LETTER for a disallowed character.
const DISALLOWED_CHARACTERS: &[char] = &[
    '*', '#', '@', ',', '%', '!', '?', '&', ';', '"', '\'', '\\', '(', ')', '[', ']', '{', '}',
    '<', '>', '|', '^', '~', '`', '$', '\t', '\n', '—', '©', '🙂',
];

/// One row: the tag entry a write carries, and whether this service must accept it.
struct TagCase {
    case: String,
    key: String,
    value: String,
    allowed: bool,
}

fn allowed(case: impl Into<String>, key: impl Into<String>, value: impl Into<String>) -> TagCase {
    TagCase {
        case: case.into(),
        key: key.into(),
        value: value.into(),
        allowed: true,
    }
}

fn refused(case: impl Into<String>, key: impl Into<String>, value: impl Into<String>) -> TagCase {
    TagCase {
        case: case.into(),
        key: key.into(),
        value: value.into(),
        allowed: false,
    }
}

fn tag_cases() -> Vec<TagCase> {
    let mut cases = vec![
        allowed("plain ASCII letters and digits", "env1", "prod2"),
        allowed("spaces in the key and in the value", "two words", "a b c"),
        allowed(
            "every documented punctuation character at once",
            "a+b-c=d.e_f:g/h",
            "1+2-3=4.5_6:7/8",
        ),
        // "UTF-8 letters, numbers" is not "ASCII letters, ASCII digits": a tag written in
        // another script is inside the documented set.
        allowed("Cyrillic letters", "ключ", "значение"),
        allowed("Japanese letters", "日本語", "テスト"),
        allowed("Arabic-Indic digits", "٣٤", "٥٦"),
        allowed("a key that is only punctuation and spaces", "a : b / c", "-"),
    ];

    // Each character the catalog in `state.rs` lists, on its own, in the key and in the
    // value — so the accepted set is exercised entry by entry rather than only through
    // the two characters the issue happened to cite.
    for punctuation in state::TAG_PUNCTUATION {
        cases.push(allowed(
            format!("documented punctuation {punctuation:?} on its own"),
            format!("a{punctuation}b"),
            format!("v{punctuation}w"),
        ));
    }

    // Each disallowed character, refused in the key and refused in the value.
    for character in DISALLOWED_CHARACTERS {
        cases.push(refused(
            format!("{character:?} in the key"),
            format!("bad{character}key"),
            "v",
        ));
        cases.push(refused(
            format!("{character:?} in the value"),
            "k",
            format!("bad{character}value"),
        ));
    }

    // Where in the string the character sits makes no difference either.
    cases.push(refused("a disallowed character first", "*key", "v"));
    cases.push(refused("a disallowed character last", "key*", "v"));
    cases.push(refused(
        "a disallowed character in an otherwise legal key",
        "cost:center*1",
        "cc-1",
    ));
    cases.push(refused(
        "a disallowed character in an otherwise legal value",
        "cost:center",
        "cc/1,cc/2",
    ));
    cases
}

/// The tag entry one row writes, as the `Tags` member of a request body.
fn tag_map(case: &TagCase) -> Value {
    let mut entry = serde_json::Map::new();
    entry.insert(case.key.clone(), Value::String(case.value.clone()));
    Value::Object(entry)
}

/// A JSON object of string values as a sorted map, so a comparison does not depend on map
/// ordering.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The tags `ListTagsForResource` reports for `arn` — read back through the API, so a
/// refused write is checked against what a later caller can actually see.
async fn stored_tags(
    emulator: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let listed = call(
        emulator,
        REGION,
        "GET",
        &format!("/tags/{}", percent_encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource for {arn:?} must be 200, got {} ({:?})",
        listed.status, listed.body
    );
    as_map(listed.get("Tags"))
}

/// Start an untagged search job and answer its ARN, so each row writes into a resource of
/// its own.
async fn untagged_search_job(emulator: &MultiServiceRouter, case: &str) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed to set the row up, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobArn")
}

/// Assert `answer` is the refusal the rule calls for: 400 `ValidationException`, naming
/// the member whose value was rejected so the caller can fix the request from the text.
fn assert_refused(answer: &Answer, case: &str, operation: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?} at {operation}: a tag outside the documented character set must be \
         refused 400, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?} at {operation}: the refusal must be a ValidationException, got {:?}",
        answer.error_code
    );
    let message = answer.string("message");
    assert!(
        message.contains("Tags"),
        "case {case:?} at {operation}: the refusal must name the member it refused, got \
         {message:?}"
    );
}

/// How many search jobs the store holds — what proves a refused create minted nothing.
fn stored_search_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>()
        .lock()
        .search_jobs
        .len()
}

/// How many export jobs the store holds, for the same reason.
fn stored_export_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>()
        .lock()
        .export_jobs
        .len()
}

#[tokio::test]
async fn tag_resource_stores_a_documented_tag_and_refuses_one_outside_the_documented_set() {
    let (emulator, _engine) = emulator_with_engine();
    for case in tag_cases() {
        let arn = untagged_search_job(&emulator, &case.case).await;
        let answer = call(
            &emulator,
            REGION,
            "POST",
            &format!("/tags/{}", percent_encode(&arn)),
            Some(json!({"Tags": tag_map(&case)})),
        )
        .await;
        let reported = stored_tags(&emulator, &case.case, &arn).await;
        if case.allowed {
            assert_eq!(
                answer.status,
                state::NO_CONTENT_SUCCESS_STATUS,
                "case {:?} at TagResource: a tag inside the documented character set must be \
                 stored, got {} ({:?})",
                case.case,
                answer.status,
                answer.body
            );
            assert_eq!(
                reported.get(&case.key).map(String::as_str),
                Some(case.value.as_str()),
                "case {:?} at TagResource: the stored entry must read back, got {reported:?}",
                case.case
            );
        } else {
            assert_refused(&answer, &case.case, "TagResource");
            assert!(
                reported.is_empty(),
                "case {:?} at TagResource: a refused write must leave the resource untagged, got \
                 {reported:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn start_search_job_mints_no_job_carrying_a_tag_outside_the_documented_set() {
    let (emulator, engine) = emulator_with_engine();
    let mut expected_jobs = 0usize;
    for case in tag_cases() {
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({
                "Name": "tagged",
                "SearchScope": {"BackupResourceTypes": ["S3"]},
                "Tags": tag_map(&case),
            })),
        )
        .await;
        if case.allowed {
            assert_eq!(
                started.status, 200,
                "case {:?} at StartSearchJob: a tag inside the documented character set must be \
                 accepted, got {} ({:?})",
                case.case, started.status, started.body
            );
            expected_jobs += 1;
            let arn = started.string("SearchJobArn");
            let reported = stored_tags(&emulator, &case.case, &arn).await;
            assert_eq!(
                reported.get(&case.key).map(String::as_str),
                Some(case.value.as_str()),
                "case {:?} at StartSearchJob: the minted job must carry the tag it was created \
                 with, got {reported:?}",
                case.case
            );
        } else {
            assert_refused(&started, &case.case, "StartSearchJob");
        }
        assert_eq!(
            stored_search_jobs(&engine),
            expected_jobs,
            "case {:?} at StartSearchJob: a refused create must mint no search job",
            case.case
        );
    }
}

#[tokio::test]
async fn start_search_result_export_job_mints_no_job_carrying_a_tag_outside_the_documented_set() {
    let (emulator, engine) = emulator_with_engine();
    // One exportable search job for every row: what varies here is the tag set, not how
    // the search job reached a status an export may be started from.
    seed_job(&engine, EXPORTABLE_JOB, state::COMPLETED, Vec::new());
    let mut expected_exports = 0usize;
    for case in tag_cases() {
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": EXPORTABLE_JOB,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "search-exports"}
                },
                "RoleArn": passable_role("backup-search-export"),
                "Tags": tag_map(&case),
            })),
        )
        .await;
        if case.allowed {
            assert_eq!(
                started.status, 200,
                "case {:?} at StartSearchResultExportJob: a tag inside the documented character \
                 set must be accepted, got {} ({:?})",
                case.case, started.status, started.body
            );
            expected_exports += 1;
            let arn = started.string("ExportJobArn");
            let reported = stored_tags(&emulator, &case.case, &arn).await;
            assert_eq!(
                reported.get(&case.key).map(String::as_str),
                Some(case.value.as_str()),
                "case {:?} at StartSearchResultExportJob: the minted export job must carry the \
                 tag it was created with, got {reported:?}",
                case.case
            );
        } else {
            assert_refused(&started, &case.case, "StartSearchResultExportJob");
        }
        assert_eq!(
            stored_export_jobs(&engine),
            expected_exports,
            "case {:?} at StartSearchResultExportJob: a refused create must mint no export job",
            case.case
        );
    }
}

#[tokio::test]
async fn the_character_set_a_tag_may_use_is_the_one_the_model_documents() {
    // The catalog's own invariant, over the same rows: the shared helper every write path
    // goes through accepts exactly the documented set, and reports the FIRST character
    // outside it so the refusal can name it.
    for case in tag_cases() {
        let checked = state::check_tag_entry(&case.key, &case.value);
        assert_eq!(
            checked.is_ok(),
            case.allowed,
            "case {:?}: expected accepted={}, got {checked:?}",
            case.case,
            case.allowed
        );
        let offending = state::disallowed_tag_character(&case.key)
            .or_else(|| state::disallowed_tag_character(&case.value));
        assert_eq!(
            offending.is_none(),
            case.allowed,
            "case {:?}: expected a character outside the set to be reported for a refused row, \
             got {offending:?}",
            case.case
        );
    }
    for punctuation in state::TAG_PUNCTUATION {
        assert!(
            state::tag_character_allowed(*punctuation),
            "case {punctuation:?}: a character the model documents must be allowed"
        );
    }
    for character in DISALLOWED_CHARACTERS {
        assert!(
            !state::tag_character_allowed(*character),
            "case {character:?}: a character outside the documented set must be refused"
        );
    }
}
