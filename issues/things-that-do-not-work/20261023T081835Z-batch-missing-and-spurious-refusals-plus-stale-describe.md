Issue-Source: trace-sweep-divergence
Issue-Trace: batch-batch-job-api-error-contracts
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-batch-job-api-error-contracts batch-compute-environment-type-validation-errors batch-consumable-resource-pool-lifecycle batch-create-describe-fairshare-policy batch-scheduling-policy-standalone-lifecycle batch-job-definition-api-error-contracts batch-duplicate-create-scheduling-policy-error batch-scheduling-policy-list-coverage batch-describe-unknown-arn-returns-empty batch-list-and-describe-multiple-policies batch-service-environment-standalone-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeJobs|4xx->2xx|$.__type
Issue-Key: batch|DescribeJobs|4xx->2xx|$.message
Issue-Key: batch|DescribeJobs|4xx->2xx|status
Issue-Key: batch|CreateComputeEnvironment|4xx->2xx|$.__type
Issue-Key: batch|CreateComputeEnvironment|4xx->2xx|$.message
Issue-Key: batch|CreateComputeEnvironment|4xx->2xx|status
Issue-Key: batch|DescribeConsumableResource|4xx->2xx|$.__type
Issue-Key: batch|DescribeConsumableResource|4xx->2xx|$.message
Issue-Key: batch|DescribeConsumableResource|4xx->2xx|status
Issue-Key: batch|DescribeSchedulingPolicies|2xx->2xx|$.schedulingPolicies[*]
Issue-Key: batch|DescribeSchedulingPolicies|4xx->2xx|$.__type
Issue-Key: batch|DescribeSchedulingPolicies|4xx->2xx|$.message
Issue-Key: batch|DescribeSchedulingPolicies|4xx->2xx|status
Issue-Key: batch|TerminateJob|2xx->4xx|$
Issue-Key: batch|TerminateJob|2xx->4xx|status
Issue-Key: batch|DeregisterJobDefinition|2xx->4xx|$
Issue-Key: batch|DeregisterJobDefinition|2xx->4xx|status
Issue-Key: batch|CreateSchedulingPolicy|4xx->4xx|$.message
Issue-Key: batch|CreateSchedulingPolicy|4xx->4xx|status
Issue-Key: batch|DescribeSchedulingPolicies|2xx->2xx|$.schedulingPolicies[arn="arn
Issue-Key: batch|DescribeServiceEnvironments|2xx->2xx|$.serviceEnvironments[*]
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# batch: missing and spurious refusals plus stale describe state across the control plane

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchedulingPolicies

## Description
TEN distinct root causes in the AWS Batch emulator, all from the simparity-wire-aws-v1 replay and none marked stubbed or owned by an open issue. Each cause is numbered; a fixer must close all ten before submitting. The family: calls that AWS refuses are accepted, calls AWS accepts are refused, deletes do not reach the state behind Describe, and Describe responses omit members AWS always materializes.

Related open issues (behavioral corpus; they do NOT carry these wire replay keys): '20261022T221856Z-batch-deletecomputeenvironment-deletejobqueue-createquotashare-describequotashare-answer-501-and' (its cause 6 concerns TerminateJob idempotency) and '20261022T221856Z-batch-submitted-jobs-are-driven-straight-to-failed'. The causes below are filed separately because neither issue's pointer replays any of the fixtures named here.

=== Cause 1 of 10: DescribeJobs accepts an over-limit jobs list instead of refusing it ===
AWS: Smithy com.amazonaws.batch#DescribeJobs binds POST /v1/describejobs with required jobs:StringList and models ClientException (400) / ServerException (500); the list itself carries no @length in the model, so the limit is recorded behavior only: a DescribeJobs with more than 100 job ids is refused with HTTP 400, __type 'ClientException', message 'Error executing request, Exception : Maximum number of jobs supported is  100 ., RequestId: <request id>'.
Emulator: answers HTTP 200 with a normal DescribeJobs body and no error at all.
Why: the request quota is unenforced, so a client that would be throttled/refused by AWS at >100 ids succeeds in the emulator and ships code that fails in production; no 400/ClientException branch is ever exercised.
Fix: the DescribeJobs handler's input validation — cap the jobs list at 100 entries and raise ClientException.
Traces: batch-batch-job-api-error-contracts#task#2.

=== Cause 2 of 10: CreateComputeEnvironment accepts an UNMANAGED Fargate compute environment that AWS rejects ===
AWS: Smithy com.amazonaws.batch#CreateComputeEnvironment requires type (CEType MANAGED|UNMANAGED) and takes computeResources (ComputeResource, whose type may be FARGATE/FARGATE_SPOT); errors ClientException (400). Recorded behavior: an UNMANAGED compute environment combined with Fargate compute resources is refused with HTTP 400, __type 'ClientException', message 'Error executing request, Exception : Cannot create an UNMANAGED Fargate Compute Environment., RequestId: <request id>'.
Emulator: HTTP 200 — the compute environment is created.
Why: an invalid combination becomes a live resource, so downstream job queues/jobs are built on a configuration AWS cannot produce, and the modeled refusal a client handles never arrives.
Fix: the CreateComputeEnvironment handler's type/computeResources cross-validation.
Traces: batch-compute-environment-type-validation-errors#task#2.

=== Cause 3 of 10: DeleteConsumableResource does not remove the resource, so Describe still answers 200 after the delete ===
AWS: Smithy com.amazonaws.batch#DeleteConsumableResource binds POST /v1/deleteconsumableresource and returns an empty 200; afterwards DescribeConsumableResource for that name is refused with HTTP 400 __type 'ClientException', message '<the consumable-resource ARN> does not exist'.
Emulator: after a successful DeleteConsumableResource, DescribeConsumableResource still answers HTTP 200 with the resource body.
Why: deletion is not durable in service state, so teardown appears to work while the resource lingers; name reuse, 'is it gone yet' polling and idempotent cleanup all behave differently from AWS.
Fix: the DeleteConsumableResource handler must remove the entry from the service state (and DescribeConsumableResource must then take its not-found path).
Traces: batch-consumable-resource-pool-lifecycle#teardown#13.

=== Cause 4 of 10: DeleteSchedulingPolicy does not remove the policy, so DescribeSchedulingPolicies still returns it ===
AWS: Smithy com.amazonaws.batch#DeleteSchedulingPolicy binds POST /v1/deleteschedulingpolicy, empty 200; afterwards DescribeSchedulingPolicies for that ARN returns 200 with schedulingPolicies absent/empty for the deleted ARN.
Emulator: DescribeSchedulingPolicies after the delete returns 200 with the full deleted policy still present in schedulingPolicies[0] (name, arn, fairsharePolicy, tags).
Why: same state-durability defect as cause 3 but on the scheduling-policy store; a deleted policy stays attachable and listable, so a client cannot verify teardown and may reuse a name AWS has freed.
Fix: the DeleteSchedulingPolicy handler must remove the policy from state so Describe/List no longer see it.
Traces: batch-create-describe-fairshare-policy#teardown#4 (cluster also covers batch-scheduling-policy-standalone-lifecycle#teardown#13).

=== Cause 5 of 10: DescribeSchedulingPolicies answers 200 where AWS refuses the request with ClientException ===
AWS: Smithy com.amazonaws.batch#DescribeSchedulingPolicies binds POST /v1/describeschedulingpolicies with required arns:StringList, errors ClientException (400). Recorded behavior at this step: HTTP 400 with __type 'ClientException' (the fixture does not assert the message text, only the status and the error type).
Emulator: HTTP 200 with no error members at all.
Why: a whole refusal class of DescribeSchedulingPolicies (an arns input AWS will not accept) is silently successful, so a client's error path is dead code and a bad request looks like an empty result.
Fix: the DescribeSchedulingPolicies handler's arns validation must raise ClientException (400) for the input AWS refuses instead of returning an empty 200.
Traces: batch-scheduling-policy-standalone-lifecycle#task#9.

=== Cause 6 of 10: TerminateJob on an unknown jobId is refused instead of being accepted as a no-op ===
AWS: Smithy com.amazonaws.batch#TerminateJob binds POST /v1/terminatejob with required jobId+reason and an EMPTY output structure; recorded behavior for a jobId that does not exist is HTTP 200 with body {} — the call is idempotently accepted.
Emulator: HTTP 400, __type 'ClientException', message 'Error executing request, Exception : Job 00000000-0000-0000-0000-000000000000 does not exist, RequestId: None'.
Why: blind-terminate and retry/cleanup paths that AWS accepts now fail, and a client cannot distinguish 'already gone' from a real error. (The behavioral issue named above reaches the same conclusion from a different corpus; this wire step is unreplayed by it.)
Fix: the TerminateJob handler — return the empty 200 output for an unresolvable jobId instead of raising ClientException.
Traces: batch-batch-job-api-error-contracts#task#5.

=== Cause 7 of 10: DeregisterJobDefinition on an absent definition is refused instead of being accepted as a no-op ===
AWS: Smithy com.amazonaws.batch#DeregisterJobDefinition binds POST /v1/deregisterjobdefinition with required jobDefinition and an EMPTY output; recorded behavior for a '<name>:<revision>' that does not exist is HTTP 200 with body {}.
Emulator: HTTP 400, __type 'ClientException', message 'Error executing request, Exception : Job definition <name>:1 does not exist, RequestId: None'.
Why: idempotent deregistration (the normal teardown pattern, and the retry behaviour of any at-least-once pipeline) fails against the emulator while succeeding on AWS.
Fix: the DeregisterJobDefinition handler — unresolvable jobDefinition must yield the empty 200.
Traces: batch-job-definition-api-error-contracts#task#5.

=== Cause 8 of 10: duplicate CreateSchedulingPolicy returns 400 with a wrapped message instead of 409 'Object already exists' ===
AWS: Smithy com.amazonaws.batch#CreateSchedulingPolicy binds POST /v1/createschedulingpolicy and declares ClientException with httpError 400, but the recorded conflict response is HTTP 409 carrying __type 'ClientException' and the bare message 'Object already exists' — i.e. for name conflicts the wire status is 409, not the model's default 400.
Emulator: HTTP 400 with message 'Error executing request, Exception : Object already exists: scheduling policy <name>, RequestId: None'.
Why: the status class is wrong (409 vs 400), so clients and SDK middlewares that treat a conflict distinctly from a validation failure (create-if-absent flows, conflict retries) mis-route; the documented bare conflict message is also buried in the generic envelope.
Fix: the CreateSchedulingPolicy handler's duplicate-name path — emit HTTP 409 with ClientException and the bare 'Object already exists' detail.
Traces: batch-duplicate-create-scheduling-policy-error#task#2 (cluster also covers batch-scheduling-policy-list-coverage#task#4).

=== Cause 9 of 10: DescribeSchedulingPolicies omits the default fairsharePolicy and tags that AWS always materializes ===
AWS: Smithy SchedulingPolicyDetail carries name, arn, quotaSharePolicy, fairsharePolicy (FairsharePolicy: shareDecaySeconds, computeReservation, shareDistribution) and tags (TagrisTagsMap). Recorded behavior for a policy created without a fairsharePolicy: DescribeSchedulingPolicies returns both members PRESENT and zero-valued — fairsharePolicy = {shareDecaySeconds: 0, computeReservation: 0, shareDistribution: []} and tags = {}.
Emulator: both members are absent from the serialized detail (null), for every policy created without them.
Why: typed SDK clients read policy.fairsharePolicy().shareDecaySeconds() and policy.tags() without a null guard because AWS always sends them; against the emulator those are null/absent and callers NPE or mis-render, and code that round-trips a described policy back into an update loses the defaults.
Fix: the DescribeSchedulingPolicies serializer / scheduling-policy state defaults — materialize the zero-valued fairsharePolicy and an empty tags map.
Traces: batch-describe-unknown-arn-returns-empty#task#3 (cluster also covers batch-duplicate-create-scheduling-policy-error#task#3, batch-list-and-describe-multiple-policies#task#4, batch-scheduling-policy-standalone-lifecycle#task#2 and #task#4).

=== Cause 10 of 10: a created service environment never leaves CREATING, and DescribeServiceEnvironments omits tags ===
AWS: Smithy ServiceEnvironmentDetail carries status:ServiceEnvironmentStatus (enum CREATING|UPDATING|DELETING|DELETED|VALID|INVALID) and tags:TagrisTagsMap. Recorded behavior after CreateServiceEnvironment: DescribeServiceEnvironments reports status 'VALID' (the environment settles out of CREATING) and includes tags = {} when none were set.
Emulator: the same environment is reported with status 'CREATING' at every later describe, and the tags member is omitted entirely.
Why: 'VALID' is the precondition for attaching a service environment to a job queue, so a client that waits for VALID (the documented pattern) hangs forever and the SAGEMAKER_TRAINING job-queue path is unreachable; the absent tags member breaks typed readers as in cause 9.
Fix: the service-environment state transition (CREATING -> VALID on create completion) plus the DescribeServiceEnvironments serializer's tags default.
Traces: batch-service-environment-standalone-lifecycle#task#2 (cluster also covers task#4 and task#12 of the same fixture).

## Evidence
simparity-wire-aws-v1 replay against the emulator built from this checkout: 96 steps, 20 passed, 76 mismatched, score 20.8%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0 (so none of these clusters is contaminated by a leaked previous fixture's state). All ten clusters are verdict=mismatch with empty stub_path and empty owned_by.

Cause 1 — batch-batch-job-api-error-contracts#task#2, DescribeJobs: status expected 400, got 200; $.message missing (expected 'Error executing request, Exception : Maximum number of jobs supported is  100 ., RequestId: <id>'); $.__type missing (expected 'ClientException').
Cause 2 — batch-compute-environment-type-validation-errors#task#2, CreateComputeEnvironment: status expected 400, got 200; $.message missing (expected '... Cannot create an UNMANAGED Fargate Compute Environment., RequestId: <id>'); $.__type missing (expected 'ClientException').
Cause 3 — batch-consumable-resource-pool-lifecycle#teardown#13, DescribeConsumableResource: status expected 400, got 200; expected message '<consumable-resource arn> does not exist' and __type 'ClientException', got neither.
Cause 4 — batch-create-describe-fairshare-policy#teardown#4 (+batch-scheduling-policy-standalone-lifecycle#teardown#13), DescribeSchedulingPolicies, 200 -> 200: $.schedulingPolicies[0] extra — expected None, got the deleted policy {name, arn, fairsharePolicy{shareDecaySeconds 600, computeReservation 25, shareDistribution[teamA 1.0, teamB 2.0]}, tags{Environment: probe}}.
Cause 5 — batch-scheduling-policy-standalone-lifecycle#task#9, DescribeSchedulingPolicies: status expected 400, got 200; $.__type missing (expected 'ClientException'); $.message expectation recorded as '<shaped-not-asserted>' (status + error type are the assertion).
Cause 6 — batch-batch-job-api-error-contracts#task#5, TerminateJob: status expected 200, got 400; body expected {}, got {'__type': 'ClientException', 'message': 'Error executing request, Exception : Job 00000000-0000-0000-0000-000000000000 does not exist, RequestId: None'}.
Cause 7 — batch-job-definition-api-error-contracts#task#5, DeregisterJobDefinition: status expected 200, got 400; body expected {}, got {'__type': 'ClientException', 'message': 'Error executing request, Exception : Job definition <name>:1 does not exist, RequestId: None'}.
Cause 8 — batch-duplicate-create-scheduling-policy-error#task#2 (+batch-scheduling-policy-list-coverage#task#4), CreateSchedulingPolicy: status expected 409, got 400; $.message expected 'Object already exists', got 'Error executing request, Exception : Object already exists: scheduling policy <name>, RequestId: None'.
Cause 9 — batch-describe-unknown-arn-returns-empty#task#3 (+3 more fixtures, 5 steps total), DescribeSchedulingPolicies, 200 -> 200: $.schedulingPolicies[arn=...].fairsharePolicy missing (expected {'shareDecaySeconds': 0, 'computeReservation': 0, 'shareDistribution': []}, got None); $.schedulingPolicies[arn=...].tags missing (expected {}, got None).
Cause 10 — batch-service-environment-standalone-lifecycle#task#2 (+task#4, task#12), DescribeServiceEnvironments, 200 -> 200: serviceEnvironments[0] expected status 'VALID' and tags {}, got status 'CREATING' and no tags member (all other members matched).

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json') — operation query for DescribeJobs, CreateComputeEnvironment, DeleteConsumableResource, DescribeConsumableResource, DeleteSchedulingPolicy, DescribeSchedulingPolicies, TerminateJob, DeregisterJobDefinition, CreateSchedulingPolicy, DescribeServiceEnvironments: all POST /v1/<op>, success 200, errors exactly ClientException(400)/ServerException(500); TerminateJob and DeregisterJobDefinition have EMPTY output structures; DescribeJobs.jobs is a required StringList with no @length. shape query for SchedulingPolicyDetail (fairsharePolicy + tags members), FairsharePolicy (shareDecaySeconds/computeReservation/shareDistribution), ServiceEnvironmentDetail (status + tags) and ServiceEnvironmentStatus (CREATING|UPDATING|DELETING|DELETED|VALID|INVALID).

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (corpus simparity-wire-aws-v1) and read the batch clusters.
2. Cause 1: aws batch describe-jobs --jobs <101 job ids> -> HTTP 200 instead of 400 ClientException 'Maximum number of jobs supported is  100 .' (batch-batch-job-api-error-contracts#task#2).
3. Cause 2: aws batch create-compute-environment --compute-environment-name ce --type UNMANAGED --compute-resources type=FARGATE,maxvCpus=4,subnets=...,securityGroupIds=... -> HTTP 200 instead of 400 ClientException (batch-compute-environment-type-validation-errors#task#2).
4. Cause 3: create a consumable resource, aws batch delete-consumable-resource --consumable-resource <name>, then aws batch describe-consumable-resource --consumable-resource <name> -> 200 with the resource still present instead of 400 ClientException (batch-consumable-resource-pool-lifecycle#teardown#13).
5. Cause 4: create a scheduling policy, aws batch delete-scheduling-policy --arn <arn>, then aws batch describe-scheduling-policies --arns <arn> -> the deleted policy is still returned (batch-create-describe-fairshare-policy#teardown#4).
6. Cause 5: replay batch-scheduling-policy-standalone-lifecycle step task#9 (DescribeSchedulingPolicies) -> 200 instead of 400 with __type ClientException.
7. Cause 6: aws batch terminate-job --job-id 00000000-0000-0000-0000-000000000000 --reason x -> 400 ClientException instead of 200 {} (batch-batch-job-api-error-contracts#task#5).
8. Cause 7: aws batch deregister-job-definition --job-definition <absent-name>:1 -> 400 ClientException instead of 200 {} (batch-job-definition-api-error-contracts#task#5).
9. Cause 8: aws batch create-scheduling-policy --name p twice -> second call 400 with the wrapped message instead of 409 'Object already exists' (batch-duplicate-create-scheduling-policy-error#task#2).
10. Cause 9: aws batch create-scheduling-policy --name p (no fairshare policy, no tags), then aws batch describe-scheduling-policies --arns <arn> -> fairsharePolicy and tags absent instead of {shareDecaySeconds:0, computeReservation:0, shareDistribution:[]} and {} (batch-describe-unknown-arn-returns-empty#task#3).
11. Cause 10: aws batch create-service-environment --service-environment-name se --service-environment-type SAGEMAKER_TRAINING --capacity-limits maxCapacity=4,capacityUnit=NUM_INSTANCES, then aws batch describe-service-environments --service-environments se -> status 'CREATING' forever and no tags member, instead of 'VALID' with tags {} (batch-service-environment-standalone-lifecycle#task#2).
