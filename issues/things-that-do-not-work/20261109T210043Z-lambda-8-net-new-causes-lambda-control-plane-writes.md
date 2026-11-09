Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.account-settings-limits-drive-enforcement lambda.ephemeral-storage-size-bounds-rejected-and-accepted lambda.create-function-with-container-image-rejects-code-signing-config lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach lambda.invoke-without-tenant-id-on-tenant-isolated-function-rejected lambda.msk-esm-invalid-starting-position-rejected lambda.msk-esm-topics-list-too-long-rejected lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.account-settings-limits-drive-enforcement|run#13|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: lambda.ephemeral-storage-size-bounds-rejected-and-accepted|_error#25|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.create-function-with-container-image-rejects-code-signing-config|run#26|AssertionError|assert message == "Code signing is not supported for functions created with container images.", (
Issue-Key: lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach|_client_error#30|AssertionError|raise AssertionError(
Issue-Key: lambda.invoke-without-tenant-id-on-tenant-isolated-function-rejected|_error#4|AssertionError|raise AssertionError(f"expected the invoke to be refused, but it succeeded: {result!r}")
Issue-Key: lambda.msk-esm-invalid-starting-position-rejected|run#13|AssertionError|assert got == enum_refusal, f"StartingPosition={bad_position!r} was refused with {got!r}"
Issue-Key: lambda.msk-esm-topics-list-too-long-rejected|run#15|AssertionError|assert got == role_refusal, f"one topic was refused with {got!r}"
Issue-Key: lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update|_scenario#9|KeyError|assert created["VpcConfig"]["VpcId"] == vpc_id, (
Issue-Key: lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs|run#9|AssertionError|assert time.monotonic() < prime_deadline, (
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 8 net-new causes: Lambda control-plane writes skip documented validation and mis-shape their refusals

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Eight distinct root causes in Lambda's control plane and its refusal messages. They are separate work items; all eight must be closed.

Cause 1 of 8 — a deployment package over the account's CodeSizeZipped limit is answered with a bare HTTP 413 instead of a modelled error.
AWS: CreateFunction refuses a request entity larger than the account's CodeSizeZipped limit (as reported by GetAccountSettings) with RequestEntityTooLargeException and HTTP 413, carrying an error code and message a client can read; the Lambda Smithy model's CreateFunction errors (CodeStorageExceededException 400, InvalidParameterValueException 400, ResourceConflictException 409, ...) are all code-carrying JSON errors, never a bodyless status.
Emulator: answers a bare `413` with no error code the SDK can map, so the SDK surfaces the status rather than an exception type.
Why: a client cannot catch the over-size case by error type; it sees an unmodelled transport status and treats a deterministic validation refusal as an unknown failure.
Fix: CreateFunction's request-size validation and its error serializer — emit RequestEntityTooLargeException (413) with the error code and message in the body.
Traces: lambda.account-settings-limits-drive-enforcement.

Cause 2 of 8 — CreateFunction accepts an EphemeralStorage.Size outside the documented range.
AWS: Smithy EphemeralStorage.Size is required and documented as "any whole number between 512 and 10,240 MB"; a value outside that range is refused with InvalidParameterValueException (400) and no function is created.
Emulator: CreateFunction returns 201 and creates the function.
Why: a client's /tmp sizing is silently accepted at values AWS rejects, so configuration that cannot be deployed to AWS passes here and the function's reported EphemeralStorage is meaningless.
Fix: CreateFunction/UpdateFunctionConfiguration validation of EphemeralStorage.Size.
Traces: lambda.ephemeral-storage-size-bounds-rejected-and-accepted.

Cause 3 of 8 — CreateFunction with PackageType=Image plus CodeSigningConfigArn is refused for the wrong reason.
AWS: code signing applies only to .zip packages; CreateFunction with PackageType=Image and a CodeSigningConfigArn is refused with InvalidParameterValueException whose message says code signing is not supported for container-image functions. That refusal is made before any image is resolved, so the message never mentions the image.
Emulator: answers `Source image <image uri> does not exist. Provide a valid source image.` — it resolved the image first and reported an image problem instead of the code-signing conflict.
Why: a client is told to fix its image URI when the real defect is an unsupported combination of parameters; fixing the image still will not make the call succeed.
Fix: CreateFunction's parameter validation ordering — refuse the Image+CodeSigningConfigArn combination before resolving the image.
Traces: lambda.create-function-with-container-image-rejects-code-signing-config.

Cause 4 of 8 — an Enforce code-signing policy is not applied to a layer attach.
AWS: when a function's code-signing config has UntrustedArtifactOnDeployment=Enforce, attaching a layer whose version is unsigned is refused (InvalidCodeSignatureException / CodeVerificationFailedException, HTTP 400) and the function's Layers are unchanged.
Emulator: the call SUCCEEDS and the unsigned layer is attached.
Why: the whole point of Enforce is that unsigned artifacts cannot reach the function; a client's signing gate is bypassed and a supply-chain control silently does nothing.
Fix: the UpdateFunctionConfiguration layer-attach path — consult the attached code-signing config and verify each layer version's signature.
Traces: lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach.

Cause 5 of 8 — Invoke of a tenant-isolated function without a tenant id succeeds.
AWS: Smithy TenancyConfig.TenantIsolationMode is required on a function configured for tenant isolation; an Invoke that carries no tenant id is refused (HTTP 4xx with a code-carrying error) and the handler is not run.
Emulator: Invoke returns 200 with a payload.
Why: tenant isolation is a hard boundary. Accepting an untenanted invoke means a client's multi-tenant function runs with no tenant context, which is exactly the case the configuration exists to prevent.
Fix: the Invoke admission path — require and resolve the tenant id when the function's TenancyConfig requests isolation.
Traces: lambda.invoke-without-tenant-id-on-tenant-isolated-function-rejected.

Cause 6 of 8 — MSK event-source-mapping refusals use the wrong enum order and the wrong cluster-access wording.
AWS: (a) for an out-of-enum StartingPosition, CreateEventSourceMapping answers ValidationException `1 validation error detected: Value '<value>' at 'startingPosition' failed to satisfy constraint: Member must satisfy enum value set: [LATEST, AT_TIMESTAMP, TRIM_HORIZON]` — the bracketed list is AWS's fixed ordering, and the only variable part is the rejected value from the request; (b) when the execution role cannot describe the cluster, it answers InvalidParameterValueException `Cannot access cluster <cluster ARN>. Please ensure the role can perform the DescribeClusterV2 (recommended) or DescribeCluster and GetBootstrapBrokers actions on your cluster in IAM.` — fixed text with the cluster ARN from the request substituted.
Emulator: (a) renders the enum set in the Smithy declaration order `[TRIM_HORIZON, LATEST, AT_TIMESTAMP]`; (b) answers `The function execution role does not have permissions to call DescribeClusterV2 on MSK`, which names no cluster and is not AWS's sentence. (A similar wording problem is already filed for a sibling check in 20261106T225443Z-lambda-6-net-new-causes-layer-permission-preconditions.md; it does not carry these checks.)
Why: a client that asserts on or displays the refusal text gets a message AWS never emits, and the cluster-access refusal loses the ARN that tells the operator which cluster to grant.
Fix: CreateEventSourceMapping's validation messages — use AWS's fixed enum ordering for EventSourcePosition, and AWS's cluster-access sentence with the request's cluster ARN.
Traces: lambda.msk-esm-invalid-starting-position-rejected, lambda.msk-esm-topics-list-too-long-rejected.

Cause 7 of 8 — a function in a VPC-pending state exposes no VpcId.
AWS: once a function is created with VpcConfig, GetFunctionConfiguration's VpcConfig response carries VpcId along with SubnetIds and SecurityGroupIds, including while the ENI work is still pending, so a client can tell which VPC the function is attaching to.
Emulator: the VpcConfig response has no VpcId at all (the read raises KeyError 'VpcId'), so the pending-state behavior (invoke and update admission while the attach is in flight) cannot be observed. (20261108T101433Z-lambda-2-net-new-causes-an-invoke-inside.md carries a sibling check with the same symptom; it does not carry this one.)
Why: a client that resolves the function's VPC from its configuration — to check routing, or to wait for the attach — has nothing to read.
Fix: the VpcConfigResponse serializer and the function state that records the VPC resolved from the configured subnets.
Traces: lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update.

Cause 8 of 8 — a handler's own in-function control-plane call never completes before the deadline.
AWS: a handler that calls Lambda's control plane against its own function (here TagResource) completes well inside a 90 s budget; the task-timeout path is then exercised separately with a deliberately short Timeout.
Emulator: the control invoke never completes — the handler is killed with `Sandbox.Timedout` / `Task timed out after 8.00 seconds`, so the timeout-overrun behavior under test is never reached.
Why: any handler that calls back into Lambda about itself (tagging, concurrency, configuration reads) hangs until it is killed, which a client reads as its own code being slow rather than the in-sandbox SDK path stalling.
Fix: the in-sandbox AWS SDK path for Lambda control-plane calls from a running handler (its endpoint resolution/dispatch back into the emulator).
Traces: lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs.

## Evidence
Cause 1: AssertionError: expected CreateFunction to reject a 58720514-byte deployment package, which exceeds the account's reported CodeSizeZipped limit of 52428800 bytes, with RequestEntityTooLargeException, got '413'.
Cause 2: AssertionError: expected the call to be refused, but it succeeded: HTTPStatusCode 201, FunctionName '<fn>' (EphemeralStorage out of the documented 512..10240 range).
Cause 3: AssertionError: unexpected refusal message: 'Source image <account>.dkr.ecr.<region>.amazonaws.com/<repo>:latest does not exist. Provide a valid source image.'
Cause 4: AssertionError: the call was expected to be refused but it SUCCEEDED — an unsigned layer was accepted under an Enforce code-signing policy.
Cause 5: AssertionError: expected the invoke to be refused, but it succeeded: HTTPStatusCode 200, StatusCode 200, ExecutedVersion '$LATEST'.
Cause 6: AssertionError: StartingPosition='EARLIEST' was refused with ('ValidationException', "...Member must satisfy enum value set: [TRIM_HORIZON, LATEST, AT_TIMESTAMP]") (expected [LATEST, AT_TIMESTAMP, TRIM_HORIZON]); AssertionError: one topic was refused with ('InvalidParameterValueException', 'The function execution role does not have permissions to call DescribeClusterV2 on MSK') (expected 'Cannot access cluster <arn>. Please ensure the role can perform the DescribeClusterV2 (recommended) or DescribeCluster and GetBootstrapBrokers actions on your cluster in IAM.').
Cause 7: KeyError: 'VpcId'.
Cause 8: AssertionError: the handler could not tag its own function within 90s: {'errorMessage': '<request id> Task timed out after 8.00 seconds', 'errorType': 'Sandbox.Timedout'}.
(byte counts, ARNs, image URIs, request ids and function names are per-run values; the templates and status/code pairs are the requirements.)

## Steps to Reproduce
Cause 1: GetAccountSettings to read CodeSizeZipped, then CreateFunction with a ZipFile larger than it.
Cause 2: CreateFunction with EphemeralStorage={'Size': <value outside 512..10240>}.
Cause 3: CreateFunction with PackageType='Image', Code={'ImageUri': ...} and CodeSigningConfigArn set.
Cause 4: Attach an Enforce code-signing config to a function, then UpdateFunctionConfiguration adding an unsigned layer version.
Cause 5: CreateFunction with TenancyConfig requesting tenant isolation, then Invoke without a tenant id.
Cause 6: CreateEventSourceMapping against an MSK cluster ARN with StartingPosition='EARLIEST'; and again with a role lacking kafka:DescribeClusterV2.
Cause 7: CreateFunction with VpcConfig and read GetFunctionConfiguration's VpcConfig while the attach is pending.
Cause 8: Invoke a handler that calls lambda:TagResource on its own function with a generous Timeout.
