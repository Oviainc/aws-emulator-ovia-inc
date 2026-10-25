// SPDX-License-Identifier: CC0-1.0

//! `UpdateVocabulary` over a custom vocabulary that has reached a TERMINAL state —
//! the precondition the operation itself publishes: "Your custom vocabulary must be
//! in a terminal state (READY or FAILED) before you can update it. You must include
//! either Phrases or VocabularyFileUri in your request."
//! (`smithy_query(query="operation", operation="UpdateVocabulary")`).
//!
//! THE RULE THESE ROWS ASSERT: an update that names EITHER of the two alternatives
//! the operation publishes for where the vocabulary's terms come from — the list in
//! `Phrases` or the file at `VocabularyFileUri` — is SERVED, not refused: it
//! overwrites all of the record's previous information with the request's, answers
//! 200 with `VocabularyName`, `LanguageCode`, `LastModifiedTime` and
//! `VocabularyState: PENDING`, moves the record's modification instant forward, and
//! starts the rebuild the record's next state follows from. Which alternative the
//! update chose changes only where the rebuild lands: a vocabulary rebuilt from the
//! terms the request listed becomes `READY` with a `DownloadUri`, while one whose
//! terms would have had to be READ from a file ends in the service's own modelled
//! `FAILED` state with a `FailureReason` naming that file — this emulator reads no
//! object store and makes no outbound request, so nothing built it and nothing may
//! report it as usable.
//!
//! What this suite pins that `crate::vocabulary_file_uri` does not: every row starts
//! from a vocabulary whose build has already SETTLED (from each of the two terminal
//! states), reads the whole answer the update returns, and then reads the record back
//! through `GetVocabulary` both before and after the rebuild falls due — so an update
//! that is accepted but leaves the stored record untouched fails here. The create's
//! own file form, the family's listing and its refusals (neither alternative, or both
//! at once) are `crate::vocabulary_file_uri`'s and `crate::vocabularies`' subjects,
//! and the filter family's pair of alternatives is
//! `crate::vocabulary_filter_file_uri`'s.
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

/// Long enough for the one clock-owed transition of a vocabulary created or updated
/// during the test to have fallen due: one span of
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// A valid `DataAccessRoleArn`, which the member's `@pattern` pins to an IAM role ARN
/// (`smithy_query(query="operation", operation="UpdateVocabulary")`).
const DATA_ACCESS_ROLE: &str = "arn:aws:iam::000000000000:role/TranscribeAccessRole";

/// One of the two alternatives a create or an update may name as the source of a
/// vocabulary's terms.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The terms are listed in the request's `Phrases`.
    Inline(&'static [&'static str]),
    /// The terms are in the file at the request's `VocabularyFileUri`. The schemes
    /// used below are the ones the member's own `@pattern` admits —
    /// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
    /// operation="UpdateVocabulary")`).
    File(&'static str),
}

impl Source {
    /// The request members that name this source.
    fn members(self) -> Value {
        match self {
            Source::Inline(phrases) => json!({"Phrases": phrases}),
            Source::File(uri) => json!({"VocabularyFileUri": uri}),
        }
    }

    /// The state the build of a vocabulary from this source settles in once the clock
    /// makes the transition due, and whether such a vocabulary has a file to download.
    fn settles(self) -> (&'static str, bool) {
        match self {
            Source::Inline(_) => ("READY", true),
            Source::File(_) => ("FAILED", false),
        }
    }

    /// The file this source names, for the rows that assert the failure reason names it.
    fn file(self) -> Option<&'static str> {
        match self {
            Source::Inline(_) => None,
            Source::File(uri) => Some(uri),
        }
    }
}

/// One row: a vocabulary created from one source and left to settle, then updated
/// from another.
struct Row {
    case: &'static str,
    name: &'static str,
    /// Where the create's terms came from, and the language it named.
    created_from: (Source, &'static str),
    /// Extra members the create carries beyond its name, language and term source.
    create_extras: Value,
    /// Where the update's terms come from, and the language it names.
    updated_from: (Source, &'static str),
    /// Extra members the update carries beyond its name, language and term source —
    /// the ones an update "overwrites all existing information" with.
    update_extras: Value,
    /// The `DataAccessRoleArn` the record must report after the update, if any: the
    /// update's own, because an update replaces rather than merges.
    role_after_update: Option<&'static str>,
}

/// The class of `UpdateVocabulary` request this rule covers, one row per axis it
/// generalises over: which of the two alternatives the UPDATE names, which one the
/// record was built from before (so each terminal state is the starting point of a
/// row, and each direction of the switch is covered), which URI scheme names the
/// file, which of the model's `LanguageCode` values the vocabulary is in, and whether
/// the update also carries a data-access role and an encryption configuration.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a READY vocabulary rebuilt from an s3 file",
            name: "ready-then-s3-file",
            created_from: (Source::Inline(&["Aardvark", "Zebra"]), "en-US"),
            create_extras: json!({}),
            updated_from: (Source::File("s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"), "en-US"),
            update_extras: json!({}),
            role_after_update: None,
        },
        Row {
            case: "a READY vocabulary rebuilt from an https file, in another language",
            name: "Ready.Then-Https_File",
            created_from: (Source::Inline(&["Aardvark"]), "en-US"),
            create_extras: json!({}),
            updated_from: (
                Source::File("https://example-bucket.s3.amazonaws.com/vocab.txt"),
                "en-GB",
            ),
            update_extras: json!({}),
            role_after_update: None,
        },
        Row {
            case: "a READY vocabulary rebuilt from an http file, with a role and a key",
            name: "ready-then-http-file",
            created_from: (Source::Inline(&["Aardvark"]), "es-US"),
            create_extras: json!({}),
            updated_from: (
                Source::File("http://example-bucket.s3.amazonaws.com/vocab.txt"),
                "es-US",
            ),
            update_extras: json!({
                "DataAccessRoleArn": DATA_ACCESS_ROLE,
                "EncryptionConfiguration": {"KMSKey": "alias/aws/transcribe"}
            }),
            role_after_update: Some(DATA_ACCESS_ROLE),
        },
        Row {
            case: "a FAILED file-backed vocabulary rebuilt from another file",
            name: "failed-then-another-file",
            created_from: (Source::File("s3://DOC-EXAMPLE-BUCKET/ja-vocab-file.txt"), "ja-JP"),
            create_extras: json!({}),
            updated_from: (Source::File("s3://DOC-EXAMPLE-BUCKET/ja-vocab-file-v2.txt"), "ja-JP"),
            update_extras: json!({}),
            role_after_update: None,
        },
        Row {
            case: "a FAILED file-backed vocabulary rebuilt from inline terms",
            name: "failed-then-inline",
            created_from: (Source::File("s3://DOC-EXAMPLE-BUCKET/de-vocab-file.txt"), "de-DE"),
            create_extras: json!({}),
            updated_from: (Source::Inline(&["Aardvark", "Gnu", "Zebra"]), "de-DE"),
            update_extras: json!({}),
            role_after_update: None,
        },
        Row {
            case: "a READY vocabulary whose update drops the role the create named",
            name: "ready-then-role-dropped",
            created_from: (Source::Inline(&["Aardvark"]), "fr-FR"),
            // The one create that names a role, so "an update replaces rather than
            // merges" is observable on the row whose update does not name one.
            create_extras: json!({"DataAccessRoleArn": DATA_ACCESS_ROLE}),
            updated_from: (Source::Inline(&["Zèbre", "Gnou"]), "fr-FR"),
            update_extras: json!({}),
            role_after_update: None,
        },
    ]
}

/// The request body of a call naming `name`, `language`, `source` and `extras`.
fn request(name: &str, language: &str, source: Source, extras: &Value) -> Value {
    let mut body = json!({"VocabularyName": name, "LanguageCode": language});
    for members in [source.members(), extras.clone()] {
        let members = members.as_object().cloned().unwrap_or_default();
        for (key, value) in members {
            body[key] = value;
        }
    }
    body
}

/// Read the record back and assert its state, language and modification instant,
/// returning the `LastModifiedTime` the reader reported.
fn assert_read_back(got: &Reply, case: &str, state: &str, language: &str) -> f64 {
    got.ok(case);
    assert_eq!(
        (
            got.member(case, "VocabularyState"),
            got.member(case, "LanguageCode"),
        ),
        (state.to_string(), language.to_string()),
        "case {case:?}: the reader must report the record the update stored, got {:?}",
        got.body
    );
    got.timestamp(case, "LastModifiedTime")
}

/// An update naming either term source is served: the record is overwritten, the
/// answer carries the stored facts with `VocabularyState: PENDING`, and the rebuild
/// settles where the chosen source says it does.
#[tokio::test]
async fn an_update_naming_either_term_source_rebuilds_a_settled_vocabulary() {
    let transcribe = Transcribe::new();
    let rows = rows();

    // Every row's create, so the one wait for the builds to settle is shared.
    for row in &rows {
        let (source, language) = row.created_from;
        let create = request(row.name, language, source, &row.create_extras);
        let created = transcribe.call("CreateVocabulary", create).await;
        created.ok(row.case);
        assert_eq!(
            created.member(row.case, "VocabularyState"),
            "PENDING",
            "case {case:?}: a fresh vocabulary is PENDING, got {:?}",
            created.body,
            case = row.case
        );
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for row in &rows {
        let case = row.case;
        let (created_source, created_language) = row.created_from;
        let (updated_source, updated_language) = row.updated_from;
        let (settled_before, _) = created_source.settles();
        let (settled_after, has_download_uri) = updated_source.settles();

        // The precondition the operation publishes: the record is in a terminal state.
        let before = transcribe
            .call("GetVocabulary", json!({"VocabularyName": row.name}))
            .await;
        let before_time = assert_read_back(&before, case, settled_before, created_language);

        // The update itself: SERVED, with the whole answer the model declares.
        let updated = transcribe
            .call(
                "UpdateVocabulary",
                request(row.name, updated_language, updated_source, &row.update_extras),
            )
            .await;
        updated.ok_status(case, 200);
        assert_eq!(
            (
                updated.member(case, "VocabularyName"),
                updated.member(case, "LanguageCode"),
                updated.member(case, "VocabularyState"),
            ),
            (
                row.name.to_string(),
                updated_language.to_string(),
                "PENDING".to_string()
            ),
            "case {case:?}: the update must answer the record it stored, PENDING, got {:?}",
            updated.body
        );
        let updated_time = updated.timestamp(case, "LastModifiedTime");
        assert!(
            updated_time > before_time,
            "case {case:?}: the update changed the record, so its LastModifiedTime must \
             move past {before_time}, got {updated_time}"
        );

        // The stored record agrees with that answer: the update is a state change, not
        // an accepted no-op. The language and the role it overwrote hold whatever the
        // rebuild has done since, while the facts that belong to the record WHILE it is
        // being rebuilt are asserted only for as long as the reader still reports it as
        // such: the rebuild falls due a fixed span after the update
        // (`crate::state::VOCABULARY_TRANSITION_DELAY`), so a machine slow enough for
        // that span to pass between these two calls legitimately reports the settled
        // state here, which the settle assertions below are what cover.
        let pending = transcribe
            .call("GetVocabulary", json!({"VocabularyName": row.name}))
            .await;
        pending.ok(case);
        assert_eq!(
            pending.member(case, "LanguageCode"),
            updated_language,
            "case {case:?}: the reader must report the language the update named, got {:?}",
            pending.body
        );
        if pending.member(case, "VocabularyState") == "PENDING" {
            let pending_time = pending.timestamp(case, "LastModifiedTime");
            assert_eq!(
                pending_time, updated_time,
                "case {case:?}: the reader must report the instant the update answered, \
                 got {pending_time}"
            );
            assert!(
                is_absent(&pending, "FailureReason"),
                "case {case:?}: a vocabulary being rebuilt has not failed, got {:?}",
                pending.body
            );
        }
        match row.role_after_update {
            Some(role) => assert_eq!(
                pending.member(case, "DataAccessRoleArn"),
                role,
                "case {case:?}: the update's own data-access role must be stored, got {:?}",
                pending.body
            ),
            None => assert!(
                is_absent(&pending, "DataAccessRoleArn"),
                "case {case:?}: an update overwrites all previous information, so a role \
                 it does not name is gone, got {:?}",
                pending.body
            ),
        }

        // And the rebuild the update started settles where the source it named says.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = transcribe
            .call("GetVocabulary", json!({"VocabularyName": row.name}))
            .await;
        let settled_time = assert_read_back(&settled, case, settled_after, updated_language);
        assert!(
            settled_time > updated_time,
            "case {case:?}: ending the rebuild changed the record again, so its \
             LastModifiedTime must move past the update's own {updated_time}, got \
             {settled_time}"
        );
        assert_eq!(
            !is_absent(&settled, "DownloadUri"),
            has_download_uri,
            "case {case:?}: a vocabulary has a file to download only when this service \
             built it from the terms the request listed, got {:?}",
            settled.body
        );
        match updated_source.file() {
            Some(uri) => {
                let reason = settled.member(case, "FailureReason");
                assert!(
                    reason.contains(uri),
                    "case {case:?}: the failure reason must name the file that was not \
                     read ({uri}), got {reason:?}"
                );
            }
            None => assert!(
                is_absent(&settled, "FailureReason"),
                "case {case:?}: a vocabulary this service built has not failed, got {:?}",
                settled.body
            ),
        }
    }
}
