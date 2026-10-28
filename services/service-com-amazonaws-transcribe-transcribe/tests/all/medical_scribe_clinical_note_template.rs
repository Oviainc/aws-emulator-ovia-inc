// SPDX-License-Identifier: CC0-1.0

//! The clinical-note template a Medical Scribe (AWS HealthScribe) job is RECORDED
//! with, end to end over the wire: `StartMedicalScribeJob` -> `GetMedicalScribeJob`.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: a member whose model documentation states a default value is
//! materialised on the resource when it is recorded, so every operation that reports
//! the resource reports the same value instead of leaving the member absent — here
//! `ClinicalNoteGenerationSettings.NoteTemplate`, whose documentation says "The
//! default is HISTORY_AND_PHYSICAL" (`smithy_query(query="shape",
//! shape="ClinicalNoteGenerationSettings")`), applying to every job this family
//! accepts because the operation generates a clinical note for each one.
//!
//! The test asserts that over a table spanning the axis the rule generalises over:
//! every value of the model's `MedicalScribeNoteTemplate` enum, the clinical-note
//! settings given but empty, and no clinical-note settings member at all — each
//! crossed with both of the speaker-partitioning modes the family's settings offer, so
//! the template is pinned independently of the rest of the settings.

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::ops::types::MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES;
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_NOTE_TEMPLATE;

use crate::transcription_jobs::Transcribe;

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// One row of the table: the `ClinicalNoteGenerationSettings` a start asks for (JSON
/// null for a start that names the member not at all), and the `NoteTemplate` the job
/// it starts must be recorded with.
struct Row {
    case: String,
    clinical_note: Value,
    expected_template: String,
}

/// The clinical-note settings a start can carry, one row per axis the rule generalises
/// over: every value the model's `MedicalScribeNoteTemplate` enum carries (taken from
/// the model's own list, `MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES`, so no value is missed),
/// the settings object present but naming no template, and no clinical-note settings
/// member at all.
fn clinical_note_cases() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "no ClinicalNoteGenerationSettings member at all".to_string(),
            clinical_note: Value::Null,
            expected_template: DEFAULT_NOTE_TEMPLATE.to_string(),
        },
        Row {
            case: "ClinicalNoteGenerationSettings naming no NoteTemplate".to_string(),
            clinical_note: json!({}),
            expected_template: DEFAULT_NOTE_TEMPLATE.to_string(),
        },
    ];
    for template in MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES {
        rows.push(Row {
            case: format!("NoteTemplate {template}"),
            clinical_note: json!({"NoteTemplate": template}),
            expected_template: (*template).to_string(),
        });
    }
    rows
}

/// The two speaker-partitioning modes a Medical Scribe start must choose exactly one of
/// ("One and only one of ChannelIdentification and ShowSpeakerLabels must be set"), as
/// the settings members and the channel definitions each one requires.
fn partitioning_modes() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "speaker partitioning",
            json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}),
            Value::Null,
        ),
        (
            "channel identification",
            json!({"ChannelIdentification": true}),
            json!([
                {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                {"ChannelId": 1, "ParticipantRole": "PATIENT"}
            ]),
        ),
    ]
}

/// A start request named `name`, asking for `mode`'s partitioning and `clinical_note`'s
/// clinical-note settings.
fn start_request(
    name: &str,
    mode_settings: &Value,
    definitions: &Value,
    clinical_note: &Value,
) -> Value {
    let mut settings = mode_settings.clone();
    if !clinical_note.is_null() {
        settings
            .as_object_mut()
            .expect("the mode's settings are a JSON object")
            .insert(
                "ClinicalNoteGenerationSettings".to_string(),
                clinical_note.clone(),
            );
    }
    let mut request = json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": settings
    });
    if !definitions.is_null() {
        request
            .as_object_mut()
            .expect("a start request is a JSON object")
            .insert("ChannelDefinitions".to_string(), definitions.clone());
    }
    request
}

/// The settings a start asking for `request`'s settings must be RECORDED with: what it
/// asked for, with the clinical-note template materialised at `expected_template`.
fn expected_settings(request: &Value, expected_template: &str) -> Value {
    let mut settings = request["Settings"].clone();
    settings
        .as_object_mut()
        .expect("a request's Settings are a JSON object")
        .insert(
            "ClinicalNoteGenerationSettings".to_string(),
            json!({"NoteTemplate": expected_template}),
        );
    settings
}

#[tokio::test]
async fn a_started_job_records_the_requested_clinical_note_template_or_the_documented_default() {
    // The default this crate records is a value of the member's own modelled enum, so
    // a job recorded with it reports a `MedicalScribeNoteTemplate` a client can read.
    assert!(
        MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES.contains(&DEFAULT_NOTE_TEMPLATE),
        "the recorded default must be a modelled MedicalScribeNoteTemplate, got {:?} \
         against {MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES:?}",
        DEFAULT_NOTE_TEMPLATE
    );

    let transcribe = Transcribe::new();

    for (mode_index, (mode_case, mode_settings, definitions)) in
        partitioning_modes().iter().enumerate()
    {
        for (row_index, row) in clinical_note_cases().iter().enumerate() {
            let case = format!("{mode_case}, {}", row.case);
            let name = format!("note-template-{mode_index}-{row_index}");
            let request = start_request(&name, mode_settings, definitions, &row.clinical_note);

            let started = transcribe
                .call("StartMedicalScribeJob", request.clone())
                .await;
            let job = started.ok(&case)["MedicalScribeJob"].clone();
            let expected = expected_settings(&request, &row.expected_template);
            assert_eq!(
                job["Settings"], expected,
                "case {case:?}: the started job must report the settings it was recorded \
                 with, expected {expected:?}, got {:?}",
                job["Settings"]
            );

            // Read back through the operation a client reads a job with: the template
            // is on the RECORD, so the start's answer and every later read agree.
            let fetched = transcribe
                .call("GetMedicalScribeJob", json!({"MedicalScribeJobName": name}))
                .await;
            let stored = fetched.ok(&case)["MedicalScribeJob"].clone();
            assert_eq!(
                stored["Settings"], expected,
                "case {case:?}: GetMedicalScribeJob must report the recorded settings, \
                 expected {expected:?}, got {:?}",
                stored["Settings"]
            );
            assert_eq!(
                stored["Settings"]["ClinicalNoteGenerationSettings"]["NoteTemplate"],
                json!(row.expected_template),
                "case {case:?}: expected the recorded NoteTemplate {:?}, got {:?}",
                row.expected_template,
                stored["Settings"]["ClinicalNoteGenerationSettings"]["NoteTemplate"]
            );

            // A read does not change what a later read sees.
            let again = transcribe
                .call("GetMedicalScribeJob", json!({"MedicalScribeJobName": name}))
                .await;
            assert_eq!(
                again.ok(&case)["MedicalScribeJob"]["Settings"], stored["Settings"],
                "case {case:?}: a second read must report the same settings, got {:?}",
                again.body
            );
        }
    }
}
