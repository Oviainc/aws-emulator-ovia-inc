Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected lambda.deep-lambda-3-url-cors-not-enforced-server-side lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler lambda.dead-letter-config-roundtrip lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected lambda.create-function-code-validation-errors lambda.blueprint-tag-create-function-system-tag-rejected lambda.deep-lambda-runtime-truth-reserved-env-var-at-create-rejected apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative cloudformation__iam__lambda__s3.iam-role-trust-policy-missing-lambda-principal-negative cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative lambda.account-settings-limits-drive-enforcement lambda.blueprint-tag-tag-resource-empty-tag-map-rejected lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected lambda.blueprint-tag-tag-resource-system-key-rejected lambda.add-layer-permission-policy-length-exceeded lambda.capacity-provider-invalid-params-rejected lambda.delete-then-not-found lambda.create-duplicate lambda.capacity-provider-name-unicode-rejected
Issue-Key: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied|run#10|AssertionError|assert status == 200 and json.loads(body).get("nonce") == "granted", (
Issue-Key: lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected|run#10|AssertionError|assert status == 200 and json.loads(body).get("marker") == MARKER, (
Issue-Key: lambda.deep-lambda-3-url-cors-not-enforced-server-side|run#10|AssertionError|assert status == 200 and json.loads(body).get("marker") == MARKER, (
Issue-Key: lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler|run#12|AssertionError|assert status == 200, f"preflight must be answered 200, got {status!r} {hdrs!r} {body[:200]!r}"
Issue-Key: lambda.dead-letter-config-roundtrip|run#13|AssertionError|assert actual_arn == q_arn, (
Issue-Key: lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update|_scenario#9|KeyError|assert created["VpcConfig"]["VpcId"] == vpc_id, (
Issue-Key: lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected|run#7|KeyError|node_arn = lam.get_function_configuration(FunctionName=node_fn)["RuntimeVersionConfig"][
Issue-Key: lambda.create-function-code-validation-errors|_error_code#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: lambda.blueprint-tag-create-function-system-tag-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.deep-lambda-runtime-truth-reserved-env-var-at-create-rejected|_error_code#7|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED; a ClientError was expected")
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative|_retry_negative_create#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.iam-role-trust-policy-missing-lambda-principal-negative|_expect_create_function_rejected#4|AssertionError|raise AssertionError(f"CreateFunction succeeded for {name!r} with a role Lambda cannot assume")
Issue-Key: cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative|run#5|AssertionError|assert proc.returncode != 0, "create-function unexpectedly succeeded with a nonexistent role ARN"
Issue-Key: lambda.account-settings-limits-drive-enforcement|run#13|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: lambda.blueprint-tag-tag-resource-empty-tag-map-rejected|run#5|AssertionError|assert code == "InvalidParameterValueException", f"an empty Tags map must be refused; got {code!r}"
Issue-Key: lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected|run#5|AssertionError|assert code == "ValidationException", f"a key with `!` must be refused; got {code!r}"
Issue-Key: lambda.blueprint-tag-tag-resource-system-key-rejected|run#5|AssertionError|assert code == "InvalidParameterValueException", f"an aws: key must be refused; got {code!r}"
Issue-Key: lambda.add-layer-permission-policy-length-exceeded|run#14|AssertionError|assert error_code == "PolicyLengthExceededException", (
Issue-Key: lambda.capacity-provider-invalid-params-rejected|run#9|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Key: lambda.delete-then-not-found|run#9|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Key: lambda.create-duplicate|run#20|AssertionError|assert dead_mutation_code == "ResourceNotFoundException", (
Issue-Key: lambda.capacity-provider-name-unicode-rejected|run#10|AssertionError|assert message == expected_create_message, (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Lambda: Function URLs unrouted, configuration blocks dropped from reads, CreateFunction skips validation, and limit/tag errors answer NotImplemented or bare 413

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Seven distinct root causes in the Lambda emulation, all grounded in the failing checks listed per cause.

Cause 1 of 7 - Function URL endpoints are not routed.
AWS: CreateFunctionUrlConfig returns a FunctionUrl of the form https://{url-id}.lambda-url.{region}.on.aws/. A GET to it with AuthType NONE (or a signed request with AuthType AWS_IAM) is answered by the handler; a CORS preflight OPTIONS is answered by Lambda itself with the configured Access-Control-* headers and HTTP 200; a caller without lambda:InvokeFunctionUrl gets 403. Smithy: lambda FunctionUrlConfig/Cors are modeled; the URL data plane is not a modeled operation, so these statuses are recorded behavior.
Emulator: every request to the function URL is answered HTTP 404 with x-amzn-errortype: UnknownService and body {"__type":"UnknownService","message":"no emulated service is registered for '<url-id>'"} - the front door tries to resolve the 32-hex URL id as a service name. The emulator minted a URL it does not serve.
Why: Function URLs are entirely unusable, and the auth and CORS legs that depend on them cannot be distinguished from a routing failure.
Fix: front-door host resolution for *.lambda-url.*.on.aws, dispatching to the function-URL handler (auth check, CORS preflight, then invoke).
Checks: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied, lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected, lambda.deep-lambda-3-url-cors-not-enforced-server-side, lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler.

Cause 2 of 7 - GetFunctionConfiguration drops configuration blocks the function was created with.
AWS: Smithy: GetFunctionConfiguration output shape is FunctionConfiguration and carries VpcConfig (VpcConfigResponse), DeadLetterConfig, RuntimeVersionConfig and LoggingConfig; CreateFunction accepts VpcConfig, DeadLetterConfig and LoggingConfig as input. An independent read must echo what create set, and RuntimeVersionConfig/LoggingConfig are populated by the service even when not supplied.
Emulator: the read omits them - the checks fail with KeyError: 'VpcConfig', KeyError: 'RuntimeVersionConfig', KeyError: 'LoggingConfig', and "expected GetFunctionConfiguration to report DeadLetterConfig.TargetArn=... (set at CreateFunction), got None from an independent read"; one check reports VpcConfig present but empty ({}).
Why: a client cannot read back the networking, DLQ, logging or runtime-pin configuration it just set, so configuration round-trips and any code branching on those members break.
Fix: the CreateFunction/UpdateFunctionConfiguration state write and the FunctionConfiguration serializer shared by GetFunctionConfiguration/GetFunction.
Checks: lambda.dead-letter-config-roundtrip, lambda.deep-lambda-1-fn-state-vpc-create-pending-rejects-invoke-and-update, lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected.

Cause 3 of 7 - CreateFunction accepts input AWS rejects.
AWS: Smithy: CreateFunction errors include InvalidParameterValueException (400) and ResourceNotFoundException (404). AWS rejects a role that does not exist, a role whose trust policy does not allow lambda.amazonaws.com to assume it (InvalidParameterValueException, "The role defined for the function cannot be assumed by Lambda"), a reserved AWS_* environment-variable key, a system tag key (aws:...), and a malformed/undeployable Code block.
Emulator: all of these SUCCEED with HTTP 201 and a full FunctionConfiguration (State=Pending). Replay: "expected the call to be refused, but it SUCCEEDED and returned {... 'State': 'Pending' ...}", "create-function unexpectedly succeeded with a nonexistent role ARN", "CreateFunction succeeded with role ... whose trust policy allows only ec2.amazonaws.com to assume it", "the call unexpectedly SUCCEEDED; a ClientError was expected".
Why: negative tests for IAM wiring pass against the emulator and then fail on real AWS; a broken function appears created and healthy.
Fix: CreateFunction input validation (role existence + trust-policy principal check, reserved env-var keys, tag keys, Code validation) in the CreateFunction handler.
Checks: lambda.create-function-code-validation-errors, lambda.blueprint-tag-create-function-system-tag-rejected, lambda.deep-lambda-runtime-truth-reserved-env-var-at-create-rejected, apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative, cloudformation__iam__lambda__s3.iam-role-trust-policy-missing-lambda-principal-negative, cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative.

Cause 4 of 7 - an oversized deployment package is refused with a bare HTTP 413 instead of a modeled error.
AWS: Smithy: CreateFunction's modeled errors include CodeStorageExceededException (400) and InvalidParameterValueException (400); a zipped package over the account's CodeSizeZipped limit is refused with RequestEntityTooLargeException (HTTP 413) carrying an x-amzn-ErrorType so the SDK raises a typed exception.
Emulator: the response carries no error code at all - the check only sees '413', so botocore cannot classify it. Replay: "expected CreateFunction to reject a 58720514-byte deployment package, which exceeds the account's reported CodeSizeZipped limit of 52428800 bytes, with RequestEntityTooLargeException, got '413'".
Why: SDK clients catch typed exceptions; an untyped 413 surfaces as a generic ClientError with no error code, so retry/branching logic misfires.
Fix: the CreateFunction size-limit rejection path - emit the modeled error shape and x-amzn-ErrorType with the 413.
Checks: lambda.account-settings-limits-drive-enforcement.

Cause 5 of 7 - validation that AWS performs before any work answers NotImplemented instead of the modeled error.
AWS: Smithy: TagResource errors are InvalidParameterValueException (400), ResourceConflictException, ResourceNotFoundException, ServiceException, TooManyRequestsException; AddLayerVersionPermission additionally models PolicyLengthExceededException (400); CreateCapacityProvider validation failures are InvalidParameterValueException (400). An empty Tags map, a key containing '!', and an aws:-prefixed key are all rejected 400 before the resource is touched.
Emulator: these requests are answered NotImplemented (HTTP 501). Replay: "an empty Tags map must be refused; got 'NotImplemented'", "a key with `!` must be refused; got 'NotImplemented'", "an aws: key must be refused; got 'NotImplemented'", "expected PolicyLengthExceededException after 0 statements, got 'NotImplemented'", "expected InvalidParameterValueException, got 'NotImplemented'". These share a cause with the already-filed scaffold-stub issues for lambda TagResource and AddLayerVersionPermission, but the fidelity requirement here is the error classification, not merely that the operation exists: input validation must run and return the modeled 400 even where the happy path is still a stub.
Why: a client cannot tell a rejected request from an unimplemented service; NotImplemented is a 501 server error, so SDK retry policy treats a permanent 400 as retryable.
Fix: input validation in the TagResource, AddLayerVersionPermission and CreateCapacityProvider handlers, ahead of the unimplemented path.
Checks: lambda.blueprint-tag-tag-resource-empty-tag-map-rejected, lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected, lambda.blueprint-tag-tag-resource-system-key-rejected, lambda.add-layer-permission-policy-length-exceeded, lambda.capacity-provider-invalid-params-rejected.

Cause 6 of 7 - UpdateFunctionConfiguration against a deleted function answers NotImplemented instead of ResourceNotFoundException.
AWS: Smithy: UpdateFunctionConfiguration models ResourceNotFoundException (404) and that is what a name that no longer exists returns.
Emulator: NotImplemented (501). Replay: "UpdateFunctionConfiguration on the deleted name 'sim2real-behav-fn-...' returned 'NotImplemented', expected ResourceNotFoundException" and "UpdateFunctionConfiguration on deleted function: expected ResourceNotFoundException, got NotImplemented".
Why: deletion cannot be confirmed through the configuration API, and a 501 is retried where a 404 is final.
Fix: resolve the function in state and return the modeled 404 in the UpdateFunctionConfiguration handler before reaching the unimplemented body.
Checks: lambda.delete-then-not-found, lambda.create-duplicate.

Cause 7 of 7 - a validation message re-renders the modeled pattern with added anchors.
AWS: the "1 validation error detected: ... failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" message is built from the request and quotes the member's modeled @pattern verbatim, so the rule producing the text is known. For capacityProviderName the modeled pattern has no ^...$ anchors.
Emulator: the message quotes "^(arn:aws[a-zA-Z-]*:lambda:...capacity-provider:[a-zA-Z0-9-_]+)|[a-zA-Z0-9-_]+$" - a '^(' prefix and '$' suffix the model does not have, which also changes the alternation's meaning.
Why: clients and operators compare the quoted pattern against the published model; an altered pattern misdirects the fix.
Fix: the validation-message builder - emit the @pattern string as modeled, without wrapping anchors.
Checks: lambda.capacity-provider-name-unicode-rejected.

## Evidence
Cause 1: expected HTTP 200 from the function URL (and 200 for a CORS preflight); actual HTTP 404, x-amzn-errortype: UnknownService, body {"__type":"UnknownService","message":"no emulated service is registered for '10c982836e16481d9298e4c626464dda'"}.
Cause 2: expected GetFunctionConfiguration to carry VpcConfig / DeadLetterConfig / RuntimeVersionConfig / LoggingConfig; actual the members are absent (KeyError: 'VpcConfig', KeyError: 'RuntimeVersionConfig', KeyError: 'LoggingConfig') or empty ({}), and DeadLetterConfig reads back as None.
Cause 3: expected HTTP 400 InvalidParameterValueException (or 404 ResourceNotFoundException for a missing role); actual HTTP 201 with a complete FunctionConfiguration.
Cause 4: expected HTTP 413 RequestEntityTooLargeException; actual HTTP 413 with no error code ('413').
Cause 5: expected HTTP 400 InvalidParameterValueException / PolicyLengthExceededException; actual HTTP 501 NotImplemented.
Cause 6: expected HTTP 404 ResourceNotFoundException; actual HTTP 501 NotImplemented.
Cause 7: expected the modeled pattern "(arn:aws[a-zA-Z-]*:lambda:...)|[a-zA-Z0-9-_]+"; actual "^(arn:aws[a-zA-Z-]*:lambda:...)|[a-zA-Z0-9-_]+$".

## Steps to Reproduce
Cause 1: CreateFunction, CreateFunctionUrlConfig(AuthType=NONE), then GET the returned FunctionUrl -> 404 UnknownService.
Cause 2: CreateFunction with VpcConfig, DeadLetterConfig and LoggingConfig set, then GetFunctionConfiguration -> those members are missing from the response.
Cause 3: CreateFunction with (a) a role ARN that was never created, (b) a role trusting only ec2.amazonaws.com, (c) an environment variable named AWS_REGION, (d) a tag key 'aws:foo' -> each returns 201.
Cause 4: CreateFunction with a zip larger than the account CodeSizeZipped limit -> 413 with no error code.
Cause 5: TagResource with Tags={} / a key containing '!' / a key 'aws:x'; AddLayerVersionPermission with an oversized policy; CreateCapacityProvider with invalid params -> 501 NotImplemented.
Cause 6: CreateFunction, DeleteFunction, then UpdateFunctionConfiguration on the same name -> 501 NotImplemented.
Cause 7: CreateCapacityProvider with a non-ASCII name -> validation message quotes an anchored pattern.
