// SPDX-License-Identifier: CC0-1.0

//! One value per tag key, whichever operation wrote the tag set, end to end over the
//! wire.
//!
//! The rule these tests pin: a resource's tags are keyed by their key — a tag is
//! "metadata, in the form of a key:value pair" (`Tag`'s model doc) — so a write holds at
//! most one value for a key: a request naming one key more than once leaves the LAST
//! value it gave, and a write of a key the resource already carries replaces that key's
//! value rather than storing a second entry beside it. So no read can report one key
//! twice, and a later write of that key leaves exactly one value in force.
//!
//! The rows differ along what the rule generalises over: how many times a key repeats,
//! where the repeats sit in the list, how many distinct keys the list carries, whether
//! the repeated values differ at all, the value a repeat ends on (including the empty
//! string the modelled `TagValue` `@length(min: 0, max: 256)` allows), and keys that
//! differ only in case — two keys, because a tag key is case-sensitive. Each row is
//! driven through BOTH writers of a tag set (every family's create/start `Tags` member
//! and `TagResource`) and over EVERY resource family this service stores records of,
//! reusing `crate::tagging_resource_families`'s table rather than writing a second copy.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`), so these tests exercise the awsJson1_1 envelope and
//! the modelled tag constraints too.

use serde_json::json;

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::tagging_resource_families::{expected_pairs, families, pairs, tag_list, Family};
use crate::transcription_jobs::Transcribe;

/// One request's `Tags` list and the tag set the resource must hold once it is written.
struct Row {
    case: &'static str,
    /// The list as the request carries it, repeated keys and all.
    written: Vec<(&'static str, &'static str)>,
    /// The set the resource holds afterwards: one entry per key, carrying the last value
    /// the list gave for it.
    expected: Vec<(&'static str, &'static str)>,
}

/// The table: tag lists whose keys repeat, and the one-value-per-key set each collapses
/// to, plus the row that pins what is NOT a repeat (keys differing only in case).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one key twice, two values",
            written: vec![("env", "a"), ("env", "b")],
            expected: vec![("env", "b")],
        },
        Row {
            case: "one key three times",
            written: vec![("env", "a"), ("env", "b"), ("env", "c")],
            expected: vec![("env", "c")],
        },
        Row {
            case: "a repeat straddling a distinct key",
            written: vec![("env", "a"), ("team", "speech"), ("env", "b")],
            expected: vec![("env", "b"), ("team", "speech")],
        },
        Row {
            case: "two keys each repeated",
            written: vec![("env", "a"), ("cost", "1"), ("env", "b"), ("cost", "2")],
            expected: vec![("env", "b"), ("cost", "2")],
        },
        Row {
            case: "a repeat whose last value is the empty string",
            written: vec![("env", "test"), ("env", "")],
            expected: vec![("env", "")],
        },
        Row {
            case: "the same key and value twice",
            written: vec![("env", "test"), ("env", "test")],
            expected: vec![("env", "test")],
        },
        Row {
            case: "keys differing only in case are two keys",
            written: vec![("Env", "a"), ("env", "b")],
            expected: vec![("Env", "a"), ("env", "b")],
        },
    ]
}

/// `ListTagsForResource` for `arn`, as the tag set it reports.
async fn listed(transcribe: &Transcribe, arn: &str, case: &str) -> Vec<(String, String)> {
    let reply = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": arn}))
        .await;
    let body = reply.ok(case);
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {body:?}"));
    pairs(tags, case)
}

/// The ARN of `family`'s resource named `name`, as the tagging operations take it for a
/// request served in the emulator's default Region.
fn arn_of(family: &Family, name: &str) -> String {
    family.arn_in(DEFAULT_REGION, name)
}

/// RULE: a create/start whose `Tags` name one key more than once records ONE value for
/// that key — the last the list gave — so the tagging API reports the key once.
#[tokio::test]
async fn a_create_stores_one_value_per_tag_key() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{} / {} / {}", family.case, family.create, row.case);
            let name = family.resource_name(&format!("createdup{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &row.written))
                .await
                .ok(&case);

            let found = listed(&transcribe, &arn_of(&family, &name), &case).await;
            assert_eq!(
                found,
                expected_pairs(&row.expected),
                "case {case:?}: a create must store one value per key, got {found:?}"
            );
        }
    }
}

/// RULE, the other writer: `TagResource` collapses a repeated key in its own request the
/// same way.
#[tokio::test]
async fn tag_resource_stores_one_value_per_tag_key() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{} / TagResource / {}", family.case, row.case);
            let name = family.resource_name(&format!("tagdup{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &[]))
                .await
                .ok(&case);
            let arn = arn_of(&family, &name);

            transcribe
                .call(
                    "TagResource",
                    json!({"ResourceArn": arn, "Tags": tag_list(&row.written)}),
                )
                .await
                .ok(&case);

            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                expected_pairs(&row.expected),
                "case {case:?}: TagResource must store one value per key, got {found:?}"
            );
        }
    }
}

/// RULE, where the two writers meet: whatever a create stored, a later `TagResource` for
/// one of its keys leaves exactly one value for that key in force — never a new value
/// beside a stale one — and the resource's other keys are untouched.
#[tokio::test]
async fn a_later_tag_resource_leaves_one_value_in_force() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{} / re-tagged / {}", family.case, row.case);
            let name = family.resource_name(&format!("retagged{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &row.written))
                .await
                .ok(&case);
            let arn = arn_of(&family, &name);

            // Re-tag every key the create's list named, each with a value no entry of
            // that list carried, so a stale copy left beside the new value shows up.
            let retagged: Vec<(&str, &str)> = row
                .expected
                .iter()
                .map(|(key, _)| (*key, "in-force"))
                .collect();
            transcribe
                .call(
                    "TagResource",
                    json!({"ResourceArn": arn, "Tags": tag_list(&retagged)}),
                )
                .await
                .ok(&case);

            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                expected_pairs(&retagged),
                "case {case:?}: one value per key must be in force after re-tagging, got \
                 {found:?}"
            );

            // And the value in force is the only one: removing each key empties the set
            // rather than uncovering a second copy of it.
            let keys: Vec<&str> = retagged.iter().map(|(key, _)| *key).collect();
            transcribe
                .call(
                    "UntagResource",
                    json!({"ResourceArn": arn, "TagKeys": keys}),
                )
                .await
                .ok_status(&case, 200);
            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                Vec::<(String, String)>::new(),
                "case {case:?}: removing every key must leave no stale copy, got {found:?}"
            );
        }
    }
}

/// RULE, through the family's OWN read: a family whose output shape carries `Tags`
/// reports the same one-value-per-key set the tagging API does, so no read of the record
/// can see a key twice.
#[tokio::test]
async fn a_family_s_own_read_reports_one_value_per_key() {
    let transcribe = Transcribe::new();

    // The reads this service implements for the families whose modelled record carries a
    // `Tags` member (`smithy_query(query="shape", shape="TranscriptionJob,\
    // MedicalTranscriptionJob,CategoryProperties,MedicalScribeJob,CallAnalyticsJob")`);
    // the other families' records are read only through the tagging API, which the tests
    // above cover.
    let readers: Vec<(&str, &str, &str, &str)> = vec![
        (
            "StartTranscriptionJob",
            "GetTranscriptionJob",
            "TranscriptionJobName",
            "TranscriptionJob",
        ),
        (
            "StartMedicalTranscriptionJob",
            "GetMedicalTranscriptionJob",
            "MedicalTranscriptionJobName",
            "MedicalTranscriptionJob",
        ),
        (
            "CreateCallAnalyticsCategory",
            "GetCallAnalyticsCategory",
            "CategoryName",
            "CategoryProperties",
        ),
        (
            "StartMedicalScribeJob",
            "GetMedicalScribeJob",
            "MedicalScribeJobName",
            "MedicalScribeJob",
        ),
        (
            "StartCallAnalyticsJob",
            "GetCallAnalyticsJob",
            "CallAnalyticsJobName",
            "CallAnalyticsJob",
        ),
    ];

    for (create, read, name_member, record_member) in readers {
        let family = families()
            .into_iter()
            .find(|family| family.create == create)
            .unwrap_or_else(|| panic!("no family creates through {create}"));
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{read} / {}", row.case);
            let name = family.resource_name(&format!("ownread{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &row.written))
                .await
                .ok(&case);

            let reply = transcribe.call(read, json!({name_member: name})).await;
            let body = reply.ok(&case);
            let record = body.get(record_member).unwrap_or_else(|| {
                panic!("case {case:?}: expected {record_member}, got {body:?}")
            });
            let tags = record
                .get("Tags")
                .filter(|tags| !tags.is_null())
                .unwrap_or_else(|| panic!("case {case:?}: expected Tags, got {record:?}"));
            let found = pairs(tags, &case);
            assert_eq!(
                found,
                expected_pairs(&row.expected),
                "case {case:?}: {read} must report one value per key, got {found:?}"
            );
            // Nothing read twice: the list the record reports is as long as its key set.
            let reported = tags.as_array().map(Vec::len).unwrap_or_default();
            assert_eq!(
                reported,
                row.expected.len(),
                "case {case:?}: {read} must report {} tags, got {reported}",
                row.expected.len()
            );
        }
    }
}

/// The modelled `TagList` is `@length(min: 1, max: 200)`
/// (`smithy_query(query="shape", shape="TagList")`), so a list of 200 entries is a valid
/// request however many of its keys repeat: the collapse happens on the stored set, not
/// at the boundary.
#[tokio::test]
async fn a_full_tag_list_of_one_repeated_key_collapses_to_that_key() {
    let transcribe = Transcribe::new();
    let family = &families()[0];
    let name = family.resource_name("fulllist");
    let case = "200 entries of one key";

    let values: Vec<String> = (0..200).map(|index| format!("value{index}")).collect();
    let written: Vec<(&str, &str)> = values
        .iter()
        .map(|value| ("env", value.as_str()))
        .collect();
    transcribe
        .call(family.create, family.create_request(&name, &written))
        .await
        .ok(case);

    let last = values.last().expect("the table holds 200 values");
    let found = listed(&transcribe, &arn_of(family, &name), case).await;
    assert_eq!(
        found,
        expected_pairs(&[("env", last.as_str())]),
        "case {case:?}: the stored set must hold the last value of the only key, got \
         {found:?}"
    );
}

/// RULE, at the ceiling: a list whose repeats collapse to a set within the per-resource
/// tag ceiling is stored, and the stored set is the collapsed one — so a repeat does not
/// consume room the resource does not have.
#[tokio::test]
async fn repeats_do_not_consume_room_in_the_stored_set() {
    let transcribe = Transcribe::new();
    let family = &families()[0];
    let name = family.resource_name("ceilingroom");
    let case = "repeats leave room for other keys";

    transcribe
        .call(
            family.create,
            family.create_request(&name, &[("env", "a"), ("env", "b")]),
        )
        .await
        .ok(case);
    let arn = arn_of(family, &name);

    // The create's two entries hold ONE key, so a later write of a different key is
    // room the resource still has.
    transcribe
        .call(
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tag_list(&[("team", "speech")])}),
        )
        .await
        .ok(case);

    let found = listed(&transcribe, &arn, case).await;
    let expected: Vec<(&str, &str)> = vec![("env", "b"), ("team", "speech")];
    assert_eq!(
        found,
        expected_pairs(&expected),
        "case {case:?}: the collapsed set must carry both keys, got {found:?}"
    );
}

/// A value the stored set must not depend on: the ORDER of the request's entries decides
/// which value wins, so the same keys written in the other order end on the other value.
#[tokio::test]
async fn the_last_entry_of_the_request_is_the_value_in_force() {
    let transcribe = Transcribe::new();
    let family = &families()[0];

    for (index, (case, written, winner)) in [
        ("a before b", [("env", "a"), ("env", "b")], "b"),
        ("b before a", [("env", "b"), ("env", "a")], "a"),
    ]
    .into_iter()
    .enumerate()
    {
        let name = family.resource_name(&format!("ordered{index}"));
        transcribe
            .call(family.create, family.create_request(&name, &written))
            .await
            .ok(case);
        let found = listed(&transcribe, &arn_of(family, &name), case).await;
        assert_eq!(
            found,
            expected_pairs(&[("env", winner)]),
            "case {case:?}: the last entry for a key is the value in force, got {found:?}"
        );
    }
}
