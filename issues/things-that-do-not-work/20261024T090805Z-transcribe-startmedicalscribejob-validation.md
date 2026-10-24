Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# StartMedicalScribeJob accepts Settings.VocabularyName / VocabularyFilterName naming resources the account does not hold

_Filed: 2026-10-24T09:08:05Z_
_Operation: StartMedicalScribeJob | Class: validation_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates the request's internal consistency thoroughly (MediaFileUri present, ShowSpeakerLabels XOR ChannelIdentification, MaxSpeakerLabels, ChannelDefinitions pairing, VocabularyFilterMethod co-requirement) but it never consults the store for the resources `Settings` NAMES. A start whose `Settings.VocabularyName` or `Settings.VocabularyFilterName` refers to a custom vocabulary / vocabulary filter that was never created — or to a vocabulary that is still `PENDING` or has `FAILED` — is accepted with HTTP 200, stored, and "completes" against a vocabulary that does not exist.

Expected (real service): the start is refused with HTTP 400 `BadRequestException` ("Your request didn't pass one or more validation tests") naming the custom vocabulary / vocabulary filter it could not find, and no job record is created, so a following `GetMedicalScribeJob` reports the job as unknown. Actual: 200, a stored record, a job that reaches `COMPLETED` and must be deleted, and `GetMedicalScribeJob` echoing a `Settings.VocabularyName` that resolves to nothing. This is the same class of hole already filed for `StartTranscriptionJob` (open issue 20261024T015920Z-transcribe-starttranscriptionjob-validation, item 5) but on a different operation — the Medical Scribe handler has its own, separate request-validation block and performs no lookup either.

## Evidence

Spec: `smithy_query(query="shape", shape="MedicalScribeSettings")` — `VocabularyName` is "The name of the custom vocabulary you want to include in your Medical Scribe request" and `VocabularyFilterName` "The name of the custom vocabulary filter you want to include", i.e. both must name an existing resource of this account; the operation models `BadRequestException` (@httpError 400) whose own doc is "Your request didn't pass one or more validation tests", and `VocabularyState` exists precisely so a vocabulary is usable only when READY ("If the state is READY, you can use the custom vocabulary"). Handler: `src/ops/start_medical_scribe_job.rs` lines 247-321 are the complete set of checks — none of them touches `store.vocabularies` or `store.vocabulary_filters`; the record is then inserted at lines 360-384 with `settings: input.settings` copied verbatim. Tests: `tests/all/medical_scribe_jobs.rs` lines 84-85 START a job with `"VocabularyName": "clinic-terms"` and `"VocabularyFilterName": "blocked-terms"` without ever creating either resource and assert success, and the refusal rows (lines ~392-525) cover only the intra-request consistency rules — so no test asserts the AWS refusal and the suite currently encodes the divergence.

## Steps to Reproduce

1. Start an emulator with no custom vocabularies and no vocabulary filters
2. POST / with `X-Amz-Target: Transcribe.StartMedicalScribeJob` and body {"MedicalScribeJobName":"dangling","Media":{"MediaFileUri":"s3://example-media/visit.flac"},"OutputBucketName":"example-output","DataAccessRoleArn":"arn:aws:iam::000000000000:role/TranscribeRole","Settings":{"ShowSpeakerLabels":true,"MaxSpeakerLabels":2,"VocabularyName":"never-created"}}
3. Emulator answers 200 and stores the job; AWS answers 400 BadRequestException because the named custom vocabulary does not exist
4. Call `Transcribe.GetMedicalScribeJob` with {"MedicalScribeJobName":"dangling"} — the emulator reports a job (eventually COMPLETED) whose Settings name a vocabulary that was never created; AWS has no such job
5. Repeat with {"Settings":{"ShowSpeakerLabels":true,"MaxSpeakerLabels":2,"VocabularyFilterName":"never-created","VocabularyFilterMethod":"mask"}} — same divergence
6. Create a vocabulary from a VocabularyFileUri (which settles in a non-READY state here), then start a job naming it — AWS refuses a vocabulary that is not READY, the emulator accepts
