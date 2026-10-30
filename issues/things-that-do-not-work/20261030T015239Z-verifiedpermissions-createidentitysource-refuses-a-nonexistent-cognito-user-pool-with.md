Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected|run#20|AssertionError|assert nonexistent == expected_nonexistent, (
Issue-Commit: 3721cdcd5d81ad7b59ab57035aad7fc30db6f74c

# CreateIdentitySource refuses a nonexistent Cognito user pool with the generic message "Invalid input" instead of "User pool <poolId> does not exist."

> _Note: this issue was identified against the code base at commit `3721cdcd5d81ad7b59ab57035aad7fc30db6f74c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentitySource

## Description
REOPENS 20261029T093530Z-verifiedpermissions-createidentitysource-validation.md: its fix left the call refused. That fixed issue added the missing existence lookup for configuration.cognitoUserPoolConfiguration.userPoolArn, so the call is now refused with the right status and the right error code — but the refusal it emits carries the emulator's own generic prose, so the replayed check still fails on the same operation and the same input. Also related: the still-open 20261029T184203Z-verifiedpermissions-createidentitysource-accepts-a-well-formed-cognitouserpoolconfiguration-userpoolarn-naming.md carries this same check id, but it is bound to the older replay key (the call SUCCEEDING, _error#18 "real AWS rejects this call, but it SUCCEEDED"); that acceptance half no longer reproduces, and the message defect described here is what remains.

Cause 1 of 1: the ValidationException raised for a well-formed userPoolArn naming a user pool that does not exist carries the message "Invalid input" instead of the Verified Permissions wording for that error class.

AWS: CreateIdentitySource resolves configuration.cognitoUserPoolConfiguration.userPoolArn in two independent stages. Stage 1 is the pure shape check against the member's own constraint (Smithy, com.amazonaws.verifiedpermissions#CognitoUserPoolConfiguration$userPoolArn: required, @length 1..255, @pattern ^arn:[a-zA-Z0-9-]+:cognito-idp:(([a-zA-Z0-9-]+:\\d{12}:userpool/[\\w-]+_[0-9a-zA-Z]+))$); a shape violation is refused with the standard constraint-violation message "1 validation error detected. Value at '/configuration/cognitoUserPoolConfiguration/userPoolArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the pattern>" — the emulator already gets this half right, which isolates the defect. Stage 2 is the EXISTENCE lookup of the pool the ARN names. Recorded behavior (the model fixes the error shape, not the text): HTTP 400, error code ValidationException (Smithy: CreateIdentitySource errors include ValidationException, httpError 400, httpErrorDefaulted; required member message), message "User pool <poolId> does not exist." In that message the FIXED text is "User pool " and " does not exist." including the trailing period; the single VARIABLE part is <poolId>, the user-pool id segment taken verbatim from the request's own userPoolArn (the replay's <region>_ffffffff9 is one filled-in value, not part of the template). The refusal leaves no identity source behind.

Emulator: the status (400) and the code (ValidationException) are correct, but the message is the constant string "Invalid input" — a generic classification that names neither the resource kind that was missing nor the pool id the caller got wrong, and that is indistinguishable from any other input refusal on this operation. Replay output: expected ('ValidationException', 'User pool us-west-2_ffffffff9 does not exist.'), got ('ValidationException', 'Invalid input').

Why: a client cannot tell from the emulator's refusal WHICH part of its identity-source wiring is wrong. AWS's message distinguishes "the ARN is syntactically wrong" (the constraint-violation message, which names the member path and the pattern) from "the ARN is well formed but the pool does not exist" (which names the pool id), and both from the other ValidationException classes this operation raises. Against the emulator all of the latter collapse into "Invalid input", so operators reading logs and any test or deployment script asserting on the documented sentence — or merely matching the pool id it echoes — pass against AWS and fail against the emulator, or vice versa. It also hides the actionable datum (the offending pool id) that the caller needs to repair the request.

Fix: the CreateIdentitySource handler's validation of the Cognito configuration — specifically the existence-lookup branch added for the reopened issue, and the shared identity-source configuration helper CreateIdentitySource and UpdateIdentitySource both use. That branch must construct the ValidationException with the resource-specific message formatted from the resolved pool id rather than returning the crate's generic "Invalid input" default. The lookup itself is already correct and must not change; only the message this one refusal path emits does.

Traces: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected (the positive control, the malformed-ARN negative and the effect-absent ListIdentitySources assertions in the same check all pass; the failure is on the nonexistent-pool message assertion, run.py line 80).

## Evidence
Check: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected (verdict: mismatch). Suite run: 28/29 passed; verifiedpermissions 22/23.

Request (after a positive control that created a source from a REAL cognito-idp pool, which succeeded, and after a malformed-ARN negative, which was refused correctly):
  CreateIdentitySource(policyStoreId=<store created with validationSettings.mode=OFF>,
                       configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<region>_ffffffff9"}})
  — that pool id was never created by cognito-idp.

Expected (AWS, recorded): HTTP 400, ("ValidationException", "User pool <poolId> does not exist.") where <poolId> is the user-pool id segment of the request's userPoolArn. In the replay environment that rendered as 'User pool us-west-2_ffffffff9 does not exist.'; the region/pool-id value is per-run and only the fixed text "User pool " / " does not exist." plus the request-derived pool id are the requirement.

Actual (emulator): HTTP 400, ('ValidationException', 'Invalid input').

Assertion from the replay:
  File "check_verifiedpermissions_create_identity_source_nonexistent_user_pool_rejected.py", line 80, in run
    assert nonexistent == expected_nonexistent
  AssertionError: expected ('ValidationException', 'User pool us-west-2_ffffffff9 does not exist.') for a nonexistent user pool ARN, got ('ValidationException', 'Invalid input')

Isolation: the sibling negative in the same check — userPoolArn "arn:aws:s3:::not-a-user-pool-<region>", whose SHAPE does not match the member pattern — is refused with the exact AWS constraint-violation message ("1 validation error detected. Value at '/configuration/cognitoUserPoolConfiguration/userPoolArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-zA-Z0-9-]+:cognito-idp:(([a-zA-Z0-9-]+:\\d{12}:userpool/[\\w-]+_[0-9a-zA-Z]+))$") and that assertion passes. The effect-absent assertion (ListIdentitySources returns only the positive control's identitySourceId) also passes, so the existence lookup both runs and leaves no state behind. The defect is confined to the message string of the existence refusal.

Smithy (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json): CreateIdentitySource input configuration (required, target Configuration); errors ValidationException (400, httpErrorDefaulted), ResourceNotFoundException (404), ConflictException (409), ServiceQuotaExceededException (402). ValidationException members: message (required), fieldList (optional). CognitoUserPoolConfiguration$userPoolArn: required, @length 1..255, @pattern as quoted above.

## Steps to Reproduce
1. CreatePolicyStore(validationSettings={"mode": "OFF"}) and note policyStoreId.
2. Positive control (must keep passing): cognito-idp CreateUserPool, then CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<realPoolId>"}}) -> 200 with an identitySourceId.
3. Shape negative (must keep passing): CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:s3:::not-a-user-pool-<region>"}}) -> 400 ValidationException with the "1 validation error detected. Value at '/configuration/cognitoUserPoolConfiguration/userPoolArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." message.
4. Existence negative (the failing one): CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:<region>:<account>:userpool/<region>_ffffffff9"}}) where that pool id was never created.
5. Read the error: the emulator answers 400 ValidationException with message "Invalid input". Expected 400 ValidationException with message "User pool <region>_ffffffff9 does not exist." — fixed text "User pool "/" does not exist.", variable part = the pool id taken from the request's userPoolArn.
6. ListIdentitySources(policyStoreId) must still return only the positive control's identitySourceId (it already does).
7. Replay: verifiedpermissions.create-identity-source-nonexistent-user-pool-rejected. It passes only when step 5's message matches as well as the status and code.
