// SPDX-License-Identifier: CC0-1.0

//! The STATE precondition of the updates that overwrite a custom vocabulary, over the
//! wire: "Your custom vocabulary must be in a terminal state (READY or FAILED) before
//! you can update it" (`smithy_query(query="doc", shape="UpdateVocabulary")`).
//!
//! THE RULE THESE ROWS ASSERT: an update that would overwrite a custom vocabulary
//! whose build has NOT finished is refused with `BadRequestException` (HTTP 400) and
//! stores nothing — the build the service is still running keeps its own terms,
//! language and outcome — while the SAME request is served once that build has
//! finished, in either terminal state the model publishes. `PENDING` is the one
//! non-terminal value of the `VocabularyState` enum
//! (`smithy_query(query="shape", shape="VocabularyState")`), and the refusal is the
//! one this service already answers for the same records in the same state: "Your
//! request didn't pass one or more validation tests. This can occur when the entity
//! you're trying to delete doesn't exist or if it's in a non-terminal state"
//! (`BadRequestException`'s own documentation).
//!
//! The rows span both families whose records carry a `VocabularyState` — the custom
//! vocabularies and the custom MEDICAL vocabularies — each terminal state a build can
//! settle in, both alternatives a request may name as the source of a vocabulary's
//! terms, several entries of the model's `LanguageCode` enum and several forms of
//! `VocabularyName`, including one at the member's own 200-character ceiling.
//!
//! The second test pins the other half of the rule: the update reaches the record
//! through the family's clock-materialising lookup, so a vocabulary whose build fell
//! due while nobody was looking is updated rather than refused for a state it had
//! already left — nothing reads those records between the create and the update.
//!
//! What this suite pins that `crate::vocabulary_update_term_source` and
//! `crate::medical_vocabulary_updates` do not: those suites deliberately wait out the
//! build before updating, so neither of them calls an update while a build is running.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.

use serde_json::{json, Value};

use crate::medical_vocabularies::{is_absent, PAST_THE_TRANSITION};
use crate::transcription_jobs::Transcribe;

/// The two families whose stored records carry a `VocabularyState`, and whose updates
/// therefore have a state to honour.
#[derive(Debug, Clone, Copy)]
enum Family {
    /// `CreateVocabulary` / `UpdateVocabulary` / `GetVocabulary`.
    Vocabulary,
    /// `CreateMedicalVocabulary` / `UpdateMedicalVocabulary` /
    /// `GetMedicalVocabulary`.
    Medical,
}

impl Family {
    fn create_op(self) -> &'static str {
        match self {
            Family::Vocabulary => "CreateVocabulary",
            Family::Medical => "CreateMedicalVocabulary",
        }
    }

    fn update_op(self) -> &'static str {
        match self {
            Family::Vocabulary => "UpdateVocabulary",
            Family::Medical => "UpdateMedicalVocabulary",
        }
    }

    fn get_op(self) -> &'static str {
        match self {
            Family::Vocabulary => "GetVocabulary",
            Family::Medical => "GetMedicalVocabulary",
        }
    }
}

/// One of the two alternatives a request may name as the source of a vocabulary's
/// terms. A MEDICAL vocabulary models only the file one.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The terms are listed in the request's `Phrases`.
    Inline(&'static [&'static str]),
    /// The terms are in the file at the request's `VocabularyFileUri`. The schemes
    /// below are the ones the member's own `@pattern` admits —
    /// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
    /// operation="UpdateVocabulary,UpdateMedicalVocabulary")`).
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

    /// The terminal state a build from this source settles in: a vocabulary built from
    /// the terms the request listed is `READY`, while one whose terms were to be READ
    /// from a file ends `FAILED` — no Amazon S3 is mounted beside the service in this
    /// suite, so nothing serves the file and nothing loaded its terms.
    fn settles(self) -> &'static str {
        match self {
            Source::Inline(_) => "READY",
            Source::File(_) => "FAILED",
        }
    }

    /// The file this source names, for the rows that assert whose file the stored
    /// record's failure reason names.
    fn file(self) -> Option<&'static str> {
        match self {
            Source::Inline(_) => None,
            Source::File(uri) => Some(uri),
        }
    }
}

/// One row: a vocabulary created from one source and updated — while its build is
/// still running — to another.
struct Row {
    case: &'static str,
    family: Family,
    name: String,
    /// The language the create named, and where its terms came from.
    created_from: (&'static str, Source),
    /// The language the update names, and where it says the terms come from. Chosen so
    /// an update that LANDED would be visible in the record: a different language, and
    /// a source that settles in the other terminal state.
    updated_from: (&'static str, Source),
}

/// The class of request this rule covers, one row per axis it generalises over: which
/// family the record belongs to, which terminal state its build settles in, which
/// alternative names its terms before and after the update, which `LanguageCode` it is
/// in, and what form its name takes.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`), and a MEDICAL row is in US
/// English because that is the only language Amazon Transcribe Medical supports (model
/// doc for `UpdateMedicalVocabularyRequest$LanguageCode`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a vocabulary built from inline terms, updated to a file",
            family: Family::Vocabulary,
            name: "pending-inline-to-file".to_string(),
            created_from: ("en-US", Source::Inline(&["Aardvark", "Zebra"])),
            updated_from: (
                "fr-FR",
                Source::File("s3://DOC-EXAMPLE-BUCKET/replacement-vocab-file.txt"),
            ),
        },
        Row {
            case: "a vocabulary built from an s3 file, updated to inline terms, on a \
                   dotted mixed-case name",
            family: Family::Vocabulary,
            name: "Pending.File-To_Inline".to_string(),
            created_from: ("en-GB", Source::File("s3://DOC-EXAMPLE-BUCKET/created.txt")),
            updated_from: ("es-US", Source::Inline(&["Hola", "Mundo"])),
        },
        Row {
            // `VocabularyName`'s own `length {min 1, max 200}`
            // (`smithy_query(query="operation", operation="UpdateVocabulary")`).
            case: "a vocabulary built from an https file, updated to inline terms, on a \
                   name at the modelled 200-character ceiling",
            family: Family::Vocabulary,
            name: "z".repeat(200),
            created_from: (
                "de-DE",
                Source::File("https://example-bucket.s3.amazonaws.com/created.txt"),
            ),
            updated_from: ("de-CH", Source::Inline(&["Aardvark"])),
        },
        Row {
            case: "a medical vocabulary built from an s3 file, updated to an https file",
            family: Family::Medical,
            name: "clinical-pending".to_string(),
            created_from: ("en-US", Source::File("s3://DOC-EXAMPLE-BUCKET/clinical.txt")),
            updated_from: (
                "en-US",
                Source::File("https://example-bucket.s3.amazonaws.com/clinical-v2.txt"),
            ),
        },
        Row {
            case: "a medical vocabulary built from an http file, updated to an s3 file, \
                   on an underscored name",
            family: Family::Medical,
            name: "Oncology_terms.v2".to_string(),
            created_from: (
                "en-US",
                Source::File("http://example-bucket.s3.amazonaws.com/oncology.txt"),
            ),
            updated_from: (
                "en-US",
                Source::File("s3://DOC-EXAMPLE-BUCKET/oncology-v2.txt"),
            ),
        },
    ]
}

/// The request members a create or an update of `name` carries.
fn request(name: &str, language: &str, source: Source) -> Value {
    let mut request = json!({"VocabularyName": name, "LanguageCode": language});
    for (member, value) in source
        .members()
        .as_object()
        .expect("the source members are an object")
        .clone()
    {
        request[member] = value;
    }
    request
}

/// An update issued while the build is still running is refused and stores nothing,
/// and the same update is served once that build has finished.
#[tokio::test]
async fn an_update_of_a_vocabulary_being_built_is_refused() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let case = row.case;
        let (created_language, created_from) = row.created_from;
        let (updated_language, updated_from) = row.updated_from;

        // The create opens the window its build runs in
        // (`crate::state::VOCABULARY_TRANSITION_DELAY`), and the update below is the
        // very next call, so it is issued inside that window.
        let created = transcribe
            .call(
                row.family.create_op(),
                request(&row.name, created_language, created_from),
            )
            .await;
        created.ok(case);
        assert_eq!(
            created.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: a fresh vocabulary's build is still running, got {:?}",
            created.body
        );
        let created_time = created.timestamp(case, "LastModifiedTime");

        let refused = transcribe
            .call(
                row.family.update_op(),
                request(&row.name, updated_language, updated_from),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        for expected in [row.name.as_str(), "PENDING", "can't be updated"] {
            assert!(
                message.contains(expected),
                "case {case:?}: the refusal must name {expected:?}, got {message:?}"
            );
        }

        // The refused update stored nothing: the record still carries the language the
        // create named, at the instant the create stored it.
        let during = transcribe
            .call(row.family.get_op(), json!({"VocabularyName": row.name}))
            .await;
        during.ok(case);
        assert_eq!(
            during.member(case, "LanguageCode"),
            created_language,
            "case {case:?}: a refused update must leave the stored language alone, got {:?}",
            during.body
        );
        // Asserted only while the reader still reports the build as running: the
        // transition falls due a fixed span after the create, so a machine slow enough
        // for that span to pass between these calls legitimately reports the settled
        // record here — which the settle assertions below are what cover.
        if during.member(case, "VocabularyState") == "PENDING" {
            let during_time = during.timestamp(case, "LastModifiedTime");
            assert_eq!(
                during_time, created_time,
                "case {case:?}: a refused update must not move the record's \
                 LastModifiedTime off the create's own {created_time}, got {during_time}"
            );
        }

        // The build the refused update would have discarded finishes on the CREATE's
        // own terms: its outcome, and the file its failure reason names, are the
        // create's and never the update's.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = transcribe
            .call(row.family.get_op(), json!({"VocabularyName": row.name}))
            .await;
        settled.ok(case);
        assert_eq!(
            (
                settled.member(case, "VocabularyState"),
                settled.member(case, "LanguageCode"),
            ),
            (
                created_from.settles().to_string(),
                created_language.to_string()
            ),
            "case {case:?}: the in-flight build must settle on the create's own values, \
             got {:?}",
            settled.body
        );
        match created_from.file() {
            Some(file) => {
                let reason = settled.member(case, "FailureReason");
                assert!(
                    reason.contains(file),
                    "case {case:?}: the failure reason must name the file the create \
                     gave ({file}), got {reason:?}"
                );
                if let Some(refused_file) = updated_from.file() {
                    assert!(
                        !reason.contains(refused_file),
                        "case {case:?}: the refused update's file ({refused_file}) must \
                         not reach the stored record, got {reason:?}"
                    );
                }
            }
            None => assert!(
                is_absent(&settled, "FailureReason"),
                "case {case:?}: a vocabulary built from the terms the request listed has \
                 not failed, got {:?}",
                settled.body
            ),
        }

        // And the SAME request the running build refused is served now that build has
        // finished: the refusal is about the state, not about the request.
        let served = transcribe
            .call(
                row.family.update_op(),
                request(&row.name, updated_language, updated_from),
            )
            .await;
        served.ok_status(case, 200);
        assert_eq!(
            (
                served.member(case, "VocabularyName"),
                served.member(case, "LanguageCode"),
                served.member(case, "VocabularyState"),
            ),
            (
                row.name.clone(),
                updated_language.to_string(),
                "PENDING".to_string()
            ),
            "case {case:?}: the update of a settled vocabulary must answer the record it \
             stored, got {:?}",
            served.body
        );
    }
}

/// A vocabulary whose build fell due while nobody read it is updated, not refused: the
/// update's own lookup materialises the transition the clock already owed the record.
#[tokio::test]
async fn an_update_sees_the_transition_the_clock_already_owed() {
    let transcribe = Transcribe::new();
    let rows = rows();

    // Every row's create first, so the one wait for the builds to fall due is shared —
    // and NOTHING reads these records between the create and the update below.
    for row in &rows {
        let (created_language, created_from) = row.created_from;
        transcribe
            .call(
                row.family.create_op(),
                request(&row.name, created_language, created_from),
            )
            .await
            .ok(row.case);
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for row in &rows {
        let case = row.case;
        let (updated_language, updated_from) = row.updated_from;
        let served = transcribe
            .call(
                row.family.update_op(),
                request(&row.name, updated_language, updated_from),
            )
            .await;
        served.ok_status(case, 200);
        assert_eq!(
            served.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: the update's rebuild starts again, so the record it answers \
             is PENDING, got {:?}",
            served.body
        );

        // The record the update stored is the request's, read back through the family's
        // own reader.
        let read_back = transcribe
            .call(row.family.get_op(), json!({"VocabularyName": row.name}))
            .await;
        read_back.ok(case);
        assert_eq!(
            read_back.member(case, "LanguageCode"),
            updated_language,
            "case {case:?}: the reader must report the language the update named, got {:?}",
            read_back.body
        );
    }
}
