// SPDX-License-Identifier: CC0-1.0

//! `UpdateMedicalVocabulary` end to end over the wire: a stored custom medical
//! vocabulary is OVERWRITTEN with the values the request carries, and the rebuild
//! that overwrite starts runs again from this call.
//!
//! THE RULE THESE ROWS ASSERT: an update of a medical vocabulary this Region holds
//! replaces the record's language and table-file location with the request's — "This
//! operation overwrites all existing information with your new values; you cannot
//! append new terms onto an existing custom vocabulary"
//! (`smithy_query(query="doc", shape="UpdateMedicalVocabulary")`) — answers 200 with
//! `VocabularyName`, `LanguageCode`, `LastModifiedTime` and `VocabularyState:
//! PENDING`, moves the record's modification instant forward, and leaves the
//! record's identity (its name, its Region, its creation order and its tag set, none
//! of which this operation models an input for) untouched. An update naming a
//! medical vocabulary the Region holds none of is refused, and so is one in any
//! language but US English.
//!
//! WHERE THE REBUILD LANDS: a medical vocabulary's terms can only come from the
//! table file at `VocabularyFileUri`, which this operation models `@required` with no
//! inline alternative, and no Amazon S3 is mounted beside the service in this suite,
//! so no file any row names is served — the rebuild therefore ends in the service's
//! own modelled `FAILED` state with a `FailureReason` naming the file the UPDATE gave,
//! never the one it replaced. A rebuild from a file the object store DOES serve is
//! `crate::vocabulary_file_objects`' subject.
//!
//! What this suite pins that `crate::medical_vocabularies` does not: the family's
//! update path. The create's own form, the listing, the paging and the tagging
//! operations are that suite's subjects, and the reader's is
//! `crate::get_medical_vocabulary_reads`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.

use serde_json::json;

use crate::medical_vocabularies::{
    is_absent, listed_names, medical_vocabulary_arn, MEDICAL_LANGUAGE, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::Transcribe;

/// One row: a medical vocabulary created from one table file, then updated to name
/// another.
struct Row {
    case: &'static str,
    name: String,
    /// The table file the create named — the one the update replaces.
    created_file: &'static str,
    /// The table file the update names.
    updated_file: &'static str,
    /// The tags the create named, which the update must leave alone.
    tags: Vec<serde_json::Value>,
}

/// The class of `UpdateMedicalVocabulary` request this rule covers, one row per axis
/// it generalises over: the form of the vocabulary name, which URI scheme names the
/// table file before and after the update, and whether the record carries tags the
/// update must not disturb.
///
/// The schemes are the ones `VocabularyFileUri`'s own `@pattern` admits —
/// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
/// operation="UpdateMedicalVocabulary")`) — so the rows cover each alternative of
/// that catalog in both positions, including ones no recorded request used.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an s3 file replaced by another s3 file",
            name: "clinical-terms".to_string(),
            created_file: "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt",
            updated_file: "s3://DOC-EXAMPLE-BUCKET/my-vocab-file-v2.txt",
            tags: Vec::new(),
        },
        Row {
            case: "an s3 file replaced by an https file, on a dotted mixed-case name",
            name: "Cardiology.v2".to_string(),
            created_file: "s3://DOC-EXAMPLE-BUCKET/cardiology.txt",
            updated_file: "https://example-bucket.s3.amazonaws.com/cardiology.txt",
            tags: Vec::new(),
        },
        Row {
            case: "an https file replaced by an http file, on an underscored name with tags",
            name: "oncology_terms".to_string(),
            created_file: "https://example-bucket.s3.amazonaws.com/oncology.txt",
            updated_file: "http://example-bucket.s3.amazonaws.com/oncology-v2.txt",
            tags: vec![json!({"Key": "team", "Value": "clinical"})],
        },
        Row {
            // `VocabularyName`'s own `length {min 1, max 200}`
            // (`smithy_query(query="operation", operation="UpdateMedicalVocabulary")`).
            case: "an http file replaced by an s3 file, on a name at the modelled \
                   200-character ceiling",
            name: "z".repeat(200),
            created_file: "http://example-bucket.s3.amazonaws.com/long-name-vocab.txt",
            updated_file: "s3://DOC-EXAMPLE-BUCKET/long-name-vocab-v2.txt",
            tags: vec![
                json!({"Key": "env", "Value": "test"}),
                json!({"Key": "team", "Value": "clinical"}),
            ],
        },
    ]
}

/// An update overwrites the stored record, answers it as `PENDING`, and starts the
/// rebuild again — which ends `FAILED` naming the file the UPDATE gave.
#[tokio::test]
async fn an_update_overwrites_the_stored_medical_vocabulary() {
    let transcribe = Transcribe::new();
    let rows = rows();

    // Every row's create, so the one wait for the builds to settle is shared.
    for row in &rows {
        let mut create = json!({
            "VocabularyName": row.name,
            "LanguageCode": MEDICAL_LANGUAGE,
            "VocabularyFileUri": row.created_file
        });
        // `TagList`'s own `length {min 1, max 200}` bounds what a request may carry,
        // so the untagged rows leave the member out rather than sending an empty list.
        if !row.tags.is_empty() {
            create["Tags"] = json!(row.tags);
        }
        transcribe
            .call("CreateMedicalVocabulary", create)
            .await
            .ok(row.case);
    }
    // The order the family's listing reports, before any update: an update names no
    // input that could reorder it.
    let before_order = listed_names(
        &transcribe.call("ListMedicalVocabularies", json!({})).await,
        "the listing before any update",
    );

    std::thread::sleep(PAST_THE_TRANSITION);

    for row in &rows {
        let case = row.case;

        // The record the update is about to overwrite: settled on the create's file.
        let before = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": row.name}))
            .await;
        before.ok(case);
        let name = row.name.as_str();
        assert_eq!(
            before.member(case, "VocabularyState"),
            "FAILED",
            "case {case:?}: the create's build settles FAILED before the update, got {:?}",
            before.body
        );
        let before_time = before.timestamp(case, "LastModifiedTime");

        let updated = transcribe
            .call(
                "UpdateMedicalVocabulary",
                json!({
                    "VocabularyName": row.name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": row.updated_file
                }),
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
                row.name.clone(),
                MEDICAL_LANGUAGE.to_string(),
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
        // an accepted no-op. The facts that belong to the record WHILE it is being
        // rebuilt are asserted only for as long as the reader still reports it as such:
        // the rebuild falls due a fixed span after the update
        // (`crate::state::VOCABULARY_TRANSITION_DELAY`), so a machine slow enough for
        // that span to pass between these two calls legitimately reports the settled
        // state here, which the settle assertions below are what cover.
        let pending = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": row.name}))
            .await;
        pending.ok(case);
        assert_eq!(
            pending.member(case, "LanguageCode"),
            MEDICAL_LANGUAGE,
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

        // The rebuild the update started settles the same way the create's did, on the
        // file the UPDATE named: no object store served it, so there is still nothing
        // to download.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": row.name}))
            .await;
        settled.ok(case);
        assert_eq!(
            settled.member(case, "VocabularyState"),
            "FAILED",
            "case {case:?}: a medical vocabulary whose file no object store served \
             must end FAILED, got {:?}",
            settled.body
        );
        let reason = settled.member(case, "FailureReason");
        assert!(
            reason.contains(row.updated_file) && !reason.contains(row.created_file),
            "case {case:?}: the failure reason must name the file the update gave \
             ({updated}) and not the one it replaced ({created}), got {reason:?}",
            updated = row.updated_file,
            created = row.created_file
        );
        let settled_time = settled.timestamp(case, "LastModifiedTime");
        assert!(
            settled_time > updated_time,
            "case {case:?}: ending the rebuild changed the record again, so its \
             LastModifiedTime must move past the update's own {updated_time}, got \
             {settled_time}"
        );
        assert!(
            is_absent(&settled, "DownloadUri"),
            "case {case:?}: nothing built this vocabulary, so there is no file to \
             download, got {:?}",
            settled.body
        );

        // The record's identity is untouched: the tag set is the one the create named,
        // because this operation models no input that could change it.
        let tags = transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": medical_vocabulary_arn(name)}),
            )
            .await;
        tags.ok(case);
        assert_eq!(
            tags.body["Tags"],
            json!(row.tags),
            "case {case:?}: an update must leave the stored tag set alone, got {:?}",
            tags.body
        );
    }

    // Creation order is part of the record's identity too, so the listing still walks
    // the same vocabularies in the same order.
    assert_eq!(
        listed_names(
            &transcribe.call("ListMedicalVocabularies", json!({})).await,
            "the listing after every update",
        ),
        before_order,
        "an update must not reorder the family's listing"
    );
}

/// An update naming a medical vocabulary this Region holds none of is refused with
/// the service's own not-found wording, and creates nothing.
#[tokio::test]
async fn an_update_of_an_unknown_medical_vocabulary_is_refused() {
    let transcribe = Transcribe::new();

    // One record that DOES exist, created in another Region: a regional endpoint
    // serves only its own Region's resources, so an update elsewhere finds nothing.
    transcribe
        .call_in_region(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": "elsewhere",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/elsewhere.txt"
            }),
            Some("eu-west-1"),
        )
        .await
        .ok("a medical vocabulary in another Region");

    for (case, name) in [
        ("a name nothing was ever created under", "never-created"),
        ("a name only another Region holds", "elsewhere"),
    ] {
        let refused = transcribe
            .call(
                "UpdateMedicalVocabulary",
                json!({
                    "VocabularyName": name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
                }),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            crate::transcription_job_request_rules::not_found_message("medical vocabulary"),
            "case {case:?}: the refusal must be the family's own not-found sentence, \
             got {message:?}"
        );
    }

    // A refused update creates nothing in the Region it was served in.
    let listed = transcribe.call("ListMedicalVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&listed, "after the refused updates"),
        Vec::<String>::new(),
        "a refused update must store nothing, got {:?}",
        listed.body
    );
}

/// An update is accepted in US English only, and a refused one leaves the stored
/// record exactly as it was.
#[tokio::test]
async fn an_update_must_be_us_english() {
    let transcribe = Transcribe::new();
    let created_file = "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt";
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": "language-rule",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": created_file
            }),
        )
        .await
        .ok("the medical vocabulary to update");

    // Entries of the model's own `LanguageCode` enum
    // (`smithy_query(query="shape", shape="LanguageCode")`), deliberately including
    // ones no recorded request used: each is a valid `LanguageCode` that Amazon
    // Transcribe MEDICAL does not support.
    for language in ["en-GB", "es-US", "ja-JP", "af-ZA", "de-DE"] {
        let refused = transcribe
            .call(
                "UpdateMedicalVocabulary",
                json!({
                    "VocabularyName": "language-rule",
                    "LanguageCode": language,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/other-vocab-file.txt"
                }),
            )
            .await;
        refused.refused(language, 400, "BadRequestException");
        let message = refused.error_message(language);
        assert!(
            message.contains(language) && message.contains(MEDICAL_LANGUAGE),
            "case {language:?}: the refusal must name the language asked for and the \
             only one supported, got {message:?}"
        );
    }

    // A refused update stores nothing: the record still names the create's own file.
    std::thread::sleep(PAST_THE_TRANSITION);
    let got = transcribe
        .call(
            "GetMedicalVocabulary",
            json!({"VocabularyName": "language-rule"}),
        )
        .await;
    got.ok("after the refusals");
    let reason = got.member("after the refusals", "FailureReason");
    assert!(
        reason.contains(created_file),
        "a refused update must leave the stored file alone ({created_file}), got {reason:?}"
    );

    // And the one row in the supported language is served.
    transcribe
        .call(
            "UpdateMedicalVocabulary",
            json!({
                "VocabularyName": "language-rule",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/other-vocab-file.txt"
            }),
        )
        .await
        .ok_status("en-US, the supported language", 200);
}
