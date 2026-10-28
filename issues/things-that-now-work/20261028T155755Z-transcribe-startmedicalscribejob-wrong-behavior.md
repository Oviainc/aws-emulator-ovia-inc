Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# The documented ClinicalNoteGenerationSettings.NoteTemplate default is never materialised, unlike every other documented default this crate records at start time

_Filed: 2026-10-28T15:57:55Z_
_Operation: StartMedicalScribeJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MedicalScribeSettings.ClinicalNoteGenerationSettings` is stored and reported verbatim as the request sent it, so a start that carries `ClinicalNoteGenerationSettings` without a `NoteTemplate` (and a start that carries no `ClinicalNoteGenerationSettings` at all) produces a stored job whose `Settings.ClinicalNoteGenerationSettings.NoteTemplate` is ABSENT for every later read — although the model states a default for that member.

This crate's own rule for a documented default is the opposite: the default is applied when the resource is recorded, so every operation that reports it reports the same value. `CreateCallAnalyticsCategory` records `DEFAULT_INPUT_TYPE` for a request that named no `InputType`, and `StartTranscriptionJob` records `OutputStartIndex: 0` for a `Subtitles` that named none — both quoting the model sentence that states the default. `StartMedicalScribeJob` is the one start that skips that step for the one default its own settings shape documents, so the clinical-note template a HealthScribe job will actually be summarised with is unreadable from the job record.

## Evidence

Smithy (smithy_query query="shape" shape="com.amazonaws.transcribe#ClinicalNoteGenerationSettings"), member `NoteTemplate`:
  "Specify one of the following templates to use for the clinical note summary. The default is HISTORY_AND_PHYSICAL."

The handler stores the settings untouched — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_medical_scribe_job.rs:456:
    settings: input.settings,
and the record is reported untouched — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:4157:
    settings: Some(self.settings.clone()),
Nothing in the crate ever names the value: `grep -n 'HISTORY_AND_PHYSICAL' src/` matches only the generated enum value list, src/ops/types/shared_types.rs:471:
    pub const MEDICAL_SCRIBE_NOTE_TEMPLATE_VALUES: &[&str] = &["BEHAVIORAL_SOAP", "BIRP", "DAP", "GIRPP", "HISTORY_AND_PHYSICAL", "PHYSICAL_SOAP", "SIRP"];

The same crate materialises its two other documented defaults:
- src/ops/start_transcription_job.rs:626-630:
    subtitles: input.subtitles.map(|subtitles| SubtitlesOutput {
        formats: subtitles.formats,
        // "The default start index for Amazon Transcribe is 0" (model doc
        // for `Subtitles$OutputStartIndex`).
        output_start_index: Some(subtitles.output_start_index.unwrap_or(0)),
- src/state.rs:789-794 (DEFAULT_INPUT_TYPE): "'If you do not include InputType, your category is created as a post-call category by default' (model doc for `CreateCallAnalyticsCategoryRequest$InputType`). The default is RECORDED on the category, so every operation that reports it reports the same value rather than each one re-defaulting." — applied at src/ops/create_call_analytics_category.rs:278-280.

Test coverage: the transcription suite explicitly accommodates the materialised default outside its echo table — tests/all/transcription_jobs.rs:429-437:
            assert_eq!(
                fetched["Subtitles"]["OutputStartIndex"],
                subtitles.get("OutputStartIndex").cloned().unwrap_or(json!(0)),
                "case {case:?}: OutputStartIndex must be the requested value (default 0), ..."
The medical-scribe suite has no such accommodation: tests/all/medical_scribe_jobs.rs:46-53 lists "Settings" in `ECHOED_MEMBERS` ("a stored job must read them back unchanged"), and its only NoteTemplate row deliberately avoids the default — tests/all/medical_scribe_jobs.rs:90-93:
                    // "BIRP" is a value of `MedicalScribeNoteTemplate`
                    // ... not the default-looking HISTORY_AND_PHYSICAL.
                    "ClinicalNoteGenerationSettings": {"NoteTemplate": "BIRP"}
so no test pins the missing default.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.StartMedicalScribeJob and body {"MedicalScribeJobName":"scribe-default-template","Media":{"MediaFileUri":"s3://example-media/consult.flac"},"OutputBucketName":"example-scribe-output","DataAccessRoleArn":"arn:aws:iam::111122223333:role/HealthScribeAccess","Settings":{"ShowSpeakerLabels":true,"MaxSpeakerLabels":2,"ClinicalNoteGenerationSettings":{}}}
2. Read the job back: X-Amz-Target: Transcribe.GetMedicalScribeJob with {"MedicalScribeJobName":"scribe-default-template"}
3. Observe MedicalScribeJob.Settings.ClinicalNoteGenerationSettings carries no NoteTemplate, although the member's model documentation says "The default is HISTORY_AND_PHYSICAL"
4. Repeat with the same request minus ClinicalNoteGenerationSettings entirely: the reported Settings again names no clinical-note template, while the sibling defaults (CategoryProperties.InputType, SubtitlesOutput.OutputStartIndex) are reported as the values their docs declare
