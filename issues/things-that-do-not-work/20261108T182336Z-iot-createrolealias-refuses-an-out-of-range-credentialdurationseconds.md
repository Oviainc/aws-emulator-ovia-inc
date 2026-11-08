Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.role-alias-credential-duration-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.role-alias-credential-duration-out-of-range-rejected|run#16|AssertionError|assert high_code == "InvalidRequestException", f"expected InvalidRequestException, got {high_code!r}"
Issue-Commit: cf75889d0041250f9d4c4a6a69f11963f546e0f7

# iot: CreateRoleAlias refuses an out-of-range credentialDurationSeconds with the non-AWS error code ValidationException instead of InvalidRequestException with AWS's 'N validation error(s) detected' message

> _Note: this issue was identified against the code base at commit `cf75889d0041250f9d4c4a6a69f11963f546e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoleAlias

## Description
One net-new root cause in the iot role-alias control plane, grounded in behavioral check iot.role-alias-credential-duration-out-of-range-rejected. The open bundle 20261108T104730Z-iot-role-alias-plane-3-causes-deleterolealias-leaves.md describes other role-alias defects (duplicate-name wording, DescribeRoleAlias not-found wording, non-durable delete) and does NOT carry this range-refusal step, so it does not cover this cause.

Cause 1 of 1: an out-of-bound credentialDurationSeconds on CreateRoleAlias is refused with the wrong error class (and therefore the wrong message).

AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json) CreateRoleAlias POST /role-aliases/{roleAlias}; member credentialDurationSeconds targets CredentialDurationSeconds with @range min 900, max 43200. The error set modeled for CreateRoleAlias is InternalFailureException(500), InvalidRequestException(400), LimitExceededException(410), ResourceAlreadyExistsException(409), ServiceUnavailableException(503), ThrottlingException(400), UnauthorizedException(401) — there is no ValidationException in this service's error set at all. AWS refuses a value outside that range with HTTP 400 InvalidRequestException whose message is the standard AWS constraint sentence: fixed text "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>" for a too-high value and "... Member must have value greater than or equal to <min>" for a too-low one. The variable parts are the error count, the member name taken from the request ('credentialDurationSeconds') and the bound taken from the model (43200 / 900); a refused alias is never created, so a following DescribeRoleAlias must answer 404 ResourceNotFoundException with 'Role alias <roleAlias> not found' (roleAlias from the request URI label).

Emulator: the in-range positive control (credentialDurationSeconds=3600) is accepted and read back correctly, but a request carrying credentialDurationSeconds above the modeled maximum is refused with error code 'ValidationException' — an error class iot does not model and AWS never emits for this operation — rather than InvalidRequestException, so the AWS constraint message is not produced either. The refusal is being produced by the generic/centralized modeled-constraint rejection path (the Scaffolded::validation-style 400 ValidationException) instead of this service's modeled refusal. The same path governs the below-minimum request (899), which the check sends with botocore parameter validation disabled.

Why: a client that catches iot's modeled InvalidRequestException to handle bad credential durations (SDK error-type dispatch, retry/classification logic, CloudFormation-style error surfacing) never matches the emulator's ValidationException, so the refusal is seen as an unknown/unmodeled failure; code that parses AWS's 'validation error detected ... Member must have value less than or equal to 43200' sentence to report which member and bound was violated gets no such text.

Fix: iot's CreateRoleAlias request validation — refuse an out-of-range credentialDurationSeconds (both bounds) in the operation's own modeled error channel as InvalidRequestException with AWS's constraint-violation message template, instead of letting the shared ValidationException constraint path answer. The same treatment applies to UpdateRoleAlias, which models the identical @range 900..43200 member and the same InvalidRequestException.

Traces: iot.role-alias-credential-duration-out-of-range-rejected (suite simparity-behavioral-aws-v1, service heading 'iot').

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (33/272 checks passed; bootstrap_signal false, emulator serving).

[mismatch] iot.role-alias-credential-duration-out-of-range-rejected:
  File "check_iot_role_alias_credential_duration_out_of_range_rejected.py", line 65, in run
    assert high_code == "InvalidRequestException", f"expected InvalidRequestException, got {high_code!r}"
  AssertionError: expected InvalidRequestException, got 'ValidationException'

Expected (AWS) for CreateRoleAlias(roleAlias=<name>, roleArn=<role>, credentialDurationSeconds=46800):
  HTTP 400, Error.Code = InvalidRequestException
  Error.Message = "1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200"
  (fixed text: "1 validation error detected: Value at '" / "' failed to satisfy constraint: Member must have value less than or equal to "; variable parts: the member name 'credentialDurationSeconds' from the request and the bound 43200 from the model)
Actual (emulator): HTTP 400 with Error.Code = 'ValidationException'.

The same check then asserts, for credentialDurationSeconds=899 sent with botocore parameter validation disabled, InvalidRequestException with "1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value greater than or equal to 900", and that neither refused alias exists afterwards (DescribeRoleAlias -> 404 ResourceNotFoundException, message 'Role alias <name> not found'); those assertions are not reached because the first refusal already diverges.

Model evidence: smithy_query(operation="CreateRoleAlias") -> input.credentialDurationSeconds {target: CredentialDurationSeconds, range: {min: 900, max: 43200}}; errors: InternalFailureException/InvalidRequestException/LimitExceededException/ResourceAlreadyExistsException/ServiceUnavailableException/ThrottlingException/UnauthorizedException (no ValidationException). UpdateRoleAlias carries the same member and range.

## Steps to Reproduce
1. git_clone() and boot the emulator built from this checkout.
2. iam.create_role(...) a role trusted by credentials.iot.amazonaws.com and wait for it to exist.
3. Positive control: iot.create_role_alias(roleAlias=<a>, roleArn=<role>, credentialDurationSeconds=3600) — succeeds; describe_role_alias reports credentialDurationSeconds 3600.
4. iot.create_role_alias(roleAlias=<b>, roleArn=<role>, credentialDurationSeconds=46800) (above the modeled maximum 43200). Expect HTTP 400 InvalidRequestException with "1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200"; the emulator answers 400 with error code 'ValidationException'.
5. With a client built using Config(parameter_validation=False), iot.create_role_alias(roleAlias=<c>, roleArn=<role>, credentialDurationSeconds=899). Expect HTTP 400 InvalidRequestException with "... Member must have value greater than or equal to 900".
6. iot.describe_role_alias(roleAlias=<b>) and (roleAlias=<c>): expect 404 ResourceNotFoundException with 'Role alias <name> not found' for each, confirming no refused alias was created.
7. Re-replay behavioral check iot.role-alias-credential-duration-out-of-range-rejected to confirm the fix.
