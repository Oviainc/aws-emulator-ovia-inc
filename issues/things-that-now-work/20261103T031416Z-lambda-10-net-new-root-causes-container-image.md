Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.arm64-image-function-runtime-reports-aarch64 lambda.image-runtime-honors-entrypoint-cmd-workdir-and-merges-env lambda.cli-lambda-invoke-nonexistent-error lambda.container-image-layers-rejected lambda.container-image-packagetype-immutable-on-update-rejected lambda.container-image-uri-source-validation-rejected lambda.image-function-runs-or-refuses-honestly lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied lambda.durable-execution-kms-enforcement-rejected lambda.e2e-sqs-trigger lambda.event-source-mapping-concurrent-update-conflict lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted lambda.put-function-scaling-config-invalid-values-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.arm64-image-function-runtime-reports-aarch64|run#18|AssertionError|assert resolved.endswith(f"@{digest}"), (
Issue-Key: lambda.image-runtime-honors-entrypoint-cmd-workdir-and-merges-env|run#18|AssertionError|assert resolved == f"{repo_uri}@{digest}", (
Issue-Key: lambda.cli-lambda-invoke-nonexistent-error|_run_shell#3|AssertionError|raise AssertionError(
Issue-Key: lambda.container-image-layers-rejected|run#14|AssertionError|raise AssertionError("expected CreateFunction to be refused, but it succeeded")
Issue-Key: lambda.container-image-packagetype-immutable-on-update-rejected|run#17|AssertionError|assert err.get("Message") == "Please don't provide ImageUri when updating a function with packageType Zip.", (
Issue-Key: lambda.container-image-uri-source-validation-rejected|_refused_error#12|AssertionError|raise AssertionError("expected CreateFunction to be refused, but it succeeded")
Issue-Key: lambda.image-function-runs-or-refuses-honestly|run#21|AssertionError|raise AssertionError(
Issue-Key: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied|run#12|AssertionError|assert status == 200 and json.loads(body).get("nonce") == "granted", (
Issue-Key: lambda.durable-execution-kms-enforcement-rejected|run#18|Timeout|time.sleep(2)
Issue-Key: lambda.e2e-sqs-trigger|run#17|Timeout|time.sleep(2)
Issue-Key: lambda.event-source-mapping-concurrent-update-conflict|run#13|AssertionError|assert update_msg == "Cannot update the event source mapping because it is in use.", update_msg
Issue-Key: lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted|run#10|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: lambda.put-function-scaling-config-invalid-values-rejected|run#15|AssertionError|assert message == (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# lambda: 10 net-new root causes — container-image source/Layers/PackageType validation missing or misworded, ResolvedImageUri reports the wrong digest, Invoke answers NotImplemented from the CLI, function URLs are unrouted, a 413 is returned bare, and two checks hang

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Ten distinct root causes under the suite heading `lambda`. A fixer must close all ten before submitting.

Cause 1 of 10: `GetFunction().Code.ResolvedImageUri` reports a digest that is not the one PutImage returned for the pushed manifest.
AWS: for a `PackageType=Image` function, `Code.ResolvedImageUri` is `<repo-uri>@<digest>` where `<digest>` is the manifest digest ECR returned from PutImage for the tag the function names — the same value `DescribeImages`/`BatchGetImage` report.
Emulator: ResolvedImageUri ends with a different `sha256:` digest than the one PutImage returned for the same manifest (two independent checks, two different images).
Why: ResolvedImageUri is how a client pins and audits exactly which image bytes a function runs; a digest the registry never minted makes that audit wrong and breaks any comparison against ECR.
Fix: the lambda image-resolution path that records the manifest digest at Create/UpdateFunctionCode (it must use ECR's returned manifest digest, not a digest computed over something else) and the GetFunction serializer that emits it.
Traces: lambda.arm64-image-function-runtime-reports-aarch64, lambda.image-runtime-honors-entrypoint-cmd-workdir-and-merges-env

Cause 2 of 10: Invoke refuses a real invocation with an unmodelled `NotImplemented`.
AWS: Smithy: lambda `Invoke` returns 200 with the handler's `Payload`; `NotImplemented` is not among its modelled errors.
Emulator: `An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` (aws CLI rc=254). The check could not even reach the negative case it was written for.
Fix: the lambda Invoke handler / in-process runtime. Related open issue `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` describes the same refusal but does not carry this check's replay key.
Why: a client gets an error code no SDK models, so no retry/handling path applies.
Traces: lambda.cli-lambda-invoke-nonexistent-error

Cause 3 of 10: CreateFunction accepts `Layers` together with `PackageType=Image`.
AWS: layers are not supported for container-image functions; CreateFunction is refused with HTTP 400 `InvalidParameterValueException`, message `Lambda layers are not supported for functions created with container images.` (fixed text, no variable parts), and nothing is created — GetFunction then answers `ResourceNotFoundException` / `Function not found: arn:<partition>:lambda:<region>:<account>:function:<name>`.
Emulator: the create SUCCEEDS (`AssertionError: expected CreateFunction to be refused, but it succeeded`).
Why: a client gets a function whose declared layers can never be mounted, with no error at the call AWS refuses.
Fix: CreateFunction input validation — reject Layers when PackageType is Image.
Traces: lambda.container-image-layers-rejected

Cause 4 of 10: the PackageType-immutability refusal on UpdateFunctionCode is worded differently from AWS.
AWS: supplying `ImageUri` when updating a function whose PackageType is Zip is refused with `InvalidParameterValueException`, message `Please don't provide ImageUri when updating a function with packageType Zip.` (fixed text).
Emulator: correct refusal, wrong message — `ImageUri may only name the code of a function whose PackageType is Image`.
Why: the message is a requirement; a client matching AWS's wording does not recognise the refusal.
Fix: the UpdateFunctionCode validation message for the Zip/Image mismatch.
Traces: lambda.container-image-packagetype-immutable-on-update-rejected

Cause 5 of 10: CreateFunction performs no validation of the `ImageUri` source — a cross-Region URI, a tag absent from an existing repository, and a URI naming no image at all are all accepted.
AWS: Lambda resolves the image at create time. A cross-Region URI is refused with `InvalidParameterValueException` / `Source image <uri> is not valid. Provide a valid source image.`; an existing repository with a tag that was never pushed is refused with `InvalidParameterValueException` / `Source image <uri> does not exist. Provide a valid source image.` (in both, the fixed text surrounds the `<uri>` taken from the request). Neither function is created. The positive control — the same repository/Region with the pushed tag — succeeds and reports `Code.ImageUri` equal to the URI given.
Emulator: all of these creates SUCCEED (`AssertionError: expected CreateFunction to be refused, but it succeeded`; `CreateFunction with an ImageUri naming no image succeeded; it must be refused, since there is no image for the function to ever run`).
Why: a client typo'ing a tag or a Region gets a function that can never start, and the emulator is over-permissive on the only check that distinguishes a real image from a string.
Fix: CreateFunction's image-source resolution — resolve the ImageUri against ECR in the function's Region and emit the two distinct InvalidParameterValueException messages, creating nothing.
Traces: lambda.container-image-uri-source-validation-rejected, lambda.image-function-runs-or-refuses-honestly

Cause 6 of 10: a Lambda function URL host is not routed — the data plane answers 404 `UnknownService`.
AWS: a function URL with AuthType NONE and a `lambda:InvokeFunctionUrl` grant to `*` serves the handler's response (200 with the handler's body) for an anonymous GET.
Emulator: `404 {"__type":"UnknownService","message":"no emulated service is registered for '<function-url-id>'"}` — the function-url host is not registered as a route at all, so the authorization behaviour under test cannot be observed.
Why: every function-URL client gets a 404 with an emulator-internal error type instead of the handler's response, and an auth-denied case is indistinguishable from an unrouted host.
Fix: the lambda function-URL data-plane router (register the URL id as a host/route and dispatch to the function), then the AuthType/resource-policy evaluation on it. Related open issue `20261029T211043Z-lambda-data-plane-function-url-endpoints-are-unrouted.md` describes the same unrouted data plane but does not carry this check's replay key.
Traces: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied

Cause 7 of 10: two checks never complete — the emulator neither answers nor refuses.
AWS: a durable execution with a KMS key that cannot be used is refused at the point of use, and an SQS-triggered function is invoked by its event source mapping; in both cases the call under test terminates.
Emulator: both checks report `Error: Check timed out after 60.0s`, producing no status, error code or message.
Why: a hang leaves a client with no result and no error branch; it is strictly worse than a wrong answer.
Fix: the lambda durable-execution KMS enforcement path and the SQS event-source-mapping poller/dispatcher — each must terminate with a response or a modelled error.
Traces: lambda.durable-execution-kms-enforcement-rejected, lambda.e2e-sqs-trigger

Cause 8 of 10: the in-use refusal on UpdateEventSourceMapping is worded differently from AWS.
AWS: Smithy: UpdateEventSourceMapping models `ResourceInUseException` (400); updating a mapping that is not yet settled is refused with message `Cannot update the event source mapping because it is in use.` (fixed text).
Emulator: the message is `The operation cannot be performed at this time. The event source mapping <uuid> is currently in the following state: Creating` — different wording, and it leaks the mapping's internal state value into a message AWS keeps constant.
Why: a client matching AWS's wording to decide whether to retry does not recognise the refusal.
Fix: the UpdateEventSourceMapping conflict path's message.
Traces: lambda.event-source-mapping-concurrent-update-conflict

Cause 9 of 10: an over-size direct-upload PublishLayerVersion returns a bare HTTP 413 with an empty body instead of a modelled error.
AWS: Smithy: PublishLayerVersion models `InvalidParameterValueException`, `CodeStorageExceededException`, `ResourceNotFoundException`, `ServiceException`, `TooManyRequestsException`; a request body over the direct-upload limit is answered with the `RequestEntityTooLargeException` error code (HTTP 413) carrying that `__type` and a message, so botocore raises a ClientError whose Code is `RequestEntityTooLargeException`.
Emulator: HTTP 413 with an EMPTY body — no `__type`, no message. Replay detail: `expected a ...-byte direct-upload PublishLayerVersion request to be rejected with RequestEntityTooLargeException, got 413: ''`.
Why: an SDK cannot classify the failure (no error code), so client code that branches on `RequestEntityTooLargeException` sees an unparsable response instead.
Fix: the lambda request-size guard / error serializer — emit the RequestEntityTooLargeException body with the 413.
Traces: lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted

Cause 10 of 10: PutFunctionScalingConfig's invalid-range refusal is worded as a generic validation error instead of AWS's sentence.
AWS: Smithy: PutFunctionScalingConfig models `InvalidParameterValueException` (400). With `MinExecutionEnvironments` greater than `MaxExecutionEnvironments` it answers that code with message `Invalid function scaling configuration. Min execution environments should be lower or equal than Max execution environments.` (fixed text, no variable parts), and the stored configuration is unchanged.
Emulator: `1 validation error detected: Value '5' at 'functionScalingConfig.minExecutionEnvironments' failed to satisfy constraint: Member must be less than or equal to the value '2' at 'functionScalingConfig.maxExecutionEnvironments'` — a synthesised constraint-validation envelope AWS does not use here, with the request's values embedded.
Why: the message is a requirement; a client matching AWS's wording does not recognise the refusal.
Fix: the PutFunctionScalingConfig validation message for the min>max relation.
Traces: lambda.put-function-scaling-config-invalid-values-rejected

## Evidence
Cause 1 — expected `Code.ResolvedImageUri` to end with the digest PutImage returned for the pushed manifest. Actual: `...@sha256:f7ec765f...` where PutImage returned `sha256:23947c8f...` (and `...@sha256:c2708918...` vs `sha256:e722f2f7...` in the second check).
Cause 2 — expected Invoke 200 + Payload. Actual: `NotImplemented` / `Invoke resolved <arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator...`.
Cause 3 — expected ('InvalidParameterValueException', 'Lambda layers are not supported for functions created with container images.') and the function absent. Actual: CreateFunction succeeded.
Cause 4 — expected message `Please don't provide ImageUri when updating a function with packageType Zip.`; actual `ImageUri may only name the code of a function whose PackageType is Image`.
Cause 5 — expected ('InvalidParameterValueException', 'Source image <uri> is not valid. Provide a valid source image.') for a cross-Region URI and ('InvalidParameterValueException', 'Source image <uri> does not exist. Provide a valid source image.') for an unpushed tag, plus a refusal for an ImageUri naming no image; actual: all three creates succeeded.
Cause 6 — expected 200 with the handler's body for an anonymous GET of the function URL; actual `404 {"__type":"UnknownService","message":"no emulated service is registered for '<id>'"}`.
Cause 7 — expected either a response or a modelled error; actual `Error: Check timed out after 60.0s` on both checks.
Cause 8 — expected message `Cannot update the event source mapping because it is in use.`; actual `The operation cannot be performed at this time. The event source mapping <uuid> is currently in the following state: Creating`.
Cause 9 — expected a 413 carrying error code `RequestEntityTooLargeException`; actual `413: ''` (empty body, no error code).
Cause 10 — expected ('InvalidParameterValueException', 'Invalid function scaling configuration. Min execution environments should be lower or equal than Max execution environments.'); actual message `1 validation error detected: Value '5' at 'functionScalingConfig.minExecutionEnvironments' failed to satisfy constraint: Member must be less than or equal to the value '2' at 'functionScalingConfig.maxExecutionEnvironments'`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run every check id in trace_ids.
2. Cause 1: create an ECR repo, PutImage a manifest, note the returned digest, CreateFunction with PackageType=Image on that tag, then GetFunction and compare `Code.ResolvedImageUri`.
3. Cause 3: CreateFunction with PackageType=Image and a non-empty `Layers` list.
4. Cause 4: create a Zip function, then UpdateFunctionCode passing `ImageUri`.
5. Cause 5: CreateFunction with (a) an ImageUri whose Region differs from the function's, (b) an existing repo with a tag never pushed, (c) an ImageUri naming no image.
6. Cause 6: CreateFunctionUrlConfig(AuthType='NONE') + AddPermission(Principal='*', Action='lambda:InvokeFunctionUrl'), then anonymous GET of the URL.
7. Cause 8: CreateEventSourceMapping then immediately UpdateEventSourceMapping on the same UUID.
8. Cause 9: PublishLayerVersion with a ~52 MB inline ZipFile and inspect the response body.
9. Cause 10: on a capacity-provider function version, PutFunctionScalingConfig with MinExecutionEnvironments=5, MaxExecutionEnvironments=2.
