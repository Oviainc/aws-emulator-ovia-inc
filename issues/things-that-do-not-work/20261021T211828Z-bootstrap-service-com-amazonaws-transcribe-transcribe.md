Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the transcribe emulator

_Filed: 2026-10-21T21:18:28Z_
_Operation: service-com-amazonaws-transcribe-transcribe | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 44 handlers under `services/service-com-amazonaws-transcribe-transcribe/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Transcribe client can do anything. Implement a FIRST, SMALL stateful slice: the transcription-job lifecycle, which is Transcribe's primary resource.

### Happy path to implement (in order)

1. **StartTranscriptionJob** (`PUT /transcriptionjobs/{TranscriptionJobName}`) — input: `TranscriptionJobName` (required, 1..200, `^[0-9a-zA-Z._-]+$`), `Media` (required, with `MediaFileUri` matching `^(s3://|http(s*)://).+$`), `LanguageCode`, `MediaFormat`, `MediaSampleRateHertz` (8000..48000), `OutputBucketName`, `OutputKey`, `Settings`, `Subtitles`, `Tags`. Output: `TranscriptionJob` built from the record just stored — `TranscriptionJobName`, `TranscriptionJobStatus` (start at `IN_PROGRESS` or `QUEUED`), `LanguageCode`, `Media`, `MediaFormat`, `MediaSampleRateHertz`, `CreationTime`, `StartTime`. Re-using an existing job name must return `ConflictException` (HTTP 409); malformed name/URI must return `BadRequestException` (HTTP 400).
2. **GetTranscriptionJob** (`GET /transcriptionjobs/{TranscriptionJobName}`) — returns the SAME `TranscriptionJob` structure read back from the store, including a `Transcript { TranscriptFileUri }` and `CompletionTime` once the job has been advanced to `COMPLETED`. Unknown job name → `NotFoundException` (HTTP 404).
3. **ListTranscriptionJobs** (`GET /transcriptionjobs`) — returns `TranscriptionJobSummaries` (one `TranscriptionJobSummary` per stored job: `TranscriptionJobName`, `CreationTime`, `StartTime`, `CompletionTime`, `LanguageCode`, `TranscriptionJobStatus`, `OutputLocationType`), honouring the `Status` filter, the `JobNameContains` substring filter, `MaxResults` (1..100) and `NextToken` paging (echo `Status` back in the output when filtered).
4. **DeleteTranscriptionJob** (`DELETE /transcriptionjobs/{TranscriptionJobName}`) — removes the record, empty body, HTTP 204. A subsequent GetTranscriptionJob must then return `NotFoundException`, and the job must disappear from ListTranscriptionJobs.

Optional fifth step if cheap: a simple job-completion rule so a job started in a previous call reads back as `COMPLETED` with a synthesized `Transcript.TranscriptFileUri` (e.g. `s3://{OutputBucketName}/{OutputKey or JobName.json}`, or a service-managed URI when no output bucket was given) — this is what makes a realistic poll-until-done client script terminate.

NEVER STUBS: each of these must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (per the crate README). No canned bodies, no echoing the input without storing it, no random IDs that are never persisted. A start followed by a get must return exactly what was started.

## State Model

In `src/state.rs` (`ComAmazonawsTranscribeTranscribeState`):

- `transcription_jobs: BTreeMap<String /* TranscriptionJobName */, TranscriptionJobRecord>` — insertion/creation ordered so List is deterministic.
- `TranscriptionJobRecord { name: String, status: TranscriptionJobStatus, language_code: Option<LanguageCode>, media: Media /* media_file_uri */, media_format: Option<MediaFormat>, media_sample_rate_hertz: Option<i32>, output_bucket_name: Option<String>, output_key: Option<String>, output_location_type: OutputLocationType /* CUSTOMER_BUCKET when OutputBucketName given, else SERVICE_BUCKET */, settings: Option<Settings>, subtitles: Option<SubtitlesOutput>, transcript: Option<Transcript /* transcript_file_uri */>, failure_reason: Option<String>, creation_time: SystemTime, start_time: Option<SystemTime>, completion_time: Option<SystemTime>, tags: Vec<Tag> }`.
- List paging: `NextToken` encodes the last returned job name (opaque string), resumed by scanning the ordered map past that key.

Done = a client script can StartTranscriptionJob → GetTranscriptionJob (fields match what was started) → ListTranscriptionJobs (job present, filters and MaxResults respected) → DeleteTranscriptionJob → GetTranscriptionJob returns 404, with no 501 anywhere in the trace. Everything else (medical jobs, call analytics, custom vocabularies, language models, tagging) stays stubbed at 501 and is filed separately.

## Evidence

`services/service-com-amazonaws-transcribe-transcribe/OPERATIONS.md` lists 44 operations for the `awsJson1_1`/REST `Transcribe` service, including StartTranscriptionJob (`PUT /transcriptionjobs/{TranscriptionJobName}`, required `TranscriptionJobName` + `Media`, output `TranscriptionJob`, errors BadRequest/Conflict/InternalFailure/LimitExceeded), GetTranscriptionJob (`GET /transcriptionjobs/{TranscriptionJobName}`, output `TranscriptionJob`, errors include NotFoundException 404), ListTranscriptionJobs (`GET /transcriptionjobs`, @paginated inputToken=NextToken/outputToken=NextToken, inputs Status/JobNameContains/NextToken/MaxResults 1..100, output `TranscriptionJobSummaries`) and DeleteTranscriptionJob (`DELETE /transcriptionjobs/{TranscriptionJobName}` -> 204). The `TranscriptionJob`, `TranscriptionJobSummary`, `Media` and `Transcript` shapes in `smithy-model/com-amazonaws-transcribe-transcribe.json` (via smithy_query) supply the exact member names used above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 44 handler files (e.g. `src/ops/start_transcription_job.rs`, `get_transcription_job.rs`, `list_transcription_jobs.rs`, `delete_transcription_job.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No tests exercise any transcription-job behavior.

## Steps to Reproduce

1. POST/PUT StartTranscriptionJob with TranscriptionJobName="demo-job", Media={MediaFileUri:"s3://my-bucket/audio.mp3"}, LanguageCode="en-US", MediaFormat="mp3", OutputBucketName="my-out-bucket" — expect 200 with TranscriptionJob.TranscriptionJobStatus of IN_PROGRESS/QUEUED and the submitted Media/LanguageCode echoed from stored state; actual: HTTP 501 NotImplemented
2. Call GetTranscriptionJob with TranscriptionJobName="demo-job" — expect 200 with the same job, CreationTime set, and (once completed) Transcript.TranscriptFileUri; actual: 501
3. Call StartTranscriptionJob again with the same name — expect ConflictException (HTTP 409)
4. Call ListTranscriptionJobs with MaxResults=1 and JobNameContains="demo" — expect TranscriptionJobSummaries containing the demo-job summary and NextToken paging honoured; actual: 501
5. Call DeleteTranscriptionJob with TranscriptionJobName="demo-job" — expect HTTP 204 and empty body; actual: 501
6. Call GetTranscriptionJob with TranscriptionJobName="demo-job" again — expect NotFoundException (HTTP 404), and the job absent from ListTranscriptionJobs
