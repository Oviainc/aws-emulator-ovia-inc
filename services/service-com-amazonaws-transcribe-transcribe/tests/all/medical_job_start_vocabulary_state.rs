// SPDX-License-Identifier: CC0-1.0

//! The STATE a custom MEDICAL vocabulary has to be in for a medical transcription job
//! to apply it, end to end over the wire.
//!
//! THE RULE UNDER TEST: a start may apply a custom vocabulary only once the service has
//! finished building it successfully, in EVERY family that carries a `VocabularyState` —
//! `READY` is that state for the medical family too ("If the state is READY, you can use
//! the custom vocabulary in a `StartMedicalTranscriptionJob` request",
//! `smithy_query(query="doc", shape="GetMedicalVocabularyResponse",
//! member="VocabularyState")`) — so a start naming one whose build is still running
//! (`PENDING`) or whose build produced no terms (`FAILED`) is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores no job, while the same start, once the
//! named vocabulary has settled `READY`, is served and the job reports the vocabulary it
//! was started with. The non-medical half of the same rule is
//! `crate::job_start_vocabulary_state`'s subject.
//!
//! The rule is asserted over a table spanning what it generalises over: the name forms
//! the member's `@pattern`/`@length` admit, how the vocabulary's table file is addressed
//! (a key at the bucket root, a key under a nested prefix), what that file holds (one
//! term per line, a tab-separated vocabulary table, CRLF line endings), both states that
//! are not `READY`, and — for the state a build that loaded nothing settles in — each way
//! a build loads no terms from a file the request may name (a URI naming a host rather
//! than an object, which no object store is asked about, and an object the store does
//! serve that holds no terms, only blank lines, or no text at all). A request whose
//! object the store reports it does not hold never reaches a build at all: it is refused
//! outright, which is `crate::vocabulary_file_names_an_object`'s subject.
//!
//! This family's terms can come from nowhere but the table file at its
//! `VocabularyFileUri`, so every row runs on the harness that mounts the emulator's own
//! Amazon S3 beside the service (`crate::transcription_jobs::Transcribe::with_object_store`),
//! which is also what makes a `READY` record of this family reachable at all.

use serde_json::{json, Value};

use crate::transcription_jobs::{
    not_usable_message as state_refusal, Reply, Transcribe, VOCABULARY_TABLE_BUCKET,
};

/// The media every start below is pointed at, staged in the same object store: with a
/// store to ask, a start whose media it does not hold is refused for its media, which is
/// `crate::media_object_resolution`'s subject.
const MEDIA_URI: &str = "s3://example-media/dictation.wav";

/// The state a build that loaded its terms settles in, and the one a job may apply the
/// vocabulary in.
const READY: &str = "READY";
/// The state a build that loaded none settles in.
const FAILED: &str = "FAILED";
/// The state a build that has not fallen due yet is in.
const PENDING: &str = "PENDING";

/// One row of [`served_rows`]: a name form of the member, the key its table file is
/// staged at and the bytes that file holds.
struct Served {
    case: &'static str,
    vocabulary: String,
    key: &'static str,
    table: &'static [u8],
}

/// A table file's forms, paired with the name forms `VocabularyName` admits
/// (`@pattern ^[0-9a-zA-Z._-]+$`, `@length(min: 1, max: 200)` —
/// `smithy_query(query="operation", operation="CreateMedicalVocabulary")`), including
/// both published length bounds, so no row is pinned to one spelling or one file shape.
fn served_rows() -> Vec<Served> {
    vec![
        Served {
            case: "a table file of one term per line",
            vocabulary: "clinic-terms".to_string(),
            key: "terms.txt",
            table: b"biopsy\ncarcinoma\nelectrocardiogram\n",
        },
        Served {
            case: "a tab-separated vocabulary table under a nested prefix",
            vocabulary: "a".to_string(),
            key: "nested/tables/medical-table.txt",
            // The `Phrase IPA SoundsLike DisplayAs` column header
            // `CreateMedicalVocabulary` documents for a vocabulary table, and one row.
            table: b"Phrase\tIPA\tSoundsLike\tDisplayAs\n\
                     Tympanic-membrane\t\t\tTympanic membrane\n",
        },
        Served {
            case: "a table file with CRLF line endings and a blank line",
            vocabulary: "clinic.terms_9".to_string(),
            key: "windows-line-endings.txt",
            table: b"alpha\r\n\r\nbeta\r\n",
        },
        Served {
            case: "a single-term table file with no trailing newline",
            vocabulary: "9".to_string(),
            key: "single-term.txt",
            table: b"sphygmomanometer",
        },
        Served {
            case: "a table file for a name at the published length ceiling",
            vocabulary: "z".repeat(200),
            key: "ceiling-name-table.txt",
            table: b"myocardial-infarction\n",
        },
    ]
}

/// One row of [`unserved_rows`]: a name form, and the `VocabularyFileUri` its create
/// names — one a build loads no terms from, in each way that can happen to a file the
/// request may name.
struct Unserved {
    case: &'static str,
    vocabulary: String,
    uri: String,
}

/// The objects this suite stages for [`unserved_rows`]: the store ANSWERS for each of
/// them, and there is still nothing to build from.
const UNSERVED_OBJECTS: &[(&str, &[u8])] = &[
    ("empty.txt", b""),
    ("blank-lines.txt", b"\n\n   \n"),
    ("not-text.bin", &[0xff, 0xfe, 0x00, 0x01]),
];

/// Each way a build loads no terms from a file the request may name, one per row, on
/// name forms the served rows do not use.
fn unserved_rows() -> Vec<Unserved> {
    vec![
        Unserved {
            case: "a URI naming a host rather than an object",
            vocabulary: "hosted.table_7".to_string(),
            uri: "https://example.invalid/vocabulary-table.txt".to_string(),
        },
        Unserved {
            case: "an object the store serves that holds no terms",
            vocabulary: "y".repeat(200),
            uri: format!("s3://{VOCABULARY_TABLE_BUCKET}/empty.txt"),
        },
        Unserved {
            case: "an object the store serves that holds only blank lines",
            vocabulary: "T".to_string(),
            uri: format!("s3://{VOCABULARY_TABLE_BUCKET}/blank-lines.txt"),
        },
        Unserved {
            case: "an object the store serves that is not UTF-8 text",
            vocabulary: "absent-table".to_string(),
            uri: format!("s3://{VOCABULARY_TABLE_BUCKET}/not-text.bin"),
        },
    ]
}

/// How this service refuses a start that names a custom medical vocabulary it cannot
/// apply: the name the request asked for and the state this account holds that
/// vocabulary in.
fn not_usable_message(name: &str, state: &str) -> String {
    state_refusal("medical vocabulary", name, state, READY)
}

/// A `StartMedicalTranscriptionJob` request for the job `job` applying the custom
/// medical vocabulary `vocabulary`.
fn start_request(job: &str, vocabulary: &str) -> Value {
    json!({
        "MedicalTranscriptionJobName": job,
        // "US English (en-US) is the only valid value for medical transcription jobs"
        // (the model's doc for this request's LanguageCode).
        "LanguageCode": "en-US",
        "Media": {"MediaFileUri": MEDIA_URI},
        "MediaFormat": "wav",
        "OutputBucketName": "example-medical-output",
        "Specialty": "PRIMARYCARE",
        "Type": "DICTATION",
        "Settings": {"VocabularyName": vocabulary}
    })
}

/// `CreateMedicalVocabulary` for `vocabulary` from the table file at `uri`, which
/// leaves the record `PENDING` until its build falls due.
async fn create(transcribe: &Transcribe, vocabulary: &str, uri: &str) -> Reply {
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": vocabulary,
                "LanguageCode": "en-US",
                "VocabularyFileUri": uri
            }),
        )
        .await
}

/// `GetMedicalTranscriptionJob` for `job`, so a refused start can be shown to have
/// stored nothing.
async fn read_job(transcribe: &Transcribe, job: &str) -> Reply {
    transcribe
        .call(
            "GetMedicalTranscriptionJob",
            json!({"MedicalTranscriptionJobName": job}),
        )
        .await
}

/// An emulator with the object store beside the service and the media every start names
/// already staged.
async fn emulator() -> Transcribe {
    let transcribe = Transcribe::with_object_store();
    transcribe.given_media_objects(&[MEDIA_URI]).await;
    transcribe
}

#[tokio::test]
async fn a_start_naming_a_medical_vocabulary_still_being_built_is_refused_and_served_once_ready() {
    for (row, served) in served_rows().iter().enumerate() {
        // A fresh emulator per row, so a vocabulary one row built cannot be what makes
        // the next row's start succeed.
        let transcribe = emulator().await;
        let case = served.case;
        let vocabulary = &served.vocabulary;

        transcribe
            .given_object(VOCABULARY_TABLE_BUCKET, served.key, served.table)
            .await;
        let created = create(
            &transcribe,
            vocabulary,
            &format!("s3://{VOCABULARY_TABLE_BUCKET}/{}", served.key),
        )
        .await;
        let state = created.ok(case)["VocabularyState"].clone();
        assert_eq!(
            state,
            json!(PENDING),
            "case {case:?}: a create leaves the build still running, got {state:?}"
        );

        // Just created, the vocabulary is still being built, so no job may apply it.
        let pending_job = format!("pending-medical-vocabulary-{row}");
        let refused = transcribe
            .call(
                "StartMedicalTranscriptionJob",
                start_request(&pending_job, vocabulary),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_usable_message(vocabulary, PENDING),
            "case {case:?}: the refusal must name the state the vocabulary is in, got \
             {message:?}"
        );

        // A refused start stores nothing, so the job name it asked for is unknown.
        read_job(&transcribe, &pending_job)
            .await
            .refused(case, 400, "BadRequestException");

        // Once the build has finished successfully, the very same start is served and
        // the stored job reports the vocabulary it applies.
        let settled = transcribe.medical_vocabulary_settled(vocabulary).await;
        assert_eq!(
            settled, READY,
            "case {case:?}: a build from a table file the store serves must settle \
             {READY}, got {settled:?}"
        );
        let ready_job = format!("ready-medical-vocabulary-{row}");
        transcribe
            .call(
                "StartMedicalTranscriptionJob",
                start_request(&ready_job, vocabulary),
            )
            .await
            .ok(case);
        let fetched = read_job(&transcribe, &ready_job).await;
        let settings = fetched.ok(case)["MedicalTranscriptionJob"]["Settings"].clone();
        assert_eq!(
            settings,
            json!({"VocabularyName": vocabulary}),
            "case {case:?}: the stored job must report the custom medical vocabulary it \
             was started with, got {settings:?}"
        );
    }
}

#[tokio::test]
async fn a_start_naming_a_medical_vocabulary_whose_build_did_not_produce_terms_is_refused() {
    for (row, unserved) in unserved_rows().iter().enumerate() {
        let transcribe = emulator().await;
        let case = unserved.case;
        let vocabulary = &unserved.vocabulary;

        // The bucket the served rows use exists here too, holding the objects the rows
        // name: the store ANSWERS for each of them, and there is still nothing to build
        // from.
        for (key, bytes) in UNSERVED_OBJECTS {
            transcribe
                .given_object(VOCABULARY_TABLE_BUCKET, key, bytes)
                .await;
        }
        create(&transcribe, vocabulary, &unserved.uri)
            .await
            .ok(case);

        let settled = transcribe.medical_vocabulary_settled(vocabulary).await;
        assert_eq!(
            settled, FAILED,
            "case {case:?}: a build whose table file loaded no terms must settle \
             {FAILED}, got {settled:?}"
        );

        let job = format!("failed-medical-vocabulary-{row}");
        let refused = transcribe
            .call(
                "StartMedicalTranscriptionJob",
                start_request(&job, vocabulary),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_usable_message(vocabulary, FAILED),
            "case {case:?}: the refusal must name the state the vocabulary is in, got \
             {message:?}"
        );
        read_job(&transcribe, &job)
            .await
            .refused(case, 400, "BadRequestException");

        // The failure is terminal: a second start, after the state could only have been
        // re-read, is refused in the same words.
        let again = transcribe
            .call(
                "StartMedicalTranscriptionJob",
                start_request(&job, vocabulary),
            )
            .await;
        again.refused(case, 400, "BadRequestException");
        let repeated = again.error_message(case);
        assert_eq!(
            repeated,
            not_usable_message(vocabulary, FAILED),
            "case {case:?}: a failed build does not become usable by being read, got \
             {repeated:?}"
        );
    }
}
