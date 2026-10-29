Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-entities-detection-v2-job-family
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-entities-detection-v2-job-family comprehendmedical-describe-job-malformed-jobid-rejected
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|StartEntitiesDetectionV2Job|4xx->2xx|$.Message
Issue-Key: comprehendmedical|StartEntitiesDetectionV2Job|4xx->2xx|$.__type
Issue-Key: comprehendmedical|StartEntitiesDetectionV2Job|4xx->2xx|status
Issue-Key: comprehendmedical|DescribeEntitiesDetectionV2Job|4xx->4xx|$.Message
Issue-Key: comprehendmedical|DescribeEntitiesDetectionV2Job|4xx->4xx|status
Issue-Key: comprehendmedical|StopEntitiesDetectionV2Job|4xx->4xx|$.Message
Issue-Key: comprehendmedical|StopEntitiesDetectionV2Job|4xx->4xx|status
Issue-Key: comprehendmedical|ListEntitiesDetectionV2Jobs|2xx->2xx|$.ComprehendMedicalAsyncJobPropertiesList[*]
Issue-Key: comprehendmedical|DescribePHIDetectionJob|4xx->4xx|$.Message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# ComprehendMedical asynchronous job error contract diverges from AWS recordings

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartEntitiesDetectionV2Job

## Description
Five net-new wire divergences in the ComprehendMedical asynchronous-job surface share one root fault: the job handlers accept and answer requests that AWS refuses, and where they do refuse they pick a different modeled error, status and message than AWS. All five are replayed against the IMPLEMENTED entities-detection-v2 job family (its bootstrap issue is resolved), so this is a fidelity gap, not a stub gap.

Cause 1 of 5: StartEntitiesDetectionV2Job accepts a DataAccessRoleArn whose role does not trust the Comprehend Medical service principal.
AWS: Smithy: StartEntitiesDetectionV2Job input DataAccessRoleArn (IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$), InputDataConfig/OutputDataConfig/LanguageCode required; modeled errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429. Recorded response: HTTP 400 with __type 'InvalidRequestException' and Message 'The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam'. That message is entirely fixed text for this error class — it carries no value from the request or from state. AWS validates the role's trust policy BEFORE minting a JobId, so no job exists after the refusal.
Emulator: HTTP 200 with a minted JobId. src/ops/start_entities_detection_v2_job.rs::handle does an idempotency-token lookup, mints a job id and inserts an AsyncJob; it never looks at data_access_role_arn beyond copying it onto the stored record, so there is no trust check and no path that can return InvalidRequestException for an untrusting role.
Why: a client whose role is misconfigured is told the batch job started. It then polls a job that can never read the input bucket, instead of being told at submit time what to fix; error-handling code keyed on InvalidRequestException never runs, and the emulator's state accrues a job AWS would never have created.
Fix: StartEntitiesDetectionV2Job's handler (src/ops/start_entities_detection_v2_job.rs) must resolve the supplied role and refuse one that does not trust the comprehendmedical service principal with InvalidRequestException/400 and the fixed message above, before minting a JobId.

Cause 2 of 5: DescribeEntitiesDetectionV2Job answers the wrong error class, status and message for an unknown JobId.
AWS: Smithy: DescribeEntitiesDetectionV2Job input JobId (required, @length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$); errors InvalidRequestException/400, ResourceNotFoundException/404, InternalServerException/500, TooManyRequestsException/429. Recorded response for a well-formed JobId that names no job: HTTP 400 with Message 'Could not find specified job.' — fixed text, with no variable part: the requested JobId is NOT echoed.
Emulator: HTTP 404 with Message "No entities detection job was found for JobId '<jobId>'." (src/ops/describe_entities_detection_v2_job.rs:166-178 returns ResourceNotFoundException with that interpolated message).
Why: the status class flips from 400 to 404 and the error shape from InvalidRequestException to ResourceNotFoundException, so SDK error handling and retry classification branch differently than against AWS; a client matching on the documented message text never matches, and the emulator leaks the requested JobId into a message AWS keeps constant.
Fix: the unknown-job branch of DescribeEntitiesDetectionV2Job's handler must raise InvalidRequestException/400 with the fixed message 'Could not find specified job.'.

Cause 3 of 5: StopEntitiesDetectionV2Job has the same wrong unknown-job refusal.
AWS: Smithy: StopEntitiesDetectionV2Job input JobId (required, same constraints), output JobId; errors InvalidRequestException/400, ResourceNotFoundException/404, InternalServerException/500. Recorded response for an unknown JobId: HTTP 400, Message 'Could not find specified job.' (fixed text, no interpolation).
Emulator: HTTP 404 with the interpolated "No entities detection job was found for JobId '<jobId>'." message, from the same unknown-job pattern as Cause 2.
Why: same client impact as Cause 2 on the stop path — a stop against a job that has already expired or never existed is reported as a 404 resource error rather than AWS's 400 invalid-request, and the message differs from AWS's wording.
Fix: the unknown-job branch of StopEntitiesDetectionV2Job's handler (src/ops/stop_entities_detection_v2_job.rs); fix it together with Cause 2 so the two paths cannot drift.

Cause 4 of 5: ListEntitiesDetectionV2Jobs reports a job AWS never accepted.
AWS: Smithy: ListEntitiesDetectionV2Jobs output ComprehendMedicalAsyncJobPropertiesList + NextToken. Recorded response after the refused Start of Cause 1: HTTP 200 with NO ComprehendMedicalAsyncJobPropertiesList entry for that submission — a job that was refused at submit time is not listed.
Emulator: HTTP 200 whose ComprehendMedicalAsyncJobPropertiesList contains an extra element: the phantom job minted by Cause 1, in JobStatus SUBMITTED with the submitted InputDataConfig/OutputDataConfig/DataAccessRoleArn.
Why: list output is the client's inventory of batch work. An entry AWS would not have created makes a reconciliation loop (and any 'wait for all jobs' logic) track a job that cannot progress.
Fix: this is the state consequence of Cause 1 — once Start refuses an untrusting role before inserting into state.entities_detection_v2_jobs, the list projection (src/ops/list_entities_detection_v2_jobs.rs, over crate::state) is correct. Re-replay this step after fixing Cause 1 rather than filtering the list.

Cause 5 of 5: a JobId that violates the modeled pattern is refused with an EMPTY message.
AWS: Smithy: DescribePHIDetectionJob input JobId (required, @length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$). Recorded response for a JobId that violates that pattern: HTTP 400 whose Message is the standard constraint-violation report — fixed text '1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>', whose variable parts are the input member path (here 'jobId') and the member's own modeled pattern (here ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$). The error count prefix is the number of violations found.
Emulator: HTTP 400 with the right error type but NO Message member at all ($.Message missing, got None): the pre-handler validation layer rejects the request without rendering the constraint report.
Why: the status is right but the body carries no reason, so a client (or a human reading a CLI error) is told only that the request was invalid, never which member failed or which constraint it failed — exactly the information AWS's validation message exists to deliver. The same empty-message refusal applies to every pattern/length-constrained member in this service, not only jobId.
Fix: the service's request-validation layer must render AWS's constraint-violation message into the error's Message member for pattern (and length) violations; DescribePHIDetectionJob is the replayed example (src/ops/describe_phi_detection_job.rs is only the entry point — the defect is in the shared validation path).

Related open issues, neither of which covers these five causes: 'ComprehendMedical's text-inference operations are still stubs' (20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md) is about the synchronous inference operations, and its own 'Not in scope' section excludes the job families; 'ComprehendMedical: the PHI asynchronous job family is still a 501 stub' (20261023T185154Z-comprehendmedical-the-phi-asynchronous-job-family-is-still.md) is about the PHI job handlers answering 501 — Cause 5 here is reached by the validation layer BEFORE any handler, and Causes 1-4 are on the implemented entities-detection family.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator; 5 of the 9 divergence clusters, all service=comprehendmedical, verdict=mismatch, no stub_path and no owning issue.

1) comprehendmedical-entities-detection-v2-job-family#task#1 StartEntitiesDetectionV2Job: 'status: expected 400, got 200; $.__type: missing (expected 'InvalidRequestException', got None); $.Message: missing (expected 'The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam', got None)'.
2) comprehendmedical-entities-detection-v2-job-family#task#2 DescribeEntitiesDetectionV2Job: 'status: expected 400, got 404; $.Message: value_mismatch (expected 'Could not find specified job.', got "No entities detection job was found for JobId '<jobId>'.")'.
3) comprehendmedical-entities-detection-v2-job-family#task#3 StopEntitiesDetectionV2Job: 'status: expected 400, got 404; $.Message: value_mismatch (expected 'Could not find specified job.', got "No entities detection job was found for JobId '<jobId>'.")'.
4) comprehendmedical-entities-detection-v2-job-family#task#4 ListEntitiesDetectionV2Jobs: '$.ComprehendMedicalAsyncJobPropertiesList[0]: extra (expected None, got {JobStatus: SUBMITTED, ...})' — the emulator lists the job minted by the Start that AWS refused.
5) comprehendmedical-describe-job-malformed-jobid-rejected#task#1 DescribePHIDetectionJob: status 400 matched, '$.Message: missing (expected "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-%@]*)$", got None)'.

Source of Causes 2/3: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/describe_entities_detection_v2_job.rs:166-178 returns ResourceNotFoundException with an interpolated JobId message. Source of Cause 1/4: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_entities_detection_v2_job.rs:162-227 — handle() mints the job id and inserts the record with no validation of data_access_role_arn.

Model grounding (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): all four job operations model InvalidRequestException/400 alongside ResourceNotFoundException/404, so AWS's 400 choice is expressible as modeled.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS CLI/SDK comprehendmedical client at it.
2. Cause 1/4: create an IAM role whose trust policy does NOT name comprehendmedical.amazonaws.com, then `aws comprehendmedical start-entities-detection-v2-job --input-data-config S3Bucket=<b>,S3Key=input/ --output-data-config S3Bucket=<b>,S3Key=output/ --data-access-role-arn <thatRole> --language-code en`. Expect HTTP 400 InvalidRequestException with the fixed 'does not trust Comprehend Medical service principle' message; observe HTTP 200 and a JobId. Then `aws comprehendmedical list-entities-detection-v2-jobs` — expect no entry for that submission, observe the phantom SUBMITTED job.
3. Cause 2: `aws comprehendmedical describe-entities-detection-v2-job --job-id <well-formed id that names no job>`. Expect HTTP 400 with Message 'Could not find specified job.'; observe HTTP 404 ResourceNotFoundException with "No entities detection job was found for JobId '...'.".
4. Cause 3: the same JobId through `stop-entities-detection-v2-job`. Same expected/observed divergence.
5. Cause 5: `aws comprehendmedical describe-phi-detection-job --job-id 'bad$id!'` (violates the JobId @pattern). Expect HTTP 400 whose Message is "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$"; observe a 400 with no Message member.
6. Re-replay fixture comprehendmedical-entities-detection-v2-job-family (steps task#1..task#4) and comprehendmedical-describe-job-malformed-jobid-rejected (task#1) to confirm all five causes.
