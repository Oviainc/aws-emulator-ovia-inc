Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-web-resource-policy-and-tag-lifecycle
Issue-Step: teardown#20
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-web-resource-policy-and-tag-lifecycle lambda-web-resource-policy-and-tags-isolated-across-functions
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web|DeleteWebFunction|2xx->4xx|status
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# lambda-web DeleteWebFunction answers 404 ResourceNotFoundException where the teardown delete recorded 204 No Content

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebFunction

## Description
Cause 1 of 1: `DeleteWebFunction` answers HTTP 404 `ResourceNotFoundException` for the teardown delete of a web function, where the recorded AWS response for the same request is HTTP 204 with an empty body. The same single cause shows up on three replayed steps in two fixtures (the lifecycle fixture's final teardown delete, and both teardown deletes of the cross-function isolation fixture).

AWS: Smithy (services/service-com-amazonaws-lambdaweb-lambdaweb/smithy-model/com-amazonaws-lambdaweb-lambdaweb.json), service LambdaWeb (sdkId "Lambda Web", restJson1). `DeleteWebFunction` is `DELETE /2025-03-07/web-functions/{functionName}`, output `smithy.api#Unit`, modelled success code **204**, and the operation carries the `@idempotent` trait. Its input is the single required `@httpLabel` member `functionName` (target `FunctionName`, @length 1..256), documented as accepting either the bare function name or the function ARN. Its modelled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400); `ResourceNotFoundException` is documented as "The specified web function, revision, endpoint, or resource-based policy doesn't exist. Verify the resource identifier and try again." and its `message` member is required. Beyond the model this is recorded behavior only: for the teardown delete of each fixture's web function the recorded response is 204 with no body — i.e. the recorded service completes the delete of the function named in the URI label rather than reporting it absent. Hypothesis: the recording exercises the `@idempotent` contract — a delete of a function name that is already gone (a repeat/retry of the destructive call, which is exactly what `@idempotent` makes safe) still settles as 204 — and/or the function is still present in the recorded account at teardown because the recorded setup steps that this emulator refuses (see below) did create it. Either way, the requirement the recording states is the status: 204 with an empty body, no error shape.

Emulator: the handler resolves the label to a store key and, when `state.functions` holds no record under it, returns the modelled 404 instead of settling the delete: HTTP 404, error code `ResourceNotFoundException`, message `The web function <functionName> doesn't exist.` with `resourceId` = the request's `functionName` and `resourceType` = `WebFunction`. In that message the fixed text is "The web function " ... " doesn't exist."; the only variable part is `<functionName>`, taken verbatim from the request's URI label (the emulator echoes the caller's spelling, so an ARN-form input is echoed as the ARN). The module's own header comment states this as a deliberate choice ("`@idempotent` is about retry safety, not about inventing success for a name that never existed, so an unknown name is a 404"), which is what the recorded corpus contradicts for this request class. Note that these same two fixtures also exercise operations this emulator refuses with the scaffold's 501 `NotImplemented` channel — the already-open issue `20261104T160229Z-lambda-web-createwebfunction-refuses-any-request-carrying-endpointconfig.md` (CreateWebFunction refusing any request carrying `endpointConfig`) is one of them, and the remaining unimplemented-operation steps of these fixtures are filed by the sweep as stub issues. Neither of those issues carries this step's replay key, and neither covers the delete's status/error contract, so this divergence is filed on its own: if the store ends up without the record the teardown names, the delete must still settle the way the recording shows rather than reporting the resource absent.

Why: a client that deletes a web function and retries the call (an SDK retry after a dropped response, a Terraform/CloudFormation-style converging destroy, or any idempotent cleanup) gets a hard `ResourceNotFoundException` instead of the recorded 204. Because `ResourceNotFoundException` is a modelled, non-retryable client error, SDK retry classifiers surface it to the caller, so a cleanup path that is correct against AWS fails against the emulator and teardown aborts — leaving every test or orchestration built on "delete then confirm" wedged, and making the emulator unusable for the end of any resource lifecycle even when the earlier steps of that lifecycle succeed.

Fix: belongs in the `DeleteWebFunction` handler's not-found branch and in the function slice of the service state it reads. The handler must settle a delete of a name that holds no stored record the way the corpus records it — 204 with an empty body, no error shape — rather than raising `ResourceNotFoundException`, so the operation's modelled `@idempotent` trait and its 204 success code hold on a repeat call; and the create path must persist the function record (and the records cascaded under it) so the teardown delete of a function this emulator did create resolves to that record by either the bare name or the function ARN.

Traces: lambda-web-resource-policy-and-tag-lifecycle#teardown#20, lambda-web-resource-policy-and-tags-isolated-across-functions#teardown#19, lambda-web-resource-policy-and-tags-isolated-across-functions#teardown#20. File and line citations: services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/delete_web_function.rs lines 191-210 (the `state.functions.remove(name)` else-branch that returns `ResourceNotFoundException`) and lines 5-7 (the comment stating the choice), with the store and the name/ARN resolution at services/service-com-amazonaws-lambdaweb-lambdaweb/src/state.rs lines 297 and 417-424.

## Evidence
Expected (recorded AWS): DELETE /2025-03-07/web-functions/{functionName} -> HTTP 204, empty body, no error shape.

Actual (emulator): HTTP 404 with error code `ResourceNotFoundException` and message `The web function <functionName> doesn't exist.` (fixed text "The web function " / " doesn't exist."; variable part is the `functionName` URI label copied from the request), plus `resourceId` = that same functionName and `resourceType` = "WebFunction".

Replay verdict: mismatch, divergence_key `service=lambda-web|DeleteWebFunction|status=204->404|keys=status`, detail "status: expected 204, got 404", 3 affected steps:
- lambda-web-resource-policy-and-tag-lifecycle#teardown#20 (representative)
- lambda-web-resource-policy-and-tags-isolated-across-functions#teardown#19
- lambda-web-resource-policy-and-tags-isolated-across-functions#teardown#20

Model evidence: DeleteWebFunction http {method: DELETE, uri: /2025-03-07/web-functions/{functionName}, code: 204}, `idempotent: true`, output shape `Unit`.

Source: services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/delete_web_function.rs:191-210 returns DeleteWebFunctionErrorModeled::ResourceNotFoundException whenever `state.functions.remove(resolve_function_name(&input.function_name))` yields None; SUCCESS_STATUS is 204 (line 171), so the success path's status itself is correct — only the absent-record branch diverges.

## Steps to Reproduce
1. Boot the emulator and point an SDK/botocore lambda-web client at it.
2. Replay the fixture lambda-web-resource-policy-and-tag-lifecycle and observe step teardown#20: the DELETE of the fixture's web function answers 404 ResourceNotFoundException where the recording answers 204.
3. Minimal reproduction of the same cause without the fixture: call CreateWebFunction for a fresh functionName without `endpointConfig` (so the create is accepted), then call DeleteWebFunction with that functionName -> 204 as expected.
4. Call DeleteWebFunction a second time with the same functionName (the retry the operation's `@idempotent` trait makes safe): the emulator answers HTTP 404, error code ResourceNotFoundException, message "The web function <functionName> doesn't exist.", instead of the recorded 204 with an empty body.
5. Same result when the function name is spelled as its ARN (arn:aws:lambda:<region>:<account>:web-function/<functionName>): the 404 message echoes the ARN, confirming the branch is reached after name/ARN resolution.
6. Replay lambda-web-resource-policy-and-tags-isolated-across-functions and observe teardown#19 and teardown#20 diverging identically.
