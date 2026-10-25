Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.delete-alias-nonexistent-is-a-no-op lambda.delete-function-url lambda.delete-resource-in-use-blocked lambda.event-source-mapping-concurrent-update-conflict lambda.get-function-scaling-config-invalid-qualifier-rejected lambda.get-function-scaling-config-nonexistent-function-rejected lambda.invoke-with-response-stream-ec2-errors lambda.put-resource-policy-public-document-round-trips lambda.update-alias-concurrent-conflicting-writes lambda.update-capacity-provider-concurrent-conflicting-writes lambda.update-capacity-provider-without-permission-denied lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment lambda.stream-response-chunks-map-onto-handler-writes
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.delete-alias-nonexistent-is-a-no-op|run#7|AssertionError|assert error_code is None, (
Issue-Key: lambda.delete-function-url|run#12|AssertionError|assert mutate_code == "ResourceNotFoundException", (
Issue-Key: lambda.delete-resource-in-use-blocked|run#13|AssertionError|assert alias_delete_code in conflict_codes, (
Issue-Key: lambda.event-source-mapping-concurrent-update-conflict|run#12|AssertionError|assert update_code in conflict_codes, f"expected one of {conflict_codes}, got {update_code}"
Issue-Key: lambda.get-function-scaling-config-invalid-qualifier-rejected|run#5|AssertionError|assert existing_code == "AccessDeniedException", existing_code
Issue-Key: lambda.get-function-scaling-config-nonexistent-function-rejected|run#5|AssertionError|assert existing_code == "AccessDeniedException", (
Issue-Key: lambda.invoke-with-response-stream-ec2-errors|run#6|AssertionError|assert missing_code == "ResourceNotFoundException", (
Issue-Key: lambda.put-resource-policy-public-document-round-trips|run#6|AssertionError|assert error_code is None, (
Issue-Key: lambda.update-alias-concurrent-conflicting-writes|run#13|AssertionError|assert len(oks) == 1, f"expected exactly one winner among racers, got {len(oks)}: {results}"
Issue-Key: lambda.update-capacity-provider-concurrent-conflicting-writes|run#10|AssertionError|assert outcome is not None, (
Issue-Key: lambda.update-capacity-provider-without-permission-denied|run#10|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: lambda|InvokeWithResponseStream|NotImplemented
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda: ten operations answer NotImplemented where AWS answers a success or a modeled refusal

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAlias

## Description
Ten net-new root causes of one family: an operation (or one of its paths) is a scaffold stub, so the response is the non-modeled code 'NotImplemented' where AWS returns a success or a specific modeled error. In every case the check was asserting the refusal taxonomy, so the stub is visible to clients as a wrong error code, not merely as a missing feature.

Cause 1 of 10 — DeleteAlias of a name that was never created.
AWS: Smithy (lambda) DeleteAlias is @idempotent, HTTP 204, and deleting an absent alias is a no-op success. Emulator: 'NotImplemented'. Why: idempotent cleanup code cannot run. Fix: DeleteAlias handler. Traces: lambda.delete-alias-nonexistent-is-a-no-op.

Cause 2 of 10 — mutating a deleted function-URL config.
AWS: ResourceNotFoundException (404) for a URL config that no longer exists. Emulator: 'NotImplemented'. Why: the client cannot tell a deleted config from an unavailable API. Fix: the function-URL config handlers (delete/update) in the lambda service. Traces: lambda.delete-function-url.

Cause 3 of 10 — deleting an alias that has an attached provisioned-concurrency config.
AWS: refused ResourceConflictException/ResourceInUseException (409) until the config is removed. Emulator: 'NotImplemented'. Why: the in-use protection is invisible, and a client cannot learn what to delete first. Fix: DeleteAlias + provisioned-concurrency state coupling. Traces: lambda.delete-resource-in-use-blocked.

Cause 4 of 10 — concurrent UpdateEventSourceMapping.
AWS: one of the racing updates is refused ResourceInUseException/ResourceConflictException (409). Emulator: 'NotImplemented'. Why: no conflict signal, so a client cannot retry correctly. Fix: UpdateEventSourceMapping handler (its stub is also filed separately as 'Implement lambda UpdateEventSourceMapping'), including the conflict path. Traces: lambda.event-source-mapping-concurrent-update-conflict.

Cause 5 of 10 — GetFunctionScalingConfig refusals.
AWS: an invalid qualifier is refused InvalidParameterValueException (400) and a resolvable function with no scaling config is refused AccessDeniedException. Emulator: 'NotImplemented' for both. Why: two distinct client-side conditions collapse into one non-modeled code. Fix: GetFunctionScalingConfig handler. Traces: lambda.get-function-scaling-config-invalid-qualifier-rejected, lambda.get-function-scaling-config-nonexistent-function-rejected.

Cause 6 of 10 — InvokeWithResponseStream for a nonexistent function.
AWS: Smithy (lambda) InvokeWithResponseStream models ResourceNotFoundException (404). Emulator: 'NotImplemented'. Why: a typo in the function name is reported as a platform gap. Fix: function resolution in the InvokeWithResponseStream handler. Traces: lambda.invoke-with-response-stream-ec2-errors.

Cause 7 of 10 — PutResourcePolicy with an unconditioned public invoke grant.
AWS: PutResourcePolicy accepts it (Lambda has no public-policy guard on this API) and the document round-trips through GetResourcePolicy. Emulator: 'NotImplemented'. Why: resource-policy-based access control cannot be configured at all. Fix: PutResourcePolicy/GetResourcePolicy handlers (the stub is also filed separately as 'Implement lambda PutResourcePolicy'); this check asserts the accept-and-round-trip behaviour that issue does not list. Traces: lambda.put-resource-policy-public-document-round-trips.

Cause 8 of 10 — concurrent UpdateAlias writes.
AWS: exactly one of two conflicting UpdateAlias calls wins; the loser is refused with a conflict/precondition error. Emulator: both answer 'NotImplemented', so there is no winner. Why: alias retargeting — the basis of canary deploys — cannot be performed or raced safely. Fix: UpdateAlias handler (stub also filed separately) plus revision-id conflict handling. Traces: lambda.update-alias-concurrent-conflicting-writes.

Cause 9 of 10 — UpdateCapacityProvider concurrency and authorization.
AWS: concurrent UpdateCapacityProvider calls both succeed (applied asynchronously, no synchronous conflict detection), and a caller under an explicit deny is refused AccessDeniedException. Emulator: 'NotImplemented' in both cases. Why: neither the success semantics nor the authorization refusal is observable. Fix: UpdateCapacityProvider handler (stub also filed separately) and its IAM authorization. Traces: lambda.update-capacity-provider-concurrent-conflicting-writes, lambda.update-capacity-provider-without-permission-denied.

Cause 10 of 10 — response streaming is entirely unimplemented.
AWS: Smithy (lambda) InvokeWithResponseStream is POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations, HTTP 200, with a streaming EventStream payload (PayloadChunk / InvokeComplete, the latter carrying ErrorCode/ErrorDetails in the trailer). Emulator: every call is refused 'NotImplemented' with 'operation POST /2021-11-15/functions/<fn>/response-streaming-invocations is not implemented in this emulator' (recorded message, an example). Why: no client can stream a response, observe first-chunk-before-completion behaviour, read a mid-stream error from the trailer, or see a provisioned environment serve a stream. Fix: implement the InvokeWithResponseStream route and its event-stream serializer in the lambda service. Traces: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer, lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes, lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code, lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment, lambda.stream-response-chunks-map-onto-handler-writes.

## Evidence
Cause 1: 'expected DeleteAlias of a never-created name to succeed as a no-op, got NotImplemented'.
Cause 2: 'expected ResourceNotFoundException mutating deleted config, got NotImplemented'.
Cause 3: "expected one of {'ResourceConflictException','ResourceInUseException'} deleting an alias with an attached provisioned-concurrency config, got NotImplemented".
Cause 4: "expected one of {'ResourceInUseException','ResourceConflictException'}, got NotImplemented".
Cause 5: 'AssertionError: NotImplemented' and 'expected AccessDeniedException for a resolvable function with no capacity-provider scaling config, got NotImplemented'.
Cause 6: 'expected ResourceNotFoundException for a nonexistent function, got NotImplemented'.
Cause 7: 'expected PutResourcePolicy to accept an unconditioned public invoke grant, got NotImplemented'.
Cause 8: "expected exactly one winner among racers, got 0: [('err','NotImplemented'),('err','NotImplemented')]".
Cause 9: "expected both concurrent UpdateCapacityProvider calls to succeed ... [{'a': ('error','NotImplemented'), 'b': ('error','NotImplemented')}, ...]" and "explicit-deny caller must be rejected with AccessDeniedException, got code='NotImplemented'".
Cause 10: ClientError (NotImplemented) when calling InvokeWithResponseStream: 'operation POST /2021-11-15/functions/<fn>/response-streaming-invocations is not implemented in this emulator' (5 checks).

## Steps to Reproduce
1. DeleteAlias(FunctionName=fn, Name='never-created').
2. CreateFunctionUrlConfig, DeleteFunctionUrlConfig, then UpdateFunctionUrlConfig on the deleted config.
3. Attach a provisioned-concurrency config to an alias and DeleteAlias it.
4. Issue two concurrent UpdateEventSourceMapping calls for one UUID.
5. GetFunctionScalingConfig with an invalid qualifier, and for a function with no scaling config.
6. InvokeWithResponseStream for a function name that does not exist.
7. PutResourcePolicy with a public invoke statement, then GetResourcePolicy.
8. Issue two concurrent UpdateAlias calls for one alias.
9. Issue two concurrent UpdateCapacityProvider calls, and one under an explicit-deny policy.
10. InvokeWithResponseStream on an existing streaming function.
