// SPDX-License-Identifier: CC0-1.0

//! The ONE dispatch from an ARN's `<type>` segment to the store that answers for it,
//! driven through an operation of each borrow kind, end to end over the wire.
//!
//! THE RULE UNDER TEST: which family's store a resource ARN names is a single rule, so
//! the reporting side that READS a tag set (`ListTagsForResource`, and the reference
//! check a job start makes) and the side that WRITES one (`TagResource`,
//! `UntagResource`) reach the SAME stored record for the same ARN — for every family
//! this service holds records of, with no family readable through one side and unknown
//! to the other.
//!
//! The rows are every resource family, reused from `crate::tagging_resource_families`
//! rather than tabulated a second time, and for each of them the write side is driven
//! first and the read side afterwards, so a family whose two sides disagreed would show
//! up as a write the read cannot see (or a refusal on one side only).
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`).

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::tagging_resource_families::{expected_pairs, families, pairs, tag_list};
use crate::transcription_jobs::Transcribe;

/// The modelled client error this service answers for an ARN, or a settings reference,
/// that names no resource it holds, and the status its awsJson1_1 wire protocol puts on
/// every client error (both pinned by `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// `ListTagsForResource` for `arn`, as the tag set it reported.
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

/// RULE: for every family, a tag set WRITTEN through an ARN is the tag set READ back
/// through that same ARN — the two borrow kinds of the dispatch reach one record.
///
/// The rows differ along the family (and so the ARN `<type>` segment and the create that
/// establishes a record) and along how many keys each write carries.
#[tokio::test]
async fn both_borrow_kinds_of_the_dispatch_reach_the_same_record_for_every_family() {
    let transcribe = Transcribe::new();

    for (index, family) in families().into_iter().enumerate() {
        let case = format!("{} / write then read", family.case);
        let name = family.resource_name("dispatched");
        transcribe
            .call(family.create, family.create_request(&name, &[]))
            .await
            .ok(&case);
        let arn = family.arn_in(DEFAULT_REGION, &name);

        // The read side before any write: the record is held and carries no tags.
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            Vec::<(String, String)>::new(),
            "case {case:?}: a held record created without tags must report an empty set"
        );

        // The write side. Each family writes a different NUMBER of keys, one of them
        // naming the family, so a write landing on another family's record could not
        // read back as this one's set.
        let value = family.resource_type;
        let written: Vec<(&str, &str)> = match index % 3 {
            0 => vec![("family", value)],
            1 => vec![("family", value), ("env", "test")],
            _ => vec![("family", value), ("env", "test"), ("cost", "7")],
        };
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tag_list(&written)}),
            )
            .await
            .ok(&case);
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&written),
            "case {case:?}: the read side must report exactly what the write side stored"
        );

        // And the other write operation, through the same dispatch: what it removes the
        // read side no longer reports.
        transcribe
            .call(
                "UntagResource",
                json!({"ResourceArn": arn, "TagKeys": ["family"]}),
            )
            .await
            .ok_status(&case, 200);
        let kept: Vec<(&str, &str)> = written
            .into_iter()
            .filter(|(key, _)| *key != "family")
            .collect();
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&kept),
            "case {case:?}: the read side must report the removal the write side made"
        );
    }
}

/// RULE, the read side's OTHER caller: the lookup a job start makes to decide whether
/// this Region holds the resource its settings name is the same lookup the tagging
/// operations resolve an ARN through — so a name both see is served by both, and a name
/// neither holds is refused by both.
#[tokio::test]
async fn a_job_start_and_a_tagging_call_agree_about_which_names_are_held() {
    let transcribe = Transcribe::new();
    let held = "blocked-terms";
    let absent = "never-created-filter";

    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": held,
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok("a held custom vocabulary filter");

    // A start naming the held filter, and a tagging write through its ARN: both resolve.
    let start = |job: &str, filter: &str| {
        json!({
            "TranscriptionJobName": job,
            "LanguageCode": "en-US",
            "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
            "Settings": {
                "VocabularyFilterName": filter,
                "VocabularyFilterMethod": "remove"
            }
        })
    };
    let arn_of = |filter: &str| {
        engine::emulator_arn(
            "transcribe",
            DEFAULT_REGION,
            &format!("vocabulary-filter/{filter}"),
        )
    };

    let case = "the filter this Region holds";
    transcribe
        .call("StartTranscriptionJob", start("job-on-held-filter", held))
        .await
        .ok(case);
    transcribe
        .call(
            "TagResource",
            json!({"ResourceArn": arn_of(held), "Tags": tag_list(&[("team", "speech")])}),
        )
        .await
        .ok(case);
    assert_eq!(
        listed(&transcribe, &arn_of(held), case).await,
        expected_pairs(&[("team", "speech")]),
        "case {case:?}: the name a start resolves must be the name a tagging write reaches"
    );

    // The name no create recorded: the start's reference check and all three tagging
    // operations refuse it alike.
    let case = "a filter name this Region holds no record of";
    transcribe
        .call("StartTranscriptionJob", start("job-on-absent-filter", absent))
        .await
        .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    let calls: Vec<(&str, Value)> = vec![
        ("ListTagsForResource", json!({"ResourceArn": arn_of(absent)})),
        (
            "TagResource",
            json!({"ResourceArn": arn_of(absent), "Tags": tag_list(&[("team", "speech")])}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn_of(absent), "TagKeys": ["team"]}),
        ),
    ];
    for (operation, request) in calls {
        let case = format!("{case} / {operation}");
        transcribe
            .call(operation, request)
            .await
            .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }
}
