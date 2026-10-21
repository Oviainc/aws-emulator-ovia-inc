Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the ComprehendMedical emulator

_Filed: 2026-10-21T21:38:13Z_
_Operation: service-com-amazonaws-comprehendmedical-comprehendmedical-20181030 | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 26 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no ComprehendMedical client can do anything at all. Implement a FIRST small slice: the synchronous entity-detection call plus the async entities-detection job lifecycle, which together are the core workflow of this service (awsJson1_1, `POST /` with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **DetectEntitiesV2** — input `Text` (required, len 1..20000). Output `Entities` (list<Entity>, required), `ModelVersion` (required), optional `UnmappedAttributes`, `PaginationToken`. Each `Entity` must carry `Id`, `BeginOffset`, `EndOffset`, `Score`, `Text`, `Category`, `Type`, `Traits`, `Attributes`. The returned entities must be DERIVED FROM THE SUBMITTED TEXT: `Text` must be the exact substring of the input at `[BeginOffset, EndOffset)`, `Id` monotonically increasing within the response, and a deterministic lexicon/matcher (e.g. a small built-in dictionary mapping terms to `EntityType`/`EntitySubType`) used for detection. A fixed canned entity list that ignores the input is NOT acceptable. Empty/oversize `Text` must raise `InvalidRequestException` (400) / `TextSizeLimitExceededException` (400).
2. **StartEntitiesDetectionV2Job** — input `InputDataConfig` (required, `S3Bucket` required + `S3Key`), `OutputDataConfig` (required, same shape), `DataAccessRoleArn` (required, must match `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`), `LanguageCode` (required, only `en`), optional `JobName`, `ClientRequestToken` (@idempotencyToken), `KMSKey`. Output `JobId` (len 1..32, generated, stored). Must persist a job record; repeating the same `ClientRequestToken` must return the SAME `JobId` rather than creating a second job.
3. **DescribeEntitiesDetectionV2Job** — input `JobId` (required). Output `ComprehendMedicalAsyncJobProperties` reconstructed from the stored job: `JobId`, `JobName`, `JobStatus`, `Message`, `SubmitTime`, `EndTime`, `ExpirationTime`, `InputDataConfig`, `OutputDataConfig`, `LanguageCode`, `DataAccessRoleArn`, `ManifestFilePath`, `KMSKey`, `ModelVersion` — i.e. exactly what StartEntitiesDetectionV2Job was given. An unknown `JobId` must return `ResourceNotFoundException` (404); a `JobId` violating the length/pattern constraint must return `InvalidRequestException` (400).
4. **ListEntitiesDetectionV2Jobs** — input `Filter` (`JobName`, `JobStatus`, `SubmitTimeBefore`, `SubmitTimeAfter`), `MaxResults` (1..500), `NextToken`. Output `ComprehendMedicalAsyncJobPropertiesList` + `NextToken`. Must list the jobs actually created, honour every filter member, truncate at `MaxResults` and return an opaque `NextToken` that resumes exactly where the previous page stopped (absent on the last page).
5. **StopEntitiesDetectionV2Job** — input `JobId` (required). Output `JobId`. Must transition the stored job's `JobStatus` to `STOP_REQUESTED`/`STOPPED` and set `EndTime`, so a subsequent Describe observes the new status. Unknown `JobId` → `ResourceNotFoundException` (404).

Job status progression must be observable: a newly started job is `SUBMITTED`/`IN_PROGRESS` and reaches `COMPLETED` (deterministically, e.g. on the next Describe or after a short simulated delay) unless stopped.

NEVER STUBS: every one of these five operations must compute its response from persisted state. No hardcoded responses, no echoing the request back without storing it, no generating a `JobId` that is never recorded.

## State Model

In `src/state.rs`, on the service state struct reached via `crate::state::service_state(&*self.engine)`:

- `entities_detection_v2_jobs: BTreeMap<String /* JobId */, AsyncJob>` — insertion/submit-time ordered store of async entities-detection jobs.
- `AsyncJob { job_id: String, job_name: Option<String>, job_status: JobStatus, message: Option<String>, submit_time: DateTime, end_time: Option<DateTime>, expiration_time: Option<DateTime>, input_data_config: InputDataConfig { s3_bucket, s3_key }, output_data_config: OutputDataConfig { s3_bucket, s3_key }, language_code: LanguageCode, data_access_role_arn: String, manifest_file_path: Option<String>, kms_key: Option<String>, model_version: String, client_request_token: Option<String> }`.
- `idempotency_tokens: HashMap<String /* ClientRequestToken */, String /* JobId */>` — makes StartEntitiesDetectionV2Job idempotent.
- `next_job_seq: u64` — source of unique JobIds (<=32 chars).
- `list_page_tokens: HashMap<String /* NextToken */, usize /* offset */>` (or an encoded last-seen JobId) — backs ListEntitiesDetectionV2Jobs pagination.
- A static `medical_lexicon: &[(term, EntityType, EntitySubType)]` table used by DetectEntitiesV2 so detections are a deterministic function of the input `Text` (no persistence needed, but no canned output either).

The other 21 operations (DetectEntities, DetectPHI, Infer*, and the ICD10CM/RxNorm/SNOMEDCT/PHI job families) stay 501 for now and will be filed as individual gap issues; once the entities family above works, those reuse the same `AsyncJob` structures keyed per family.

## Evidence

All 26 handler files under `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, i.e. zero operations are implemented and no state exists. Operation contracts are taken from the crate's `OPERATIONS.md` (DetectEntitiesV2, StartEntitiesDetectionV2Job, DescribeEntitiesDetectionV2Job, ListEntitiesDetectionV2Jobs, StopEntitiesDetectionV2Job, with their required members, constraints and modeled errors) and from the Smithy model shapes `ComprehendMedicalAsyncJobProperties`, `ComprehendMedicalAsyncJobFilter`, `InputDataConfig`, `OutputDataConfig` and `Entity` (queried via smithy_query). Wire protocol per OPERATIONS.md: awsJson1_1, target prefix `ComprehendMedical_20181030`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI comprehendmedical client at it.
2. `aws comprehendmedical detect-entities-v2 --text 'Patient takes 50 mg of Ibuprofen twice daily for headache.'` → expect HTTP 200 with a non-empty `Entities` list whose each element's `Text` equals the input substring at its `BeginOffset`/`EndOffset`, plus a `ModelVersion`; today returns HTTP 501 NotImplemented.
3. `aws comprehendmedical start-entities-detection-v2-job --input-data-config S3Bucket=in-bucket,S3Key=docs/ --output-data-config S3Bucket=out-bucket --data-access-role-arn arn:aws:iam::123456789012:role/CMRole --language-code en --job-name demo-job --client-request-token tok-1` → expect 200 with a `JobId`.
4. Repeat the identical start call with `--client-request-token tok-1` → expect the SAME `JobId` (idempotency), not a new job.
5. `aws comprehendmedical describe-entities-detection-v2-job --job-id <JobId>` → expect `ComprehendMedicalAsyncJobProperties` echoing the stored `JobName=demo-job`, both data configs, `DataAccessRoleArn`, `LanguageCode=en`, a `SubmitTime`, and a real `JobStatus`.
6. `aws comprehendmedical list-entities-detection-v2-jobs --filter JobName=demo-job --max-results 1` → expect exactly the job just created; with two jobs created, `--max-results 1` must return a `NextToken` whose next page returns the second job and no token.
7. `aws comprehendmedical stop-entities-detection-v2-job --job-id <JobId>` → expect 200 with the `JobId`, then Describe shows `JobStatus` STOP_REQUESTED/STOPPED with an `EndTime`.
8. `aws comprehendmedical describe-entities-detection-v2-job --job-id doesnotexist` → expect `ResourceNotFoundException` (HTTP 404).
