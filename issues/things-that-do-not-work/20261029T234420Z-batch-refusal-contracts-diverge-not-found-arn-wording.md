Issue-Source: trace-sweep-divergence
Issue-Trace: batch-service-job-api-error-contracts
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-service-job-api-error-contracts batch-job-queue-and-quota-share-error-contracts batch-consumable-resource-pool-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|SubmitServiceJob|4xx->4xx|$.message
Issue-Key: batch|SubmitServiceJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: batch|SubmitServiceJob|4xx->4xx|status
Issue-Key: batch|ListQuotaShares|2xx->4xx|$.quotaShares
Issue-Key: batch|ListQuotaShares|2xx->4xx|status
Issue-Key: batch|GetJobQueueSnapshot|4xx->4xx|$.message
Issue-Key: batch|ListJobsByConsumableResource|4xx->4xx|$.message
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# batch refusal contracts diverge: not-found ARN wording, ListQuotaShares refusing an unknown queue, and SubmitServiceJob authorizing after queue lookup

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitServiceJob, ListQuotaShares, GetJobQueueSnapshot, ListJobsByConsumableResource

## Description
Four net-new root causes on the AWS Batch job-queue / consumable-resource / service-job request plane: a refusal is raised where AWS answers success, a refusal is raised in the wrong class and status because resource resolution runs before authorization, and two not-found refusals are worded by the emulator instead of using the service's "<arn> does not exist." sentence. None of the four is marked ALREADY FILED or OWNED in the replay report (owned_by was empty for every cluster). Related but non-covering open issues: 20261029T173319Z-batch-8-net-new-root-causes-on-the.md describes the same ListQuotaShares behavior from the BEHAVIORAL corpus (its cause 6) but carries no wire replay key, so the wire step below is not gated by it; 20261029T195005Z-batch-deletequotashare-501-notimplemented-leaves-both-of-its.md and 20261029T173319Z-batch-8-net-new-root-causes-relationship-delete.md cover other operations.

Cause 1 of 4 - SubmitServiceJob resolves the job queue before authorizing the caller, so an unauthorized request is answered 400 ClientException instead of 401.
AWS: Smithy: SubmitServiceJob, POST /v1/submitservicejob, required jobName/jobQueue/serviceRequestPayload/serviceJobType (ServiceJobType enum), declared errors ClientException (httpError 400) and ServerException (500). The model declares NO UnauthorizedException shape (lookup of 'UnauthorizedException' in com-amazonaws-batch-awsbatchv20160810.json returns not found), so the refusal below is recorded behavior only: the recorded response is HTTP 401 with x-amzn-errortype: UnauthorizedException and message body "Unauthorized" - the whole message is fixed text, with no variable part (it names neither the queue nor the job). The ordering is the requirement: the service-job submission path decides authorization BEFORE it resolves jobQueue, so a request that is not authorized is refused 401 whether or not the named queue exists.
Emulator: skips that gate entirely, resolves the queue first, and answers HTTP 400 with x-amzn-errortype: ClientException and message "JobQueue <jobQueue from the request> not found." - wrong status class (400 vs 401), wrong error type header (ClientException vs UnauthorizedException) and a message that is the emulator's own not-found text.
Why: a client branches on 401 (re-acquire credentials / stop, never retry with the same identity) versus 400 ClientException (fix the request or create the queue, then retry). It takes the retry branch forever, and the leaked "queue not found" text tells an unauthorized caller whether a queue name exists - an existence oracle AWS deliberately does not expose on this path.
Fix: the SubmitServiceJob handler in the batch crate - the authorization decision must run ahead of the jobQueue lookup, and its refusal must be rendered with the service-job plane's 401/UnauthorizedException envelope rather than the generic ClientException renderer.
Traces: batch-service-job-api-error-contracts#task#1.

Cause 2 of 4 - ListQuotaShares refuses a job queue it cannot resolve instead of answering an empty page.
AWS: Smithy: ListQuotaShares, POST /v1/listquotashares, input jobQueue (required) + maxResults/nextToken, output quotaShares -> QuotaShareList + nextToken, @paginated (inputToken nextToken, outputToken nextToken, items quotaShares), errors ClientException (400) / ServerException (500). Recorded behavior: the call answers HTTP 200 with quotaShares serialized as an empty list - the listing is a read that reports "no shares" rather than a precondition on the queue, which is exactly why the recorded fixture uses it to prove that a refused quota-share create left nothing behind.
Emulator: answers HTTP 400 and emits no quotaShares member at all (replay detail: "status: expected 200, got 400" and "$.quotaShares: missing (expected [], got None)"), raising a ClientException out of the queue lookup.
Why: every sweep or reconcile over quota shares fails instead of returning an empty page, and the client cannot distinguish "this queue has no quota shares" from "the call failed" - a paginator fed this response aborts mid-sweep.
Fix: the ListQuotaShares handler's job-queue resolution must not be a refusal precondition; an unresolved queue yields HTTP 200 with an empty quotaShares list and no nextToken.
Traces: batch-job-queue-and-quota-share-error-contracts#task#9.

Cause 3 of 4 - GetJobQueueSnapshot's not-found refusal is the emulator's own sentence and names the bare queue name instead of the resolved ARN.
AWS: Smithy: GetJobQueueSnapshot, POST /v1/getjobqueuesnapshot, input jobQueue (required), output frontOfQueue/frontOfQuotaShares/queueUtilization, errors ClientException (httpError 400) / ServerException (500). The model states no message text, so the wording is recorded behavior only: status HTTP 400, error ClientException, message = the full job-queue ARN the service composes for the requested queue, followed by the fixed text " does not exist." - i.e. fixed text " does not exist." (including the trailing period), variable part the ARN built from the partition/region/account of the request and the jobQueue name it supplied. The sentence carries no "Error executing request, Exception : ... , RequestId: ..." envelope, unlike this service's input-validation refusals.
Emulator: keeps the status and class but substitutes its own sentence "JobQueue <name> not found." - wrong fixed text (" not found." plus a "JobQueue " prefix) and it names the bare queue name where AWS names the resolved ARN.
Why: a client or test that matches the service's not-found sentence (or parses the ARN out of it to learn the account/region the service resolved) sees an unknown message and cannot recover the resource identity, so not-found handling that is exact-match keyed on AWS's text falls through to a generic error path.
Fix: the not-found refusal in the GetJobQueueSnapshot handler - it must render the resolved job-queue ARN plus the service's "<arn> does not exist." sentence, using the same not-found renderer as the other job-queue readers rather than a hand-written message.
Traces: batch-job-queue-and-quota-share-error-contracts#task#7.

Cause 4 of 4 - ListJobsByConsumableResource's not-found sentence drops the terminating period.
AWS: Smithy: ListJobsByConsumableResource, POST /v1/listjobsbyconsumableresource, input consumableResource (required) + filters/maxResults/nextToken, output jobs (required) + nextToken, @paginated, errors ClientException (400) / ServerException (500). Recorded behavior only for the text: HTTP 400 ClientException whose message is the consumable-resource ARN followed by the fixed text " does not exist." - the period is part of the fixed text, the ARN is the variable part derived from the request's consumableResource and the caller's partition/region/account.
Emulator: emits the same ARN and the same words but ends the sentence without the period - "<arn> does not exist" (replay detail: $.message value_mismatch, expected '<arn> does not exist.', got '<arn> does not exist').
Why: the message is a wire requirement, so any exact-match on the service's not-found sentence fails; combined with cause 3 it shows the crate has no single not-found renderer, so each handler's wording drifts independently.
Fix: the not-found refusal in the ListJobsByConsumableResource handler (same shared not-found renderer as cause 3 - one sentence template "<arn> does not exist." used by every batch reader).
Traces: batch-consumable-resource-pool-lifecycle#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 14 fixtures, 98 steps, 85 passed, 13 mismatched, score 86.7%).

Cause 1 - divergence_key service=batch|SubmitServiceJob|status=401->400|keys=$.message,$headers.x-amzn-errortype,status at batch-service-job-api-error-contracts#task#1: expected_status 401, actual_status 400; detail "status: expected 401, got 400 / $.message: value_mismatch (expected 'Unauthorized', got 'JobQueue sim2real-probe-batch-jq-absent-<suffix> not found.') / $headers.x-amzn-errortype: value_mismatch (expected 'UnauthorizedException', got 'ClientException')". The recorded queue name is a per-run probe value; the requirement is the 401 status, the UnauthorizedException errortype and the fixed message "Unauthorized".

Cause 2 - divergence_key service=batch|ListQuotaShares|status=200->400|keys=$.quotaShares,status at batch-job-queue-and-quota-share-error-contracts#task#9: expected_status 200, actual_status 400; detail "status: expected 200, got 400 / $.quotaShares: missing (expected [], got None)".

Cause 3 - divergence_key service=batch|GetJobQueueSnapshot|status=400->400|keys=$.message at batch-job-queue-and-quota-share-error-contracts#task#7: detail "$.message: value_mismatch (expected 'arn:aws:batch:<region>:<account>:job-queue/<queue> does not exist.', got 'JobQueue <queue> not found.')". The recorded instance was arn:aws:batch:us-west-2:000000000000:job-queue/sim2real-probe-batch-jq-absent-snap-<suffix>; region, account and queue name are request/state values filling the template, the fixed text is " does not exist.".

Cause 4 - divergence_key service=batch|ListJobsByConsumableResource|status=400->400|keys=$.message at batch-consumable-resource-pool-lifecycle#task#11: detail "$.message: value_mismatch (expected 'arn:aws:batch:<region>:<account>:consumable-resource/<name> does not exist.', got 'arn:aws:batch:<region>:<account>:consumable-resource/<name> does not exist')" - identical except the missing terminating period.

Smithy evidence (services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json): all four operations declare exactly ClientException (httpError 400) and ServerException (500); there is NO UnauthorizedException shape in the model, so the recorded 401/UnauthorizedException on SubmitServiceJob is recorded behavior the emulator must reproduce without model support. ListQuotaShares is @paginated over quotaShares.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator.
2. Cause 1: fixture batch-service-job-api-error-contracts, step task#1 - POST /v1/submitservicejob naming a jobQueue that does not exist, with a valid jobName, serviceJobType and serviceRequestPayload, from an identity that is not authorized for the service-job plane. Expect HTTP 401, header x-amzn-errortype: UnauthorizedException and body message "Unauthorized"; observed HTTP 400, x-amzn-errortype: ClientException, message "JobQueue <name> not found.".
3. Cause 2: fixture batch-job-queue-and-quota-share-error-contracts, step task#9 - POST /v1/listquotashares with jobQueue set to the probe queue name. Expect HTTP 200 with quotaShares == []; observed HTTP 400 with no quotaShares member.
4. Cause 3: same fixture, step task#7 - POST /v1/getjobqueuesnapshot with jobQueue set to a queue that does not exist. Expect HTTP 400 ClientException whose message is the resolved job-queue ARN followed by " does not exist."; observed "JobQueue <name> not found.".
5. Cause 4: fixture batch-consumable-resource-pool-lifecycle, step task#11 - POST /v1/listjobsbyconsumableresource naming a consumable resource that does not exist. Expect HTTP 400 ClientException "<consumable-resource ARN> does not exist." with the terminating period; observed the same sentence without it.
