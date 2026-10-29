Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.create-quota-share-duplicate-name-blocked batch.create-quota-share-duplicate-name-fails batch.create-quota-share-duplicate-name-rejected batch.create-quota-share-invalid-account-id-rejected batch.create-quota-share-invalid-share-value-rejected batch.update-quota-share-negative-amount-rejected batch.op-contract-createServiceEnvironment batch.op-contract-update-service-environment batch.create-quota-share-missing-required-field-rejected batch.create-quota-share-nonexistent-job-queue-rejected batch.create-quota-share-unknown-scheduling-policy-fails batch.op-contract-cancelJob batch.deregister-job-definition-contract batch.delete-consumable-resource-lifecycle batch.describe-quota-share-unknown-fails batch.describe-quota-share-unknown-id-fails batch.describe-unknown-quota-share-fails batch.update-quota-share-unknown-fails batch.update-quota-share-unknown-id-fails
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.create-quota-share-duplicate-name-blocked|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.create-quota-share-duplicate-name-fails|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.create-quota-share-duplicate-name-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.create-quota-share-invalid-account-id-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: batch.create-quota-share-invalid-share-value-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: batch.update-quota-share-negative-amount-rejected|_error_response#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: batch.op-contract-createServiceEnvironment|_error#13|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: batch.op-contract-update-service-environment|_error_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: batch.create-quota-share-missing-required-field-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch|ListQuotaShares|ClientException
Issue-Key: batch.op-contract-cancelJob|_error#11|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: batch.deregister-job-definition-contract|_error#13|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: batch.delete-consumable-resource-lifecycle|_error_code#10|AssertionError|raise AssertionError("expected the call to be refused with a ClientError, but it succeeded")
Issue-Key: batch.describe-quota-share-unknown-fails|run#7|AssertionError|assert control_message == expected_control, (
Issue-Key: batch.describe-quota-share-unknown-id-fails|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.describe-unknown-quota-share-fails|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: batch.update-quota-share-unknown-fails|run#16|AssertionError|assert message == expected_message, (
Issue-Key: batch.update-quota-share-unknown-id-fails|run#13|AssertionError|assert message == expected_message, (
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# batch: 8 net-new root causes on the quota-share / identifier plane — duplicate-name and not-found refusal text, missing maxCapacity and malformed-identifier validation, wrong error code for a malformed ARN, and ListQuotaShares refusing an unknown queue

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQuotaShare

## Description
Eight independent root causes in the AWS Batch quota-share / consumable-resource / job-definition planes. All eight are net-new: none is marked ALREADY FILED or OWNED in the replay report. The open issue 20261024T120647Z-batch-overly-permissive-control-and-service-job-planes.md and the resolved 20261024T120647Z-batch-refusal-classification-and-message-fidelity-tag-plane.md cover other operations of the same families; neither covers any cause below.

Cause 1 of 8 — CreateQuotaShare duplicate-name refusal carries a generic store message.
AWS: Smithy: CreateQuotaShare (POST /v1/createquotashare) declares exactly two errors, ClientException (httpError 400) and ServerException (500); the model states no message text, so this is recorded behavior only. The recording shows HTTP 400 ClientException whose message is the fixed text "QuotaShare with name " + the request's quotaShareName + " already exists for job queue " + the request's jobQueue (the recordings show both a bare queue name and a full job-queue ARN in that last slot, whichever the request supplied). Fixed text: "QuotaShare with name ", " already exists for job queue ". Variable parts: quotaShareName and jobQueue, both taken verbatim from the request. Note this refusal class carries NO "Error executing request, Exception : … RequestId: …" envelope.
Emulator: answers ClientException with 'Error executing request, Exception : Object already exists, RequestId: <id>' — a generic persistence-layer message that names neither the share nor the queue, and wraps it in the envelope this class does not use.
Why: a client cannot tell WHICH share name collided or on which queue, so idempotent create-or-reuse logic that parses the name out of the message (or matches on the documented text) never matches and retries forever.
Fix: the CreateQuotaShare handler's duplicate-name check in the batch crate — it must raise its own ClientException with the quota-share text instead of letting the state store's "Object already exists" surface.
Traces: batch.create-quota-share-duplicate-name-blocked, batch.create-quota-share-duplicate-name-fails, batch.create-quota-share-duplicate-name-rejected.

Cause 2 of 8 — CreateQuotaShare with a malformed jobQueue ARN is classified ClientException, not BadRequestException.
AWS: Smithy declares only ClientException/ServerException for CreateQuotaShare, so the model says nothing here; recorded behavior only (the check records that two consecutive real-AWS replays answered BadRequestException). Recorded response: error code "BadRequestException" with message "<jobQueue>/quota-share/<quotaShareName> is not a valid AmazonResourceName (ARN)" — fixed text " is not a valid AmazonResourceName (ARN)", variable part the ARN the service composes from the request's jobQueue and quotaShareName. AWS treats a syntactically invalid ARN (here an account-id segment that is not 12 digits) as an EARLIER boundary than the ClientException a well-formed-but-nonexistent queue draws.
Emulator: answers code 'ClientException', collapsing the malformed-ARN boundary into the not-found boundary.
Why: a client that branches on BadRequestException (malformed input, never retry) versus ClientException (resource state, may retry after creating the queue) takes the wrong branch and retries a request that can never succeed.
Fix: ARN parsing/validation in the CreateQuotaShare handler's input validation, ahead of the job-queue lookup.
Traces: batch.create-quota-share-invalid-account-id-rejected.

Cause 3 of 8 — a non-positive capacityLimits[].maxCapacity is accepted on CreateQuotaShare and UpdateQuotaShare.
AWS: Smithy: CreateQuotaShare.capacityLimits -> QuotaShareCapacityLimits is a required list; UpdateQuotaShare.capacityLimits is the same list shape. The model carries no range trait on maxCapacity, so the bound is recorded behavior only: both recordings show HTTP 400 ClientException with message "Error executing request, Exception : maxCapacity must be greater than 0., RequestId: <request id of this very call>". Fixed text: "Error executing request, Exception : maxCapacity must be greater than 0., RequestId: "; variable part: the refused call's own request id.
Emulator: returns HTTP 200 and mints the share (CreateQuotaShare) / applies the update (UpdateQuotaShare) with the negative maxCapacity, so a negative capacity enters the scheduler's capacity accounting.
Why: the scheduler's capacity arithmetic is seeded with a negative allocation, and a client that relies on the server to reject garbage silently ends up with an unschedulable or over-committed share.
Fix: input validation of capacityLimits entries in the CreateQuotaShare and UpdateQuotaShare handlers.
Traces: batch.create-quota-share-invalid-share-value-rejected, batch.update-quota-share-negative-amount-rejected.

Cause 4 of 8 — a non-positive capacityLimits[].maxCapacity is accepted on CreateServiceEnvironment and UpdateServiceEnvironment.
AWS: Smithy: CreateServiceEnvironment.capacityLimits -> CapacityLimits (required list), UpdateServiceEnvironment.capacityLimits -> CapacityLimits; no range trait is modelled, so recorded behavior only. Both recordings show ClientException; the message's fixed text is "Max capacity must be more than 0." — on CreateServiceEnvironment it arrives inside the envelope "Error executing request, Exception : Max capacity must be more than 0., RequestId: <this call's request id>", on UpdateServiceEnvironment it arrives bare, with no envelope.
Emulator: returns HTTP 200 for both, minting a service environment (and leaving it behind, which the check's follow-up DescribeServiceEnvironments proves) / applying the negative capacity to an existing one.
Why: a SAGEMAKER_TRAINING service environment with a negative capacity limit is accepted into state, so every placement decision made against it is computed from an impossible capacity.
Fix: input validation of capacityLimits entries in the CreateServiceEnvironment and UpdateServiceEnvironment handlers, which must also leave no resource behind on the refused create.
Traces: batch.op-contract-createServiceEnvironment, batch.op-contract-update-service-environment.

Cause 5 of 8 — the empty-capacityLimits refusal on CreateQuotaShare is worded by the emulator.
AWS: Smithy: capacityLimits is a required member of CreateQuotaShareRequest; an empty-but-present list is refused server-side. Recorded response: ClientException, message "Error executing request, Exception : capacityLimits is required and cannot be empty., RequestId: <this call's request id>". Fixed text is everything but the request id.
Emulator: same envelope and same request id, but the inner sentence is the emulator's own: 'capacityLimits must name at least one capacity limit.'
Why: a client matching AWS's documented refusal sentence (or a test fixture asserting it) sees an unknown message for a known refusal class.
Fix: the message string in the CreateQuotaShare handler's capacityLimits validation.
Traces: batch.create-quota-share-missing-required-field-rejected.

Cause 6 of 8 — ListQuotaShares refuses an unknown jobQueue instead of answering an empty list.
AWS: Smithy: ListQuotaShares (POST /v1/listquotashares) takes a required jobQueue and returns quotaShares -> QuotaShareList; it is @paginated. Recorded behavior: a ListQuotaShares naming a job queue that does not exist answers HTTP 200 with an empty quotaShares list — the recorded checks use exactly this call to prove a rejected CreateQuotaShare left nothing behind under the (nonexistent) queue it named.
Emulator: answers HTTP 400 ClientException "Error executing request, Exception : <jobQueue> does not exist., RequestId: <id>", so the read itself raises.
Why: any client sweeping or reconciling quota shares across queue names it has not yet created gets an exception instead of an empty page, and cannot distinguish "no shares" from "call failed".
Fix: the ListQuotaShares handler — the queue lookup must not be a precondition that refuses; an unresolved queue yields an empty page.
Traces: batch.create-quota-share-nonexistent-job-queue-rejected, batch.create-quota-share-unknown-scheduling-policy-fails.

Cause 7 of 8 — a malformed resource identifier is accepted with HTTP 200 on three operations.
AWS: Smithy: CancelJob.jobId, DeregisterJobDefinition.jobDefinition and DeleteConsumableResource.consumableResource are each a required String with no pattern trait, so the name rule is recorded behavior only. All three recordings show HTTP 400 ClientException with the message "Provided name <the identifier exactly as submitted> is not supported pattern[a-zA-Z_0-9-]{1,128}" — fixed text "Provided name " and " is not supported pattern[a-zA-Z_0-9-]{1,128}", variable part the submitted identifier echoed verbatim. Identifiers containing spaces or punctuation outside [a-zA-Z_0-9-] are refused before any lookup.
Emulator: answers HTTP 200 with an empty body for all three — CancelJob on 'not a valid job id!!', DeregisterJobDefinition on '!! not a valid name !!:1', DeleteConsumableResource on '!! not a valid consumable resource identifier !!'.
Why: a typo'd or user-supplied id is reported as a successful cancel/deregister/delete, so a client believes a resource was acted on when nothing was found, and no error ever surfaces to correct it.
Fix: a shared identifier-pattern validation applied in the CancelJob, DeregisterJobDefinition and DeleteConsumableResource handlers before resolution.
Traces: batch.op-contract-cancelJob, batch.deregister-job-definition-contract, batch.delete-consumable-resource-lifecycle.

Cause 8 of 8 — the unknown-quota-share refusal on DescribeQuotaShare and UpdateQuotaShare is wrapped in an envelope AWS does not use for it.
AWS: Smithy: DescribeQuotaShare and UpdateQuotaShare both take a required quotaShareArn and declare ClientException/ServerException. Recorded behavior: a well-formed but nonexistent quotaShareArn draws ClientException whose message is the BARE sentence "<quotaShareArn> does not exist." — fixed text " does not exist.", variable part the ARN from the request. There is no "Error executing request, Exception :" prefix and no ", RequestId: …" suffix on this class, unlike the create/validation classes above where the envelope IS part of AWS's message.
Emulator: answers ClientException 'Error executing request, Exception : <arn> does not exist., RequestId: <id>' — the correct sentence, wrapped in the envelope this refusal class does not carry.
Why: the emulator applies the envelope uniformly, so a client cannot use the envelope's presence to tell a validation refusal from a not-found refusal, and exact-message matching fails for every unknown-share read and update.
Fix: the error-rendering path for the not-found refusal in the DescribeQuotaShare and UpdateQuotaShare handlers — the envelope must be applied per refusal class, not globally.
Traces: batch.describe-quota-share-unknown-fails, batch.describe-quota-share-unknown-id-fails, batch.describe-unknown-quota-share-fails, batch.update-quota-share-unknown-fails, batch.update-quota-share-unknown-id-fails.

## Evidence
Cause 1: expected message 'QuotaShare with name sim2real_batch_qs_<id> already exists for job queue sim2real-batch-jq-<id>', got 'Error executing request, Exception : Object already exists, RequestId: <uuid>' (three checks, one with a full job-queue ARN in the queue slot).
Cause 2: expected BadRequestException with '<arn>/quota-share/<name> is not a valid AmazonResourceName (ARN)', got code 'ClientException'.
Cause 3: expected ClientException 'Error executing request, Exception : maxCapacity must be greater than 0., RequestId: <id>'; CreateQuotaShare with capacityLimits=[{maxCapacity: -5, capacityUnit: ml.m6i.large}] returned HTTP 200; UpdateQuotaShare with maxCapacity -5 returned HTTP 200 {'quotaShareName': ..., 'quotaShareArn': ...}.
Cause 4: expected ClientException 'Max capacity must be more than 0.' (enveloped on create, bare on update); CreateServiceEnvironment with capacityLimits=[{maxCapacity: -1, capacityUnit: NUM_INSTANCES}] returned HTTP 200 {'serviceEnvironmentName': ..., 'serviceEnvironmentArn': ...}; UpdateServiceEnvironment with maxCapacity -5 returned HTTP 200.
Cause 5: expected 'Error executing request, Exception : capacityLimits is required and cannot be empty., RequestId: <id>', got 'Error executing request, Exception : capacityLimits must name at least one capacity limit., RequestId: <same id>'.
Cause 6: ListQuotaShares(jobQueue='sim2real-batch-no-such-queue-<id>') raised botocore.errorfactory.ClientException: 'Error executing request, Exception : sim2real-batch-no-such-queue-<id> does not exist., RequestId: <id>'; expected HTTP 200 with quotaShares == [].
Cause 7: CancelJob(jobId='not a valid job id!!') -> HTTP 200, content-length 2; DeregisterJobDefinition(jobDefinition='!! not a valid name !!:1') -> HTTP 200, content-length 2; DeleteConsumableResource(consumableResource='!! not a valid consumable resource identifier !!') -> succeeded. All three expected ClientException 'Provided name <value> is not supported pattern[a-zA-Z_0-9-]{1,128}'.
Cause 8: expected 'arn:aws:batch:<region>:<account>:job-queue/<queue>/quota-share/<share> does not exist.', got 'Error executing request, Exception : arn:aws:batch:<region>:<account>:job-queue/<queue>/quota-share/<share> does not exist., RequestId: <id>' — identical sentence, extra envelope (5 checks across DescribeQuotaShare and UpdateQuotaShare).

## Steps to Reproduce
Replay the behavioral suite against the emulator and run exactly the checks in trace_ids.
1. Cause 1: create a SAGEMAKER_TRAINING job queue, CreateQuotaShare once (succeeds), then CreateQuotaShare again with the same quotaShareName on the same jobQueue; read Error.Message.
2. Cause 2: CreateQuotaShare with jobQueue='arn:aws:batch:<region>:12345:job-queue/nonexistent-queue' (5-digit account segment); read Error.Code.
3. Cause 3: CreateQuotaShare with capacityLimits=[{'maxCapacity': -5, 'capacityUnit': 'ml.m6i.large'}]; then UpdateQuotaShare on a VALID share with capacityLimits=[{'maxCapacity': -5, ...}]. Both must be refused.
4. Cause 4: CreateServiceEnvironment(serviceEnvironmentType='SAGEMAKER_TRAINING', capacityLimits=[{'maxCapacity': -1, 'capacityUnit': 'NUM_INSTANCES'}]); then UpdateServiceEnvironment on a VALID environment with maxCapacity -5. Both must be refused and leave state unchanged.
5. Cause 5: CreateQuotaShare with capacityLimits=[] against a real VALID queue; read Error.Message.
6. Cause 6: ListQuotaShares(jobQueue=<a name no queue has>); expect HTTP 200 and quotaShares == [].
7. Cause 7: CancelJob(jobId='not a valid job id!!', reason='probe'); DeregisterJobDefinition(jobDefinition='!! not a valid name !!:1'); DeleteConsumableResource(consumableResource='!! not a valid consumable resource identifier !!').
8. Cause 8: DescribeQuotaShare and UpdateQuotaShare on a well-formed quotaShareArn that was never created; read Error.Message.
