// SPDX-License-Identifier: CC0-1.0

//! A custom vocabulary whose terms the request names a FILE for, end to end over the
//! wire: `CreateVocabulary` / `UpdateVocabulary` with `VocabularyFileUri` instead of
//! `Phrases`, then `GetVocabulary` -> `ListVocabularies` -> `TagResource` ->
//! `DeleteVocabulary` over the record they stored.
//!
//! THE RULE THESE ROWS ASSERT: `CreateVocabulary` and `UpdateVocabulary` publish two
//! alternatives for where a vocabulary's terms come from — the list in `Phrases` and
//! the file at `VocabularyFileUri` — and a request choosing EITHER is accepted and
//! recorded, so every later operation of the family finds the resource. Only the one
//! transition the clock owes the build differs: a vocabulary built from the terms the
//! request listed becomes `READY`, while one whose terms would have had to be READ
//! from a file ends in the service's own modelled `FAILED` state with a
//! `FailureReason` naming that file — this emulator reads no object store and makes
//! no outbound request, so nothing built it and nothing may report it as usable. The
//! refusals that remain (neither alternative, or both at once) are
//! `crate::vocabularies`' subject; the medical family, whose terms can only come from
//! such a file, is `crate::medical_vocabularies`'.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.

use std::time::Duration;

use serde_json::{json, Value};

use crate::medical_vocabularies::is_absent;
use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the one clock-owed transition of a vocabulary created during the
/// test to have fallen due: one span of `crate::state::VOCABULARY_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The region every row of this suite is served in (the rows are unsigned, and this
/// is the region this emulator serves a request carrying no SigV4 credential scope
/// in), so a tagging ARN is predictable.
const REGION: &str = "us-east-1";

/// A valid `DataAccessRoleArn`, which the member's `@pattern` pins to an IAM role ARN
/// (`smithy_query(query="operation", operation="CreateVocabulary")`).
const DATA_ACCESS_ROLE: &str = "arn:aws:iam::000000000000:role/TranscribeAccessRole";

/// The class of `CreateVocabulary` request that names a file as the source of its
/// terms, one row per axis the rule generalises over: which URI scheme names the
/// file, the form of the vocabulary name, which of the model's `LanguageCode` values
/// the vocabulary is in, and whether the request also carries tags, a data-access
/// role and an encryption configuration.
///
/// The schemes are the ones `VocabularyFileUri`'s own `@pattern` admits —
/// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
/// operation="CreateVocabulary")`) — so the rows cover each alternative of that
/// catalog, including the two the issue did not cite; the language codes are entries
/// of the model's own `LanguageCode` enum (`smithy_query(query="shape",
/// shape="LanguageCode")`).
fn file_backed_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an s3 file, en-US",
            json!({
                "VocabularyName": "terms-from-s3",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
        ),
        (
            "an https file and a dotted, mixed-case name, en-GB",
            json!({
                "VocabularyName": "Terms.From-Https",
                "LanguageCode": "en-GB",
                "VocabularyFileUri": "https://example-bucket.s3.amazonaws.com/vocab.txt"
            }),
        ),
        (
            "an http file, an underscored name and tags, es-US",
            json!({
                "VocabularyName": "terms_from_http",
                "LanguageCode": "es-US",
                "VocabularyFileUri": "http://example-bucket.s3.amazonaws.com/vocab.txt",
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]
            }),
        ),
        (
            "an s3 file with a data-access role and an encryption configuration, ja-JP",
            json!({
                "VocabularyName": "terms-fully-specified",
                "LanguageCode": "ja-JP",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/ja-vocab-file.txt",
                "DataAccessRoleArn": DATA_ACCESS_ROLE,
                "EncryptionConfiguration": {"KMSKey": "alias/aws/transcribe"}
            }),
        ),
        (
            "an s3 file and a name at the modelled 200-character ceiling, de-DE",
            json!({
                "VocabularyName": "z".repeat(200),
                "LanguageCode": "de-DE",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/de-vocab-file.txt"
            }),
        ),
    ]
}

/// The `VocabularyName` member of a request row.
fn name_of(request: &Value) -> String {
    request["VocabularyName"]
        .as_str()
        .expect("every row names a vocabulary")
        .to_string()
}

/// The `VocabularyFileUri` member of a request row.
fn file_uri_of(request: &Value) -> String {
    request["VocabularyFileUri"]
        .as_str()
        .expect("every row names a file")
        .to_string()
}

/// The `VocabularyName`s of a `ListVocabularies` answer, in the order it listed them.
fn listed_names(reply: &Reply, case: &str) -> Vec<String> {
    reply.ok(case)["Vocabularies"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Vocabularies, got {:?}", reply.body))
        .iter()
        .map(|entry| {
            entry["VocabularyName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a name in {entry:?}"))
                .to_string()
        })
        .collect()
}

/// A create that names a file is ACCEPTED and recorded: it answers 200 with the
/// vocabulary `PENDING`, the record reads back through `GetVocabulary`, and the build
/// this emulator cannot perform ends `FAILED` with a reason naming the file nothing
/// read once the clock makes that transition due.
#[tokio::test]
async fn a_file_backed_vocabulary_is_recorded_pending_then_fails_and_reads_back() {
    let transcribe = Transcribe::new();
    let rows = file_backed_rows();
    let mut created_at: Vec<(&str, String, String, f64)> = Vec::new();

    for (case, request) in &rows {
        let name = name_of(request);
        let uri = file_uri_of(request);
        let language = request["LanguageCode"]
            .as_str()
            .expect("every row names a language")
            .to_string();

        let created = transcribe.call("CreateVocabulary", request.clone()).await;
        created.ok(case);
        assert_eq!(
            (
                created.member(case, "VocabularyName"),
                created.member(case, "LanguageCode"),
                created.member(case, "VocabularyState"),
            ),
            (name.clone(), language.clone(), "PENDING".to_string()),
            "case {case:?}: the create must answer the record it stored, PENDING, got {:?}",
            created.body
        );
        assert!(
            is_absent(&created, "FailureReason"),
            "case {case:?}: a PENDING vocabulary has not failed, got {:?}",
            created.body
        );
        let created_time = created.timestamp(case, "LastModifiedTime");

        // The record reads back through the family's own reader, with the same facts.
        let got = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        got.ok(case);
        assert_eq!(
            (
                got.member(case, "VocabularyName"),
                got.member(case, "LanguageCode"),
                got.member(case, "VocabularyState"),
                got.timestamp(case, "LastModifiedTime"),
            ),
            (
                name.clone(),
                language.clone(),
                "PENDING".to_string(),
                created_time
            ),
            "case {case:?}: the reader must report the record just created, got {:?}",
            got.body
        );
        assert!(
            is_absent(&got, "DownloadUri"),
            "case {case:?}: nothing read this vocabulary's file, so there is no file to \
             download, got {:?}",
            got.body
        );
        if let Some(role) = request.get("DataAccessRoleArn").and_then(Value::as_str) {
            assert_eq!(
                got.member(case, "DataAccessRoleArn"),
                role,
                "case {case:?}: the stored data-access role must read back, got {:?}",
                got.body
            );
        }

        created_at.push((case, name, uri, created_time));
    }

    // The transition the clock owes every one of these records: the build that would
    // have had to READ the file ends in the modelled failure state.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, name, uri, created_time) in &created_at {
        let got = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        got.ok(case);
        assert_eq!(
            got.member(case, "VocabularyState"),
            "FAILED",
            "case {case:?}: a vocabulary whose file is never read must end FAILED, got {:?}",
            got.body
        );
        let reason = got.member(case, "FailureReason");
        assert!(
            reason.contains(uri),
            "case {case:?}: the failure reason must name the file that was not read \
             ({uri}), got {reason:?}"
        );
        let failed_time = got.timestamp(case, "LastModifiedTime");
        assert!(
            failed_time > *created_time,
            "case {case:?}: the record changed when the build ended, so its \
             LastModifiedTime must move past {created_time}, got {failed_time}"
        );
        assert!(
            is_absent(&got, "DownloadUri"),
            "case {case:?}: a vocabulary nothing built has no file to download, got {:?}",
            got.body
        );
    }

    // Every one of them is in the store, and the state filter selects them by the
    // state they actually reached — never as READY, which nothing built them for.
    let failed = transcribe
        .call("ListVocabularies", json!({"StateEquals": "FAILED"}))
        .await;
    let mut listed = listed_names(&failed, "StateEquals FAILED");
    listed.sort();
    let mut expected: Vec<String> = rows.iter().map(|(_, request)| name_of(request)).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "every file-backed vocabulary must be listed as FAILED, got {:?}",
        failed.body
    );
    let ready = transcribe
        .call("ListVocabularies", json!({"StateEquals": "READY"}))
        .await;
    assert_eq!(
        listed_names(&ready, "StateEquals READY"),
        Vec::<String>::new(),
        "no vocabulary built from a file this emulator never read may be READY, got {:?}",
        ready.body
    );

    // FAILED is a terminal state, so the delete the issue's lifecycle ends with goes
    // through and the record is gone afterwards.
    for (case, name, _, _) in &created_at {
        transcribe
            .call("DeleteVocabulary", json!({"VocabularyName": name}))
            .await
            .ok_status(case, 200);
        transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await
            .refused(case, 400, "BadRequestException");
    }
}

/// The three tagging operations reach a file-backed vocabulary through the same
/// `vocabulary` ARN resource type as any other, and the create's own tags are on the
/// record.
#[tokio::test]
async fn tags_round_trip_on_a_file_backed_vocabulary() {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "tagged-file-vocab",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt",
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
        )
        .await
        .ok("create the file-backed vocabulary to tag");

    let resource_arn = format!(
        "arn:aws:transcribe:{REGION}:{account}:vocabulary/tagged-file-vocab",
        account = engine::emulator_account_id()
    );

    let listed = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": &resource_arn}))
        .await;
    listed.ok("the tags the create named");
    assert_eq!(
        listed.body["Tags"],
        json!([{"Key": "team", "Value": "speech"}]),
        "the create's own tags must be stored on the record, got {:?}",
        listed.body
    );

    transcribe
        .call(
            "TagResource",
            json!({
                "ResourceArn": &resource_arn,
                "Tags": [{"Key": "env", "Value": "test"}]
            }),
        )
        .await
        .ok("tag the file-backed vocabulary");
    let after_tag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": &resource_arn}))
        .await;
    after_tag.ok("after tagging");
    assert_eq!(
        after_tag.body["Tags"],
        json!([{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]),
        "tagging must merge into the stored set, got {:?}",
        after_tag.body
    );

    transcribe
        .call(
            "UntagResource",
            json!({"ResourceArn": &resource_arn, "TagKeys": ["team"]}),
        )
        .await
        .ok_status("untag the file-backed vocabulary", 200);
    let after_untag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": &resource_arn}))
        .await;
    after_untag.ok("after untagging");
    assert_eq!(
        after_untag.body["Tags"],
        json!([{"Key": "env", "Value": "test"}]),
        "untagging must remove exactly the named keys, got {:?}",
        after_untag.body
    );
}

/// `UpdateVocabulary` accepts either source too, and which one the LAST update chose
/// is what the rebuilt vocabulary's state and `DownloadUri` follow from — in both
/// directions, so nothing here is keyed to the shape of one request.
#[tokio::test]
async fn an_update_switches_a_vocabulary_between_its_two_term_sources() {
    let transcribe = Transcribe::new();
    let name = "switched";
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("create the vocabulary from inline terms");

    // Each row is one `UpdateVocabulary`, the state its rebuild must reach once the
    // clock makes it due, and whether a vocabulary rebuilt that way has a file to
    // download. The rows alternate between the two sources, so each direction of the
    // switch is covered.
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        (
            "inline terms replaced by an s3 file",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
            "FAILED",
            false,
        ),
        (
            "an s3 file replaced by inline terms in another language",
            json!({
                "VocabularyName": name,
                "LanguageCode": "fr-FR",
                "Phrases": ["Zèbre", "Gnou"]
            }),
            "READY",
            true,
        ),
        (
            "inline terms replaced by an https file",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-GB",
                "VocabularyFileUri": "https://example-bucket.s3.amazonaws.com/vocab.txt"
            }),
            "FAILED",
            false,
        ),
        (
            "an https file replaced by one inline term",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            "READY",
            true,
        ),
    ];

    for (case, request, settled_state, has_download_uri) in rows {
        let language = request["LanguageCode"].as_str().expect("a code").to_string();
        let updated = transcribe.call("UpdateVocabulary", request.clone()).await;
        updated.ok(case);
        assert_eq!(
            (
                updated.member(case, "VocabularyState"),
                updated.member(case, "LanguageCode"),
            ),
            ("PENDING".to_string(), language.clone()),
            "case {case:?}: a rebuilt vocabulary is PENDING again, in the language the \
             update named, got {:?}",
            updated.body
        );

        std::thread::sleep(PAST_THE_TRANSITION);
        let got = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        got.ok(case);
        assert_eq!(
            got.member(case, "VocabularyState"),
            settled_state,
            "case {case:?}: the rebuild must settle in {settled_state}, got {:?}",
            got.body
        );
        assert_eq!(
            !is_absent(&got, "DownloadUri"),
            has_download_uri,
            "case {case:?}: a vocabulary has a file to download only when this service \
             built it from the terms the request listed, got {:?}",
            got.body
        );
        assert_eq!(
            !is_absent(&got, "FailureReason"),
            settled_state == "FAILED",
            "case {case:?}: a FailureReason belongs to a failed build and nothing else, \
             got {:?}",
            got.body
        );
    }
}
