Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the comprehend emulator

_Filed: 2026-10-21T21:38:30Z_
_Operation: service-com-amazonaws-comprehend-comprehend-20171127 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 85 operations in `services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/` is an untouched scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Scaffolded::not_implemented(..)` (HTTP 501), and the crate holds no state at all. No client workflow can run.

This issue defines the FIRST, minimal slice: Amazon Comprehend's core loop of synchronous inference plus the asynchronous analysis-job lifecycle. Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: Comprehend_20171127.<OperationName>` (see OPERATIONS.md "Wire protocol").

Implement these 6 operations, in this order:

1. **DetectDominantLanguage** — input `Text` (required, len 1..). Output `Languages`: list of `DominantLanguage{LanguageCode, Score}`. Must be computed deterministically from the submitted `Text` (e.g. a simple character/stopword heuristic mapping to one of the modeled `LanguageCode` values), with `Score` in 0.0..1.0 and the list sorted by descending `Score`. Reject empty `Text` with `InvalidRequestException` (HTTP 400).
2. **DetectSentiment** — input `Text` (required), `LanguageCode` (required, one of ar|de|en|es|fr|hi|it|ja|ko|pt|zh|zh-TW). Output `Sentiment` (MIXED|NEGATIVE|NEUTRAL|POSITIVE) and `SentimentScore{Positive, Negative, Neutral, Mixed}`. The four scores must sum to ~1.0 and the returned `Sentiment` must be the arg-max of those scores — i.e. derived from the input text, not a constant. A `LanguageCode` outside the enum must produce `InvalidRequestException`; an unsupported-but-valid language may produce `UnsupportedLanguageException` (HTTP 400).
3. **StartSentimentDetectionJob** — input `InputDataConfig` (required), `OutputDataConfig` (required), `DataAccessRoleArn` (required), `LanguageCode` (required), optional `JobName`, `ClientRequestToken` (@idempotencyToken), `Tags`, `VolumeKmsKeyId`, `VpcConfig`. Must mint a fresh 32-char-max `JobId`, build a `JobArn` of the modeled form `arn:aws:comprehend:<region>:<account>:sentiment-detection-job/<JobId>`, persist the whole job record, and return `JobId`/`JobArn`/`JobStatus` = `SUBMITTED`. Repeating the same `ClientRequestToken` with the same inputs must return the SAME `JobId` rather than creating a second job.
4. **DescribeSentimentDetectionJob** — input `JobId` (required). Output `SentimentDetectionJobProperties` reconstructed from the stored record: `JobId`, `JobArn`, `JobName`, `JobStatus`, `Message`, `SubmitTime`, `EndTime`, `InputDataConfig`, `OutputDataConfig`, `LanguageCode`, `DataAccessRoleArn`, `VolumeKmsKeyId`, `VpcConfig` — the exact values passed to Start, not echoes of the Describe request. An unknown `JobId` must return `JobNotFoundException` (HTTP 404), not 200 with an empty body. Status must progress monotonically on read of the stored clock (SUBMITTED -> IN_PROGRESS -> COMPLETED, with `EndTime` set when terminal).
5. **ListSentimentDetectionJobs** — input `Filter` (`SentimentDetectionJobFilter`: JobName, JobStatus, SubmitTimeBefore, SubmitTimeAfter), `MaxResults` (1..500), `NextToken`. Output `SentimentDetectionJobPropertiesList` + `NextToken`. Must enumerate the persisted jobs, honour the filter, truncate at `MaxResults`, and return an opaque `NextToken` that resumes exactly where the page ended (absent on the last page). A job created in step 3 must appear here.
6. **StopSentimentDetectionJob** — input `JobId` (required). Output `JobId`, `JobStatus`. Must mutate the stored job: a job in SUBMITTED/IN_PROGRESS moves to `STOP_REQUESTED`/`STOPPED` and a subsequent Describe must observe that new status. Unknown `JobId` -> `JobNotFoundException` (HTTP 404).

NEVER STUBS: each of the six must compute its response from persisted state or from the submitted text. Hardcoded sentiment, echoed request fields, or a random `JobId` that is never stored are failures of this issue.

## State Model

A single `ComprehendState` behind the service's state lock:

- `sentiment_jobs: HashMap<JobId /* String */, AnalysisJob>` where
  `AnalysisJob { job_id: String, job_arn: String, job_name: Option<String>, job_status: JobStatus, message: Option<String>, submit_time: SystemTime, end_time: Option<SystemTime>, input_data_config: InputDataConfig, output_data_config: OutputDataConfig, language_code: LanguageCode, data_access_role_arn: String, volume_kms_key_id: Option<String>, vpc_config: Option<VpcConfig>, tags: Vec<Tag> }`
  (the same struct is reusable later for the other `Start*DetectionJob` families, keyed per family).
- `idempotency_tokens: HashMap<(JobFamily, String /* ClientRequestToken */), JobId>` so step 3's retry returns the first `JobId`.
- `job_id_seq: u64` plus the account id / region taken from the request context, used to mint `JobId` and the `JobArn`.
- Synchronous inference (`DetectDominantLanguage`, `DetectSentiment`) holds no state: it is a pure, deterministic function of (`Text`, `LanguageCode`) so repeated identical calls return identical scores.

All other 79 operations (document classifiers, entity recognizers, endpoints, flywheels, datasets, the other Detect*/Batch*/Start*/Stop* families, tagging, resource policies) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate (85 operations, `awsJson1_1`, target prefix `Comprehend_20171127.`) documents every field cited: DetectDominantLanguage (input `Text` required; output `Languages` list<DominantLanguage>, lines 231-235), DetectSentiment (input `Text`+`LanguageCode` required; output `Sentiment` enum MIXED|NEGATIVE|NEUTRAL|POSITIVE and `SentimentScore`, lines 255-259), StartSentimentDetectionJob (required `InputDataConfig`/`OutputDataConfig`/`DataAccessRoleArn`/`LanguageCode`, `ClientRequestToken` @idempotencyToken; output `JobId`/`JobArn`/`JobStatus`, lines 458-462), DescribeSentimentDetectionJob (input `JobId` required; output `SentimentDetectionJobProperties`; declares `JobNotFoundException` HTTP 404, lines 213-217), ListSentimentDetectionJobs (@paginated inputToken=NextToken outputToken=NextToken, `Filter`/`MaxResults` 1..500, lines 383-388), StopSentimentDetectionJob (input `JobId` required; output `JobId`/`JobStatus`; `JobNotFoundException` HTTP 404, lines 506-510). All-stub status confirmed: `src/ops/detect_sentiment.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and all 88 files under `src/ops/` are the same generated scaffold. No tests exist for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `comprehend` client at it.
2. `DetectDominantLanguage` with Text="Bonjour, comment allez-vous aujourd'hui ?" -> expect 200 with `Languages[0].LanguageCode == "fr"` and a Score in (0,1]; today returns HTTP 501 NotImplemented.
3. `DetectSentiment` with Text="I love this product, it is wonderful!", LanguageCode="en" -> expect 200 with Sentiment="POSITIVE" and SentimentScore.Positive the largest of the four scores summing to ~1.0. Repeat the identical call and expect identical scores.
4. `DetectSentiment` with Text="This is terrible and I am very unhappy.", LanguageCode="en" -> expect Sentiment="NEGATIVE" (proves the result is derived from the text, not canned).
5. `StartSentimentDetectionJob` with JobName="demo-job", LanguageCode="en", InputDataConfig.S3Uri="s3://bucket/in/", OutputDataConfig.S3Uri="s3://bucket/out/", DataAccessRoleArn="arn:aws:iam::123456789012:role/ComprehendRole", ClientRequestToken="tok-1" -> expect 200 with a non-empty JobId, a JobArn containing that JobId, and JobStatus="SUBMITTED".
6. Repeat the previous call verbatim (same ClientRequestToken) -> expect the SAME JobId, and exactly one job in the store.
7. `DescribeSentimentDetectionJob` with that JobId -> expect SentimentDetectionJobProperties echoing the stored JobName="demo-job", LanguageCode="en", both S3 URIs and the role ARN, with SubmitTime populated.
8. `DescribeSentimentDetectionJob` with JobId="doesnotexist" -> expect JobNotFoundException (HTTP 404), not 200 and not 501.
9. `ListSentimentDetectionJobs` with MaxResults=1 -> expect the created job in SentimentDetectionJobPropertiesList; start a second job and verify MaxResults=1 returns one item plus a NextToken, and passing that NextToken returns the other job with no overlap.
10. `ListSentimentDetectionJobs` with Filter.JobName="demo-job" -> expect only the matching job; Filter.JobName="nope" -> expect an empty list.
11. `StopSentimentDetectionJob` with the first JobId -> expect 200 with JobStatus in {STOP_REQUESTED, STOPPED}; then `DescribeSentimentDetectionJob` on it and expect the stopped status to persist.
