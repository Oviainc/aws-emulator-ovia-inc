Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehendmedical.describe-job-malformed-jobid-rejected comprehendmedical.start-job-invalid-role-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical.describe-job-malformed-jobid-rejected|run#9|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: comprehendmedical.start-job-invalid-role-arn-pattern-rejected|run#14|AssertionError|assert code2 == "InvalidRequestException", f"expected InvalidRequestException for an absent role, got {code2}"
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# ComprehendMedical PHI job handlers answer 501 NotImplemented on the resolution paths, so absent-job and absent-role refusals never happen (2 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePHIDetectionJob

## Description
Two net-new behavioral causes in the ComprehendMedical PHI asynchronous-job surface. In BOTH checks the shared pre-handler validation layer now behaves correctly: a JobId that violates the modeled @pattern, and a DataAccessRoleArn that violates the modeled @pattern, are each refused with InvalidRequestException and AWS's exact message (those assertions pass). The divergence is on the NEXT phase of each check, where the input is well-formed and the request therefore reaches the operation handler: the handler is still a scaffold stub and answers HTTP 501 with error code `NotImplemented`, so the resolution-stage refusal AWS gives (absent job / untrusting-or-absent role) is never produced and the two negative classes are indistinguishable to a client.

Cause 1 of 2: DescribePHIDetectionJob with a well-formed JobId that names no job answers 501 NotImplemented instead of AWS's not-found refusal.
AWS: Smithy (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): DescribePHIDetectionJob input JobId (required, @length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$); output ComprehendMedicalAsyncJobProperties; modeled errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429 — 501/NotImplemented is NOT modeled. Recorded behavior for a JobId that satisfies the pattern/length but names no job: error code `ResourceNotFoundException` with Message `Could not find specified job.` — that message is entirely FIXED text for this error class; it has no variable part and in particular does NOT echo the requested JobId. The point of the check is that AWS validates syntax BEFORE existence, so the two negative classes carry different error codes and different messages.
Emulator: the handler body in src/ops/describe_phi_detection_job.rs returns Err(crate::scaffolded::Scaffolded::not_implemented("DescribePHIDetectionJob")) — HTTP 501, error code `NotImplemented`. The replay asserts `expected ResourceNotFoundException, got NotImplemented`.
Why: a client cannot tell "this job id is garbage" from "this job does not exist (expired, deleted, wrong account/region)". Polling code that treats ResourceNotFoundException as a terminal condition never sees it, and 501 is not classified by SDK error handling as either a client fault or a retryable server fault, so retry loops either spin or abort on an unmodeled code. Any branch keyed on the documented message text never matches.
Fix: implement the handler body of DescribePHIDetectionJob over the persisted job state (crate::state async-job store) and delete the SCAFFOLDED marker; the unknown-job branch must return the modeled not-found refusal with AWS's fixed message `Could not find specified job.` and must not interpolate the requested JobId into it. Note the sibling entities-detection family currently interpolates the JobId into its unknown-job message (an already-open wire issue); the PHI handler must not copy that.

Cause 2 of 2: StartPHIDetectionJob with a syntactically valid DataAccessRoleArn naming a role that does not exist answers 501 NotImplemented instead of AWS's InvalidRequestException.
AWS: Smithy: StartPHIDetectionJob input DataAccessRoleArn (IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$), InputDataConfig/OutputDataConfig/LanguageCode required; modeled errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429 — no 501. Recorded behavior: AWS reuses ONE error code, `InvalidRequestException`, for both arn negative classes and distinguishes them by MESSAGE. A malformed arn gets `DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.` (the emulator already produces this — that assertion passes). A well-formed arn in the caller's own account naming a role that cannot be resolved gets the fixed text `The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam` — fixed text for this class, no value from the request or state, and the refusal happens BEFORE a JobId is minted.
Emulator: src/ops/start_phi_detection_job.rs:181 returns Err(crate::scaffolded::Scaffolded::not_implemented("StartPHIDetectionJob")) — HTTP 501, error code `NotImplemented`. The replay asserts `expected InvalidRequestException for an absent role, got NotImplemented`.
Why: the role-trust contract is the single most common submit-time failure of PHI batch analysis. A client is handed an unmodeled 501 instead of the 400 that tells it exactly what to fix, and because the code is the same for every reason the call fails, the emulator also destroys AWS's message-level distinction between a malformed arn and an unusable role.
Fix: implement StartPHIDetectionJob's handler body (src/ops/start_phi_detection_job.rs) and delete the SCAFFOLDED marker; after the modeled pattern validation passes, resolve the role and refuse one that does not exist or does not trust the comprehendmedical service principal with InvalidRequestException/400 carrying the fixed trust message above, before any JobId is minted.

Relation to open issues (neither OWNS these checks; the suite reported them as unowned, un-filed mismatches): 20261023T185154Z-comprehendmedical-the-phi-asynchronous-job-family-is-still.md carries only the replay keys of the two timed-out PHI checks (async-phi-job-completes-and-writes-output-to-s3, start-job-idempotency-token-reuse) and describes the happy-path/idempotency contract; the two negative-class contracts above are not gated by it, so fixing them must be confirmed by re-replaying the two check ids in Issue-Trace-Ids here. 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md covers the entities-detection-v2 family and the shared validation-message rendering — its Cause 5 (DescribePHIDetectionJob pattern violation with an empty Message) is the phase that now PASSES in both checks here.

Traces: comprehendmedical.describe-job-malformed-jobid-rejected (Cause 1), comprehendmedical.start-job-invalid-role-arn-pattern-rejected (Cause 2).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: comprehendmedical 9/21 (43%), 12 failed. Both failures below are verdict [mismatch], NOT marked "ALREADY FILED" and NOT marked "OWNED by" any open issue.

1) [mismatch] comprehendmedical.describe-job-malformed-jobid-rejected
   File "check_comprehendmedical_describe_job_malformed_jobid_rejected.py", line 42, in run
     assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
   AssertionError: expected ResourceNotFoundException, got NotImplemented
   Phase 1 of the same check PASSED: describe_phi_detection_job(JobId="not#a#valid#job#id!") returned InvalidRequestException with exactly "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$" (fixed text; variable parts = the violation count, the input member path 'jobId', and that member's modeled pattern). Phase 2, describe_phi_detection_job(JobId="a"*32) — pattern- and length-valid, names no job — expected code ResourceNotFoundException with the fixed Message "Could not find specified job."; observed HTTP 501 error code NotImplemented.

2) [mismatch] comprehendmedical.start-job-invalid-role-arn-pattern-rejected
   File "check_comprehendmedical_start_job_invalid_role_arn_pattern_rejected.py", line 89, in run
     assert code2 == "InvalidRequestException", f"expected InvalidRequestException for an absent role, got {code2}"
   AssertionError: expected InvalidRequestException for an absent role, got NotImplemented
   Phase 1 PASSED (client with parameter_validation=False, DataAccessRoleArn="not-a-valid-arn"): InvalidRequestException with exactly "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid." Phase 2 uses arn:aws:iam::<callers own account id from sts:GetCallerIdentity>:role/sim2real-nonexistent-role (the account id is a per-environment value, not part of the contract); expected code InvalidRequestException with the fixed Message "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam"; observed HTTP 501 error code NotImplemented.

Source proof that the handlers, not the validation layer, are the gap:
  services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/describe_phi_detection_job.rs:4 — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented"; same file:164 — Err(crate::scaffolded::Scaffolded::not_implemented("DescribePHIDetectionJob")).
  services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_phi_detection_job.rs:181 — Err(crate::scaffolded::Scaffolded::not_implemented("StartPHIDetectionJob")).
That both checks' first phases are refused with AWS's exact codes and messages proves requests do reach this service and the shared pre-handler validation path is correct; only the handler bodies are missing.

Model grounding (smithy_query on services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): DescribePHIDetectionJob errors = InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429. StartPHIDetectionJob errors = the same four; DataAccessRoleArn @length 20..2048 @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$. Neither operation models a 501/NotImplemented response, so the emulator's code is off-contract for the SDK.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point a botocore/AWS CLI comprehendmedical client at it.
2. Cause 1, syntax control (currently correct, keep it passing): `aws comprehendmedical describe-phi-detection-job --job-id 'not#a#valid#job#id!'` -> InvalidRequestException, Message "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$".
3. Cause 1, the divergence: `aws comprehendmedical describe-phi-detection-job --job-id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` (32 chars, satisfies the pattern, names no job).
   Expected: error code ResourceNotFoundException with Message "Could not find specified job." (fixed text, JobId NOT echoed).
   Actual: HTTP 501, error code NotImplemented ("operation DescribePHIDetectionJob is scaffolded but not implemented in this emulator").
4. Create an S3 bucket with an object at key input/doc.txt (used only so the data configs are valid).
5. Cause 2, syntax control (currently correct): with a client built using botocore Config(parameter_validation=False), call start_phi_detection_job(InputDataConfig={S3Bucket:<b>,S3Key:'input/doc.txt'}, OutputDataConfig={S3Bucket:<b>,S3Key:'output/'}, DataAccessRoleArn='not-a-valid-arn', LanguageCode='en') -> InvalidRequestException, Message "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid."
6. Cause 2, the divergence: take ACCOUNT=$(aws sts get-caller-identity --query Account --output text) and call the same operation with --data-access-role-arn arn:aws:iam::$ACCOUNT:role/sim2real-nonexistent-role (well-formed, role absent, same account so cross-account PassRole is not in play).
   Expected: error code InvalidRequestException with Message "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam", and NO job minted.
   Actual: HTTP 501, error code NotImplemented.
7. Re-replay comprehendmedical.describe-job-malformed-jobid-rejected and comprehendmedical.start-job-invalid-role-arn-pattern-rejected to confirm both causes; each check also re-asserts its first-phase message, so a fix must not regress the validation-layer messages.
