// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request about a record it holds NOTHING under, end to
//! end over the wire.
//!
//! TWO RULES, both recorded behaviour of the real service:
//!
//! 1. An operation that names an existing record resolves THAT RECORD first: a name
//!    the Region holds nothing under is answered with the family's own absent-record
//!    refusal, whatever else the request carries — the Amazon S3 file an update names
//!    is not resolved on behalf of a resource that is not there. Once the record does
//!    resolve, the request's other inputs are judged as they always were, so the same
//!    update against a record that IS held still gets the refusal its URI earns.
//!
//! 2. The custom MEDICAL vocabulary family words an absent record with the PLAIN
//!    vocabulary sentence — "The requested vocabulary couldn't be found. Check the
//!    vocabulary name and try your request again." — the very text the non-medical
//!    family carries, with no "medical" in either clause and no part of the request in
//!    it. Both families' readers, updates and tagging calls answer it; a neighbouring
//!    family (the vocabulary FILTER) still names itself, so this is one family sharing
//!    one sentence rather than a blanket rewrite.
//!
//! Each rule is asserted over a table spanning what it generalises over: rule 1 over
//! every update operation of this service that takes an Amazon S3 file (the custom
//! vocabulary, custom medical vocabulary and vocabulary filter families); rule 2 over
//! every operation that resolves a medical vocabulary by name or by ARN, crossed with
//! the forms of name `VocabularyName`'s own `@length(1..200)` /
//! `@pattern(^[0-9a-zA-Z._-]+$)` admits.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use std::time::Duration;

use serde_json::{json, Value};

use crate::medical_vocabularies::{medical_vocabulary_arn, MEDICAL_LANGUAGE};
use crate::transcription_jobs::Transcribe;

/// Long enough for the one clock-owed build of a record created here to have fallen
/// due, so an update under test is answered for the record's PRESENCE rather than
/// refused for a non-terminal state: one `crate::state::VOCABULARY_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
const PAST_THE_BUILD: Duration = Duration::from_millis(1_200);

/// The status this service puts on every client error
/// (`crate::scaffolded::wire_status`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The shape every absent-record refusal of this service carries.
const ABSENT_RECORD_SHAPE: &str = "BadRequestException";

/// A `VocabularyFileUri` / `VocabularyFilterFileUri` of the Amazon S3 scheme that
/// addresses no OBJECT at all: it names a bucket, and every example of these members
/// names a bucket and a key (`s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt`, the model's
/// own doc for `CreateMedicalVocabularyRequest$VocabularyFileUri`). No object store has
/// to be asked to see that it names no file, which is why these rows need none.
const URI_NAMING_NO_OBJECT: &str = "s3://DOC-EXAMPLE-BUCKET";

/// The refusal a request naming a file that is not there carries: entirely fixed text,
/// with no part of the request in it.
const NO_SUCH_OBJECT_MESSAGE: &str =
    "The URI that you provided doesn't point to an S3 object. Make sure that the \
     object exists and try your request again.";

/// The absent-record sentence the two custom-vocabulary families share, as the
/// recorded refusal words it.
const ABSENT_VOCABULARY_MESSAGE: &str =
    "The requested vocabulary couldn't be found. Check the vocabulary name and try \
     your request again.";

/// One update operation that takes an Amazon S3 file, with the create that establishes
/// a record for it and the reader that proves one is there.
struct Updatable {
    case: &'static str,
    create: &'static str,
    /// The create that establishes a record named by the argument. Where a family can
    /// carry its content inline it does, so the row needs no object store.
    create_body: fn(&str) -> Value,
    read: &'static str,
    update: &'static str,
    /// The member every operation of this family names the record with.
    name_member: &'static str,
    /// The update request for the record named by the argument, naming
    /// [`URI_NAMING_NO_OBJECT`] as its file.
    update_body: fn(&str) -> Value,
    /// The word this family's own absent-record refusal names it by — the plain
    /// vocabulary noun for BOTH vocabulary families (rule 2).
    absent: &'static str,
}

/// Every update of this service that resolves an Amazon S3 file.
fn updatables() -> Vec<Updatable> {
    vec![
        Updatable {
            case: "UpdateVocabulary",
            create: "CreateVocabulary",
            create_body: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                })
            },
            read: "GetVocabulary",
            update: "UpdateVocabulary",
            name_member: "VocabularyName",
            update_body: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "VocabularyFileUri": URI_NAMING_NO_OBJECT
                })
            },
            absent: ABSENT_VOCABULARY_MESSAGE,
        },
        Updatable {
            case: "UpdateMedicalVocabulary",
            create: "CreateMedicalVocabulary",
            // This family's terms come only from a table file, and no object store is
            // mounted here, so the build settles in the service's own FAILED state —
            // terminal, which is all an update needs of it.
            create_body: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/clinical-terms.txt"
                })
            },
            read: "GetMedicalVocabulary",
            update: "UpdateMedicalVocabulary",
            name_member: "VocabularyName",
            update_body: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": URI_NAMING_NO_OBJECT
                })
            },
            absent: ABSENT_VOCABULARY_MESSAGE,
        },
        Updatable {
            case: "UpdateVocabularyFilter",
            create: "CreateVocabularyFilter",
            create_body: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "LanguageCode": "en-US",
                    "Words": ["crud", "bogus"]
                })
            },
            read: "GetVocabularyFilter",
            update: "UpdateVocabularyFilter",
            name_member: "VocabularyFilterName",
            update_body: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "VocabularyFilterFileUri": URI_NAMING_NO_OBJECT
                })
            },
            absent: "The requested vocabulary filter couldn't be found. Check the \
                     vocabulary filter name and try your request again.",
        },
    ]
}

/// The forms of name the absent half is probed with: every class the name members'
/// shared `@pattern` admits, at both published `@length` bounds.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest allowed name", "a".to_string()),
        ("every allowed punctuation character", "x.y_z-0.9".to_string()),
        ("a plainly spelled name", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "n".repeat(200)),
    ]
}

/// RULE 1: an update of a name the Region holds nothing under is the family's
/// absent-record refusal — not the refusal the file it names would earn — and the same
/// update against a record that IS held still earns that file's refusal.
#[tokio::test]
async fn an_update_resolves_the_record_it_names_before_the_file_it_carries() {
    let transcribe = Transcribe::new();

    for family in updatables() {
        for (name_case, name) in absent_names() {
            let case = format!("{} / {name_case}", family.case);
            let refused = transcribe
                .call(family.update, (family.update_body)(&name))
                .await;
            refused.refused(&case, CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
            let message = refused.error_message(&case);
            assert_eq!(
                message, family.absent,
                "case {case:?}: expected the family's absent-record sentence, got \
                 {message:?}"
            );
            // A refused update mints nothing, so the family's reader still answers
            // "absent" for the name it asked about.
            transcribe
                .call(family.read, json!({family.name_member: name}))
                .await
                .refused(&case, CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
        }
    }

    // The counter-case, through the very same requests: with the record there, the
    // update is answered for the FILE it names.
    for family in updatables() {
        let case = format!("{} / a record that is held", family.case);
        let held = "record-that-is-held";
        transcribe
            .call(family.create, (family.create_body)(held))
            .await
            .ok(&case);
        std::thread::sleep(PAST_THE_BUILD);

        let refused = transcribe
            .call(family.update, (family.update_body)(held))
            .await;
        refused.refused(&case, CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
        let message = refused.error_message(&case);
        assert_eq!(
            message, NO_SUCH_OBJECT_MESSAGE,
            "case {case:?}: a held record's update must be answered for the file it \
             names, got {message:?}"
        );
        // Refused, so nothing was overwritten: the record is still readable.
        let read = transcribe
            .call(family.read, json!({family.name_member: held}))
            .await;
        assert_eq!(
            read.ok(&case)[family.name_member].as_str(),
            Some(held),
            "case {case:?}: a refused update must leave the record readable, got {:?}",
            read.body
        );
    }
}

/// One operation that resolves a medical vocabulary, by name or by the ARN that names
/// one, and the request it does so with.
struct MedicalPath {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
}

/// Every path by which a request of this service names a custom medical vocabulary
/// that has to exist: this family's reader and update, the three tagging operations
/// over its ARN (`arn:aws:transcribe:<region>:<account>:medical-vocabulary/<name>`),
/// and the start whose `Settings.VocabularyName` applies one.
fn medical_paths() -> Vec<MedicalPath> {
    vec![
        MedicalPath {
            case: "GetMedicalVocabulary",
            operation: "GetMedicalVocabulary",
            request: |name| json!({"VocabularyName": name}),
        },
        MedicalPath {
            case: "UpdateMedicalVocabulary",
            operation: "UpdateMedicalVocabulary",
            request: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/clinical-terms.txt"
                })
            },
        },
        MedicalPath {
            case: "ListTagsForResource on a medical-vocabulary ARN",
            operation: "ListTagsForResource",
            request: |name| json!({"ResourceArn": medical_vocabulary_arn(name)}),
        },
        MedicalPath {
            case: "TagResource on a medical-vocabulary ARN",
            operation: "TagResource",
            request: |name| {
                json!({
                    "ResourceArn": medical_vocabulary_arn(name),
                    "Tags": [{"Key": "team", "Value": "clinical"}]
                })
            },
        },
        MedicalPath {
            case: "UntagResource on a medical-vocabulary ARN",
            operation: "UntagResource",
            request: |name| {
                json!({
                    "ResourceArn": medical_vocabulary_arn(name),
                    "TagKeys": ["team"]
                })
            },
        },
        MedicalPath {
            case: "StartMedicalTranscriptionJob applying it",
            operation: "StartMedicalTranscriptionJob",
            request: |name| {
                json!({
                    "MedicalTranscriptionJobName": "job-applying-an-absent-vocabulary",
                    // "US English (en-US) is the only valid value for medical
                    // transcription jobs" (model doc for this request's LanguageCode).
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION",
                    "Settings": {"VocabularyName": name}
                })
            },
        },
    ]
}

/// RULE 2: every path that resolves a custom medical vocabulary words an absent one
/// with the plain vocabulary sentence — the same text the non-medical family's reader
/// carries — and the vocabulary FILTER family still names itself.
#[tokio::test]
async fn an_absent_medical_vocabulary_is_worded_as_the_vocabulary_family_words_one() {
    let transcribe = Transcribe::new();

    for path in medical_paths() {
        for (name_case, name) in absent_names() {
            let case = format!("{} / {name_case}", path.case);
            let refused = transcribe
                .call(path.operation, (path.request)(&name))
                .await;
            refused.refused(&case, CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
            let message = refused.error_message(&case);
            assert_eq!(
                message, ABSENT_VOCABULARY_MESSAGE,
                "case {case:?}: expected the plain vocabulary sentence, got {message:?}"
            );
            assert!(
                !message.contains("medical"),
                "case {case:?}: the medical family's sentence carries no \"medical\", \
                 got {message:?}"
            );
            // The text is the service's own: the name the caller asked about appears
            // nowhere in it. A name of three characters or fewer is skipped, since the
            // sentence contains most short letter runs somewhere.
            assert!(
                name.len() <= 3 || !message.contains(name.as_str()),
                "case {case:?}: the sentence must not echo the requested {name:?}, got \
                 {message:?}"
            );
        }
    }

    // The two vocabulary families answer the IDENTICAL sentence for one absent name,
    // and the family next door answers its own.
    let shared = "absent-in-every-family";
    for (case, operation, request) in [
        (
            "the non-medical reader",
            "GetVocabulary",
            json!({"VocabularyName": shared}),
        ),
        (
            "the medical reader",
            "GetMedicalVocabulary",
            json!({"VocabularyName": shared}),
        ),
    ] {
        let refused = transcribe.call(operation, request).await;
        refused.refused(case, CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
        let message = refused.error_message(case);
        assert_eq!(
            message, ABSENT_VOCABULARY_MESSAGE,
            "case {case:?}: both vocabulary families share one sentence, got {message:?}"
        );
    }
    let filter = transcribe
        .call(
            "GetVocabularyFilter",
            json!({"VocabularyFilterName": shared}),
        )
        .await;
    filter.refused("the filter family", CLIENT_ERROR_STATUS, ABSENT_RECORD_SHAPE);
    let message = filter.error_message("the filter family");
    assert!(
        message.contains("vocabulary filter"),
        "the vocabulary filter family must still name itself, got {message:?}"
    );
}
