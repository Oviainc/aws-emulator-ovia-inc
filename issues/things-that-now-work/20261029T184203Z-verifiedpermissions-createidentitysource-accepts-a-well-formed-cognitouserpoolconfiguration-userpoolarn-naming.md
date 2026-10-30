Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected|_error#18|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 1f4ddd4c47b0507bb1e50344cb9c3bb105203334

# CreateIdentitySource accepts a well-formed cognitoUserPoolConfiguration.userPoolArn naming a user pool that does not exist, instead of refusing it with ValidationException "User pool <poolId> does not exist."

> _Note: this issue was identified against the code base at commit `1f4ddd4c47b0507bb1e50344cb9c3bb105203334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentitySource

## Description
Related open issue: 20261029T093530Z-verifiedpermissions-createidentitysource-validation.md describes the same acceptance gap from the code side, but it is not bound to this replay key and it predicts the WRONG wire refusal (ResourceNotFoundException / HTTP 404). The recorded AWS behavior this check replays is a ValidationException, so the refusal this cause requires is stated here.

Cause 1 of 1: CreateIdentitySource does not verify that the Cognito user pool named by configuration.cognitoUserPoolConfiguration.userPoolArn exists.

AWS: CreateIdentitySource resolves cognitoUserPoolConfiguration.userPoolArn in two independent stages. The first is a pure shape check against the member's pattern (Smithy: CognitoUserPoolConfiguration$userPoolArn pattern ^arn:[a-zA-Z0-9-]+:cognito-idp:(([a-zA-Z0-9-]+:\d{12}:userpool/[\w-]+_[0-9a-zA-Z]+))$), which the emulator already performs correctly — that half of the check passes. The second is an EXISTENCE lookup of the pool the ARN names: a syntactically valid ARN whose pool id was never created is refused. Recorded behavior: HTTP 400, error code ValidationException, message "User pool <poolId> does not exist." The fixed text is "User pool " and " does not exist."; the only variable part is <poolId>, the user-pool id segment taken from the request's userPoolArn (the recording's us-east-1_ffffffff9 is one filled-in value, not part of the template). Smithy: CreateIdentitySource models ValidationException (400, httpErrorDefaulted) and ResourceNotFoundException (404); the recorded refusal for this class is ValidationException, not ResourceNotFoundException. Neither rejected call leaves an identity source behind — ListIdentitySources still shows only sources created from real pools.

Emulator: the call SUCCEEDS. The replay shows HTTP 200 with a minted identitySourceId (createdDate/lastUpdatedDate/policyStoreId/identitySourceId), i.e. the emulator stores a reference to a pool that does not exist and reports it as a live identity source. The check's _error() helper therefore raises instead of comparing codes: "real AWS rejects this call, but it SUCCEEDED and returned {...'identitySourceId': ...}".

Why: a client that relies on the control plane to validate its wiring gets a silently broken identity source. Creation looks successful, GetIdentitySource/ListIdentitySources report it as usable, and the failure surfaces only much later on the data plane — in this emulator as a 500 InternalServerException when a token whose issuer is that pool is presented — instead of the deterministic 400 ValidationException at create time. Any test or deployment script asserting that a typo'd or deleted pool id is rejected passes against the emulator and fails against AWS; worse, the error surface moves from a client fault to a server fault.

Fix: the CreateIdentitySource handler's validation of the Cognito configuration (and the shared identity-source configuration helper it and UpdateIdentitySource both use) must, after the ARN shape resolution succeeds, look the resolved pool id up in the cognito-idp user-pool registry and refuse an unknown pool with ValidationException and the message above. The crate already performs exactly that lookup on the token-verification path, so the dependency and the accessor needed are present; the control-plane store paths simply skip it.

Traces: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected (the malformed-ARN assertion earlier in the same check passes; the failure is on the nonexistent-pool assertion).

## Evidence
Check: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected (mismatch).

Request (after a positive control that created a source from a REAL pool, which succeeded):
  CreateIdentitySource(policyStoreId=<store>, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<region>_ffffffff9"}})
  — the pool id was never created by cognito-idp.

Expected (AWS, recorded): HTTP 400, ("ValidationException", "User pool <region>_ffffffff9 does not exist.") — fixed text "User pool "/" does not exist.", variable part = the pool id from the request's userPoolArn; and ListIdentitySources still returns only the positive control's identitySourceId.

Actual (emulator): HTTP 200 with a created source —
  {'createdDate': ..., 'identitySourceId': 'IS9U2CCCsiTSu74BBHdXEuyj', 'lastUpdatedDate': ..., 'policyStoreId': 'PSJPfe8fgQnwl8FAu3TKk6j5', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Assertion raised by the check's _error() helper:
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {...}

The sibling negative in the same check — a userPoolArn whose SHAPE does not match (arn:aws:s3:::not-a-user-pool-<region>) — is refused correctly with ValidationException "1 validation error detected. Value at '/configuration/cognitoUserPoolConfiguration/userPoolArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...", which isolates the defect to the missing EXISTENCE lookup rather than to ARN parsing.

Smithy (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json): CreateIdentitySource input configuration (required, target Configuration); errors include ValidationException (400, httpErrorDefaulted) and ResourceNotFoundException (404).

## Steps to Reproduce
1. CreatePolicyStore(validationSettings={"mode": "OFF"}) and note policyStoreId.
2. (Positive control, should keep working) cognito-idp CreateUserPool, then CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<realPoolId>"}}) -> 200 with an identitySourceId.
3. Call CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<region>_ffffffff9"}}) where that pool id was never created.
4. Observe HTTP 200 and a minted identitySourceId. Expected: HTTP 400 ValidationException with message "User pool <region>_ffffffff9 does not exist.".
5. ListIdentitySources(policyStoreId) now returns TWO sources; AWS returns only the positive control's source, because the rejected call leaves nothing behind.
6. Replay: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected.
