Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# StartMedicalTranscriptionJob accepts Settings combinations the model documents as co-required, and vocabulary names the account does not hold

_Filed: 2026-10-24T10:53:35Z_
_Operation: StartMedicalTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler performs exactly two request checks of its own — a non-empty `Media.MediaFileUri` and `LanguageCode == en-US` — and then stores every other member verbatim (`settings: input.settings`). The co-dependency rules the model documents on `MedicalTranscriptionSetting` are never applied, so requests AWS refuses with HTTP 400 `BadRequestException` are answered 200 with a stored job that later "completes":

1. `Settings.ShowSpeakerLabels: true` without `Settings.MaxSpeakerLabels` — model doc: "If you enable ShowSpeakerLabels in your request, you must also include MaxSpeakerLabels." Emulator: 200, job stored, `GetMedicalTranscriptionJob` echoes speaker partitioning with no speaker ceiling.
2. `Settings.MaxSpeakerLabels` without `ShowSpeakerLabels: true` — model doc: "If you specify the MaxSpeakerLabels field, you must set the ShowSpeakerLabels field to true." Emulator: 200.
3. `Settings.ShowAlternatives: true` without `Settings.MaxAlternatives` — model doc: "If you include ShowAlternatives, you must also include MaxAlternatives." Emulator: 200.
4. `Settings.MaxAlternatives` without `ShowAlternatives: true` — model doc: "If you include MaxAlternatives in your request, you must also include ShowAlternatives with a value of true." Emulator: 200.
5. `Settings.VocabularyName` naming a custom medical vocabulary this account never created: AWS refuses the start with `BadRequestException`; the emulator never consults `store.medical_vocabularies`, starts the job and reports it as COMPLETED against a vocabulary that does not exist.

Expected in each case: HTTP 400 `BadRequestException` and nothing stored (the name stays unknown to `GetMedicalTranscriptionJob`). Actual: HTTP 200, a stored record, and a job that must be deleted. The same operation family already implements exactly these rules for Medical Scribe (`start_medical_scribe_job.rs` lines 262-321 refuse speaker labels without `MaxSpeakerLabels` and a vocabulary filter without its method), so the medical transcription family is the inconsistent one.

## Evidence

Spec: `smithy_query(query="shape", shape="MedicalTranscriptionSetting")` carries all four co-dependency sentences verbatim on `ShowSpeakerLabels`, `MaxSpeakerLabels`, `ShowAlternatives` and `MaxAlternatives`; `StartMedicalTranscriptionJob` models `BadRequestException` @httpError 400 whose own doc is "Your request didn't pass one or more validation tests." Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/start_medical_transcription_job.rs — lines 265-291 are the ONLY request checks (media URI, language code); line 350 copies `settings` straight into the record with no cross-field rule, and no branch anywhere looks up the named vocabulary in `crate::state`. Tests: tests/all/medical_transcription_jobs.rs lines 326-392 enumerate the refusal rows and cover only a `Media` with no file URI and four non-en-US language rows; lines 92-93 and 125 exercise only VALID speaker-label / alternatives pairs, so no test asserts any of the five combinations above. Precedent in this crate: the sibling issues already filed for StartTranscriptionJob and StartMedicalScribeJob cover the same class on their own operations; this operation is not covered by either.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.StartMedicalTranscriptionJob and body {"MedicalTranscriptionJobName":"labels-no-max","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://b/a.wav"},"OutputBucketName":"out","Specialty":"PRIMARYCARE","Type":"CONVERSATION","Settings":{"ShowSpeakerLabels":true}} — emulator answers 200 and stores the job; AWS answers 400 BadRequestException because MaxSpeakerLabels is missing
2. Repeat with "Settings":{"MaxSpeakerLabels":2} and no ShowSpeakerLabels — emulator 200, AWS 400
3. Repeat with "Settings":{"ShowAlternatives":true} and no MaxAlternatives — emulator 200, AWS 400
4. Repeat with "Settings":{"MaxAlternatives":3} and no ShowAlternatives — emulator 200, AWS 400
5. Repeat with "Settings":{"VocabularyName":"never-created"} — emulator 200 and the job later reports COMPLETED; AWS 400 BadRequestException for the non-existent custom medical vocabulary
6. Call Transcribe.GetMedicalTranscriptionJob for each name: the emulator reports a job AWS never created
