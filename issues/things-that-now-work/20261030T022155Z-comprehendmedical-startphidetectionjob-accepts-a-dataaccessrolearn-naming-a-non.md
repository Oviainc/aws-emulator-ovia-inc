Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehendmedical.start-job-invalid-role-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical.start-job-invalid-role-arn-pattern-rejected|_error_code_and_message#13|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: d2ebcd8c12cc6501ce5e652d8f3a2ce865548576

# ComprehendMedical StartPHIDetectionJob accepts a DataAccessRoleArn naming a non-existent role and returns a JobId instead of refusing with InvalidRequestException (1 cause)

> _Note: this issue was identified against the code base at commit `d2ebcd8c12cc6501ce5e652d8f3a2ce865548576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPHIDetectionJob

## Description
One net-new behavioral cause in ComprehendMedical's PHI asynchronous-job submit path. The shared pre-handler validation layer is now correct (the modeled @pattern refusal for a malformed ARN passes), and the handler is no longer a 501 scaffold stub, but the handler performs NO resolution of the submitted DataAccessRoleArn beyond the syntactic pattern check: a syntactically valid ARN in the caller's own account naming a role that does not exist (and therefore cannot trust the Comprehend Medical service principal) is ACCEPTED and a JobId is minted, where AWS refuses the submission.

Cause 1 of 1: StartPHIDetectionJob succeeds (2xx with a minted JobId) for a well-formed DataAccessRoleArn whose role cannot be resolved, instead of refusing with InvalidRequestException/400.
AWS: Smithy (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): StartPHIDetectionJob input DataAccessRoleArn -> IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$; InputDataConfig, OutputDataConfig and LanguageCode also required; output carries only JobId (@length 1..32). Modeled errors: InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429. The model constrains only the ARN's SYNTAX; recorded behavior shows AWS additionally RESOLVES the role at submit time, BEFORE any JobId is minted, and refuses a role it cannot resolve or that does not trust the service principal with HTTP 400, error code InvalidRequestException and the message "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam". That message is ENTIRELY FIXED text for this error class — it has no variable part: it does not echo the ARN, the role name, the account id, the job name or the bucket. AWS reuses the SAME error code for the malformed-ARN class with a DIFFERENT fixed text, "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.", so the code alone does not identify the class — the message does. The emulator already emits that malformed-ARN message correctly (phase 1 of the check passes); only the resolution-stage refusal is missing.
Emulator: the call returns success. The replay fails inside the check's _error_code_and_message helper with "AssertionError: expected the call to be rejected, but it succeeded" at the absent-role phase (line 81 of the check, raised from line 23) — i.e. no error code and no Message at all are produced; the handler mints a JobId and records the job. The expected status/code/message (400 / InvalidRequestException / the fixed trust text above) are absent entirely.
Why: role-trust failure is the most common submit-time failure of PHI batch analysis, and it is the one AWS reports synchronously so a caller can fix its IAM configuration before waiting on a job. Against the emulator an IAM misconfiguration is reported as a working submission: the caller gets a JobId for a job that can never produce output, every error-handling branch keyed on InvalidRequestException plus the documented trust message is dead code, and AWS's message-level distinction between "malformed ARN" and "unusable role" is destroyed because one of the two classes never occurs. Worse, the phantom job is persisted, so Describe/List report a job AWS never created.
Fix: StartPHIDetectionJob's handler, in its submit-time validation stage — after the modeled @pattern/@length checks and BEFORE a JobId is minted or any job is written to the async-job state, resolve the DataAccessRoleArn and return the modeled InvalidRequestException/400 carrying the fixed trust message when the role cannot be resolved or does not trust the comprehendmedical service principal. No JobId may be allocated and no job may be persisted on that path. The same resolution gate belongs on the sibling Start*Job operations.
Traces: comprehendmedical.start-job-invalid-role-arn-pattern-rejected.

Relation to open issues (neither OWNS this check — the suite reported it as neither ALREADY FILED nor OWNED by any open issue): 20261029T175936Z-comprehendmedical-phi-job-handlers-answer-501-notimplemented-on.md carries this check id in its Issue-Trace-Ids, but its replay key for it is the assertion `code2 == "InvalidRequestException" ... got NotImplemented`, which no longer reproduces: the handler no longer answers 501, it now answers success, so that issue's key does not gate this divergence. 20261030T011657Z-comprehendmedical-startphidetectionjob-skips-dataaccessrolearn-trust-resolution-and-mints.md describes the same root fault from the WIRE corpus (fixture comprehendmedical-phi-detection-job-family steps task#1/task#4) and is grounded in fixture steps, not in this behavioral check id; closing it must additionally be confirmed by re-replaying the check id listed here. 20261023T185154Z-comprehendmedical-the-phi-asynchronous-job-family-is-still.md covers the family's happy-path/idempotency contract, and 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md covers the entities-detection-v2 family's error contract.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: comprehendmedical 10/21 (48%), 11 failed. The failure below is verdict [mismatch] and is NOT marked "ALREADY FILED" and NOT marked "OWNED by" any open issue.

[mismatch] comprehendmedical.start-job-invalid-role-arn-pattern-rejected
  File "check_comprehendmedical_start_job_invalid_role_arn_pattern_rejected.py", line 81, in run
    code2, message2 = _error_code_and_message(
  File "check_comprehendmedical_start_job_invalid_role_arn_pattern_rejected.py", line 23, in _error_code_and_message
    raise AssertionError("expected the call to be rejected, but it succeeded")
  AssertionError: expected the call to be rejected, but it succeeded

Phase 1 of the same check PASSED: with a botocore client built using Config(parameter_validation=False), start_phi_detection_job(..., DataAccessRoleArn="not-a-valid-arn") was refused with error code InvalidRequestException and exactly "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid." (all fixed text). This proves requests reach the service, the modeled pattern validation is correct, and the handler is no longer a 501 stub.

Phase 2 is the divergence: the same operation with a well-formed ARN of the form arn:aws:iam::<caller's own account id from sts:GetCallerIdentity>:role/<absent-role-name> (the account id and role name are per-environment values and are NOT part of the contract; the contract is that an unresolvable role is refused).
  Expected: HTTP 400, error code InvalidRequestException, Message exactly "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam" — entirely fixed text, no value taken from the request or state — and NO JobId minted and no job persisted.
  Actual: the call SUCCEEDS (2xx) with a minted JobId; no error code and no Message are emitted.

Model grounding (smithy_query on services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): StartPHIDetectionJob errors = InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429; input DataAccessRoleArn target IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$; output = JobId only. The model carries no success contract for an unresolvable role, and the recorded refusal is InvalidRequestException/400.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point a botocore/AWS CLI comprehendmedical client at its data listener.
2. Create an S3 bucket in ctx.region with an object at key input/doc.txt, so InputDataConfig/OutputDataConfig are valid and the submission can only fail on the role.
3. Syntax control (currently correct, must keep passing): with a client built using botocore Config(parameter_validation=False), call start_phi_detection_job(InputDataConfig={S3Bucket:<bucket>,S3Key:'input/doc.txt'}, OutputDataConfig={S3Bucket:<bucket>,S3Key:'output/'}, DataAccessRoleArn='not-a-valid-arn', LanguageCode='en').
   Expected and observed: error code InvalidRequestException, Message "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid."
4. The divergence: take ACCOUNT=$(aws sts get-caller-identity --query Account --output text) and call the same operation with --data-access-role-arn arn:aws:iam::$ACCOUNT:role/sim2real-nonexistent-role (well-formed, role absent, same account so cross-account PassRole is not in play).
   Expected: HTTP 400, error code InvalidRequestException, Message exactly "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam", and no JobId returned.
   Actual: HTTP 2xx with a minted JobId.
5. Then call list_phi_detection_jobs with no filter and confirm the refused submission left NO job in the store (the fix must gate the state write, not only the response).
6. Re-replay comprehendmedical.start-job-invalid-role-arn-pattern-rejected to confirm the fix; the check re-asserts the phase-1 malformed-ARN message too, so the fix must not regress the validation-layer message.
