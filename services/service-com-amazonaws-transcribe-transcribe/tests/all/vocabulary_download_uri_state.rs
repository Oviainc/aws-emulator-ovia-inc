// SPDX-License-Identifier: CC0-1.0

//! When a custom vocabulary reports a `DownloadUri`, end to end over the wire:
//! `CreateVocabulary` / `UpdateVocabulary` / `CreateMedicalVocabulary` followed by the
//! family's own reader, both while the build is still running and once the clock has
//! made its outcome due.
//!
//! THE RULE THESE ROWS ASSERT: `DownloadUri` is where the BUILT custom vocabulary is
//! stored ("The Amazon S3 location where the custom vocabulary is stored; use this URI
//! to view or download the custom vocabulary" — `smithy_query(query="operation",
//! operation="GetVocabulary")`), so a read reports it only once the build that
//! produces it has finished successfully: `VocabularyState` `READY`, the state whose
//! own documentation says the vocabulary "is available to use". While the service is
//! still processing the terms (`PENDING`) nothing has been built, and after a build
//! that ended `FAILED` nothing ever will be, so the member is absent — whichever of
//! the two sources the request named its terms from, in both vocabulary families, and
//! on a rebuild just as on a first build. The model agrees structurally:
//! `CreateVocabularyResponse` and `UpdateVocabularyResponse`, the two answers that
//! always report `PENDING`, have no `DownloadUri` member at all.
//!
//! WHICH SOURCE the terms came from is a different suite's subject
//! (`crate::vocabulary_file_uri`): it decides which state the build settles in, and
//! this suite asserts the member follows that STATE.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope and the handlers'
//! own answers — not just the handler bodies. The emulator, the call and the answer
//! assertions are the transcription-job suite's harness
//! (`crate::transcription_jobs`), reused rather than stood up a second time.

use std::time::Duration;

use serde_json::{json, Value};

use crate::medical_vocabularies::is_absent;
use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the one clock-owed transition of a vocabulary created or updated
/// during the test to have fallen due: one span of
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The state whose documentation makes the vocabulary usable, and the only one a
/// `DownloadUri` belongs to.
const READY: &str = "READY";

/// One way of asking this service to build a custom vocabulary, with the state its
/// build settles in once the clock makes that outcome due.
struct Row {
    /// What this row is, named in every assertion it fails.
    case: &'static str,
    /// The operation that starts the build.
    create: &'static str,
    /// The family's own reader.
    get: &'static str,
    /// The request body the build is started with.
    request: Value,
    /// The `VocabularyState` the build settles in.
    settled_state: &'static str,
}

/// The class of build a custom-vocabulary request can start, one row per axis the rule
/// generalises over: which of the two sources the terms come from (the list in
/// `Phrases`, the file at `VocabularyFileUri`), how many terms there are, which of the
/// model's `LanguageCode` values they are in, whether the request carries tags, and
/// which of the two vocabulary families — non-medical or medical — serves it. The
/// state each build settles in differs across the rows, which is exactly what the
/// reported member must follow.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`), including ones the issue did
/// not cite; the URI schemes are the ones `VocabularyFileUri`'s `@pattern` admits,
/// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
/// operation="CreateVocabulary")`). A medical vocabulary is in `en-US`, "the only
/// language supported with Amazon Transcribe Medical" (the model's doc for
/// `CreateMedicalVocabularyRequest$LanguageCode`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one inline term, en-US",
            create: "CreateVocabulary",
            get: "GetVocabulary",
            request: json!({
                "VocabularyName": "pending-one-term",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            settled_state: READY,
        },
        Row {
            case: "several inline terms and tags, fr-FR",
            create: "CreateVocabulary",
            get: "GetVocabulary",
            request: json!({
                "VocabularyName": "pending.several-terms",
                "LanguageCode": "fr-FR",
                "Phrases": ["Zèbre", "Gnou", "Amazon Transcribe"],
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
            settled_state: READY,
        },
        Row {
            case: "an inline term at the published 256-character ceiling, de-DE",
            create: "CreateVocabulary",
            get: "GetVocabulary",
            request: json!({
                "VocabularyName": "pending_long_term",
                "LanguageCode": "de-DE",
                "Phrases": ["a".repeat(256)]
            }),
            settled_state: READY,
        },
        Row {
            case: "terms from an s3 file, en-GB",
            create: "CreateVocabulary",
            get: "GetVocabulary",
            request: json!({
                "VocabularyName": "pending-from-s3",
                "LanguageCode": "en-GB",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
            settled_state: "FAILED",
        },
        Row {
            case: "terms from an https file, ja-JP",
            create: "CreateVocabulary",
            get: "GetVocabulary",
            request: json!({
                "VocabularyName": "pending-from-https",
                "LanguageCode": "ja-JP",
                "VocabularyFileUri": "https://example-bucket.s3.amazonaws.com/vocab.txt"
            }),
            settled_state: "FAILED",
        },
        Row {
            case: "a medical vocabulary from an s3 table file, en-US",
            create: "CreateMedicalVocabulary",
            get: "GetMedicalVocabulary",
            request: json!({
                "VocabularyName": "pending-medical",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-medical-vocab.txt"
            }),
            settled_state: "FAILED",
        },
    ]
}

/// The `VocabularyName` a row names.
fn name_of(request: &Value) -> String {
    request["VocabularyName"]
        .as_str()
        .expect("every row names a vocabulary")
        .to_string()
}

/// The rule itself, over one answer: a `DownloadUri` is reported exactly when the
/// vocabulary this answer describes is `READY`, and a reported one names this
/// vocabulary's own location.
fn assert_download_uri_matches_state(reply: &Reply, case: &str, name: &str) {
    let state = reply.member(case, "VocabularyState");
    let reported = reply.body.get("DownloadUri").filter(|v| !v.is_null());
    assert_eq!(
        reported.is_some(),
        state == READY,
        "case {case:?}: a DownloadUri belongs to a BUILT vocabulary, so it is reported \
         exactly when the state is {READY} — state {state:?}, got {:?}",
        reply.body
    );
    if let Some(uri) = reported.and_then(Value::as_str) {
        assert!(
            (uri.starts_with("s3://") || uri.starts_with("http")) && uri.ends_with(name),
            "case {case:?}: a reported DownloadUri must name this vocabulary's own \
             location, got {uri:?}"
        );
    }
}

/// A build that has not finished reports no `DownloadUri`, and one that finished
/// successfully does — whichever source its terms came from and whichever family
/// serves it.
#[tokio::test]
async fn a_download_uri_is_reported_only_once_the_build_has_finished() {
    let transcribe = Transcribe::new();
    let rows = rows();

    for row in &rows {
        let name = name_of(&row.request);
        let created = transcribe.call(row.create, row.request.clone()).await;
        created.ok(row.case);
        assert_eq!(
            created.member(row.case, "VocabularyState"),
            "PENDING",
            "case {:?}: a vocabulary whose build just started is PENDING, got {:?}",
            row.case,
            created.body
        );
        // The create's own answer models no `DownloadUri` member at all, so it never
        // reaches the wire however the build turns out.
        assert!(
            is_absent(&created, "DownloadUri"),
            "case {:?}: the create's answer models no DownloadUri, got {:?}",
            row.case,
            created.body
        );

        // Read while the service is still processing the terms: nothing is built, so
        // there is nothing to download.
        let pending = transcribe.call(row.get, json!({"VocabularyName": &name})).await;
        pending.ok(row.case);
        assert_eq!(
            pending.member(row.case, "VocabularyState"),
            "PENDING",
            "case {:?}: reading a vocabulary must not finish its processing, got {:?}",
            row.case,
            pending.body
        );
        assert_download_uri_matches_state(&pending, row.case, &name);
    }

    // The transition the clock owed every one of these builds has now fallen due.
    std::thread::sleep(PAST_THE_TRANSITION);

    for row in &rows {
        let name = name_of(&row.request);
        let settled = transcribe.call(row.get, json!({"VocabularyName": &name})).await;
        settled.ok(row.case);
        assert_eq!(
            settled.member(row.case, "VocabularyState"),
            row.settled_state,
            "case {:?}: the build must settle in {}, got {:?}",
            row.case,
            row.settled_state,
            settled.body
        );
        assert_download_uri_matches_state(&settled, row.case, &name);

        // A read changes nothing, so a second one answers the same.
        let again = transcribe.call(row.get, json!({"VocabularyName": &name})).await;
        again.ok(row.case);
        assert_eq!(
            again.body.get("DownloadUri"),
            settled.body.get("DownloadUri"),
            "case {:?}: reading a vocabulary twice must report the same location",
            row.case
        );
    }
}

/// A REBUILD withdraws the location until it finishes: an update puts the vocabulary
/// back to `PENDING`, where there is nothing built to download again, and the member
/// returns only if that rebuild settles `READY`.
#[tokio::test]
async fn a_rebuild_withdraws_the_download_uri_until_it_finishes() {
    let transcribe = Transcribe::new();
    let name = "rebuilt";
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
        .ok("create the vocabulary to rebuild");
    std::thread::sleep(PAST_THE_TRANSITION);

    let built = transcribe
        .call("GetVocabulary", json!({"VocabularyName": name}))
        .await;
    built.ok("the first build");
    let first_uri = built.member("the first build", "DownloadUri");
    assert_download_uri_matches_state(&built, "the first build", name);

    // Each row is one `UpdateVocabulary` and the state its rebuild settles in. The
    // rows alternate between the two term sources, so the member is withdrawn and
    // restored in both directions and nothing here is keyed to one request's shape.
    let updates: Vec<(&str, Value, &str)> = vec![
        (
            "rebuilt from other inline terms, in another language",
            json!({
                "VocabularyName": name,
                "LanguageCode": "fr-FR",
                "Phrases": ["Zèbre", "Gnou"]
            }),
            READY,
        ),
        (
            "rebuilt from an s3 file",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
            "FAILED",
        ),
        (
            "rebuilt from one inline term again",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            READY,
        ),
        (
            "rebuilt from an http file",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-GB",
                "VocabularyFileUri": "http://example-bucket.s3.amazonaws.com/vocab.txt"
            }),
            "FAILED",
        ),
    ];

    for (case, request, settled_state) in updates {
        let updated = transcribe.call("UpdateVocabulary", request).await;
        updated.ok(case);
        assert_eq!(
            updated.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: a rebuilt vocabulary is PENDING again, got {:?}",
            updated.body
        );
        assert!(
            is_absent(&updated, "DownloadUri"),
            "case {case:?}: the update's answer models no DownloadUri, got {:?}",
            updated.body
        );

        let rebuilding = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        rebuilding.ok(case);
        assert_eq!(
            rebuilding.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: the rebuild must still be running, got {:?}",
            rebuilding.body
        );
        assert_download_uri_matches_state(&rebuilding, case, name);

        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        settled.ok(case);
        assert_eq!(
            settled.member(case, "VocabularyState"),
            settled_state,
            "case {case:?}: the rebuild must settle in {settled_state}, got {:?}",
            settled.body
        );
        assert_download_uri_matches_state(&settled, case, name);
        if settled_state == READY {
            assert_eq!(
                settled.member(case, "DownloadUri"),
                first_uri,
                "case {case:?}: a rebuilt vocabulary is stored in the same place as \
                 before, got {:?}",
                settled.body
            );
        }
    }
}
