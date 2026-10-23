Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.op-contract-deleteComputeEnvironment batch.ce-delete-while-enabled-fails batch.op-contract-deleteJobQueue batch.delete-enabled-job-queue-fails batch.create-quota-share-unknown-scheduling-policy-fails batch.describe-quota-share-unknown-fails batch.job-list-unknown-queue-returns-empty batch.list-jobs-invalid-filter-combination-fails batch.terminate-unknown-job-is-accepted batch.terminate-service-job-unknown-id-silently-accepted batch.job-queue-tag-limit-exceeded-rejected batch.create-job-queue-unknown-compute-environment-fails batch.duplicate-compute-environment-name-fails batch.op-contract-describeJobDefinitions batch.register-job-definition-contract batch.register-job-definition-missing-container-properties-fails
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.op-contract-deleteComputeEnvironment|run#4|AssertionError|assert code == "ClientException", f"expected ClientException, got {code!r}"
Issue-Key: batch.ce-delete-while-enabled-fails|run#6|AssertionError|assert code == "ClientException", f"expected ClientException deleting an ENABLED CE, got {code!r}"
Issue-Key: batch.op-contract-deleteJobQueue|run#5|AssertionError|assert code == "ClientException", f"expected ClientException, got {code!r}"
Issue-Key: batch.delete-enabled-job-queue-fails|run#7|AssertionError|assert code == "ClientException", f"expected ClientException deleting an ENABLED queue, got {code!r}"
Issue-Key: batch.create-quota-share-unknown-scheduling-policy-fails|run#5|AssertionError|assert code == "ClientException", f"expected ClientException, got {code!r}"
Issue-Key: batch.describe-quota-share-unknown-fails|run#5|AssertionError|assert control_code == "ClientException", (
Issue-Key: batch|ListJobs|ClientException
Issue-Key: batch.terminate-unknown-job-is-accepted|run#4|AssertionError|assert code is None, (
Issue-Key: batch|TerminateJob|ClientException
Issue-Key: batch.job-queue-tag-limit-exceeded-rejected|run#14|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for 51 tags, got {code!r}"
Issue-Key: batch.create-job-queue-unknown-compute-environment-fails|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.duplicate-compute-environment-name-fails|run#7|AssertionError|assert message == "Object already exists", f"expected message 'Object already exists', got {message!r}"
Issue-Key: batch.op-contract-describeJobDefinitions|run#21|AssertionError|assert message == "Error processing next token", f"unexpected message {message!r}"
Issue-Key: batch.register-job-definition-contract|run#26|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: batch.register-job-definition-missing-container-properties-fails|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# batch: DeleteComputeEnvironment/DeleteJobQueue/CreateQuotaShare/DescribeQuotaShare answer 501, and ListJobs/TerminateJob/TagResource error semantics diverge (8 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteJobQueue

## Description
Eight distinct root causes in the AWS Batch emulator's control plane, all grounded in failing behavioral checks that are NOT marked ALREADY FILED/OWNED by the replay report. Numbered; a fixer must close all eight.

=== Cause 1 of 8: DeleteComputeEnvironment is an unimplemented scaffold stub ===
AWS: Smithy (com.amazonaws.batch#DeleteComputeEnvironment) binds POST /v1/deletecomputeenvironment, success 200 with an empty output, and models ClientException (HTTP 400) / ServerException (HTTP 500). Deleting a compute environment that is still ENABLED is refused with ClientException per the recorded check (AWS documents that the CE must first be set to DISABLED with UpdateComputeEnvironment).
Emulator: every call answers HTTP 501 with error code 'NotImplemented' ('operation DeleteComputeEnvironment is scaffolded but not implemented in this emulator'). services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/delete_compute_environment.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns Scaffolded::not_implemented at line 156.
Why: a client can never tear down a compute environment, and the modeled ClientException refusal for an ENABLED CE is replaced by a 501 that no SDK retry/error path models — teardown of any Batch resource graph is impossible and the error class a caller branches on is wrong.
Fix: implement the handler in that op file (state lookup + DISABLED precondition + removal from the service state).
Traces: batch.op-contract-deleteComputeEnvironment, batch.ce-delete-while-enabled-fails (both assert code=='ClientException', got 'NotImplemented').

=== Cause 2 of 8: DeleteJobQueue is an unimplemented scaffold stub ===
AWS: Smithy (com.amazonaws.batch#DeleteJobQueue) binds POST /v1/deletejobqueue, success 200 empty output, errors ClientException (400) / ServerException (500). Deleting a queue whose state is still ENABLED is refused with ClientException (the model's own docs: submissions must first be disabled with UpdateJobQueue).
Emulator: HTTP 501 'NotImplemented'. services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/delete_job_queue.rs still carries the SCAFFOLDED marker and returns Scaffolded::not_implemented at line 154.
Why: queues cannot be deleted at all, so every suite/stack that provisions a queue leaks it, and the ENABLED-queue refusal a client depends on never happens.
Fix: implement the handler in that op file.
Traces: batch.op-contract-deleteJobQueue, batch.delete-enabled-job-queue-fails (assert code=='ClientException', got 'NotImplemented').

=== Cause 3 of 8: CreateQuotaShare is an unimplemented scaffold stub ===
AWS: Smithy (com.amazonaws.batch#CreateQuotaShare) binds POST /v1/createquotashare, success 200 returning quotaShareName/quotaShareArn, errors ClientException (400) / ServerException (500). CreateQuotaShare naming a job queue that does not exist is refused with ClientException; the recorded message is of the form 'Error executing request, Exception : <queue> does not exist., RequestId: <the call's request id>'.
Emulator: HTTP 501 'NotImplemented' for every CreateQuotaShare call, so neither the rejection nor the positive-control creation happens.
Fix: the CreateQuotaShare handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/create_quota_share.rs) plus quota-share state.
Traces: batch.create-quota-share-unknown-scheduling-policy-fails (assert code=='ClientException', got 'NotImplemented').

=== Cause 4 of 8: DescribeQuotaShare is an unimplemented scaffold stub ===
AWS: Smithy (com.amazonaws.batch#DescribeQuotaShare) binds POST /v1/describequotashare, success 200 with the share's fields, errors ClientException (400) / ServerException (500). A well-formed quotaShareArn that does not exist is refused with ClientException; a quotaShareArn with no job-queue/quota-share path segments is refused with a DISTINCT ClientException message ('... Quota Share arn should match a valid Quota Share arn pattern ...').
Emulator: HTTP 501 'NotImplemented', so both negative classes collapse into one unmodeled status.
Fix: the DescribeQuotaShare handler (.../src/ops/describe_quota_share.rs) — must distinguish malformed-ARN from nonexistent-ARN.
Traces: batch.describe-quota-share-unknown-fails (assert code=='ClientException', got 'NotImplemented').

=== Cause 5 of 8: ListJobs rejects unknown filter targets instead of returning 200 with an empty list ===
AWS: Smithy models ListJobs POST /v1/listjobs, 200 with required jobSummaryList; recorded behavior (blessed checks) is that ListJobs does NOT fail for an unknown jobQueue or an unknown arrayJobId — it returns 200 with jobSummaryList == []. ClientException is reserved for FILTER-SELECTION errors: zero filters ('Job queue name is required.') or two at once ('Array job id and multinode job id cannot both be set.').
Emulator: ListJobs(jobQueue='sim2real-behav-batch-absent-queue') answers ClientException 'Error executing request, Exception : Job queue sim2real-behav-batch-absent-queue does not exist, RequestId: None'; ListJobs(arrayJobId=<random uuid>) answers ClientException 'Error executing request, Exception : Job <uuid> does not exist, RequestId: None'. Both existence checks are over-strict, and they also mask the real filter-combination validation the check then wants to exercise.
Why: callers that poll a queue before/after it exists, or that list by array id defensively, get a 400 where AWS gives an empty page; pagination loops turn into errors.
Fix: the ListJobs handler's filter resolution — resolve unknown jobQueue/arrayJobId/multiNodeJobId to an empty result set and keep ClientException only for the zero-filter and multiple-filter cases.
Traces: batch.job-list-unknown-queue-returns-empty, batch.list-jobs-invalid-filter-combination-fails.

=== Cause 6 of 8: TerminateJob on an unknown jobId is rejected instead of being an idempotent no-op ===
AWS: Smithy models TerminateJob POST /v1/terminatejob, success 200 with an empty output. Recorded behavior: TerminateJob for a jobId that does not exist is ACCEPTED (200, idempotent no-op) and fabricates no job — DescribeJobs still returns nothing for it.
Emulator: answers ClientException 400 'Error executing request, Exception : Job <uuid> does not exist, RequestId: None'.
Why: cleanup/teardown paths and at-least-once terminate retries, which AWS lets you call blindly, now fail; a client cannot distinguish 'already gone' from a real error.
Fix: the TerminateJob handler — return the empty 200 output for an unknown jobId instead of raising ClientException.
Traces: batch.terminate-unknown-job-is-accepted, batch.terminate-service-job-unknown-id-silently-accepted (the latter fails on the same TerminateJob call).

=== Cause 7 of 8: over-limit tag maps are refused with an unmodeled ValidationException instead of BadRequestException ===
AWS: Smithy puts @length(min:1,max:50) on TagrisTagsMap for TagResource (and CreateJobQueue/RegisterJobDefinition), and models only ClientException (400) / ServerException (500) for TagResource. Recorded behavior for 51 tags on a job queue is HTTP 400 with error code 'BadRequestException' (the Tagris-backed tag limit refusal).
Emulator: answers error code 'ValidationException' — the generic scaffold validation code, which this service's model does not declare at all.
Why: a client branching on the error code (retry/limit handling) never matches; 'ValidationException' is not in the Batch error set, so SDK error-shape dispatch falls through to a generic error.
Fix: the tag-limit refusal path for Batch (the service's tag validation / error mapping, not the generic Scaffolded::validation channel) must emit BadRequestException, HTTP 400.
Traces: batch.job-queue-tag-limit-exceeded-rejected (expected BadRequestException for 51 tags, got 'ValidationException').

=== Cause 8 of 8: ClientException messages use a uniform 'Error executing request, Exception : ..., RequestId: None' envelope with the wrong wording and a null request id ===
AWS: Batch uses the 'Error executing request, Exception : <detail>, RequestId: <id>' envelope for SOME refusals, and a bare message for others, and when it uses the envelope the trailing RequestId is the refused call's own request id (the blessed checks build the expectation from the response's ResponseMetadata.RequestId, so that interpolation is the rule, not a recorded constant). Recorded bare-message refusals: CreateJobQueue naming a nonexistent compute environment -> 'Compute Environments must be created and valid before attaching them to a job queue'; CreateComputeEnvironment with a duplicate name -> 'Object already exists'; DescribeJobDefinitions with a nextToken it did not issue -> 'Error processing next token'. Recorded envelope detail for RegisterJobDefinition of type container with no properties -> 'ECS ContainerProperties, ecsProperties, or eksProperties is required when using type container.'
Emulator: wraps EVERY ClientException in the envelope and always writes the literal 'None' where the request id belongs, and the details are reworded. Observed: 'Error executing request, Exception : Compute environment arn:aws:batch:...:compute-environment/<name> does not exist, RequestId: None'; 'Error executing request, Exception : Object already exists: compute environment <name>, RequestId: None'; 'Error executing request, Exception : nextToken is not a token returned by DescribeJobDefinitions, RequestId: None'; 'Error executing request, Exception : containerProperties, ecsProperties or eksProperties is required for job definitions of type container, RequestId: None'. The HTTP status and the 'ClientException' code match AWS in these four cases — the divergence is the message envelope, the reworded detail, and the un-propagated request id.
Why: the request-id interpolation is a knowable rule and is broken for every Batch error (the literal 'None' is never a request id), so logs and support flows lose correlation; clients and fixtures that match on the documented bare messages mis-classify the refusal.
Fix: the Batch error serializer / ClientException construction — propagate the request id into the message and stop applying the envelope uniformly; the per-refusal detail strings belong in the handler that raises them.
Traces: batch.create-job-queue-unknown-compute-environment-fails, batch.duplicate-compute-environment-name-fails, batch.op-contract-describeJobDefinitions, batch.register-job-definition-contract, batch.register-job-definition-missing-container-properties-fails.

## Evidence
From the simparity-behavioral-aws-v1 replay (batch 5/232 passed):

Cause 1: [mismatch] batch.op-contract-deleteComputeEnvironment — AssertionError: expected ClientException, got 'NotImplemented'. [mismatch] batch.ce-delete-while-enabled-fails — AssertionError: expected ClientException deleting an ENABLED CE, got 'NotImplemented'. Source: delete_compute_environment.rs:156 returns Scaffolded::not_implemented("DeleteComputeEnvironment").
Cause 2: [mismatch] batch.op-contract-deleteJobQueue — expected ClientException, got 'NotImplemented'. [mismatch] batch.delete-enabled-job-queue-fails — expected ClientException deleting an ENABLED queue, got 'NotImplemented'. Source: delete_job_queue.rs:154.
Cause 3: [mismatch] batch.create-quota-share-unknown-scheduling-policy-fails — expected ClientException, got 'NotImplemented'.
Cause 4: [mismatch] batch.describe-quota-share-unknown-fails — expected ClientException for a well-formed but nonexistent quota share, got 'NotImplemented'.
Cause 5: [error] batch.job-list-unknown-queue-returns-empty — botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the ListJobs operation: Error executing request, Exception : Job queue sim2real-behav-batch-absent-queue does not exist, RequestId: None (expected HTTP 200, jobSummaryList == []). [error] batch.list-jobs-invalid-filter-combination-fails — ClientException ... Job d2f30357-a68b-4772-8a03-c2a8564f2244 does not exist (expected 200 + empty list for list_jobs(arrayJobId=<uuid>)).
Cause 6: [mismatch] batch.terminate-unknown-job-is-accepted — AssertionError: real Batch accepts TerminateJob for an unknown jobId (idempotent no-op), got 'ClientException'. [error] batch.terminate-service-job-unknown-id-silently-accepted — ClientException on TerminateJob: 'Job 0389e230-... does not exist'.
Cause 7: [mismatch] batch.job-queue-tag-limit-exceeded-rejected — AssertionError: expected BadRequestException for 51 tags, got 'ValidationException'.
Cause 8: [mismatch] batch.create-job-queue-unknown-compute-environment-fails — expected message 'Compute Environments must be created and valid before attaching them to a job queue', got 'Error executing request, Exception : Compute environment arn:aws:batch:us-west-2:...:compute-environment/sim2real-behav-ce-missing-... does not exist, RequestId: None'. [mismatch] batch.duplicate-compute-environment-name-fails — expected 'Object already exists', got 'Error executing request, Exception : Object already exists: compute environment ..., RequestId: None'. [mismatch] batch.op-contract-describeJobDefinitions — expected 'Error processing next token', got 'Error executing request, Exception : nextToken is not a token returned by DescribeJobDefinitions, RequestId: None'. [mismatch] batch.register-job-definition-contract and batch.register-job-definition-missing-container-properties-fails — expected 'Error executing request, Exception : ECS ContainerProperties, ecsProperties, or eksProperties is required when using type container., RequestId: None', got '... containerProperties, ecsProperties or eksProperties is required for job definitions of type container ...'.

Model facts used: smithy_query(operation="DeleteJobQueue,DeleteComputeEnvironment,CreateQuotaShare,DescribeQuotaShare,ListJobs,TerminateJob,TagResource") — all bind POST /v1/<op>, success 200, and model exactly ClientException(400)/ServerException(500); TagrisTagsMap carries @length(min 1, max 50); ListJobs output requires jobSummaryList.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Cause 1: aws batch delete-compute-environment --compute-environment <name> (any CE) -> observe HTTP 501 NotImplemented instead of 200/ClientException.
3. Cause 2: aws batch delete-job-queue --job-queue <name> -> HTTP 501 NotImplemented.
4. Cause 3: aws batch create-quota-share --quota-share-name q --job-queue <absent> --capacity-limits ... -> 501 instead of ClientException.
5. Cause 4: aws batch describe-quota-share --quota-share-arn arn:aws:batch:<region>:<acct>:job-queue/<jq>/quota-share/<qs> -> 501 instead of ClientException.
6. Cause 5: aws batch list-jobs --job-queue absent-queue, and aws batch list-jobs --array-job-id <random uuid> -> both 400 ClientException instead of 200 with jobSummaryList [].
7. Cause 6: aws batch terminate-job --job-id <random uuid> --reason x -> 400 ClientException instead of 200.
8. Cause 7: create a CE + job queue, then aws batch tag-resource --resource-arn <jobQueueArn> --tags <51 pairs> -> code ValidationException instead of BadRequestException.
9. Cause 8: aws batch create-job-queue with a computeEnvironmentOrder naming a nonexistent CE; aws batch create-compute-environment twice with the same name; aws batch describe-job-definitions --next-token bogus; aws batch register-job-definition --type container with no containerProperties -> inspect Error.Message: every one is wrapped in 'Error executing request, Exception : ..., RequestId: None'.
