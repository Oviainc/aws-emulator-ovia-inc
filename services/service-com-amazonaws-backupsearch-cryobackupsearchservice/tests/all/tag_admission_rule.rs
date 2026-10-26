// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE admission rule every write of a tag in this service goes
//! through, over each operation that writes one: `StartSearchJob`,
//! `StartSearchResultExportJob` and `TagResource`.
//!
//! The rule under test: a submitted tag set is admitted only when every entry it carries
//! is an admissible entry AND the number of tags the record would hold once the write
//! landed is within the per-resource maximum; otherwise the write is refused 400
//! `ValidationException` and persists nothing. The three operations differ only in the
//! count they check — a create merges into nothing, so it checks what the request carries,
//! while a merge checks what the record would hold afterwards, which grows only by the
//! submitted keys the record does not already hold.
//!
//! So the table below spans the rule rather than one request: its rows differ in which
//! half of the rule the set trips (per-entry versus count), which member carries the
//! offending value (key or value), the kind of offence (length versus character set), the
//! size of the set, and whether the set is empty — each row at a cap's bound and one past
//! it. Every row is driven through all three write paths and asserted to answer the SAME
//! status, the SAME error code and the SAME refusal text, which is what one shared rule
//! means and what three copies of it could not guarantee. A second table varies the merge
//! dimension the creates cannot reach: how many tags the record already holds, how many
//! new keys the write adds, and how many submitted keys only overwrite a stored entry.
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

/// The region every call in this suite is signed for, matching the region `seed_job`
/// mints its ARNs for.
const REGION: &str = "us-west-2";

/// One row of the admission table: the tag set a write submits, and whether this service
/// must admit it.
struct SetCase {
    case: String,
    tags: Value,
    admitted: bool,
}

fn set_case(case: impl Into<String>, tags: Value, admitted: bool) -> SetCase {
    SetCase {
        case: case.into(),
        tags,
        admitted,
    }
}

/// `count` distinct entries, for the rows that vary the SIZE of the submitted set.
fn entries(count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("size-key-{index}"), json!("v"));
    }
    Value::Object(tags)
}

/// A one-entry set, for the rows that vary one key or one value.
fn entry(key: impl Into<String>, value: impl Into<String>) -> Value {
    let mut tags = serde_json::Map::new();
    tags.insert(key.into(), Value::String(value.into()));
    Value::Object(tags)
}

/// The rows of the admission table.
///
/// The caps and the documented character set are read off `state.rs` rather than retyped,
/// so a row sits at whatever bound this service enforces.
fn set_cases() -> Vec<SetCase> {
    let max_tags = state::MAX_TAGS_PER_JOB;
    let max_key = state::MAX_TAG_KEY_LENGTH;
    let max_value = state::MAX_TAG_VALUE_LENGTH;
    // A character outside the documented set, taken as the complement of the catalog in
    // `state.rs` rather than assumed: whatever the catalog holds, this is not in it.
    let outside = ('!'..='~')
        .find(|character| !state::tag_character_allowed(*character))
        .expect("some printable character is outside the documented tag character set");

    let mut cases = vec![
        // The empty set: nothing to check per entry, and a count of nothing.
        set_case("no tags at all", json!({}), true),
        set_case("one ordinary entry", entry("env", "prod"), true),
        // The count half of the rule, at its bound and one past it.
        set_case(
            format!("{max_tags} entries, at the per-resource maximum"),
            entries(max_tags),
            true,
        ),
        set_case(
            format!("{} entries, one past the maximum", max_tags + 1),
            entries(max_tags + 1),
            false,
        ),
        set_case(
            format!("{} entries, well past the maximum", max_tags * 2),
            entries(max_tags * 2),
            false,
        ),
        // The per-entry half, on the key: a length bound and a character outside the set.
        set_case(
            format!("a {max_key}-character key, at the cap"),
            entry("k".repeat(max_key), "v"),
            true,
        ),
        set_case(
            format!("a {}-character key, one past the cap", max_key + 1),
            entry("k".repeat(max_key + 1), "v"),
            false,
        ),
        set_case(
            format!("{outside:?} in the key"),
            entry(format!("bad{outside}key"), "v"),
            false,
        ),
        // And the same two offences on the value.
        set_case(
            format!("a {max_value}-character value, at the cap"),
            entry("k", "v".repeat(max_value)),
            true,
        ),
        set_case(
            format!("a {}-character value, one past the cap", max_value + 1),
            entry("k", "v".repeat(max_value + 1)),
            false,
        ),
        set_case(
            format!("{outside:?} in the value"),
            entry("k", format!("bad{outside}value")),
            false,
        ),
    ];

    // The per-entry half runs over EVERY entry, not only the first: a set that is within
    // the count maximum and whose last entry alone is inadmissible is still refused.
    let mut mixed = entries(max_tags - 1);
    mixed
        .as_object_mut()
        .expect("entries builds an object")
        .insert("k".repeat(max_key + 1), json!("v"));
    cases.push(set_case(
        format!("{max_tags} entries of which one key is past the cap"),
        mixed,
        false,
    ));

    // Both halves tripped at once: still one refusal, and the per-entry half is the one
    // the rule checks first, so the text names the entry rather than the count.
    let mut both = entries(max_tags + 1);
    both.as_object_mut()
        .expect("entries builds an object")
        .insert("k".to_string(), json!("v".repeat(max_value + 1)));
    cases.push(set_case(
        "a set past the maximum that also carries an inadmissible entry",
        both,
        false,
    ));
    cases
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

/// The tags `ListTagsForResource` reports for `arn`, read back through the API.
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

/// What one write path answered, reduced to what the rule decides: the status, the wire
/// error code and the refusal text. Two paths applying ONE rule answer the same triple.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    status: u16,
    error_code: Option<String>,
    message: String,
}

/// The verdict `answer` carries, with `success` as the status an admitted write answers on
/// that path (the two creates answer 200, `TagResource` the no-content success status).
fn verdict(answer: &Answer, success: u16) -> Verdict {
    Verdict {
        status: if answer.status == success {
            // Normalised, so paths whose SUCCESS status differs by model can still be
            // compared on the one thing the rule decides: admitted or refused.
            0
        } else {
            answer.status
        },
        error_code: answer.error_code.clone(),
        message: answer.string("message"),
    }
}

/// Submit `tags` to `StartSearchJob` and answer its verdict.
async fn start_search_job(emulator: &MultiServiceRouter, tags: &Value) -> Answer {
    call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({
            "Name": "admission",
            "SearchScope": {"BackupResourceTypes": ["S3"]},
            "Tags": tags,
        })),
    )
    .await
}

/// Submit `tags` to `StartSearchResultExportJob` for an already exportable search job.
async fn start_export_job(
    emulator: &MultiServiceRouter,
    search_job: &str,
    tags: &Value,
) -> Answer {
    call(
        emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "admission-exports"}
            },
            "RoleArn": passable_role("backup-search-export"),
            "Tags": tags,
        })),
    )
    .await
}

/// Submit `tags` to `TagResource` for the resource `arn` names.
async fn tag_resource(emulator: &MultiServiceRouter, arn: &str, tags: &Value) -> Answer {
    call(
        emulator,
        REGION,
        "POST",
        &format!("/tags/{}", percent_encode(arn)),
        Some(json!({"Tags": tags})),
    )
    .await
}

#[tokio::test]
async fn every_write_of_a_tag_applies_one_admission_rule_and_answers_one_verdict() {
    let (emulator, engine) = emulator_with_engine();
    // One exportable search job for every row: what varies here is the submitted tag set,
    // not how a search job reached a status an export may be started from.
    let exportable = seed_job(&engine, state::COMPLETED, Vec::new());

    for case in set_cases() {
        let started = start_search_job(&emulator, &case.tags).await;
        let exported = start_export_job(&emulator, &exportable, &case.tags).await;
        // An untagged resource of its own per row, so `TagResource` checks the same count
        // the creates do and the three verdicts are comparable.
        let untagged = start_search_job(&emulator, &json!({})).await;
        assert_eq!(
            untagged.status, 200,
            "case {:?}: an untagged StartSearchJob must succeed to set the row up, got {} ({:?})",
            case.case, untagged.status, untagged.body
        );
        let untagged_arn = untagged.string("SearchJobArn");
        let tagged = tag_resource(&emulator, &untagged_arn, &case.tags).await;

        let create = verdict(&started, 200);
        let export = verdict(&exported, 200);
        let merge = verdict(&tagged, state::NO_CONTENT_SUCCESS_STATUS);

        if case.admitted {
            assert_eq!(
                create.status, 0,
                "case {:?} at StartSearchJob: an admissible set must be accepted, got {} ({:?})",
                case.case, started.status, started.body
            );
            // Admitted means persisted: the set reads back through the API on each path.
            let wanted = as_map(&case.tags);
            for (operation, arn) in [
                ("StartSearchJob", started.string("SearchJobArn")),
                ("StartSearchResultExportJob", exported.string("ExportJobArn")),
                ("TagResource", untagged_arn.clone()),
            ] {
                let reported = stored_tags(&emulator, &case.case, &arn).await;
                assert_eq!(
                    reported, wanted,
                    "case {:?} at {operation}: an admitted set must read back exactly, got \
                     {reported:?}",
                    case.case
                );
            }
        } else {
            assert_eq!(
                create.status, 400,
                "case {:?} at StartSearchJob: an inadmissible set must be refused 400, got {} \
                 ({:?})",
                case.case, started.status, started.body
            );
            assert_eq!(
                create.error_code.as_deref(),
                Some("ValidationException"),
                "case {:?} at StartSearchJob: the refusal must be a ValidationException, got {:?}",
                case.case,
                create.error_code
            );
            assert!(
                create.message.contains("Tags"),
                "case {:?} at StartSearchJob: the refusal must name the member it refused, got \
                 {:?}",
                case.case,
                create.message
            );
            // A refused write persists nothing a later caller can see.
            let reported = stored_tags(&emulator, &case.case, &untagged_arn).await;
            assert!(
                reported.is_empty(),
                "case {:?} at TagResource: a refused write must leave the resource untagged, got \
                 {reported:?}",
                case.case
            );
        }

        assert_eq!(
            export, create,
            "case {:?}: StartSearchResultExportJob must answer the same verdict as \
             StartSearchJob, got {export:?}",
            case.case
        );
        assert_eq!(
            merge, create,
            "case {:?}: TagResource must answer the same verdict as StartSearchJob, got {merge:?}",
            case.case
        );
    }
}

/// One row of the merge table: how many tags the resource already holds, how many of the
/// submitted keys are new, how many only overwrite a stored entry.
struct MergeCase {
    case: String,
    stored: usize,
    added: usize,
    overwritten: usize,
    admitted: bool,
}

fn merge_cases() -> Vec<MergeCase> {
    let max_tags = state::MAX_TAGS_PER_JOB;
    let rows = [
        // (stored, added, overwritten, admitted)
        (0, 1, 0, true),
        (1, 1, 0, true),
        (max_tags - 1, 1, 0, true),
        (max_tags - 1, 2, 0, false),
        (max_tags, 0, 1, true),
        // A full record still takes a write that only overwrites stored keys: an existing
        // key is replaced, so the count the rule checks does not grow.
        (max_tags, 0, max_tags, true),
        (max_tags, 1, 0, false),
        (max_tags, 1, 5, false),
        (max_tags / 2, max_tags / 2, 3, true),
        (max_tags / 2, max_tags, 0, false),
    ];
    rows.into_iter()
        .map(|(stored, added, overwritten, admitted)| MergeCase {
            case: format!(
                "{stored} stored, {added} new keys and {overwritten} overwritten ones submitted"
            ),
            stored,
            added,
            overwritten,
            admitted,
        })
        .collect()
}

/// `count` entries named for the record they are stored under first, so the submitted set
/// can name the same keys to overwrite them or fresh ones to add.
fn stored_entries(count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("stored-{index}"), json!("first"));
    }
    Value::Object(tags)
}

#[tokio::test]
async fn a_merge_is_admitted_on_the_count_the_record_would_hold_afterwards() {
    let (emulator, _engine) = emulator_with_engine();

    for case in merge_cases() {
        // A record holding exactly `stored` tags, minted through the create path so the
        // starting state is one a client can produce.
        let created = start_search_job(&emulator, &stored_entries(case.stored)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: StartSearchJob must succeed to set the row up, got {} ({:?})",
            case.case, created.status, created.body
        );
        let arn = created.string("SearchJobArn");
        let before = stored_tags(&emulator, &case.case, &arn).await;
        assert_eq!(
            before.len(),
            case.stored,
            "case {:?}: the row must start from {} stored tags, got {before:?}",
            case.case,
            case.stored
        );

        let mut submitted = serde_json::Map::new();
        for index in 0..case.added {
            submitted.insert(format!("added-{index}"), json!("second"));
        }
        for index in 0..case.overwritten.min(case.stored) {
            submitted.insert(format!("stored-{index}"), json!("second"));
        }
        let submitted = Value::Object(submitted);
        let merged = tag_resource(&emulator, &arn, &submitted).await;
        let after = stored_tags(&emulator, &case.case, &arn).await;

        if case.admitted {
            assert_eq!(
                merged.status,
                state::NO_CONTENT_SUCCESS_STATUS,
                "case {:?}: a merge within the maximum must be accepted, got {} ({:?})",
                case.case,
                merged.status,
                merged.body
            );
            assert_eq!(
                after.len(),
                case.stored + case.added,
                "case {:?}: an overwritten key must not grow the record, got {after:?}",
                case.case
            );
            for (key, value) in as_map(&submitted) {
                assert_eq!(
                    after.get(&key).map(String::as_str),
                    Some(value.as_str()),
                    "case {:?}: the merged entry {key:?} must read back, got {after:?}",
                    case.case
                );
            }
        } else {
            assert_eq!(
                merged.status, 400,
                "case {:?}: a merge past the maximum must be refused 400, got {} ({:?})",
                case.case, merged.status, merged.body
            );
            assert_eq!(
                merged.error_code.as_deref(),
                Some("ValidationException"),
                "case {:?}: the refusal must be a ValidationException, got {:?}",
                case.case,
                merged.error_code
            );
            assert_eq!(
                after, before,
                "case {:?}: a refused merge must leave the record as it was, got {after:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn the_admission_rule_checks_every_entry_and_then_the_resulting_count() {
    // The shared rule's own invariant, under the state module, over the same two
    // dimensions the HTTP rows walk: an inadmissible entry is refused whatever the count,
    // and the count that is checked is the one the record would hold AFTERWARDS.
    let max_tags = state::MAX_TAGS_PER_JOB;
    let admissible: std::collections::HashMap<String, String> = (0..max_tags)
        .map(|index| (format!("key-{index}"), "v".to_string()))
        .collect();
    let checked = state::check_tag_map::<()>(&admissible, None);
    assert!(
        checked.is_ok(),
        "case {:?}: a set at the maximum of admissible entries must be admitted, got {checked:?}",
        format!("{max_tags} admissible entries")
    );

    let mut past_the_count = admissible.clone();
    past_the_count.insert("one-more".to_string(), "v".to_string());
    assert!(
        state::check_tag_map::<()>(&past_the_count, None).is_err(),
        "case {:?}: a set past the maximum must be refused",
        format!("{} admissible entries", max_tags + 1)
    );
    // The SAME set is admitted as a merge that overwrites, because the record would still
    // hold no more than the maximum afterwards.
    assert!(
        state::check_tag_map::<()>(&admissible, Some(&admissible)).is_ok(),
        "case {:?}: a merge that only overwrites stored keys must be admitted",
        format!("{max_tags} stored, the same {max_tags} keys submitted")
    );
    let one_new: std::collections::HashMap<String, String> =
        [("one-more".to_string(), "v".to_string())].into_iter().collect();
    assert!(
        state::check_tag_map::<()>(&one_new, Some(&admissible)).is_err(),
        "case {:?}: a merge adding a key to a full record must be refused",
        format!("{max_tags} stored, one new key submitted")
    );

    // An inadmissible entry is refused whether it sits in a set of one or in a full one.
    let over_long: std::collections::HashMap<String, String> = [(
        "k".repeat(state::MAX_TAG_KEY_LENGTH + 1),
        "v".to_string(),
    )]
    .into_iter()
    .collect();
    for (case, submitted) in [
        ("on its own", over_long.clone()),
        ("among admissible entries", {
            let mut mixed = admissible.clone();
            mixed.extend(over_long.clone());
            mixed
        }),
    ] {
        assert!(
            state::check_tag_map::<()>(&submitted, None).is_err(),
            "case {case:?}: an entry past the key cap must be refused"
        );
        assert!(
            state::check_tag_map::<()>(&submitted, Some(&admissible)).is_err(),
            "case {case:?}: an entry past the key cap must be refused on a merge too"
        );
    }
}
