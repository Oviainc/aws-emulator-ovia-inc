Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-phi-detection-job-family
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-phi-detection-job-family
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|StartPHIDetectionJob|4xx->2xx|$.Message
Issue-Key: comprehendmedical|StartPHIDetectionJob|4xx->2xx|$.__type
Issue-Key: comprehendmedical|StartPHIDetectionJob|4xx->2xx|status
Issue-Key: comprehendmedical|ListPHIDetectionJobs|2xx->2xx|$.ComprehendMedicalAsyncJobPropertiesList[*]
Issue-Commit: dd49200ab94ba4871d8de05234c5eb8d60052131

# ComprehendMedical StartPHIDetectionJob skips DataAccessRoleArn trust resolution and mints a phantom PHI job (2 causes)

> _Note: this issue was identified against the code base at commit `dd49200ab94ba4871d8de05234c5eb8d60052131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPHIDetectionJob

## Description
Two net-new wire causes in the ComprehendMedical PHI asynchronous-job family, sharing ONE root fault: StartPHIDetectionJob's handler performs no resolution of the submitted DataAccessRoleArn beyond the modeled @pattern check, so a role that AWS refuses to use is accepted, a JobId is minted and the resulting job is persisted into the service's job store — where it then leaks out of the list operation as a job AWS never created. Both causes must be closed before submitting.

Cause 1 of 2: StartPHIDetectionJob answers 200 with a minted JobId where AWS refuses the submission with InvalidRequestException/400 because the supplied IAM role does not trust the Comprehend Medical service principal. Fixture comprehendmedical-phi-detection-job-family, step task#1.
AWS: Smithy (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): StartPHIDetectionJob input DataAccessRoleArn -> IamRoleArn, required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$; InputDataConfig, OutputDataConfig and LanguageCode also required; output carries only JobId (@length 1..32, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$); modeled errors are InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429. The model constrains only the ARN's SYNTAX; the recorded response shows AWS additionally RESOLVES the role at submit time and refuses one that does not trust the service principal, BEFORE any JobId is minted: HTTP 400, error code (JSON `__type`) `InvalidRequestException`, message `The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam`. That message is entirely FIXED text for this error class — it has no variable part: it does not echo the ARN, the role name, the account id, the job name or the bucket. (The sibling malformed-ARN class is the same error code with a different fixed text, `DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.`, so the code alone does not identify the class; the message does.)
Emulator: replay shows status 200 with a StartPHIDetectionJobResponse body; the response carries neither `__type` nor `Message` (`$.__type: missing (expected 'InvalidRequestException', got None)`, `$.Message: missing`). The handler goes straight from idempotency-token lookup to minting a JobId and recording an AsyncJob in the store; there is no role resolution step at all.
Why: the role-trust failure is the most common submit-time failure of PHI batch analysis, and it is the one AWS reports synchronously so a caller can fix its IAM configuration before waiting on a job. Against the emulator the caller gets a success and a JobId, so an IAM misconfiguration is reported as a working submission; any error-handling branch keyed on InvalidRequestException plus the documented trust message is dead code, and integration tests that assert the refusal pass against AWS and fail only in the emulator (or, worse, pass in the emulator and ship a broken role).
Fix: StartPHIDetectionJob's handler, in its submit-time validation stage — after the modeled @pattern/@length checks and BEFORE a JobId is minted or any job is recorded in state, resolve the DataAccessRoleArn and return the modeled InvalidRequestException/400 with the fixed trust message above when the role cannot be resolved or does not trust the comprehendmedical service principal. No JobId may be allocated and no job may be persisted on that path. The same resolution gate belongs on the sibling Start*Job operations, which share this fault (see StartEntitiesDetectionV2Job, covered for the entities family by open issue 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md).
Traces: comprehendmedical-phi-detection-job-family#task#1. Handler source: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_phi_detection_job.rs:187-209 (token lookup, then mint_job_id + phi_detection_jobs.record with no role check).

Cause 2 of 2: ListPHIDetectionJobs reports a PHI job that AWS never created, so the list operation's result set is not the set of jobs the service accepted. Fixture comprehendmedical-phi-detection-job-family, step task#4.
AWS: Smithy: ListPHIDetectionJobs input Filter/NextToken/MaxResults (@range 1..500), output ComprehendMedicalAsyncJobPropertiesList + NextToken; modeled errors InternalServerException/500, InvalidRequestException/400, TooManyRequestsException/429, ValidationException/400. Recorded behavior: after the submission in task#1 was refused, the list for that account/region contains NO entry for it — the response carries an empty ComprehendMedicalAsyncJobPropertiesList. The invariant is that the list reflects only submissions the service ACCEPTED; a refused StartPHIDetectionJob leaves no listable job.
Emulator: status 200, but the list has one extra element (`$.ComprehendMedicalAsyncJobPropertiesList[0]: extra (expected None, got {...JobStatus: SUBMITTED...})`) — the phantom job minted by Cause 1, carrying the very DataAccessRoleArn whose submission AWS rejected, in state SUBMITTED.
Why: a client that enumerates jobs to reconcile work (poll for completion, resume after restart, report status to a user) sees a job that does not exist on AWS and will never complete there, so reconciliation logic diverges permanently: counts, idempotency decisions driven by "is there already a job for this input?", and cleanup passes all act on an entry AWS does not have.
Why this is listed as its own cause rather than folded into Cause 1: the gap is in the state write, not only in the error path — the job must never enter the job store, so the fix must be confirmed on the LIST step as well as on the submit step.
Fix: the same submit-time gate in StartPHIDetectionJob's handler plus the service's async-job state: the refusal path must not record an AsyncJob, so ListPHIDetectionJobs (and DescribePHIDetectionJob) then naturally report nothing for it. ListPHIDetectionJobs' own handler/serializer need not change beyond returning the empty list when the store holds no matching job.
Traces: comprehendmedical-phi-detection-job-family#task#4.

Relation to open issues (neither OWNS these two steps — the replay reported both as unowned): 20261029T175936Z-comprehendmedical-phi-job-handlers-answer-501-notimplemented-on.md describes StartPHIDetectionJob answering 501 NotImplemented from a scaffold stub and is grounded in BEHAVIORAL check ids; the handler is now implemented and answers 200, so its replay keys do not gate the role-trust refusal reported here. 20261023T185154Z-comprehendmedical-the-phi-asynchronous-job-family-is-still.md covers the happy-path/idempotency contract of the family. 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md covers the entities-detection-v2 family's error contract, not the PHI family's.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200). Both clusters below are verdict [mismatch] with an EMPTY stub_path and an EMPTY owned_by, i.e. neither already filed nor owned by an open issue.

1) Cause 1 — fixture comprehendmedical-phi-detection-job-family, step_key task#1, operation StartPHIDetectionJob
   divergence_key: service=comprehendmedical|StartPHIDetectionJob|status=400->200|keys=$.Message,$.__type,status
   detail: status: expected 400, got 200
           $.__type: missing (expected 'InvalidRequestException', got None)
           $.Message: missing (expected 'The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam', got None)
   Expected status 400 / error code InvalidRequestException / the fixed trust message above (no variable parts). Actual: HTTP 200 with a minted JobId and no error members at all.

2) Cause 2 — fixture comprehendmedical-phi-detection-job-family, step_key task#4, operation ListPHIDetectionJobs
   divergence_key: service=comprehendmedical|ListPHIDetectionJobs|status=200->200|keys=$.ComprehendMedicalAsyncJobPropertiesList[0]
   detail: $.ComprehendMedicalAsyncJobPropertiesList[0]: extra (expected None, got {'JobId': ..., 'JobName': ..., 'JobStatus': 'SUBMITTED', 'SubmitTime': ..., 'ExpirationTime': ..., 'InputDataConfig': {...}, 'OutputDataConfig': {...}, 'LanguageCode': 'en', 'DataAccessRoleArn': 'arn:aws:iam::000000000000:role/sim2real-infra-lambda-exec-role', 'ModelVersion': '2.0.0'})
   Expected: ComprehendMedicalAsyncJobPropertiesList empty, because the submission in task#1 was refused. Actual: one SUBMITTED job — the phantom minted by Cause 1. (The JobId, JobName, bucket names and the SubmitTime/ExpirationTime values in the observed entry are per-run values and are not part of the contract; the contract is that there is NO entry.)

Source proof that the submit path performs no role resolution:
  services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_phi_detection_job.rs:173-183 — acquires the job store and expires jobs;
  :187-193 — the only pre-mint branch is the ClientRequestToken idempotency lookup;
  :195-209 — mint_job_id(now) then AsyncJob::submitted(...) / state.phi_detection_jobs.record(job), with the submitted data_access_role_arn stored verbatim and never resolved.

Model grounding (smithy_query on services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): StartPHIDetectionJob errors = InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429; DataAccessRoleArn required, @length 20..2048, @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$. ListPHIDetectionJobs output = ComprehendMedicalAsyncJobPropertiesList + NextToken.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point a botocore/AWS CLI comprehendmedical client at its data listener.
2. Create the S3 bucket and input/output prefixes the fixture uses, so InputDataConfig/OutputDataConfig are valid and the submission fails only on the role.
3. Cause 1: call StartPHIDetectionJob with InputDataConfig={S3Bucket:<bucket>,S3Key:'input/'}, OutputDataConfig={S3Bucket:<bucket>,S3Key:'output/'}, LanguageCode='en' and a syntactically valid DataAccessRoleArn naming a role that does not trust the Comprehend Medical service principal (the fixture uses arn:aws:iam::<account>:role/<some-non-trusting-role>; the account id and role name are per-environment values).
   Expected: HTTP 400, error code InvalidRequestException, Message exactly "The provided IAM role does not trust Comprehend Medical service principle. See: https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam" (all fixed text), and NO JobId minted.
   Actual: HTTP 200 with a JobId; no __type and no Message in the body.
4. Cause 2: immediately call ListPHIDetectionJobs with no filter.
   Expected: ComprehendMedicalAsyncJobPropertiesList empty — the refused submission created no job.
   Actual: one entry with JobStatus 'SUBMITTED' whose DataAccessRoleArn is the role rejected in step 3.
5. Re-replay fixture comprehendmedical-phi-detection-job-family and confirm BOTH steps: task#1 (StartPHIDetectionJob -> 400 InvalidRequestException with the fixed trust message) and task#4 (ListPHIDetectionJobs -> empty list). Closing only the submit refusal without keeping the job out of the store leaves task#4 diverging.
