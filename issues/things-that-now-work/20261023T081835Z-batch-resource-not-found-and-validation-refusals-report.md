Issue-Source: trace-sweep-divergence
Issue-Trace: batch-compute-environment-type-validation-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-compute-environment-type-validation-errors batch-consumable-resource-pool-lifecycle batch-job-queue-and-quota-share-error-contracts batch-service-job-api-error-contracts batch-batch-job-api-error-contracts batch-job-definition-api-error-contracts
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|UpdateComputeEnvironment|4xx->4xx|$.message
Issue-Key: batch|DescribeConsumableResource|4xx->4xx|$.message
Issue-Key: batch|UpdateJobQueue|4xx->4xx|$.message
Issue-Key: batch|DescribeServiceJob|4xx->4xx|$.message
Issue-Key: batch|CreateJobQueue|4xx->4xx|$.message
Issue-Key: batch|CreateComputeEnvironment|4xx->4xx|$.message
Issue-Key: batch|ListJobs|4xx->4xx|$.message
Issue-Key: batch|SubmitJob|4xx->4xx|$.message
Issue-Key: batch|RegisterJobDefinition|4xx->4xx|$.message
Issue-Key: batch|SubmitServiceJob|4xx->4xx|$.message
Issue-Key: batch|SubmitServiceJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: batch|SubmitServiceJob|4xx->4xx|status
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# batch: resource-not-found and validation refusals report the wrong check and the wrong message envelope

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateComputeEnvironment

## Description
TEN distinct root causes in the AWS Batch emulator's refusal path, all from the simparity-wire-aws-v1 replay and none marked stubbed or owned by an open issue. Each cause is numbered; a fixer must close all ten before submitting. All ten share one fault family: when Batch refuses a call, the emulator picks the wrong check or renders the refusal in a non-AWS message envelope, so the status/error-code/detail a client branches on is wrong.

Related open issue (behavioral corpus, does NOT carry these wire replay keys): '20261022T221856Z-batch-deletecomputeenvironment-deletejobqueue-createquotashare-describequotashare-answer-501-and' (its causes 5, 6 and 8 touch ListJobs filter selection, TerminateJob idempotency and the 'Error executing request' envelope). These wire causes are filed separately because that issue's pointer does not replay any of the fixtures below.

=== Cause 1 of 10: UpdateComputeEnvironment on an absent compute environment wraps the not-found refusal instead of returning AWS's bare ARN message ===
AWS: Smithy com.amazonaws.batch#UpdateComputeEnvironment binds POST /v1/updatecomputeenvironment, success 200, and models ClientException (httpError 400) / ServerException (500). For a computeEnvironment that does not exist the recorded 400 ClientException message is the bare, request-derived form '<the compute-environment ARN resolved from the request> does not exist' — i.e. AWS echoes the full ARN of the identifier it could not resolve and nothing else.
Emulator: same 400/ClientException, but the message is 'Error executing request, Exception : Compute environment <name> does not exist, RequestId: None' — the generic envelope, the bare name instead of the resolved ARN, and the literal string 'None' where a request id belongs.
Why: the request id is interpolated from the call, so 'None' is never a correct value and support/log correlation on Batch errors is lost; clients and tooling that match AWS's '<arn> does not exist' form (or parse the ARN back out of it) mis-classify a plain not-found.
Fix: the UpdateComputeEnvironment handler's not-found path plus the service's ClientException construction/serializer — emit the bare '<arn> does not exist' detail and propagate the real request id instead of formatting Option::None.
Traces: batch-compute-environment-type-validation-errors#task#4.

=== Cause 2 of 10: DescribeConsumableResource on an absent resource wraps the not-found refusal the same way ===
AWS: Smithy com.amazonaws.batch#DescribeConsumableResource binds POST /v1/describeconsumableresource, errors ClientException (400) / ServerException (500). Recorded refusal for an unknown consumableResource: 400 ClientException with the bare message '<the consumable-resource ARN for the requested name> does not exist'.
Emulator: 400 ClientException 'Error executing request, Exception : Consumable resource <name> does not exist, RequestId: None'.
Why: same as cause 1 — wrong detail shape, name instead of ARN, and an un-propagated request id.
Fix: the DescribeConsumableResource handler's lookup-failure path (plus the shared error construction).
Traces: batch-consumable-resource-pool-lifecycle#task#10.

=== Cause 3 of 10: UpdateJobQueue on an absent queue wraps the not-found refusal the same way ===
AWS: Smithy com.amazonaws.batch#UpdateJobQueue binds POST /v1/updatejobqueue, errors ClientException (400) / ServerException (500). Recorded refusal for an unknown jobQueue: 400 ClientException with the bare message '<the job-queue ARN resolved from the request> does not exist.' (note AWS terminates this one with a period).
Emulator: 400 ClientException 'Error executing request, Exception : Job queue <name> does not exist, RequestId: None'.
Why: same family; a caller cannot tell an update of a missing queue from any other ClientException by the detail AWS documents.
Fix: the UpdateJobQueue handler's not-found path.
Traces: batch-job-queue-and-quota-share-error-contracts#task#4.

=== Cause 4 of 10: DescribeServiceJob on an absent job id wraps the not-found refusal the same way ===
AWS: Smithy com.amazonaws.batch#DescribeServiceJob binds POST /v1/describeservicejob, errors ClientException (400) / ServerException (500). Recorded refusal for an unknown jobId: 400 ClientException with the bare message '<the service-job ARN built from the requested jobId> does not exist'.
Emulator: 400 ClientException 'Error executing request, Exception : Service job <jobId> does not exist, RequestId: None'.
Why: same family.
Fix: the DescribeServiceJob handler's not-found path.
Traces: batch-service-job-api-error-contracts#task#2.

=== Cause 5 of 10: CreateJobQueue with an unusable serviceEnvironmentOrder reports a resource-existence error instead of AWS's service-environment precondition ===
AWS: Smithy com.amazonaws.batch#CreateJobQueue takes serviceEnvironmentOrder (ServiceEnvironmentOrders) and models ClientException (400). Recorded behavior: a CreateJobQueue whose serviceEnvironmentOrder names a service environment that is not created-and-valid is refused with 400 ClientException and the fixed precondition message 'Service environments must be created and valid before attaching them to a job queue.' — AWS states the precondition, it does not name the resource.
Emulator: 400 ClientException 'Error executing request, Exception : Service environment <name> does not exist, RequestId: None' — a per-resource existence error in the generic envelope.
Why: the two refusals are different contracts: AWS's single precondition message covers both 'absent' and 'present but not VALID', so a client that keys on it (or retries until the environment becomes VALID) never matches the emulator's text, and the state dimension (validity) disappears from the refusal entirely.
Fix: the CreateJobQueue handler's serviceEnvironmentOrder validation — check created-AND-valid and raise the precondition refusal.
Traces: batch-job-queue-and-quota-share-error-contracts#task#1.

=== Cause 6 of 10: the MANAGED compute-environment precondition is reworded and still carries 'RequestId: None' ===
AWS: Smithy com.amazonaws.batch#CreateComputeEnvironment has required type (CEType MANAGED|UNMANAGED) and optional computeResources (ComputeResource), errors ClientException (400). Recorded refusal when type=MANAGED is submitted with no computeResources: 400 ClientException, envelope form 'Error executing request, Exception : computeResources must be provided for a MANAGED compute environment, RequestId: <the call's own request id>'.
Emulator: right status and code, but the detail is reworded ('computeResources is required for a MANAGED compute environment') and the envelope's request id is the literal 'None'.
Why: this is the one Batch refusal where AWS really does use the envelope, so the request-id interpolation is a knowable rule and is broken — every Batch error message is uncorrelatable. The reworded detail is secondary (the recorded wording is the example), but the 'None' request id is wrong in every environment.
Fix: the service's ClientException construction/serializer must interpolate the active request id into the envelope; the detail string belongs in the CreateComputeEnvironment validation.
Traces: batch-compute-environment-type-validation-errors#task#1.

=== Cause 7 of 10: ListJobs with no job selector reports the mutual-exclusion error instead of the missing-jobQueue error ===
AWS: Smithy com.amazonaws.batch#ListJobs has all of jobQueue / arrayJobId / multiNodeJobId optional, output requires jobSummaryList, errors ClientException (400). Recorded behavior for a ListJobs that supplies NONE of the three selectors: 400 ClientException 'Error executing request, Exception : Job queue name is required., RequestId: <request id>' — AWS reports the missing-selector condition.
Emulator: 400 ClientException reporting the opposite condition — 'You must specify only one of jobQueue, arrayJobId or multiNodeJobId' — i.e. it claims too many selectors were given when none were.
Why: the emulator evaluates the wrong branch of ListJobs selector validation, so the refusal tells a client to remove a selector when the fix is to add one; two distinct, separately-documented validation outcomes are collapsed into one.
Fix: the ListJobs handler's selector validation — zero selectors must raise the 'Job queue name is required.' refusal, and the mutual-exclusion refusal must fire only when more than one selector is present.
Traces: batch-batch-job-api-error-contracts#task#1.

=== Cause 8 of 10: SubmitJob validates the job queue before the job definition, so the wrong missing resource is reported ===
AWS: Smithy com.amazonaws.batch#SubmitJob requires jobName, jobQueue and jobDefinition, errors ClientException (400). Recorded behavior for a SubmitJob where BOTH the jobQueue and the jobDefinition are absent: AWS refuses on the JOB DEFINITION first — 400 ClientException 'JobDefinition name <jobDefinition> does not exist or is not in ACTIVE status.' (bare message, and it folds the non-ACTIVE case into the same refusal).
Emulator: 400 ClientException 'Error executing request, Exception : Job queue <jobQueue> does not exist, RequestId: None' — it resolves the queue first and never reaches the job-definition check.
Why: validation precedence is observable: a client submitting against a stale job definition is told its queue is broken, so remediation targets the wrong resource; the ACTIVE-status dimension of AWS's refusal is absent altogether.
Fix: the SubmitJob handler's input resolution order — resolve/validate jobDefinition (existence AND ACTIVE revision) before the jobQueue lookup, and emit the combined 'does not exist or is not in ACTIVE status.' refusal.
Traces: batch-batch-job-api-error-contracts#task#4.

=== Cause 9 of 10: RegisterJobDefinition runs its properties mutual-exclusion check before the node-range container requirement ===
AWS: Smithy com.amazonaws.batch#RegisterJobDefinition accepts containerProperties, nodeProperties, ecsProperties and eksProperties and models ClientException (400). Recorded behavior for a multi-node definition whose nodeRangeProperties entry carries no container: 400 ClientException 'Error executing request, Exception : Must specify container for node range 0:1., RequestId: <request id>' — AWS reports the per-node-range container requirement, naming the offending range.
Emulator: 400 ClientException reporting a different rule entirely — 'only one of containerProperties, nodeProperties, ecsProperties and eksProperties can be specified, but containerProperties and nodeProperties were'.
Why: the emulator's mutual-exclusion gate pre-empts the node-range validation, so a client never learns which node range is malformed and cannot fix a multi-node definition from the refusal it gets.
Fix: the RegisterJobDefinition validation order — validate nodeProperties.nodeRangeProperties (each range must carry a container spec, message naming the targetNodes range) before/alongside the properties mutual-exclusion check.
Traces: batch-job-definition-api-error-contracts#task#1 (cluster also covers task#2 of the same fixture).

=== Cause 10 of 10: SubmitServiceJob answers 400 ClientException where AWS refuses with 401 UnauthorizedException ===
AWS: Smithy com.amazonaws.batch#SubmitServiceJob binds POST /v1/submitservicejob and declares only ClientException (400) / ServerException (500) — the model does NOT declare the recorded error, so this is recorded behavior only: the fixture's SubmitServiceJob is refused with HTTP 401, header x-amzn-errortype 'UnauthorizedException' and body message 'Unauthorized'. Hypothesis: the service-jobs surface is authorized before the jobQueue is resolved, so an unauthorized caller is rejected at that layer and the referenced queue is never looked up.
Emulator: HTTP 400 with x-amzn-errortype 'ClientException' and message 'JobQueue <name> not found.' — it skips the authorization refusal and answers from the resource lookup instead.
Why: the status class differs (401 vs 400) and the error type differs ('UnauthorizedException' vs 'ClientException'), so SDK error-shape dispatch and any retry/credential-refresh branch keyed on an authorization failure never triggers; a caller is told to create a queue when the call is being refused for authorization.
Fix: the SubmitServiceJob handler/auth gate — emit the 401 UnauthorizedException refusal (status, x-amzn-errortype header and body code) ahead of job-queue resolution.
Traces: batch-service-job-api-error-contracts#task#1.

## Evidence
simparity-wire-aws-v1 replay against the emulator built from this checkout: 96 steps, 20 passed, 76 mismatched, score 20.8%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0. All ten clusters below are verdict=mismatch with empty stub_path and empty owned_by.

Cause 1 — batch-compute-environment-type-validation-errors#task#4, UpdateComputeEnvironment, 400 -> 400: $.message value_mismatch, expected 'arn:aws:batch:<region>:<account>:compute-environment/<name> does not exist', got 'Error executing request, Exception : Compute environment <name> does not exist, RequestId: None'.
Cause 2 — batch-consumable-resource-pool-lifecycle#task#10, DescribeConsumableResource, 400 -> 400: expected 'arn:aws:batch:<region>:<account>:consumable-resource/<name> does not exist', got 'Error executing request, Exception : Consumable resource <name> does not exist, RequestId: None'.
Cause 3 — batch-job-queue-and-quota-share-error-contracts#task#4, UpdateJobQueue, 400 -> 400: expected 'arn:aws:batch:<region>:<account>:job-queue/<name> does not exist.', got 'Error executing request, Exception : Job queue <name> does not exist, RequestId: None'.
Cause 4 — batch-service-job-api-error-contracts#task#2, DescribeServiceJob, 400 -> 400: expected 'arn:aws:batch:<region>:<account>:service-job/<jobId> does not exist', got 'Error executing request, Exception : Service job <jobId> does not exist, RequestId: None'.
Cause 5 — batch-job-queue-and-quota-share-error-contracts#task#1 (cluster also covers task#2), CreateJobQueue, 400 -> 400: expected 'Service environments must be created and valid before attaching them to a job queue.', got 'Error executing request, Exception : Service environment <name> does not exist, RequestId: None'.
Cause 6 — batch-compute-environment-type-validation-errors#task#1, CreateComputeEnvironment, 400 -> 400: expected '... computeResources must be provided for a MANAGED compute environment, RequestId: <id>', got '... computeResources is required for a MANAGED compute environment, RequestId: None'.
Cause 7 — batch-batch-job-api-error-contracts#task#1, ListJobs, 400 -> 400: expected '... Job queue name is required., RequestId: <id>', got '... You must specify only one of jobQueue, arrayJobId or multiNodeJobId, RequestId: None'.
Cause 8 — batch-batch-job-api-error-contracts#task#4, SubmitJob, 400 -> 400: expected 'JobDefinition name <jobdef> does not exist or is not in ACTIVE status.', got 'Error executing request, Exception : Job queue <jq> does not exist, RequestId: None'.
Cause 9 — batch-job-definition-api-error-contracts#task#1 (+task#2), RegisterJobDefinition, 400 -> 400: expected '... Must specify container for node range 0:1., RequestId: <id>', got '... only one of containerProperties, nodeProperties, ecsProperties and eksProperties can be specified, but containerProperties and nodeProperties were, RequestId: None'.
Cause 10 — batch-service-job-api-error-contracts#task#1, SubmitServiceJob: status expected 401, got 400; $headers.x-amzn-errortype expected 'UnauthorizedException', got 'ClientException'; $.message expected 'Unauthorized', got 'JobQueue <name> not found.'.

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json', query='operation', operation='UpdateComputeEnvironment,DescribeConsumableResource,UpdateJobQueue,DescribeServiceJob,CreateJobQueue,CreateComputeEnvironment,ListJobs,SubmitJob,RegisterJobDefinition,SubmitServiceJob') — every one binds POST /v1/<op>, success 200, and declares exactly ClientException(400)/ServerException(500); ListJobs has jobQueue/arrayJobId/multiNodeJobId all optional and requires jobSummaryList; SubmitJob requires jobName+jobQueue+jobDefinition; CreateComputeEnvironment requires type (CEType) with computeResources optional; CreateJobQueue carries serviceEnvironmentOrder. smithy_query(query='shape', shape='ClientException') — error structure with a single `message` member, httpError 400; no UnauthorizedException is bound to this service.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (corpus simparity-wire-aws-v1) and read the batch clusters.
2. Cause 1: aws batch update-compute-environment --compute-environment <absent-name> --state ENABLED -> 400 ClientException, inspect Error.Message: wrapped envelope with 'RequestId: None' instead of the bare '<arn> does not exist' (fixture batch-compute-environment-type-validation-errors, step task#4).
3. Cause 2: aws batch describe-consumable-resource --consumable-resource <absent-name> -> same wrapped message (batch-consumable-resource-pool-lifecycle#task#10).
4. Cause 3: aws batch update-job-queue --job-queue <absent-name> --state ENABLED -> same (batch-job-queue-and-quota-share-error-contracts#task#4).
5. Cause 4: aws batch describe-service-job --job-id 00000000-0000-0000-0000-000000000000 -> same (batch-service-job-api-error-contracts#task#2).
6. Cause 5: aws batch create-job-queue --job-queue-name q --priority 1 --service-environment-order serviceEnvironment=<absent>,order=1 --job-queue-type SAGEMAKER_TRAINING -> expect the 'Service environments must be created and valid ...' precondition message, observe a per-resource not-found (batch-job-queue-and-quota-share-error-contracts#task#1).
7. Cause 6: aws batch create-compute-environment --compute-environment-name ce --type MANAGED (no --compute-resources) -> 400, message detail reworded and ends 'RequestId: None' (batch-compute-environment-type-validation-errors#task#1).
8. Cause 7: aws batch list-jobs (no --job-queue / --array-job-id / --multi-node-job-id) -> observe the mutual-exclusion message instead of 'Job queue name is required.' (batch-batch-job-api-error-contracts#task#1).
9. Cause 8: aws batch submit-job --job-name n --job-queue <absent-jq> --job-definition <absent-jobdef> -> observe the job-queue not-found instead of the JobDefinition ACTIVE-status refusal (batch-batch-job-api-error-contracts#task#4).
10. Cause 9: aws batch register-job-definition --job-definition-name d --type multinode with nodeProperties whose nodeRangeProperties 0:1 entry has no container (and containerProperties also set) -> observe the mutual-exclusion message instead of 'Must specify container for node range 0:1.' (batch-job-definition-api-error-contracts#task#1).
11. Cause 10: aws batch submit-service-job --job-name n --job-queue <absent> --service-job-type SAGEMAKER_TRAINING --service-request-payload '{}' -> observe HTTP 400/ClientException instead of HTTP 401/UnauthorizedException 'Unauthorized' (batch-service-job-api-error-contracts#task#1).
