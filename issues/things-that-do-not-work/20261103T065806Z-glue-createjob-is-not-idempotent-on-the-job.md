Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.reject-duplicate-job-name
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateJob|IdempotentParameterMismatchException
Issue-Commit: 3ecb0d6b353755c874093f3eab62bbd62a85b9a2

# Glue CreateJob is not idempotent on the job name: an identical create replay is refused 400 IdempotentParameterMismatchException instead of answering 200

> _Note: this issue was identified against the code base at commit `3ecb0d6b353755c874093f3eab62bbd62a85b9a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
REOPENS 20261022T171422Z-codecommit-codepipeline-glue-iam-lambda-codecommit-listbranches-and-glue-createjob-answer-501-notimplemented.md: its fix left the call refused. CreateJob no longer answers 501, but a second CreateJob under an existing job name is still refused with a 4xx where the recording answers 200, so the operation's idempotency contract is still unreachable.

Cause 1 of 1: CreateJob's duplicate-name branch refuses a create replay that carries the SAME job definition, and it chooses which refusal to raise by comparing members that are not part of the job definition's identity.

AWS: Smithy com.amazonaws.glue#CreateJob takes Name (@required, NameString len 1..255), Role (@required) and Command (@required) and answers 200 with CreateJobResponse$Name ("The unique name that was provided for this job definition"); Name is documented as "It must be unique in your account." The operation models AlreadyExistsException, ConcurrentModificationException, IdempotentParameterMismatchException ("The same unique identifier was associated with two different records.", HTTP 400), InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException and InternalServiceException. The model does not state which of those a repeat take; recorded behavior supplies it: a repeat CreateJob under an existing Name whose definition is unchanged (same Role, same Command including Name/PythonVersion/ScriptLocation, same MaxCapacity, same Timeout; the replay omits Tags) is answered 200 with the Name echoed, and the stored definition — in particular Command.ScriptLocation — is left exactly as the first create stored it: CreateJob is idempotent on the name. Only a repeat whose definition DIFFERS is refused, with HTTP 400, error code IdempotentParameterMismatchException and the message "Job already submitted with different configuration." — that sentence is entirely fixed text for this error class; it interpolates no name, id, value or limit from the request or from state. The recording also shows the refused mismatch attempt changes nothing: a subsequent identical create still succeeds and the ScriptLocation is still the original one. Tags are not part of the definition compared for this decision (the recorded idempotent replay sends no Tags at all while the stored job has them, and is still accepted).

Emulator: the identical replay is REFUSED with HTTP 400 IdempotentParameterMismatchException and the message "Job already submitted with different configuration." (botocore: "An error occurred (IdempotentParameterMismatchException) when calling the CreateJob operation: Job already submitted with different configuration."), where the recorded response is 200 with Name. Because that refusal fires on the idempotent replay, the check never reaches its mismatch leg, so the genuinely-differing create and the "refused attempt left no corrupted state" property are never exercised. Two defects live in that single branch: (a) its matching path answers AlreadyExistsException rather than 200 — i.e. no repeat of an existing name can ever succeed; and (b) its equality test treats members that are not part of the definition's identity as significant — notably Tags, which the replay omits — so a definitionally identical replay is classified as a mismatch and gets the mismatch error instead of even the branch's own already-exists answer.

Why: replaying CreateJob with an unchanged definition is the normal path for every retrying client — an SDK retry after a timeout, CloudFormation/CDK/Terraform re-applying an unchanged template, any idempotent deploy script. Against AWS the second call is a 200 no-op and the deploy proceeds; against the emulator it hard-fails 400, aborting the deploy, and the error it raises asserts the client's definition changed when it did not. Symmetrically, a client that really did change the definition under the same name cannot distinguish its own mistake from a harmless retry, because both answer the same code and message.

Fix: the duplicate-name arm of the CreateJob handler and the member set its equality test compares — services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs (the `if let Some(stored) = store.job(&region, &input.name)` arm, which returns IdempotentParameterMismatchException when `stored.matches_create_input(&input)` is false and AlreadyExistsException when it is true) together with `crate::state::JobEntry::matches_create_input`. A create whose definition matches the stored job must answer 200 with the stored Name and leave the entry untouched; only a differing definition may take the IdempotentParameterMismatchException path with its fixed message; Tags must not participate in that comparison.

Traces: glue.reject-duplicate-job-name.

## Evidence
Failing behavioral check glue.reject-duplicate-job-name (verdict: error).

Step: create job J (Command {Name 'pythonshell', PythonVersion '3.9', ScriptLocation s3://<bucket>/scripts/etl.py}, Role <glue-assumable role ARN>, MaxCapacity 0.0625, Timeout 10, Tags {sim2real: <check>}) -> 200. Then replay CreateJob with the identical Name/Role/Command/MaxCapacity/Timeout and no Tags.

Expected (recorded): HTTP 200, CreateJobResponse{Name: J}; GetJob(J).Job.Command.ScriptLocation unchanged.
Actual (emulator): HTTP 400, Error.Code 'IdempotentParameterMismatchException', Error.Message 'Job already submitted with different configuration.' — raised by botocore as botocore.errorfactory.IdempotentParameterMismatchException: An error occurred (IdempotentParameterMismatchException) when calling the CreateJob operation: Job already submitted with different configuration.

Consequently the later legs of the check are never reached:
- a CreateJob under the same name with a DIFFERENT ScriptLocation must answer 400 IdempotentParameterMismatchException with exactly 'Job already submitted with different configuration.' (fixed text, no per-request values);
- the stored Command.ScriptLocation must be unchanged after that refusal;
- a further identical CreateJob must succeed again.

Handler evidence: services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs returns IdempotentParameterMismatchException when state::JobEntry::matches_create_input is false and AlreadyExistsException when it is true — there is no 200 path for an existing name at all.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region any).
2. iam.create_role(...) with a trust policy for glue.amazonaws.com and attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole; take its ARN as R.
3. glue.create_job(Name='J', Role=R, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://b/scripts/etl.py'}, MaxCapacity=0.0625, Timeout=10, Tags={'sim2real':'x'}) -> 200 {'Name': 'J'}.
4. S = glue.get_job(JobName='J')['Job']['Command']['ScriptLocation'].
5. glue.create_job(Name='J', Role=R, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':S}, MaxCapacity=0.0625, Timeout=10)  — i.e. the identical definition, no Tags. Expect HTTP 200 with {'Name': 'J'} and get_job's ScriptLocation still S; observe HTTP 400 IdempotentParameterMismatchException 'Job already submitted with different configuration.'
6. glue.create_job(Name='J', Role=R, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://other/other.py'}, MaxCapacity=0.0625, Timeout=10) -> expect HTTP 400 IdempotentParameterMismatchException with exactly 'Job already submitted with different configuration.', and get_job's ScriptLocation still S.
7. Repeat step 5 -> expect 200 again.
8. Re-replay the check id in trace_ids: glue.reject-duplicate-job-name.
