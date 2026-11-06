// SPDX-License-Identifier: CC0-1.0

//! The co-dependencies the settings shapes of the job families document, end to end
//! over the wire.
//!
//! THE RULE UNDER TEST: a settings toggle and the value that qualifies it are
//! CO-REQUIRED in both directions — the model documents each direction on the member
//! itself ("If you enable ShowSpeakerLabels in your request, you must also include
//! MaxSpeakerLabels", "If you specify the MaxSpeakerLabels field, you must set the
//! ShowSpeakerLabels field to true", and the same two sentences for ShowAlternatives /
//! MaxAlternatives: `smithy_query(query="shape",
//! shape="Settings,MedicalTranscriptionSetting,MedicalScribeSettings")`) — so a start
//! that satisfies only one side of such a pair is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores nothing, while a start whose pairs are
//! whole (or absent) is accepted and reads its settings back unchanged. A second rule
//! of the same validation class: a `Settings.VocabularyName` must name a custom
//! vocabulary of ITS OWN family that this account holds, so a dangling name is refused
//! rather than stored on a job that would report it back.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_NOTE_TEMPLATE;

use crate::transcription_jobs::Transcribe;

/// A `DataAccessRoleArn` of the form the model's `@pattern` allows, for the Medical
/// Scribe rows (that operation models the member `@required`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TranscribeRole";

/// The two job families whose settings shape carries BOTH documented pairs
/// (`Settings` on `StartTranscriptionJob`, `MedicalTranscriptionSetting` on
/// `StartMedicalTranscriptionJob`): the same rows apply to each, so a rule applied to
/// one operation and not the other fails here.
struct Family {
    /// The family, for the case label.
    case: &'static str,
    /// The operation that starts a job of this family.
    start: &'static str,
    /// A start request for `name`, carrying every member this family requires.
    base: fn(&str) -> Value,
    /// The operation that reads a started job back.
    read: &'static str,
    /// The member a read request names the job in.
    read_name_member: &'static str,
    /// The member a read answer carries the job record in.
    record: &'static str,
}

fn families() -> Vec<Family> {
    vec![
        Family {
            case: "transcription job",
            start: "StartTranscriptionJob",
            base: |name| {
                json!({
                    "TranscriptionJobName": name,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
                })
            },
            read: "GetTranscriptionJob",
            read_name_member: "TranscriptionJobName",
            record: "TranscriptionJob",
        },
        Family {
            case: "medical transcription job",
            start: "StartMedicalTranscriptionJob",
            base: |name| {
                json!({
                    "MedicalTranscriptionJobName": name,
                    // "US English (en-US) is the only valid value for medical
                    // transcription jobs" (model doc for this request's LanguageCode).
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                })
            },
            read: "GetMedicalTranscriptionJob",
            read_name_member: "MedicalTranscriptionJobName",
            record: "MedicalTranscriptionJob",
        },
    ]
}

/// A start request of `family` named `name`, asking for `settings` — or asking for no
/// `Settings` at all when `settings` is JSON null.
fn start_request(family: &Family, name: &str, settings: &Value) -> Value {
    let mut request = (family.base)(name);
    if !settings.is_null() {
        request
            .as_object_mut()
            .expect("a start request is a JSON object")
            .insert("Settings".to_string(), settings.clone());
    }
    request
}

/// Read the job `name` of `family` back.
async fn read(
    transcribe: &Transcribe,
    family: &Family,
    name: &str,
) -> crate::transcription_jobs::Reply {
    let mut request = serde_json::Map::new();
    request.insert(family.read_name_member.to_string(), json!(name));
    transcribe.call(family.read, Value::Object(request)).await
}

/// Settings that satisfy only one side of a documented pair, one row per direction of
/// each pair and per published bound of the qualifying member (`MaxSpeakers` is
/// `@range(min: 2, max: 30)` and `MaxAlternatives` `@range(min: 2, max: 10)` —
/// `smithy_query(query="shape", shape="MedicalTranscriptionSetting")`), including rows
/// where the half-specified pair sits beside a whole one or beside channel
/// identification.
fn half_specified_settings() -> Vec<(&'static str, Value)> {
    vec![
        (
            "speaker partitioning with no speaker ceiling",
            json!({"ShowSpeakerLabels": true}),
        ),
        (
            "the lowest published speaker ceiling, no partitioning asked for",
            json!({"MaxSpeakerLabels": 2}),
        ),
        (
            "the highest published speaker ceiling, partitioning switched off",
            json!({"ShowSpeakerLabels": false, "MaxSpeakerLabels": 30}),
        ),
        (
            "alternative transcriptions with no alternatives ceiling",
            json!({"ShowAlternatives": true}),
        ),
        (
            "the lowest published alternatives ceiling, no alternatives asked for",
            json!({"MaxAlternatives": 2}),
        ),
        (
            "the highest published alternatives ceiling, alternatives switched off",
            json!({"ShowAlternatives": false, "MaxAlternatives": 10}),
        ),
        (
            "a dangling speaker ceiling beside a whole alternatives pair",
            json!({"ShowAlternatives": true, "MaxAlternatives": 3, "MaxSpeakerLabels": 4}),
        ),
        (
            "dangling alternatives beside a whole speaker pair",
            json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2, "ShowAlternatives": true}),
        ),
        (
            "a dangling speaker ceiling beside channel identification",
            json!({"ChannelIdentification": true, "MaxSpeakerLabels": 5}),
        ),
    ]
}

/// Settings whose pairs all hold together, including the rows where neither member of
/// a pair is given at all and where a toggle is explicitly off with no ceiling.
fn whole_settings() -> Vec<(&'static str, Value)> {
    vec![
        ("no Settings member at all", Value::Null),
        (
            "the speaker pair whole at the lowest published ceiling",
            json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}),
        ),
        (
            "the alternatives pair whole at the highest published ceiling",
            json!({"ShowAlternatives": true, "MaxAlternatives": 10}),
        ),
        (
            "both pairs whole at once",
            json!({
                "ShowSpeakerLabels": true,
                "MaxSpeakerLabels": 30,
                "ShowAlternatives": true,
                "MaxAlternatives": 2
            }),
        ),
        (
            "channel identification, neither pair asked for",
            json!({"ChannelIdentification": true}),
        ),
        (
            "both toggles explicitly off, neither ceiling given",
            json!({"ShowSpeakerLabels": false, "ShowAlternatives": false}),
        ),
    ]
}

#[tokio::test]
async fn a_start_satisfying_only_one_side_of_a_settings_pair_is_refused() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, (row, settings)) in half_specified_settings().iter().enumerate() {
            let case = format!("{} / {row}", family.case);
            let name = format!("half-pair-{index}");
            let refused = transcribe
                .call(family.start, start_request(&family, &name, settings))
                .await;
            refused.refused(&case, 400, "BadRequestException");

            // A refused start stores nothing, so the name it asked for is unknown.
            read(&transcribe, &family, &name)
                .await
                .refused(&case, 400, "BadRequestException");
        }
    }
}

#[tokio::test]
async fn a_start_whose_settings_pairs_hold_together_is_accepted_and_read_back() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, (row, settings)) in whole_settings().iter().enumerate() {
            let case = format!("{} / {row}", family.case);
            let name = format!("whole-pair-{index}");
            transcribe
                .call(family.start, start_request(&family, &name, settings))
                .await
                .ok(&case);

            let fetched = read(&transcribe, &family, &name).await;
            let job = fetched.ok(&case)[family.record].clone();
            // A member the request left out is absent from the answer rather than
            // defaulted: nothing is invented for it.
            assert_eq!(
                job["Settings"], *settings,
                "case {case:?}: the stored job must report the settings it was started \
                 with, got {:?}",
                job["Settings"]
            );
        }
    }
}

/// The Medical Scribe family carries only the speaker pair (its settings shape has no
/// alternatives members), and its own "one and only one of ChannelIdentification and
/// ShowSpeakerLabels" rule already refuses a request that names no toggle — so the row
/// that the pair rule alone decides is a speaker ceiling beside channel
/// identification.
#[tokio::test]
async fn a_medical_scribe_start_with_a_speaker_ceiling_and_no_partitioning_is_refused() {
    let transcribe = Transcribe::new();

    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a speaker ceiling beside channel identification",
            json!({"ChannelIdentification": true, "MaxSpeakerLabels": 2}),
            false,
        ),
        (
            "the highest published speaker ceiling beside channel identification",
            json!({"ChannelIdentification": true, "MaxSpeakerLabels": 30}),
            false,
        ),
        (
            "channel identification alone",
            json!({"ChannelIdentification": true}),
            true,
        ),
        (
            "the speaker pair whole",
            json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}),
            true,
        ),
    ];

    for (index, (case, settings, accepted)) in rows.iter().enumerate() {
        let name = format!("scribe-pair-{index}");
        let mut request = json!({
            "MedicalScribeJobName": name,
            "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
            "OutputBucketName": "example-scribe-output",
            "DataAccessRoleArn": ROLE_ARN,
            "Settings": settings.clone()
        });
        if settings["ChannelIdentification"] == json!(true) {
            // "A MedicalScribeChannelDefinitions array should be set if and only if the
            // ChannelIdentification value of Settings is set to true."
            request.as_object_mut().expect("an object").insert(
                "ChannelDefinitions".to_string(),
                json!([
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ]),
            );
        }
        let reply = transcribe.call("StartMedicalScribeJob", request).await;
        if *accepted {
            reply.ok(case);
        } else {
            reply.refused(case, 400, "BadRequestException");
        }

        let fetched = transcribe
            .call(
                "GetMedicalScribeJob",
                json!({"MedicalScribeJobName": name}),
            )
            .await;
        if *accepted {
            // The settings the job was started with, plus the clinical-note template
            // its record materialises from the member's documented default ("The
            // default is HISTORY_AND_PHYSICAL", model doc for
            // `ClinicalNoteGenerationSettings$NoteTemplate`; pinned over the whole
            // template class by `crate::medical_scribe_clinical_note_template`).
            let mut expected = settings.clone();
            expected
                .as_object_mut()
                .expect("a row's settings are a JSON object")
                .insert(
                    "ClinicalNoteGenerationSettings".to_string(),
                    json!({"NoteTemplate": DEFAULT_NOTE_TEMPLATE}),
                );
            assert_eq!(
                fetched.ok(case)["MedicalScribeJob"]["Settings"],
                expected,
                "case {case:?}: the stored job must report the settings it was started \
                 with, expected {expected:?}, got {:?}",
                fetched.body
            );
        } else {
            // A refused start stores nothing, so the name it asked for is unknown.
            fetched.refused(case, 400, "BadRequestException");
        }
    }
}

/// The media the medical family's rows name, staged in the object store the two
/// medical-vocabulary tests below mount beside the service: with a store to ask, a
/// start whose media it does not hold is refused for the media, which is another
/// suite's subject (`crate::media_object_resolution`).
const STAGED_MEDIA: &[&str] = &["s3://example-media/dictation.wav"];

/// An emulator that can hold a USABLE custom medical vocabulary: the service with the
/// object store beside it, and the media the medical rows name already staged.
///
/// A custom medical vocabulary's terms come only from the table file at its
/// `VocabularyFileUri`, so a build that makes one `READY` needs a store that serves
/// that file ([`Transcribe::medical_vocabulary_ready`]).
async fn given_the_object_store() -> Transcribe {
    let transcribe = Transcribe::with_object_store();
    transcribe.given_media_objects(STAGED_MEDIA).await;
    transcribe
}

/// A start whose `Settings.VocabularyName` names a custom medical vocabulary this
/// account never created is refused and stores nothing; the same name, created and
/// built first, is accepted and read back.
///
/// Holding the name is what this test is about; that the vocabulary must also be in a
/// state a job may APPLY it in is the `medical_job_start_vocabulary_state` suite's
/// subject, so every vocabulary here is waited out until its build settled `READY`.
#[tokio::test]
async fn a_start_naming_a_custom_medical_vocabulary_this_account_lacks_is_refused() {
    let transcribe = given_the_object_store().await;
    let family = families()
        .into_iter()
        .find(|family| family.start == "StartMedicalTranscriptionJob")
        .expect("the medical transcription family");

    // The name forms `VocabularyName`'s pattern allows (`^[0-9a-zA-Z._-]+$`),
    // including both published length bounds (`@length(min: 1, max: 200)`).
    let long_name = "v".repeat(200);
    let names = ["never-created", "a", "clinic.terms_2", "9", &long_name];

    for (index, vocabulary) in names.iter().enumerate() {
        let case = format!("a job naming the unheld custom medical vocabulary {vocabulary:?}");
        let name = format!("dangling-vocabulary-{index}");
        let refused = transcribe
            .call(
                family.start,
                start_request(&family, &name, &json!({"VocabularyName": vocabulary})),
            )
            .await;
        refused.refused(&case, 400, "BadRequestException");
        // The service's own not-found text names the KIND of resource and never
        // echoes the name the caller asked about — and the medical vocabulary family
        // is named with the PLAIN vocabulary noun, as its recorded refusal words it
        // (`crate::transcription_job_request_rules::not_found_message`).
        let message = refused.error_message(&case);
        assert_eq!(
            message,
            crate::transcription_job_request_rules::not_found_message("medical vocabulary"),
            "case {case:?}: unexpected refusal message, got {message:?}"
        );

        read(&transcribe, &family, &name)
            .await
            .refused(&case, 400, "BadRequestException");
    }

    for (index, vocabulary) in names.iter().enumerate() {
        let case = format!("a job naming the held custom medical vocabulary {vocabulary:?}");
        let name = format!("held-vocabulary-{index}");
        transcribe.medical_vocabulary_ready(vocabulary).await;
        transcribe
            .call(
                family.start,
                start_request(&family, &name, &json!({"VocabularyName": vocabulary})),
            )
            .await
            .ok(&case);

        let fetched = read(&transcribe, &family, &name).await;
        assert_eq!(
            fetched.ok(&case)[family.record]["Settings"],
            json!({"VocabularyName": vocabulary}),
            "case {case:?}: the stored job must report the custom medical vocabulary it \
             was started with, got {:?}",
            fetched.body
        );
    }
}

/// The custom MEDICAL vocabularies are a family of their own: a custom vocabulary of
/// the same name is a different resource, so creating one does not make the name known
/// to a medical transcription job.
#[tokio::test]
async fn a_custom_vocabulary_does_not_stand_in_for_a_custom_medical_vocabulary() {
    let transcribe = given_the_object_store().await;
    let family = families()
        .into_iter()
        .find(|family| family.start == "StartMedicalTranscriptionJob")
        .expect("the medical transcription family");

    for (index, vocabulary) in ["plain-terms", "shared-name"].iter().enumerate() {
        let case = format!("a job naming the custom vocabulary {vocabulary:?}");
        transcribe
            .call(
                "CreateVocabulary",
                json!({
                    "VocabularyName": vocabulary,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                }),
            )
            .await
            .ok(&case);

        let name = format!("wrong-family-{index}");
        transcribe
            .call(
                family.start,
                start_request(&family, &name, &json!({"VocabularyName": vocabulary})),
            )
            .await
            .refused(&case, 400, "BadRequestException");
        read(&transcribe, &family, &name)
            .await
            .refused(&case, 400, "BadRequestException");

        // Created in the MEDICAL family too, and built, the same name is accepted.
        transcribe.medical_vocabulary_ready(vocabulary).await;
        let accepted = format!("medical-family-{index}");
        transcribe
            .call(
                family.start,
                start_request(&family, &accepted, &json!({"VocabularyName": vocabulary})),
            )
            .await
            .ok(&case);
    }
}
