Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.function-codesigning-association-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.function-codesigning-association-lifecycle|run#19|Timeout|time.sleep(2)
Issue-Commit: 8323f4d349418e3a01a6cea3a48f7209706d6844

# lambda: DeleteFunctionCodeSigningConfig never dissociates the config — the function stays in ListFunctionsByCodeSigningConfig and GetFunctionCodeSigningConfig keeps reporting the ARN (1 cause)

> _Note: this issue was identified against the code base at commit `8323f4d349418e3a01a6cea3a48f7209706d6844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunctionCodeSigningConfig

## Description
One net-new root cause. A fixer must close it before submitting.

=== Cause 1 of 1: the function<->CodeSigningConfig association is never torn down ===
Failing check: lambda.function-codesigning-association-lifecycle

AWS: the association between a function and a code-signing config is a two-sided, mutable binding. Smithy: PutFunctionCodeSigningConfig (PUT /2020-06-30/functions/{FunctionName}/code-signing-config, 200) attaches CodeSigningConfigArn (required, pattern ^arn:...:code-signing-config:csc-[a-z0-9]{17}$) to FunctionName; GetFunctionCodeSigningConfig (GET on the same URI, 200) reads it back; DeleteFunctionCodeSigningConfig (DELETE on the same URI, 204 No Content, errors CodeSigningConfigNotFoundException 404, InvalidParameterValueException 400, ResourceConflictException 409, ResourceNotFoundException 404) removes it; ListFunctionsByCodeSigningConfig (GET /2020-04-22/code-signing-configs/{CodeSigningConfigArn}/functions, 200, paginated by Marker/NextMarker over FunctionArns) enumerates the functions currently bound to one config. After a successful DELETE the binding is gone on BOTH sides: the function's ARN no longer appears in that config's FunctionArns, and GetFunctionCodeSigningConfig on the function still SUCCEEDS with 200 (the function itself exists, so neither ResourceNotFoundException nor CodeSigningConfigNotFoundException applies) but its response simply no longer contains a CodeSigningConfigArn member. Note GetFunctionCodeSigningConfig's output marks CodeSigningConfigArn as required in the model while the documented no-association response omits it — the recorded AWS behavior (omission, not an error, not an empty string) is the requirement here.
Emulator: the dissociation never takes effect. The check attaches the config (CreateCodeSigningConfig, PutFunctionCodeSigningConfig, GetFunctionCodeSigningConfig reading the same ARN back, the function ARN present in ListFunctionsByCodeSigningConfig), then calls DeleteFunctionCodeSigningConfig and polls — bounded at 60s each — for the function to leave FunctionArns and for GetFunctionCodeSigningConfig to stop reporting CodeSigningConfigArn. Neither converges and the check never completes: `Error: Check timed out after 60.0s`. The only waits in this check are those two post-DELETE convergence polls, so after the DELETE at least one side of the binding is still reported: ListFunctionsByCodeSigningConfig keeps returning the function ARN, and/or GetFunctionCodeSigningConfig keeps returning the CodeSigningConfigArn. The DELETE itself is not refused (no error code is reported), so it is accepted and silently has no effect on the stored association.
Why: a client cannot detach a code-signing config. Every teardown and re-binding flow breaks: DeleteCodeSigningConfig is expected to be refused while functions are still attached, so a config whose members never drain can never be deleted, and a function can never be moved to a different config or returned to unsigned deployment. Worse, the membership query that security tooling uses to audit "which functions are under this signing config" reports functions that are no longer governed by it, which is a false positive in exactly the direction that hides risk once the real enforcement path is fixed.
Fix: the DeleteFunctionCodeSigningConfig handler and the code-signing association state in the lambda service — the DELETE must remove the binding from both indexes (the function's attached-config field that GetFunctionCodeSigningConfig serializes, and the per-config function membership that ListFunctionsByCodeSigningConfig pages over) and answer 204; GetFunctionCodeSigningConfig must then still answer 200 for the function with CodeSigningConfigArn absent from the body rather than echoing the detached ARN.
Related but not covering: the open issue 20261026T070343Z-lambda-code-signing-is-not-enforced-updatefunctioncode-and.md is about ENFORCEMENT (UpdateFunctionCode/UpdateFunctionConfiguration ignoring an Enforce policy, and Signer writing no signed object) and carries different check ids; it says nothing about the association lifecycle or the two read paths above.

Traces: lambda.function-codesigning-association-lifecycle (Cause 1).

## Evidence
Cause 1 of 1 — lambda.function-codesigning-association-lifecycle
Expected (real AWS): with a function <fn> and a code-signing config <csc-arn> created and attached, (a) ListFunctionsByCodeSigningConfig(CodeSigningConfigArn=<csc-arn>) includes <fn>'s ARN in FunctionArns and (b) GetFunctionCodeSigningConfig(FunctionName=<fn>) returns 200 with CodeSigningConfigArn == <csc-arn>. After DeleteFunctionCodeSigningConfig(FunctionName=<fn>) returns HTTP 204 with an empty body, (a) becomes: <fn>'s ARN is absent from FunctionArns for <csc-arn> (paginating the full Marker/NextMarker sequence), and (b) becomes: GetFunctionCodeSigningConfig(FunctionName=<fn>) still returns HTTP 200 and the response contains no CodeSigningConfigArn member — it is NOT an error (no ResourceNotFoundException, no CodeSigningConfigNotFoundException) and NOT the old ARN.
Actual (emulator): the pre-DELETE state is reached normally (CreateCodeSigningConfig, PutFunctionCodeSigningConfig and the GetFunctionCodeSigningConfig read-back all succeed and the function is listed), but after DeleteFunctionCodeSigningConfig the state never changes. The check's two bounded (60s) convergence polls both keep observing the stale association and the whole check is terminated by the harness: `Error: Check timed out after 60.0s`. The assertions those polls guard are the exact divergences: "function still present in ListFunctionsByCodeSigningConfig after dissociation" and "function still reports a CodeSigningConfigArn after dissociation". No error code or status is returned by the DELETE — it is accepted and inert.

## Steps to Reproduce
1. Boot the emulator and replay lambda.function-codesigning-association-lifecycle.
2. Create an execution role and a basic python3.12 function <fn>; wait for State=Active and keep its ARN.
3. lambda:CreateCodeSigningConfig(AllowedPublishers={'SigningProfileVersionArns':['arn:aws:signer:<region>:<account>:/signing-profiles/SimRealAssocProfile/1']}) and keep CodeSigningConfigArn <csc>.
4. lambda:PutFunctionCodeSigningConfig(CodeSigningConfigArn=<csc>, FunctionName=<fn>).
5. lambda:GetFunctionCodeSigningConfig(FunctionName=<fn>) -> CodeSigningConfigArn == <csc>; lambda:ListFunctionsByCodeSigningConfig(CodeSigningConfigArn=<csc>) (paginate Marker/NextMarker) -> contains <fn>'s ARN. (Both hold on the emulator.)
6. lambda:DeleteFunctionCodeSigningConfig(FunctionName=<fn>) -> observe it is accepted.
7. Re-run step 5's ListFunctionsByCodeSigningConfig, polling for up to 60s -> observe <fn>'s ARN is STILL returned, instead of being absent.
8. Re-run step 5's GetFunctionCodeSigningConfig, polling for up to 60s -> observe the response STILL carries CodeSigningConfigArn == <csc>, instead of a 200 whose body omits CodeSigningConfigArn.
