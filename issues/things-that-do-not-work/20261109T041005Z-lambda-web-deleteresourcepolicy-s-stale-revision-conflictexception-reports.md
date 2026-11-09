Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged|run#7|AssertionError|assert message == "The policy was updated by another request. Try again.", (
Issue-Commit: 161bd4511631ea440c3ef5c62880db3e0b52f4c6

# lambda-web DeleteResourcePolicy's stale-revision ConflictException reports PutResourcePolicy's sentence from a shared constant, not its own wording (1 cause)

> _Note: this issue was identified against the code base at commit `161bd4511631ea440c3ef5c62880db3e0b52f4c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
One net-new root cause in lambda-web's resource-policy optimistic-concurrency path. It is a REGRESSION of the now-fixed issue 20261106T141524Z-lambda-web-the-stale-revision-conflictexception-message-from.md: that fix corrected PutResourcePolicy's stale-revision sentence but wrote it into the SHARED constant that DeleteResourcePolicy also reads, so DeleteResourcePolicy now reports Put's sentence instead of its own. (This is not a left-refused fix: the refusal itself is correct and recorded; only the message text of the correctly-refused call diverges.)

--------------------------------------------------------------------
Cause 1 of 1: DeleteResourcePolicy, refusing a request whose revisionId is not the stored policy's current generation, reports the PutResourcePolicy stale-revision sentence instead of the shorter sentence AWS answers for the delete path. The two mutating resource-policy operations do NOT share one message in AWS.
Failing check ids: lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged

AWS: Smithy (services/service-com-amazonaws-lambdaweb-lambdaweb/smithy-model/com-amazonaws-lambdaweb-lambdaweb.json), service LambdaWeb, restJson1. DeleteResourcePolicy = DELETE /2025-03-07/resource-policy/{resourceArn}, @idempotent, success 204; input resourceArn (ResourceArn, @required, @httpLabel, @length 1..256, @pattern ^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$) and revisionId (PolicyRevisionId, @httpQuery "RevisionId", @pattern ^[0-9a-f-]{36}$). It models ConflictException (errorKind client, httpError 409, `message` @required), whose shape doc covers "...or the policy revision ID that you specified doesn't match the current policy. Resolve the conflict and try again." The model does not fix the TEXT, so the text is recorded behavior only: for a DELETE carrying a revisionId that is not the stored policy's current generation, AWS answers HTTP 409, error code ConflictException, message `The policy was updated by another request. Try again.` That message is ENTIRELY FIXED TEXT — it has NO variable parts: no revision id, no resource ARN, no function name, nothing taken from the request or from state. The refusal is total: the stored policy document and its revisionId are left exactly as they were, and presenting the current revisionId afterwards does delete it (after which GetResourcePolicy answers 404 ResourceNotFoundException with the fixed sentence `The function does not have a policy.`).

The wording is operation-specific. PutResourcePolicy's stale-revision refusal is recorded as the longer sentence pair `The policy was updated or deleted by another request. Try again with the latest policy revisionId.` (that is what issue 20261106T141524Z and the wire corpus pinned, for PUT). The DELETE path's recording is the shorter sentence above. One shared constant therefore cannot satisfy both: the stale-revision message must be per-operation.

Emulator: the classification is correct — HTTP 409 with error code ConflictException, the policy left unchanged, and the current revisionId still deleting it. The MESSAGE is wrong: DeleteResourcePolicy returns `The policy was updated or deleted by another request. Try again with the latest policy revisionId.` It reads the single shared constant STALE_POLICY_REVISION_MESSAGE, which now holds PutResourcePolicy's sentence, so the delete path emits Put's text verbatim (fixed text in both cases; neither sentence has variable parts, so the divergence is the whole sentence). The replay asserts on the delete path and fails with: AssertionError: unexpected conflict message 'The policy was updated or deleted by another request. Try again with the latest policy revisionId.'

Why: the message is the only part of this error contract that distinguishes the operations' conflict reports, since both answer the same 409/ConflictException. A client that matches AWS's DeleteResourcePolicy sentence to decide "re-read the policy and retry the delete with the latest revisionId" versus "surface an unexpected conflict" takes the wrong branch, and golden-output/SDK-integration assertions that pin AWS's delete-path text fail even though the retry logic and the no-op-on-conflict semantics are right. Because the emulator's own constant was made correct for PUT, a fixer who treats the two paths as one cause will keep re-breaking whichever path it is not tuned for.

Fix: belongs in the resource-policy slice of the service state that defines the stale-revision message, plus the DeleteResourcePolicy handler's conflict branch that reads it. Split the single shared constant into the two per-operation messages (PUT keeps the longer sentence pair; DELETE gets AWS's shorter `The policy was updated by another request. Try again.`) and have each mutating handler raise its own. Do not change the 409 status, the ConflictException code, the extra error members, or the leave-the-policy-untouched behavior — those are correct here.

Traces: lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged. Citations against this checkout (may have drifted): services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/resource_policies.rs:51-66 (STALE_POLICY_REVISION_MESSAGE, documented as "The `message` BOTH mutating resource-policy operations report" — that sharing is the defect), raised at src/ops/delete_resource_policy.rs:219-228 and src/ops/put_resource_policy.rs:212-231.

Related but NOT covering this: fixed issue 20261106T141524Z-lambda-web-the-stale-revision-conflictexception-message-from.md carries only the PUT check's replay key (lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation) and explicitly prescribed one shared sentence for both operations, which is what produced this divergence. Open issue 20261107T125554Z-lambda-web-resource-policy-and-tag-refusals-diverge.md (wire corpus) Cause 5 likewise pins only PutResourcePolicy's sentence; its Cause 4 covers the extra resourceId/resourceType members on these same refusals and is a different defect. Open issue 20261107T082505Z-lambda-web-resource-not-found-and-reserved-tag.md owns the not-found and reserved-tag-key wording and carries neither this check nor this message.

## Evidence
Check lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged, verdict [mismatch]:

  AssertionError: unexpected conflict message 'The policy was updated or deleted by another request. Try again with the latest policy revisionId.'
    at check_lambda_web_delete_resource_policy_with_stale_revision_leaves_policy_unchanged.py:142

Sequence: CreateWebFunction -> current_rev = PutResourcePolicy(resourceArn=<functionArn>, policy=<valid policy>)["revisionId"]; then DeleteResourcePolicy(resourceArn=<functionArn>, revisionId=<a revision id that is not current_rev>).

  Expected (AWS):    HTTP 409, Error.Code = "ConflictException", Error.Message = "The policy was updated by another request. Try again."  — entirely fixed text, no request- or state-derived values.
  Actual (emulator): HTTP 409, Error.Code = "ConflictException" (both correct), Error.Message = "The policy was updated or deleted by another request. Try again with the latest policy revisionId."  — PutResourcePolicy's sentence, served from the shared constant; also entirely fixed text, so the whole sentence is the divergence.

The check's earlier assertion on the error CODE passed (`code == "ConflictException"`), so only the message text diverges. The assertions after the failing one were never reached, but they state the rest of the recorded contract for this path and should hold once the message is fixed: GetResourcePolicy still reports the unchanged policy at the unchanged revisionId with Statement[0].Sid == "kept" (the rejected delete is a no-op), DeleteResourcePolicy with the CURRENT revisionId succeeds, and a following GetResourcePolicy answers ResourceNotFoundException with the fixed message "The function does not have a policy."

Model evidence: DeleteResourcePolicy http {DELETE /2025-03-07/resource-policy/{resourceArn}, code 204}, @idempotent; revisionId is @httpQuery "RevisionId" with @pattern ^[0-9a-f-]{36}$; errors include ConflictException (client, httpError 409) with @required message. The model fixes neither the text nor any per-operation wording, so the two operations' recorded sentences are the requirement.

Source evidence for the shared constant (this checkout): src/state/resource_policies.rs:51-66 declares STALE_POLICY_REVISION_MESSAGE = "The policy was updated or deleted by another request. Try again with the latest policy revisionId." and documents it as the message "both mutating resource-policy operations report"; src/ops/delete_resource_policy.rs:222 interpolates exactly that constant into the ConflictException it raises at :219-228.

## Steps to Reproduce
1. Boot the emulator and point a botocore `lambda-web` client at its data listener (region us-west-2, any credentials).
2. Create an execution role (iam) and a versioned code bucket (s3) holding code.zip, then CreateWebFunction(functionName=<name>, revisionConfig={buildConfig:{codeConfig:{s3Object:{bucket,key}}, runtimeConfig:{runtime:"nodejs24.x"}}, serviceConfig:{executionRoleArn}}, endpointConfig={endpointName:"ep", endpointType:"HomeRegion", authType:"IamAuth"}); keep the returned functionArn.
3. current_rev = PutResourcePolicy(resourceArn=<functionArn>, policy=<any valid resource policy JSON>)["revisionId"].
4. Call DeleteResourcePolicy(resourceArn=<functionArn>, revisionId=<any well-formed revision id that is not current_rev, e.g. "00000000-0000-0000-0000-000000000000">).
5. Observe the ClientError: Code is ConflictException and the status is 409 (both correct), but Message is "The policy was updated or deleted by another request. Try again with the latest policy revisionId." instead of AWS's "The policy was updated by another request. Try again."
6. Confirm the rest of the path is already right: GetResourcePolicy(resourceArn=<functionArn>) still returns the original policy at current_rev (the rejected delete changed nothing), DeleteResourcePolicy(resourceArn=<functionArn>, revisionId=current_rev) succeeds, and GetResourcePolicy then raises ResourceNotFoundException "The function does not have a policy."
7. Note the counterpart that must NOT regress: PutResourcePolicy with a stale revisionId must keep answering the longer sentence "The policy was updated or deleted by another request. Try again with the latest policy revisionId." — so the single shared constant has to become two per-operation messages. Re-replay both checks together: lambda-web.delete-resource-policy-with-stale-revision-leaves-policy-unchanged and lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation.
