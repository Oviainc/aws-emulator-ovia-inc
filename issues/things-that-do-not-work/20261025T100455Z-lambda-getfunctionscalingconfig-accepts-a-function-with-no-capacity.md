Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.get-function-scaling-config-invalid-qualifier-rejected lambda.get-function-scaling-config-nonexistent-function-rejected lambda.get-function-scaling-config-default-when-unset lambda.outbound-https-fetches-s3-object-via-virtual-hosted-presigned-url
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.get-function-scaling-config-invalid-qualifier-rejected|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: lambda.get-function-scaling-config-nonexistent-function-rejected|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: lambda.get-function-scaling-config-default-when-unset|_poll_scaling_config#11|AssertionError|raise AssertionError(
Issue-Key: lambda.outbound-https-fetches-s3-object-via-virtual-hosted-presigned-url|run#14|AssertionError|assert host is not None and host.startswith(bucket), (
Issue-Commit: ee3608f46372fd0499ccb380262b1cbf197ec697

# lambda: GetFunctionScalingConfig accepts a function with no capacity-provider configuration and invents no service-managed default, and the emulator's S3 endpoint offers no virtual-hosted-style address (3 causes)

> _Note: this issue was identified against the code base at commit `ee3608f46372fd0499ccb380262b1cbf197ec697`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionScalingConfig

## Description
Three net-new root causes in the `lambda` service, all grounded in failing behavioral checks of this suite. None of these checks is marked ALREADY FILED or OWNED in the replay report; the open lambda issues listed below cover other operations and do not carry these replay keys.

=== Cause 1 of 3: GetFunctionScalingConfig answers 200 for a function that has no capacity-provider configuration, where AWS refuses the call ===
Failing checks: lambda.get-function-scaling-config-invalid-qualifier-rejected, lambda.get-function-scaling-config-nonexistent-function-rejected (both fail on this same positive-control call, before their negative leg is ever reached).

AWS: GetFunctionScalingConfig is modeled as GET /2025-11-30/functions/{FunctionName}/function-scaling-config with required FunctionName and required Qualifier (PublishedFunctionQualifier, len 1..128, pattern `^(\$LATEST\.PUBLISHED|[0-9]+)$`), output members FunctionArn / AppliedFunctionScalingConfig / RequestedFunctionScalingConfig, modeled errors InvalidParameterValueException (400), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). The operation is documented as retrieving the scaling configuration of a *Lambda Managed Instances* function. Recorded behavior (this check's oracle): a plain Zip function with NO CapacityProviderConfig, published to version 1, is NOT answered with a 200 — the call is REFUSED with error code `AccessDeniedException` and the message "The function provided by the arn does not contain a capacity provider configuration" (that message is built from the request's own ARN resolution and is asserted verbatim by two independent checks, so the code is the requirement and that message is the recorded form of it). The code is not in the operation's modeled error list — recorded behavior only.

Emulator: the call succeeds with HTTP 200 and a body containing only FunctionArn, e.g. `{"FunctionArn": "arn:aws:lambda:us-west-2:000000000000:function:sim2real-lambda-fn-27242f9932d4:1"}` (content-length 98/101, no scaling members at all). The handler deliberately treats "qualifier exists but holds no configuration" as a success case and returns an empty payload.

Why: a client cannot distinguish "this function is not a Managed Instances function" from "this function has scaling bounds". Code written against AWS catches AccessDeniedException to detect an unconfigured function; against the emulator it instead receives a 200 whose scaling members are absent, so it either crashes on the missing key or silently concludes the function has no bounds. It also destroys the three-way error taxonomy the two rejection checks exist to prove (ResourceNotFoundException for an unknown function, ValidationException for a malformed qualifier, AccessDeniedException for a resolvable-but-unconfigured one): because the positive control now succeeds, neither negative leg is even exercised, so the emulator's behavior for a malformed Qualifier and for a never-created function remains ungraded by this suite until this cause is fixed.

Fix: the GetFunctionScalingConfig handler's resolution/authorization step — after the function and published qualifier resolve, a function whose record carries no capacity-provider configuration must be refused with the recorded AccessDeniedException instead of returning an empty 200.

Traces: lambda.get-function-scaling-config-invalid-qualifier-rejected, lambda.get-function-scaling-config-nonexistent-function-rejected. Handler: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/get_function_scaling_config.rs lines 182-200 (the `None => Ok(GetFunctionScalingConfigOutput { .. })` arm, and the module doc at lines 12-17 that states the empty answer is intentional).

=== Cause 2 of 3: a Managed Instances function version that was never PutFunctionScalingConfig'd reports no RequestedFunctionScalingConfig, where AWS reports the service-managed default ===
Failing check: lambda.get-function-scaling-config-default-when-unset.

AWS: Smithy FunctionScalingConfig has members MinExecutionEnvironments and MaxExecutionEnvironments (integer, range 0..15000); GetFunctionScalingConfig's output carries both RequestedFunctionScalingConfig and AppliedFunctionScalingConfig. Recorded behavior: for a function created with CapacityProviderConfig.LambdaManagedInstancesCapacityProviderConfig and published to a version, GetFunctionScalingConfig returns a real, service-managed RequestedFunctionScalingConfig the moment the function exists even though PutFunctionScalingConfig was never called — MinExecutionEnvironments = 3 and NO MaxExecutionEnvironments member. (AppliedFunctionScalingConfig legitimately stays absent until the capacity provider has provisioned environments; the check does not assert it.)

Emulator: HTTP 200 with keys `['FunctionArn']` only — RequestedFunctionScalingConfig is absent, so the check's bounded poll never sees it and fails with "GetFunctionScalingConfig response missing RequestedFunctionScalingConfig".

Why: a client that reads the requested bounds of a Managed Instances function to display or reconcile them (a CDK/Terraform-style diff, an autoscaling reconciler) sees no bounds at all and concludes nothing is configured, then writes bounds that differ from the service-managed default it should have been diffing against.

Fix: the GetFunctionScalingConfig handler together with the function record's scaling state — a function whose record carries a Managed Instances capacity-provider configuration must report the service-managed default requested configuration (minimum 3 execution environments, no maximum) when no explicit PutFunctionScalingConfig record exists, rather than omitting the member.

Traces: lambda.get-function-scaling-config-default-when-unset. Handler: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/get_function_scaling_config.rs lines 193-200.

=== Cause 3 of 3: the emulator's S3 endpoint has no virtual-hosted-style address, so a bucket hostname can never be formed ===
Failing check: lambda.outbound-https-fetches-s3-object-via-virtual-hosted-presigned-url.

AWS: S3 serves buckets under virtual-hosted-style addressing — `<bucket>.s3.<region>.amazonaws.com` — and that is the addressing a client configured with `s3={"addressing_style": "virtual"}` uses for both presigned URLs and live requests (path-style is the legacy form). Recorded behavior only for the exact hostname shape; the requirement is that the S3 endpoint the emulator advertises be a DNS-addressable host under which a bucket-prefixed hostname exists and is routed to the bucket.

Emulator: the advertised S3 endpoint is an IP literal, so no bucket-prefixed hostname can exist; a presigned GET generated with virtual addressing comes back path-style with the bucket in the path — `https://127.0.0.1:39241/sim2real-behav-lambdadns-554a595a5d75/payload.txt?AWSAccessKeyId=EMULATOR…&Signature=…&Expires=…` — and the check fails at setup because `urlparse(url).hostname` is the bare address rather than a hostname starting with the bucket name. Hypothesis: the S3 front end has no bucket-in-host routing, and the endpoint it announces is an address rather than a name, so the SDK cannot use virtual addressing at all. (The specific host, port and bucket name above are environment-specific examples, not requirements; the requirement is that a virtual-hosted-style address exists and resolves to the bucket.)

Why: every client and tool that defaults to or requires virtual-hosted-style S3 addressing — presigned URLs handed to a browser, SDKs with `addressing_style=virtual`, code built against AWS's documented hostname form, and anything that parses the bucket out of the URL host — cannot address the emulator's buckets at all, and a handler running inside the emulator can never exercise the hostname form real traffic uses.

Fix: the S3 front end's request routing plus the endpoint the emulator advertises — bucket-in-host requests must be routed to the named bucket, and the announced S3 endpoint must be a name under which `<bucket>.<endpoint>` is reachable.

Traces: lambda.outbound-https-fetches-s3-object-via-virtual-hosted-presigned-url (fails in the check's own setup at the virtual-hosted-URL assertion, before Invoke is reached, so it is independent of the already-filed Invoke NotImplemented issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md).

## Evidence
Cause 1 — expected (AWS, recorded): GetFunctionScalingConfig(FunctionName=<plain Zip fn>, Qualifier="1") is refused with Error.Code="AccessDeniedException", Error.Message="The function provided by the arn does not contain a capacity provider configuration". Actual (emulator): HTTP 200, body `{"FunctionArn":"arn:aws:lambda:us-west-2:000000000000:function:sim2real-lambda-fn-27242f9932d4:1"}`; the harness reports `AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'FunctionArn': '...:1'}` for BOTH lambda.get-function-scaling-config-invalid-qualifier-rejected (line 33) and lambda.get-function-scaling-config-nonexistent-function-rejected (line 32). Because that positive control now succeeds, the negative legs — Qualifier="not a valid qualifier!" expecting ValidationException with the modeled-pattern message, and a never-created FunctionName expecting ResourceNotFoundException "The resource you requested does not exist." — were never sent.

Cause 2 — expected (AWS, recorded): the response carries RequestedFunctionScalingConfig = {MinExecutionEnvironments: 3} with no MaxExecutionEnvironments. Actual (emulator): HTTP 200 with keys ['FunctionArn', 'ResponseMetadata'] only; `AssertionError: GetFunctionScalingConfig response missing RequestedFunctionScalingConfig, got: ['FunctionArn', 'ResponseMetadata'] -> {... 'FunctionArn': 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-lambda-mi-fn-924147491f33:1'}` after the check's 180 s bounded poll. Every upstream call succeeded: ec2 vpc/subnet/security-group, iam roles, lambda:CreateCapacityProvider, lambda:CreateFunction with CapacityProviderConfig, lambda:PublishVersion returning version 1.

Cause 3 — expected (AWS): a presigned GET generated with addressing_style=virtual has a hostname beginning with the bucket name (`<bucket>.s3.<region>.amazonaws.com`). Actual (emulator): `https://127.0.0.1:39241/sim2real-behav-lambdadns-554a595a5d75/payload.txt?AWSAccessKeyId=EMULATORFC824D5793C0&Signature=…&Expires=…` — host `127.0.0.1`, bucket in the path; `AssertionError: test setup did not produce a virtual-hosted-style URL: host='127.0.0.1' for bucket 'sim2real-behav-lambdadns-554a595a5d75'`. CreateBucket and PutObject both succeeded first, so the bucket exists; only its addressing form diverges.

## Steps to Reproduce
Cause 1: (a) iam:CreateRole with the lambda.amazonaws.com trust policy and AWSLambdaBasicExecutionRole attached; (b) lambda:CreateFunction (python3.12, Code.ZipFile, no CapacityProviderConfig), wait for State=Active; (c) lambda:PublishVersion -> "1"; (d) lambda:GetFunctionScalingConfig(FunctionName=<fn>, Qualifier="1"). AWS refuses with AccessDeniedException "The function provided by the arn does not contain a capacity provider configuration"; the emulator answers HTTP 200 with only FunctionArn. Then (e) repeat with Qualifier="not a valid qualifier!" (AWS: ValidationException naming the `qualifier` pattern) and (f) with a FunctionName that was never created (AWS: ResourceNotFoundException "The resource you requested does not exist.") — both legs only become observable once (d) is refused.

Cause 2: (a) ec2:CreateVpc/CreateSubnet/CreateSecurityGroup; (b) iam:CreateRole for the capacity-provider operator (AWSLambdaManagedEC2ResourceOperator) and a Lambda execution role; (c) lambda:CreateCapacityProvider with that VpcConfig, PermissionsConfig.CapacityProviderOperatorRoleArn and InstanceRequirements.Architectures=[x86_64]; (d) lambda:CreateFunction with CapacityProviderConfig.LambdaManagedInstancesCapacityProviderConfig.CapacityProviderArn=<cp arn>; (e) lambda:PublishVersion; (f) lambda:GetFunctionScalingConfig(FunctionName=<fn>, Qualifier=<version>) WITHOUT ever calling PutFunctionScalingConfig. AWS returns RequestedFunctionScalingConfig={MinExecutionEnvironments:3} with no MaxExecutionEnvironments; the emulator returns 200 with only FunctionArn.

Cause 3: (a) build an s3 client with Config(s3={"addressing_style": "virtual"}) against the emulator endpoint; (b) s3:CreateBucket (+ CreateBucketConfiguration.LocationConstraint outside us-east-1); (c) s3:PutObject key=payload.txt; (d) s3.generate_presigned_url("get_object", ExpiresIn=300) and inspect urlparse(url).hostname. AWS yields `<bucket>.s3.<region>.amazonaws.com`; the emulator yields the bare endpoint address with the bucket in the path, and no `<bucket>.<endpoint>` hostname exists to GET.
