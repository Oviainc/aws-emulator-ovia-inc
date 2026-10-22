Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.create-job-template-concurrent-same-client-token-dedup
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.create-job-template-concurrent-same-client-token-dedup|run#12|AssertionError|assert successes, f"expected at least one racer to win, got outcomes={outcomes}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# emr-containers CreateJobTemplate: clientToken idempotency/dedup contract unimplemented — every concurrent racer gets NotImplemented instead of one shared template id

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJobTemplate

## Description
One root cause (Cause 1 of 1): the emr-containers emulator has no working CreateJobTemplate, so the operation's @idempotencyToken clientToken contract — the behavior that makes N racing identical creates collapse to exactly one persisted job template — does not exist at all. Failing check: emr-containers.create-job-template-concurrent-same-client-token-dedup.

AWS: Smithy (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json): CreateJobTemplate is POST /jobtemplates, HTTP 200 on success; input members name (ResourceNameString, required, @length 1..64, @pattern ^[\.\-_/#A-Za-z0-9]+$), clientToken (ClientToken, required, @idempotencyToken, @length 1..64, @pattern \S) and jobTemplateData (required); output carries id (ResourceIdString @pattern ^[0-9a-z]+$), name, arn (JobTemplateArn) and createdAt. Modeled errors: ValidationException (httpError 400), ResourceNotFoundException (400), InternalServerException (500) — NotImplemented is not a modeled error for this operation and 501 is not a modeled status. Because clientToken is @idempotencyToken, concurrent creates that share one token and identical parameters must not fan out: recorded behavior shows at least one call returning HTTP 200 with a job-template id, every other successful call returning that SAME id, any losing racer refused with the modeled ValidationException (400) (the real service surfaces its underlying transaction conflict / same-token reuse conflict as ValidationException), and a subsequent ListJobTemplates showing exactly ONE template with that name. Requirements here are the status (200 for the winner, 400 for a loser) and the error code (ValidationException); any loser message text is an example only.

Emulator: every one of the 8 racing CreateJobTemplate calls fails with error code NotImplemented ("operation CreateJobTemplate is scaffolded but not implemented in this emulator"), so there is no winner, no shared id, no ValidationException for losers, and nothing is persisted for ListJobTemplates/DescribeJobTemplate to read back.

Why: a client that provisions job templates cannot create one at all, and specifically cannot rely on clientToken idempotency — retry-on-timeout logic (the exact reason clientToken exists) has no defined outcome, and SDK error dispatch that catches ValidationException never fires because an unmodeled NotImplemented code arrives instead. Any caller doing at-least-once create either sees a hard failure today or, if CreateJobTemplate is implemented without the dedup path, silently mints duplicate templates for one logical create.

Fix: CreateJobTemplate's handler and state in the emr-containers service crate (services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_job_template.rs plus the service's job-template state): implement the operation and key persisted templates by clientToken so a repeat/concurrent create with the same token and identical parameters returns the already-minted id (200) and a conflicting/losing racer is refused with the modeled ValidationException (400), with exactly one template visible to ListJobTemplates and DescribeJobTemplate. NOTE: the open scaffold-stub issue filed for UNIMPLEMENTED CreateJobTemplate (naming that same handler file, blocking 6 other checks in this suite) covers only standing the operation up; this issue additionally requires the clientToken dedup/no-fan-out semantics above, which a bare implementation does not supply. Close both before submitting.

Traces: emr-containers.create-job-template-concurrent-same-client-token-dedup.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (service emr-containers is bootstrapped and serving — other emr-containers operations answer on the wire).

[mismatch] emr-containers.create-job-template-concurrent-same-client-token-dedup
  check_emr_containers_create_job_template_concurrent_same_client_token_dedup.py, line 52, in run
    assert successes, f"expected at least one racer to win, got outcomes={outcomes}"
  AssertionError: expected at least one racer to win, got outcomes=[('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented'), ('err', 'NotImplemented')]

expected: >=1 call returns HTTP 200 with a job-template id; all successful calls return the same id; any failing call returns error code ValidationException (HTTP 400); ListJobTemplates then shows exactly one template with the requested name.
actual:   all 8 calls return error code NotImplemented ("operation CreateJobTemplate is scaffolded but not implemented in this emulator"); no template persisted.

Model evidence: CreateJobTemplate POST /jobtemplates code 200; clientToken required + @idempotencyToken (@length 1..64, @pattern \S); errors = {ValidationException 400, ResourceNotFoundException 400, InternalServerException 500} — NotImplemented is unmodeled.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Point an emr-containers client at it and pick one name N and one clientToken T, with jobTemplateData = {executionRoleArn: arn:aws:iam::<account>:role/<any>, releaseLabel: emr-6.15.0-latest, jobDriver: {sparkSubmitJobDriver: {entryPoint: s3://<bucket>/a.py}}}.
3. Issue 8 concurrent CreateJobTemplate calls, all with name=N, clientToken=T and that identical jobTemplateData.
4. Expected: at least one call returns HTTP 200 with an id; every successful call returns the SAME id; any failing call returns HTTP 400 with error code ValidationException. Observed: all 8 calls return error code NotImplemented.
5. Call ListJobTemplates: expected exactly one template named N; observed none.
6. Re-replay emr-containers.create-job-template-concurrent-same-client-token-dedup to confirm the fix.
