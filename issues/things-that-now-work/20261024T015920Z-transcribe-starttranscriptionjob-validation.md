Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# StartTranscriptionJob accepts request combinations the service documents as errors (co-required sub-parameters, dangling vocabulary references)

_Filed: 2026-10-24T01:59:20Z_
_Operation: StartTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler performs exactly two request checks of its own — a non-empty `Media.MediaFileUri` and "exactly one of LanguageCode / IdentifyLanguage / IdentifyMultipleLanguages" — and then stores every other member verbatim. Several combinations the service documents as failures are therefore accepted with HTTP 200 and a stored job:

1. `Settings.VocabularyFilterName` without `Settings.VocabularyFilterMethod`: AWS answers HTTP 400 BadRequestException; the emulator answers 200 and stores the job, and GetTranscriptionJob later echoes a filter name with no method.
2. `LanguageOptions` supplied together with `LanguageCode` (i.e. without `IdentifyLanguage`): AWS answers 400; the emulator answers 200 because its language check only counts the three language specifiers and never looks at `LanguageOptions`.
3. `JobExecutionSettings` without `DataAccessRoleArn`: AWS answers 400; the emulator answers 200 and even honours `AllowDeferredExecution` to queue the job (start_transcription_job.rs lines 374-394).
4. `OutputKey` without `OutputBucketName`: AWS answers 400; the emulator answers 200 and then reports a service-bucket transcript URI built from the caller's key.
5. `Settings.VocabularyName` / `Settings.VocabularyFilterName` / `ModelSettings.LanguageModelName` naming a resource this account does not hold: AWS refuses the start with BadRequestException ("The custom vocabulary that you've specified doesn't exist..."); the emulator never consults `store.vocabularies` / `store.vocabulary_filters` and starts the job, which then "completes" against a vocabulary that was never created.

In each case the expected observable result is HTTP 400 BadRequestException with no job stored; the actual result is HTTP 200, a stored record, and a job that appears in ListTranscriptionJobs and must be deleted. This is disjoint from the already-open issue about `Media.MediaFileUri` pointing at a non-existent S3 object — none of the five combinations above is validated by that path.

## Evidence

Spec (model docs, all quoted verbatim in the handler's own generated header and confirmed by smithy_query doc): Settings$VocabularyFilterName — "Note that if you include VocabularyFilterName in your request, you must also include VocabularyFilterMethod."; StartTranscriptionJobRequest$LanguageOptions — "If you include LanguageOptions in your request, you must also include IdentifyLanguage."; StartTranscriptionJobRequest$JobExecutionSettings — "If you include JobExecutionSettings in your request, you must also include the sub-parameters: AllowDeferredExecution and DataAccessRoleArn."; StartTranscriptionJobRequest$OutputKey — "Use in combination with OutputBucketName to specify the output location...". The operation models BadRequestException @httpError 400 for exactly these refusals, and the service's own BadRequestException doc is "Your request didn't pass one or more validation tests." Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs lines 305-331 are the only request checks; lines 396-437 insert the record with `settings`, `job_execution_settings`, `language_options`, `output_key` and `model_settings` copied straight from the input, with no cross-field rule and no lookup of the named vocabulary/filter/model in `crate::state`. Tests: tests/all/transcription_jobs.rs lines 655-723 enumerate the refusal rows and cover only missing MediaFileUri, zero or two language specifiers, and three boundary-validator rows — none of the five combinations above is asserted anywhere.

## Steps to Reproduce

1. POST / X-Amz-Target: Transcribe.StartTranscriptionJob with {"TranscriptionJobName":"filter-no-method","Media":{"MediaFileUri":"s3://b/a.flac"},"LanguageCode":"en-US","Settings":{"VocabularyFilterName":"my-filter"}} — emulator answers 200 and stores the job; AWS answers 400 BadRequestException because VocabularyFilterMethod is missing
2. Repeat with {"LanguageCode":"en-US","LanguageOptions":["en-US","fr-FR"]} (no IdentifyLanguage) — emulator 200, AWS 400
3. Repeat with {"JobExecutionSettings":{"AllowDeferredExecution":true}} and no DataAccessRoleArn — emulator 200 and the job is QUEUED, AWS 400
4. Repeat with {"OutputKey":"transcripts/out.json"} and no OutputBucketName — emulator 200 and later reports a service-bucket URI built from that key, AWS 400
5. Repeat with {"Settings":{"VocabularyName":"never-created"}} — emulator 200 and the job completes, AWS 400 BadRequestException for the non-existent custom vocabulary
6. In each case call Transcribe.ListTranscriptionJobs: the emulator lists a job AWS never created
