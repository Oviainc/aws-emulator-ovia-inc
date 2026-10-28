Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-service-last-accessed-details-with-entities
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.get-service-last-accessed-details-with-entities|run#19|AssertionError|assert message == expected, f"NoSuchEntity message: expected {expected!r}, got {message!r}"
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# iam: GetServiceLastAccessedDetailsWithEntities refuses a never-issued JobId with the wrong NoSuchEntity message ("The job ID <id> cannot be found." instead of "Job <id> does not exist.")

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceLastAccessedDetailsWithEntities

## Description
One root cause, grounded in 1 failing behavioral check in service 'iam'. The failure is NOT marked ALREADY FILED or OWNED in the replay report, and it is not a refusal of an operation a fixed issue claimed to fix: the operation answers, with the right error code, and only the message text diverges.

Cause 1 of 1: the not-found refusal of GetServiceLastAccessedDetailsWithEntities carries a message whose fixed text is not AWS's. The error class and HTTP status are correct; the human-readable message is reworded.

AWS: Smithy com.amazonaws.iam#GetServiceLastAccessedDetailsWithEntities (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) — input JobId (required, jobIDType, @length exactly 36) and ServiceNamespace (required, @length 1..64, @pattern ^[\w-]*$); modeled errors NoSuchEntityException (client, httpError 404, awsQueryError code 'NoSuchEntity', httpResponseCode 404) and InvalidInputException (client, 400, awsQueryError 'InvalidInput'). For a JobId that passes shape validation but was never issued to the caller's account, AWS refuses with Error.Code 'NoSuchEntity' and (recorded behavior for the text) Error.Message `Job <JobId> does not exist.` — the fixed text is 'Job ' + ' does not exist.' and the single variable part is the JobId taken verbatim from the request.

Emulator: the same request is refused with Error.Code 'NoSuchEntity' (correct) but Error.Message `The job ID <JobId> cannot be found.` — a different fixed text ('The job ID ' + ' cannot be found.') around the same request-supplied JobId.

Why: a client relying on AWS's behavior that surfaces, logs, parses or asserts the IAM error message gets different text for the same condition; message-matching tests, log-scraping diagnostics and user-facing error surfaces that quote AWS's wording diverge, and operators comparing emulator output with AWS runbooks see a refusal that does not look like IAM's. Because the code is right, the divergence is invisible to a shape-only check and only shows up in text.

Fix: the GetServiceLastAccessedDetailsWithEntities handler's not-found path (the NoSuchEntityException it raises for an unresolvable JobId) must build its message from AWS's template — the fixed text 'Job ' ... ' does not exist.' with the request's JobId interpolated — rather than its own wording. The service-last-accessed job lookup/state is otherwise correct: the preceding assertions (a COMPLETED job, an EntityDetailsList for s3, an empty EntityDetailsList for a never-accessed 'lambda' namespace) all passed.

Traces: iam.get-service-last-accessed-details-with-entities (line 97: AssertionError: NoSuchEntity message: expected 'Job 00000000-0000-0000-0000-000000000000 does not exist.', got 'The job ID 00000000-0000-0000-0000-000000000000 cannot be found.'). Note the InvalidInput assertion that follows it in the same check (an unsupported ServiceNamespace must answer InvalidInput with 'Service namepace <ns> is either invalid or unsupported.') was never reached and is unverified until this is fixed.

## Evidence
Replay report, service iam:

  [mismatch] iam.get-service-last-accessed-details-with-entities
  File "check_iam_get_service_last_accessed_details_with_entities.py", line 97, in run
    assert message == expected
  AssertionError: NoSuchEntity message: expected 'Job 00000000-0000-0000-0000-000000000000 does not exist.', got 'The job ID 00000000-0000-0000-0000-000000000000 cannot be found.'

Expected (AWS): GetServiceLastAccessedDetailsWithEntities(JobId=<36-char id never issued>, ServiceNamespace='s3') -> HTTP 404, Error.Code 'NoSuchEntity', Error.Message 'Job <JobId> does not exist.' Fixed text: 'Job ' and ' does not exist.'; variable part: the JobId copied from the request (in this run the all-zero id the check derives from its real JobId so the value clears the @length 36 shape check).

Actual (emulator): Error.Code 'NoSuchEntity' (matches) with Error.Message 'The job ID <JobId> cannot be found.' Fixed text diverging: 'The job ID ' / ' cannot be found.'; the interpolated JobId is correct.

Passing context from the same check (so this is a message-only defect, not a broken operation): GenerateServiceLastAccessedDetails returned a JobId, GetServiceLastAccessedDetails reached COMPLETED, GetServiceLastAccessedDetailsWithEntities returned JobStatus COMPLETED with an EntityDetailsList for 's3' and an empty EntityDetailsList for 'lambda'.

Smithy citation: GetServiceLastAccessedDetailsWithEntities input JobId -> jobIDType @length {min 36, max 36} (required); errors NoSuchEntityException {client, httpError 404, awsQueryError {code 'NoSuchEntity', httpResponseCode 404}} and InvalidInputException {client, 400, awsQueryError {code 'InvalidInput'}}.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the emulator.
2. iam: CreateUser, AttachUserPolicy with arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess, GetUser to read the user ARN.
3. GenerateServiceLastAccessedDetails(Arn=<user arn>) -> JobId; poll GetServiceLastAccessedDetails(JobId) until JobStatus is COMPLETED.
4. Call GetServiceLastAccessedDetailsWithEntities(JobId=<a 36-character id that was never issued, e.g. the real JobId with every alphanumeric replaced by '0'>, ServiceNamespace='s3').
5. Observe Error.Code 'NoSuchEntity' (correct) with Error.Message 'The job ID <JobId> cannot be found.'; AWS returns 'Job <JobId> does not exist.' with the same code and HTTP 404.
6. After the fix, also verify the next assertion in the check: GetServiceLastAccessedDetailsWithEntities(JobId=<real job>, ServiceNamespace='not-a-real-aws-service-namespace') must answer Error.Code 'InvalidInput' with Error.Message 'Service namepace <namespace> is either invalid or unsupported.' (AWS's wording, including its 'namepace' spelling), the namespace being the request's value.
7. Re-replay iam.get-service-last-accessed-details-with-entities.
