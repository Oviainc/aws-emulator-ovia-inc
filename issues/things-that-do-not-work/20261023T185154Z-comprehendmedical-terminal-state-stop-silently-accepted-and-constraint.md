Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehendmedical.async-job-stop-transition-and-duplicate-stop-rejected comprehendmedical.describe-job-malformed-jobid-rejected comprehendmedical.detect-entities-text-exceeds-size-limit-rejected comprehendmedical.infer-rxnorm-text-exceeds-ontology-size-limit-rejected comprehendmedical.list-jobs-filter-jobname-pattern-rejected comprehendmedical.list-jobs-max-results-range-validation-rejected comprehendmedical.start-job-invalid-role-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical.async-job-stop-transition-and-duplicate-stop-rejected|_error#9|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded: {result!r}")
Issue-Key: comprehendmedical.describe-job-malformed-jobid-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehendmedical.detect-entities-text-exceeds-size-limit-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehendmedical.infer-rxnorm-text-exceeds-ontology-size-limit-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehendmedical.list-jobs-filter-jobname-pattern-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehendmedical.list-jobs-max-results-range-validation-rejected|run#8|AssertionError|assert message_over == expected_over, f"expected message {expected_over!r}, got {message_over!r}"
Issue-Key: comprehendmedical.start-job-invalid-role-arn-pattern-rejected|run#10|AssertionError|assert message1 == expected1, f"expected message {expected1!r}, got {message1!r}"
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# ComprehendMedical: terminal-state stop silently accepted and constraint rejections carry non-AWS messages (3 causes)

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopEntitiesDetectionV2Job

## Description
This issue bundles 3 distinct root causes in the implemented ComprehendMedical slice (the asynchronous entities-detection family plus the shared request-validation layer). All 3 must be closed before submitting.

=== Cause 1 of 3: stopping an already-terminal job is accepted with 200 instead of InvalidRequestException ===
AWS: StopEntitiesDetectionV2Job models InvalidRequestException (client, HTTP 400) and ResourceNotFoundException (404) (Smithy: StopEntitiesDetectionV2Job errors = InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404). Recorded behavior: once a job has reached a terminal status (STOPPED), a second Stop for the same JobId is REJECTED with HTTP 400 InvalidRequestException; the recorded message is, as an example, "Job is already in terminal state.". The job's state is unchanged by the refused call.
Emulator: the second Stop on a job already observed as STOPPED returns HTTP 200 with a body echoing {"JobId": ...} (replay shows content-type application/x-amz-json-1.1, content-length 44, no error). The handler documents this intentionally: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/stop_entities_detection_v2_job.rs header says "a job that already reached a terminal status keeps it" — it keeps the status but still answers success.
Why: a client cannot tell a stop that took effect from a stop that was a no-op on a finished job. Idempotent-retry and job-reaper logic that relies on the 400 to detect "already finished" will treat the terminal job as still stoppable, loop, or report a successful cancellation of a job that in fact completed or was already stopped.
Fix: the StopEntitiesDetectionV2Job handler's state-transition guard — it must classify the stored job's current status and return the modeled InvalidRequestException for terminal statuses (COMPLETED / FAILED / STOPPED / PARTIAL_SUCCESS) rather than returning the success output. The same guard is owed by the sibling Stop operations of the other job families.
Traces: comprehendmedical.async-job-stop-transition-and-duplicate-stop-rejected (the check never reached its code assertion: the duplicate stop simply succeeded).

=== Cause 2 of 3: modeled-constraint rejections do not use AWS's per-member validation message format ===
AWS: for an input member that violates a modeled constraint, ComprehendMedical returns the standard AWS validation message, built from the offending member path and the constraint it failed: "N validation error(s) detected: Value at '<memberPath>' failed to satisfy constraint: Member must <constraint>". The member path and constraint come straight from the model, so the rule producing the text is known (not a recorded-only string). Smithy constraints confirming each: JobId @length 1..32 @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$ (DescribePHIDetectionJob.JobId, required); ComprehendMedicalAsyncJobFilter.JobName @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$ (ListPHIDetectionJobs.Filter); MaxResultsInteger @range min 1 max 500 (ListEntitiesDetectionV2Jobs.MaxResults); DetectEntitiesV2.Text -> BoundedLengthString @length 1..20000; InferRxNorm.Text -> OntologyLinkingBoundedLengthString @length 1..10000. The top-level error CODE is correct in every one of these replays (InvalidRequestException for jobId/filter.jobName/maxResults, TextSizeLimitExceededException for oversized Text) — only the message rule diverges.
Emulator: the shared validation layer emits a single generic sentence instead, independent of which constraint failed: "The request that you made is invalid: member(s) <names> did not satisfy the constraints this service declares. Check your request to determine why it's invalid and then retry the request." and, for text size, "The size of the text you submitted exceeds the size limit. Reduce the size of the text or use a smaller document and then retry your request. Offending member(s): text." It names the member but never states the constraint (the pattern, the length ceiling, or the numeric bound), and never uses the "N validation error(s) detected: Value at '...' failed to satisfy constraint: ..." form. Observed for: 'jobId' (pattern), 'filter.jobName' (pattern), 'maxResults' (range, value 501), 'text' (length 20000 and 10000).
Why: the message is the only place AWS reports WHICH constraint was violated, since one code (InvalidRequestException) covers every constrained member of an operation. Clients and SDK-level test suites that parse the validation message to surface "maxResults must be <= 500" or to retry with a corrected value get no machine-usable detail, and anything asserting on AWS's documented validation format diverges.
Fix: the service's shared request-validation/serializer path that turns a modeled constraint violation into an error body — it should render one entry per failing member from the member path and the violated constraint (pattern / length / range), in AWS's "N validation error(s) detected: Value at '<path>' failed to satisfy constraint: Member must ..." form, and keep the existing per-operation error code selection.
Traces: comprehendmedical.describe-job-malformed-jobid-rejected, comprehendmedical.list-jobs-filter-jobname-pattern-rejected, comprehendmedical.list-jobs-max-results-range-validation-rejected, comprehendmedical.detect-entities-text-exceeds-size-limit-rejected, comprehendmedical.infer-rxnorm-text-exceeds-ontology-size-limit-rejected.

=== Cause 3 of 3: a malformed DataAccessRoleArn is not tagged DATA_ACCESS_ROLE_ARN_INVALID, and the offending member is reported twice ===
AWS: Smithy: StartPHIDetectionJob.DataAccessRoleArn -> IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$. Recorded behavior: a DataAccessRoleArn that fails that pattern is rejected with HTTP 400 InvalidRequestException whose message is tagged with the service's own reason code: "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.". The service reuses the SAME code (InvalidRequestException) for a well-formed ARN naming a role it cannot resolve/that does not trust the service principal, distinguishing the two cases only by the message — so the reason tag is the discriminator, not decoration.
Emulator: returns InvalidRequestException (code correct) with the generic constraint sentence from Cause 2, and additionally lists the member twice: "The request that you made is invalid: member(s) dataAccessRoleArn, dataAccessRoleArn did not satisfy the constraints this service declares...". The duplication indicates the same member is being collected once per violated constraint (length and pattern) without de-duplication.
Why: a client cannot tell "your ARN is syntactically wrong" from "that role does not exist / does not trust Comprehend Medical", which are fixed in completely different ways (correct the string vs. create/retrust the role). The duplicated member name also misreports how many members were at fault.
Fix: the StartPHIDetectionJob input validation for DataAccessRoleArn (and the equivalent Start* operations of the other job families) must emit the DATA_ACCESS_ROLE_ARN_INVALID-tagged InvalidRequestException for a pattern/length failure on that member; the shared validation collector should report each failing member once.
Traces: comprehendmedical.start-job-invalid-role-arn-pattern-rejected (failed on the first phase, the malformed-ARN message; its second phase — a well-formed but absent role ARN, which must also be InvalidRequestException — is blocked by the unimplemented PHI job family filed separately).

Related: the open issue "ComprehendMedical's text-inference operations are still stubs" (20261022T000000Z-stub-text-inference-...) covers the 501 answers of DetectEntitiesV2/InferRxNorm/etc. It does NOT cover any cause above: the two Text-length causes here are the pre-handler validation layer's messages, which are reached and answered with a 400 BEFORE the stub handler runs (that issue's own evidence notes the 400 refusal table), and Causes 1 and 3 are in implemented code.

## Evidence
Suite: comprehendmedical 0/21 (0%).

Cause 1 — comprehendmedical.async-job-stop-transition-and-duplicate-stop-rejected [mismatch]:
  expected: ClientError InvalidRequestException (HTTP 400) on the second StopEntitiesDetectionV2Job for a job already polled to STOPPED
  actual:   AssertionError: expected a ClientError but the call succeeded: {'JobId': '00065e8665e440260000000000000000', 'ResponseMetadata': {'HTTPStatusCode': 200, ...'content-length': '44'...}}
  (the first Stop and the STOPPED transition themselves worked, so this is purely the missing terminal-state guard)

Cause 2 — message rule diverges, error code correct in all five:
  describe-job-malformed-jobid-rejected (JobId="not#a#valid#job#id!"):
    expected: "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$"
    actual:   "The request that you made is invalid: member(s) jobId did not satisfy the constraints this service declares. Check your request to determine why it's invalid and then retry the request."
  list-jobs-filter-jobname-pattern-rejected (Filter={"JobName": "bad#name#!"}):
    expected: "1 validation error detected: Value at 'filter.jobName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$"
    actual:   "...member(s) filter.jobName did not satisfy the constraints this service declares..."
  list-jobs-max-results-range-validation-rejected (MaxResults=501, client-side validation disabled):
    expected: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500"
    actual:   "...member(s) maxResults did not satisfy the constraints this service declares..."
    (the MaxResults=0 lower-bound case, expected "...Member must have value greater than or equal to 1", was not reached)
  detect-entities-text-exceeds-size-limit-rejected (Text > 20000 chars, code TextSizeLimitExceededException matched):
    expected: "1 validation error detected: Value at 'text' failed to satisfy constraint: Member must have length less than or equal to 20000"
    actual:   "The size of the text you submitted exceeds the size limit. Reduce the size of the text or use a smaller document and then retry your request. Offending member(s): text."
  infer-rxnorm-text-exceeds-ontology-size-limit-rejected (Text > 10000 chars, code matched):
    expected: "1 validation error detected: Value at 'text' failed to satisfy constraint: Member must have length less than or equal to 10000"
    actual:   same size-limit sentence as above

Cause 3 — comprehendmedical.start-job-invalid-role-arn-pattern-rejected [mismatch] (DataAccessRoleArn="not-a-valid-arn", parameter_validation=False so it reaches the service; code InvalidRequestException matched):
  expected: "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid."
  actual:   "The request that you made is invalid: member(s) dataAccessRoleArn, dataAccessRoleArn did not satisfy the constraints this service declares. Check your request to determine why it's invalid and then retry the request."

Model grounding (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json):
  StopEntitiesDetectionV2Job errors: InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404
  DescribePHIDetectionJob.JobId: required, @length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$
  ListPHIDetectionJobs/ListEntitiesDetectionV2Jobs.MaxResults -> MaxResultsInteger @range 1..500; Filter -> ComprehendMedicalAsyncJobFilter
  DetectEntitiesV2.Text -> BoundedLengthString @length 1..20000; InferRxNorm.Text -> OntologyLinkingBoundedLengthString @length 1..10000; both model TextSizeLimitExceededException/400
  StartPHIDetectionJob.DataAccessRoleArn -> IamRoleArn required @length 20..2048 @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$

Source: src/ops/stop_entities_detection_v2_job.rs header documents "a job that already reached a terminal status keeps it" (status kept, but a 200 still returned).

## Steps to Reproduce
Point a botocore/AWS CLI comprehendmedical client at the emulator.

Cause 1 (terminal-state stop):
1. Create an S3 bucket with an object under input/ and an IAM role, then StartEntitiesDetectionV2Job with InputDataConfig={S3Bucket,S3Key:"input/"}, OutputDataConfig={...,"output/"}, DataAccessRoleArn=<role>, LanguageCode=en.
2. StopEntitiesDetectionV2Job(JobId=<id>), then poll DescribeEntitiesDetectionV2Job until JobStatus == STOPPED.
3. Call StopEntitiesDetectionV2Job(JobId=<id>) again.
   Expected: HTTP 400 InvalidRequestException. Actual: HTTP 200 with {"JobId": "<id>"}.

Cause 2 (validation message format):
4. DescribePHIDetectionJob(JobId="not#a#valid#job#id!") -> InvalidRequestException with a generic message instead of "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ...".
5. ListPHIDetectionJobs(Filter={"JobName": "bad#name#!"}) -> same generic form for 'filter.jobName'.
6. With a client built as Config(parameter_validation=False): ListEntitiesDetectionV2Jobs(MaxResults=501) and MaxResults=0 -> generic form instead of the 'maxResults' range message.
7. DetectEntitiesV2(Text=<20001+ chars>) -> TextSizeLimitExceededException with the prose size sentence instead of "...Value at 'text' failed to satisfy constraint: Member must have length less than or equal to 20000"; InferRxNorm(Text=<10001+ chars>) likewise for 10000.

Cause 3 (role ARN reason tag):
8. With Config(parameter_validation=False): StartPHIDetectionJob(InputDataConfig=..., OutputDataConfig=..., DataAccessRoleArn="not-a-valid-arn", LanguageCode="en").
   Expected: InvalidRequestException "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid."
   Actual: InvalidRequestException with the generic constraint sentence naming dataAccessRoleArn TWICE.
