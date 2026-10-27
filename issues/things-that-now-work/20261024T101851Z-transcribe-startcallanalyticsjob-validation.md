Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# StartCallAnalyticsJob accepts a media URI naming no object and Settings naming resources the account does not hold

_Filed: 2026-10-24T10:18:51Z_
_Operation: StartCallAnalyticsJob | Class: validation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates only three things of its own — that `Media` names a non-empty `MediaFileUri` or `RedactedMediaFileUri`, that `Settings.VocabularyFilterName` is accompanied by `VocabularyFilterMethod`, and that two `ChannelDefinitions` do not collide — and then stores every other member verbatim. Two classes of request AWS refuses are accepted with HTTP 200 and a stored, "completing" job:

1. `Media.MediaFileUri` / `Media.RedactedMediaFileUri` naming no existing S3 object. AWS answers HTTP 400 BadRequestException ("The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." — the response already recorded for the sibling operation StartTranscriptionJob) and stores nothing. The emulator accepts any non-empty string, stores the record, and later lists and completes the job.

2. `Settings.VocabularyName`, `Settings.VocabularyFilterName` and `Settings.LanguageModelName` naming a custom vocabulary / vocabulary filter / custom language model this account does not hold. AWS refuses the start with BadRequestException ("The custom vocabulary that you've specified doesn't exist..."). The handler never consults `store.vocabularies`, `store.vocabulary_filters` or `store.language_models`, so the job starts against resources that were never created and GetCallAnalyticsJob then echoes the dangling names back.

Expected in both cases: HTTP 400 BadRequestException and no record stored (so ListCallAnalyticsJobs does not report it and the name stays free). Actual: HTTP 200 with a CallAnalyticsJob body, a stored record, and a phantom job that must be deleted. This is the same gap already filed for StartTranscriptionJob and StartMedicalScribeJob, in the third job family.

## Evidence

Spec: StartCallAnalyticsJob models BadRequestException @httpError 400 ("Your request didn't pass one or more validation tests") and `Media` is @required with its members documented as "The Amazon S3 location of the media file you want to redact/transcribe" (smithy_query operation=StartCallAnalyticsJob, shape=Media); `CallAnalyticsJobSettings$VocabularyName`/`VocabularyFilterName`/`LanguageModelName` are documented as naming an existing custom vocabulary / filter / language model. Recorded AWS behavior for the identical condition on the sibling start operation is quoted in open issue 20261022T175151Z-transcribe-starttranscriptionjob-accepts-a-media-mediafileuri-aws-rejects-creating.md. Handler: services/.../src/ops/start_call_analytics_job.rs lines 241-308 are the only request checks (non-empty media URI, filter-method pairing, channel-definition uniqueness); lines 350-380 insert the record with `settings` copied straight from the input and no store lookup. Tests: tests/all/call_analytics_jobs.rs lines 80-110 start a job with Settings.VocabularyName "call-terms", VocabularyFilterName "blocked-terms" and LanguageModelName "support-model" that were never created and assert SUCCESS, and lines 693-748 enumerate the refusal rows without any dangling-reference or unresolvable-media row — so no test asserts the AWS behavior.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.StartCallAnalyticsJob and {"CallAnalyticsJobName":"ca-1","Media":{"MediaFileUri":"s3://no-such-bucket/no-such-call.wav"}} — emulator answers 200 and stores the job; AWS answers 400 BadRequestException and stores nothing.
2. Call Transcribe.ListCallAnalyticsJobs: the emulator lists a job AWS never created.
3. POST StartCallAnalyticsJob with {"CallAnalyticsJobName":"ca-2","Media":{"MediaFileUri":"s3://b/call.wav"},"Settings":{"VocabularyName":"never-created"}} — emulator 200; AWS 400 BadRequestException for the non-existent custom vocabulary.
4. Repeat with {"Settings":{"LanguageModelName":"never-created"}} and with {"Settings":{"VocabularyFilterName":"never-created","VocabularyFilterMethod":"mask"}} — emulator 200 in both cases.
5. Call Transcribe.GetCallAnalyticsJob for ca-2 after the transition delay: the emulator reports COMPLETED with the dangling Settings echoed back.
